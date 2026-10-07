//! Start at login against a temporary HOME and a fake `launchctl`.
mod support;
use mailtriage_tray::autostart::{self, Env, Platform};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use support::{write_script, FakeCli, AUTOSTART_LAUNCHCTL};

fn env(platform: Platform, dir: &Path) -> Env {
    write_script(&dir.join("launchctl"), AUTOSTART_LAUNCHCTL);
    Env {
        platform,
        home: dir.join("home"),
        xdg_config_home: None,
        launchctl: dir.join("launchctl"),
        uid: 501,
        path_env: Some("/usr/bin:/bin".into()),
        tray: PathBuf::from("/Apps/mail tray/mailtriage-tray"),
    }
}

const CLI: &str = "/opt/mail triage/mailtriage";
const CONFIG: &str = "/Users/a & b/50% $HOME \"x\"/mailtriage.json";

#[test]
fn the_plist_records_both_paths() {
    let dir = tempfile::tempdir().unwrap();
    let env = env(Platform::MacOs, dir.path());
    let expected = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>XMailtriageManaged</key>
  <true/>
  <key>Label</key>
  <string>digital.wirdrei.mailtriage-tray</string>
  <key>ProgramArguments</key>
  <array>
    <string>/Apps/mail tray/mailtriage-tray</string>
    <string>--config</string>
    <string>/Users/a &amp; b/50% $HOME "x"/mailtriage.json</string>
    <string>--mailtriage</string>
    <string>/opt/mail triage/mailtriage</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>/usr/bin:/bin</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>LimitLoadToSessionType</key>
  <string>Aqua</string>
</dict>
</plist>
"#;
    assert_eq!(
        autostart::plist(&env, Path::new(CLI), Path::new(CONFIG)),
        expected
    );
}

#[test]
fn the_desktop_entry_quotes_exec_per_the_specification() {
    let dir = tempfile::tempdir().unwrap();
    let env = env(Platform::Linux, dir.path());
    let text = autostart::desktop(&env, Path::new(CLI), Path::new(CONFIG));
    assert_eq!(
        text,
        "# managed by mailtriage\n[Desktop Entry]\nType=Application\nName=mailtriage\nExec=\"/Apps/mail tray/mailtriage-tray\" --config \"/Users/a & b/50%% \\\\$HOME \\\\\"x\\\\\"/mailtriage.json\" --mailtriage \"/opt/mail triage/mailtriage\"\nNoDisplay=true\n"
    );
    let exec = text.lines().find_map(|l| l.strip_prefix("Exec=")).unwrap();
    assert_eq!(
        autostart::desktop_arguments(exec).unwrap(),
        [
            "/Apps/mail tray/mailtriage-tray",
            "--config",
            CONFIG,
            "--mailtriage",
            CLI
        ]
    );
    assert_eq!(autostart::desktop_arg("a\\b"), "\"a\\\\\\\\b\"");
    assert_eq!(
        autostart::desktop_arguments(&autostart::desktop_arg("a\\b")).unwrap(),
        ["a\\b"]
    );
}

#[test]
fn enable_clears_a_disabled_label_and_status_follows_it() {
    let dir = tempfile::tempdir().unwrap();
    let env = env(Platform::MacOs, dir.path());
    fs::write(dir.path().join("disabled"), "").unwrap();
    let entry = autostart::status(&env).unwrap();
    assert!(!entry.enabled);
    assert_eq!(entry.config, None);
    let entry = autostart::enable(&env, Path::new(CLI), Path::new(CONFIG)).unwrap();
    assert!(entry.enabled);
    assert_eq!(entry.config.as_deref(), Some(Path::new(CONFIG)));
    assert_eq!(entry.mailtriage.as_deref(), Some(Path::new(CLI)));
    assert_eq!(
        entry.path,
        dir.path()
            .join("home/Library/LaunchAgents/digital.wirdrei.mailtriage-tray.plist")
    );
    let log = fs::read_to_string(dir.path().join("launchctl.log")).unwrap();
    assert!(log.contains("enable gui/501/digital.wirdrei.mailtriage-tray\n"));
    assert!(!log.contains("bootstrap"), "enable never loads the job");
    // A label disabled afterwards: the file stays, status says off.
    fs::write(dir.path().join("disabled"), "").unwrap();
    assert!(!autostart::status(&env).unwrap().enabled);
    let entry = autostart::disable(&env).unwrap();
    assert!(!entry.enabled);
    assert!(!entry.path.exists());
}

