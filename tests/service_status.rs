#![cfg(unix)]
//! `service status` for every account and the fields the tray reads: the
//! config a service runs with, whether it is enabled, its interval, filing
//! mode and identity. Fake `launchctl`/`systemctl` and a fake `/proc`.
mod common;
use common::{config_file, unit_for, write_proc, write_tool, LAUNCHCTL, SYSTEMCTL};
use mailtriage::{
    config,
    service_control::{self, Inspection},
    system_service::{self, Context, Manager, Unit},
};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn context(manager: Manager, dir: &Path) -> Context {
    write_tool(dir, "launchctl", LAUNCHCTL);
    write_tool(dir, "systemctl", SYSTEMCTL);
    Context {
        manager,
        home: dir.join("home"),
        tool: dir.join(match manager {
            Manager::Launchd => "launchctl",
            Manager::Systemd => "systemctl",
        }),
        uid: 501,
    }
}

/// A fake `/proc` in `dir` with process `pid` running `unit`.
fn proc_for(dir: &Path, pid: u32, unit: &Unit) -> PathBuf {
    let root = dir.join("proc");
    write_proc(&root, pid, unit);
    root
}

fn inspect(ctx: &Context, proc_root: &Path) -> Inspection {
    service_control::inspect(ctx, "work", proc_root)
}

fn matches(ctx: &Context, proc_root: &Path, config: &Path) -> Option<bool> {
    service_control::config_matches(&inspect(ctx, proc_root), ctx.manager, config)
}

#[test]
fn two_configs_that_share_an_account_name() {
    for manager in [Manager::Launchd, Manager::Systemd] {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(manager, dir.path());
        let (a, b) = (
            config_file(dir.path(), "a.json"),
            config_file(dir.path(), "b.json"),
        );
        let unit = unit_for(dir.path(), &a);
        system_service::install(&ctx, &unit).unwrap();
        let proc_root = proc_for(dir.path(), 4343, &unit);
        let found = inspect(&ctx, &proc_root);
        assert_eq!(
            found.service_config.as_deref(),
            Some(a.as_path()),
            "{manager:?}"
        );
        assert_eq!(found.file_config.as_deref(), Some(a.as_path()));
        assert_eq!(found.interval_seconds, Some(60));
        assert_eq!(matches(&ctx, &proc_root, &a), Some(true), "{manager:?}");
        assert_eq!(matches(&ctx, &proc_root, &b), Some(false), "{manager:?}");
        assert_eq!(
            service_control::other_config(&found, &b).as_deref(),
            Some(a.as_path())
        );
    }
}

/// A config path with spaces, `&`, `%`, `$` and quotes names the same config
/// in every source: the running process, the loaded definition and the file.
#[test]
fn a_config_path_with_special_characters_matches() {
    for manager in [Manager::Launchd, Manager::Systemd] {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(manager, dir.path());
        let odd = dir.path().join("a & b/50% $HOME \"x\"");
        fs::create_dir_all(&odd).unwrap();
        let config = config_file(&odd, "mailtriage.json");
        let unit = unit_for(dir.path(), &config);
        system_service::install(&ctx, &unit).unwrap();
        let proc_root = proc_for(dir.path(), 4343, &unit);
        let found = inspect(&ctx, &proc_root);
        assert!(found.running, "{manager:?}");
        assert_eq!(
            found.service_config.as_deref(),
            Some(config.as_path()),
            "{manager:?}"
        );
        assert_eq!(found.file_config.as_deref(), Some(config.as_path()));
        assert_eq!(
            matches(&ctx, &proc_root, &config),
            Some(true),
            "{manager:?}"
        );
        if manager == Manager::Systemd {
            // Loaded but not running: the config of the loaded `ExecStart`.
            fs::remove_file(dir.path().join("active")).unwrap();
            let found = inspect(&ctx, &proc_root);
            assert!(!found.running);
            assert_eq!(found.service_config.as_deref(), Some(config.as_path()));
            assert_eq!(matches(&ctx, &proc_root, &config), Some(true));
        }
    }
}

