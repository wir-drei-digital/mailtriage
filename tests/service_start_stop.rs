#![cfg(unix)]
//! `service start` and `stop`: call sequences with fake `launchctl` and
//! `systemctl`, the config binding, and one service command at a time.
mod common;
use common::{config_file, unit_for, write_proc, write_tool, LAUNCHCTL, SYSTEMCTL};
use mailtriage::{
    config,
    service::{Service, ServiceError},
    service_control,
    system_service::{self, Context, Manager},
};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

const TARGET: &str = "gui/501/digital.wirdrei.mailtriage.work";
const UNIT: &str = "mailtriage-work.service";
const SHORT: Duration = Duration::from_millis(300);

struct Fixture {
    dir: tempfile::TempDir,
    ctx: Context,
    config: PathBuf,
    proc_root: PathBuf,
}

impl Fixture {
    /// A config with the `work` account and its service installed for it.
    fn installed(manager: Manager) -> Self {
        let dir = tempfile::tempdir().unwrap();
        write_tool(dir.path(), "launchctl", LAUNCHCTL);
        write_tool(dir.path(), "systemctl", SYSTEMCTL);
        let ctx = Context {
            manager,
            home: dir.path().join("home"),
            tool: dir.path().join(match manager {
                Manager::Launchd => "launchctl",
                Manager::Systemd => "systemctl",
            }),
            uid: 501,
        };
        let config = config_file(dir.path(), "a.json");
        let unit = unit_for(dir.path(), &config);
        system_service::install(&ctx, &unit).unwrap();
        let proc_root = dir.path().join("proc");
        write_proc(&proc_root, 4343, &unit);
        let fixture = Self {
            dir,
            ctx,
            config,
            proc_root,
        };
        fixture.clear_log();
        fixture
    }

