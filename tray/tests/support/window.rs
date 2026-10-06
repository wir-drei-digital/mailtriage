//! Driving the categories window headless: open it on a fake `mailtriage`,
//! step it until workers answer, and use controls by their accessible labels.
use super::FakeCli;
use eframe::egui::{self, Key, Modifiers};
use egui_kittest::{
    kittest::{NodeT, Queryable},
    Harness,
};
use mailtriage_tray::{
    cli::Cli,
    editor::{Editor, Setup},
    model::window::Options,
};
use std::{
    path::Path,
    sync::{atomic::AtomicI32, Arc},
    time::Duration,
};

pub fn fast() -> Options {
    Options {
        check_delay: Duration::from_millis(20),
        retry_delay: Duration::from_millis(20),
        retries: 3,
    }
}

/// The window for `cli`, loaded.
pub fn open_with<'a>(cli: Cli, drafts: &Path, account: Option<&str>) -> Harness<'a, Editor> {
    let setup = Setup {
        cli,
        account: account.map(str::to_owned),
        options: fast(),
        drafts: drafts.to_path_buf(),
        lock: None,
        temp: None,
    };
    let mut h = Harness::builder()
        .with_size(egui::vec2(720.0, 520.0))
        .build_eframe(|cc| Editor::new(cc, Ok(setup), Arc::new(AtomicI32::new(0))));
    settle(&mut h, |h| h.state().model().loaded.is_some());
    h
}

/// The window on the fake's config.
pub fn open<'a>(fake: &FakeCli, drafts: &Path) -> Harness<'a, Editor> {
    open_with(
        Cli {
            program: fake.program.clone(),
            config: Some(fake.config()),
        },
        drafts,
        None,
    )
}

/// Steps the window until `done` holds (workers answer in real time).
pub fn settle(h: &mut Harness<Editor>, done: impl Fn(&Harness<Editor>) -> bool) {
    for _ in 0..400 {
        h.step();
        if done(h) {
            h.step();
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the window did not settle");
}

/// Clicks the control labelled `label` and lets the frames settle: egui
/// lays out a dialog it opens over two frames.
pub fn click(h: &mut Harness<Editor>, label: &str) {
    h.get_by_label(label).click();
    for _ in 0..3 {
        h.step();
    }
}

pub fn disabled(h: &Harness<Editor>, label: &str) -> bool {
    h.get_by_label(label).accesskit_node().is_disabled()
}

/// Some control or text contains `text`.
pub fn shows(h: &Harness<Editor>, text: &str) -> bool {
    h.query_all_by_label_contains(text).next().is_some()
}

/// Replaces the text of the field labelled `label` (one edit).
pub fn type_into(h: &mut Harness<Editor>, label: &str, text: &str) {
    h.get_by_label(label).focus();
    h.step();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.step();
    if text.is_empty() {
        h.key_press(Key::Backspace);
    } else {
        h.get_by_label(label).type_text(text);
    }
    h.step();
}

/// Renames the `news` category to "Newsletters" and waits for its check.
pub fn rename_news(h: &mut Harness<Editor>) {
    click(h, "News\nNews");
    type_into(h, "Name", "Newsletters");
    settle(h, |h| !disabled(h, "Apply"));
}
