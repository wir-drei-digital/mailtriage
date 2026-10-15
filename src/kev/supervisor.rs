//! `mailtriage kev run` (hidden): the supervisor `ensure_running` and setup
//! start. It holds `server.lock`, runs Kev with `uv`, stops it after the
//! idle time or on TERM, and exits when Kev exits, logging why.
use super::{control, installation, Installed, Layout};
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
    let Some(StartLock { mut lock, startup }) = start_lock_with(&layout, || {
        installation::supervisor_start(&layout, args.allow_download)
    })?
    else {
        return Ok(json!({"schema_version": 1, "kev": {"action": "already_running"}}));
    };
    // A delayed ordinary child may resume after setup has replaced the
    // installation. Revalidate the received arguments under its own shared
    // startup guard, not merely the caller's earlier guard.
    if !args.allow_download {
        Installed::read(&layout)
            .filter(|i| {
                i.matches(&args.checkpoint)
                    && i.uv == args.uv
                    && process::is_executable(&i.uv)
                    && layout.src().is_dir()
            })
            .ok_or_else(|| std::io::Error::other(super::runtime::NOT_INSTALLED))?;
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
    drop(startup);
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
    let mut idle_shutdown_guard = None;
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
                let Some(guard) = super::runtime::idle_guard(&layout, idle) else {
                    continue;
                };
                idle_shutdown_guard = Some(guard);
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
    // Publish confirmed shutdown and release supervisor ownership before a
    // waiting request can probe the port and start the next supervisor.
    drop(lock);
    drop(idle_shutdown_guard);
    Ok(json!({"schema_version": 1, "kev": {"action": action}}))
}

struct StartLock {
    lock: File,
    startup: Option<File>,
}

fn start_lock_with(
    layout: &Layout,
    authorize: impl FnOnce() -> std::io::Result<Option<File>>,
) -> std::io::Result<Option<StartLock>> {
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(layout.lock())?;
    match lock.try_lock_exclusive() {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
        Err(e) => return Err(e),
    }
    // Validate the installer ticket only while holding server.lock. A
    // replacement installer must stop this holder before mutating files,
    // and a stale/missing ticket exits before publishing or spawning.
    let startup = authorize()?;
    Ok(Some(StartLock { lock, startup }))
}

pub(super) fn spawn(command: &mut Command) -> std::io::Result<Child> {
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
pub(super) fn end(child: &mut Child, grace: Duration) -> std::io::Result<std::process::ExitStatus> {
    let group = -(child.id() as i32);
    checked_signal(group, process::SIGTERM)?;
    let deadline = Instant::now() + grace;
    // Reaping the leader does not prove its helpers ended. Keep checking
    // the group and send KILL even when the leader exited first.
    while Instant::now() < deadline {
        child.try_wait()?;
        if !group_present(child, deadline)? {
            return child.wait();
        }
        thread::sleep(Duration::from_millis(100));
    }
    checked_signal(group, process::SIGKILL)?;
    let status = child.wait()?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while group_present(child, deadline)? {
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

/// macOS can report EPERM for signal 0 while a successfully signalled
/// owned group is exiting but not yet waitable. Keep reaping within the
/// cleanup deadline; only a later ESRCH proves disappearance. A persistent
/// permission error still propagates and leaves ownership unconfirmed.
fn group_present(child: &mut Child, deadline: Instant) -> std::io::Result<bool> {
    let group = -(child.id() as i32);
    group_present_with(child, deadline, || checked_signal(group, 0))
}

fn group_present_with(
    child: &mut Child,
    deadline: Instant,
    mut probe: impl FnMut() -> std::io::Result<bool>,
) -> std::io::Result<bool> {
    loop {
        match probe() {
            Err(e)
                if e.kind() == std::io::ErrorKind::PermissionDenied
                    && Instant::now() < deadline =>
            {
                child.try_wait()?;
                thread::sleep(
                    Duration::from_millis(20)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            result => return result,
        }
    }
}

#[cfg(all(test, unix))]
mod shutdown_tests {
    use super::*;
    use std::os::unix::process::CommandExt;

    fn owned_child() -> Child {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exit 0"]).process_group(0);
        command.spawn().unwrap()
    }

    #[test]
    fn transient_permission_probe_waits_for_confirmed_group_disappearance() {
        let mut child = owned_child();
        child.wait().unwrap();
        let group = -(child.id() as i32);
        let mut calls = 0;
        let result =
            group_present_with(&mut child, Instant::now() + Duration::from_secs(1), || {
                calls += 1;
                if calls == 1 {
                    Err(std::io::Error::from_raw_os_error(1))
                } else {
                    checked_signal(group, 0)
                }
            });
        assert!(!result.unwrap());
        assert!(calls >= 2);
    }

    #[test]
    fn persistent_permission_probe_is_bounded_and_never_confirms_shutdown() {
        let mut child = owned_child();
        child.wait().unwrap();
        let began = Instant::now();
        let mut calls = 0;
        let result = group_present_with(&mut child, began + Duration::from_millis(40), || {
            calls += 1;
            Err(std::io::Error::from_raw_os_error(1))
        });
        assert_eq!(
            result.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert!(began.elapsed() >= Duration::from_millis(40));
        assert!(began.elapsed() < Duration::from_secs(1));
        assert!(calls >= 2);
    }
}

#[cfg(all(test, unix))]
#[test]
fn a_replacement_installer_cannot_pass_stop_before_stale_ticket_validation() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path().to_path_buf());
    let old = installation::Guard::acquire(&layout).unwrap();
    let old_owner = std::fs::read_to_string(layout.installation_owner()).unwrap();
    drop(old);
    let result = start_lock_with(&layout, || {
        let _replacement = installation::Guard::acquire(&layout)?;
        // This deliberately schedules replacement between ticket inspection
        // and spawn. Its stop must see the startup lock holder, preventing
        // checkout mutation until this stale download start has exited.
        let outcome = super::runtime::stop(&layout, Duration::ZERO)?;
        assert_eq!(outcome, super::runtime::StopOutcome::TimedOut);
        installation::validate_owner(&layout, &old_owner)?;
        Ok(None)
    });
    assert!(result.is_err(), "a stale ticket must not start a child");
    // Other tests fork concurrently. A forked child can briefly retain this
    // lock's open-file description until exec closes its inherited FD, even
    // after start_lock_with has returned. Use the bounded stop check rather
    // than requiring immediate lock availability.
    assert!(matches!(
        super::runtime::stop(&layout, Duration::from_secs(2)).unwrap(),
        super::runtime::StopOutcome::AlreadyStopped | super::runtime::StopOutcome::Stopped
    ));
    assert!(!layout.ownership().exists());
}
