//! The tray process without a menu bar: the controller against a fake
//! `mailtriage`, one tray and one window per config, reaping only the
//! windows the tray started, and the restart rule.
mod support;
use chrono::{DateTime, Duration, FixedOffset, Utc};
use mailtriage_tray::{
    controller::{self, Controller, Done, Flags, Work},
    instances::{self, EditorLock, Windows},
    model::menu::{Action, Entry, Menu},
    paths::{Env, Resolved},
    restart::{self, Restarter},
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::Mutex,
    time::Instant,
};
use support::{write_script, FakeCli};

fn now() -> DateTime<Utc> {
    Utc::now()
}

fn local() -> DateTime<FixedOffset> {
    now().fixed_offset()
}

/// Runs `work` and collects its answers.
fn run(c: &Controller, work: Work) -> Vec<Done> {
    let answers = Mutex::new(vec![]);
    controller::run(work, &c.context(), &|done| {
        answers.lock().unwrap().push(done)
    });
    answers.into_inner().unwrap()
}

/// Runs `work` and feeds every answer back, as the event loop does.
fn settle(c: &mut Controller, work: Vec<Work>, at: DateTime<Utc>) {
    let mut queue = work;
    while let Some(next) = queue.pop() {
        for done in run(c, next) {
            queue.extend(c.done(done, at));
        }
    }
}

fn texts(menu: &Menu) -> Vec<String> {
    menu.entries
        .iter()
        .filter_map(|e| match e {
            Entry::Text(t) => Some(t.clone()),
            Entry::Item { label, .. } => Some(label.clone()),
            _ => None,
        })
        .collect()
}

fn submenu(menu: &Menu, account: &str) -> Vec<String> {
    menu.entries
        .iter()
        .find_map(|e| match e {
            Entry::Submenu { label, entries } if label.starts_with(account) => Some(
                entries
                    .iter()
                    .filter_map(|e| match e {
                        Entry::Text(t) | Entry::Item { label: t, .. } => Some(t.clone()),
                        _ => None,
                    })
                    .collect(),
            ),
            _ => None,
        })
        .unwrap()
}

/// A controller started as `mailtriage-tray --config mailtriage.json
/// --mailtriage <fake>` in the fake's directory.
fn controller(fake: &FakeCli) -> Controller {
    fake.config();
    Controller::new(
        Flags {
            mailtriage: Some(fake.program.clone()),
            config: Some(PathBuf::from("mailtriage.json")),
        },
        Env {
            cwd: fake.dir.path().to_path_buf(),
            ..Env::default()
        },
        fake.dir.path().join("mailtriage-tray"),
        None,
        Windows::default(),
    )
}

#[test]
fn a_refresh_resolves_the_paths_once_and_shows_the_accounts() {
    let fake = FakeCli::new();
    fake.respond_fixture("service-status", "status.json");
    let mut c = controller(&fake);
    settle(&mut c, vec![Work::Refresh], now());
    assert_eq!(
        c.paths.as_ref().unwrap(),
        &Resolved {
            cli: fake.program.clone(),
            config: fake.config()
        }
    );
    let menu = c.menu(now(), local());
    assert_eq!(menu.summary, "mailtriage: daniel needs attention");
    let config = fake.config().display().to_string();
    assert_eq!(
        fake.calls(),
        [
            vec!["--version".to_owned()],
            vec![
                "service".into(),
                "status".into(),
                "--json".into(),
                "--config".into(),
                config.clone()
            ]
        ]
    );
    // The CLI file did not change: no second `--version`.
    settle(&mut c, vec![Work::Refresh], now());
    assert_eq!(
        fake.calls().iter().filter(|c| c[0] == "--version").count(),
        1
    );
}

