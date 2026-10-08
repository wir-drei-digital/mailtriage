//! Which `mailtriage` and which config: resolved once at start, made
//! absolute and canonical, and passed explicitly to every command, window
//! and login item.
use crate::{
    brew,
    cli::{self, Cli, Failure, Request},
};
use std::{
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
};

/// The process environment that resolution reads.
#[derive(Debug, Clone, Default)]
pub struct Env {
    pub cwd: PathBuf,
    /// This program's canonical path.
    pub own_exe: Option<PathBuf>,
    pub path: Option<OsString>,
    pub home: Option<OsString>,
    pub xdg_cache_home: Option<OsString>,
}

impl Env {
    pub fn current() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            own_exe: std::env::current_exe()
                .ok()
                .and_then(|p| std::fs::canonicalize(p).ok()),
            path: std::env::var_os("PATH"),
            home: std::env::var_os("HOME"),
            xdg_cache_home: std::env::var_os("XDG_CACHE_HOME"),
        }
    }
}

/// The CLI and the config every command uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub cli: PathBuf,
    pub config: PathBuf,
}

impl Resolved {
    pub fn cli(&self) -> Cli {
        Cli {
            program: self.cli.clone(),
            config: Some(self.config.clone()),
        }
    }
}

/// Why resolution failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// No `mailtriage` found; the places looked at.
    CliMissing(Vec<String>),
    /// No config: the `--config` path that does not exist, or `None` when
    /// the CLI found none.
    NotSetUp(Option<PathBuf>),
    /// `service status` failed for another reason.
    Status(Failure),
}

impl Problem {
    /// The summary line for the tray and the window.
    pub fn text(&self) -> String {
        match self {
            Problem::CliMissing(looked) => {
                format!("mailtriage not found (looked in {})", looked.join(", "))
            }
            Problem::NotSetUp(None) => "Not set up. Run mailtriage setup in a terminal.".into(),
            // `mailtriage setup` alone would write the default config.
            Problem::NotSetUp(Some(path)) => {
                let shown = path.display();
                let quoted = cli::quote(&path.to_string_lossy());
                format!(
                    "Not set up: no config at {shown}. Run mailtriage setup --config {quoted} in a terminal."
                )
            }
            Problem::Status(failure) => failure.message.clone(),
        }
    }
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// `path` made absolute against `cwd` and canonicalized (symlinks resolved).
/// A path that is not UTF-8 is refused: it travels as command-line text.
pub fn canonical(path: &Path, cwd: &Path) -> io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let canonical = std::fs::canonicalize(absolute)?;
    if canonical.to_str().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path is not valid UTF-8",
        ));
    }
    Ok(canonical)
}

/// `--mailtriage` when given, else `mailtriage` next to this program's
/// canonical path, else the first `mailtriage` on `PATH`; canonicalized,
/// and for a Homebrew keg its `opt` path, which outlives `brew upgrade`.
/// On failure, the places looked at.
pub fn resolve_cli(flag: Option<&Path>, env: &Env) -> Result<PathBuf, Vec<String>> {
    if let Some(flag) = flag {
        return canonical(flag, &env.cwd)
            .ok()
            .filter(|p| is_executable(p))
            .map(|p| brew::launch_path(&p))
            .ok_or_else(|| vec![flag.display().to_string()]);
    }
    let mut looked = vec![];
    if let Some(dir) = env.own_exe.as_deref().and_then(Path::parent) {
        let next = dir.join("mailtriage");
        if is_executable(&next) {
            return canonical(&next, &env.cwd)
                .map(|p| brew::launch_path(&p))
                .map_err(|_| vec![next.display().to_string()]);
        }
        looked.push(next.display().to_string());
    }
    if let Some(path) = &env.path {
        for dir in std::env::split_paths(path).filter(|d| !d.as_os_str().is_empty()) {
            let candidate = dir.join("mailtriage");
            if is_executable(&candidate) {
                if let Ok(found) = canonical(&candidate, &env.cwd) {
                    return Ok(brew::launch_path(&found));
                }
            }
        }
    }
    looked.push("PATH".into());
    Err(looked)
}

/// The config: `--config` when given, else the `config` of the first
/// `service status` (the path the CLI resolved), canonicalized once.
pub fn resolve_config(flag: Option<&Path>, program: &Path, env: &Env) -> Result<PathBuf, Problem> {
    if let Some(flag) = flag {
        return canonical(flag, &env.cwd).map_err(|_| {
            Problem::NotSetUp(Some(if flag.is_absolute() {
                flag.to_path_buf()
            } else {
                env.cwd.join(flag)
            }))
        });
    }
    let first = Cli {
        program: program.to_path_buf(),
        config: None,
    };
    match cli::status(&first.run(&Request::Status)) {
        Ok(status) => canonical(&status.config, &env.cwd)
            .map_err(|_| Problem::NotSetUp(Some(status.config.clone()))),
        Err(failure)
            if failure.code == Some(2)
                && failure.message.starts_with("configuration not found") =>
        {
            Err(Problem::NotSetUp(None))
        }
        Err(failure) => Err(Problem::Status(failure)),
    }
}

/// Both paths, in that order.
pub fn resolve(
    flag_cli: Option<&Path>,
    flag_config: Option<&Path>,
    env: &Env,
) -> Result<Resolved, Problem> {
    let cli = resolve_cli(flag_cli, env).map_err(Problem::CliMissing)?;
    let config = resolve_config(flag_config, &cli, env)?;
    Ok(Resolved { cli, config })
}

/// The automatic-update spec's cache directory, which holds the tray's and
/// the window's locks: `~/Library/Caches/mailtriage` on macOS; on Linux
/// `$XDG_CACHE_HOME/mailtriage` when that is absolute, else
/// `~/.cache/mailtriage`.
pub fn cache_dir(home: Option<&OsStr>, xdg_cache_home: Option<&OsStr>) -> Option<PathBuf> {
    let home = home.filter(|h| !h.is_empty()).map(PathBuf::from)?;
    if cfg!(target_os = "macos") {
        return Some(home.join("Library/Caches/mailtriage"));
    }
    match xdg_cache_home.map(Path::new).filter(|p| p.is_absolute()) {
        Some(xdg) => Some(xdg.join("mailtriage")),
        None => Some(home.join(".cache/mailtriage")),
    }
}
