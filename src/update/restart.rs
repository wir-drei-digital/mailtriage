//! Restarting `watch` onto a replaced binary: the installation path and
//! the identity of the running image are recorded at start; between
//! passes a changed file is probed and re-executed with the original
//! arguments and environment, keeping the PID.
use super::{
    events,
    install::Hooks,
    platform::{self, FileIdentity},
    version, CLI,
};
use serde_json::{json, Value};
use std::{
    io::Write,
    path::PathBuf,
    time::{Duration, Instant},
};

/// The first retry after a failed restart; it doubles up to `MAX_BACKOFF`.
pub const FIRST_BACKOFF: Duration = Duration::from_secs(60);
pub const MAX_BACKOFF: Duration = Duration::from_secs(3600);

/// What `watch` records about itself at start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// The canonical installation path.
    pub path: PathBuf,
    /// The identity of the file this process runs.
    pub identity: FileIdentity,
    /// The file was replaced between launch and the recording: restart
    /// before the first pass.
    pub replaced_at_start: bool,
}

impl Image {
    /// Records the installation path and the image identity. On Linux the
    /// image is `/proc/self/exe`, which follows the loaded file even after
    /// it was replaced. On macOS it is the installation path at start, plus
    /// one `--version` run: another version there means the file was
    /// replaced since launch. `Err` says why the restart rule is off.
    pub fn record() -> Result<Self, String> {
        let path = platform::installation_path()
            .map_err(|e| format!("{e}; restarting onto a replaced binary is off"))?;
        let identity = image_identity(&path).map_err(|e| {
            format!(
                "cannot read {} ({e}); restarting onto a replaced binary is off",
                path.display()
            )
        })?;
        let replaced_at_start = if cfg!(target_os = "linux") {
            FileIdentity::read(&path).ok() != Some(identity)
        } else {
            platform::probe(&path, CLI).is_ok_and(|found| found.to_string() != version::RUNNING)
        };
        Ok(Self {
            path,
            identity,
            replaced_at_start,
        })
    }
}

#[cfg(target_os = "linux")]
fn image_identity(_path: &std::path::Path) -> std::io::Result<FileIdentity> {
    FileIdentity::read(std::path::Path::new("/proc/self/exe"))
}

#[cfg(not(target_os = "linux"))]
fn image_identity(path: &std::path::Path) -> std::io::Result<FileIdentity> {
    FileIdentity::read(path)
}

/// Why a restart did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Failure {
    Missing,
    Probe,
    Exec,
}

/// What the restart check decided.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// The file is the running image, or a failed restart waits for its retry.
    Stay,
    /// A stop was requested: `watch` exits instead.
    Stopped,
    /// Re-execute the probed file, which printed this version.
    Exec(semver::Version),
    /// The file is missing or does not run.
    Failed(Failure, String),
}

/// Where events go: `events::emit` in `watch`, a collector in tests.
pub type Emit = Box<dyn Fn(&Value)>;

/// The restart rule for one `watch` process.
pub struct Restarter {
    image: Option<Image>,
    emit: Emit,
    /// (identity, kind) pairs already reported, so each is printed once.
    reported: Vec<(Option<FileIdentity>, Failure)>,
    /// The identity whose restart failed, and when to try it again.
    failed: Option<(Option<FileIdentity>, Instant)>,
    backoff: Duration,
}

impl Restarter {
    /// The rule for `image`; without one it is off, and that is reported once.
    pub fn new(image: Result<Image, String>, emit: Emit) -> Self {
        let image = match image {
            Ok(image) => Some(image),
            Err(why) => {
                emit(&events::error(&why));
                None
            }
        };
        Self {
            image,
            emit,
            reported: Vec::new(),
            failed: None,
            backoff: FIRST_BACKOFF,
        }
    }

    pub fn image(&self) -> Option<&Image> {
        self.image.as_ref()
    }

    /// Restarts onto a replaced binary when there is one. Never returns
    /// after a successful `exec`. Call it only between passes: no
    /// `Service`, database connection, account lock or update lock may be
    /// open, because `exec` runs no destructors.
    pub fn check(&mut self, stopped: &dyn Fn() -> bool, hooks: &dyn Hooks) {
        let decision = self.decide(stopped);
        let Some(image) = &self.image else { return };
        let path = image.path.clone();
        match decision {
            Decision::Stay | Decision::Stopped => {}
            Decision::Failed(kind, message) => self.fail(kind, &message),
            Decision::Exec(to) => {
                let error = match hooks.at("exec") {
                    Err(e) => e.to_string(),
                    Ok(()) => {
                        (self.emit)(&json!({"schema_version": 1, "update": {
                            "event": "restarting",
                            "pid": std::process::id(),
                            "from": version::RUNNING,
                            "to": to.to_string(),
                        }}));
                        let _ = std::io::stdout().flush();
                        let _ = std::io::stderr().flush();
                        exec(&path).to_string()
                    }
                };
                self.fail(
                    Failure::Exec,
                    &format!("cannot restart onto {}: {error}", path.display()),
                );
            }
        }
    }

