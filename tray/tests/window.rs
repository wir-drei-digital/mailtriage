//! The categories window, headless with egui_kittest, against a fake
//! `mailtriage`. Controls are found by their accessible labels.
mod support;
use eframe::egui::{self, Key, Modifiers};
use egui_kittest::{kittest::Queryable, Harness};
use mailtriage_tray::{
    cli::Cli,
    editor::{self, Editor, Setup},
    instances::{self, EditorLock},
    model::words,
    paths,
};
use std::{
    fs,
    process::{Command, Output, Stdio},
    sync::{atomic::AtomicI32, Arc},
    time::{Duration, Instant},
};
use support::{
    window::{click, disabled, fast, open, rename_news, settle, shows, type_into},
    FakeCli,
};

/// A fake that answers a window on `daniel` (filing live).
fn fake() -> FakeCli {
    let fake = FakeCli::new();
    fake.respond_fixture("service-status", "status.json");
    fake.respond_fixture("categories-export", "export-daniel.json");
    fake.respond_fixture("categories-validate", "validate-ok.json");
    fake.respond("categories-apply", "{\"schema_version\":1}");
    fake
}

#[test]
fn apply_sends_the_confirmed_revision_with_the_load_digest() {
    let fake = fake();
    let drafts = tempfile::tempdir().unwrap();
    let mut h = open(&fake, drafts.path());
    assert!(disabled(&h, "Apply"));
    rename_news(&mut h);
    assert!(shows(&h, "1 renamed"));
    let revision = h.state().model().revision;
    click(&mut h, "Apply");
    assert!(shows(&h, "Rename News to Newsletters"));
    fake.hold("categories-apply");
    click(&mut h, "Apply changes");
    settle(&mut h, |_| !fake.calls_of("categories", "apply").is_empty());
    // While apply runs the form is read-only.
    assert!(disabled(&h, "Name"));
    assert!(disabled(&h, "Reload"));
    fake.release("categories-apply");
    settle(&mut h, |h| shows(h, "Saved."));
    let draft = drafts.path().join(format!("draft-{revision}.json"));
    let apply = &fake.calls_of("categories", "apply")[0];
    assert_eq!(
        apply[2..],
        [
            "--account",
            "daniel",
            "--file",
            draft.to_str().unwrap(),
            "--expect-digest",
            "v1:aaaa",
            "--json",
            "--config",
            fake.config().to_str().unwrap()
        ]
    );
    let sent: serde_json::Value = serde_json::from_slice(&fs::read(&draft).unwrap()).unwrap();
    assert_eq!(sent["categories"][1]["name"], "Newsletters");
    assert_eq!(sent["categories"][1]["folder"], "News");
}

#[test]
fn categories_changed_offers_a_reload_and_keep_editing_keeps_the_draft() {
    let fake = fake();
    fake.fail(
        "categories-apply",
        5,
        "categories changed since export; export again",
        Some("categories_changed"),
    );
    let drafts = tempfile::tempdir().unwrap();
    let mut h = open(&fake, drafts.path());
    rename_news(&mut h);
    click(&mut h, "Apply");
    click(&mut h, "Apply changes");
    settle(&mut h, |h| shows(h, words::CATEGORIES_CHANGED));
    assert!(shows(&h, "Reload (discard my edits)"));
    click(&mut h, "Keep editing");
    assert_eq!(
        h.get_by_label("Name").value().as_deref(),
        Some("Newsletters")
    );
}

#[test]
fn config_busy_is_retried_then_offers_try_again() {
    let fake = fake();
    fake.fail(
        "categories-apply",
        5,
        "configuration is being edited",
        Some("config_busy"),
    );
    let drafts = tempfile::tempdir().unwrap();
    let mut h = open(&fake, drafts.path());
    rename_news(&mut h);
    click(&mut h, "Apply");
    click(&mut h, "Apply changes");
    settle(&mut h, |h| shows(h, "Try again"));
    assert_eq!(fake.calls_of("categories", "apply").len(), 4);
    assert!(shows(&h, words::BUSY));
}

