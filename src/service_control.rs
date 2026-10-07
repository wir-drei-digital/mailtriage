//! What `service status`, `start` and `stop` add to `system_service`: the
//! config a service runs with, whether its manager starts it again, and the
//! fields the tray reads. Nothing here writes a service file.
use crate::{
    config, filing,
    process::{self, Ending},
    service::{err, err_kind, ErrorKind, Service},
    setup::shell_line,
    system_service::{
        self, label, parse_launchctl_print, parse_systemctl_show, tool, unit_name, Context, Manager,
    },
};
// The updater's decoders: they invert exactly what the writers write and
// fail closed (`None`) on anything else.
use crate::update::service_files::{plist_arguments, unit_arguments};
use anyhow::Result;
use fs2::FileExt;
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

/// Where the running processes' command lines are read on Linux.
pub const PROC_ROOT: &str = "/proc";
const TOOL_TIMEOUT: Duration = Duration::from_secs(30);
const TOOL_MAX_OUTPUT: usize = 1024 * 1024;

/// The manager tool's exit code and stdout, whatever the code; `None` when
/// it could not start, timed out, printed too much or was killed.
fn query(ctx: &Context, args: &[&str]) -> Option<(i32, String)> {
    let out = process::run_bounded(&ctx.tool, args, TOOL_TIMEOUT, TOOL_MAX_OUTPUT).ok()?;
    match out.ending {
        Ending::Exited(status) => Some((
            status.code()?,
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )),
        _ => None,
    }
}

/// `--config` and `--interval-seconds` of a `watch` command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchArgs {
    pub config: PathBuf,
    pub interval_seconds: Option<u64>,
}

/// The values after `--config` and `--interval-seconds` in `args`; `None`
/// when `--config` is missing, has no value or appears twice.
pub fn watch_args(args: &[String]) -> Option<WatchArgs> {
    let value = |flag: &str| -> Option<&String> {
        let mut found = args.iter().enumerate().filter(|(_, a)| *a == flag);
        let (at, _) = found.next()?;
        if found.next().is_some() {
            return None;
        }
        args.get(at + 1)
    };
    Some(WatchArgs {
        config: PathBuf::from(value("--config")?),
        interval_seconds: value("--interval-seconds").and_then(|v| v.parse().ok()),
    })
}

/// Splits systemd command-line words: a bare word holds no quote, backslash
/// or space; a quoted word ends at an unescaped `"` and `\x` stands for `x`.
fn split_words(line: &str) -> Option<Vec<String>> {
    let mut words = vec![];
    let mut chars = line.chars().peekable();
    loop {
        while chars.peek() == Some(&' ') {
            chars.next();
        }
        let Some(&first) = chars.peek() else {
            return Some(words);
        };
        let mut word = String::new();
        if first == '"' {
            chars.next();
            loop {
                match chars.next()? {
                    '\\' => word.push(chars.next()?),
                    '"' => break,
                    c => word.push(c),
                }
            }
            if !matches!(chars.peek(), None | Some(' ')) {
                return None;
            }
        } else {
            while let Some(&c) = chars.peek() {
                match c {
                    ' ' => break,
                    '"' | '\\' => return None,
                    _ => word.push(c),
                }
                chars.next();
            }
        }
        words.push(word);
    }
}

/// The `arguments = { … }` block of `launchctl print`: one argument per
/// line, two tabs deep.
pub fn launchctl_arguments(text: &str) -> Option<Vec<String>> {
    let mut lines = text.lines().skip_while(|l| *l != "\targuments = {");
    lines.next()?;
    let mut args = vec![];
    for line in lines {
        if line == "\t}" {
            return Some(args);
        }
        args.push(line.strip_prefix("\t\t")?.to_owned());
    }
    None
}

/// `argv[]` of `systemctl show -p ExecStart`. Only a command line shaped
/// like the one `service install` writes counts, so a path that `show`
/// printed without quotes cannot be mistaken for a shorter one. systemd
/// resolved `%%` when it loaded the unit but keeps `$$` until it runs the
/// command, so `$$` is undone here and `%%` is not.
pub fn exec_start_arguments(value: &str) -> Option<Vec<String>> {
    let rest = &value[value.find("argv[]=")? + "argv[]=".len()..];
    let end = rest.find(" ; ").unwrap_or(rest.len());
    let args: Vec<String> = split_words(&rest[..end])?
        .into_iter()
        .map(|w| w.replace("$$", "$"))
        .collect();
    let shaped = args.len() == 11
        && args[1] == "watch"
        && args[2] == "--config"
        && args[4] == "--account"
        && args[6] == "--interval-seconds";
    shaped.then_some(args)
}

