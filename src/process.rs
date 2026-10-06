//! Child processes with limits: an argument array (no shell), stdin closed,
//! a deadline and a cap on stdout. `run_bounded` discards stderr;
//! `run_captured` keeps its start.
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

/// No `Debug`: `stdout` may hold the key a key command printed.
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
    let out = run(program, args, timeout, max_stdout, None)?;
    Ok(Bounded {
        stdout: out.stdout,
        ending: out.ending,
    })
}

/// What `run_captured` collected. No `Debug`, like `Bounded`.
pub struct Captured {
    pub stdout: Vec<u8>,
    /// The first bytes of stderr, up to the cap; the rest was read and dropped.
    pub stderr: Vec<u8>,
    pub ending: Ending,
}

/// `run_bounded` that also keeps the first `max_stderr` bytes of stderr.
/// More stderr does not end the child; it is read and dropped. For
/// `--version` probes, whose stderr explains why a binary does not run;
/// key commands keep using `run_bounded`, which discards stderr.
pub fn run_captured<S: AsRef<OsStr>>(
    program: &Path,
    args: &[S],
    timeout: Duration,
    max_stdout: usize,
    max_stderr: usize,
) -> std::io::Result<Captured> {
    run(program, args, timeout, max_stdout, Some(max_stderr))
}

fn run<S: AsRef<OsStr>>(
    program: &Path,
    args: &[S],
    timeout: Duration,
    max_stdout: usize,
    max_stderr: Option<usize>,
) -> std::io::Result<Captured> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(if max_stderr.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
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
    let errors = Arc::new(Mutex::new(Vec::new()));
    let error_reader = match (max_stderr, child.stderr.take()) {
        (Some(cap), Some(mut stderr)) => {
            let errors = Arc::clone(&errors);
            Some(thread::spawn(move || {
                let mut buffer = [0u8; 4096];
                while let Ok(size) = stderr.read(&mut buffer) {
                    if size == 0 {
                        break;
                    }
                    if let Ok(mut kept) = errors.lock() {
                        let room = cap.saturating_sub(kept.len());
                        kept.extend_from_slice(&buffer[..size.min(room)]);
                    }
                }
            }))
        }
        _ => None,
    };
    let readers_done =
        || reader.is_finished() && error_reader.as_ref().is_none_or(|r| r.is_finished());
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
        if let (Some(status), true) = (status, readers_done()) {
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
    let take = |buffer: &Mutex<Vec<u8>>| {
        std::mem::take(&mut *buffer.lock().unwrap_or_else(|e| e.into_inner()))
    };
    Ok(Captured {
        stdout: take(&collected),
        stderr: take(&errors),
        ending,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Final review M2: `Bounded` holds a key command's stdout, so it must
    /// not implement `Debug`. With a `Debug` impl both impls below apply and
    /// the call is ambiguous, which fails to compile.
    #[test]
    fn bounded_output_is_not_debug() {
        trait AmbiguousIfDebug<A> {
            fn check() {}
        }
        impl<T: ?Sized> AmbiguousIfDebug<()> for T {}
        struct IsDebug;
        impl<T: ?Sized + std::fmt::Debug> AmbiguousIfDebug<IsDebug> for T {}
        <Bounded as AmbiguousIfDebug<_>>::check();
        <Captured as AmbiguousIfDebug<_>>::check();
    }

    #[cfg(unix)]
    #[test]
    fn captured_runs_keep_the_start_of_stderr() {
        let sh = Path::new("/bin/sh");
        let script = "echo out; head -c 10000 /dev/zero | tr '\\0' e >&2; exit 3";
        let out = run_captured(sh, &["-c", script], Duration::from_secs(10), 4096, 4096).unwrap();
        assert_eq!(out.stdout, b"out\n");
        assert_eq!(out.stderr, vec![b'e'; 4096]);
        assert!(matches!(out.ending, Ending::Exited(s) if s.code() == Some(3)));
        let slow =
            run_captured(sh, &["-c", "sleep 30"], Duration::from_millis(200), 10, 10).unwrap();
        assert!(matches!(slow.ending, Ending::TimedOut));
        assert!(run_captured(
            Path::new("/nonexistent/x"),
            &["--version"],
            Duration::from_secs(1),
            10,
            10
        )
        .is_err());
    }
}
