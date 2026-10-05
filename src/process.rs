//! Child processes with limits: an argument array (no shell), stdin closed,
//! stderr discarded, a deadline and a cap on stdout.
use std::{
    ffi::OsStr,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

/// How a bounded child ended.
#[derive(Debug)]
pub enum Ending {
    Exited(ExitStatus),
    /// Killed at the deadline.
    TimedOut,
    /// Killed after writing more than the stdout cap.
    Overflowed,
}

#[derive(Debug)]
pub struct Bounded {
    /// Everything read before the child ended or was killed.
    pub stdout: Vec<u8>,
    pub ending: Ending,
}

impl Bounded {
    /// Stdout of a child that exited 0.
    pub fn success(self) -> Option<Vec<u8>> {
        match self.ending {
            Ending::Exited(status) if status.success() => Some(self.stdout),
            _ => None,
        }
    }
}

/// Runs `program` with `args`; `Err` only when it cannot be started.
pub fn run_bounded<S: AsRef<OsStr>>(
    program: &Path,
    args: &[S],
    timeout: Duration,
    max_stdout: usize,
) -> std::io::Result<Bounded> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let overflow = Arc::new(AtomicBool::new(false));
    let collected = Arc::new(Mutex::new(Vec::new()));
    let reader = {
        let overflow = Arc::clone(&overflow);
        let collected = Arc::clone(&collected);
        thread::spawn(move || {
            let _ = read_bounded(stdout, max_stdout, &overflow, |chunk| {
                collected
                    .lock()
                    .map(|mut all| all.extend_from_slice(chunk))
                    .is_ok()
            });
        })
    };
    let deadline = Instant::now() + timeout;
    let mut status = None;
    let ending = loop {
        if overflow.load(Ordering::Relaxed) {
            terminate(&mut child);
            break Ending::Overflowed;
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(done) => status = done,
                Err(e) => {
                    terminate(&mut child);
                    return Err(e);
                }
            }
        }
        if let (Some(status), true) = (status, reader.is_finished()) {
            break if overflow.load(Ordering::Relaxed) {
                Ending::Overflowed
            } else {
                Ending::Exited(status)
            };
        }
        if Instant::now() >= deadline {
            terminate(&mut child);
            break Ending::TimedOut;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let stdout = std::mem::take(&mut *collected.lock().unwrap_or_else(|e| e.into_inner()));
    Ok(Bounded { stdout, ending })
}

/// The first executable file called `name` in `PATH`, made absolute.
/// Symlinks are kept, so a package upgrade does not break the stored path.
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
        .and_then(|found| std::path::absolute(found).ok())
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

pub(crate) fn terminate(child: &mut Child) {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        // The child starts in its own process group, so helpers cannot keep pipes open.
        unsafe {
            kill(-(child.id() as i32), 9);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Reads `input` to EOF, handing each chunk to `sink`. Raises `overflow` and
/// stops once more than `limit` bytes arrive; also stops when `sink` refuses.
pub(crate) fn read_bounded<R: Read>(
    mut input: R,
    limit: usize,
    overflow: &AtomicBool,
    mut sink: impl FnMut(&[u8]) -> bool,
) -> std::io::Result<()> {
    let mut total = 0usize;
    let mut buffer = [0u8; 8192];
    loop {
        let size = input.read(&mut buffer)?;
        if size == 0 {
            return Ok(());
        }
        if size > limit.saturating_sub(total) {
            overflow.store(true, Ordering::Relaxed);
            return Ok(());
        }
        total += size;
        if !sink(&buffer[..size]) {
            return Ok(());
        }
    }
}