#[test]
fn config_changed_checks_the_draft_again() {
    let fake = fake();
    fake.fail(
        "categories-apply",
        5,
        "configuration changed during command; retry with current configuration",
        Some("config_changed"),
    );
    let drafts = tempfile::tempdir().unwrap();
    let mut h = open(&fake, drafts.path());
    rename_news(&mut h);
    let checks = fake.calls_of("categories", "validate").len();
    click(&mut h, "Apply");
    click(&mut h, "Apply changes");
    settle(&mut h, |_| {
        fake.calls_of("categories", "validate").len() > checks
    });
    settle(&mut h, |h| !disabled(h, "Apply"));
    let calls = fake.calls_of("categories", "validate");
    assert_eq!(
        calls[calls.len() - 1],
        calls[calls.len() - 2],
        "same draft file"
    );
}

#[test]
fn a_validation_error_shows_inline_and_an_older_answer_does_not_enable_apply() {
    let fake = fake();
    let drafts = tempfile::tempdir().unwrap();
    let mut h = open(&fake, drafts.path());
    click(&mut h, "News\nNews");
    let first = h.state().model().revision + 1;
    fake.hold(&format!("draft-{first}"));
    type_into(&mut h, "Name", "Newsletters");
    settle(&mut h, |_| {
        !fake.calls_of("categories", "validate").is_empty()
            || drafts.path().join(format!("draft-{first}.json")).exists()
    });
    fake.fail(
        "categories-validate",
        2,
        "invalid categories; require unique IDs, descriptions and exactly one catch-all",
        None,
    );
    type_into(&mut h, "What belongs here", "");
    settle(&mut h, |h| shows(h, "Every category needs a name"));
    assert!(disabled(&h, "Apply"));
    fake.respond_fixture("categories-validate", "validate-ok.json");
    fake.release(&format!("draft-{first}"));
    settle(&mut h, |_| {
        !drafts.path().join(format!("draft-{first}.json")).exists()
    });
    assert!(disabled(&h, "Apply"));
    assert!(shows(&h, "Every category needs a name"));
}

#[test]
fn a_reclassifying_change_warns_in_the_confirmation() {
    let fake = fake();
    fake.respond_fixture("categories-validate", "validate-reclassify.json");
    let drafts = tempfile::tempdir().unwrap();
    let mut h = open(&fake, drafts.path());
    h.key_press_modifiers(Modifiers::COMMAND, Key::N);
    h.step();
    type_into(&mut h, "Name", "Travel");
    type_into(&mut h, "What belongs here", "Trips and bookings");
    settle(&mut h, |h| !disabled(h, "Apply"));
    h.key_press_modifiers(Modifiers::COMMAND, Key::S);
    h.step();
    assert!(shows(&h, "Add Travel"));
    assert!(shows(&h, words::RECLASSIFY_WARNING));
}

#[test]
fn remove_asks_first_and_writes_nothing() {
    let fake = fake();
    let drafts = tempfile::tempdir().unwrap();
    let mut h = open(&fake, drafts.path());
    click(&mut h, "Promotions\nPromotions");
    click(&mut h, "Remove category");
    assert!(shows(
        &h,
        "Mail in Promotions stays there until you move it"
    ));
    click(&mut h, "Remove");
    assert!(h.query_by_label("Promotions\nPromotions").is_none());
    assert!(fake.calls_of("categories", "apply").is_empty());
}

#[test]
fn closing_asks_about_edits_and_waits_for_a_command() {
    let fake = fake();
    let drafts = tempfile::tempdir().unwrap();
    let mut h = open(&fake, drafts.path());
    rename_news(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::W);
    h.step();
    assert!(shows(&h, "Discard changes?"));
    click(&mut h, "Cancel");
    assert!(!h.state().closed());
    fake.hold("categories-apply");
    click(&mut h, "Apply");
    click(&mut h, "Apply changes");
    settle(&mut h, |_| !fake.calls_of("categories", "apply").is_empty());
    h.key_press_modifiers(Modifiers::COMMAND, Key::W);
    h.step();
    assert!(shows(&h, "Finishing…"));
    assert!(!h.state().closed());
    fake.release("categories-apply");
    settle(&mut h, |h| h.state().closed());
}