#[test]
fn a_service_action_is_busy_until_it_ends_and_refreshes_afterwards() {
    let fake = FakeCli::new();
    fake.respond_fixture("service-status", "status.json");
    let mut c = controller(&fake);
    settle(&mut c, vec![Work::Refresh], now());
    let work = c.act(Action::Stop("daniel".into()), now());
    assert_eq!(
        work,
        [Work::Service {
            verb: mailtriage_tray::model::menu::Verb::Stop,
            account: "daniel".into()
        }]
    );
    assert!(submenu(&c.menu(now(), local()), "daniel").contains(&"Stopping…".to_owned()));
    assert!(
        c.act(Action::Stop("daniel".into()), now()).is_empty(),
        "one at a time"
    );
    fake.fail(
        "service-stop",
        5,
        "service for account daniel runs config /x.json; pass --config /x.json",
        Some("service_config_mismatch"),
    );
    let answers = run(&c, work[0].clone());
    let follow = c.done(answers.into_iter().next().unwrap(), now());
    assert_eq!(follow, [Work::Refresh]);
    let menu = c.menu(now(), local());
    let lines = texts(&menu);
    assert!(lines.contains(
        &"Could not stop daniel: service for account daniel runs config /x.json; pass --config /x.json"
            .to_owned()
    ));
    assert!(lines.contains(&"Show details".to_owned()));
    assert!(submenu(&menu, "daniel").contains(&"Stop service".to_owned()));
    // The notice goes after a minute.
    let later = now() + Duration::seconds(61);
    assert!(!texts(&c.menu(later, local()))
        .iter()
        .any(|l| l.starts_with("Could not")));
}

