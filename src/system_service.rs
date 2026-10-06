//! The background service: a launchd agent (macOS) or a systemd user unit
//! (Linux) running `mailtriage watch` for one account. Files carry a marker;
//! mailtriage replaces or removes only files it wrote. Service files hold
//! absolute paths and never a secret.
use crate::{
    config, process,
    service::{err, Service},
};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub const LABEL_PREFIX: &str = "digital.wirdrei.mailtriage";
const PLIST_MARKER: &str = "<key>XMailtriageManaged</key>";
const UNIT_MARKER: &str = "# managed by mailtriage";
const TOOL_TIMEOUT: Duration = Duration::from_secs(30);
/// How long `install`/`uninstall` wait for launchd to drop a booted-out job.
const BOOTOUT_WAIT: Duration = Duration::from_secs(10);
const TOOL_MAX_OUTPUT: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manager {
    Launchd,
    Systemd,
}

impl Manager {
    pub fn name(self) -> &'static str {
        match self {
            Manager::Launchd => "launchd",
            Manager::Systemd => "systemd",
        }
    }
}

/// The `--key-store` values of the platform's own key stores, preferred
/// first: the Keychain with launchd, Secret Service or `pass` with systemd.
pub fn platform_key_stores(manager: Manager) -> &'static [&'static str] {
    match manager {
        Manager::Launchd => &["keychain"],
        Manager::Systemd => &["secret-service", "pass"],
    }
}

/// How to reach this user's service manager.
#[derive(Debug, Clone)]
pub struct Context {
    pub manager: Manager,
    pub home: PathBuf,
    /// `launchctl` or `systemctl`.
    pub tool: PathBuf,
    /// launchd's `gui/<uid>` domain.
    pub uid: u32,
}

impl Context {
    /// This platform's manager; other platforms exit 2.
    pub fn detect() -> Result<Self> {
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| err(2, "HOME is not set"))?;
        let (manager, tool) = if cfg!(target_os = "macos") {
            (
                Manager::Launchd,
                process::find_on_path("launchctl")
                    .unwrap_or_else(|| PathBuf::from("/bin/launchctl")),
            )
        } else if cfg!(target_os = "linux") {
            (
                Manager::Systemd,
                process::find_on_path("systemctl")
                    .ok_or_else(|| err(3, "systemctl is not on PATH; the service needs systemd"))?,
            )
        } else {
            return Err(err(
                2,
                "unsupported platform: the background service needs macOS (launchd) or Linux (systemd)",
            ));
        };
        Ok(Self {
            manager,
            home,
            tool,
            uid: current_uid(),
        })
    }

    pub fn unit_path(&self, account: &str) -> PathBuf {
        match self.manager {
            Manager::Launchd => self
                .home
                .join("Library/LaunchAgents")
                .join(format!("{}.plist", label(account))),
            Manager::Systemd => self
                .home
                .join(".config/systemd/user")
                .join(unit_name(account)),
        }
    }
}

#[cfg(unix)]
fn current_uid() -> u32 {
    unsafe extern "C" {
        fn getuid() -> u32;
    }
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { getuid() }
}

#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

pub fn label(account: &str) -> String {
    format!("{LABEL_PREFIX}.{account}")
}

pub fn unit_name(account: &str) -> String {
    format!("mailtriage-{account}.service")
}

/// What the service runs.
#[derive(Debug, Clone)]
pub struct Unit {
    pub account: String,
    /// The mailtriage executable, absolute.
    pub exe: PathBuf,
    /// The config, absolute.
    pub config: PathBuf,
    pub interval_seconds: u64,
    pub limit: usize,
    /// Where launchd writes stdout and stderr: `<config dir>/logs`.
    pub log_dir: PathBuf,
    /// PATH for the service, so key tools such as `pass` find their helpers.
    pub path_env: Option<String>,
}

