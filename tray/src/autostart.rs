//! Starting the tray at login: a launchd agent on macOS, an XDG autostart
//! entry on Linux. Both record the absolute CLI and config, and carry a
//! marker; a file without it is never changed.
use crate::{
    args::{Args, AutostartAction},
    cli::{self, Invocation},
    paths::{self, Problem},
};
use serde::Serialize;
use serde_json::json;
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

pub const LABEL: &str = "digital.wirdrei.mailtriage-tray";
const PLIST_MARKER: &str = "<key>XMailtriageManaged</key>";
const DESKTOP_MARKER: &str = "# managed by mailtriage";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Linux,
}

/// Where the login item lives and what it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Env {
    pub platform: Platform,
    pub home: PathBuf,
    pub xdg_config_home: Option<OsString>,
    pub launchctl: PathBuf,
    pub uid: u32,
    /// Recorded in the plist, as `service install` does.
    pub path_env: Option<String>,
    /// This program's canonical path.
    pub tray: PathBuf,
}

/// A failure with its exit code: 3 when a file cannot be written or the
/// config cannot be resolved, 5 for a file mailtriage did not write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub code: i32,
    pub message: String,
}

fn error(code: i32, message: impl Into<String>) -> Error {
    Error {
        code,
        message: message.into(),
    }
}

impl Env {
    pub fn detect() -> Result<Self, Error> {
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| error(3, "HOME is not set"))?;
        let tray = std::env::current_exe()
            .and_then(fs::canonicalize)
            .map_err(|e| error(3, format!("cannot find this program: {e}")))?;
        let launchctl = std::env::var_os("PATH")
            .and_then(|path| {
                std::env::split_paths(&path)
                    .map(|d| d.join("launchctl"))
                    .find(|p| p.is_file())
            })
            .unwrap_or_else(|| PathBuf::from("/bin/launchctl"));
        Ok(Self {
            platform: if cfg!(target_os = "macos") {
                Platform::MacOs
            } else {
                Platform::Linux
            },
            home,
            xdg_config_home: std::env::var_os("XDG_CONFIG_HOME"),
            launchctl,
            // SAFETY: getuid has no preconditions and cannot fail.
            uid: unsafe { libc::getuid() },
            path_env: std::env::var("PATH").ok().filter(|p| !p.is_empty()),
            tray,
        })
    }
}

/// `autostart`'s result object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub enabled: bool,
    pub path: PathBuf,
    pub config: Option<PathBuf>,
    pub mailtriage: Option<PathBuf>,
}

/// The login item's path.
pub fn path(env: &Env) -> PathBuf {
    match env.platform {
        Platform::MacOs => env
            .home
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist")),
        Platform::Linux => {
            let base = env
                .xdg_config_home
                .as_ref()
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| env.home.join(".config"));
            base.join("autostart/mailtriage-tray.desktop")
        }
    }
}

fn arguments(env: &Env, cli: &Path, config: &Path) -> Vec<String> {
    vec![
        env.tray.display().to_string(),
        "--config".into(),
        config.display().to_string(),
        "--mailtriage".into(),
        cli.display().to_string(),
    ]
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The launchd agent: `RunAtLoad`, restarted after a crash but not after
/// Quit, only in the graphical session.
pub fn plist(env: &Env, cli: &Path, config: &Path) -> String {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n",
    );
    s += &format!("  {PLIST_MARKER}\n  <true/>\n");
    s += &format!("  <key>Label</key>\n  <string>{LABEL}</string>\n");
    s += "  <key>ProgramArguments</key>\n  <array>\n";
    for arg in arguments(env, cli, config) {
        s += &format!("    <string>{}</string>\n", xml(&arg));
    }
    s += "  </array>\n";
    if let Some(path) = &env.path_env {
        s += &format!(
            "  <key>EnvironmentVariables</key>\n  <dict>\n    <key>PATH</key>\n    <string>{}</string>\n  </dict>\n",
            xml(path)
        );
    }
    s += "  <key>RunAtLoad</key>\n  <true/>\n  <key>KeepAlive</key>\n  <dict>\n    <key>SuccessfulExit</key>\n    <false/>\n  </dict>\n  <key>LimitLoadToSessionType</key>\n  <string>Aqua</string>\n</dict>\n</plist>\n";
    s
}

