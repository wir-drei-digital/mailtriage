//! `mailtriage self install --dir DIR`: installs this binary (and a tray)
//! into DIR with the updater's install transaction, reports PATH, and offers
//! setup and the tray's login item. Every program it runs is `DIR/mailtriage`
//! or `DIR/mailtriage-tray`, never a bare name from `PATH`.
use super::protected::{self, Unsafe, Why};
use crate::{
    config,
    process::is_executable,
    prompt::Prompter,
    service::{err, err_kind, ErrorKind},
    setup::shell_line,
    update::{
        cache::Cache,
        github,
        install::{self, DoesNotRun, Hooks, Placement},
        platform::{self, FileIdentity},
        version, CLI, TRAY,
    },
};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    ffi::{OsStr, OsString},
    fs,
    io::{self, IsTerminal, Read},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Debug builds only: treat stdin as a terminal, so tests can answer the
/// prompts through a pipe. Release builds never read it.
pub const TEST_TERMINAL: &str = "MAILTRIAGE_TEST_TERMINAL";
/// Printed when the tray does not run on Linux.
pub const TRAY_PACKAGES: &str = "the tray needs GTK 3 and the Ayatana AppIndicator library (Debian/Ubuntu: sudo apt install libgtk-3-0 libayatana-appindicator3-1 libxdo3)";

/// `self install`'s flags.
#[derive(Debug, Clone, Default)]
pub struct Args {
    pub dir: PathBuf,
    pub tray_file: Option<PathBuf>,
    pub no_setup: bool,
    pub yes: bool,
}

/// Whether stdin is a terminal (or, in debug builds, `TEST_TERMINAL` is 1).
pub fn terminal() -> bool {
    #[cfg(debug_assertions)]
    if std::env::var_os(TEST_TERMINAL).is_some_and(|v| v == "1") {
        return true;
    }
    io::stdin().is_terminal()
}

/// Runs `self install`. Errors carry exit codes: 2 for a refused
/// downgrade, 3 for an unsafe directory (reason `unsafe_permissions`) or a
/// failed CLI transaction, 5 when the installation lock is held. A failed
/// tray transaction or a failed setup leaves the binaries installed: the
/// result then carries `exit_code` 3, which the CLI strips.
pub fn run(args: &Args, hooks: &dyn Hooks, p: &mut Prompter) -> Result<Value> {
    let cwd = std::env::current_dir().map_err(|e| err(2, format!("--dir: {e}")))?;
    let dir = install_dir(&cwd.join(&args.dir))?;
    let lock = install::lock(&dir, install::update_lock_wait())
        .map_err(|e| err(3, format!("{e:#}")))?
        .ok_or_else(|| err(5, "another update is running"))?;
    // Stale files of earlier attempts, as the updater's step 4 removes them.
    install::remove_leftovers(&dir);
    let running = version::running();
    let cli = dir.join(CLI.name);
    let before = regular_file(&cli)?;
    let mut installed = None;
    if before.is_some() {
        if let Ok(found) = platform::probe(&cli, CLI) {
            if version::is_newer(&found, &running) {
                return Err(err(
                    2,
                    format!(
                        "{} is {found}, newer than {running}; to go back, follow the guide's rollback steps",
                        cli.display()
                    ),
                ));
            }
            installed = Some(found);
        }
    }
    let cache = Cache::for_user();
    let own = own_binary()?;
    let placement = Placement {
        component: CLI,
        path: &cli,
        before,
        version: &running,
        cache: cache.as_ref(),
        hooks,
        keep_previous: keeps_previous(&cli, installed.as_ref(), &running),
    };
    let placed = install::place(&placement, &lock, &own).map_err(|e| err(3, format!("{e:#}")))?;
    drop(own);
    for warning in &placed.warnings {
        p.say(&format!("mailtriage: {warning}"));
    }
    p.say(&format!(
        "Installed mailtriage {running} at {}.",
        cli.display()
    ));
    let mut failed = false;
    let tray = args.tray_file.as_deref().map(|file| {
        let result = install_tray(&dir, file, &running, cache.as_ref(), hooks, &lock, p);
        failed |= result["action"] == "failed";
        result
    });
    drop(lock);
    let tray_installed = tray.as_ref().is_some_and(|t| t["action"] == "installed");
    // PATH.
    let report = path_report(
        &dir,
        std::env::var_os("PATH").as_deref(),
        std::env::var_os("SHELL").as_deref(),
        cfg!(target_os = "macos"),
    );
    for line in &report.lines {
        p.say(line);
    }
    // Setup, then the login item.
    let asking = !args.no_setup && !args.yes && terminal();
    let setup_line = shell_line(&[cli.as_os_str(), OsStr::new("setup")]);
    let (setup, config) = if asking && p.confirm_or("Run mailtriage setup now?", true) {
        match run_setup(&cli) {
            Ok(Some(config)) => ("ran", Some(config)),
            Ok(None) => {
                p.say("Setup aborted; nothing was changed.");
                ("skipped", None)
            }
            Err(message) => {
                p.say(&format!("mailtriage: setup failed: {message}"));
                failed = true;
                ("failed", None)
            }
        }
    } else {
        p.say(&format!("Next: run {setup_line}"));
        ("skipped", None)
    };
    let tray_exe = dir.join(TRAY.name);
    let autostart = match (&config, tray_installed) {
        (Some(config), true) => {
            if p.confirm_or("Start the tray at login?", true) {
                match enable_login_item(&tray_exe, config, &cli) {
                    Ok(()) => "enabled",
                    Err(message) => {
                        p.say(&format!("mailtriage: start at login failed: {message}"));
                        "failed"
                    }
                }
            } else {
                "skipped"
            }
        }
        (None, true) if setup != "failed" => {
            let config = default_config().unwrap_or_else(|| PathBuf::from("CONFIG"));
            p.say(&format!(
                "To start the tray at login after setup: {}",
                login_item_line(&tray_exe, &config, &cli)
            ));
            "skipped"
        }
        _ => "skipped",
    };
    let mut out = json!({"schema_version": 1, "self_install": {
        "dir": dir,
        "cli": {"action": "installed", "version": running.to_string(), "path": cli},
        "tray": tray,
        "on_path": report.on_path,
        "shadowed_by": report.shadowed_by,
        "setup": setup,
        "autostart": autostart,
    }});
    if failed {
        out["exit_code"] = json!(3);
    }
    Ok(out)
}

