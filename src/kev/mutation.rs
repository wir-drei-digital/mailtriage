//! Internal setup writer supervision. The inherited exclusive installation
//! lock survives setup's disappearance. A durable group journal blocks
//! replacement if this helper disappears before confirming writer cleanup.
use std::{ffi::OsString, path::PathBuf, time::Duration};

pub struct Args {
    pub dir: PathBuf,
    pub guard_fd: i32,
    pub program: PathBuf,
    pub cwd: PathBuf,
    pub arguments: Vec<OsString>,
    pub timeout: Duration,
}

#[cfg(not(unix))]
pub fn run(_args: &Args) -> anyhow::Result<serde_json::Value> {
    anyhow::bail!("local Kev mutation requires Unix")
}

#[cfg(unix)]
pub fn run(args: &Args) -> anyhow::Result<serde_json::Value> {
    use super::{control, installation, supervisor, Layout, STOP_GRACE};
    use fs2::FileExt;
    use std::{
        fs::OpenOptions,
        io::Write,
        os::unix::{fs::MetadataExt, process::CommandExt},
        process::{Command, Stdio},
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        thread,
        time::Instant,
    };
    let layout = Layout::new(args.dir.clone());
    anyhow::ensure!(args.guard_fd >= 3, "invalid Kev mutation descriptor");
    // Duplicate and validate the inherited guard rather than taking ownership
    // of a raw descriptor supplied by a caller. Both descriptors are closed
    // on tool exec; only the helper retains exclusion through group cleanup.
    let guard = installation::copy_guard(args.guard_fd)?;
    let actual = guard.metadata()?;
    let expected = std::fs::metadata(layout.installation_lock())?;
    anyhow::ensure!(
        actual.is_file()
            && expected.is_file()
            && actual.dev() == expected.dev()
            && actual.ino() == expected.ino(),
        "invalid Kev mutation guard"
    );
    guard.try_lock_exclusive()?;
    // Tools and descendants may close unknown descriptors: the helper alone
    // retains exclusion, while its durable proof covers unexpected death.
    let mut lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(layout.lock())?;
    lock.try_lock_exclusive()?;
    control::confirm_terminated(&layout, &lock)?;
    let stopping = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&stopping);
    ctrlc::set_handler(move || signal.store(true, Ordering::SeqCst))?;
    let mut command = Command::new(&args.program);
    command
        .args(&args.arguments)
        .current_dir(&args.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .process_group(0);
    command.env_remove("KEV_API_KEY");
    let mut ownership = control::Ownership::begin(&layout)?;
    lock.set_len(0)?;
    write!(lock, "{}", std::process::id())?;
    lock.flush()?;
    let mut child = match supervisor::spawn(&mut command) {
        Ok(child) => child,
        Err(e) => {
            ownership.finish(&layout)?;
            return Err(e.into());
        }
    };
    if let Err(e) = ownership.child(&layout, child.id()) {
        supervisor::end(&mut child, STOP_GRACE)?;
        ownership.finish(&layout)?;
        return Err(e.into());
    }
    let deadline = Instant::now() + args.timeout;
    let (status, interrupted) = loop {
        if child.try_wait()?.is_some() {
            break (supervisor::end(&mut child, STOP_GRACE)?, false);
        }
        if stopping.load(Ordering::SeqCst)
            || control::requested(&layout)?
            || Instant::now() >= deadline
        {
            break (supervisor::end(&mut child, STOP_GRACE)?, true);
        }
        thread::sleep(Duration::from_millis(50));
    };
    ownership.finish(&layout)?;
    anyhow::ensure!(
        !interrupted,
        "Kev installation command stopped or exceeded its download deadline"
    );
    anyhow::ensure!(
        status.success(),
        "Kev installation command exited ({status})"
    );
    // No saved group/PID is ever signalled. All cleanup used the live Child.
    Ok(serde_json::json!({"schema_version":1,"kev":{"action":"mutation_completed"}}))
}
