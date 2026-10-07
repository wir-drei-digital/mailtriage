//! Installing a release (spec steps 4 to 10): under the installation lock,
//! download, verify, unpack, smoke-test, then replace the binary by rename.
//! The rename is the commit point; later problems are warnings.
use super::{
    archive,
    cache::{install_key, Cache},
    github::{self, Net},
    platform::{self, FileIdentity},
    release::{self, CachedRelease},
    schedule, version, Component,
};
use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use fs2::FileExt;
use semver::Version;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

/// The installation lock, in the installation path's directory. Created
/// when missing and never deleted.
pub const LOCK_FILE: &str = ".mailtriage-update.lock";
/// Temporary files of an update attempt: `.tmp` (the staged binary) and
/// `.prev` (the backup before it is published).
pub const TEMP_PREFIX: &str = ".mailtriage-update-";
/// The hidden test hook (debug builds only).
pub const TEST_HOOK: &str = "MAILTRIAGE_UPDATE_TEST_HOOK";
/// The hidden override of `update`'s lock wait in milliseconds (debug builds only).
pub const TEST_LOCK_WAIT: &str = "MAILTRIAGE_UPDATE_TEST_LOCK_WAIT_MS";

/// Points where tests inject faults. Production code uses `EnvHooks`,
/// which does nothing in release builds.
pub trait Hooks {
    /// Called before the step named `point`; an error fails that step.
    fn at(&self, point: &str) -> Result<()>;
}

/// No hooks.
pub struct NoHooks;

impl Hooks for NoHooks {
    fn at(&self, _point: &str) -> Result<()> {
        Ok(())
    }
}

/// In debug builds, `MAILTRIAGE_UPDATE_TEST_HOOK` names a program that runs
/// with the hook point as its only argument; a non-zero exit fails the
/// step. Release builds never read the variable.
pub struct EnvHooks;

impl Hooks for EnvHooks {
    #[cfg(debug_assertions)]
    fn at(&self, point: &str) -> Result<()> {
        use std::process::{Command, Stdio};
        let Some(program) = std::env::var_os(TEST_HOOK) else {
            return Ok(());
        };
        let status = Command::new(program)
            .arg(point)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .status()?;
        if !status.success() {
            bail!("the test hook failed at {point}");
        }
        Ok(())
    }

    #[cfg(not(debug_assertions))]
    fn at(&self, _point: &str) -> Result<()> {
        Ok(())
    }
}

/// How long `update` waits for the installation lock: 60 s.
pub fn update_lock_wait() -> Duration {
    #[cfg(debug_assertions)]
    if let Some(ms) = std::env::var(TEST_LOCK_WAIT)
        .ok()
        .and_then(|v| v.parse().ok())
    {
        return Duration::from_millis(ms);
    }
    Duration::from_secs(60)
}

/// The held installation lock.
pub struct InstallLock {
    _file: File,
}

/// Takes the installation lock of `dir`, trying for up to `wait` (zero:
/// once). `None` when another updater holds it; an error when locking
/// fails for another reason, such as a filesystem without locks.
pub fn lock(dir: &Path, wait: Duration) -> Result<Option<InstallLock>> {
    let path = dir.join(LOCK_FILE);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    let locked = try_until(wait, || file.try_lock_exclusive())
        .with_context(|| format!("cannot lock {}", path.display()))?;
    Ok(locked.then_some(InstallLock { _file: file }))
}