impl Unit {
    pub fn arguments(&self) -> Vec<String> {
        vec![
            self.exe.display().to_string(),
            "watch".into(),
            "--config".into(),
            self.config.display().to_string(),
            "--account".into(),
            self.account.clone(),
            "--interval-seconds".into(),
            self.interval_seconds.to_string(),
            "--limit".into(),
            self.limit.to_string(),
            "--json".into(),
        ]
    }

    pub fn log_paths(&self) -> [PathBuf; 2] {
        [
            self.log_dir.join(format!("{}.log", self.account)),
            self.log_dir.join(format!("{}.err", self.account)),
        ]
    }
}

pub fn plist(unit: &Unit) -> String {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n",
    );
    s += &format!("  {PLIST_MARKER}\n  <true/>\n");
    s += &format!(
        "  <key>Label</key>\n  <string>{}</string>\n",
        xml(&label(&unit.account))
    );
    s += "  <key>ProgramArguments</key>\n  <array>\n";
    for arg in unit.arguments() {
        s += &format!("    <string>{}</string>\n", xml(&arg));
    }
    s += "  </array>\n";
    if let Some(path) = &unit.path_env {
        s += &format!(
            "  <key>EnvironmentVariables</key>\n  <dict>\n    <key>PATH</key>\n    <string>{}</string>\n  </dict>\n",
            xml(path)
        );
    }
    s += "  <key>RunAtLoad</key>\n  <true/>\n  <key>KeepAlive</key>\n  <dict>\n    <key>SuccessfulExit</key>\n    <false/>\n  </dict>\n  <key>ThrottleInterval</key>\n  <integer>30</integer>\n  <key>Umask</key>\n  <integer>63</integer>\n";
    let [out, errors] = unit.log_paths();
    s += &format!(
        "  <key>StandardOutPath</key>\n  <string>{}</string>\n  <key>StandardErrorPath</key>\n  <string>{}</string>\n",
        xml(&out.display().to_string()),
        xml(&errors.display().to_string())
    );
    s += "</dict>\n</plist>\n";
    s
}

pub fn systemd_unit(unit: &Unit) -> String {
    let exec: Vec<String> = unit.arguments().iter().map(|a| systemd_arg(a)).collect();
    let mut s = format!(
        "{UNIT_MARKER}\n[Unit]\nDescription=mailtriage watch for account {}\n\n[Service]\nType=simple\nExecStart={}\n",
        unit.account,
        exec.join(" ")
    );
    if let Some(path) = &unit.path_env {
        // Environment= expands % specifiers but not $ variables.
        s += &format!(
            "Environment=\"PATH={}\"\n",
            path.replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('%', "%%")
        );
    }
    s += "Restart=on-failure\nRestartSec=30\nUMask=0077\n\n[Install]\nWantedBy=default.target\n";
    s
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// One `ExecStart=` word: `%` and `$` doubled, quoted when it holds
/// whitespace, quotes, backslashes or `;`.
fn systemd_arg(arg: &str) -> String {
    let escaped = arg.replace('%', "%%").replace('$', "$$");
    if escaped.is_empty()
        || escaped
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '\\' | ';'))
    {
        format!("\"{}\"", escaped.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        escaped
    }
}

pub(crate) fn is_marked(manager: Manager, text: &str) -> bool {
    match manager {
        Manager::Launchd => text.contains(PLIST_MARKER),
        Manager::Systemd => text.lines().next() == Some(UNIT_MARKER),
    }
}

/// Exit 5 when a file mailtriage did not write is at `path`.
fn refuse_unmarked(ctx: &Context, path: &Path) -> Result<()> {
    match fs::read_to_string(path) {
        Ok(text) if is_marked(ctx.manager, &text) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(err(
            5,
            format!(
                "{} exists and was not written by mailtriage; move it away first",
                path.display()
            ),
        )),
    }
}

/// Runs the manager tool; anything but exit 0 is exit 3.
pub(crate) fn tool(ctx: &Context, args: &[&str]) -> Result<()> {
    tool_output(ctx, args).map(|_| ()).ok_or_else(|| {
        err(
            3,
            format!(
                "`{} {}` failed",
                ctx.tool.file_name().unwrap_or_default().to_string_lossy(),
                args.join(" ")
            ),
        )
    })
}

