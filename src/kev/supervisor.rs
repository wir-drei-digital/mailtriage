//! `mailtriage kev run` (hidden): the supervisor `ensure_running` and setup
//! start. It holds `server.lock`, runs Kev with `uv`, stops it after the
//! idle time or on TERM, and exits when Kev exits, logging why.
use super::{control, Layout};
use crate::process;
use anyhow::Result;
use fs2::FileExt;
use serde_json::{json, Value};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

/// `ETXTBSY`: another thread forked while a write handle to `uv` was open;
/// starting it is retried briefly, as `update::platform` does.
const TEXT_FILE_BUSY: i32 = 26;

/// `kev run`'s flags.
#[derive(Debug, Clone)]
pub struct RunArgs {
    pub dir: PathBuf,
    pub port: u16,
    pub checkpoint: String,
    pub uv: PathBuf,
    /// Setup's first start: the weights may be downloaded.
    pub allow_download: bool,
    pub idle: Duration,
    pub check: Duration,
    pub grace: Duration,
}

fn note(log: &mut File, text: &str) {
    let _ = writeln!(log, "{} {text}", chrono::Utc::now().to_rfc3339());
}

pub fn run(args: &RunArgs) -> Result<Value> {
    let layout = Layout::new(args.dir.clone());
    std::fs::create_dir_all(layout.root())?;
    let mut lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(layout.lock())?;
    match lock.try_lock_exclusive() {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
            return Ok(json!({"schema_version": 1, "kev": {"action": "already_running"}}))
        }
        Err(e) => return Err(e.into()),
    }
    control::confirm_terminated(&layout, &lock)?;
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(layout.log())?;
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        ctrlc::set_handler(move || stop.store(true, Ordering::SeqCst))?;
    }
    note(
        &mut log,
        &format!(
            "starting Kev: --run {} on 127.0.0.1:{}{}",
            args.checkpoint,
            args.port,
            if args.allow_download {
                " (downloads allowed)"
            } else {
                ""
            }
        ),
    );
    let port = args.port.to_string();
    let mut command = Command::new(&args.uv);
    command
        .args([
            "run",
            "--frozen",
            "--extra",
            "serve",
            "python",
            "-m",
            "kev.serve",
        ])
        .args([
            "--run",
            &args.checkpoint,
            "--host",
            "127.0.0.1",
            "--port",
            &port,
        ])
        .current_dir(layout.src())
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log.try_clone()?);
    for (name, value) in layout.uv_env() {
        command.env(name, value);
    }
    command.env_remove("KEV_API_KEY");
    if args.allow_download {
        command
            .env_remove("HF_HUB_OFFLINE")
            .env_remove("UV_OFFLINE")
            .env_remove("UV_NO_SYNC");
    } else {
        command.env("HF_HUB_OFFLINE", "1").env("UV_OFFLINE", "1");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut ownership = control::Ownership::begin(&layout)?;
    lock.set_len(0)?;
    write!(lock, "{}", std::process::id())?;
    lock.flush()?;
    let mut child = match spawn(&mut command) {
        Ok(child) => child,
        Err(e) => {
            note(&mut log, &format!("Kev could not start: {e}"));
            ownership.finish(&layout)?;
            return Err(e.into());
        }
    };
    if let Err(e) = ownership.child(&layout, child.id()) {
        process::terminate(&mut child);
        return Err(e.into());
    }
    layout.touch_last_used();
    // Setup owns this start until its deadline and probe finish. A bounded
    // fallback still ends an orphaned first-download supervisor. The probe
    // margin covers the maximum allowed provider timeout (300 seconds).
    let idle = if args.allow_download {
        args.idle
            .max(super::download_timeout() + Duration::from_secs(300) + args.grace)
    } else {
        args.idle
    };
    let mut checked = Instant::now();
    let action = loop {
        if let Some(status) = child.try_wait()? {
            end(&mut child, args.grace)?;
            note(&mut log, &format!("Kev exited ({status})"));
            break "exited";
        }
        if stop.load(Ordering::SeqCst) || control::requested(&layout)? {
            note(&mut log, "stopping Kev on request");
            let status = end(&mut child, args.grace)?;
            note(&mut log, &format!("Kev exited ({status})"));
            break "stopped";
        }
        if checked.elapsed() >= args.check {
            checked = Instant::now();
            if layout.idle_for() >= idle {
                note(
                    &mut log,
                    &format!(
                        "stopping Kev after {} seconds without a request",
                        idle.as_secs()
                    ),
                );
                let status = end(&mut child, args.grace)?;
                note(&mut log, &format!("Kev exited ({status})"));
                break "idle";
            }
        }
        thread::sleep(Duration::from_millis(200));
    };
    ownership.finish(&layout)?;
    Ok(json!({"schema_version": 1, "kev": {"action": action}}))
}

fn spawn(command: &mut Command) -> std::io::Result<Child> {
    let mut attempt = 0;
    loop {
        match command.spawn() {
            Err(e) if e.raw_os_error() == Some(TEXT_FILE_BUSY) && attempt < 20 => {
                attempt += 1;
                thread::sleep(Duration::from_millis(50));
            }
            result => return result,
        }
    }
}

/// TERM to Kev's process group, KILL after `grace`.
fn end(child: &mut Child, grace: Duration) -> std::io::Result<std::process::ExitStatus> {
    let group = -(child.id() as i32);
    checked_signal(group, process::SIGTERM)?;
    let deadline = Instant::now() + grace;
    // Reaping the leader does not prove its helpers ended. Keep checking
    // the group and send KILL even when the leader exited first.
    while Instant::now() < deadline {
        child.try_wait()?;
        if !checked_signal(group, 0)? {
            return child.wait();
        }
        thread::sleep(Duration::from_millis(100));
    }
    checked_signal(group, process::SIGKILL)?;
    let status = child.wait()?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while checked_signal(group, 0)? {
        if Instant::now() >= deadline {
            return Err(std::io::Error::other(
                "Kev process group did not stop after KILL",
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
    Ok(status)
}

/// ESRCH means the process group has ended; all other signal errors propagate.
fn checked_signal(pid: i32, signal: i32) -> std::io::Result<bool> {
    if process::signal(pid, signal) {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(3) {
        Ok(false)
    } else {
        Err(error)
    }
}