#[test]
fn the_refile_panel_moves_a_retired_folder_and_reports_the_result() {
    let fake = fake();
    fake.respond_fixture("filing-refile", "refile-preview.json");
    fake.respond_fixture("filing-refile-apply", "refile-marked-folder.json");
    let drafts = tempfile::tempdir().unwrap();
    let mut h = open(&fake, drafts.path());
    click(&mut h, "Move filed mail…");
    settle(&mut h, |h| shows(h, "Promotions is no longer used"));
    assert!(shows(&h, "38 messages are in a folder that no longer matches their category (30 of them in folders no longer used)."));
    assert!(shows(&h, "Move all 38"));
    // Skipped counts are collapsed.
    assert!(!shows(&h, "corrected by you: 3"));
    click(&mut h, "Not moved");
    assert!(shows(&h, "corrected by you: 3"));
    click(&mut h, "Move mail from Promotions");
    settle(&mut h, |h| shows(h, "Marked 30 messages"));
    let moved = &fake.calls_of("filing", "refile")[1];
    assert_eq!(
        moved[2..8],
        [
            "--account",
            "daniel",
            "--folder",
            "INBOX.Promotions",
            "--apply",
            "--json"
        ]
    );
    assert!(shows(
        &h,
        "12 more will move if their new category calls for it."
    ));
}

#[test]
fn dry_run_shows_the_preview_only() {
    let fake = fake();
    let status = support::fixture("status.json", &fake.config())
        .replace("\"filing_mode\":\"live\"", "\"filing_mode\":\"dry_run\"");
    fake.respond("service-status", &status);
    fake.respond_fixture("filing-refile", "refile-preview.json");
    let drafts = tempfile::tempdir().unwrap();
    let mut h = open(&fake, drafts.path());
    click(&mut h, "Move filed mail…");
    settle(&mut h, |h| shows(h, "Promotions is no longer used"));
    assert!(shows(&h, words::DRY_RUN));
    assert!(h.query_by_label("Move mail from Promotions").is_none());
    assert!(h.query_by_label_contains("Move all").is_none());
}

#[test]
fn a_window_without_mailtriage_explains_and_exits_3() {
    let exit = Arc::new(AtomicI32::new(0));
    let h = Harness::builder()
        .with_size(egui::vec2(720.0, 520.0))
        .build_eframe(|cc| {
            Editor::new(
                cc,
                Err("mailtriage not found (looked in /x/mailtriage, PATH)".into()),
                Arc::clone(&exit),
            )
        });
    assert!(h.query_by_label_contains("mailtriage not found").is_some());
    assert_eq!(exit.load(std::sync::atomic::Ordering::SeqCst), 3);
}

/// Hovers the control labelled `label` until its tooltip shows `why`. The
/// pointer first rests on an empty spot, so no earlier tooltip counts.
fn says_why(h: &mut Harness<Editor>, label: &str, why: &str) -> bool {
    h.hover_at(egui::pos2(4.0, 4.0));
    h.step();
    h.step();
    assert!(!shows(h, why), "no tooltip before hovering {label}");
    h.get_by_label(label).hover();
    for _ in 0..8 {
        h.step();
        if shows(h, why) {
            return true;
        }
    }
    false
}

/// Disabled controls say why in their tooltip, with the actual reason.
#[test]
fn disabled_controls_say_why() {
    let fake = fake();
    fake.respond_fixture("filing-refile", "refile-preview.json");
    fake.respond_fixture("filing-refile-apply", "refile-marked-folder.json");
    let drafts = tempfile::tempdir().unwrap();
    let mut h = open(&fake, drafts.path());
    click(&mut h, "Move filed mail…");
    settle(&mut h, |h| shows(h, "Move all 38"));
    fake.hold("filing-refile-apply");
    click(&mut h, "Move mail from Promotions");
    settle(&mut h, |_| fake.calls_of("filing", "refile").len() >= 2);
    let busy = "Wait until the current action finishes";
    for label in [
        "Move mail from Promotions",
        "Move all 38",
        "Add",
        "Name",
        "Account",
        "Reload",
    ] {
        assert!(disabled(&h, label), "{label} is disabled");
        assert!(says_why(&mut h, label, busy), "{label} says why");
    }
    fake.release("filing-refile-apply");
    settle(&mut h, |h| shows(h, "Marked 30 messages"));
    // The default category cannot be removed, and says so.
    click(&mut h, "Other\nOther · default");
    assert!(disabled(&h, "Remove category"));
    assert!(says_why(
        &mut h,
        "Remove category",
        "Choose another default category first"
    ));
}

