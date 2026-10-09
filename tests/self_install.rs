#![cfg(unix)]
//! `mailtriage self install` through the binary: a copy of the binary under
//! test in a "download" directory installs itself into DIR. HOME, the cache
//! and the data directory are temporary; launchctl, systemctl, Himalaya and
//! the tray are fakes; stdin counts as a terminal only through the debug
//! builds' `MAILTRIAGE_TEST_TERMINAL`.
mod common;
mod install_support;
mod update_support;
use common::{write_tool, LAUNCHCTL, SYSTEMCTL};
use fs2::FileExt;
use install_support::{fake_himalaya, fake_tray, output, spawn, write_exe};
use mailtriage::{system_service::Manager, update::service_files};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};
use update_support::{cache_dir, Server, Watch};

const RUNNING: &str = env!("CARGO_PKG_VERSION");

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        for d in [
            "download",
            "home/.config/himalaya",
            "xdg",
            "data",
            "tools",
            "state",
        ] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        let download = root.join("download/mailtriage");
        fs::copy(env!("CARGO_BIN_EXE_mailtriage"), &download).unwrap();
        fs::set_permissions(&download, fs::Permissions::from_mode(0o755)).unwrap();
        write_tool(&root.join("tools"), "launchctl", LAUNCHCTL);
        write_tool(&root.join("tools"), "systemctl", SYSTEMCTL);
        write_exe(
            &root.join("tools/himalaya"),
            &fake_himalaya("himalaya v2.1.0 +imap"),
        );
        fs::write(
            root.join("home/.config/himalaya/config.toml"),
            "[accounts.work]\nemail = \"work@example.test\"\nimap.server = \"imaps://mail.example.test\"\n",
        )
        .unwrap();
        Self { _dir: dir, root }
    }

    fn dir(&self) -> PathBuf {
        self.root.join("bin")
    }

    fn installed(&self) -> PathBuf {
        self.dir().join("mailtriage")
    }

    /// `program ARGS` with this fixture's environment; `stdin` is piped in.
    fn run_program(
        &self,
        program: &Path,
        args: &[&str],
        stdin: &str,
        env: &[(&str, &str)],
    ) -> (Output, Value) {
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(&self.root)
            .env("HOME", self.root.join("home"))
            .env("XDG_CACHE_HOME", self.root.join("xdg"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("tools").display()),
            )
            .env("MT_FAKE_DIR", self.root.join("state"))
            .env("MT_FAKE_HOME", self.root.join("home"))
            .env("TZ", "Europe/Berlin")
            .env("SHELL", "/bin/zsh")
            .env_remove("MAILTRIAGE_CONFIG")
            .env_remove("HIMALAYA_CONFIG")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("MAILTRIAGE_TEST_TERMINAL")
            .env_remove("MAILTRIAGE_UPDATE_TEST_HOOK")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = spawn(&mut command);
        let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
        let out = child.wait_with_output().unwrap();
        let value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
        (out, value)
    }

    /// The downloaded copy's `self install --dir DIR --json ARGS`.
    fn install(&self, args: &[&str], stdin: &str, env: &[(&str, &str)]) -> (Output, Value) {
        let dir = self.dir();
        let all = [
            &["self", "install", "--json", "--dir", dir.to_str().unwrap()][..],
            args,
        ]
        .concat();
        self.run_program(&self.root.join("download/mailtriage"), &all, stdin, env)
    }

    fn cache(&self) -> Value {
        fs::read(cache_dir(&self.root.join("home"), &self.root.join("xdg")).join("update.json"))
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or(Value::Null)
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

#[test]
fn a_fresh_install_creates_the_directory_0755_whatever_the_umask() {
    let f = Fixture::new();
    let dir = f.root.join("new/bin");
    let out = output(
        Command::new("/bin/sh")
            .args(["-c", "umask 002; exec \"$@\"", "sh"])
            .arg(f.root.join("download/mailtriage"))
            .args(["self", "install", "--json", "--no-setup", "--dir"])
            .arg(&dir)
            .env("HOME", f.root.join("home"))
            .env("XDG_CACHE_HOME", f.root.join("xdg"))
            .env("PATH", "/usr/bin:/bin")
            .env_remove("MAILTRIAGE_TEST_TERMINAL")
            .stdin(Stdio::null()),
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    let s = &v["self_install"];
    assert_eq!(s["dir"], dir.to_str().unwrap());
    assert_eq!(
        s["cli"],
        json!({"action": "installed", "version": RUNNING, "path": dir.join("mailtriage")})
    );
    assert_eq!(s["tray"], Value::Null);
    assert_eq!(s["setup"], "skipped");
    assert_eq!(s["autostart"], "skipped");
    assert_eq!(mode(&f.root.join("new")), 0o755);
    assert_eq!(mode(&dir), 0o755);
    assert_eq!(mode(&dir.join("mailtriage")), 0o755);
    assert!(!dir.join("mailtriage.previous").exists());
    let version = Command::new(dir.join("mailtriage"))
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&version.stdout),
        format!("mailtriage {RUNNING}\n")
    );
    assert!(stderr(&out).contains(&format!("Next: run {}/mailtriage setup", dir.display())));
}