/// An install for config B whose reload failed: the file names B, the
/// loaded job still runs A.
#[test]
fn a_unit_rewritten_for_another_config_while_the_old_job_runs() {
    for manager in [Manager::Launchd, Manager::Systemd] {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(manager, dir.path());
        let (a, b) = (
            config_file(dir.path(), "a.json"),
            config_file(dir.path(), "b.json"),
        );
        let unit_a = unit_for(dir.path(), &a);
        system_service::install(&ctx, &unit_a).unwrap();
        let proc_root = proc_for(dir.path(), 4343, &unit_a);
        let unit_b = unit_for(dir.path(), &b);
        let text = match manager {
            Manager::Launchd => system_service::plist(&unit_b),
            Manager::Systemd => system_service::systemd_unit(&unit_b),
        };
        fs::write(ctx.unit_path("work"), text).unwrap();
        if manager == Manager::Systemd {
            fs::write(dir.path().join("needs-reload"), "").unwrap();
        }
        assert_eq!(matches(&ctx, &proc_root, &a), Some(false), "{manager:?}");
        assert_eq!(matches(&ctx, &proc_root, &b), Some(false), "{manager:?}");
    }
}

/// systemd: a reload succeeded while the old process keeps running config A.
#[test]
fn systemd_compares_the_running_process_too() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = context(Manager::Systemd, dir.path());
    let (a, b) = (
        config_file(dir.path(), "a.json"),
        config_file(dir.path(), "b.json"),
    );
    let unit_a = unit_for(dir.path(), &a);
    system_service::install(&ctx, &unit_a).unwrap();
    let proc_root = proc_for(dir.path(), 4343, &unit_a);
    fs::write(
        ctx.unit_path("work"),
        system_service::systemd_unit(&unit_for(dir.path(), &b)),
    )
    .unwrap();
    let reload = Command::new(&ctx.tool)
        .args(["--user", "daemon-reload"])
        .status()
        .unwrap();
    assert!(reload.success());
    let found = inspect(&ctx, &proc_root);
    assert_eq!(found.service_config.as_deref(), Some(a.as_path()));
    assert_eq!(found.file_config.as_deref(), Some(b.as_path()));
    assert_eq!(matches(&ctx, &proc_root, &b), Some(false));
    // The process is gone from /proc: its config cannot be read.
    fs::write(dir.path().join("mainpid"), "4444").unwrap();
    assert_eq!(matches(&ctx, &proc_root, &b), None);
}

#[test]
fn failed_queries_and_pending_reloads_are_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = context(Manager::Launchd, dir.path());
    let a = config_file(dir.path(), "a.json");
    system_service::install(&ctx, &unit_for(dir.path(), &a)).unwrap();
    fs::write(dir.path().join("print-fails"), "").unwrap();
    let found = inspect(&ctx, Path::new("/nonexistent"));
    assert_eq!(found.service_config, None);
    assert_eq!(found.loaded, None);
    assert_eq!(matches(&ctx, Path::new("/nonexistent"), &a), None);

    let dir = tempfile::tempdir().unwrap();
    let ctx = context(Manager::Systemd, dir.path());
    let a = config_file(dir.path(), "a.json");
    let unit = unit_for(dir.path(), &a);
    system_service::install(&ctx, &unit).unwrap();
    let proc_root = proc_for(dir.path(), 4343, &unit);
    assert_eq!(matches(&ctx, &proc_root, &a), Some(true));
    fs::write(dir.path().join("needs-reload"), "").unwrap();
    assert_eq!(inspect(&ctx, &proc_root).needs_daemon_reload, Some(true));
    assert_eq!(matches(&ctx, &proc_root, &a), None);
    fs::remove_file(dir.path().join("needs-reload")).unwrap();
    fs::write(dir.path().join("show-fails"), "").unwrap();
    assert_eq!(matches(&ctx, &proc_root, &a), None);
}

#[test]
fn not_installed_matches_and_is_not_enabled() {
    for manager in [Manager::Launchd, Manager::Systemd] {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(manager, dir.path());
        let a = config_file(dir.path(), "a.json");
        let found = inspect(&ctx, Path::new("/nonexistent"));
        assert!(!found.installed);
        assert_eq!(
            (found.enabled, found.enablement.as_str()),
            (Some(false), "not_installed")
        );
        assert_eq!(matches(&ctx, Path::new("/nonexistent"), &a), Some(true));
    }
}

