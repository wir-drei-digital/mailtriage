//! Screenshots for the design review: every window screen in light and dark
//! mode, rendered offscreen with wgpu, and every tray menu as text. Run on
//! macOS with `cargo test -p mailtriage-tray --test screens -- --ignored`;
//! the files land in `target/tray-screens/`.
mod support;
use chrono::{DateTime, Duration, Utc};
use eframe::egui::{self, Theme};
use egui_kittest::{kittest::Queryable, Harness};
use mailtriage_tray::{
    cli::{self, Cli},
    editor::{Editor, Setup},
    model::{
        health::Observations,
        menu::{self, Entry, Input, Notice, Verb},
    },
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{atomic::AtomicI32, Arc},
    time::Duration as StdDuration,
};
use support::{
    window::{click, fast, settle, shows, type_into},
    FakeCli,
};

fn out_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/tray-screens")
}

fn harness<'a>(
    fake: &FakeCli,
    drafts: &Path,
    theme: Theme,
    account: Option<&str>,
) -> Harness<'a, Editor> {
    let setup = Setup {
        cli: Cli {
            program: fake.program.clone(),
            config: Some(fake.config()),
        },
        account: account.map(str::to_owned),
        options: fast(),
        drafts: drafts.to_path_buf(),
        lock: None,
        temp: None,
    };
    Harness::builder()
        .with_size(egui::vec2(720.0, 520.0))
        .with_theme(theme)
        .wgpu()
        .build_eframe(|cc| Editor::new(cc, Ok(setup), Arc::new(AtomicI32::new(0))))
}

fn shot(h: &mut Harness<Editor>, theme: Theme, name: &str) {
    for _ in 0..6 {
        h.step();
        std::thread::sleep(StdDuration::from_millis(10));
    }
    let dir = out_dir().join(match theme {
        Theme::Light => "light",
        Theme::Dark => "dark",
    });
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join(format!("{name}.png"));
    h.render().unwrap().save(&file).unwrap();
    assert!(fs::metadata(&file).unwrap().len() > 0, "{}", file.display());
}

fn answering() -> FakeCli {
    let fake = FakeCli::new();
    fake.respond_fixture("service-status", "status.json");
    fake.respond_fixture("categories-export", "export-daniel.json");
    fake.respond_fixture("categories-validate", "validate-ok.json");
    fake.respond("categories-apply", "{\"schema_version\":1}");
    fake.respond_fixture("filing-refile", "refile-preview.json");
    fake.respond_fixture("filing-refile-apply", "refile-marked-folder.json");
    fake
}

fn loaded<'a>(
    fake: &FakeCli,
    drafts: &Path,
    theme: Theme,
    account: Option<&str>,
) -> Harness<'a, Editor> {
    let mut h = harness(fake, drafts, theme, account);
    settle(&mut h, |h| h.state().model().loaded.is_some());
    h
}