#[test]
fn an_unsafe_directory_is_refused_and_nothing_installed() {
    let f = Fixture::new();
    let shared = f.root.join("shared");
    fs::create_dir(&shared).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o775)).unwrap();
    symlink(&shared, f.root.join("link")).unwrap();
    for dir in [shared.clone(), f.root.join("link")] {
        let (out, v) = f.run_program(
            &f.root.join("download/mailtriage"),
            &[
                "self",
                "install",
                "--json",
                "--no-setup",
                "--dir",
                dir.to_str().unwrap(),
            ],
            "",
            &[],
        );
        assert_eq!(out.status.code(), Some(3), "{v}");
        assert_eq!(v["error"]["reason"], "unsafe_permissions");
        let message = v["error"]["message"].as_str().unwrap();
        assert!(
            message.contains(&format!(
                "run chmod go-w {}, or pass another --dir",
                shared.display()
            )),
            "{message}"
        );
        assert!(!shared.join("mailtriage").exists());
    }
}

#[test]
fn a_newer_installed_version_is_never_downgraded() {
    let f = Fixture::new();
    let newer = "#!/bin/sh\necho 'mailtriage 9.9.9'\n";
    write_exe(&f.installed(), newer);
    fs::set_permissions(f.dir(), fs::Permissions::from_mode(0o755)).unwrap();
    let (out, v) = f.install(&["--no-setup"], "", &[]);
    assert_eq!(out.status.code(), Some(2), "{v}");
    assert_eq!(
        v["error"]["message"],
        format!(
            "{} is 9.9.9, newer than {RUNNING}; to go back, follow the guide's rollback steps",
            f.installed().display()
        )
    );
    assert_eq!(fs::read_to_string(f.installed()).unwrap(), newer);
}

#[test]
fn a_held_installation_lock_exits_5() {
    let f = Fixture::new();
    fs::create_dir_all(f.dir()).unwrap();
    fs::set_permissions(f.dir(), fs::Permissions::from_mode(0o755)).unwrap();
    let lock = fs::File::create(f.dir().join(".mailtriage-update.lock")).unwrap();
    lock.lock_exclusive().unwrap();
    let (out, v) = f.install(
        &["--no-setup"],
        "",
        &[("MAILTRIAGE_UPDATE_TEST_LOCK_WAIT_MS", "300")],
    );
    assert_eq!(out.status.code(), Some(5), "{v}");
    assert!(!f.installed().exists());
}

#[test]
fn a_running_service_restarts_onto_the_installed_file() {
    let f = Fixture::new();
    fs::create_dir_all(f.dir()).unwrap();
    fs::set_permissions(f.dir(), fs::Permissions::from_mode(0o755)).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_mailtriage"), f.installed()).unwrap();
    let server = Server::start();
    let config = f.root.join("cfg/mailtriage.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let (out, _) = f.run_program(
        &f.installed(),
        &["init", "--json", "--config", config.to_str().unwrap()],
        "",
        &[],
    );
    assert!(out.status.success());
    update_support::set_updates(&config, "off");
    let mut watch = Watch::spawn(
        Command::new(f.installed())
            .args([
                "watch",
                "--account",
                "work",
                "--interval-seconds",
                "1",
                "--json",
            ])
            .current_dir(&f.root)
            .env("HOME", f.root.join("home"))
            .env("XDG_CACHE_HOME", f.root.join("xdg"))
            .env("MAILTRIAGE_UPDATE_URL", &server.base)
            .env("MAILTRIAGE_CONFIG", &config),
    );
    watch.wait_passes(1);
    let (out, v) = f.install(&["--no-setup"], "", &[]);
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    let restarting = watch.event("restarting");
    assert_eq!(restarting["update"]["pid"], watch.child.id());
    assert!(f.dir().join("mailtriage.previous").exists());
    watch.stop();
}