#[test]
fn enabled_and_enablement_for_each_state() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = context(Manager::Launchd, dir.path());
    let a = config_file(dir.path(), "a.json");
    system_service::install(&ctx, &unit_for(dir.path(), &a)).unwrap();
    let state = |ctx: &Context| {
        let found = inspect(ctx, Path::new("/nonexistent"));
        (found.enabled, found.enablement)
    };
    assert_eq!(state(&ctx), (Some(true), "enabled".into()));
    fs::write(
        dir.path().join("disabled"),
        "digital.wirdrei.mailtriage.work\n",
    )
    .unwrap();
    assert_eq!(state(&ctx), (Some(false), "disabled".into()));
    fs::write(dir.path().join("print-disabled-fails"), "").unwrap();
    assert_eq!(state(&ctx), (None, "unknown".into()));

    let dir = tempfile::tempdir().unwrap();
    let ctx = context(Manager::Systemd, dir.path());
    system_service::install(&ctx, &unit_for(dir.path(), &a)).unwrap();
    assert_eq!(state(&ctx), (Some(true), "enabled".into()));
    for (text, enabled) in [("masked", Some(false)), ("static", None)] {
        fs::write(dir.path().join("is-enabled"), format!("{text}\n")).unwrap();
        assert_eq!(state(&ctx), (enabled, text.to_owned()));
    }
}

fn run(dir: &Path, bin: &Path, args: &[&str]) -> (i32, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
        .current_dir(dir.join("cwd"))
        .args(args)
        .env("HOME", dir.join("home"))
        .env("MT_FAKE_HOME", dir.join("home"))
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env_remove("MAILTRIAGE_CONFIG")
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null),
    )
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn status_reports_every_account_through_the_cli() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    for d in ["home", "bin", "cwd"] {
        fs::create_dir_all(dir.path().join(d)).unwrap();
    }
    write_tool(&bin, "launchctl", LAUNCHCTL);
    write_tool(&bin, "systemctl", SYSTEMCTL);
    let path = dir.path().join("cwd/mailtriage.json");
    let mut c = config::default_config();
    c.state_dir = PathBuf::from(".state");
    let mut alpha = c.accounts["work"].clone();
    alpha.identity = "alpha@example.invalid".into();
    alpha.filing.mode = mailtriage::domain::FilingMode::DryRun;
    c.accounts.insert("alpha".into(), alpha);
    config::save(&path, &c).unwrap();
    let config = fs::canonicalize(&path).unwrap();

    let (code, v) = run(dir.path(), &bin, &["service", "status", "--json"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["config"], json!(config));
    let services = v["services"].as_array().unwrap();
    assert_eq!(services.len(), 2);
    assert_eq!(services[0]["account"], "alpha");
    assert_eq!(services[0]["filing_mode"], "dry_run");
    assert_eq!(services[0]["identity"], "alpha@example.invalid");
    assert_eq!(services[1]["account"], "work");
    assert_eq!(services[1]["filing_mode"], "off");
    for s in services {
        assert_eq!(s["installed"], false);
        assert_eq!(s["config_matches"], true);
        assert_eq!(s["enabled"], false);
        assert_eq!(s["enablement"], "not_installed");
        assert_eq!(s["service_config"], Value::Null);
        assert_eq!(s["file_config"], Value::Null);
        assert_eq!(s["interval_seconds"], Value::Null);
    }

    let (code, v) = run(
        dir.path(),
        &bin,
        &[
            "service",
            "install",
            "--account",
            "work",
            "--interval-seconds",
            "120",
            "--json",
        ],
    );
    assert_eq!(code, 0, "{v}");
    // On Linux the fake `systemctl` reports this process as the service's main PID.
    let mut watch = None;
    if cfg!(target_os = "linux") {
        write_tool(&bin, "fake-watch", "#!/bin/sh\nsleep 30\n");
        let child = Command::new(bin.join("fake-watch"))
            .args(["watch", "--config"])
            .arg(&config)
            .args([
                "--account",
                "work",
                "--interval-seconds",
                "120",
                "--limit",
                "100",
                "--json",
            ])
            .spawn()
            .unwrap();
        fs::write(bin.join("mainpid"), child.id().to_string()).unwrap();
        watch = Some(child);
    }
    let (code, v) = run(
        dir.path(),
        &bin,
        &["service", "status", "--account", "work", "--json"],
    );
    if let Some(mut child) = watch {
        child.kill().unwrap();
        child.wait().unwrap();
    }
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["config"], json!(config));
    let s = &v["service"];
    assert_eq!(s["running"], true);
    assert_eq!(s["service_config"], json!(config));
    assert_eq!(s["file_config"], json!(config));
    assert_eq!(s["config_matches"], true, "{s}");
    assert_eq!(s["enabled"], true);
    assert_eq!(s["enablement"], "enabled");
    assert_eq!(s["interval_seconds"], 120);
    assert_eq!(
        s.get("needs_daemon_reload").is_some(),
        cfg!(target_os = "linux")
    );
}