fn tool_output(ctx: &Context, args: &[&str]) -> Option<String> {
    process::run_bounded(&ctx.tool, args, TOOL_TIMEOUT, TOOL_MAX_OUTPUT)
        .ok()?
        .success()
        .map(|out| String::from_utf8_lossy(&out).into_owned())
}

fn target(ctx: &Context, account: &str) -> String {
    format!("gui/{}/{}", ctx.uid, label(account))
}

/// Unloads a loaded launchd job and waits until launchd reports it gone:
/// `bootout` returns before the job is fully removed, and a `bootstrap`
/// right after it fails ("Bootstrap failed: 5").
pub(crate) fn bootout(ctx: &Context, target: &str) -> Result<()> {
    if tool_output(ctx, &["print", target]).is_none() {
        return Ok(());
    }
    tool(ctx, &["bootout", target])?;
    let deadline = Instant::now() + BOOTOUT_WAIT;
    while tool_output(ctx, &["print", target]).is_some() {
        if Instant::now() >= deadline {
            return Err(err(
                3,
                format!(
                    "launchd still lists {target} {} s after `launchctl bootout`; run `launchctl bootout {target}` and retry",
                    BOOTOUT_WAIT.as_secs()
                ),
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

pub fn install(ctx: &Context, unit: &Unit) -> Result<Value> {
    let path = ctx.unit_path(&unit.account);
    refuse_unmarked(ctx, &path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    match ctx.manager {
        Manager::Launchd => {
            fs::create_dir_all(&unit.log_dir)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&unit.log_dir, fs::Permissions::from_mode(0o700))?;
            }
            let target = target(ctx, &unit.account);
            bootout(ctx, &target)?;
            fs::write(&path, plist(unit))?;
            // Clears a `service stop`, which disables the label.
            tool(ctx, &["enable", &target])?;
            let domain = format!("gui/{}", ctx.uid);
            tool(ctx, &["bootstrap", &domain, &path.display().to_string()])?;
        }
        Manager::Systemd => {
            fs::write(&path, systemd_unit(unit))?;
            let name = unit_name(&unit.account);
            tool(ctx, &["--user", "daemon-reload"])?;
            tool(ctx, &["--user", "enable", &name])?;
            tool(ctx, &["--user", "restart", &name])?;
        }
    }
    Ok(json!({
        "action": "installed",
        "manager": ctx.manager.name(),
        "account": unit.account,
        "unit_path": path,
        "log_paths": log_paths(ctx.manager, unit),
        "command": unit.arguments(),
    }))
}

pub fn uninstall(ctx: &Context, account: &str) -> Result<Value> {
    let path = ctx.unit_path(account);
    refuse_unmarked(ctx, &path)?;
    let existed = path.exists();
    match ctx.manager {
        Manager::Launchd => {
            bootout(ctx, &target(ctx, account))?;
            if existed {
                fs::remove_file(&path)?;
            }
        }
        Manager::Systemd => {
            if existed {
                tool(ctx, &["--user", "disable", "--now", &unit_name(account)])?;
                fs::remove_file(&path)?;
                tool(ctx, &["--user", "daemon-reload"])?;
            }
        }
    }
    Ok(json!({
        "action": if existed { "uninstalled" } else { "not_installed" },
        "manager": ctx.manager.name(),
        "account": account,
        "unit_path": path,
    }))
}

fn log_paths(manager: Manager, unit: &Unit) -> Vec<PathBuf> {
    match manager {
        Manager::Launchd => unit.log_paths().to_vec(),
        Manager::Systemd => vec![],
    }
}

/// What the manager reports for a job.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ManagerState {
    pub loaded: bool,
    pub running: bool,
    pub pid: Option<u32>,
    pub last_exit_status: Option<i64>,
}

/// `launchctl print` output of a loaded job; only top-level properties
/// (one tab deep) count, nested blocks repeat keys such as `state`.
pub fn parse_launchctl_print(text: &str) -> ManagerState {
    let mut state = ManagerState {
        loaded: true,
        ..ManagerState::default()
    };
    for line in text.lines() {
        let Some(property) = line.strip_prefix('\t').filter(|l| !l.starts_with('\t')) else {
            continue;
        };
        let Some((key, value)) = property.split_once(" = ") else {
            continue;
        };
        match key {
            "state" => state.running = value == "running",
            "pid" => state.pid = value.parse().ok(),
            "last exit code" => state.last_exit_status = leading_integer(value),
            _ => {}
        }
    }
    state
}

/// The integer a value starts with: `78` of `78: EX_CONFIG`; `None` for
/// `(never exited)`.
fn leading_integer(value: &str) -> Option<i64> {
    let value = value.trim_start();
    let digits = value.strip_prefix('-').unwrap_or(value);
    let end = value.len() - digits.len()
        + digits
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(digits.len());
    value[..end].parse().ok()
}

pub fn parse_systemctl_show(text: &str) -> ManagerState {
    let get = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
    };
    ManagerState {
        loaded: get("LoadState") == Some("loaded"),
        running: get("ActiveState") == Some("active") && get("SubState") == Some("running"),
        pid: get("MainPID")
            .and_then(|p| p.parse().ok())
            .filter(|&p| p > 0),
        last_exit_status: get("ExecMainStatus").and_then(|s| s.parse().ok()),
    }
}

/// `service status` manager fields; `None` means no supported manager.
pub fn status(ctx: Option<&Context>, account: &str, log_dir: &Path) -> Value {
    let Some(ctx) = ctx else {
        return json!({"manager":"none","installed":false,"loaded":false,"running":false,"pid":null,"last_exit_status":null,"unit_path":null,"log_paths":[]});
    };
    let path = ctx.unit_path(account);
    let installed = fs::read_to_string(&path).is_ok_and(|text| is_marked(ctx.manager, &text));
    let state = match ctx.manager {
        Manager::Launchd => tool_output(ctx, &["print", &target(ctx, account)])
            .map(|text| parse_launchctl_print(&text)),
        Manager::Systemd => tool_output(
            ctx,
            &[
                "--user",
                "show",
                &unit_name(account),
                "--property=LoadState,ActiveState,SubState,MainPID,ExecMainStatus",
            ],
        )
        .map(|text| parse_systemctl_show(&text)),
    }
    .unwrap_or_default();
    let log_paths: Vec<PathBuf> = match ctx.manager {
        Manager::Launchd => vec![
            log_dir.join(format!("{account}.log")),
            log_dir.join(format!("{account}.err")),
        ],
        Manager::Systemd => vec![],
    };
    json!({
        "manager": ctx.manager.name(),
        "installed": installed,
        "loaded": state.loaded,
        "running": state.running,
        "pid": state.pid,
        "last_exit_status": state.last_exit_status,
        "unit_path": path,
        "log_paths": log_paths,
    })
}

/// The unit for `account` of the config at `config_path`.
pub fn unit_for(
    config_path: &Path,
    account: &str,
    interval_seconds: u64,
    limit: usize,
) -> Result<Unit> {
    if !config::valid_account_name(account) {
        return Err(err(
            2,
            format!(
                "account name {account:?} cannot name a service; use 1 to 64 letters, digits, - or _"
            ),
        ));
    }
    let config = fs::canonicalize(config_path)?;
    let log_dir = config
        .parent()
        .map_or_else(|| PathBuf::from("logs"), |dir| dir.join("logs"));
    Ok(Unit {
        account: account.to_owned(),
        exe: std::env::current_exe()?,
        config,
        interval_seconds,
        limit,
        log_dir,
        path_env: std::env::var("PATH").ok().filter(|p| !p.is_empty()),
    })
}

/// `service install` for an account of `service`'s config, with a note
/// when the key comes from an environment variable the service lacks.
pub fn install_account(
    service: &Service,
    config_path: &Path,
    account: &str,
    interval_seconds: u64,
    limit: usize,
    ctx: &Context,
) -> Result<Value> {
    if !service.config.accounts.contains_key(account) {
        return Err(err(2, "unknown account"));
    }
    let unit = unit_for(config_path, account, interval_seconds, limit)?;
    let mut out = install(ctx, &unit)?;
    let provider = &service.config.provider;
    if provider.kind == "openrouter" && provider.api_key_command.is_none() {
        out["note"] = json!(env_key_note(ctx.manager, &unit, &provider.api_key_env));
    }
    Ok(out)
}

/// Why a service whose key comes from the variable `env` will not find it,
/// and the setup command that moves the key into the platform's own store.
fn env_key_note(manager: Manager, unit: &Unit, env: &str) -> String {
    let stores = platform_key_stores(manager);
    let setup = crate::setup::shell_line(&[
        OsStr::new("mailtriage"),
        OsStr::new("setup"),
        OsStr::new("--update"),
        OsStr::new("--config"),
        unit.config.as_os_str(),
        OsStr::new("--account"),
        OsStr::new(&unit.account),
        OsStr::new("--key-store"),
        OsStr::new(stores[0]),
    ]);
    let alternatives: Vec<String> = stores[1..]
        .iter()
        .chain(&["command"])
        .map(|s| format!("`--key-store {s}`"))
        .collect();
    let file = match manager {
        Manager::Launchd => "plist",
        Manager::Systemd => "unit",
    };
    format!(
        "The key comes from {env}, which the service does not inherit. Store it with `{setup}` (or {}). Do not put the key into the {file} yourself: that file is readable, and `service install` rewrites it.",
        alternatives.join(", ")
    )
}

/// `service status`: manager fields plus the last sync pass from the state
/// database, which works without any service.
pub fn status_account(
    service: &Service,
    config_path: &Path,
    account: &str,
    ctx: Option<&Context>,
) -> Result<Value> {
    if !service.config.accounts.contains_key(account) {
        return Err(err(2, "unknown account"));
    }
    let config = fs::canonicalize(config_path)?;
    let log_dir = config
        .parent()
        .map_or_else(|| PathBuf::from("logs"), |dir| dir.join("logs"));
    let mut out = status(ctx, account, &log_dir);
    out["account"] = json!(account);
    out["last_pass"] = service.store.heartbeat(account)?.unwrap_or(Value::Null);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn systemd_args_escape_specifiers() {
        assert_eq!(systemd_arg("plain"), "plain");
        assert_eq!(systemd_arg("50%$x"), "50%%$$x");
        assert_eq!(systemd_arg("a b\"c"), "\"a b\\\"c\"");
        assert_eq!(systemd_arg(""), "\"\"");
    }

    #[test]
    fn launchctl_print_reads_only_top_level_properties() {
        let text = "gui/501/x = {\n\tstate = waiting\n\tlast exit code = (never exited)\n\tendpoints = {\n\t\tstate = running\n\t\tpid = 9\n\t}\n}\n";
        let state = parse_launchctl_print(text);
        assert!(state.loaded && !state.running);
        assert_eq!((state.pid, state.last_exit_status), (None, None));
    }

    /// Final review M1: launchd prints a code with its name, such as
    /// `78: EX_CONFIG`.
    #[test]
    fn launchctl_last_exit_code_is_its_leading_integer() {
        for (value, expected) in [
            ("0", Some(0)),
            ("78: EX_CONFIG", Some(78)),
            ("1: Operation not permitted", Some(1)),
            ("(never exited)", None),
        ] {
            let text = format!("gui/501/x = {{\n\tlast exit code = {value}\n}}\n");
            assert_eq!(
                parse_launchctl_print(&text).last_exit_status,
                expected,
                "{value}"
            );
        }
    }

    #[test]
    fn systemctl_show_parsing() {
        let state = parse_systemctl_show(
            "LoadState=loaded\nActiveState=activating\nSubState=auto-restart\nMainPID=0\nExecMainStatus=3\n",
        );
        assert!(state.loaded && !state.running);
        assert_eq!((state.pid, state.last_exit_status), (None, Some(3)));
    }
}