fn window_screens(theme: Theme) {
    let drafts = tempfile::tempdir().unwrap();
    let d = drafts.path();

    let fake = answering();
    fake.hold("service-status");
    let mut h = harness(&fake, d, theme, None);
    shot(&mut h, theme, "01-loading");
    fake.release("service-status");

    let fake = answering();
    let mut h = loaded(&fake, d, theme, None);
    shot(&mut h, theme, "02-loaded");
    click(&mut h, "News\nNews");
    type_into(&mut h, "Name", "Newsletters");
    settle(&mut h, |h| shows(h, "1 renamed"));
    shot(&mut h, theme, "03-change-summary");
    click(&mut h, "Apply");
    shot(&mut h, theme, "04-confirm-apply");
    click(&mut h, "Cancel");
    fake.fail(
        "categories-validate",
        2,
        "invalid categories; require unique IDs, descriptions and exactly one catch-all",
        None,
    );
    type_into(&mut h, "What belongs here", "");
    settle(&mut h, |h| shows(h, "Every category needs"));
    shot(&mut h, theme, "05-validation-error");
    click(&mut h, "Show details");
    shot(&mut h, theme, "06-details");

    let fake = answering();
    fake.respond_fixture("categories-validate", "validate-reclassify.json");
    let mut h = loaded(&fake, d, theme, None);
    click(&mut h, "Add");
    type_into(&mut h, "Name", "Travel");
    type_into(&mut h, "What belongs here", "Trips, bookings and tickets");
    click(&mut h, "Advanced");
    settle(&mut h, |h| shows(h, "1 added"));
    shot(&mut h, theme, "07-new-category-advanced");
    click(&mut h, "Apply");
    shot(&mut h, theme, "08-confirm-reclassify");

    let fake = answering();
    let mut h = loaded(&fake, d, theme, None);
    click(&mut h, "Promotions\nPromotions");
    click(&mut h, "Remove category");
    shot(&mut h, theme, "09-confirm-remove");

    for (reason, message, name, wait) in [
        (
            "categories_changed",
            "categories changed since export; export again",
            "10-categories-changed",
            words_changed(),
        ),
        (
            "config_busy",
            "configuration is being edited",
            "11-busy",
            "Try again",
        ),
    ] {
        let fake = answering();
        fake.fail("categories-apply", 5, message, Some(reason));
        let mut h = loaded(&fake, d, theme, None);
        click(&mut h, "News\nNews");
        type_into(&mut h, "Name", "Newsletters");
        settle(&mut h, |h| shows(h, "1 renamed"));
        click(&mut h, "Apply");
        click(&mut h, "Apply changes");
        settle(&mut h, |h| shows(h, wait));
        shot(&mut h, theme, name);
    }

    let fake = answering();
    let mut h = loaded(&fake, d, theme, None);
    click(&mut h, "Move filed mail…");
    settle(&mut h, |h| shows(h, "no longer used"));
    shot(&mut h, theme, "12-refile");
    click(&mut h, "Move mail from Promotions");
    settle(&mut h, |h| shows(h, "Marked 30 messages"));
    shot(&mut h, theme, "13-refile-moved");

    let fake = answering();
    fake.respond(
        "service-status",
        &support::fixture("status.json", &fake.config())
            .replace("\"filing_mode\":\"live\"", "\"filing_mode\":\"dry_run\""),
    );
    let mut h = loaded(&fake, d, theme, None);
    click(&mut h, "Move filed mail…");
    settle(&mut h, |h| shows(h, "no longer used"));
    shot(&mut h, theme, "14-refile-dry-run");

    let fake = answering();
    fake.respond_fixture("categories-export", "export-info.json");
    let mut h = loaded(&fake, d, theme, Some("info"));
    shot(&mut h, theme, "15-empty-filing-off");

    let mut h = Harness::builder()
        .with_size(egui::vec2(720.0, 520.0))
        .with_theme(theme)
        .wgpu()
        .build_eframe(|cc| {
            Editor::new(
                cc,
                Err("mailtriage not found (looked in /Applications/mailtriage, PATH)".into()),
                Arc::new(AtomicI32::new(0)),
            )
        });
    shot(&mut h, theme, "16-not-found");
    assert!(h.query_by_label_contains("mailtriage not found").is_some());
}

fn words_changed() -> &'static str {
    mailtriage_tray::model::words::CATEGORIES_CHANGED
}

#[test]
#[ignore]
fn window_in_light_mode() {
    window_screens(Theme::Light);
}

#[test]
#[ignore]
fn window_in_dark_mode() {
    window_screens(Theme::Dark);
}

fn print(entries: &[Entry], depth: usize, out: &mut String) {
    for e in entries {
        let pad = "    ".repeat(depth);
        match e {
            Entry::Text(t) => out.push_str(&format!("{pad}{t}\n")),
            Entry::Item { label, .. } => out.push_str(&format!("{pad}[{label}]\n")),
            Entry::Check { label, checked, .. } => out.push_str(&format!(
                "{pad}[{label}{}]\n",
                if *checked { " ✓" } else { "" }
            )),
            Entry::Separator => out.push_str(&format!("{pad}──────\n")),
            Entry::Submenu { label, entries } => {
                out.push_str(&format!("{pad}{label}  ▸\n"));
                print(entries, depth + 1, out);
            }
        }
    }
}

/// A named change to the fixture's status, and when the tray first saw it.
type Variant = (
    &'static str,
    Box<dyn Fn(&mut cli::Status)>,
    Option<DateTime<Utc>>,
);

