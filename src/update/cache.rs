//! The per-user update cache: `update.json`, read and written under
//! `update.lock`, replaced atomically.
use super::release::CachedRelease;
use crate::domain::UpdateMode;
use anyhow::{anyhow, Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
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

/// `update.json`. Unknown fields are dropped on the next write.
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

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `update.json`; a missing or unreadable file counts as empty. Takes
    /// the cache lock shared when its file exists, and creates nothing.
    pub fn read(&self) -> CacheFile {
        let _lock = OpenOptions::new()
            .read(true)
            .open(self.dir.join(LOCK))
            .ok()
            .filter(|lock| wait_for(|| lock.try_lock_shared().is_ok()));
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
    pub fn update<T>(&self, change: impl FnOnce(&mut CacheFile) -> T) -> Result<T> {
        create_private_dir(&self.dir)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.dir.join(LOCK))
            .with_context(|| format!("open {}", self.dir.join(LOCK).display()))?;
        if !wait_for(|| lock.try_lock_exclusive().is_ok()) {
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

/// Polls `try_lock` until it succeeds or `LOCK_WAIT` passes.
fn wait_for(mut try_lock: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        if try_lock() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn an_unwritable_cache_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("file"), "").unwrap();
        let cache = Cache::new(dir.path().join("file").join("c"));
        assert!(cache.update(|c| c.check_failures = 1).is_err());
    }
}
