//! The categories window, headless with egui_kittest, against a fake
//! `mailtriage`. Controls are found by their accessible labels.
mod support;
use eframe::egui::{self, Key, Modifiers};
use egui_kittest::{
    kittest::{NodeT, Queryable},
    Harness,
};
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
    let mut h = Harness::builder()
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
    // No load runs, so the disabled controls do not ask to wait for one.
    for label in ["Add", "Apply"] {
        assert!(disabled(&h, label), "{label} is disabled");
        assert!(
            says_why(&mut h, label, words::NOT_LOADED),
            "{label} says why"
        );
    }
}

/// A dialog's buttons sit at its right edge, the confirming one rightmost,
/// and are created in that order, so Tab and screen readers meet the
/// other choice first.
#[test]
fn dialogs_end_with_the_confirming_button_at_the_right() {
    let fake = fake();
    let drafts = tempfile::tempdir().unwrap();
    let mut h = open(&fake, drafts.path());
    rename_news(&mut h);
    let check = |h: &Harness<Editor>, title: &str, confirm: &str| {
        let title = h.get_by_label_contains(title).rect();
        let cancel = h.get_by_label("Cancel").rect();
        let confirm_rect = h.get_by_label(confirm).rect();
        assert!(
            cancel.max.x < confirm_rect.min.x,
            "Cancel {cancel:?} is left of {confirm} {confirm_rect:?}"
        );
        // The dialog's content is 440 px wide, starting where its text does.
        assert!(
            (confirm_rect.max.x - (title.min.x + 440.0)).abs() <= 1.0,
            "{confirm} {confirm_rect:?} ends at the right edge of the dialog that starts at {title:?}"
        );
        let labels: Vec<String> = h
            .query_all_by(|n| n.label().is_some())
            .filter_map(|n| n.accesskit_node().label())
            .collect();
        let at = |label: &str| labels.iter().position(|l| l == label).unwrap();
        assert!(at("Cancel") < at(confirm), "{labels:#?}");
    };
    click(&mut h, "Apply");
    check(&h, "Apply these changes", "Apply changes");
    click(&mut h, "Cancel");
    click(&mut h, "Promotions\nPromotions");
    click(&mut h, "Remove category");
    check(&h, "Remove Promotions?", "Remove");
}

/// "Show details" opens the exact command, its exit code and its output in a
/// text box that screen readers announce as "Details", with Copy and Close.
#[test]
fn show_details_opens_the_command_in_a_labelled_box() {
    let fake = fake();
    let drafts = tempfile::tempdir().unwrap();
    let mut h = open(&fake, drafts.path());
    fake.fail(
        "categories-validate",
        2,
        "invalid categories; require unique IDs, descriptions and exactly one catch-all",
        None,
    );
    click(&mut h, "News\nNews");
    type_into(&mut h, "What belongs here", "");
    settle(&mut h, |h| shows(h, "Every category needs a name"));
    click(&mut h, "Show details");
    let text = h
        .query_all_by_label("Details")
        .find(|n| n.accesskit_node().role() == egui::accesskit::Role::MultilineTextInput)
        .expect("a text box labelled Details");
    let value = text.accesskit_node().value().unwrap_or_default();
    assert!(
        value.contains("categories validate --account daniel"),
        "{value}"
    );
    assert!(value.contains("exit code: 2"), "{value}");
    assert!(h.query_by_label("Copy").is_some());
    click(&mut h, "Close");
    assert!(h.query_by_label("Copy").is_none());
}

/// The refile panel's fixed room holds a whole preview, "Not moved"
/// included, above the footer.
#[test]
fn the_refile_panel_shows_a_whole_preview() {
    let fake = fake();
    fake.respond_fixture("filing-refile", "refile-preview.json");
    let drafts = tempfile::tempdir().unwrap();
    let mut h = open(&fake, drafts.path());
    click(&mut h, "Move filed mail…");
    settle(&mut h, |h| shows(h, "Move all 38"));
    let last = h.get_by_label("Not moved").rect();
    // The footer's buttons sit 8 px below its top; the panel's content ends
    // 8 px above that.
    let footer_top = h.get_by_label("Move filed mail…").rect().min.y - 8.0;
    assert!(
        last.max.y <= footer_top - 8.0,
        "\"Not moved\" {last:?} ends above the footer at {footer_top}"
    );
}