/// A generator for the design review, not a check: it writes the tray
/// menu for every account state of the health table to
/// `target/tray-screens/menus.txt`, for a person to read (the real menu bar
/// cannot be captured).
#[test]
#[ignore]
fn tray_menus_as_text() {
    let config = PathBuf::from("/Users/daniel/.config/mailtriage/mailtriage.json");
    let base: cli::Status =
        serde_json::from_str(&support::fixture("status.json", &config).replace(
            "2026-10-06T10:03:00+00:00",
            &(Utc::now() - Duration::minutes(2)).to_rfc3339(),
        ))
        .unwrap();
    let now = Utc::now();
    let variants: Vec<Variant> = vec![
        ("as-fixture", Box::new(|_| {}), None),
        (
            "all-ok",
            Box::new(|s| {
                s.services[0].last_pass.as_mut().unwrap().exit_code = 0;
                s.services[1] = s.services[0].clone();
                s.services[1].account = "info".into();
                s.services[1].identity = "info".into();
            }),
            None,
        ),
        (
            "stopped",
            Box::new(|s| {
                s.services[0].running = false;
                s.services[0].enabled = Some(false);
            }),
            None,
        ),
        (
            "restarting",
            Box::new(|s| s.services[0].running = false),
            Some(now),
        ),
        (
            "error",
            Box::new(|s| s.services[0].last_pass.as_mut().unwrap().exit_code = 3),
            None,
        ),
        (
            "other-config",
            Box::new(|s| {
                s.services[0].config_matches = Some(false);
                s.services[0].service_config = Some("/Users/daniel/old/mailtriage.json".into());
            }),
            None,
        ),
        (
            "unknown",
            Box::new(|s| s.services[0].config_matches = None),
            None,
        ),
        (
            "unavailable",
            Box::new(|s| {
                for x in &mut s.services {
                    x.manager = "none".into();
                    x.installed = false;
                    x.running = false;
                }
            }),
            None,
        ),
    ];
    let mut out = String::new();
    for (name, change, seen_stopped) in variants {
        let mut status = base.clone();
        change(&mut status);
        let mut seen = Observations::default();
        seen.observe(
            &status.services,
            seen_stopped.unwrap_or(now - Duration::minutes(10)),
        );
        let busy: BTreeMap<String, Verb> = BTreeMap::new();
        let menu = menu::build(&Input {
            status: Some(&status),
            stale: false,
            failure: None,
            observations: &seen,
            now,
            local: chrono::Local::now().fixed_offset(),
            busy: &busy,
            notices: &[Notice {
                text: "the categories window is already open".into(),
                details: None,
            }][..usize::from(name == "as-fixture")],
            autostart: name == "all-ok",
            tray_version: "0.3.0",
            cli_version: Some("0.3.0"),
            cli_path: Some(Path::new("/usr/local/bin/mailtriage")),
        });
        out.push_str(&format!("=== {name} (icon {:?})\n", menu.icon));
        print(&menu.entries, 0, &mut out);
        out.push('\n');
    }
    fs::create_dir_all(out_dir()).unwrap();
    let menus = out_dir().join("menus.txt");
    fs::write(&menus, out).unwrap();
    let written = fs::read_to_string(&menus).unwrap();
    assert!(!written.is_empty(), "{} is empty", menus.display());
    assert_eq!(written.matches("=== ").count(), 8, "one menu per state");
}

/// A generator for a manual run, not a check: it prepares
/// `target/tray-run/` (a fake `mailtriage`, its answers, a config and a
/// private HOME) so a person can run the real tray and window against it,
/// and prints the command line that does.
#[test]
#[ignore]
fn a_fake_cli_for_a_real_run() {
    // Not through `tray-screens/..`: that path only resolves once another
    // test has created `tray-screens`.
    let run = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/tray-run");
    let _ = fs::remove_dir_all(&run);
    fs::create_dir_all(run.join("home")).unwrap();
    let run = fs::canonicalize(&run).unwrap();
    support::write_script(&run.join("mailtriage"), support::FAKE_MAILTRIAGE);
    let config = run.join("mailtriage.json");
    fs::write(&config, "{}").unwrap();
    let answers = [
        ("service-status", "status.json"),
        ("categories-export", "export-daniel.json"),
        ("categories-validate", "validate-ok.json"),
        ("filing-refile", "refile-preview.json"),
        ("filing-refile-apply", "refile-marked-folder.json"),
    ];
    for (key, name) in answers {
        fs::write(
            run.join(format!("{key}.json")),
            support::fixture(name, &config),
        )
        .unwrap();
    }
    fs::write(run.join("categories-apply.json"), "{\"schema_version\":1}").unwrap();
    let written = answers
        .iter()
        .map(|(key, _)| format!("{key}.json"))
        .chain(["categories-apply.json", "mailtriage", "mailtriage.json"].map(String::from));
    for name in written {
        let len = fs::metadata(run.join(&name)).map_or(0, |m| m.len());
        assert!(len > 0, "{name} is missing or empty in {}", run.display());
    }
    assert!(run.join("home").is_dir());
    println!(
        "HOME={home} target/debug/mailtriage-tray --config {config} --mailtriage {cli}",
        home = run.join("home").display(),
        config = config.display(),
        cli = run.join("mailtriage").display()
    );
}
