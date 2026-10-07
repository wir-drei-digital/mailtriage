//! `mailtriage self uninstall [--dir DIR]`: removes only the installation
//! in DIR (default: the running executable's directory): its services, the
//! tray's login item and running tray, then its programs. The programs are
//! removed only when every service and tray step succeeded.
use super::{himalaya, login_item, self_install};
use crate::{
    process::Ending,
    prompt::Prompter,
    service::err,
    service_control,
    setup::shell_line,
    system_service::{self, Context, Manager},
    update::{
        cache::{install_key, Cache},
        install::{self, Hooks},
        platform, service_files, CLI, TRAY,
    },
};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

/// How long `mailtriage-tray quit` may take: its answer and the wait for
/// the tray's lock are 5 s each.
const QUIT_TIMEOUT: Duration = Duration::from_secs(30);

/// `self uninstall`'s flags.
#[derive(Debug, Clone, Default)]
pub struct Args {
    pub dir: Option<PathBuf>,
    pub yes: bool,
}

/// Runs `self uninstall`. Errors carry exit codes: 2 for a Homebrew
/// installation, no `HOME`, a refused confirmation, or no terminal without
/// `--yes`; 5 when the installation lock is held for 60 s. A failed service
/// or tray step removes no program file: the result then carries
/// `exit_code` 3, which the CLI strips.
pub fn run(args: &Args, hooks: &dyn Hooks, p: &mut Prompter) -> Result<Value> {
    let dir = match &args.dir {
        Some(dir) => std::env::current_dir()
            .map_err(|e| err(2, format!("--dir: {e}")))?
            .join(dir),
        None => platform::installation_path()
            .map_err(|e| err(2, e))?
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| {
                err(
                    2,
                    "cannot tell which directory this mailtriage is in; pass --dir",
                )
            })?,
    };
    let dir =
        fs::canonicalize(&dir).map_err(|e| err(2, format!("--dir: {}: {e}", dir.display())))?;
    let cli = dir.join(CLI.name);
    let tray = dir.join(TRAY.name);
    let keg = |path: &Path| {
        fs::canonicalize(path)
            .unwrap_or_else(|_| path.to_path_buf())
            .to_string_lossy()
            .contains("/Cellar/mailtriage/")
    };
    if keg(&dir) || keg(&cli) {
        return Err(err(
            2,
            "installed by Homebrew; run brew uninstall mailtriage",
        ));
    }
    // The services and the login item are found under HOME; without it
    // they cannot be, so nothing is touched.
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| err(2, "HOME is not set"))?;
    if !args.yes {
        if !self_install::terminal() {
            return Err(err(
                2,
                "self uninstall asks before it removes anything; run it in a terminal or pass --yes",
            ));
        }
        let question = format!(
            "Uninstall mailtriage from {}? Its services stop and its programs are removed.",
            dir.display()
        );
        if !p.confirm_or(&question, false) {
            return Err(err(2, "uninstall cancelled; nothing was changed"));
        }
    }
    // Held through the file removal, so no update or install recreates the
    // binaries meanwhile. The lock file is never deleted.
    let lock = install::lock(&dir, install::update_lock_wait())
        .map_err(|e| err(3, format!("{e:#}")))?
        .ok_or_else(|| {
            err(
                5,
                "another update or install is running in this directory; try again",
            )
        })?;
    let mut failures = Vec::new();
    let mut removed = Vec::new();
    // HOME is set, so a failure here means no service manager (no
    // systemctl on PATH, or another platform): no services.
    let ctx = Context::detect().ok();
    let services = match &ctx {
        Some(ctx) => uninstall_services(ctx, &cli, hooks, &mut failures),
        None => vec![],
    };
    // The login item, without the tray binary.
    let macos = cfg!(target_os = "macos");
    let item = login_item::path(macos, &home, std::env::var_os("XDG_CONFIG_HOME").as_deref());
    let ours = fs::read_to_string(&item)
        .ok()
        .and_then(|text| login_item::tray_of(macos, &text))
        .is_some_and(|program| names(&program, &tray));
    let mut login = false;
    if ours {
        match remove_login_item(&item, ctx.as_ref()) {
            Ok(()) => {
                removed.push(item);
                login = true;
            }
            Err(e) => failures.push(format!("login item {}: {e}", item.display())),
        }
    }
    // The running tray, through its own `quit`.
    let tray_result = if fs::symlink_metadata(&tray).is_ok() {
        p.say("Close any open categories window; it keeps running until you do.");
        match quit_tray(&tray) {
            Ok(result) => result,
            Err(message) => {
                failures.push(format!("tray: {message}"));
                "failed".to_owned()
            }
        }
    } else if login {
        "not_running".to_owned()
    } else {
        "not_installed".to_owned()
    };
    // The files, only when every step above succeeded.
    if failures.is_empty() {
        for path in [
            cli.clone(),
            install::previous_path(&cli),
            tray.clone(),
            install::previous_path(&tray),
        ] {
            if fs::symlink_metadata(&path).is_err() {
                continue;
            }
            match fs::remove_file(&path) {
                Ok(()) => removed.push(path),
                Err(e) => failures.push(format!("cannot remove {}: {e}", path.display())),
            }
        }
        if let Some(cache) = Cache::for_user() {
            let keys = [install_key(&cli), install_key(&tray)];
            if let Err(e) = cache.update(|c| {
                for key in &keys {
                    c.installs.remove(key);
                }
            }) {
                p.say(&format!(
                    "mailtriage: the update cache keeps entries for {}: {e:#}",
                    dir.display()
                ));
            }
        }
    } else {
        for failure in &failures {
            p.say(&format!("mailtriage: {failure}"));
        }
        p.say(
            "No program files were removed; fix the failures above and run self uninstall again.",
        );
    }
    drop(lock);
    let kept = kept(&dir, &home, p);
    let mut out = json!({"schema_version": 1, "self_uninstall": {
        "dir": dir,
        "services": services,
        "tray": tray_result,
        "removed": removed,
        "kept": kept,
        "failures": failures,
    }});
    if !failures.is_empty() {
        out["exit_code"] = json!(3);
    }
    Ok(out)
}