/// DIR, absolute: created when missing (mode 0755 whatever the umask),
/// canonicalized, owned by this user, and passing the protected path rule.
fn install_dir(dir: &Path) -> Result<PathBuf> {
    let refuse = |found: &Unsafe| {
        err_kind(
            3,
            ErrorKind::UnsafePermissions,
            format!(
                "unsafe_permissions: {found}; {}, or pass another --dir",
                found.fix()
            ),
        )
    };
    let dir = protected::prepare(dir).map_err(|e| match protected::unsafe_dir(&e) {
        Some(found) => refuse(found),
        None => err(3, format!("--dir: {e:#}")),
    })?;
    let owner = fs::metadata(&dir)
        .map_err(|e| err(3, format!("cannot read {}: {e}", dir.display())))?
        .uid();
    if owner != crate::system_service::current_uid() {
        return Err(refuse(&Unsafe {
            dir,
            why: Why::NotYours(owner),
        }));
    }
    Ok(dir)
}

/// Whether `place` keeps `<path>.previous` as it is: the installed file
/// already prints this version (a reinstall) and `.previous` exists. A
/// backup would replace the release before it with a copy of this one.
fn keeps_previous(
    path: &Path,
    installed: Option<&semver::Version>,
    running: &semver::Version,
) -> bool {
    installed == Some(running) && fs::symlink_metadata(install::previous_path(path)).is_ok()
}

/// The identity of the regular file at `path`; `None` when there is none.
/// Anything else there (a link, a directory) is refused: exit 3.
fn regular_file(path: &Path) -> Result<Option<FileIdentity>> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() => Ok(Some(FileIdentity::of(&meta))),
        Ok(_) => Err(err(
            3,
            format!(
                "{} is not a regular file; move it away first",
                path.display()
            ),
        )),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(err(3, format!("cannot read {}: {e}", path.display()))),
    }
}

/// The bytes of the binary this process runs (at most 200 MB): on Linux
/// `/proc/self/exe`, which is the running image even after a replacement.
fn own_binary() -> Result<Vec<u8>> {
    let path = if cfg!(target_os = "linux") {
        PathBuf::from("/proc/self/exe")
    } else {
        std::env::current_exe().map_err(|e| err(3, format!("cannot find this program: {e}")))?
    };
    read_capped(&path).map_err(|e| err(3, format!("cannot read {}: {e}", path.display())))
}