/// One `Exec=` argument per the Desktop Entry specification: `%` doubled,
/// quoted when it holds a reserved character (inside quotes `"`, `` ` ``,
/// `$` and `\` are escaped), then the string-level escape of `\`.
pub fn desktop_arg(arg: &str) -> String {
    let escaped = arg.replace('%', "%%");
    let reserved = |c: char| c.is_whitespace() || "\"'\\><~|&;$*?#()`".contains(c);
    let quoted = if escaped.is_empty() || escaped.chars().any(reserved) {
        let inner: String = escaped
            .chars()
            .map(|c| match c {
                '"' | '`' | '$' | '\\' => format!("\\{c}"),
                _ => c.to_string(),
            })
            .collect();
        format!("\"{inner}\"")
    } else {
        escaped
    };
    quoted.replace('\\', "\\\\")
}

/// The XDG autostart entry.
pub fn desktop(env: &Env, cli: &Path, config: &Path) -> String {
    let exec: Vec<String> = arguments(env, cli, config)
        .iter()
        .map(|a| desktop_arg(a))
        .collect();
    format!(
        "{DESKTOP_MARKER}\n[Desktop Entry]\nType=Application\nName=mailtriage\nExec={}\nNoDisplay=true\n",
        exec.join(" ")
    )
}

/// The arguments of an `Exec=` value: the inverse of `desktop_arg`.
pub fn desktop_arguments(value: &str) -> Option<Vec<String>> {
    let mut unescaped = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            unescaped.push(chars.next()?);
        } else {
            unescaped.push(c);
        }
    }
    let mut args = vec![];
    let mut chars = unescaped.chars().peekable();
    loop {
        while chars.peek() == Some(&' ') {
            chars.next();
        }
        let Some(&first) = chars.peek() else {
            break;
        };
        let mut arg = String::new();
        if first == '"' {
            chars.next();
            loop {
                match chars.next()? {
                    '\\' => arg.push(chars.next()?),
                    '"' => break,
                    c => arg.push(c),
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c == ' ' {
                    break;
                }
                arg.push(c);
                chars.next();
            }
        }
        args.push(arg.replace("%%", "%"));
    }
    Some(args)
}

fn plist_arguments(text: &str) -> Option<Vec<String>> {
    let rest = &text[text.find("<key>ProgramArguments</key>")?..];
    let array = &rest[..rest.find("</array>")?];
    Some(
        array
            .split("<string>")
            .skip(1)
            .filter_map(|s| s.split("</string>").next())
            .map(|s| {
                s.replace("&lt;", "<")
                    .replace("&gt;", ">")
                    .replace("&amp;", "&")
            })
            .collect(),
    )
}

fn value_after(args: &[String], flag: &str) -> Option<PathBuf> {
    let at = args.iter().position(|a| a == flag)?;
    args.get(at + 1).map(PathBuf::from)
}

fn marked(env: &Env, text: &str) -> bool {
    match env.platform {
        Platform::MacOs => text.contains(PLIST_MARKER),
        Platform::Linux => text.lines().next() == Some(DESKTOP_MARKER),
    }
}