/// The "Start at login" item shows this error as a notice: it names the
/// command plainly, without Markdown.
#[test]
fn a_failed_launchctl_enable_exits_3_and_names_the_command() {
    let dir = tempfile::tempdir().unwrap();
    let env = env(Platform::MacOs, dir.path());
    write_script(&env.launchctl, "#!/bin/sh\nexit 1\n");
    let e = autostart::enable(&env, Path::new(CLI), Path::new(CONFIG)).unwrap_err();
    assert_eq!(
        (e.code, e.message.as_str()),
        (
            3,
            "launchctl enable gui/501/digital.wirdrei.mailtriage-tray failed"
        )
    );
}

#[test]
fn linux_uses_xdg_config_home_when_absolute() {
    let dir = tempfile::tempdir().unwrap();
    let mut env = env(Platform::Linux, dir.path());
    env.xdg_config_home = Some(dir.path().join("xdg").into_os_string());
    let entry = autostart::enable(&env, Path::new(CLI), Path::new(CONFIG)).unwrap();
    assert!(entry.enabled);
    assert_eq!(
        entry.path,
        dir.path().join("xdg/autostart/mailtriage-tray.desktop")
    );
    assert_eq!(entry.config.as_deref(), Some(Path::new(CONFIG)));
    env.xdg_config_home = Some("relative".into());
    assert_eq!(
        autostart::path(&env),
        dir.path()
            .join("home/.config/autostart/mailtriage-tray.desktop")
    );
    assert!(!dir.path().join("launchctl.log").exists());
}

#[test]
fn a_file_without_the_marker_is_left_alone() {
    for platform in [Platform::MacOs, Platform::Linux] {
        let dir = tempfile::tempdir().unwrap();
        let env = env(platform, dir.path());
        let path = autostart::path(&env);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "mine\n").unwrap();
        for result in [
            autostart::enable(&env, Path::new(CLI), Path::new(CONFIG)),
            autostart::disable(&env),
            autostart::status(&env),
        ] {
            assert_eq!(result.unwrap_err().code, 5);
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), "mine\n");
    }
}

fn tray(dir: &Path, args: &[&str]) -> (i32, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_mailtriage-tray"))
        .args(args)
        .current_dir(dir)
        .env("HOME", dir.join("home"))
        .env("XDG_CONFIG_HOME", dir.join("xdg"))
        .env("PATH", format!("{}:/usr/bin:/bin", dir.display()))
        .env_remove("MAILTRIAGE_CONFIG")
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null),
    )
}

#[test]
fn the_command_records_absolute_paths() {
    let fake = FakeCli::new();
    let dir = fake.dir.path();
    write_script(&dir.join("launchctl"), AUTOSTART_LAUNCHCTL);
    fs::write(dir.join("mailtriage.json"), "{}").unwrap();
    let (code, v) = tray(
        dir,
        &[
            "autostart",
            "enable",
            "--config",
            "mailtriage.json",
            "--mailtriage",
            "mailtriage",
            "--json",
        ],
    );
    assert_eq!(code, 0, "{v}");
    let root = fs::canonicalize(dir).unwrap();
    assert_eq!(v["autostart"]["enabled"], true);
    assert_eq!(
        v["autostart"]["config"],
        root.join("mailtriage.json").display().to_string()
    );
    assert_eq!(
        v["autostart"]["mailtriage"],
        root.join("mailtriage").display().to_string()
    );
    let text = fs::read_to_string(v["autostart"]["path"].as_str().unwrap()).unwrap();
    assert!(text.contains(&root.join("mailtriage.json").display().to_string()));
    let (code, v) = tray(dir, &["autostart", "status", "--json"]);
    assert_eq!(
        (code, v["autostart"]["enabled"].clone()),
        (0, Value::Bool(true))
    );
    let (code, v) = tray(dir, &["autostart", "disable", "--json"]);
    assert_eq!(
        (code, v["autostart"]["enabled"].clone()),
        (0, Value::Bool(false))
    );
}

#[test]
fn enable_without_a_config_exits_3_when_status_fails() {
    let fake = FakeCli::new();
    // The fake `launchctl` comes first on the binary's PATH, so no code path
    // can reach the real one.
    write_script(&fake.dir.path().join("launchctl"), AUTOSTART_LAUNCHCTL);
    fake.fail("service-status", 3, "Operation failed", None);
    let (code, v) = tray(
        fake.dir.path(),
        &[
            "autostart",
            "enable",
            "--mailtriage",
            "mailtriage",
            "--json",
        ],
    );
    assert_eq!(code, 3);
    assert_eq!(
        v["error"]["message"],
        "cannot find the config: Operation failed; pass --config PATH"
    );
    assert_eq!(fake.calls(), [["service", "status", "--json"]]);
    let (code, _) = tray(fake.dir.path(), &["autostart", "enable", "--bogus"]);
    assert_eq!(code, 2);
}