/// The command line of process `pid`, read between two readings of its
/// start time that must agree, so a PID reused in between is never read.
pub fn process_arguments(proc_root: &Path, pid: u32) -> Option<Vec<String>> {
    process_arguments_with(proc_root, pid, |path| fs::read(path).ok())
}

fn process_arguments_with(
    proc_root: &Path,
    pid: u32,
    mut read: impl FnMut(&Path) -> Option<Vec<u8>>,
) -> Option<Vec<String>> {
    let dir = proc_root.join(pid.to_string());
    let before = start_time(&read(&dir.join("stat"))?)?;
    let cmdline = read(&dir.join("cmdline"))?;
    let after = start_time(&read(&dir.join("stat"))?)?;
    if before != after {
        return None;
    }
    let mut parts: Vec<&[u8]> = cmdline.split(|b| *b == 0).collect();
    if parts.last().is_some_and(|p| p.is_empty()) {
        parts.pop();
    }
    parts
        .into_iter()
        .map(|p| String::from_utf8(p.to_vec()).ok())
        .collect()
}

/// Field 22 of `/proc/<pid>/stat`, counted after the parenthesised name.
fn start_time(stat: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(stat).ok()?;
    text[text.rfind(')')? + 1..]
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

/// `enabled` and `enablement` from `launchctl print-disabled gui/<uid>`
/// (`None` when the query failed): a label listed as `disabled` or `true`
/// is disabled; `enabled`, `false` or not listed is enabled.
pub fn launchd_enablement(output: Option<&str>, label: &str) -> (Option<bool>, String) {
    let Some(output) = output else {
        return (None, "unknown".into());
    };
    let key = format!("\"{label}\" =>");
    match output
        .lines()
        .find_map(|l| l.trim().strip_prefix(&key).map(str::trim))
    {
        Some("disabled" | "true") => (Some(false), "disabled".into()),
        Some("enabled" | "false") | None => (Some(true), "enabled".into()),
        Some(_) => (None, "unknown".into()),
    }
}

/// `enabled` and `enablement` from `systemctl --user is-enabled` (any exit
/// code; `None` when it could not run).
pub fn systemd_enablement(output: Option<&str>) -> (Option<bool>, String) {
    let Some(state) = output
        .and_then(|o| o.lines().next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return (None, "unknown".into());
    };
    let enabled = match state {
        "enabled" | "enabled-runtime" => Some(true),
        "disabled" | "masked" | "masked-runtime" => Some(false),
        _ => None,
    };
    (enabled, state.to_owned())
}

/// What the manager and the unit file say about one account's service.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inspection {
    pub installed: bool,
    /// `None` when the manager query failed.
    pub loaded: Option<bool>,
    pub running: bool,
    pub pid: Option<u32>,
    /// The `--config` the service runs with (or would start with).
    pub service_config: Option<PathBuf>,
    /// The `--config` of the unit file.
    pub file_config: Option<PathBuf>,
    /// Each config the running process, the loaded definition and the file
    /// name, `None` where one cannot be read or decoded.
    pub named: Vec<Option<PathBuf>>,
    /// systemd's `NeedDaemonReload`; `None` on launchd or when unknown.
    pub needs_daemon_reload: Option<bool>,
    pub enabled: Option<bool>,
    pub enablement: String,
    pub interval_seconds: Option<u64>,
}

fn not_installed() -> Inspection {
    Inspection {
        enabled: Some(false),
        enablement: "not_installed".into(),
        ..Inspection::default()
    }
}

fn show_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines()
        .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
}