/// Re-running the installer at the same version replaces the files (so
/// services restart onto them) but keeps the release before it in
/// `.previous`, for rolling back by hand.
#[test]
fn a_same_version_reinstall_keeps_the_previous_release() {
    use std::os::unix::fs::MetadataExt;
    let f = Fixture::new();
    let old_cli = "#!/bin/sh\necho 'mailtriage 0.0.0'\n";
    let old_tray = fake_tray("0.0.0");
    write_exe(&f.installed(), old_cli);
    write_exe(&f.dir().join("mailtriage-tray"), &old_tray);
    fs::set_permissions(f.dir(), fs::Permissions::from_mode(0o755)).unwrap();
    let tray_file = f.root.join("download/mailtriage-tray");
    write_exe(&tray_file, &fake_tray(RUNNING));
    let args = ["--no-setup", "--tray-file", tray_file.to_str().unwrap()];
    let ino = |name: &str| fs::metadata(f.dir().join(name)).unwrap().ino();
    let (out, v) = f.install(&args, "", &[]);
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    assert_eq!(v["self_install"]["tray"]["action"], "installed", "{v}");
    assert_eq!(
        fs::read_to_string(f.dir().join("mailtriage.previous")).unwrap(),
        old_cli
    );
    assert_eq!(
        fs::read_to_string(f.dir().join("mailtriage-tray.previous")).unwrap(),
        old_tray
    );
    let (cli_before, tray_before) = (ino("mailtriage"), ino("mailtriage-tray"));
    // The same version again.
    let (out, v) = f.install(&args, "", &[]);
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    assert_eq!(v["self_install"]["cli"]["action"], "installed", "{v}");
    assert_eq!(v["self_install"]["tray"]["action"], "installed", "{v}");
    assert_ne!(ino("mailtriage"), cli_before, "the CLI is replaced");
    assert_ne!(ino("mailtriage-tray"), tray_before, "the tray is replaced");
    assert!(
        fs::read(f.installed()).unwrap() == fs::read(env!("CARGO_BIN_EXE_mailtriage")).unwrap()
    );
    assert!(
        fs::read(f.dir().join("mailtriage.previous")).unwrap() == old_cli.as_bytes(),
        "mailtriage.previous is no longer the release before"
    );
    assert!(
        fs::read(f.dir().join("mailtriage-tray.previous")).unwrap() == old_tray.as_bytes(),
        "mailtriage-tray.previous is no longer the release before"
    );
    // No backup was left behind.
    let mut names: Vec<String> = fs::read_dir(f.dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with(".mailtriage-update.lock"))
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "mailtriage",
            "mailtriage-tray",
            "mailtriage-tray.previous",
            "mailtriage.previous"
        ]
    );
}