    fn file(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn touch(&self, name: &str) {
        fs::write(self.file(name), "").unwrap();
    }

    fn clear_log(&self) {
        let _ = fs::remove_file(self.file("launchctl.log"));
        let _ = fs::remove_file(self.file("systemctl.log"));
    }

    fn calls(&self) -> Vec<String> {
        let tool = self.ctx.tool.file_name().unwrap().to_string_lossy();
        fs::read_to_string(self.file(&format!("{tool}.log")))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn service(&self, config: &Path) -> Service {
        Service::open(config).unwrap()
    }

    fn start_with(&self, config: &Path) -> anyhow::Result<Value> {
        service_control::start(
            &self.ctx,
            &self.service(config),
            config,
            "work",
            &self.proc_root,
            SHORT,
        )
    }

    fn start(&self) -> anyhow::Result<Value> {
        self.start_with(&self.config)
    }

    fn stop_with(&self, config: &Path) -> anyhow::Result<Value> {
        service_control::stop(
            &self.ctx,
            &self.service(config),
            config,
            "work",
            &self.proc_root,
        )
    }

    fn stop(&self) -> anyhow::Result<Value> {
        self.stop_with(&self.config)
    }
}

fn action(result: anyhow::Result<Value>) -> String {
    result.unwrap()["action"].as_str().unwrap().to_owned()
}

fn failure(result: anyhow::Result<Value>) -> (i32, Option<&'static str>, String) {
    let error = result.unwrap_err();
    let e = error.downcast_ref::<ServiceError>().unwrap();
    (e.code, e.kind.reason(), e.message.clone())
}

fn inspect_calls(manager: Manager) -> Vec<String> {
    match manager {
        Manager::Launchd => vec![format!("print {TARGET}"), "print-disabled gui/501".into()],
        Manager::Systemd => vec![
            format!("--user show {UNIT} --property=LoadState,ActiveState,SubState,MainPID,ExecStart,NeedDaemonReload"),
            format!("--user is-enabled {UNIT}"),
        ],
    }
}

fn with(mut calls: Vec<String>, more: &[&str]) -> Vec<String> {
    calls.extend(more.iter().map(|c| c.to_string()));
    calls
}

#[test]
fn launchd_start_for_each_job_state() {
    let f = Fixture::installed(Manager::Launchd);
    let plist = f.ctx.unit_path("work").display().to_string();
    // Loaded and running: enable repairs a disabled label, nothing else.
    assert_eq!(action(f.start()), "already_running");
    assert_eq!(
        f.calls(),
        with(
            inspect_calls(Manager::Launchd),
            &[&format!("enable {TARGET}")]
        )
    );
    // Loaded but idle: kickstart, then wait for a PID.
    f.touch("idle");
    f.clear_log();
    assert_eq!(action(f.start()), "started");
    assert_eq!(
        f.calls(),
        with(
            inspect_calls(Manager::Launchd),
            &[
                &format!("enable {TARGET}"),
                &format!("kickstart {TARGET}"),
                &format!("print {TARGET}")
            ]
        )
    );
    // Not loaded: bootstrap the plist.
    f.touch("idle");
    fs::remove_file(f.file("loaded")).unwrap();
    f.clear_log();
    assert_eq!(action(f.start()), "started");
    assert_eq!(
        f.calls(),
        with(
            inspect_calls(Manager::Launchd),
            &[
                &format!("enable {TARGET}"),
                &format!("bootstrap gui/501 {plist}"),
                &format!("print {TARGET}")
            ]
        )
    );
}

#[test]
fn launchd_stop_disables_and_install_enables_again() {
    let f = Fixture::installed(Manager::Launchd);
    assert_eq!(action(f.stop()), "stopped");
    assert_eq!(
        f.calls(),
        with(
            inspect_calls(Manager::Launchd),
            &[
                &format!("print {TARGET}"),
                &format!("bootout {TARGET}"),
                &format!("print {TARGET}"),
                &format!("disable {TARGET}")
            ]
        )
    );
    let found = service_control::inspect(&f.ctx, "work", &f.proc_root);
    assert_eq!((found.enabled, found.running), (Some(false), false));
    assert_eq!(action(f.stop()), "already_stopped");
    // `install` after `stop` enables the label before bootstrapping.
    f.clear_log();
    system_service::install(&f.ctx, &unit_for(f.dir.path(), &f.config)).unwrap();
    let calls = f.calls();
    let enable = calls.iter().position(|c| *c == format!("enable {TARGET}"));
    let bootstrap = calls.iter().position(|c| c.starts_with("bootstrap "));
    assert!(enable.is_some() && enable < bootstrap, "{calls:?}");
    let found = service_control::inspect(&f.ctx, "work", &f.proc_root);
    assert_eq!((found.enabled, found.running), (Some(true), true));
}

#[test]
fn systemd_start_and_stop() {
    let f = Fixture::installed(Manager::Systemd);
    assert_eq!(action(f.start()), "already_running");
    assert_eq!(
        f.calls(),
        with(
            inspect_calls(Manager::Systemd),
            &[&format!("--user enable --now {UNIT}")]
        )
    );
    f.clear_log();
    assert_eq!(action(f.stop()), "stopped");
    assert_eq!(
        f.calls(),
        with(
            inspect_calls(Manager::Systemd),
            &[&format!("--user disable --now {UNIT}")]
        )
    );
    assert_eq!(action(f.stop()), "already_stopped");
    f.clear_log();
    assert_eq!(action(f.start()), "started");
    assert_eq!(
        f.calls(),
        with(
            inspect_calls(Manager::Systemd),
            &[
                &format!("--user enable --now {UNIT}"),
                &format!("--user is-active {UNIT}")
            ]
        )
    );
}

#[test]
fn a_start_that_does_not_run_is_exit_3() {
    for manager in [Manager::Launchd, Manager::Systemd] {
        let f = Fixture::installed(manager);
        f.stop().unwrap();
        f.touch("start-fails");
        let (code, _, message) = failure(f.start());
        assert_eq!(code, 3, "{manager:?}");
        let log = match manager {
            Manager::Launchd => format!(
                "{}",
                f.config.parent().unwrap().join("logs/work.err").display()
            ),
            Manager::Systemd => format!("journalctl --user -u {UNIT}"),
        };
        assert_eq!(message, format!("service did not start; see {log}"));
    }
}

#[test]
fn not_installed_is_exit_2() {
    for manager in [Manager::Launchd, Manager::Systemd] {
        let f = Fixture::installed(manager);
        system_service::uninstall(&f.ctx, "work").unwrap();
        for result in [f.start(), f.stop()] {
            assert_eq!(
                failure(result),
                (
                    2,
                    None,
                    "service for account work is not installed; run mailtriage service install --account work".into()
                )
            );
        }
    }
}

#[test]
fn another_or_unknown_config_is_refused_without_manager_calls() {
    for manager in [Manager::Launchd, Manager::Systemd] {
        let f = Fixture::installed(manager);
        let other = config_file(f.dir.path(), "b c.json");
        let mismatch = format!(
            "service for account work runs config {}; pass --config {}",
            f.config.display(),
            f.config.display()
        );
        let (code, reason, message) = failure(f.start_with(&other));
        assert_eq!((code, reason), (5, Some("service_config_mismatch")));
        assert_eq!(message, mismatch);
        assert_eq!(f.calls(), inspect_calls(manager));
        f.clear_log();
        let (code, reason, message) = failure(f.stop_with(&other));
        assert_eq!((code, reason), (5, Some("service_config_mismatch")));
        assert_eq!(message, mismatch);
        assert_eq!(f.calls(), inspect_calls(manager));
        f.touch(match manager {
            Manager::Launchd => "print-fails",
            Manager::Systemd => "show-fails",
        });
        f.clear_log();
        let (code, reason, _) = failure(f.start());
        assert_eq!((code, reason), (5, Some("service_config_unknown")));
        let (code, reason, _) = failure(f.stop());
        assert_eq!((code, reason), (5, Some("service_config_unknown")));
        // Only the inspections of `start` and `stop` ran.
        assert_eq!(
            f.calls(),
            [inspect_calls(manager), inspect_calls(manager)].concat()
        );
    }
}

/// The unit file was rewritten for config B while the loaded job still runs
/// A: `stop` refuses with either config and stops nothing.
#[test]
fn stop_refuses_a_running_job_whose_unit_names_another_config() {
    for manager in [Manager::Launchd, Manager::Systemd] {
        let f = Fixture::installed(manager);
        let b = config_file(f.dir.path(), "b.json");
        let unit_b = unit_for(f.dir.path(), &b);
        let text = match manager {
            Manager::Launchd => system_service::plist(&unit_b),
            Manager::Systemd => system_service::systemd_unit(&unit_b),
        };
        fs::write(f.ctx.unit_path("work"), text).unwrap();
        if manager == Manager::Systemd {
            f.touch("needs-reload");
        }
        let found = service_control::inspect(&f.ctx, "work", &f.proc_root);
        assert!(found.running, "{manager:?}");
        assert_eq!(
            found.service_config.as_ref(),
            Some(&f.config),
            "{manager:?}"
        );
        assert_eq!(found.file_config.as_ref(), Some(&b), "{manager:?}");
        f.clear_log();
        for (config, runs) in [(&f.config, &b), (&b, &f.config)] {
            let (code, reason, message) = failure(f.stop_with(config));
            assert_eq!(
                (code, reason),
                (5, Some("service_config_mismatch")),
                "{manager:?}"
            );
            assert_eq!(
                message,
                format!(
                    "service for account work runs config {}; pass --config {}",
                    runs.display(),
                    runs.display()
                )
            );
        }
        // Only the two inspections ran: no bootout, disable or disable --now.
        assert_eq!(
            f.calls(),
            [inspect_calls(manager), inspect_calls(manager)].concat(),
            "{manager:?}"
        );
        assert!(service_control::inspect(&f.ctx, "work", &f.proc_root).running);
    }
}

#[test]
fn a_held_service_lock_is_service_busy() {
    let f = Fixture::installed(Manager::Launchd);
    let _held = service_control::lock(&f.ctx, "work", SHORT).unwrap();
    let started = Instant::now();
    let error = service_control::lock(&f.ctx, "work", SHORT).err().unwrap();
    assert!(started.elapsed() >= SHORT);
    let e = error.downcast_ref::<ServiceError>().unwrap();
    assert_eq!((e.code, e.kind.reason()), (5, Some("service_busy")));
    // Other accounts have their own lock.
    service_control::lock(&f.ctx, "home", SHORT).unwrap();
    assert!(f
        .ctx
        .home
        .join("Library/Caches/mailtriage/service-digital.wirdrei.mailtriage.work.lock")
        .exists());
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn a_second_service_command_waits_for_the_first() {
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
    let mut c = config::default_config();
    c.state_dir = PathBuf::from(".state");
    config::save(&cwd.join("mailtriage.json"), &c).unwrap();
    let ctx = Context {
        manager: if cfg!(target_os = "macos") {
            Manager::Launchd
        } else {
            Manager::Systemd
        },
        home: home.clone(),
        tool: bin.join("launchctl"),
        uid: 501,
    };
    let held = service_control::lock(&ctx, "work", SHORT).unwrap();
    let release = thread::spawn(move || {
        thread::sleep(Duration::from_millis(800));
        drop(held);
    });
    let started = Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
        .current_dir(&cwd)
        .args(["service", "start", "--account", "work", "--json"])
        .env("HOME", &home)
        .env("MT_FAKE_HOME", &home)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env_remove("MAILTRIAGE_CONFIG")
        .output()
        .unwrap();
    release.join().unwrap();
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    // It waited for the lock, then found the service not installed.
    assert!(started.elapsed() >= Duration::from_millis(700));
    assert_eq!(out.status.code(), Some(2), "{value}");
    assert_eq!(value["error"]["code"], 2);
}
