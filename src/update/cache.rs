//! The per-user update cache: `update.json`, read and written under
//! `update.lock`, replaced atomically.
use super::{install::try_until, platform::FileIdentity, release::CachedRelease, schedule};
use crate::domain::UpdateMode;
use anyhow::{anyhow, Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::Duration,
};

pub const FILE: &str = "update.json";
pub const LOCK: &str = "update.lock";
/// How long a writer waits for the cache lock, which is held only for a
/// read or a write.
const LOCK_WAIT: Duration = Duration::from_secs(10);

/// `~/Library/Caches/mailtriage` on macOS; elsewhere
/// `$XDG_CACHE_HOME/mailtriage` when that is absolute, else
/// `~/.cache/mailtriage`. `None` without the directory it needs.
pub fn cache_dir(
    macos: bool,
    home: Option<&Path>,
    xdg_cache_home: Option<&OsStr>,
) -> Option<PathBuf> {
    let home = home.filter(|h| !h.as_os_str().is_empty());
    if macos {
        return home.map(|h| h.join("Library/Caches/mailtriage"));
    }
    match xdg_cache_home.map(Path::new).filter(|p| p.is_absolute()) {
        Some(xdg) => Some(xdg.join("mailtriage")),
        None => home.map(|h| h.join(".cache/mailtriage")),
    }
}

/// This user's cache directory, from `HOME` and `XDG_CACHE_HOME`.
pub fn default_dir() -> Option<PathBuf> {
    cache_dir(
        cfg!(target_os = "macos"),
        std::env::var_os("HOME").map(PathBuf::from).as_deref(),
        std::env::var_os("XDG_CACHE_HOME").as_deref(),
    )
}