#[test]
fn the_tray_is_installed_or_skipped_keeping_the_old_one() {
    let f = Fixture::new();
    let good = f.root.join("download/mailtriage-tray");
    write_exe(&good, &fake_tray(RUNNING));
    let (out, v) = f.install(
        &["--no-setup", "--tray-file", good.to_str().unwrap()],
        "",
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    assert_eq!(
        v["self_install"]["tray"],
        json!({"action": "installed", "error": null})
    );
    let installed = f.dir().join("mailtriage-tray");
    assert_eq!(fs::read_to_string(&installed).unwrap(), fake_tray(RUNNING));
    // A tray that does not run here (here: the wrong version) is skipped.
    let bad = f.root.join("download/other-tray");
    write_exe(&bad, &fake_tray("9.9.9"));
    let (out, v) = f.install(
        &["--no-setup", "--tray-file", bad.to_str().unwrap()],
        "",
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{v}");
    let tray = &v["self_install"]["tray"];
    assert_eq!(tray["action"], "skipped");
    assert_eq!(
        tray["error"],
        format!("mailtriage-tray does not run here: printed version 9.9.9, expected {RUNNING}")
    );
    assert_eq!(fs::read_to_string(&installed).unwrap(), fake_tray(RUNNING));
    assert_eq!(
        stderr(&out).contains("sudo apt install libgtk-3-0 libayatana-appindicator3-1 libxdo3"),
        cfg!(target_os = "linux")
    );
}

#[test]
fn the_cache_records_each_component_and_clears_a_previous_failure() {
    let f = Fixture::new();
    fs::create_dir_all(f.dir()).unwrap();
    fs::set_permissions(f.dir(), fs::Permissions::from_mode(0o755)).unwrap();
    let cache = cache_dir(&f.root.join("home"), &f.root.join("xdg"));
    fs::create_dir_all(&cache).unwrap();
    let key = f.installed().to_str().unwrap().to_owned();
    let failed = json!({"version": "0.0.1", "failures": 3,
        "last_error": {"at": "2026-10-01T00:00:00Z", "message": "checksum mismatch"},
        "next_attempt_at": "2099-01-01T00:00:00Z"});
    fs::write(
        cache.join("update.json"),
        json!({"schema_version": 1, "installs": {key.clone(): failed}}).to_string(),
    )
    .unwrap();
    let tray = f.root.join("download/mailtriage-tray");
    write_exe(&tray, &fake_tray(RUNNING));
    let (out, v) = f.install(
        &["--no-setup", "--tray-file", tray.to_str().unwrap()],
        "",
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{v}");
    let installs = &f.cache()["installs"];
    for path in [
        key,
        f.dir().join("mailtriage-tray").to_str().unwrap().to_owned(),
    ] {
        let entry = &installs[&path];
        assert_eq!(entry["version"], RUNNING, "{path}");
        assert_eq!(entry["failures"], 0, "{path}");
        assert_eq!(entry["last_error"], Value::Null, "{path}");
        assert_eq!(entry["next_attempt_at"], Value::Null, "{path}");
    }
}

#[test]
fn path_and_shadowing_are_reported() {
    let f = Fixture::new();
    write_exe(&f.root.join("other/mailtriage"), "#!/bin/sh\n");
    let path = format!(
        "{}:{}:/usr/bin:/bin",
        f.root.join("other").display(),
        f.dir().display()
    );
    let (out, v) = f.install(&["--no-setup"], "", &[("PATH", &path)]);
    assert_eq!(out.status.code(), Some(0), "{v}");
    assert_eq!(v["self_install"]["on_path"], true);
    assert_eq!(
        v["self_install"]["shadowed_by"],
        f.root.join("other/mailtriage").to_str().unwrap()
    );
    assert!(stderr(&out).contains("stays in use and is not updated by this install"));
    let (out, v) = f.install(&["--no-setup"], "", &[("PATH", "/usr/bin:/bin")]);
    assert_eq!(v["self_install"]["on_path"], false);
    assert_eq!(v["self_install"]["shadowed_by"], Value::Null);
    assert!(
        stderr(&out).contains(&format!(
            "echo 'export PATH=\"{}:$PATH\"' >> ~/.zshrc",
            f.dir().display()
        )),
        "{}",
        stderr(&out)
    );
}

#[test]
fn setup_is_offered_only_with_a_terminal_and_never_with_yes() {
    let f = Fixture::new();
    for (args, env) in [
        (vec![], vec![]),
        (vec!["--yes"], vec![("MAILTRIAGE_TEST_TERMINAL", "1")]),
        (vec!["--no-setup"], vec![("MAILTRIAGE_TEST_TERMINAL", "1")]),
    ] {
        let (out, v) = f.install(&args, "\n", &env);
        assert_eq!(out.status.code(), Some(0), "{v}");
        assert_eq!(v["self_install"]["setup"], "skipped");
        assert!(
            !stderr(&out).contains("Run mailtriage setup now?"),
            "{args:?}"
        );
        assert!(stderr(&out).contains("Next: run "), "{args:?}");
    }
}

#[test]
fn setup_runs_from_the_installed_binary_then_the_login_item_uses_its_config() {
    let f = Fixture::new();
    // An existing config, so setup's interactive run updates it.
    let (out, v) = f.run_program(
        &f.root.join("download/mailtriage"),
        &[
            "setup",
            "--yes",
            "--json",
            "--himalaya-account",
            "work",
            "--provider",
            "fake",
        ],
        "",
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    let config = v["setup"]["config"].as_str().unwrap().to_owned();
    let tray = f.root.join("download/mailtriage-tray");
    write_exe(&tray, &fake_tray(RUNNING));
    // Yes to setup; Enter for every setup question, the service (yes)
    // included; yes to the login item.
    let (out, v) = f.install(
        &["--tray-file", tray.to_str().unwrap()],
        &"\n".repeat(20),
        &[("MAILTRIAGE_TEST_TERMINAL", "1")],
    );
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    let s = &v["self_install"];
    assert_eq!(
        (s["setup"].clone(), s["autostart"].clone()),
        (json!("ran"), json!("enabled"))
    );
    assert!(stderr(&out).contains("Run mailtriage setup now? [Y/n]"));
    assert!(stderr(&out).contains("Start the tray at login? [Y/n]"));
    let login = fs::read_to_string(f.dir().join("tray.log")).unwrap();
    assert_eq!(
        login.trim_end(),
        format!(
            "autostart enable --config {config} --mailtriage {} --json",
            f.installed().display()
        )
    );
    // The download directory is gone; the service runs the installed file.
    fs::remove_dir_all(f.root.join("download")).unwrap();
    let manager = service_files::platform_manager().unwrap_or(Manager::Systemd);
    let services = service_files::list(manager, &f.root.join("home"));
    assert_eq!(services.len(), 1, "{}", stderr(&out));
    assert_eq!(
        services[0].executable.as_deref(),
        Some(f.installed().as_path())
    );
    let (out, v) = f.run_program(
        &f.installed(),
        &[
            "service",
            "status",
            "--account",
            "work",
            "--json",
            "--config",
            &config,
        ],
        "",
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    assert_eq!(v["service"]["installed"], true);
}

/// A re-run whose setup the user aborts (nothing changed) is not a failed
/// install: `setup` is `skipped` and the exit code 0.
#[test]
fn choosing_abort_in_setup_counts_as_skipped() {
    let f = Fixture::new();
    // An existing config, so setup offers Update, Add or Abort.
    let (out, v) = f.run_program(
        &f.root.join("download/mailtriage"),
        &[
            "setup",
            "--yes",
            "--json",
            "--himalaya-account",
            "work",
            "--provider",
            "fake",
        ],
        "",
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    let config = PathBuf::from(v["setup"]["config"].as_str().unwrap());
    let before = fs::read(&config).unwrap();
    // Yes to setup (Enter), then Abort in setup's menu.
    let (out, v) = f.install(&[], "\n3\n", &[("MAILTRIAGE_TEST_TERMINAL", "1")]);
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    assert!(v.get("exit_code").is_none(), "{v}");
    assert_eq!(v["self_install"]["setup"], "skipped", "{v}");
    assert_eq!(v["self_install"]["cli"]["action"], "installed");
    let err = stderr(&out);
    assert!(err.contains("Run mailtriage setup now? [Y/n]"), "{err}");
    assert!(err.contains("Setup aborted; nothing was changed."), "{err}");
    assert!(!err.contains("setup failed"), "{err}");
    assert_eq!(fs::read(&config).unwrap(), before);
}

/// The old habit of `sudo install … /usr/local/bin`: a root-owned directory
/// passes the protected path rule but is not the user's, so nothing is
/// written there and the message says what to do.
#[test]
fn a_root_owned_directory_is_refused_before_anything_is_written() {
    if mailtriage_is_root() {
        return;
    }
    let f = Fixture::new();
    let (out, v) = f.run_program(
        &f.root.join("download/mailtriage"),
        &[
            "self",
            "install",
            "--json",
            "--no-setup",
            "--dir",
            "/usr/bin",
        ],
        "",
        &[],
    );
    assert_eq!(out.status.code(), Some(3), "{v}");
    assert_eq!(v["error"]["reason"], "unsafe_permissions");
    assert_eq!(
        v["error"]["message"],
        "unsafe_permissions: /usr/bin belongs to uid 0, not to you; use a directory you own, or pass another --dir"
    );
}

fn mailtriage_is_root() -> bool {
    let out = Command::new("id").arg("-u").output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim() == "0"
}
