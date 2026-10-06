#![cfg(unix)]
//! Service files from pure functions, both managers against fake tools, and
//! the `service` commands through the binary.
mod common;
use common::{write_tool, LAUNCHCTL, SYSTEMCTL};
use mailtriage::{
    service::ServiceError,
    system_service::{self, Context, Manager, Unit},
};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn spaced_unit() -> Unit {
    Unit {
        account: "work".into(),
        exe: PathBuf::from("/opt/mail triage/bin/mailtriage"),
        config: PathBuf::from("/Users/a & b/.config/mailtriage/mailtriage.json"),
        interval_seconds: 60,
        limit: 100,
        log_dir: PathBuf::from("/Users/a & b/.config/mailtriage/logs"),
        path_env: Some("/opt/homebrew/bin:/usr/bin:/bin".into()),
    }
}

#[test]
fn the_plist_is_fixed_text_with_escaped_paths() {
    let expected = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>XMailtriageManaged</key>
  <true/>
  <key>Label</key>
  <string>digital.wirdrei.mailtriage.work</string>
  <key>ProgramArguments</key>
  <array>
    <string>/opt/mail triage/bin/mailtriage</string>
    <string>watch</string>
    <string>--config</string>
    <string>/Users/a &amp; b/.config/mailtriage/mailtriage.json</string>
    <string>--account</string>
    <string>work</string>
    <string>--interval-seconds</string>
    <string>60</string>
    <string>--limit</string>
    <string>100</string>
    <string>--json</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>/opt/homebrew/bin:/usr/bin:/bin</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ThrottleInterval</key>
  <integer>30</integer>
  <key>Umask</key>
  <integer>63</integer>
  <key>StandardOutPath</key>
  <string>/Users/a &amp; b/.config/mailtriage/logs/work.log</string>
  <key>StandardErrorPath</key>
  <string>/Users/a &amp; b/.config/mailtriage/logs/work.err</string>
</dict>
</plist>
"#;
    assert_eq!(system_service::plist(&spaced_unit()), expected);
}

#[test]
fn the_unit_is_fixed_text_with_quoted_paths() {
    let expected = "# managed by mailtriage
[Unit]
Description=mailtriage watch for account work

[Service]
Type=simple
ExecStart=\"/opt/mail triage/bin/mailtriage\" watch --config \"/Users/a & b/.config/mailtriage/mailtriage.json\" --account work --interval-seconds 60 --limit 100 --json
Environment=\"PATH=/opt/homebrew/bin:/usr/bin:/bin\"
Restart=on-failure
RestartSec=30
UMask=0077

[Install]
WantedBy=default.target
";
    assert_eq!(system_service::systemd_unit(&spaced_unit()), expected);
}

fn unit_in(dir: &Path) -> Unit {
    Unit {
        account: "work".into(),
        exe: dir.join("mailtriage"),
        config: dir.join("mailtriage.json"),
        interval_seconds: 60,
        limit: 100,
        log_dir: dir.join("logs"),
        path_env: None,
    }
}

fn context(manager: Manager, dir: &Path) -> Context {
    write_tool(dir, "launchctl", LAUNCHCTL);
    write_tool(dir, "systemctl", SYSTEMCTL);
    let tool = match manager {
        Manager::Launchd => dir.join("launchctl"),
        Manager::Systemd => dir.join("systemctl"),
    };
    Context {
        manager,
        home: dir.join("home"),
        tool,
        uid: 501,
    }
}