/// Whether `program` is `target`: equal once canonicalized (a file that is
/// gone compares as written).
fn names(program: &Path, target: &Path) -> bool {
    fs::canonicalize(program).unwrap_or_else(|_| program.to_path_buf()) == target
}

/// Uninstalls every marked service file whose executable is `cli`, as
/// `service uninstall` does, under that account's service lock; a file
/// that names another executable once the lock is held is skipped. Each
/// gets `{account, unit_path, action, error}`.
fn uninstall_services(
    ctx: &Context,
    cli: &Path,
    hooks: &dyn Hooks,
    failures: &mut Vec<String>,
) -> Vec<Value> {
    let mut results = vec![];
    for file in service_files::list(ctx.manager, &ctx.home) {
        let ours = file
            .executable
            .as_deref()
            .is_some_and(|exe| names(exe, cli));
        if !ours {
            continue;
        }
        let result = (|| -> Result<&'static str> {
            let _lock =
                service_control::lock(ctx, &file.account, service_control::SERVICE_LOCK_WAIT)?;
            hooks.at("uninstall_service")?;
            let still_ours = fs::read_to_string(&file.unit_path)
                .ok()
                .filter(|text| system_service::is_marked(ctx.manager, text))
                .and_then(|text| service_files::executable(ctx.manager, &text))
                .is_some_and(|exe| names(&exe, cli));
            if !still_ours {
                return Ok("skipped");
            }
            system_service::uninstall(ctx, &file.account)?;
            Ok("uninstalled")
        })();
        let (action, error) = match result {
            Ok(action) => (action, None),
            Err(e) => {
                let message = error_text(&e);
                failures.push(format!("service {}: {message}", file.account));
                ("failed", Some(message))
            }
        };
        results.push(json!({
            "account": file.account,
            "unit_path": file.unit_path,
            "action": action,
            "error": error,
        }));
    }
    results
}

/// Removes the login item at `item`, the same way `autostart disable` does.
/// On macOS it first boots out the loaded login job, so a failed bootout
/// leaves the item in place for a rerun.
fn remove_login_item(item: &Path, ctx: Option<&Context>) -> Result<()> {
    if let Some(ctx) = ctx.filter(|c| c.manager == Manager::Launchd) {
        let target = format!("gui/{}/{}", ctx.uid, login_item::LABEL);
        system_service::bootout(ctx, &target)?;
    }
    fs::remove_file(item)?;
    Ok(())
}

/// `DIR/mailtriage-tray quit --json`: `quit`, `not_running` or
/// `other_installation`; otherwise why the tray did not stop.
fn quit_tray(tray: &Path) -> Result<String, String> {
    let out = crate::process::run_bounded(tray, &["quit", "--json"], QUIT_TIMEOUT, 64 * 1024)
        .map_err(|e| format!("cannot start {}: {e}", tray.display()))?;
    let exited_0 = matches!(out.ending, Ending::Exited(status) if status.success());
    let value: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    match (exited_0, value["quit"].as_str()) {
        (true, Some(result @ ("quit" | "not_running" | "other_installation"))) => {
            Ok(result.to_owned())
        }
        _ => Err(value["error"]["message"]
            .as_str()
            .unwrap_or("the tray does not respond; quit it from its menu")
            .to_owned()),
    }
}

/// What stays, said once: the lock file, the private Himalaya (other
/// configs may use it) and the config directory. No mail was touched.
fn kept(dir: &Path, home: &Path, p: &mut Prompter) -> Vec<PathBuf> {
    let mut kept = vec![dir.join(install::LOCK_FILE)];
    if let Some(private) = himalaya::default_data_dir()
        .map(|data| data.join("mailtriage/himalaya"))
        .filter(|path| path.exists())
    {
        p.say(&format!(
            "Kept the private Himalaya in {}: other configs may use it. Delete it with {} once none does.",
            private.display(),
            shell_line(&[Path::new("rm"), Path::new("-rf"), &private])
        ));
        kept.push(private);
    }
    let config = home.join(".config/mailtriage");
    p.say(&format!(
        "Your config, state and logs stay in {} (the default place); no mail was touched.",
        config.display()
    ));
    if config.exists() {
        kept.push(config);
    }
    kept
}

fn error_text(error: &anyhow::Error) -> String {
    error
        .downcast_ref::<crate::service::ServiceError>()
        .map_or_else(|| format!("{error:#}"), |e| e.message.clone())
}