/// Reads the account's unit file and asks the manager about its job.
pub fn inspect(ctx: &Context, account: &str, proc_root: &Path) -> Inspection {
    let Some(text) = fs::read_to_string(ctx.unit_path(account))
        .ok()
        .filter(|text| system_service::is_marked(ctx.manager, text))
    else {
        return not_installed();
    };
    let file = match ctx.manager {
        Manager::Launchd => plist_arguments(&text),
        Manager::Systemd => unit_arguments(&text),
    }
    .and_then(|args| watch_args(&args));
    let file_config = file.as_ref().map(|a| a.config.clone());
    let mut found = Inspection {
        installed: true,
        file_config: file_config.clone(),
        interval_seconds: file.and_then(|a| a.interval_seconds),
        ..Inspection::default()
    };
    let config_of = |args: Option<Vec<String>>| args.and_then(|a| watch_args(&a)).map(|a| a.config);
    match ctx.manager {
        Manager::Launchd => {
            let target = format!("gui/{}/{}", ctx.uid, label(account));
            match query(ctx, &["print", &target]) {
                Some((0, out)) => {
                    let state = parse_launchctl_print(&out);
                    found.loaded = Some(true);
                    (found.running, found.pid) = (state.running, state.pid);
                    found.service_config = config_of(launchctl_arguments(&out));
                    found.named = vec![found.service_config.clone(), file_config];
                }
                Some((113, _)) => {
                    found.loaded = Some(false);
                    found.service_config = file_config.clone();
                    found.named = vec![file_config];
                }
                _ => found.named = vec![None, file_config],
            }
            let disabled = query(ctx, &["print-disabled", &format!("gui/{}", ctx.uid)])
                .filter(|(code, _)| *code == 0)
                .map(|(_, out)| out);
            (found.enabled, found.enablement) =
                launchd_enablement(disabled.as_deref(), &label(account));
        }
        Manager::Systemd => {
            let unit = unit_name(account);
            let properties =
                "--property=LoadState,ActiveState,SubState,MainPID,ExecStart,NeedDaemonReload";
            match query(ctx, &["--user", "show", &unit, properties]) {
                Some((0, out)) => {
                    let state = parse_systemctl_show(&out);
                    found.loaded = Some(state.loaded);
                    (found.running, found.pid) = (state.running, state.pid);
                    found.needs_daemon_reload = match show_value(&out, "NeedDaemonReload") {
                        Some("yes") => Some(true),
                        Some("no") => Some(false),
                        _ => None,
                    };
                    if !state.loaded {
                        found.service_config = file_config.clone();
                        found.named = vec![file_config];
                    } else {
                        let loaded =
                            config_of(show_value(&out, "ExecStart").and_then(exec_start_arguments));
                        if state.running {
                            let running = config_of(
                                state.pid.and_then(|pid| process_arguments(proc_root, pid)),
                            );
                            found.service_config = running.clone();
                            found.named = vec![running, loaded, file_config];
                        } else {
                            found.service_config = loaded.clone();
                            found.named = vec![loaded, file_config];
                        }
                    }
                }
                _ => found.named = vec![None, file_config],
            }
            let enabled = query(ctx, &["--user", "is-enabled", &unit]).map(|(_, out)| out);
            (found.enabled, found.enablement) = systemd_enablement(enabled.as_deref());
        }
    }
    found
}

fn same_config(path: &Path, resolved: &Path) -> bool {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()) == resolved
}

/// The first config the service or its file names that is not `resolved`.
pub fn other_config(found: &Inspection, resolved: &Path) -> Option<PathBuf> {
    found
        .named
        .iter()
        .flatten()
        .find(|p| !same_config(p, resolved))
        .cloned()
}

/// `true` when every config the service and its file name is `resolved`
/// (canonical) and systemd needs no reload; `false` when one names another
/// config; `None` when one cannot be established. `true` when not installed.
pub fn config_matches(found: &Inspection, manager: Manager, resolved: &Path) -> Option<bool> {
    if !found.installed {
        return Some(true);
    }
    if other_config(found, resolved).is_some() {
        return Some(false);
    }
    if found.named.iter().any(Option::is_none)
        || (manager == Manager::Systemd && found.needs_daemon_reload != Some(false))
    {
        return None;
    }
    Some(true)
}

/// `service status`: the resolved config and one service object per
/// account of the config, sorted by name; with `account`, only that one.
pub fn status(
    service: &Service,
    config_path: &Path,
    account: Option<&str>,
    ctx: Option<&Context>,
    proc_root: &Path,
) -> Result<Value> {
    let config = fs::canonicalize(config_path)?;
    let one = |name: &str| -> Result<Value> {
        let mut out = system_service::status_account(service, config_path, name, ctx)?;
        let settings = &service.config.accounts[name];
        let found = ctx.map_or_else(not_installed, |ctx| inspect(ctx, name, proc_root));
        let matches = ctx.map_or(Some(true), |ctx| {
            config_matches(&found, ctx.manager, &config)
        });
        out["service_config"] = json!(found.service_config);
        out["file_config"] = json!(found.file_config);
        out["config_matches"] = json!(matches);
        out["enabled"] = json!(found.enabled);
        out["enablement"] = json!(found.enablement);
        out["interval_seconds"] = json!(found.interval_seconds);
        out["filing_mode"] = json!(filing::mode_str(settings.filing.mode));
        out["identity"] = json!(settings.identity);
        if ctx.is_some_and(|ctx| ctx.manager == Manager::Systemd) {
            out["needs_daemon_reload"] = json!(found.needs_daemon_reload);
        }
        Ok(out)
    };
    Ok(match account {
        Some(name) => json!({"schema_version":1,"config":config,"service":one(name)?}),
        None => {
            let services = service
                .config
                .accounts
                .keys()
                .map(|name| one(name))
                .collect::<Result<Vec<_>>>()?;
            json!({"schema_version":1,"config":config,"services":services})
        }
    })
}