fn calls(dir: &Path, tool: &str) -> Vec<String> {
    fs::read_to_string(dir.join(format!("{tool}.log")))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn code(result: anyhow::Result<Value>) -> i32 {
    result
        .unwrap_err()
        .downcast_ref::<ServiceError>()
        .unwrap()
        .code
}

#[test]
fn launchd_install_reload_status_and_uninstall() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = context(Manager::Launchd, dir.path());
    let unit = unit_in(dir.path());
    let target = "gui/501/digital.wirdrei.mailtriage.work";
    let out = system_service::install(&ctx, &unit).unwrap();
    let path = dir
        .path()
        .join("home/Library/LaunchAgents/digital.wirdrei.mailtriage.work.plist");
    assert_eq!(out["action"], "installed");
    assert_eq!(out["unit_path"], path.to_str().unwrap());
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        system_service::plist(&unit)
    );
    assert!(dir.path().join("logs").is_dir());
    assert_eq!(
        calls(dir.path(), "launchctl"),
        vec![
            format!("print {target}"),
            format!("enable {target}"),
            format!("bootstrap gui/501 {}", path.display())
        ]
    );
    let s = system_service::status(Some(&ctx), "work", &unit.log_dir);
    assert_eq!(s["manager"], "launchd");
    assert_eq!(s["installed"], true);
    assert_eq!(s["loaded"], true);
    assert_eq!(s["running"], true);
    assert_eq!(s["pid"], 4242);
    assert_eq!(s["last_exit_status"], 0);
    assert_eq!(
        s["log_paths"],
        json!([
            dir.path().join("logs/work.log"),
            dir.path().join("logs/work.err")
        ])
    );
    // Installing again reloads: bootout, wait until launchd no longer lists
    // the job, enable the label, then bootstrap, same file.
    system_service::install(&ctx, &unit).unwrap();
    let log = calls(dir.path(), "launchctl");
    assert_eq!(
        log[log.len() - 5..],
        [
            format!("print {target}"),
            format!("bootout {target}"),
            format!("print {target}"),
            format!("enable {target}"),
            format!("bootstrap gui/501 {}", path.display())
        ]
    );
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        system_service::plist(&unit)
    );
    let out = system_service::uninstall(&ctx, "work").unwrap();
    assert_eq!(out["action"], "uninstalled");
    assert!(!path.exists());
    let s = system_service::status(Some(&ctx), "work", &unit.log_dir);
    assert_eq!(
        (
            s["installed"].clone(),
            s["loaded"].clone(),
            s["running"].clone()
        ),
        (json!(false), json!(false), json!(false))
    );
    assert_eq!(
        system_service::uninstall(&ctx, "work").unwrap()["action"],
        "not_installed"
    );
}

#[test]
fn systemd_install_status_and_uninstall() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = context(Manager::Systemd, dir.path());
    let unit = unit_in(dir.path());
    system_service::install(&ctx, &unit).unwrap();
    let path = dir
        .path()
        .join("home/.config/systemd/user/mailtriage-work.service");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        system_service::systemd_unit(&unit)
    );
    assert_eq!(
        calls(dir.path(), "systemctl"),
        [
            "--user daemon-reload",
            "--user enable mailtriage-work.service",
            "--user restart mailtriage-work.service"
        ]
    );
    let s = system_service::status(Some(&ctx), "work", &unit.log_dir);
    assert_eq!(s["manager"], "systemd");
    assert_eq!(
        (s["loaded"].clone(), s["running"].clone(), s["pid"].clone()),
        (json!(true), json!(true), json!(4343))
    );
    assert_eq!(s["log_paths"], json!([]));
    system_service::uninstall(&ctx, "work").unwrap();
    assert!(!path.exists());
    // Install's three calls and status's `show` come first.
    let log = calls(dir.path(), "systemctl");
    assert_eq!(
        log[4..],
        [
            "--user disable --now mailtriage-work.service",
            "--user daemon-reload"
        ]
    );
    let s = system_service::status(Some(&ctx), "work", &unit.log_dir);
    assert_eq!(
        (s["loaded"].clone(), s["running"].clone(), s["pid"].clone()),
        (json!(false), json!(false), Value::Null)
    );
}