    /// The restart decision for the file at the installation path now.
    pub fn decide(&mut self, stopped: &dyn Fn() -> bool) -> Decision {
        let Some(image) = &self.image else {
            return Decision::Stay;
        };
        let current = FileIdentity::read(&image.path).ok();
        if current == Some(image.identity) && !image.replaced_at_start {
            return Decision::Stay;
        }
        if let Some((identity, retry_at)) = self.failed {
            if identity == current && Instant::now() < retry_at {
                return Decision::Stay;
            }
        }
        let Some(probed) = current else {
            return Decision::Failed(
                Failure::Missing,
                format!("{} is missing", image.path.display()),
            );
        };
        let to = match platform::probe(&image.path, CLI) {
            Ok(to) => to,
            Err(cause) => {
                return Decision::Failed(
                    Failure::Probe,
                    format!(
                        "the replaced binary at {} does not run: {cause}",
                        image.path.display()
                    ),
                )
            }
        };
        if FileIdentity::read(&image.path).ok() != Some(probed) {
            // Replaced again while probing: the next check probes the new file.
            return Decision::Stay;
        }
        if stopped() {
            return Decision::Stopped;
        }
        Decision::Exec(to)
    }

    /// Reports a failure once per identity and kind and schedules the
    /// retry: 1 min, doubling up to 1 h; at once for another identity.
    fn fail(&mut self, kind: Failure, message: &str) {
        let Some(image) = &self.image else { return };
        let current = FileIdentity::read(&image.path).ok();
        if !self.reported.contains(&(current, kind)) {
            self.reported.push((current, kind));
            (self.emit)(&events::error(message));
        }
        self.backoff = match self.failed {
            Some((identity, _)) if identity == current => (self.backoff * 2).min(MAX_BACKOFF),
            _ => FIRST_BACKOFF,
        };
        self.failed = Some((current, Instant::now() + self.backoff));
    }
}

/// Replaces this process with `path`, keeping `argv` and the environment.
/// Returns only when `exec` fails.
fn exec(path: &std::path::Path) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    let mut args = std::env::args_os();
    let mut command = std::process::Command::new(path);
    if let Some(argv0) = args.next() {
        command.arg0(argv0);
    }
    command.args(args).exec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::install::NoHooks;
    use std::{cell::RefCell, fs, os::unix::fs::PermissionsExt, path::Path, rc::Rc};

    type Seen = Rc<RefCell<Vec<Value>>>;

    fn collector() -> (Emit, Seen) {
        let seen: Seen = Rc::default();
        let sink = Rc::clone(&seen);
        (
            Box::new(move |event| sink.borrow_mut().push(event.clone())),
            seen,
        )
    }

    fn script(path: &Path, version: &str, mode: u32) {
        let tmp = path.with_extension("new");
        fs::write(&tmp, format!("#!/bin/sh\necho 'mailtriage {version}'\n")).unwrap();
        fs::set_permissions(&tmp, fs::Permissions::from_mode(mode)).unwrap();
        fs::rename(&tmp, path).unwrap();
    }

    fn restarter(path: &Path) -> (Restarter, Seen) {
        let image = Image {
            path: path.to_path_buf(),
            identity: FileIdentity::read(path).unwrap(),
            replaced_at_start: false,
        };
        let (emit, seen) = collector();
        (Restarter::new(Ok(image), emit), seen)
    }

    #[test]
    fn an_unchanged_file_stays_and_a_replaced_one_is_probed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mailtriage");
        script(&path, "0.1.0", 0o755);
        let (mut r, _) = restarter(&path);
        assert_eq!(r.decide(&|| false), Decision::Stay);
        script(&path, "9.9.9", 0o755);
        assert_eq!(
            r.decide(&|| false),
            Decision::Exec(semver::Version::new(9, 9, 9))
        );
        assert_eq!(r.decide(&|| true), Decision::Stopped);
    }

    #[test]
    fn a_failed_restart_waits_unless_the_file_changes_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mailtriage");
        script(&path, "0.1.0", 0o755);
        let (mut r, seen) = restarter(&path);
        script(&path, "9.9.9", 0o644);
        assert!(matches!(
            r.decide(&|| false),
            Decision::Failed(Failure::Probe, _)
        ));
        r.check(&|| false, &NoHooks);
        assert_eq!(r.backoff, FIRST_BACKOFF);
        assert_eq!(seen.borrow().len(), 1);
        assert_eq!(seen.borrow()[0]["update"]["event"], "error");
        assert!(seen.borrow()[0]["update"]["message"]
            .as_str()
            .unwrap()
            .ends_with("does not run: could not start"));
        // The same identity waits for its retry.
        assert_eq!(r.decide(&|| false), Decision::Stay);
        // `chmod +x` changes the identity (change time and mode): at once.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            r.decide(&|| false),
            Decision::Exec(semver::Version::new(9, 9, 9))
        );
        fs::remove_file(&path).unwrap();
        assert!(matches!(
            r.decide(&|| false),
            Decision::Failed(Failure::Missing, _)
        ));
    }

    #[test]
    fn retries_back_off_from_a_minute_to_an_hour() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mailtriage");
        script(&path, "0.1.0", 0o755);
        let (mut r, seen) = restarter(&path);
        script(&path, "9.9.9", 0o644);
        let mut waits = Vec::new();
        for _ in 0..8 {
            r.fail(Failure::Probe, "x");
            waits.push(r.backoff.as_secs() / 60);
        }
        assert_eq!(waits, [1, 2, 4, 8, 16, 32, 60, 60]);
        // One event per identity and kind.
        assert_eq!(seen.borrow().len(), 1);
        r.fail(Failure::Exec, "y");
        assert_eq!(seen.borrow().len(), 2);
    }

    #[test]
    fn without_an_image_the_rule_is_off_and_says_so_once() {
        let (emit, seen) = collector();
        let mut r = Restarter::new(Err("no image".into()), emit);
        assert_eq!(r.decide(&|| false), Decision::Stay);
        r.check(&|| false, &NoHooks);
        assert!(r.image().is_none());
        assert_eq!(*seen.borrow(), vec![events::error("no image")]);
    }
}
