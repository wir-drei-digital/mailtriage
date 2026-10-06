//! Agent parity: every tray and window action is exactly the command the
//! spec documents, with the absolute config, so an agent can do the same.
mod support;
use chrono::Utc;
use mailtriage_tray::{
    autostart,
    controller::{self, Controller, Done, Flags, Work},
    instances::Windows,
    model::menu::Action,
    paths::Env,
};
use std::{fs, path::PathBuf, process::Command, sync::Mutex};
use support::{
    window::{click, open_with, rename_news, settle, shows},
    write_script, FakeCli, AUTOSTART_LAUNCHCTL,
};

fn feed(c: &mut Controller, work: Vec<Work>) {
    let mut queue = work;
    while let Some(next) = queue.pop() {
        let answers = Mutex::new(vec![]);
        controller::run(next, &c.context(), &|d: Done| {
            answers.lock().unwrap().push(d)
        });
        for done in answers.into_inner().unwrap() {
            let follow = c.done(done, Utc::now());
            // Follow-up refreshes are recorded once at the start already.
            queue.extend(follow.into_iter().filter(|w| *w != Work::Refresh));
        }
    }
}

/// Recorded calls as text, with the absolute config shown as CONFIG and the
/// draft file as DRAFT.
fn lines(fake: &FakeCli, drafts: &std::path::Path) -> Vec<String> {
    let config = fake.config().display().to_string();
    let drafts = drafts.display().to_string();
    fake.calls()
        .into_iter()
        .map(|call| {
            call.into_iter()
                .map(|a| {
                    if a == config {
                        "CONFIG".to_owned()
                    } else if a.starts_with(&drafts) {
                        "DRAFT".to_owned()
                    } else {
                        a
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

#[test]
fn every_tray_and_window_action_runs_its_documented_command() {
    // Started with a relative --config, and with the config known only to
    // the CLI (MAILTRIAGE_CONFIG): the first status learns it.
    for relative in [true, false] {
        let fake = FakeCli::new();
        fake.respond_fixture("service-status", "status.json");
        fake.respond_fixture("categories-export", "export-daniel.json");
        fake.respond_fixture("categories-validate", "validate-ok.json");
        fake.respond("categories-apply", "{\"schema_version\":1}");
        fake.respond_fixture("filing-refile", "refile-preview.json");
        fake.respond_fixture("filing-refile-apply", "refile-marked.json");
        let root = fake.program.parent().unwrap().to_path_buf();
        let tray_exe = root.join("mailtriage-tray");
        write_script(
            &tray_exe,
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$(dirname \"$0\")/windows.log\"\n",
        );
        let env = Env {
            cwd: root.clone(),
            ..Env::default()
        };
        let flags = Flags {
            mailtriage: Some(fake.program.clone()),
            config: relative.then(|| PathBuf::from("mailtriage.json")),
        };
        fake.config();
        let mut c = Controller::new(
            flags,
            env.clone(),
            tray_exe.clone(),
            None,
            Windows::default(),
        );
        feed(&mut c, vec![Work::Refresh]);
        let now = Utc::now();
        for action in [
            Action::Start("daniel".into()),
            Action::Stop("daniel".into()),
            Action::Install("info".into()),
            Action::EditCategories("daniel".into()),
        ] {
            let work = c.act(action, now);
            feed(&mut c, work);
        }
        // The window, with the paths the tray passes it.
        let resolved = c.paths.clone().unwrap();
        let drafts = tempfile::tempdir().unwrap();
        let drafts_path = fs::canonicalize(drafts.path()).unwrap();
        let mut h = open_with(resolved.cli(), &drafts_path, Some("daniel"));
        rename_news(&mut h);
        click(&mut h, "Apply");
        click(&mut h, "Apply changes");
        settle(&mut h, |h| shows(h, "Saved."));
        click(&mut h, "Move filed mail…");
        settle(&mut h, |h| shows(h, "Promotions is no longer used"));
        click(&mut h, "Move mail from Promotions");
        settle(&mut h, |h| shows(h, "Marked 38 messages"));
        click(&mut h, "Move all 38");
        settle(&mut h, |_| fake.calls_of("filing", "refile").len() >= 5);

        let recorded = lines(&fake, &drafts_path);
        let expected = [
            "service status --json CONFIG_FLAG",
            "service start --account daniel --json CONFIG_FLAG",
            "service stop --account daniel --json CONFIG_FLAG",
            "service install --account info --json CONFIG_FLAG",
            "categories export --account daniel --json CONFIG_FLAG",
            "categories validate --account daniel --file DRAFT --json CONFIG_FLAG",
            "categories apply --account daniel --file DRAFT --expect-digest v1:aaaa --json CONFIG_FLAG",
            "filing refile --account daniel --json CONFIG_FLAG",
            "filing refile --account daniel --folder INBOX.Promotions --apply --json CONFIG_FLAG",
            "filing refile --account daniel --apply --json CONFIG_FLAG",
        ];
        for command in expected {
            let command = command.replace("CONFIG_FLAG", "--config CONFIG");
            assert!(recorded.contains(&command), "{command}\nin {recorded:#?}");
        }
        // Every command names the absolute config; only the first status
        // of a tray without --config may not.
        let without: Vec<&String> = recorded
            .iter()
            .filter(|l| !l.ends_with("--config CONFIG") && *l != "--version")
            .collect();
        let allowed = usize::from(!relative);
        assert_eq!(without.len(), allowed, "{without:?}");
        if !relative {
            assert_eq!(without[0], "service status --json");
        }
        assert_eq!(
            fs::read_to_string(root.join("windows.log")).unwrap(),
            format!(
                "categories --account daniel --config {} --mailtriage {}\n",
                fake.config().display(),
                fake.program.display()
            )
        );
    }
}

/// "Start at login" runs the same code as `mailtriage-tray autostart enable`
/// with the tray's resolved paths: both write the same login item.
#[test]
fn start_at_login_writes_what_the_command_writes() {
    let fake = FakeCli::new();
    fake.respond_fixture("service-status", "status.json");
    let root = fake.program.parent().unwrap().to_path_buf();
    write_script(&root.join("launchctl"), AUTOSTART_LAUNCHCTL);
    let tray = fs::canonicalize(env!("CARGO_BIN_EXE_mailtriage-tray")).unwrap();
    let path_env = format!("{}:/usr/bin:/bin", root.display());
    let platform = if cfg!(target_os = "macos") {
        autostart::Platform::MacOs
    } else {
        autostart::Platform::Linux
    };
    let menu_env = autostart::Env {
        platform,
        home: root.join("home-menu"),
        xdg_config_home: Some(root.join("xdg-menu").into_os_string()),
        launchctl: root.join("launchctl"),
        uid: unsafe { libc::getuid() },
        path_env: Some(path_env.clone()),
        tray: tray.clone(),
    };
    let mut c = Controller::new(
        Flags {
            mailtriage: Some(fake.program.clone()),
            config: Some(fake.config()),
        },
        Env {
            cwd: root.clone(),
            ..Env::default()
        },
        tray.clone(),
        Some(menu_env.clone()),
        Windows::default(),
    );
    feed(&mut c, vec![Work::Refresh]);
    let work = c.act(Action::ToggleAutostart, Utc::now());
    feed(&mut c, work);
    assert!(c.autostart);
    let out = Command::new(&tray)
        .args(["autostart", "enable", "--json", "--config"])
        .arg(fake.config())
        .arg("--mailtriage")
        .arg(&fake.program)
        .env("HOME", root.join("home-cli"))
        .env("XDG_CONFIG_HOME", root.join("xdg-cli"))
        .env("PATH", &path_env)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let cli_env = autostart::Env {
        home: root.join("home-cli"),
        xdg_config_home: Some(root.join("xdg-cli").into_os_string()),
        ..menu_env.clone()
    };
    assert_eq!(
        fs::read_to_string(autostart::path(&menu_env)).unwrap(),
        fs::read_to_string(autostart::path(&cli_env)).unwrap()
    );
}