#[test]
fn files_mailtriage_did_not_write_are_never_touched() {
    for manager in [Manager::Launchd, Manager::Systemd] {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(manager, dir.path());
        let path = ctx.unit_path("work");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "the user's own file\n").unwrap();
        assert_eq!(code(system_service::install(&ctx, &unit_in(dir.path()))), 5);
        assert_eq!(code(system_service::uninstall(&ctx, "work")), 5);
        assert_eq!(fs::read_to_string(&path).unwrap(), "the user's own file\n");
        assert!(
            calls(dir.path(), "launchctl").is_empty() && calls(dir.path(), "systemctl").is_empty()
        );
        assert_eq!(
            system_service::status(Some(&ctx), "work", dir.path())["installed"],
            false
        );
    }
}

/// Final review I3: the note for a key from the environment names the
/// platform's own store, the account and the config, and warns against
/// putting the key into the service file.
#[test]
fn the_install_note_names_the_platform_store_and_the_account() {
    for (manager, store, file) in [
        (
            Manager::Launchd,
            "--key-store keychain` (or `--key-store command`)",
            "plist",
        ),
        (
            Manager::Systemd,
            "--key-store secret-service` (or `--key-store pass`, `--key-store command`)",
            "unit",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(manager, dir.path());
        let path = dir.path().join("mailtriage.json");
        let mut c = mailtriage::config::default_config();
        c.provider.kind = "openrouter".into();
        c.provider.model = "typesafe/jev-1.13".into();
        c.provider.endpoint = mailtriage::provider::DECISIONS_ENDPOINT.into();
        c.provider.api_key_env = "OPENROUTER_API_KEY".into();
        mailtriage::config::save(&path, &c).unwrap();
        let service = mailtriage::service::Service::open(&path).unwrap();
        let out = system_service::install_account(&service, &path, "work", 60, 100, &ctx).unwrap();
        let note = out["note"].as_str().unwrap();
        let config = fs::canonicalize(&path).unwrap();
        assert!(
            note.contains(&format!(
                "`mailtriage setup --update --config {} --account work {store}",
                config.display()
            )),
            "{note}"
        );
        assert!(
            note.contains(&format!(
                "Do not put the key into the {file} yourself: that file is readable, and `service install` rewrites it."
            )),
            "{note}"
        );
    }
}

#[test]
fn a_failing_manager_is_exit_3() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = context(Manager::Systemd, dir.path());
    write_tool(dir.path(), "broken", "#!/bin/sh\nexit 1\n");
    ctx.tool = dir.path().join("broken");
    assert_eq!(code(system_service::install(&ctx, &unit_in(dir.path()))), 3);
}