fn read_capped(path: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(github::MAX_ASSET_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > github::MAX_ASSET_BYTES {
        return Err(io::Error::other("it is larger than 200 MB"));
    }
    Ok(bytes)
}

/// The tray's transaction, after the CLI's commit and under the same lock:
/// `{"action": "installed"|"skipped"|"failed", "error"}`. A tray that does
/// not run here is skipped and any existing one kept.
fn install_tray(
    dir: &Path,
    file: &Path,
    running: &semver::Version,
    cache: Option<&Cache>,
    hooks: &dyn Hooks,
    lock: &install::InstallLock,
    p: &mut Prompter,
) -> Value {
    let path = dir.join(TRAY.name);
    let result = (|| -> Result<()> {
        let before = regular_file(&path)?;
        let meta = fs::symlink_metadata(file)
            .map_err(|e| err(3, format!("--tray-file: {}: {e}", file.display())))?;
        if !meta.is_file() {
            return Err(err(
                3,
                format!("--tray-file: {} is not a regular file", file.display()),
            ));
        }
        let bytes = read_capped(file)
            .map_err(|e| err(3, format!("--tray-file: {}: {e}", file.display())))?;
        let installed = before
            .is_some()
            .then(|| platform::probe(&path, TRAY).ok())
            .flatten();
        let placement = Placement {
            component: TRAY,
            path: &path,
            before,
            version: running,
            cache,
            hooks,
            keep_previous: keeps_previous(&path, installed.as_ref(), running),
        };
        let placed = install::place(&placement, lock, &bytes)?;
        for warning in placed.warnings {
            p.say(&format!("mailtriage-tray: {warning}"));
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            p.say(&format!("Installed mailtriage-tray at {}.", path.display()));
            json!({"action": "installed", "error": null})
        }
        Err(e) => match e.downcast_ref::<DoesNotRun>() {
            Some(cause) => {
                let message = TRAY.does_not_run(&cause.0);
                p.say(&format!("Skipped the tray: {message}"));
                if cfg!(target_os = "linux") {
                    p.say(TRAY_PACKAGES);
                }
                json!({"action": "skipped", "error": message})
            }
            None => {
                let message = format!("{e:#}");
                p.say(&format!("mailtriage-tray: {message}"));
                json!({"action": "failed", "error": message})
            }
        },
    }
}

/// What `self install` says about PATH.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathReport {
    /// DIR is a directory of PATH.
    pub on_path: bool,
    /// Another `mailtriage` that PATH finds before DIR's.
    pub shadowed_by: Option<PathBuf>,
    /// The lines to print.
    pub lines: Vec<String>,
}

/// The PATH report for DIR (canonical) with `PATH`, the user's `SHELL`, and
/// whether this is macOS (bash reads `~/.bash_profile` there).
pub fn path_report(
    dir: &Path,
    path: Option<&OsStr>,
    shell: Option<&OsStr>,
    macos: bool,
) -> PathReport {
    let dirs: Vec<PathBuf> = path
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();
    let is_dir = |d: &PathBuf| fs::canonicalize(d).is_ok_and(|d| d == dir);
    let on_path = dirs.iter().any(is_dir);
    let own = dir.join(CLI.name);
    let shadowed_by = dirs
        .iter()
        .take_while(|d| !is_dir(d))
        .filter(|d| !d.as_os_str().is_empty())
        .map(|d| d.join(CLI.name))
        .find(|candidate| {
            is_executable(candidate) && fs::canonicalize(candidate).ok().as_deref() != Some(&*own)
        });
    let mut lines = Vec::new();
    if !on_path {
        lines.push(format!(
            "{} is not on your PATH. For new terminals, run: {}",
            dir.display(),
            path_line(dir, shell, macos)
        ));
    }
    if let Some(other) = &shadowed_by {
        lines.push(format!(
            "Your PATH finds {} first: it stays in use and is not updated by this install.",
            other.display()
        ));
    }
    PathReport {
        on_path,
        shadowed_by,
        lines,
    }
}

/// The line that puts DIR on PATH for the user's shell: zsh appends to
/// `~/.zshrc`, bash to `~/.bashrc` (`~/.bash_profile` on macOS), fish runs
/// `fish_add_path`, any other shell gets a POSIX `export`.
pub fn path_line(dir: &Path, shell: Option<&OsStr>, macos: bool) -> String {
    let shell = shell
        .and_then(|s| Path::new(s).file_name())
        .and_then(OsStr::to_str)
        .unwrap_or("");
    let export = format!("export PATH=\"{}:$PATH\"", double_quoted(dir));
    let append = |file: &str| format!("echo {} >> {file}", single_quoted(&export));
    match shell {
        "zsh" => append("~/.zshrc"),
        "bash" if macos => append("~/.bash_profile"),
        "bash" => append("~/.bashrc"),
        "fish" => shell_line(&[OsStr::new("fish_add_path"), dir.as_os_str()]),
        _ => export,
    }
}

/// `dir` for the inside of double quotes: `\`, `"`, `$` and `` ` `` escaped.
fn double_quoted(dir: &Path) -> String {
    let mut out = String::new();
    for c in dir.to_string_lossy().chars() {
        if matches!(c, '\\' | '"' | '$' | '`') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `text` as one single-quoted shell word.
fn single_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Runs the installed `DIR/mailtriage setup --interactive --json` with this
/// terminal as stdin and stderr; returns the config it wrote, `None` when
/// the user chose Abort (reason `setup_aborted`: nothing was changed), or
/// its error message.
fn run_setup(cli: &Path) -> Result<Option<PathBuf>, String> {
    let out = Command::new(cli)
        .args(["setup", "--interactive", "--json"])
        .stdin(Stdio::inherit())
        .stderr(Stdio::inherit())
        .stdout(Stdio::piped())
        .output()
        .map_err(|e| format!("cannot start {}: {e}", cli.display()))?;
    let value: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    if out.status.success() {
        return value["setup"]["config"]
            .as_str()
            .map(|config| Some(PathBuf::from(config)))
            .ok_or_else(|| "setup printed no config".to_owned());
    }
    let aborted = ErrorKind::SetupAborted.reason();
    if out.status.code() == Some(2) && value["error"]["reason"].as_str() == aborted {
        return Ok(None);
    }
    Err(value["error"]["message"].as_str().map_or_else(
        || format!("setup exited with {}", out.status),
        str::to_owned,
    ))
}

/// The login item command for the tray at `tray`.
fn login_item_line(tray: &Path, config: &Path, cli: &Path) -> String {
    shell_line(&login_item_args(tray, config, cli))
}

fn login_item_args(tray: &Path, config: &Path, cli: &Path) -> Vec<OsString> {
    vec![
        tray.as_os_str().to_owned(),
        "autostart".into(),
        "enable".into(),
        "--config".into(),
        config.as_os_str().to_owned(),
        "--mailtriage".into(),
        cli.as_os_str().to_owned(),
    ]
}

/// Runs `DIR/mailtriage-tray autostart enable --config CONFIG --mailtriage
/// DIR/mailtriage`; its error message on failure.
fn enable_login_item(tray: &Path, config: &Path, cli: &Path) -> Result<(), String> {
    let args = login_item_args(tray, config, cli);
    let out = Command::new(&args[0])
        .args(&args[1..])
        .arg("--json")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot start {}: {e}", tray.display()))?;
    if out.status.success() {
        return Ok(());
    }
    let value: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    Err(value["error"]["message"]
        .as_str()
        .map_or_else(|| format!("it exited with {}", out.status), str::to_owned))
}

/// The config setup writes by default: `MAILTRIAGE_CONFIG`, else
/// `~/.config/mailtriage/mailtriage.json`.
fn default_config() -> Option<PathBuf> {
    config::setup_path(
        None,
        std::env::var_os("MAILTRIAGE_CONFIG").as_deref(),
        std::env::var_os("HOME").map(PathBuf::from).as_deref(),
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_line_fits_the_shell() {
        let dir = Path::new("/Users/a/.local/bin");
        let line = |shell: &str, macos| path_line(dir, Some(OsStr::new(shell)), macos);
        assert_eq!(
            line("/bin/zsh", true),
            r#"echo 'export PATH="/Users/a/.local/bin:$PATH"' >> ~/.zshrc"#
        );
        assert_eq!(
            line("/bin/bash", true),
            r#"echo 'export PATH="/Users/a/.local/bin:$PATH"' >> ~/.bash_profile"#
        );
        assert_eq!(
            line("/usr/bin/bash", false),
            r#"echo 'export PATH="/Users/a/.local/bin:$PATH"' >> ~/.bashrc"#
        );
        assert_eq!(
            line("/usr/local/bin/fish", false),
            "fish_add_path /Users/a/.local/bin"
        );
        assert_eq!(
            line("/bin/dash", false),
            r#"export PATH="/Users/a/.local/bin:$PATH""#
        );
        assert_eq!(
            path_line(dir, None, false),
            r#"export PATH="/Users/a/.local/bin:$PATH""#
        );
        // Characters the shell would interpret stay literal.
        let odd = Path::new("/home/it's \"$x\"/bin");
        assert_eq!(
            path_line(odd, Some(OsStr::new("zsh")), false),
            r#"echo 'export PATH="/home/it'\''s \"\$x\"/bin:$PATH"' >> ~/.zshrc"#
        );
    }
}