/// The action of the item labelled `label`, in `account`'s submenu or, for
/// `None`, at the top of the menu.
fn action_of(menu: &Menu, account: Option<&str>, label: &str) -> Action {
    let entries = match account {
        None => &menu.entries,
        Some(account) => menu
            .entries
            .iter()
            .find_map(|e| match e {
                Entry::Submenu { label, entries } if label.starts_with(account) => Some(entries),
                _ => None,
            })
            .unwrap(),
    };
    entries
        .iter()
        .find_map(|e| match e {
            Entry::Item { label: l, action }
            | Entry::Check {
                label: l, action, ..
            } if l == label => Some(action.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no item {label:?}"))
}

/// Every clickable action of `entries`, submenus included.
fn actions(entries: &[Entry]) -> Vec<Action> {
    entries
        .iter()
        .flat_map(|e| match e {
            Entry::Item { action, .. } | Entry::Check { action, .. } => vec![action.clone()],
            Entry::Submenu { entries, .. } => actions(entries),
            _ => vec![],
        })
        .collect()
}

#[test]
fn a_click_on_a_redrawn_menu_does_what_its_label_said_or_nothing() {
    let fake = FakeCli::new();
    fake.respond_fixture("service-status", "status.json");
    let mut c = controller(&fake);
    settle(&mut c, vec![Work::Refresh], now());
    fake.fail("service-stop", 5, "busy", Some("service_busy"));
    let work = c.act(Action::Stop("daniel".into()), now());
    settle(&mut c, work, now());
    // The menu as shown, with a notice and its "Show details" on top.
    let shown = c.menu(now(), local());
    let edit = action_of(&shown, Some("daniel"), "Edit categories…").id();
    let refresh = action_of(&shown, None, "Refresh now").id();
    let autostart = action_of(&shown, None, "Start at login").id();
    let details = action_of(&shown, None, "Show details").id();
    // The notice expires while the menu is open: every item moves up.
    let redrawn = c.menu(now() + Duration::seconds(61), local());
    assert!(!texts(&redrawn).contains(&"Show details".to_owned()));
    assert_eq!(
        redrawn.action(&edit),
        Some(Action::EditCategories("daniel".into()))
    );
    assert_eq!(redrawn.action(&refresh), Some(Action::Refresh));
    assert_eq!(redrawn.action(&autostart), Some(Action::ToggleAutostart));
    // An item that is gone, or an id no item has, does nothing.
    assert_eq!(redrawn.action(&details), None);
    assert_eq!(redrawn.action("a3"), None);
    // Equal actions have equal ids, different actions different ones.
    let all: Vec<Action> = [actions(&shown.entries), actions(&redrawn.entries)].concat();
    for a in &all {
        for b in &all {
            assert_eq!(a.id() == b.id(), a == b, "{a:?} and {b:?}");
        }
    }
}

#[test]
fn a_failed_refresh_keeps_the_menu_for_two_minutes_marked_stale() {
    let fake = FakeCli::new();
    fake.respond_fixture("service-status", "status.json");
    let mut c = controller(&fake);
    let start = now();
    settle(&mut c, vec![Work::Refresh], start);
    fake.respond("service-status", "garbage");
    settle(&mut c, vec![Work::Refresh], start + Duration::seconds(15));
    assert_eq!(
        c.menu(start + Duration::seconds(130), local()).summary,
        "mailtriage: daniel needs attention (stale)"
    );
    let failed = c.menu(start + Duration::seconds(136), local());
    assert_eq!(
        failed.summary,
        "mailtriage: mailtriage gave an answer the tray cannot read"
    );
    assert!(texts(&failed).contains(&"Show details".to_owned()));
}

#[test]
fn missing_cli_and_missing_config_are_explained() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    let mut c = Controller::new(
        Flags::default(),
        Env {
            cwd: root.clone(),
            own_exe: Some(root.join("mailtriage-tray")),
            path: Some("".into()),
            ..Env::default()
        },
        root.join("mailtriage-tray"),
        None,
        Windows::default(),
    );
    settle(&mut c, vec![Work::Refresh], now());
    assert_eq!(
        c.menu(now(), local()).summary,
        format!(
            "mailtriage: mailtriage not found (looked in {}, PATH)",
            root.join("mailtriage").display()
        )
    );
    let fake = FakeCli::new();
    fake.fail(
        "service-status",
        2,
        "configuration not found; run `mailtriage setup` or pass --config",
        None,
    );
    let mut c = Controller::new(
        Flags {
            mailtriage: Some(fake.program.clone()),
            config: None,
        },
        Env::default(),
        PathBuf::from("mailtriage-tray"),
        None,
        Windows::default(),
    );
    settle(&mut c, vec![Work::Refresh], now());
    assert_eq!(
        c.menu(now(), local()).summary,
        "mailtriage: Not set up. Run `mailtriage setup` in a terminal."
    );
    // Once set up, the next refresh resolves the config and shows it.
    fake.respond_fixture("service-status", "status.json");
    settle(&mut c, vec![Work::Refresh], now());
    assert!(c.paths.is_ok());
}

#[test]
fn open_log_only_opens_a_listed_log() {
    let fake = FakeCli::new();
    fake.respond_fixture("service-status", "status.json");
    let mut c = controller(&fake);
    settle(&mut c, vec![Work::Refresh], now());
    let listed = PathBuf::from("/Users/daniel/.config/mailtriage/logs/daniel.log");
    assert_eq!(
        c.act(Action::OpenLog(listed.clone()), now()),
        [Work::Open(listed)]
    );
    assert!(c
        .act(Action::OpenLog("/etc/passwd".into()), now())
        .is_empty());
}

#[test]
fn a_window_is_started_with_both_paths_and_its_notice_is_shown() {
    let fake = FakeCli::new();
    fake.respond_fixture("service-status", "status.json");
    let tray = fake.dir.path().join("mailtriage-tray");
    write_script(
        &tray,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" > \"$(dirname \"$0\")/window-args\"\necho 'the categories window is already open'\n",
    );
    let mut c = controller(&fake);
    settle(&mut c, vec![Work::Refresh], now());
    let work = c.act(Action::EditCategories("daniel".into()), now());
    assert_eq!(work, [Work::Window("daniel".into())]);
    settle(&mut c, work, now());
    assert_eq!(
        fs::read_to_string(fake.dir.path().join("window-args")).unwrap(),
        format!(
            "categories --account daniel --config {} --mailtriage {}\n",
            fake.config().display(),
            fake.program.display()
        )
    );
    assert_eq!(c.windows.pids().len(), 1);
    assert!(texts(&c.menu(now(), local()))
        .contains(&"the categories window is already open".to_owned()));
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(c.windows.reap().len(), 1);
    assert!(c.windows.pids().is_empty());
}

fn cache_of(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Caches/mailtriage")
    } else {
        home.join("xdg-cache/mailtriage")
    }
}

/// Runs `command` and waits at most `limit`. A tray that did not stop at its
/// lock would run until killed: it is killed at the deadline and the test
/// fails, so `cargo test` never hangs on a menu-bar tray.
fn output_within(command: &mut Command, limit: std::time::Duration) -> Output {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + limit;
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{command:?} did not exit within {limit:?}");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    child.wait_with_output().unwrap()
}

#[test]
fn a_second_tray_exits_0() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let _held = instances::tray_lock(&cache_of(home)).unwrap().unwrap();
    let out = output_within(
        Command::new(env!("CARGO_BIN_EXE_mailtriage-tray"))
            // Should the lock check ever break, the tray that starts runs
            // no real `mailtriage`.
            .args(["--mailtriage", "/usr/bin/false"])
            .env("HOME", home)
            .env("XDG_CACHE_HOME", home.join("xdg-cache")),
        std::time::Duration::from_secs(10),
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "mailtriage-tray is already running\n"
    );
}