/// Runs `attempt` until it succeeds or `wait` is over (zero: once).
/// `false` when every attempt met lock contention; any other error is
/// returned at once.
pub(super) fn try_until(
    wait: Duration,
    mut attempt: impl FnMut() -> io::Result<()>,
) -> io::Result<bool> {
    let deadline = Instant::now() + wait;
    loop {
        match attempt() {
            Ok(()) => return Ok(true),
            Err(e) if e.kind() != fs2::lock_contended_error().kind() => return Err(e),
            Err(_) => {}
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// Removes leftover temporary files of earlier attempts in `dir`. Only
/// called under the installation lock, so no cooperating updater is using them.
pub fn remove_leftovers(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let stale = entry.file_name().to_string_lossy().starts_with(TEMP_PREFIX)
            && entry
                .file_type()
                .is_ok_and(|t| t.is_file() || t.is_symlink());
        if stale {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// What to install where.
pub struct Job<'a> {
    pub net: &'a Net,
    pub component: Component,
    /// The canonical installation path.
    pub path: &'a Path,
    pub release: &'a CachedRelease,
    /// Compared with the candidate when the installed version cannot be
    /// read: the running version, so an unreadable file is never
    /// downgraded below it.
    pub fallback: &'a Version,
    /// Where step 10 records the installation; `None` records nothing.
    pub cache: Option<&'a Cache>,
    pub hooks: &'a dyn Hooks,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The candidate is not newer than the installed version.
    Current {
        installed: Option<Version>,
    },
    Installed(Installed),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Installed {
    pub from: Option<Version>,
    pub to: Version,
    /// Where the previous binary is: `<path>.previous`, or the temporary
    /// backup when publishing it failed.
    pub previous_path: PathBuf,
    /// Problems after the commit point; the update still counts as done.
    pub warnings: Vec<String>,
}

/// Steps 4 to 10 under the held installation lock. `before_download` runs
/// once the candidate is known to be newer (`watch` writes its reservation
/// there); its error stops the install. Errors leave the installation path
/// and `<path>.previous` untouched and remove this attempt's files.
pub fn install(
    job: &Job,
    _lock: &InstallLock,
    before_download: &mut dyn FnMut() -> Result<()>,
) -> Result<Outcome> {
    let path = job.path;
    let dir = path
        .parent()
        .context("the installation path has no directory")?;
    // 4. Stale files, then the installed version and identity.
    remove_leftovers(dir);
    let installed_identity =
        FileIdentity::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let installed = platform::probe(path, job.component).ok();
    let candidate = Version::parse(&job.release.version).context("invalid release version")?;
    if !version::is_newer(&candidate, installed.as_ref().unwrap_or(job.fallback)) {
        return Ok(Outcome::Current { installed });
    }
    let platform_name = release::platform().unwrap_or("this platform");
    let asset = job
        .release
        .archives
        .get(job.component.name)
        .cloned()
        .flatten()
        .ok_or_else(|| anyhow!(job.component.missing_archive(&candidate, platform_name)))?;
    let sums = job
        .release
        .sums
        .clone()
        .ok_or_else(|| anyhow!("release v{candidate} has no SHA256SUMS"))?;
    before_download()?;
    // 5. Download. 6. Verify.
    let sums_text = job
        .net
        .download(&sums.url, sums.size, github::MAX_SUMS_BYTES)
        .map_err(|e| anyhow!("cannot download SHA256SUMS of v{candidate}: {e}"))?;
    let expected = release::expected_sha256(&String::from_utf8_lossy(&sums_text), &asset.name)
        .map_err(|e| anyhow!("release v{candidate}: {e}"))?;
    let bytes = job
        .net
        .download(&asset.url, asset.size, github::MAX_ASSET_BYTES)
        .map_err(|e| anyhow!("cannot download {}: {e}", asset.name))?;
    if hex(&Sha256::digest(&bytes)) != expected {
        bail!("checksum mismatch for {}", asset.name);
    }
    // 7. Unpack, then place it.
    let binary = archive::extract(&bytes, job.component.name, &archive::RELEASE)
        .map_err(|e| anyhow!("{}: {e}", asset.name))?;
    drop(bytes);
    let placement = Placement {
        component: job.component,
        path,
        before: Some(installed_identity),
        version: &candidate,
        cache: job.cache,
        hooks: job.hooks,
    };
    let placed = place(&placement, _lock, &binary)?;
    Ok(Outcome::Installed(Installed {
        from: installed,
        to: candidate,
        previous_path: placed.previous_path.unwrap_or_else(|| previous_path(path)),
        warnings: placed.warnings,
    }))
}

/// What `place` puts where: an installation path, what it held when its
/// version was read, and the version the new binary must print.
pub struct Placement<'a> {
    pub component: Component,
    /// The installation path; the held installation lock is its directory's.
    pub path: &'a Path,
    /// The installation path's identity when its version was read; `None`
    /// when there was no file, so there is nothing to back up.
    pub before: Option<FileIdentity>,
    /// The new binary must print `<component> <version>`.
    pub version: &'a Version,
    /// Where step 10 records the installation; `None` records nothing.
    pub cache: Option<&'a Cache>,
    pub hooks: &'a dyn Hooks,
}

/// What `place` did from its commit point on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    /// Where the previous binary is: `<path>.previous`, or the temporary
    /// backup when publishing it failed; `None` without a previous binary.
    pub previous_path: Option<PathBuf>,
    /// Problems after the commit point; the binary counts as installed.
    pub warnings: Vec<String>,
}

/// The new binary does not run here (step 8, the smoke test); nothing was
/// replaced. The cause: `could not start`, `printed version X, expected Y`, …
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoesNotRun(pub String);

impl std::fmt::Display for DoesNotRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the new binary does not run here: {}", self.0)
    }
}

impl std::error::Error for DoesNotRun {}

/// Steps 7 to 10 for `binary` (already verified), under the held
/// installation lock: write it to an exclusively created file next to the
/// installation path, smoke-test it, revalidate both files, back up the
/// installed binary when there is one, rename the new one over the path
/// (the commit point), publish the backup as `<path>.previous`, sync the
/// directory and record the installation. Errors before the commit point
/// leave the path and `<path>.previous` untouched and remove this attempt's
/// files; a binary that does not run is a `DoesNotRun` error.
pub fn place(p: &Placement, _lock: &InstallLock, binary: &[u8]) -> Result<Placed> {
    let path = p.path;
    let dir = path
        .parent()
        .context("the installation path has no directory")?;
    let mut scratch = Scratch(Vec::new());
    let staged = scratch.add(dir.join(format!("{TEMP_PREFIX}{}.tmp", uuid::Uuid::new_v4())));
    write_staged(&staged, binary)
        .map_err(|e| anyhow!("cannot write the new binary into {}: {e}", dir.display()))?;
    let staged_identity = FileIdentity::read(&staged)?;
    // 8. Smoke test.
    match platform::probe(&staged, p.component) {
        Ok(found) if found == *p.version => {}
        Ok(found) => {
            return Err(
                DoesNotRun(format!("printed version {found}, expected {}", p.version)).into(),
            )
        }
        Err(cause) => return Err(DoesNotRun(cause).into()),
    }
    // 9.1 Revalidate.
    p.hooks.at("revalidate")?;
    if FileIdentity::read(path).ok() != p.before
        || FileIdentity::read(&staged).ok() != Some(staged_identity)
    {
        bail!("the installed binary changed during the update; try again");
    }
    // 9.2 Backup copy; `.previous` is not touched yet.
    let backup = match p.before {
        Some(_) => {
            let backup =
                scratch.add(dir.join(format!("{TEMP_PREFIX}{}.prev", uuid::Uuid::new_v4())));
            p.hooks
                .at("backup")
                .and_then(|_| copy_or_link(path, &backup).map_err(Into::into))
                .map_err(|e| anyhow!("cannot back up {}: {e}", path.display()))?;
            Some(backup)
        }
        None => None,
    };
    // 9.3 The commit point.
    p.hooks
        .at("commit")
        .and_then(|_| fs::rename(&staged, path).map_err(Into::into))
        .map_err(|e| anyhow!("cannot replace {}: {e}", path.display()))?;
    scratch.keep();
    let mut warnings = Vec::new();
    // 9.4 Publish the backup.
    let previous_path = backup.map(|backup| {
        let previous = previous_path(path);
        match p
            .hooks
            .at("publish")
            .and_then(|_| fs::rename(&backup, &previous).map_err(Into::into))
        {
            Ok(()) => previous,
            Err(e) => {
                warnings.push(format!(
                    "installed; the previous binary stays at {} because {} could not be replaced: {e}",
                    backup.display(),
                    previous.display()
                ));
                backup
            }
        }
    });
    // 9.5 Make the renames durable.
    if let Err(e) = p
        .hooks
        .at("sync_dir")
        .and_then(|_| File::open(dir)?.sync_all().map_err(Into::into))
    {
        warnings.push(format!("installed; syncing {} failed: {e}", dir.display()));
    }
    // 10. Record.
    if let Some(cache) = p.cache {
        let key = install_key(path);
        let identity = FileIdentity::read(path).ok();
        let recorded = p.hooks.at("record").and_then(|_| {
            cache.update(|c| {
                let entry = c.installs.entry(key).or_default();
                entry.version = Some(p.version.to_string());
                entry.at = Some(schedule::stamp(Utc::now()));
                entry.last_error = None;
                entry.failures = 0;
                entry.next_attempt_at = None;
                entry.identity = identity;
            })
        });
        if let Err(e) = recorded {
            warnings.push(format!("installed; recording the update failed: {e:#}"));
        }
    }
    Ok(Placed {
        previous_path,
        warnings,
    })
}

/// `<path>.previous`.
pub fn previous_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".previous");
    PathBuf::from(name)
}