#[test]
fn status_without_a_manager() {
    let s = system_service::status(None, "work", Path::new("/logs"));
    assert_eq!(
        s,
        json!({"manager":"none","installed":false,"loaded":false,"running":false,"pid":null,"last_exit_status":null,"unit_path":null,"log_paths":[]})
    );
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn service_commands_through_the_cli() {
    let dir = tempfile::tempdir().unwrap();
    let (home, bin, cwd) = (
        dir.path().join("home"),
        dir.path().join("bin"),
        dir.path().join("cwd"),
    );
    for d in [&home, &bin, &cwd] {
        fs::create_dir_all(d).unwrap();
    }
    write_tool(&bin, "launchctl", LAUNCHCTL);
    write_tool(&bin, "systemctl", SYSTEMCTL);
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
            .current_dir(&cwd)
            .args(args)
            .env("HOME", &home)
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env_remove("MAILTRIAGE_CONFIG")
            .output()
            .unwrap();
        let value: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
        (out.status.code(), value)
    };
    assert_eq!(run(&["init", "--json"]).0, Some(0));
    let (code, v) = run(&["service", "status", "--account", "work", "--json"]);
    assert_eq!(code, Some(0), "{v}");
    let manager = if cfg!(target_os = "macos") {
        "launchd"
    } else {
        "systemd"
    };
    assert_eq!(v["service"]["manager"], manager);
    assert_eq!(v["service"]["installed"], false);
    assert_eq!(v["service"]["last_pass"], Value::Null);
    assert_eq!(run(&["sync", "--account", "work", "--json"]).0, Some(0));
    let (_, v) = run(&["service", "status", "--account", "work", "--json"]);
    assert_eq!(v["service"]["last_pass"]["exit_code"], 0);
    assert_eq!(v["service"]["last_pass"]["mode"], "off");

    let (code, v) = run(&[
        "service",
        "install",
        "--account",
        "work",
        "--interval-seconds",
        "120",
        "--json",
    ]);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["service"]["action"], "installed");
    let text = fs::read_to_string(v["service"]["unit_path"].as_str().unwrap()).unwrap();
    let config = fs::canonicalize(cwd.join("mailtriage.json")).unwrap();
    assert!(text.contains(config.to_str().unwrap()), "{text}");
    assert!(text.contains("120"), "{text}");
    let (_, v) = run(&["service", "status", "--account", "work", "--json"]);
    assert_eq!(v["service"]["installed"], true);
    assert_eq!(v["service"]["running"], true);
    let (code, v) = run(&["service", "uninstall", "--account", "work", "--json"]);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["service"]["action"], "uninstalled");
    assert_eq!(
        run(&["service", "install", "--account", "nobody", "--json"]).0,
        Some(2)
    );
}

/// Fake `launchctl` that, like the real one, still lists a job briefly after
/// `bootout` returns (two more `print` calls) and refuses `bootstrap` while it does.
const SLOW_LAUNCHCTL: &str = "#!/bin/sh\ndir=\"$(dirname \"$0\")\"\necho \"$*\" >> \"$dir/launchctl.log\"\ncase \"$1\" in\n  bootstrap) [ -f \"$dir/loaded\" ] && exit 5; touch \"$dir/loaded\" ;;\n  enable) ;;\n  bootout) [ -f \"$dir/loaded\" ] || exit 3; echo 2 > \"$dir/lingering\" ;;\n  print)\n    if [ -f \"$dir/lingering\" ]; then n=$(cat \"$dir/lingering\"); if [ \"$n\" -le 0 ]; then rm -f \"$dir/lingering\" \"$dir/loaded\"; exit 113; fi; echo $((n-1)) > \"$dir/lingering\"; fi\n    [ -f \"$dir/loaded\" ] || exit 113\n    printf '%s = {\\n\\tstate = running\\n}\\n' \"$2\" ;;\n  *) exit 64 ;;\nesac\n";

#[test]
fn reinstall_waits_until_launchd_drops_the_old_job() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = context(Manager::Launchd, dir.path());
    write_tool(dir.path(), "launchctl", SLOW_LAUNCHCTL);
    let unit = unit_in(dir.path());
    system_service::install(&ctx, &unit).unwrap();
    // Reinstalling a loaded job: bootout, then print until it is gone, then bootstrap.
    system_service::install(&ctx, &unit).unwrap();
    let log = calls(dir.path(), "launchctl");
    let target = "gui/501/digital.wirdrei.mailtriage.work";
    let after_bootout: Vec<&str> = log
        .iter()
        .skip_while(|call| !call.starts_with("bootout"))
        .map(String::as_str)
        .collect();
    assert_eq!(after_bootout[0], format!("bootout {target}"));
    assert_eq!(
        after_bootout
            .iter()
            .filter(|c| c.starts_with("print"))
            .count(),
        3
    );
    assert!(after_bootout
        .last()
        .unwrap()
        .starts_with("bootstrap gui/501 "));
    assert_eq!(
        system_service::status(Some(&ctx), "work", &unit.log_dir)["running"],
        true
    );
    // Uninstall waits the same way, so an install right after it succeeds.
    system_service::uninstall(&ctx, "work").unwrap();
    system_service::install(&ctx, &unit).unwrap();
}