#[test]
fn one_window_lock_per_config_held_by_the_window_itself() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path();
    let (a, b) = (Path::new("/home/x/a.json"), Path::new("/home/x/b.json"));
    assert_eq!(instances::editor_name(a).len(), "editor-".len() + 16);
    let EditorLock::Taken(_held) = instances::editor_lock(cache, a).unwrap() else {
        panic!("first window")
    };
    let EditorLock::HeldBy(pid) = instances::editor_lock(cache, a).unwrap() else {
        panic!("second window for the same config")
    };
    assert_eq!(pid, Some(std::process::id()));
    assert!(matches!(
        instances::editor_lock(cache, b).unwrap(),
        EditorLock::Taken(_)
    ));
    // A tray restart only rebuilds its list of windows; the lock stays.
    // Rebuilding the tray's window list from MAILTRIAGE_TRAY_WINDOWS does
    // not touch the lock: the lock lives in the window process (here, the
    // `_held` file above), so the config's window lock is still held.
    let _restarted = Windows::from_env(Some("1,2"));
    assert!(matches!(
        instances::editor_lock(cache, a).unwrap(),
        EditorLock::HeldBy(_)
    ));
}

/// The windows are reaped by `Windows::reap`, which is what this tests.
#[test]
// The window child is never waited for through its `Child`: `Windows::reap`
// reaps it with `waitpid`, which is what this test checks.
#[allow(clippy::zombie_processes)]
fn only_tracked_windows_are_reaped() {
    let mut command_of_a_worker = Command::new("/bin/sh")
        .args(["-c", "exit 7"])
        .spawn()
        .unwrap();
    let window = Command::new("/bin/sh")
        .args(["-c", "exit 0"])
        .spawn()
        .unwrap();
    let mut windows = Windows::default();
    windows.track(window.id());
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(windows.reap(), [window.id()]);
    // The worker still gets its own command's exit status.
    assert_eq!(command_of_a_worker.wait().unwrap().code(), Some(7));
}

#[test]
// Both children are reaped by the rebuilt `Windows::reap` (asserted at the
// end), not through their `Child` handles: that hand-over is under test.
#[allow(clippy::zombie_processes)]
fn window_pids_survive_a_restart() {
    let a = Command::new("/bin/sh")
        .args(["-c", "sleep 0.1"])
        .spawn()
        .unwrap();
    let b = Command::new("/bin/sh")
        .args(["-c", "sleep 0.1"])
        .spawn()
        .unwrap();
    let mut before = Windows::default();
    before.track(a.id());
    before.track(b.id());
    let value = before.env_value();
    assert_eq!(value, format!("{},{}", a.id(), b.id()));
    let mut after = Windows::from_env(Some(&value));
    assert_eq!(after.pids(), before.pids());
    std::thread::sleep(std::time::Duration::from_millis(400));
    assert_eq!(after.reap().len(), 2);
}