/// This attempt's temporary files, removed unless kept.
#[derive(Default)]
pub struct Scratch(Vec<PathBuf>);

impl Scratch {
    /// Removes `path` when this is dropped, unless `keep` was called.
    pub fn add(&mut self, path: PathBuf) -> PathBuf {
        self.0.push(path.clone());
        path
    }
    /// The files are kept: the commit point was reached.
    pub fn keep(&mut self) {
        self.0.clear();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

/// Created exclusively with mode 0600 (never following an existing link),
/// fsynced and closed, then made executable.
pub fn write_staged(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents)?;
    file.sync_all()?;
    drop(file);
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

/// A hard link to `path` at `backup`, or, where the filesystem refuses
/// hard links, a copy into an exclusively created file.
fn copy_or_link(path: &Path, backup: &Path) -> io::Result<()> {
    if fs::hard_link(path, backup).is_ok() {
        return Ok(());
    }
    let mut source = File::open(path)?;
    let mut copy = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(backup)?;
    io::copy(&mut source, &mut copy)?;
    copy.sync_all()?;
    fs::set_permissions(backup, fs::metadata(path)?.permissions())
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// Only contention means another updater is at work; any other error
    /// (ENOLCK on NFS without locks, say) is returned at once, not waited
    /// out as "busy".
    #[test]
    fn only_lock_contention_counts_as_busy() {
        let tries = Cell::new(0);
        let start = Instant::now();
        let busy = try_until(Duration::from_millis(250), || {
            tries.set(tries.get() + 1);
            Err(fs2::lock_contended_error())
        });
        assert!(!busy.unwrap());
        assert!(start.elapsed() >= Duration::from_millis(250));
        assert!(tries.get() >= 2, "{}", tries.get());

        let tries = Cell::new(0);
        let start = Instant::now();
        let error = try_until(Duration::from_secs(60), || {
            tries.set(tries.get() + 1);
            Err(io::Error::other("No locks available"))
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "No locks available");
        assert_eq!(tries.get(), 1);
        assert!(start.elapsed() < Duration::from_secs(5));

        let tries = Cell::new(0);
        let locked = try_until(Duration::from_secs(60), || {
            tries.set(tries.get() + 1);
            if tries.get() < 3 {
                Err(fs2::lock_contended_error())
            } else {
                Ok(())
            }
        });
        assert!(locked.unwrap());
        assert_eq!(tries.get(), 3);
    }
}