/// `update.json`. Keys this version does not know (from other versions, or
/// the tray's) are kept on every rewrite, here and in each `configs` and
/// `installs` entry.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CacheFile {
    #[serde(default)]
    pub schema_version: u32,
    /// The candidate of the last successful check; `None` when it found no
    /// stable release (or before the first check: then `checked_at` is
    /// `None` too).
    #[serde(default)]
    pub release: Option<CachedRelease>,
    #[serde(default)]
    pub checked_at: Option<String>,
    #[serde(default)]
    pub next_check_at: Option<String>,
    #[serde(default)]
    pub check_failures: u32,
    #[serde(default)]
    pub last_check_error: Option<ErrorRecord>,
    /// By canonical config path.
    #[serde(default)]
    pub configs: BTreeMap<String, ConfigEntry>,
    /// By canonical installation path.
    #[serde(default)]
    pub installs: BTreeMap<String, InstallEntry>,
    /// Keys this version does not know, kept as they are.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorRecord {
    pub at: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigEntry {
    pub mode: UpdateMode,
    #[serde(default)]
    pub notified_version: Option<String>,
    /// Keys this version does not know, kept as they are.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallEntry {
    /// The installed version: recorded by an install, or by the last
    /// reading of `--version`.
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub at: Option<String>,
    #[serde(default)]
    pub last_error: Option<ErrorRecord>,
    #[serde(default)]
    pub failures: u32,
    #[serde(default)]
    pub next_attempt_at: Option<String>,
    /// The file's identity when `version` was recorded; another identity
    /// means `--version` must run again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<super::platform::FileIdentity>,
    /// Keys this version does not know, kept as they are.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The key of the installation at `path` (canonical) in `installs`.
pub fn install_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// The cache in one directory.
#[derive(Debug, Clone)]
pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// This user's cache; `None` without `HOME` (see `default_dir`).
    pub fn for_user() -> Option<Self> {
        default_dir().map(Self::new)
    }

    /// The cache of the user whose home is `home`. `XDG_CACHE_HOME`
    /// counts only when `home` is this process's `HOME`, so a service
    /// `Context` built for another home (as tests build them) never
    /// reaches this user's cache.
    pub fn for_home(home: &Path) -> Option<Self> {
        let own = std::env::var_os("HOME").is_some_and(|h| Path::new(&h) == home);
        let xdg = own.then(|| std::env::var_os("XDG_CACHE_HOME")).flatten();
        cache_dir(cfg!(target_os = "macos"), Some(home), xdg.as_deref()).map(Self::new)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `update.json`; a missing or unreadable file counts as empty. Takes
    /// the cache lock shared when its file exists, and creates nothing.
    /// Reads without the lock when another process holds it for longer than
    /// `LOCK_WAIT`, or at once when locking fails for another reason.
    pub fn read(&self) -> CacheFile {
        self.read_with(LOCK_WAIT, FileExt::try_lock_shared)
    }

    /// `read`, locking with `attempt` and waiting out contention for up to
    /// `wait`.
    fn read_with(
        &self,
        wait: Duration,
        mut attempt: impl FnMut(&File) -> io::Result<()>,
    ) -> CacheFile {
        let _lock = OpenOptions::new()
            .read(true)
            .open(self.dir.join(LOCK))
            .ok()
            .filter(|lock| try_until(wait, || attempt(lock)).unwrap_or(false));
        self.read_unlocked()
    }

    fn read_unlocked(&self) -> CacheFile {
        fs::read(self.dir.join(FILE))
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default()
    }

    /// Applies `change` to the current contents under the exclusive cache
    /// lock and replaces the file atomically (an exclusively created
    /// temporary file, fsync, rename). Creates the directory (mode 0700).
    /// Waits up to `LOCK_WAIT` while another process holds the lock; any
    /// other lock error fails at once, naming the lock file.
    pub fn update<T>(&self, change: impl FnOnce(&mut CacheFile) -> T) -> Result<T> {
        self.update_with(LOCK_WAIT, FileExt::try_lock_exclusive, change)
    }

    /// `update`, locking with `attempt` and waiting out contention for up
    /// to `wait`.
    fn update_with<T>(
        &self,
        wait: Duration,
        mut attempt: impl FnMut(&File) -> io::Result<()>,
        change: impl FnOnce(&mut CacheFile) -> T,
    ) -> Result<T> {
        create_private_dir(&self.dir)?;
        let path = self.dir.join(LOCK);
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        let locked = try_until(wait, || attempt(&lock))
            .with_context(|| format!("cannot lock {}", path.display()))?;
        if !locked {
            return Err(anyhow!("the update cache is locked by another process"));
        }
        let mut file = self.read_unlocked();
        let out = change(&mut file);
        file.schema_version = 1;
        self.write(&file)?;
        drop(lock);
        Ok(out)
    }

    fn write(&self, file: &CacheFile) -> Result<()> {
        let tmp = self
            .dir
            .join(format!(".{FILE}.{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut out = options.open(&tmp)?;
            serde_json::to_writer_pretty(&mut out, file)?;
            out.write_all(b"\n")?;
            out.sync_all()?;
            fs::rename(&tmp, self.dir.join(FILE))?;
            if let Ok(dir) = File::open(&self.dir) {
                let _ = dir.sync_all();
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result.with_context(|| format!("write {}", self.dir.join(FILE).display()))
    }
}

/// The writes every updater shares on an `installs` entry, keyed by the
/// installation's canonical path.
impl Cache {
    /// A failed install (spec "Failure"): `last_error`, `failures + 1`,
    /// and `next_attempt_at` backing off from 1 h, doubling, up to 24 h.
    /// `update` and `watch` both record failures through this.
    pub fn record_install_failure(&self, key: &str, message: &str) -> Result<()> {
        let now = chrono::Utc::now();
        self.update(|c| {
            let entry = c.installs.entry(key.to_owned()).or_default();
            entry.failures = entry.failures.saturating_add(1);
            entry.last_error = Some(ErrorRecord {
                at: schedule::stamp(now),
                message: message.to_owned(),
            });
            entry.next_attempt_at = Some(schedule::stamp(schedule::install_backoff(
                now,
                entry.failures,
            )));
        })
    }

    /// A binary that does not run here, so nothing was installed: no
    /// version, why in `last_error`, and no backoff (`failures` and
    /// `next_attempt_at` stay as they are).
    pub fn record_skip(
        &self,
        key: &str,
        message: &str,
        identity: Option<FileIdentity>,
    ) -> Result<()> {
        let now = chrono::Utc::now();
        self.update(|c| {
            let entry = c.installs.entry(key.to_owned()).or_default();
            entry.version = None;
            entry.identity = identity;
            entry.last_error = Some(ErrorRecord {
                at: schedule::stamp(now),
                message: message.to_owned(),
            });
        })
    }

    /// A reading of the installed version (`None`: it could not be read)
    /// and the identity of the file it was read from.
    pub fn record_version(
        &self,
        key: &str,
        version: Option<&semver::Version>,
        identity: Option<FileIdentity>,
    ) -> Result<()> {
        self.update(|c| {
            let entry = c.installs.entry(key.to_owned()).or_default();
            entry.version = version.map(ToString::to_string);
            entry.identity = identity;
        })
    }
}

fn create_private_dir(dir: &Path) -> Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(dir)
        .with_context(|| format!("create {}", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::{cell::Cell, time::Instant};

    #[test]
    fn the_cache_directory_follows_the_platform_rules() {
        let home = Some(Path::new("/h"));
        assert_eq!(
            cache_dir(true, home, Some(OsStr::new("/x"))),
            Some(PathBuf::from("/h/Library/Caches/mailtriage"))
        );
        assert_eq!(
            cache_dir(false, home, Some(OsStr::new("/x"))),
            Some(PathBuf::from("/x/mailtriage"))
        );
        assert_eq!(
            cache_dir(false, home, Some(OsStr::new("relative"))),
            Some(PathBuf::from("/h/.cache/mailtriage"))
        );
        assert_eq!(
            cache_dir(false, home, None),
            Some(PathBuf::from("/h/.cache/mailtriage"))
        );
        assert_eq!(cache_dir(true, None, None), None);
        assert_eq!(cache_dir(false, Some(Path::new("")), None), None);
        assert_eq!(
            cache_dir(false, None, Some(OsStr::new("/x"))),
            Some(PathBuf::from("/x/mailtriage"))
        );
    }

    /// Only the home in `HOME` takes `XDG_CACHE_HOME` into account.
    #[test]
    fn another_home_has_its_own_cache() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::for_home(dir.path()).unwrap();
        let expected = if cfg!(target_os = "macos") {
            dir.path().join("Library/Caches/mailtriage")
        } else {
            dir.path().join(".cache/mailtriage")
        };
        assert_eq!(cache.dir(), expected);
        assert!(Cache::for_home(Path::new("")).is_none());
        if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
            assert_eq!(
                Cache::for_home(Path::new(&home)).map(|c| c.dir().to_owned()),
                default_dir()
            );
        }
    }

    #[test]
    fn a_missing_or_unreadable_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().join("c"));
        assert_eq!(cache.read(), CacheFile::default());
        assert!(!dir.path().join("c").exists(), "reading created files");
        fs::create_dir_all(dir.path().join("c")).unwrap();
        fs::write(dir.path().join("c").join(FILE), "{not json").unwrap();
        assert_eq!(cache.read(), CacheFile::default());
    }

    #[test]
    fn updates_are_written_atomically_and_privately() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().join("c"));
        cache
            .update(|c| c.next_check_at = Some("2026-11-03T09:00:00Z".into()))
            .unwrap();
        let file = cache.read();
        assert_eq!(file.schema_version, 1);
        assert_eq!(file.next_check_at.as_deref(), Some("2026-11-03T09:00:00Z"));
        let text = fs::read_to_string(dir.path().join("c").join(FILE)).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        for key in ["release", "checked_at", "last_check_error"] {
            assert!(value[key].is_null(), "{key}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&dir.path().join("c")), 0o700);
            assert_eq!(mode(&dir.path().join("c").join(FILE)), 0o600);
        }
        let leftovers: Vec<_> = fs::read_dir(dir.path().join("c"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    /// Other versions (and the tray) write keys this one does not know; a
    /// rewrite keeps them, at the top level and inside `configs` and
    /// `installs` entries.
    #[test]
    fn a_rewrite_keeps_keys_it_does_not_know() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().join("c"));
        fs::create_dir_all(cache.dir()).unwrap();
        let before = serde_json::json!({
            "schema_version": 1,
            "checked_at": "2026-11-02T09:00:00Z",
            "check_failures": 0,
            "future_top": {"tray": {"seen": ["0.3.0", "0.4.0"]}, "n": 7},
            "configs": {"/c/mailtriage.json": {
                "mode": "notify",
                "notified_version": "0.3.0",
                "future_config": {"shown_at": "2026-11-01T08:00:00Z", "times": [1, 2]}
            }},
            "installs": {"/b/mailtriage": {
                "version": "0.2.0",
                "failures": 2,
                "future_install": {"component": "mailtriage-tray", "args": ["--quiet"]}
            }}
        });
        fs::write(cache.dir().join(FILE), before.to_string()).unwrap();

        cache
            .update(|c| {
                c.installs.get_mut("/b/mailtriage").unwrap().version = Some("0.3.0".into());
            })
            .unwrap();

        let text = fs::read_to_string(cache.dir().join(FILE)).unwrap();
        let after: serde_json::Value = serde_json::from_str(&text).unwrap();
        let install = &after["installs"]["/b/mailtriage"];
        let config = &after["configs"]["/c/mailtriage.json"];
        assert_eq!(install["version"], "0.3.0", "{text}");
        assert_eq!(install["failures"], 2, "{text}");
        assert_eq!(after["checked_at"], before["checked_at"], "{text}");
        assert_eq!(config["mode"], "notify", "{text}");
        assert_eq!(config["notified_version"], "0.3.0", "{text}");
        assert_eq!(after["future_top"], before["future_top"], "{text}");
        assert_eq!(
            config["future_config"], before["configs"]["/c/mailtriage.json"]["future_config"],
            "{text}"
        );
        assert_eq!(
            install["future_install"], before["installs"]["/b/mailtriage"]["future_install"],
            "{text}"
        );
    }

    #[test]
    fn an_unwritable_cache_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("file"), "").unwrap();
        let cache = Cache::new(dir.path().join("file").join("c"));
        assert!(cache.update(|c| c.check_failures = 1).is_err());
    }

    /// R11-5: one failure recorder for `update` and `watch` (backoff from
    /// 1 h, doubling, at most 24 h); a skip and a reading leave the backoff
    /// alone.
    #[test]
    fn failures_back_off_while_skips_and_readings_do_not() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().join("c"));
        let before = Utc::now();
        cache
            .record_install_failure("/b/t", "checksum mismatch")
            .unwrap();
        cache
            .record_install_failure("/b/t", "checksum mismatch")
            .unwrap();
        let entry = cache.read().installs["/b/t"].clone();
        assert_eq!(entry.failures, 2);
        assert_eq!(entry.last_error.unwrap().message, "checksum mismatch");
        let next = schedule::parse(entry.next_attempt_at.as_deref().unwrap()).unwrap();
        assert!(next >= before + chrono::Duration::hours(2) - chrono::Duration::seconds(1));
        assert!(next <= Utc::now() + chrono::Duration::hours(2));

        let identity = FileIdentity::read(dir.path()).unwrap();
        cache
            .record_skip(
                "/b/t",
                "mailtriage-tray does not run here: x",
                Some(identity),
            )
            .unwrap();
        let entry = cache.read().installs["/b/t"].clone();
        assert_eq!(
            (entry.version, entry.failures, entry.identity),
            (None, 2, Some(identity))
        );
        assert_eq!(
            entry.last_error.unwrap().message,
            "mailtriage-tray does not run here: x"
        );
        assert_eq!(
            entry.next_attempt_at.as_deref(),
            Some(&*schedule::stamp(next))
        );

        let version = semver::Version::new(0, 3, 0);
        cache
            .record_version("/b/t", Some(&version), Some(identity))
            .unwrap();
        let entry = cache.read().installs["/b/t"].clone();
        assert_eq!(entry.version.as_deref(), Some("0.3.0"));
        assert_eq!((entry.failures, entry.identity), (2, Some(identity)));
    }

    fn no_locks() -> io::Error {
        io::Error::other("No locks available")
    }

    /// Only contention means another process holds the lock; any other
    /// error (ENOLCK on a filesystem without locks, say) is not waited out.
    #[test]
    fn a_lock_error_other_than_contention_fails_an_update_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().join("c"));
        let start = Instant::now();
        let error = cache
            .update_with(LOCK_WAIT, |_| Err(no_locks()), |c| c.check_failures = 1)
            .unwrap_err();
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
        assert_eq!(
            format!("{error:#}"),
            format!(
                "cannot lock {}: No locks available",
                cache.dir().join(LOCK).display()
            )
        );
        assert!(!cache.dir().join(FILE).exists(), "wrote without the lock");
    }

    #[test]
    fn contention_makes_an_update_wait_then_fail_as_busy() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().join("c"));
        let tries = Cell::new(0);
        let start = Instant::now();
        let error = cache
            .update_with(
                Duration::from_millis(250),
                |_| {
                    tries.set(tries.get() + 1);
                    Err(fs2::lock_contended_error())
                },
                |c| c.check_failures = 1,
            )
            .unwrap_err();
        assert!(start.elapsed() >= Duration::from_millis(250));
        assert!(tries.get() >= 2, "{}", tries.get());
        assert_eq!(
            format!("{error:#}"),
            "the update cache is locked by another process"
        );
        assert!(!cache.dir().join(FILE).exists(), "wrote without the lock");
    }

    #[test]
    fn a_held_lock_makes_an_update_busy() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().join("c"));
        cache.update(|c| c.check_failures = 1).unwrap();
        let holder = File::open(cache.dir().join(LOCK)).unwrap();
        holder.lock_exclusive().unwrap();
        let error = cache
            .update_with(
                Duration::from_millis(250),
                |lock| lock.try_lock_exclusive(),
                |c| c.check_failures = 2,
            )
            .unwrap_err();
        assert_eq!(
            format!("{error:#}"),
            "the update cache is locked by another process"
        );
        drop(holder);
        cache.update(|c| c.check_failures = 3).unwrap();
        assert_eq!(cache.read().check_failures, 3);
    }

    /// `read` cannot fail: a lock error other than contention makes it read
    /// without the lock at once; contention is waited out first.
    #[test]
    fn a_read_goes_on_without_the_lock_at_once_on_other_lock_errors() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().join("c"));
        cache.update(|c| c.check_failures = 4).unwrap();

        let start = Instant::now();
        let file = cache.read_with(LOCK_WAIT, |_| Err(no_locks()));
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
        assert_eq!(file.check_failures, 4);

        let start = Instant::now();
        let file = cache.read_with(Duration::from_millis(250), |_| {
            Err(fs2::lock_contended_error())
        });
        assert!(start.elapsed() >= Duration::from_millis(250));
        assert_eq!(file.check_failures, 4);
    }
}
