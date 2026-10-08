//! Reply queue spec (`docs/superpowers/specs/2026-10-08-reply-queue-design.md`)
//! through `sync` passes against the fake engine.
mod common;
use common::{mail, Harness};
use mailtriage::{
    domain::FilingMode::{self, DryRun, Live},
    engine::fake::{FakeOp, Fault},
};

fn queued(mode: FilingMode) -> Harness {
    let h = Harness::new(mode);
    h.edit(|c| c.accounts.get_mut("work").unwrap().filing.reply_queue = true);
    h
}

fn place(h: &Harness, mid: &str) -> (String, u64) {
    h.fake.locate(&format!("<{mid}@test>"))[0].clone()
}

fn flags(h: &Harness, mid: &str) -> Vec<String> {
    let (folder, uid) = place(h, mid);
    h.fake.flags(&folder, uid)
}

fn id_of(h: &Harness, mid: &str) -> String {
    let rfc = format!("<{mid}@test>");
    h.service()
        .store
        .records("work")
        .unwrap()
        .into_iter()
        .find(|r| r.envelope["message_id"] == rfc.as_str())
        .unwrap()
        .id
}

fn answer(h: &Harness, mid: &str) {
    let (folder, uid) = place(h, mid);
    h.fake.client_set_flag(&folder, uid, "\\Answered", true);
}

fn seen_moves(h: &Harness) -> usize {
    h.fake
        .calls()
        .iter()
        .filter(|c| c.starts_with("move_seen "))
        .count()
}

#[test]
fn answered_mail_leaves_the_inbox_read_and_filed() {
    let h = queued(Live);
    h.sync(); // creates folders
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake
        .deliver("INBOX", &mail("i", "Invoice", "Payment due next month"));
    h.fake
        .deliver("INBOX", &mail("u", "Invoice", "Payment due today, urgent"));
    let out = h.sync();
    assert_eq!(out["filing"]["moved"], 1);
    assert_eq!(out["filing"]["awaiting_reply"], 2);
    assert_eq!(out["filing"]["flagged"], 1, "only high urgency is flagged");
    assert_eq!(place(&h, "n").0, "Newsletters");
    assert_eq!(place(&h, "i").0, "INBOX");
    assert_eq!(flags(&h, "i"), Vec::<String>::new());
    assert!(flags(&h, "u").contains(&"\\Flagged".to_string()));

    let writes = h.fake.write_calls();
    h.sync();
    assert_eq!(h.fake.write_calls(), writes, "waiting is quiet");

    answer(&h, "i");
    let out = h.sync();
    assert_eq!(out["filing"]["reply_exits"], 1);
    assert_eq!(out["filing"]["moved"], 1);
    assert_eq!(place(&h, "i").0, "Transactions");
    let after = flags(&h, "i");
    assert!(after.contains(&"\\Seen".to_string()), "{after:?}");
    assert!(after.contains(&"\\Answered".to_string()), "{after:?}");
    assert_eq!(seen_moves(&h), 1);
    assert_eq!(place(&h, "u").0, "INBOX", "unanswered mail keeps waiting");
    assert!(!flags(&h, "u").contains(&"\\Seen".to_string()));

    let writes = h.fake.write_calls();
    h.sync();
    assert_eq!(h.fake.write_calls(), writes, "second pass is quiet");
    let status = h.service().filing_status("work").unwrap();
    assert_eq!(status["reply_queue"], true);
    assert_eq!(status["awaiting_reply"], 1);
    assert!(h.service().store.intents("work", true).unwrap().is_empty());
}

#[test]
fn marking_held_mail_done_files_it_read() {
    let h = queued(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("i", "Invoice", "Please pay by next month"));
    h.sync();
    assert_eq!(place(&h, "i").0, "INBOX");
    h.service().review("work", &id_of(&h, "i"), true).unwrap();
    h.sync();
    assert_eq!(place(&h, "i").0, "Transactions");
    assert!(flags(&h, "i").contains(&"\\Seen".to_string()));
}

#[test]
fn mail_that_needs_no_action_files_as_before_and_stays_unread() {
    let h = queued(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.sync();
    assert_eq!(place(&h, "n").0, "Newsletters");
    assert!(!flags(&h, "n").contains(&"\\Seen".to_string()));
    assert_eq!(seen_moves(&h), 0);
}

#[test]
fn dry_run_previews_the_reply_exit_without_writes() {
    let h = queued(DryRun);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("i", "Invoice", "Payment due next month"));
    let out = h.sync();
    assert_eq!(out["filing"]["awaiting_reply"], 1);
    answer(&h, "i");
    h.sync();
    assert_eq!(h.fake.write_calls(), 0);
    let plan = h.service().filing_plan("work", 50).unwrap();
    let actions = plan["actions"].as_array().unwrap();
    assert!(
        actions
            .iter()
            .any(|a| a["action"] == "move" && a["reason"] == "reply_exit"),
        "{plan}"
    );
}

#[test]
fn a_lost_reply_exit_response_converges_without_a_second_write() {
    let h = queued(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("i", "Invoice", "Payment due next month"));
    h.sync();
    answer(&h, "i");
    h.fake.inject(FakeOp::Move, Fault::ErrorAfter);
    h.sync();
    h.sync();
    h.sync();
    assert_eq!(seen_moves(&h), 1);
    assert_eq!(
        h.fake
            .calls()
            .iter()
            .filter(|c| c.starts_with("move "))
            .count(),
        0,
        "no plain retry either"
    );
    assert_eq!(place(&h, "i").0, "Transactions");
    assert!(h.service().store.intents("work", true).unwrap().is_empty());
}

#[test]
fn a_failed_reply_exit_is_retried_without_seen() {
    let h = queued(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("i", "Invoice", "Payment due next month"));
    h.sync();
    answer(&h, "i");
    h.fake.inject(FakeOp::Move, Fault::ErrorBefore);
    h.sync();
    assert_eq!(place(&h, "i").0, "INBOX");
    h.service()
        .store
        .expire_intent_backoff_for_tests("work")
        .unwrap();
    h.sync();
    h.sync();
    assert_eq!(place(&h, "i").0, "Transactions");
    assert!(
        !flags(&h, "i").contains(&"\\Seen".to_string()),
        "the read state is added at most once, in the first attempt"
    );
}
