#![cfg(unix)]
//! `mailtriage self uninstall` through the binary, run from a copy in DIR:
//! services, the tray's login item and the running tray of this
//! installation go, then its files; everything else stays. HOME and the
//! cache are temporary; launchctl, systemctl and the tray are fakes, so no
//! real job is touched.
mod common;
mod install_support;
mod update_support;
use common::{write_tool, LAUNCHCTL, SYSTEMCTL};
use fs2::FileExt;
use install_support::{fake_tray, write_exe};
use mailtriage::{
    system_service::{self, Manager, Unit},
    update::service_files,
};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};
use update_support::cache_dir;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

fn manager() -> Manager {
    service_files::platform_manager().expect("macOS or Linux")
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        for d in ["bin", "home", "xdg", "tools", "other"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        fs::set_permissions(root.join("bin"), fs::Permissions::from_mode(0o755)).unwrap();
        let cli = root.join("bin/mailtriage");
        fs::copy(env!("CARGO_BIN_EXE_mailtriage"), &cli).unwrap();
        fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(cli.with_file_name("mailtriage.previous"), "old").unwrap();
        write_exe(&root.join("other/mailtriage"), "#!/bin/sh\n");
        write_tool(&root.join("tools"), "launchctl", LAUNCHCTL);
        write_tool(&root.join("tools"), "systemctl", SYSTEMCTL);
        Self { _dir: dir, root }
    }

    fn cli(&self) -> PathBuf {
        self.root.join("bin/mailtriage")
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    /// A marked service file for `account` that runs `exe`.
    fn service(&self, account: &str, exe: &Path) -> PathBuf {
        let dir = system_service::unit_dir(manager(), &self.home());
        fs::create_dir_all(&dir).unwrap();
        let unit = Unit {
            account: account.into(),
            exe: exe.to_path_buf(),
            config: self.root.join("mailtriage.json"),
            interval_seconds: 60,
            limit: 100,
            log_dir: self.root.join("logs"),
            path_env: None,
        };
        let (path, text) = match manager() {
            Manager::Launchd => (
                dir.join(format!("{}.plist", system_service::label(account))),
                system_service::plist(&unit),
            ),
            Manager::Systemd => (
                dir.join(system_service::unit_name(account)),
                system_service::systemd_unit(&unit),
            ),
        };
        fs::write(&path, text).unwrap();
        path
    }

    /// The tray's login item naming `tray`, as `autostart enable` writes it.
    fn login_item(&self, tray: &Path) -> PathBuf {
        let (path, text) = if cfg!(target_os = "macos") {
            (
                self.home().join("Library/LaunchAgents/digital.wirdrei.mailtriage-tray.plist"),
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\">\n<dict>\n  <key>XMailtriageManaged</key>\n  <true/>\n  <key>Label</key>\n  <string>digital.wirdrei.mailtriage-tray</string>\n  <key>ProgramArguments</key>\n  <array>\n    <string>{}</string>\n    <string>--config</string>\n    <string>/c/mailtriage.json</string>\n    <string>--mailtriage</string>\n    <string>{}</string>\n  </array>\n</dict>\n</plist>\n",
                    tray.display(),
                    self.cli().display()
                ),
            )
        } else {
            (
                self.home().join(".config/autostart/mailtriage-tray.desktop"),
                format!(
                    "# managed by mailtriage\n[Desktop Entry]\nType=Application\nName=mailtriage\nExec={} --config /c/mailtriage.json --mailtriage {}\nNoDisplay=true\n",
                    tray.display(),
                    self.cli().display()
                ),
            )
        };
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }

    fn uninstall(&self, args: &[&str], stdin: &str, env: &[(&str, &str)]) -> (Output, Value) {
        let mut command = Command::new(self.cli());
        command
            .args(["self", "uninstall", "--json"])
            .args(args)
            .current_dir(&self.root)
            .env("HOME", self.home())
            .env("XDG_CACHE_HOME", self.root.join("xdg"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("MT_FAKE_HOME", self.home())
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("tools").display()),
            )
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("MAILTRIAGE_TEST_TERMINAL")
            .env_remove("MAILTRIAGE_UPDATE_TEST_HOOK")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = command.spawn().unwrap();
        let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
        let out = child.wait_with_output().unwrap();
        let value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
        (out, value)
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn only_this_installations_services_and_files_go() {
    let f = Fixture::new();
    let mine = f.service("work", &f.cli());
    let theirs = f.service("home", &f.root.join("other/mailtriage"));
    let brew = f.service(
        "brew",
        Path::new("/opt/homebrew/opt/mailtriage/bin/mailtriage"),
    );
    let cache = cache_dir(&f.home(), &f.root.join("xdg"));
    fs::create_dir_all(&cache).unwrap();
    let other_key = f.root.join("other/mailtriage").to_str().unwrap().to_owned();
    fs::write(
        cache.join("update.json"),
        json!({"schema_version": 1, "installs": {
            f.cli().to_str().unwrap(): {"version": "0.1.0"},
            other_key.clone(): {"version": "0.1.0"},
        }})
        .to_string(),
    )
    .unwrap();
    let (out, v) = f.uninstall(&["--yes"], "", &[]);
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    let s = &v["self_uninstall"];
    assert_eq!(s["dir"], f.root.join("bin").to_str().unwrap());
    assert_eq!(
        s["services"],
        json!([{"account": "work", "unit_path": mine, "action": "uninstalled", "error": null}])
    );
    assert_eq!(s["tray"], "not_installed");
    assert!(!mine.exists());
    assert!(theirs.exists() && brew.exists());
    assert!(!f.cli().exists());
    assert!(!f.root.join("bin/mailtriage.previous").exists());
    assert_eq!(
        s["removed"],
        json!([f.cli(), f.root.join("bin/mailtriage.previous")])
    );
    // The lock file stays, and is listed with what was kept.
    let lock = f.root.join("bin/.mailtriage-update.lock");
    assert!(lock.exists());
    assert_eq!(s["kept"][0], lock.to_str().unwrap());
    let installs: Value =
        serde_json::from_slice(&fs::read(cache.join("update.json")).unwrap()).unwrap();
    assert!(installs["installs"]
        .get(f.cli().to_str().unwrap())
        .is_none());
    assert!(installs["installs"].get(&other_key).is_some());
    assert!(stderr(&out).contains("no mail was touched"));
}

#[test]
fn a_service_that_names_another_executable_once_locked_is_skipped() {
    let f = Fixture::new();
    let unit = f.service("work", &f.cli());
    // Between the listing and the locked re-read, the file is rewritten for
    // another installation.
    let replacement = f.root.join("replacement");
    let other = f.service("work", &f.root.join("other/mailtriage"));
    fs::rename(&other, &replacement).unwrap();
    f.service("work", &f.cli());
    let hook = f.root.join("hook");
    write_exe(
        &hook,
        &format!(
            "#!/bin/sh\n[ \"$1\" = uninstall_service ] && cp '{}' '{}'\nexit 0\n",
            replacement.display(),
            unit.display()
        ),
    );
    let (out, v) = f.uninstall(
        &["--yes"],
        "",
        &[("MAILTRIAGE_UPDATE_TEST_HOOK", hook.to_str().unwrap())],
    );
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    assert_eq!(v["self_uninstall"]["services"][0]["action"], "skipped");
    assert!(unit.exists(), "the other installation's service stays");
}

#[test]
fn a_failing_service_removal_keeps_every_file() {
    let f = Fixture::new();
    if mailtriage_uid() == 0 {
        return;
    }
    let unit = f.service("work", &f.cli());
    let units = unit.parent().unwrap().to_path_buf();
    fs::set_permissions(&units, fs::Permissions::from_mode(0o555)).unwrap();
    let (out, v) = f.uninstall(&["--yes"], "", &[]);
    fs::set_permissions(&units, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(out.status.code(), Some(3), "{v}");
    let s = &v["self_uninstall"];
    assert_eq!(s["services"][0]["action"], "failed");
    assert_eq!(s["removed"], json!([]));
    assert!(!s["failures"].as_array().unwrap().is_empty());
    assert!(f.cli().exists() && unit.exists());
    assert!(stderr(&out).contains("No program files were removed"));
}

fn mailtriage_uid() -> u32 {
    let out = Command::new("id").arg("-u").output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

#[cfg(target_os = "linux")]
#[test]
fn without_a_service_manager_there_are_no_services() {
    let f = Fixture::new();
    let unit = f.service("work", &f.cli());
    let (out, v) = f.uninstall(&["--yes"], "", &[("PATH", "/nonexistent")]);
    assert_eq!(out.status.code(), Some(0), "{v}");
    assert_eq!(v["self_uninstall"]["services"], json!([]));
    assert!(unit.exists());
}

#[test]
fn without_home_nothing_is_removed() {
    let f = Fixture::new();
    let unit = f.service("work", &f.cli());
    let lock = f.root.join("bin/.mailtriage-update.lock");
    let unchanged = |out: &Output, v: &Value| {
        assert_eq!(out.status.code(), Some(2), "{v} {}", stderr(out));
        assert_eq!(v["error"]["message"], "HOME is not set");
        assert!(f.cli().exists() && f.root.join("bin/mailtriage.previous").exists());
        assert!(unit.exists());
        // Refused before the installation lock was taken.
        assert!(!lock.exists());
    };
    // HOME empty.
    let (out, v) = f.uninstall(&["--yes"], "", &[("HOME", "")]);
    unchanged(&out, &v);
    // HOME unset.
    let out = Command::new(f.cli())
        .args(["self", "uninstall", "--json", "--yes"])
        .current_dir(&f.root)
        .env_remove("HOME")
        .env("XDG_CACHE_HOME", f.root.join("xdg"))
        .env("XDG_DATA_HOME", f.root.join("data"))
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", f.root.join("tools").display()),
        )
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("MAILTRIAGE_TEST_TERMINAL")
        .env_remove("MAILTRIAGE_UPDATE_TEST_HOOK")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let v = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    unchanged(&out, &v);
}

#[test]
fn a_homebrew_installation_is_refused() {
    let f = Fixture::new();
    let keg = f.root.join("brew/Cellar/mailtriage/1.2.3/bin");
    fs::create_dir_all(&keg).unwrap();
    fs::copy(f.cli(), keg.join("mailtriage")).unwrap();
    let (out, v) = f.uninstall(&["--yes", "--dir", keg.to_str().unwrap()], "", &[]);
    assert_eq!(out.status.code(), Some(2), "{v}");
    assert_eq!(
        v["error"]["message"],
        "installed by Homebrew; run brew uninstall mailtriage"
    );
    assert!(keg.join("mailtriage").exists());
}

#[test]
fn the_tray_quits_and_its_login_item_goes() {
    let f = Fixture::new();
    let tray = f.root.join("bin/mailtriage-tray");
    write_exe(&tray, &fake_tray("0.1.0"));
    let item = f.login_item(&tray);
    // launchd has the login job loaded.
    fs::write(f.root.join("tools/loaded"), "").unwrap();
    let (out, v) = f.uninstall(&["--yes"], "", &[]);
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    let s = &v["self_uninstall"];
    assert_eq!(s["tray"], "quit");
    assert!(!item.exists());
    assert!(!tray.exists() && !f.cli().exists());
    assert!(s["removed"].as_array().unwrap().contains(&json!(item)));
    if cfg!(target_os = "macos") {
        let log = fs::read_to_string(f.root.join("tools/launchctl.log")).unwrap();
        assert!(
            log.lines().any(|l| l.starts_with("bootout gui/")
                && l.ends_with("/digital.wirdrei.mailtriage-tray")),
            "{log}"
        );
    }
    assert!(stderr(&out).contains("Close any open categories window"));
}

#[test]
fn a_tray_that_does_not_stop_keeps_every_file() {
    let f = Fixture::new();
    let tray = f.root.join("bin/mailtriage-tray");
    write_exe(&tray, &fake_tray("0.1.0"));
    fs::write(
        f.root.join("bin/quit.json"),
        r#"{"schema_version":1,"error":{"code":3,"message":"the tray does not respond; quit it from its menu"}}"#,
    )
    .unwrap();
    fs::write(f.root.join("bin/quit.code"), "3").unwrap();
    let (out, v) = f.uninstall(&["--yes"], "", &[]);
    assert_eq!(out.status.code(), Some(3), "{v}");
    let s = &v["self_uninstall"];
    assert_eq!(s["tray"], "failed");
    assert_eq!(
        s["failures"],
        json!(["tray: the tray does not respond; quit it from its menu"])
    );
    assert!(tray.exists() && f.cli().exists());
}

#[test]
fn another_installations_tray_is_left_alone() {
    let f = Fixture::new();
    let tray = f.root.join("bin/mailtriage-tray");
    write_exe(&tray, &fake_tray("0.1.0"));
    fs::write(
        f.root.join("bin/quit.json"),
        r#"{"schema_version":1,"quit":"other_installation"}"#,
    )
    .unwrap();
    let (out, v) = f.uninstall(&["--yes"], "", &[]);
    assert_eq!(out.status.code(), Some(0), "{v}");
    assert_eq!(v["self_uninstall"]["tray"], "other_installation");
    assert!(!tray.exists());
}

#[test]
fn an_update_holding_the_lock_delays_uninstall_and_the_lock_file_survives() {
    let f = Fixture::new();
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(f.root.join("bin/.mailtriage-update.lock"))
        .unwrap();
    lock.lock_exclusive().unwrap();
    let (out, v) = f.uninstall(
        &["--yes"],
        "",
        &[("MAILTRIAGE_UPDATE_TEST_LOCK_WAIT_MS", "300")],
    );
    assert_eq!(out.status.code(), Some(5), "{v}");
    assert!(f.cli().exists());
    drop(lock);
    let (out, _) = f.uninstall(&["--yes"], "", &[]);
    assert_eq!(out.status.code(), Some(0));
    assert!(f.root.join("bin/.mailtriage-update.lock").exists());
}

#[test]
fn it_asks_first_and_refuses_without_a_terminal() {
    let f = Fixture::new();
    let (out, v) = f.uninstall(&[], "", &[]);
    assert_eq!(out.status.code(), Some(2), "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("--yes"));
    let terminal = [("MAILTRIAGE_TEST_TERMINAL", "1")];
    for answer in ["n\n", "\n", ""] {
        let (out, v) = f.uninstall(&[], answer, &terminal);
        assert_eq!(out.status.code(), Some(2), "{answer:?}: {v}");
        assert_eq!(
            v["error"]["message"],
            "uninstall cancelled; nothing was changed"
        );
        assert!(stderr(&out).contains("[y/N]"));
        assert!(f.cli().exists());
    }
    let (out, v) = f.uninstall(&[], "y\n", &terminal);
    assert_eq!(out.status.code(), Some(0), "{v}");
    assert!(!f.cli().exists());
}