/// How long a service command waits for another one on the same account.
pub const SERVICE_LOCK_WAIT: Duration = Duration::from_secs(30);
/// How long `service start` waits for the job to run.
pub const START_WAIT: Duration = Duration::from_secs(5);

/// Where the service locks live: derived from `HOME` alone (never
/// `XDG_CACHE_HOME`), so commands for one account from different configs or
/// environments serialize.
pub fn lock_dir(ctx: &Context) -> PathBuf {
    match ctx.manager {
        Manager::Launchd => ctx.home.join("Library/Caches/mailtriage"),
        Manager::Systemd => ctx.home.join(".cache/mailtriage"),
    }
}

/// The exclusive lock on `service-<label>.lock`, held from reading the
/// config through the last manager call of `install`, `uninstall`, `start`
/// or `stop`.
pub struct ServiceLock {
    _file: File,
}

/// Takes the account's service lock, waiting up to `wait` for another
/// service command (exit 5, `service_busy`). The name is checked first: it
/// becomes part of a file name.
pub fn lock(ctx: &Context, account: &str, wait: Duration) -> Result<ServiceLock> {
    if !config::valid_account_name(account) {
        return Err(err(
            2,
            format!(
                "account name {account:?} cannot name a service; use 1 to 64 letters, digits, - or _"
            ),
        ));
    }
    let dir = lock_dir(ctx);
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&dir)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(format!("service-{}.lock", label(account))))?;
    let deadline = Instant::now() + wait;
    loop {
        if file.try_lock_exclusive().is_ok() {
            return Ok(ServiceLock { _file: file });
        }
        if Instant::now() >= deadline {
            return Err(err_kind(
                5,
                ErrorKind::ServiceBusy,
                format!("another service command for account {account} is running; try again"),
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// The installed service of `account`, refused unless every config it names
/// is the resolved one: `start` and `stop` never act on a job whose config
/// they cannot establish.
fn checked(
    ctx: &Context,
    service: &Service,
    config_path: &Path,
    account: &str,
    proc_root: &Path,
) -> Result<Inspection> {
    if !service.config.accounts.contains_key(account) {
        return Err(err(2, "unknown account"));
    }
    let found = inspect(ctx, account, proc_root);
    if !found.installed {
        return Err(err(
            2,
            format!(
                "service for account {account} is not installed; run mailtriage service install --account {account}"
            ),
        ));
    }
    let resolved = fs::canonicalize(config_path)?;
    match config_matches(&found, ctx.manager, &resolved) {
        Some(true) => Ok(found),
        Some(false) => {
            let other = other_config(&found, &resolved).unwrap_or_default();
            Err(err_kind(
                5,
                ErrorKind::ServiceConfigMismatch,
                format!(
                    "service for account {account} runs config {}; pass {}",
                    other.display(),
                    shell_line(&[Path::new("--config"), &other])
                ),
            ))
        }
        None => Err(err_kind(
            5,
            ErrorKind::ServiceConfigUnknown,
            format!(
                "cannot tell which config the service for account {account} runs; try again, or reinstall it with mailtriage service install --account {account}"
            ),
        )),
    }
}

fn target(ctx: &Context, account: &str) -> String {
    format!("gui/{}/{}", ctx.uid, label(account))
}

/// Polls until the job runs: `launchctl print` shows a PID, or
/// `systemctl --user is-active` prints `active`.
fn wait_running(ctx: &Context, account: &str, wait: Duration) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        let running = match ctx.manager {
            Manager::Launchd => query(ctx, &["print", &target(ctx, account)])
                .is_some_and(|(code, out)| code == 0 && parse_launchctl_print(&out).pid.is_some()),
            Manager::Systemd => query(ctx, &["--user", "is-active", &unit_name(account)])
                .is_some_and(|(_, out)| out.trim() == "active"),
        };
        if running {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(200));
    }
}

/// Where a service that did not start explains why.
fn log_hint(ctx: &Context, config_path: &Path, account: &str) -> String {
    match ctx.manager {
        Manager::Launchd => fs::canonicalize(config_path)
            .ok()
            .and_then(|c| {
                c.parent()
                    .map(|d| d.join("logs").join(format!("{account}.err")))
            })
            .map_or_else(|| format!("{account}.err"), |p| p.display().to_string()),
        Manager::Systemd => format!("journalctl --user -u {}", unit_name(account)),
    }
}

fn action(ctx: &Context, account: &str, action: &str) -> Value {
    json!({
        "action": action,
        "manager": ctx.manager.name(),
        "account": account,
        "unit_path": ctx.unit_path(account),
    })
}

/// `service start`: enables and starts the installed service, then checks
/// for up to `wait` that it runs (exit 3 `service did not start; see LOG`).
pub fn start(
    ctx: &Context,
    service: &Service,
    config_path: &Path,
    account: &str,
    proc_root: &Path,
    wait: Duration,
) -> Result<Value> {
    let found = checked(ctx, service, config_path, account, proc_root)?;
    let target = target(ctx, account);
    let result = match ctx.manager {
        Manager::Launchd => {
            tool(ctx, &["enable", &target])?;
            match (found.loaded, found.running) {
                (Some(true), true) => "already_running",
                (Some(true), false) => {
                    tool(ctx, &["kickstart", &target])?;
                    "started"
                }
                _ => {
                    let plist = ctx.unit_path(account).display().to_string();
                    tool(ctx, &["bootstrap", &format!("gui/{}", ctx.uid), &plist])?;
                    "started"
                }
            }
        }
        Manager::Systemd => {
            tool(ctx, &["--user", "enable", "--now", &unit_name(account)])?;
            if found.running {
                "already_running"
            } else {
                "started"
            }
        }
    };
    if result == "started" && !wait_running(ctx, account, wait) {
        return Err(err(
            3,
            format!(
                "service did not start; see {}",
                log_hint(ctx, config_path, account)
            ),
        ));
    }
    Ok(action(ctx, account, result))
}

/// `service stop`: stops the service and keeps it stopped across logins and
/// reboots; the unit file stays. `stopped` when launchd had the job loaded
/// or systemd had it running, else `already_stopped`.
pub fn stop(
    ctx: &Context,
    service: &Service,
    config_path: &Path,
    account: &str,
    proc_root: &Path,
) -> Result<Value> {
    let found = checked(ctx, service, config_path, account, proc_root)?;
    let result = match ctx.manager {
        Manager::Launchd => {
            let target = target(ctx, account);
            system_service::bootout(ctx, &target)?;
            tool(ctx, &["disable", &target])?;
            found.loaded == Some(true)
        }
        Manager::Systemd => {
            tool(ctx, &["--user", "disable", "--now", &unit_name(account)])?;
            found.running
        }
    };
    Ok(action(
        ctx,
        account,
        if result { "stopped" } else { "already_stopped" },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_proc(root: &Path, pid: u32, start: u64, args: &[&str]) {
        let dir = root.join(pid.to_string());
        fs::create_dir_all(&dir).unwrap();
        let fields: Vec<String> = (3..=21).map(|n| n.to_string()).collect();
        fs::write(
            dir.join("stat"),
            format!(
                "{pid} (mail triage) S {} {start} 0 0\n",
                fields[1..].join(" ")
            ),
        )
        .unwrap();
        let mut cmdline = args.join("\0").into_bytes();
        cmdline.push(0);
        fs::write(dir.join("cmdline"), cmdline).unwrap();
    }

    #[test]
    fn the_command_line_is_read_between_equal_start_times() {
        let dir = tempfile::tempdir().unwrap();
        write_proc(
            dir.path(),
            77,
            5000,
            &["/bin/mt", "watch", "--config", "/c d.json"],
        );
        assert_eq!(
            process_arguments(dir.path(), 77).unwrap(),
            ["/bin/mt", "watch", "--config", "/c d.json"]
        );
        // The PID is reused while the command line is read.
        let mut reads = 0;
        let changed = process_arguments_with(dir.path(), 77, |path| {
            reads += 1;
            let mut data = fs::read(path).ok()?;
            if reads == 3 {
                data = String::from_utf8(data)
                    .ok()?
                    .replace("5000", "5001")
                    .into_bytes();
            }
            Some(data)
        });
        assert_eq!(changed, None);
        assert_eq!(process_arguments(dir.path(), 78), None);
    }

    #[test]
    fn unit_and_plist_arguments_invert_their_writers() {
        let unit = system_service::Unit {
            account: "work".into(),
            exe: PathBuf::from("/opt/mail triage/bin/mailtriage"),
            config: PathBuf::from("/Users/a & b/50%$x/\"q\"\\mailtriage.json"),
            interval_seconds: 90,
            limit: 100,
            log_dir: PathBuf::from("/logs"),
            path_env: None,
        };
        let expected = unit.arguments();
        assert_eq!(
            unit_arguments(&system_service::systemd_unit(&unit)).unwrap(),
            expected
        );
        assert_eq!(
            plist_arguments(&system_service::plist(&unit)).unwrap(),
            expected
        );
        let args = watch_args(&expected).unwrap();
        assert_eq!(args.config, unit.config);
        assert_eq!(args.interval_seconds, Some(90));
        // What the writers never write fails closed: `None`, so `inspect`
        // reports the file's config as unknown rather than a wrong one.
        assert_eq!(unit_arguments("ExecStart=/a%b watch\n"), None);
        assert_eq!(unit_arguments("ExecStart=/a/'b' watch\n"), None);
        for value in ["/a&bogus;b", "/a<b", "/a&amp"] {
            let text =
                format!("<key>ProgramArguments</key><array><string>{value}</string></array>");
            assert_eq!(plist_arguments(&text), None, "{value}");
        }
    }

    #[test]
    fn exec_start_needs_the_install_shape() {
        let good = "{ path=/x ; argv[]=/x watch --config \"/a b.json\" --account w --interval-seconds 60 --limit 100 --json ; ignore_errors=no }";
        assert_eq!(
            watch_args(&exec_start_arguments(good).unwrap())
                .unwrap()
                .config,
            PathBuf::from("/a b.json")
        );
        // Printed without quotes, the path splits: no match rather than a wrong one.
        let split = "{ path=/x ; argv[]=/x watch --config /a b.json --account w --interval-seconds 60 --limit 100 --json ; ignore_errors=no }";
        assert_eq!(exec_start_arguments(split), None);
        // systemd resolves `%%` when it loads a unit but keeps `$$` until it
        // runs the command, so the loaded argv still holds `$$`.
        for (argv, config) in [
            ("\"/a\\$\\$b.json\"", "/a$b.json"),
            ("/50%%$$x.json", "/50%%$x.json"),
        ] {
            let value = format!("{{ path=/x ; argv[]=/x watch --config {argv} --account w --interval-seconds 60 --limit 100 --json ; ignore_errors=no }}");
            assert_eq!(
                watch_args(&exec_start_arguments(&value).unwrap())
                    .unwrap()
                    .config,
                PathBuf::from(config),
                "{argv}"
            );
        }
    }

    #[test]
    fn enablement_of_each_state() {
        let out = "\tdisabled services = {\n\t\t\"a\" => disabled\n\t\t\"b\" => enabled\n\t\t\"c\" => true\n\t\t\"d\" => false\n\t}\n";
        assert_eq!(
            launchd_enablement(Some(out), "a"),
            (Some(false), "disabled".into())
        );
        assert_eq!(
            launchd_enablement(Some(out), "b"),
            (Some(true), "enabled".into())
        );
        assert_eq!(
            launchd_enablement(Some(out), "c"),
            (Some(false), "disabled".into())
        );
        assert_eq!(
            launchd_enablement(Some(out), "d"),
            (Some(true), "enabled".into())
        );
        assert_eq!(
            launchd_enablement(Some(out), "e"),
            (Some(true), "enabled".into())
        );
        assert_eq!(launchd_enablement(None, "a"), (None, "unknown".into()));
        for (text, enabled) in [
            ("enabled\n", Some(true)),
            ("enabled-runtime\n", Some(true)),
            ("disabled\n", Some(false)),
            ("masked\n", Some(false)),
            ("masked-runtime\n", Some(false)),
            ("static\n", None),
            ("linked\n", None),
        ] {
            assert_eq!(
                systemd_enablement(Some(text)),
                (enabled, text.trim().to_owned()),
                "{text}"
            );
        }
        assert_eq!(systemd_enablement(Some("")), (None, "unknown".into()));
        assert_eq!(systemd_enablement(None), (None, "unknown".into()));
    }
}
