//! The update spec's restart rule for the tray process: when the tray's
//! file is replaced by one that runs, re-execute it with the paths resolved
//! at start, never the original arguments. A tray in a Homebrew keg also
//! follows its `opt` path, which `brew upgrade` retargets to a new keg, and
//! always re-executes that path.
use crate::{instances::Windows, paths::Resolved};
use std::{
    io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// A file's identity: a `chmod` that makes a file runnable again counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity {
    pub dev: u64,
    pub ino: u64,
    pub size: u64,
    pub mtime: (i64, i64),
    pub ctime: (i64, i64),
    pub mode: u32,
}

pub fn identity(path: &Path) -> io::Result<Identity> {
    let m = std::fs::metadata(path)?;
    Ok(Identity {
        dev: m.dev(),
        ino: m.ino(),
        size: m.size(),
        mtime: (m.mtime(), m.mtime_nsec()),
        ctime: (m.ctime(), m.ctime_nsec()),
        mode: m.mode(),
    })
}

/// `X.Y.Z` with an optional `-pre` or `+build` suffix.
pub fn semver_like(version: &str) -> bool {
    let core = version.split(['-', '+']).next().unwrap_or_default();
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// Runs `<path> --version` (10 s): stdout must be `mailtriage-tray <semver>`
/// with an optional newline. Returns the version.
pub fn probe(path: &Path) -> Result<String, String> {
    let finished = crate::cli::run(
        &crate::cli::Invocation {
            program: path.to_path_buf(),
            args: vec!["--version".into()],
        },
        Duration::from_secs(10),
    );
    let out = finished
        .stdout
        .strip_suffix('\n')
        .unwrap_or(&finished.stdout);
    match (finished.exit_code(), out.strip_prefix("mailtriage-tray ")) {
        (Some(0), Some(version)) if semver_like(version) => Ok(version.to_owned()),
        _ => Err(format!("{} --version printed {out:?}", path.display())),
    }
}

/// This process's installation path and the identity of the image it runs:
/// on Linux `/proc/self/exe` (it follows the loaded file even after it was
/// replaced), on macOS the path's identity at start.
pub fn installation() -> Option<(PathBuf, Identity)> {
    let exe = std::env::current_exe().ok()?;
    let deleted = exe.to_string_lossy().ends_with(" (deleted)");
    let path = if deleted {
        let argv0 = PathBuf::from(std::env::args_os().next()?);
        (argv0.is_absolute() && argv0.exists()).then_some(argv0)?
    } else {
        std::fs::canonicalize(&exe).ok()?
    };
    let image = if cfg!(target_os = "linux") {
        identity(Path::new("/proc/self/exe")).ok()?
    } else {
        identity(&path).ok()?
    };
    Some((path, image))
}

/// Watches the tray's own file.
#[derive(Debug, Clone)]
pub struct Restarter {
    path: PathBuf,
    image: Identity,
    /// For a Homebrew keg, its `opt` path.
    launch: Option<PathBuf>,
    /// Restart at the first check (macOS: the file printed another version
    /// at start).
    at_once: bool,
    failures: u32,
    retry_at: Option<Instant>,
    failed: Option<Identity>,
}

impl Restarter {
    pub fn new(path: PathBuf, image: Identity) -> Self {
        let launch = crate::brew::opt_path(&path).filter(|opt| opt.exists());
        Self {
            path,
            image,
            launch,
            at_once: false,
            failures: 0,
            retry_at: None,
            failed: None,
        }
    }

    /// The restarter of this process; `None` when its path cannot be
    /// established (the rule is then off). On macOS a file that already
    /// prints another version than `running` restarts at the first check.
    pub fn start(running: &str) -> Option<Self> {
        let (path, image) = installation()?;
        let mut restarter = Self::new(path, image);
        if cfg!(target_os = "macos") {
            restarter.at_once = probe(&restarter.path).is_ok_and(|v| v != running);
        }
        Some(restarter)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The path the tray starts windows from and re-executes: the `opt`
    /// path of a keg, else its own path.
    pub fn launch_path(&self) -> PathBuf {
        self.launch.clone().unwrap_or_else(|| self.path.clone())
    }

    /// The file to watch, and whether a keg's `opt` path now leads to
    /// another file than the running one.
    fn watched(&self) -> (PathBuf, bool) {
        match &self.launch {
            Some(opt) if std::fs::canonicalize(opt).ok().as_deref() != Some(&*self.path) => {
                (opt.clone(), true)
            }
            _ => (self.path.clone(), false),
        }
    }

    /// The path to re-execute when the file changed (or a keg's `opt` was
    /// retargeted) and the new one runs; after a failure it waits 1 minute,
    /// doubling up to 1 hour, unless the file changes again.
    pub fn check(&mut self, now: Instant) -> Option<PathBuf> {
        let (file, retargeted) = self.watched();
        let current = identity(&file).ok();
        if current == Some(self.image) && !self.at_once && !retargeted {
            return None;
        }
        if self.retry_at.is_some_and(|at| now < at) && current == self.failed {
            return None;
        }
        let ready = current.is_some() && probe(&file).is_ok() && identity(&file).ok() == current;
        if ready {
            return Some(self.launch_path());
        }
        self.fail(now, current);
        None
    }

    /// The `exec` of the path `check` returned failed: the old code keeps
    /// running, and the file waits as after a failed probe.
    pub fn exec_failed(&mut self, now: Instant) {
        let current = identity(&self.watched().0).ok();
        self.fail(now, current);
    }

    /// Waits 1 minute after the first failure, doubling up to 1 hour,
    /// unless the file (`current`) changes again.
    fn fail(&mut self, now: Instant, current: Option<Identity>) {
        self.failures += 1;
        let backoff = Duration::from_secs(60) * 2u32.saturating_pow(self.failures - 1);
        self.retry_at = Some(now + backoff.min(Duration::from_secs(3600)));
        self.failed = current;
    }
}

/// The re-execution: the resolved `--config` and `--mailtriage`, and the
/// windows still running in `MAILTRIAGE_TRAY_WINDOWS`.
pub fn command(path: &Path, resolved: &Resolved, windows: &Windows) -> Command {
    let mut command = Command::new(path);
    command
        .arg("--config")
        .arg(&resolved.config)
        .arg("--mailtriage")
        .arg(&resolved.cli)
        .env(crate::instances::WINDOWS_ENV, windows.env_value())
        .stdin(Stdio::inherit());
    command
}

/// Replaces this process; returns only when `exec` failed.
pub fn exec(mut command: Command) -> io::Error {
    use std::io::Write;
    use std::os::unix::process::CommandExt;
    let _ = io::stdout().flush();
    let _ = io::stderr().flush();
    command.exec()
}