/// While a dialog is open, the form behind it takes no input: egui's modal
/// takes the keyboard focus and the pointer, and the window ignores what the
/// controls behind it send (an AccessKit click reaches them otherwise).
#[test]
fn a_dialog_blocks_the_form_behind_it() {
    let fake = fake();
    let drafts = tempfile::tempdir().unwrap();
    let mut h = open(&fake, drafts.path());
    rename_news(&mut h);
    // The name field keeps the keyboard focus while the question opens.
    h.key_press_modifiers(Modifiers::COMMAND, Key::W);
    for _ in 0..3 {
        h.step();
    }
    assert!(shows(&h, "Discard changes?"));
    let revision = h.state().model().revision;
    h.get_by_label("Name").type_text(" and more");
    h.step();
    h.get_by_label("Name").focus();
    h.step();
    h.get_by_label("Name").type_text(" and more");
    h.step();
    h.get_by_label("Add").click_accesskit();
    h.step();
    h.key_press_modifiers(Modifiers::COMMAND, Key::N);
    h.step();
    let model = h.state().model();
    assert_eq!(model.revision, revision);
    assert_eq!(model.draft.len(), 4);
    assert_eq!(model.draft[1].name, "Newsletters");
    assert!(shows(&h, "Discard changes?"));
}

/// The window's private (0700) drafts directory goes with the window.
#[test]
fn the_drafts_directory_is_removed_with_the_window() {
    let fake = fake();
    let temp = editor::drafts_dir().unwrap();
    let dir = temp.path().to_path_buf();
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = dir.metadata().unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "private");
    }
    let setup = Setup {
        cli: Cli {
            program: fake.program.clone(),
            config: Some(fake.config()),
        },
        account: None,
        options: fast(),
        drafts: dir.clone(),
        lock: None,
        temp: Some(temp),
    };
    let mut h = Harness::builder()
        .with_size(egui::vec2(720.0, 520.0))
        .build_eframe(|cc| Editor::new(cc, Ok(setup), Arc::new(AtomicI32::new(0))));
    settle(&mut h, |h| h.state().model().loaded.is_some());
    rename_news(&mut h);
    assert!(fs::read_dir(&dir).unwrap().next().is_some(), "a draft");
    h.key_press_modifiers(Modifiers::COMMAND, Key::W);
    h.step();
    click(&mut h, "Discard");
    assert!(h.state().closed());
    drop(h);
    assert!(!dir.exists());
}

/// Runs `command` and waits at most `limit`; a window that did not stop at
/// its lock is killed at the deadline and the test fails.
fn output_within(command: &mut Command, limit: Duration) -> Output {
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
        std::thread::sleep(Duration::from_millis(20));
    }
    child.wait_with_output().unwrap()
}

/// A second `categories` for the same config exits 0 before any window
/// opens. The `.pid` file names a process that has exited and been reaped,
/// so bringing it to the front (macOS) cannot activate a real application.
#[test]
fn a_second_window_for_the_same_config_exits_0() {
    let fake = FakeCli::new();
    let home = tempfile::tempdir().unwrap();
    let xdg = home.path().join("xdg-cache");
    let cache = paths::cache_dir(Some(home.path().as_os_str()), Some(xdg.as_os_str())).unwrap();
    let config = fake.config();
    let EditorLock::Taken(_held) = instances::editor_lock(&cache, &config).unwrap() else {
        panic!("the first window's lock")
    };
    let mut gone = Command::new("/bin/sh")
        .args(["-c", "exit 0"])
        .spawn()
        .unwrap();
    let gone_pid = gone.id();
    gone.wait().unwrap();
    fs::write(
        cache.join(format!("{}.pid", instances::editor_name(&config))),
        gone_pid.to_string(),
    )
    .unwrap();
    // What the second window will find: the lock held, naming that PID.
    assert!(matches!(
        instances::editor_lock(&cache, &config).unwrap(),
        EditorLock::HeldBy(Some(pid)) if pid == gone_pid
    ));
    let out = output_within(
        Command::new(env!("CARGO_BIN_EXE_mailtriage-tray"))
            .arg("categories")
            .arg("--config")
            .arg(&config)
            .arg("--mailtriage")
            .arg(&fake.program)
            .env("HOME", home.path())
            .env("XDG_CACHE_HOME", &xdg)
            // Should the lock check ever come too late, a window cannot
            // open on Linux (macOS needs no display variable).
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY"),
        Duration::from_secs(10),
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "the categories window is already open\n"
    );
    // Nothing ran: the lock check comes before any command or window.
    assert!(fake.calls().is_empty());
}