/// The login item's text when it is ours; `None` when there is none;
/// exit 5 for a file mailtriage did not write.
fn read(env: &Env) -> Result<Option<String>, Error> {
    let path = path(env);
    match fs::read_to_string(&path) {
        Ok(text) if marked(env, &text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        _ => Err(error(
            5,
            format!(
                "{} exists and was not written by mailtriage; move it away first",
                path.display()
            ),
        )),
    }
}

fn launchctl(env: &Env, args: &[&str]) -> Option<String> {
    let finished = cli::run(
        &Invocation {
            program: env.launchctl.clone(),
            args: args.iter().map(|a| a.to_string()).collect(),
        },
        Duration::from_secs(30),
    );
    (finished.exit_code() == Some(0)).then_some(finished.stdout)
}

/// Whether the login item exists, is ours, and (macOS) its label is not
/// disabled. A failed `print-disabled` counts as not disabled, launchd's
/// default.
pub fn status(env: &Env) -> Result<Entry, Error> {
    let text = read(env)?;
    let args = text.as_deref().and_then(|t| match env.platform {
        Platform::MacOs => plist_arguments(t),
        Platform::Linux => t
            .lines()
            .find_map(|l| l.strip_prefix("Exec="))
            .and_then(desktop_arguments),
    });
    let disabled = env.platform == Platform::MacOs
        && text.is_some()
        && launchctl(env, &["print-disabled", &format!("gui/{}", env.uid)]).is_some_and(|out| {
            out.lines().any(|l| {
                let l = l.trim();
                l == format!("\"{LABEL}\" => disabled") || l == format!("\"{LABEL}\" => true")
            })
        });
    Ok(Entry {
        enabled: text.is_some() && !disabled,
        path: path(env),
        config: args.as_deref().and_then(|a| value_after(a, "--config")),
        mailtriage: args.as_deref().and_then(|a| value_after(a, "--mailtriage")),
    })
}

/// Writes the login item for `cli` and `config` (both absolute). On macOS it
/// also clears a disabled label; it never loads the job, so no second tray
/// starts.
pub fn enable(env: &Env, cli: &Path, config: &Path) -> Result<Entry, Error> {
    read(env)?;
    let path = path(env);
    let text = match env.platform {
        Platform::MacOs => plist(env, cli, config),
        Platform::Linux => desktop(env, cli, config),
    };
    let write = || -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, &text)?;
        fs::rename(&tmp, &path)
    };
    write().map_err(|e| error(3, format!("cannot write {}: {e}", path.display())))?;
    if env.platform == Platform::MacOs {
        let target = format!("gui/{}/{LABEL}", env.uid);
        launchctl(env, &["enable", &target])
            .ok_or_else(|| error(3, format!("`launchctl enable {target}` failed")))?;
    }
    status(env)
}

/// Deletes the login item; a running tray keeps running.
pub fn disable(env: &Env) -> Result<Entry, Error> {
    if read(env)?.is_some() {
        let path = path(env);
        fs::remove_file(&path)
            .map_err(|e| error(3, format!("cannot remove {}: {e}", path.display())))?;
    }
    status(env)
}

/// The paths `enable` records, resolved as the tray resolves them.
pub fn resolve(args: &Args, env: &paths::Env) -> Result<paths::Resolved, Error> {
    paths::resolve(args.mailtriage.as_deref(), args.config.as_deref(), env).map_err(|p| match p {
        Problem::CliMissing(_) => error(3, format!("{}; pass --mailtriage PATH", p.text())),
        Problem::NotSetUp(Some(path)) => error(3, format!("config not found: {}", path.display())),
        Problem::NotSetUp(None) => error(
            3,
            "cannot find the config: mailtriage is not set up; pass --config PATH",
        ),
        Problem::Status(f) => error(
            3,
            format!("cannot find the config: {}; pass --config PATH", f.message),
        ),
    })
}

/// `mailtriage-tray autostart …`: prints the result and returns the exit code.
pub fn run(args: &Args, action: AutostartAction, json_mode: bool) -> i32 {
    let result = Env::detect().and_then(|env| match action {
        AutostartAction::Status => status(&env),
        AutostartAction::Disable => disable(&env),
        AutostartAction::Enable => {
            let resolved = resolve(args, &paths::Env::current())?;
            enable(&env, &resolved.cli, &resolved.config)
        }
    });
    match result {
        Ok(entry) => {
            let value = json!({"schema_version": 1, "autostart": entry});
            if json_mode {
                println!("{value}");
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&value).unwrap_or_default()
                );
            }
            0
        }
        Err(e) => {
            if json_mode {
                println!(
                    "{}",
                    json!({"schema_version": 1, "error": {"code": e.code, "message": e.message}})
                );
            } else {
                eprintln!("mailtriage-tray: {}", e.message);
            }
            e.code
        }
    }
}