/// The window after a rename, with the refile panel open: every kind of
/// control is enabled and shown.
fn everything_shown(fake: &FakeCli, drafts: &std::path::Path) -> Harness<'static, Editor> {
    fake.respond_fixture("filing-refile", "refile-preview.json");
    let mut h = open(fake, drafts);
    rename_news(&mut h);
    click(&mut h, "Move filed mail…");
    settle(&mut h, |h| shows(h, "Move all 38"));
    h
}

/// The controls in the order the window shows them: header, list, form,
/// refile panel, then the footer from left to right, ending with Apply.
const VISUAL_ORDER: [&str; 19] = [
    "Account",
    "Reload",
    "Work\nWork",
    "Newsletters\nNews",
    "Promotions\nPromotions",
    "Other\nOther · default",
    "Add",
    "Name",
    "What belongs here",
    "Use for mail that fits nowhere else",
    "Mail folder",
    "Advanced",
    "Remove category",
    "Move mail from Promotions",
    "Move all 38",
    "Not moved",
    "Move filed mail…",
    "Revert",
    "Apply",
];

/// The label of the control with the keyboard focus.
fn focused(h: &Harness<Editor>) -> String {
    h.query_by(|n| n.is_focused())
        .and_then(|n| n.accesskit_node().label())
        .unwrap_or_default()
}

/// Tab moves through the window in the order it shows its controls, and
/// Shift+Tab goes back.
#[test]
fn tab_follows_the_visual_order() {
    let fake = fake();
    let drafts = tempfile::tempdir().unwrap();
    let mut h = everything_shown(&fake, drafts.path());
    h.get_by_label("Account").focus();
    h.step();
    let mut seen = vec![focused(&h)];
    while seen.len() < VISUAL_ORDER.len() {
        h.key_press(Key::Tab);
        h.step();
        seen.push(focused(&h));
    }
    assert_eq!(seen, VISUAL_ORDER);
    h.key_press_modifiers(Modifiers::SHIFT, Key::Tab);
    h.step();
    h.step();
    assert_eq!(focused(&h), "Revert");
    h.key_press_modifiers(Modifiers::SHIFT, Key::Tab);
    h.step();
    h.step();
    assert_eq!(focused(&h), "Move filed mail…");
}

/// Screen readers walk the AccessKit tree in document order, which must
/// be the visual order too.
#[test]
fn screen_readers_meet_the_controls_in_the_visual_order() {
    let fake = fake();
    let drafts = tempfile::tempdir().unwrap();
    let h = everything_shown(&fake, drafts.path());
    // Every labelled node in tree order (a field's label comes right
    // before the field, so either one stands for it).
    let labels: Vec<String> = h
        .query_all_by(|n| n.label().is_some())
        .filter_map(|n| n.accesskit_node().label())
        .collect();
    let at = |label: &str| {
        labels
            .iter()
            .position(|l| l == label)
            .unwrap_or_else(|| panic!("{label:?} not in {labels:#?}"))
    };
    for pair in VISUAL_ORDER.windows(2) {
        assert!(
            at(pair[0]) < at(pair[1]),
            "{:?} comes before {:?} in {labels:#?}",
            pair[0],
            pair[1]
        );
    }
}

/// The footer is laid out from the last pass's measurements: with a long
/// error that wraps, it settles at once (one pass per frame) and the whole
/// message stays inside the window, left of the buttons.
#[test]
fn a_long_message_wraps_in_a_footer_that_settles() {
    let fake = fake();
    let drafts = tempfile::tempdir().unwrap();
    let mut h = open(&fake, drafts.path());
    fake.fail(
        "categories-validate",
        2,
        "invalid categories; require unique IDs, descriptions and exactly one catch-all",
        None,
    );
    click(&mut h, "News\nNews");
    type_into(&mut h, "What belongs here", "");
    settle(&mut h, |h| shows(h, "Every category needs a name"));
    for _ in 0..4 {
        h.step();
        assert_eq!(h.output().platform_output.num_completed_passes, 1);
    }
    let message = h
        .get_by_label_contains("Every category needs a name")
        .rect();
    let window = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(720.0, 520.0));
    assert!(window.contains_rect(message), "{message:?}");
    assert!(message.height() > 24.0, "it wraps: {message:?}");
    assert!(message.max.x < h.get_by_label("Move filed mail…").rect().min.x);
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
