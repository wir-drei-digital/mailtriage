//! Local Kev, run by mailtriage (Kev spec, "The managed runtime"): where
//! its files live, the record setup writes once it works
//! (`installed.json`), and the test stand-ins for this machine. `runtime`
//! starts the server on demand; `supervisor` is the hidden
//! `mailtriage kev run`.
mod control;
pub mod install;
mod installation;
#[doc(hidden)]
pub mod mutation;
pub mod runtime;
pub mod supervisor;

use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

/// The Kev release a managed install checks out.
pub const TAG: &str = "kev-1.0";
pub const REPO: &str = "https://github.com/jaredpalmer/kev.git";
/// How long Kev may go without a request before the supervisor stops it.
pub const IDLE: Duration = Duration::from_secs(15 * 60);
/// How often the supervisor looks.
pub const CHECK: Duration = Duration::from_secs(30);
/// How long Kev has after TERM before it gets KILL.
pub const STOP_GRACE: Duration = Duration::from_secs(10);
/// How long `ensure_running` waits for a server it started.
pub const START_TIMEOUT: Duration = Duration::from_secs(120);
/// How long setup waits for the first start, which downloads the weights.
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Debug builds only: a JSON object (`TestKev`) that describes this
/// machine to tests. Release builds never read it.
pub const TEST_ENV: &str = "MAILTRIAGE_TEST_KEV";

/// `<data dir>/mailtriage/kev` and the files in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    root: PathBuf,
}

impl Layout {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// This user's: under `XDG_DATA_HOME`, else `~/.local/share`, next to
    /// the private Himalaya; `None` without either.
    pub fn for_user() -> Option<Self> {
        crate::distribution::himalaya::default_data_dir()
            .map(|data| Self::new(data.join("mailtriage").join("kev")))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    /// The git checkout of Kev, with its `.venv`.
    pub fn src(&self) -> PathBuf {
        self.root.join("src")
    }
    /// Where a clone goes before it is renamed to `src`.
    pub fn partial(&self) -> PathBuf {
        self.root.join("src.partial")
    }
    /// `HF_HOME`: the downloaded weights.
    pub fn hf(&self) -> PathBuf {
        self.root.join("hf")
    }
    pub fn installed(&self) -> PathBuf {
        self.root.join("installed.json")
    }
    /// Excludes shared checkout mutation from normal startup.
    pub fn installation_lock(&self) -> PathBuf {
        self.root.join("installation.lock")
    }
    pub(crate) fn installation_owner(&self) -> PathBuf {
        self.root.join("installation.owner")
    }
    /// Held by the running supervisor; holds its diagnostic PID.
    pub fn lock(&self) -> PathBuf {
        self.root.join("server.lock")
    }
    /// Durable proof of the last owned process group's termination.
    pub fn ownership(&self) -> PathBuf {
        self.root.join("server-ownership.json")
    }
    /// A request consumed only by the current supervisor lock holder.
    pub fn stop_request(&self) -> PathBuf {
        self.root.join("server.stop-request")
    }
    /// Its mtime is the last request.
    pub fn last_used(&self) -> PathBuf {
        self.root.join("last-used")
    }
    pub fn log(&self) -> PathBuf {
        self.root.join("kev.log")
    }

    /// What every `uv` call gets: the weights in `hf`, and uv's Python and
    /// cache inside this directory too, so deleting it removes everything.
    pub fn uv_env(&self) -> [(&'static str, PathBuf); 3] {
        [
            ("HF_HOME", self.hf()),
            ("UV_CACHE_DIR", self.root.join("uv-cache")),
            ("UV_PYTHON_INSTALL_DIR", self.root.join("python")),
        ]
    }

    /// Records a request now.
    pub fn touch_last_used(&self) {
        let _ = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.last_used())
            .and_then(|file| file.set_modified(SystemTime::now()));
    }

    /// The time since the last request; without a record, forever.
    pub fn idle_for(&self) -> Duration {
        match fs::metadata(self.last_used()).and_then(|m| m.modified()) {
            Ok(at) => at.elapsed().unwrap_or(Duration::ZERO),
            Err(_) => Duration::MAX,
        }
    }
}

/// `installed.json`, written last by setup: the proof that the code and the
/// weights are in place, and the `uv` that runs them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    pub tag: String,
    pub checkpoint: String,
    pub installed_at: String,
    pub uv: PathBuf,
}

impl Installed {
    /// `None` when missing or unreadable: not installed.
    pub fn read(layout: &Layout) -> Option<Self> {
        serde_json::from_slice(&fs::read(layout.installed()).ok()?).ok()
    }

    /// This release, and `checkpoint`.
    pub fn matches(&self, checkpoint: &str) -> bool {
        self.tag == TAG && self.checkpoint == checkpoint
    }

    /// Written to a temporary file and renamed, so a reader sees all or
    /// nothing.
    pub fn write(&self, layout: &Layout) -> std::io::Result<()> {
        fs::create_dir_all(layout.root())?;
        let partial = layout.root().join("installed.json.partial");
        fs::write(
            &partial,
            serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?,
        )?;
        fs::rename(partial, layout.installed())
    }
}

/// This machine as tests describe it (`MAILTRIAGE_TEST_KEV`); every field
/// left out is the real value.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestKev {
    pub apple_silicon: Option<bool>,
    pub free_gb: Option<u64>,
    pub memory_gb: Option<u64>,
    pub port: Option<u16>,
    pub start_seconds: Option<u64>,
    pub download_seconds: Option<u64>,
}

/// `MAILTRIAGE_TEST_KEV` in debug builds; empty otherwise.
pub fn test_kev() -> TestKev {
    #[cfg(debug_assertions)]
    if let Some(text) = std::env::var_os(TEST_ENV) {
        return serde_json::from_str(&text.to_string_lossy())
            .expect("MAILTRIAGE_TEST_KEV is a JSON object of TestKev fields");
    }
    TestKev::default()
}

/// A Mac with Apple Silicon: the only place mailtriage runs Kev itself.
pub fn apple_silicon() -> bool {
    test_kev()
        .apple_silicon
        .unwrap_or_else(|| crate::update::release::platform() == Some("macos-arm64"))
}

/// The port a new managed install uses: 8009.
pub fn port() -> u16 {
    test_kev()
        .port
        .unwrap_or(crate::provider::kev::DEFAULT_PORT)
}

/// How long setup waits for the first start.
pub fn download_timeout() -> Duration {
    test_kev()
        .download_seconds
        .map_or(DOWNLOAD_TIMEOUT, Duration::from_secs)
}