fn replace(path: &Path, script: &str) {
    let next = path.with_extension("new");
    write_script(&next, script);
    fs::rename(&next, path).unwrap();
}

#[test]
fn a_replaced_tray_that_runs_restarts_and_one_that_fails_waits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mailtriage-tray");
    write_script(&path, "#!/bin/sh\necho 'mailtriage-tray 0.1.0'\n");
    let mut r = Restarter::new(path.clone(), restart::identity(&path).unwrap());
    let t = Instant::now();
    assert_eq!(r.check(t), None);
    let log = dir.path().join("probes");
    replace(
        &path,
        &format!(
            "#!/bin/sh\necho x >> '{}'\necho 'not a version'\n",
            log.display()
        ),
    );
    assert_eq!(r.check(t), None);
    assert_eq!(r.check(t + std::time::Duration::from_secs(30)), None);
    assert_eq!(
        fs::read_to_string(&log).unwrap().lines().count(),
        1,
        "backs off"
    );
    // A new file is probed at once.
    replace(&path, "#!/bin/sh\necho 'mailtriage-tray 9.9.9'\n");
    assert_eq!(r.check(t + std::time::Duration::from_secs(31)), Some(path));
}

#[test]
fn a_restart_whose_exec_failed_keeps_the_old_code_and_waits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mailtriage-tray");
    write_script(&path, "#!/bin/sh\necho 'mailtriage-tray 0.1.0'\n");
    let mut r = Restarter::new(path.clone(), restart::identity(&path).unwrap());
    let log = dir.path().join("probes");
    replace(
        &path,
        &format!(
            "#!/bin/sh\necho x >> '{}'\necho 'mailtriage-tray 9.9.9'\n",
            log.display()
        ),
    );
    let probes = || fs::read_to_string(&log).unwrap().lines().count();
    let t = Instant::now();
    let after = |secs| t + std::time::Duration::from_secs(secs);
    assert_eq!(r.check(t), Some(path.clone()));
    // `exec` failed: no new probe and no `exec` for a minute…
    r.exec_failed(t);
    assert_eq!(r.check(after(59)), None);
    assert_eq!(probes(), 1);
    assert_eq!(r.check(after(60)), Some(path.clone()));
    assert_eq!(probes(), 2);
    // …then two minutes after the second failure.
    r.exec_failed(after(60));
    assert_eq!(r.check(after(60 + 119)), None);
    assert_eq!(probes(), 2);
    assert_eq!(r.check(after(60 + 120)), Some(path.clone()));
    // A new file is probed at once.
    r.exec_failed(after(180));
    replace(&path, "#!/bin/sh\necho 'mailtriage-tray 9.9.10'\n");
    assert_eq!(r.check(after(181)), Some(path));
}

#[test]
fn the_restart_uses_the_resolved_paths_and_passes_the_windows() {
    let dir = tempfile::tempdir().unwrap();
    // A config that appears in the working directory later changes nothing.
    fs::write(dir.path().join("mailtriage.json"), "{}").unwrap();
    let resolved = Resolved {
        cli: "/opt/mailtriage".into(),
        config: "/home/a/.config/mailtriage/mailtriage.json".into(),
    };
    let windows = Windows::from_env(Some("12,34"));
    let command = restart::command(Path::new("/opt/mailtriage-tray"), &resolved, &windows);
    let args: Vec<_> = command
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        [
            "--config",
            "/home/a/.config/mailtriage/mailtriage.json",
            "--mailtriage",
            "/opt/mailtriage"
        ]
    );
    let windows_env = command
        .get_envs()
        .find(|(k, _)| *k == instances::WINDOWS_ENV)
        .and_then(|(_, v)| v)
        .unwrap();
    assert_eq!(windows_env, "12,34");
    assert!(restart::semver_like("0.3.0") && restart::semver_like("1.2.3-rc.1"));
    assert!(!restart::semver_like("1.2") && !restart::semver_like("v1.2.3"));
}
