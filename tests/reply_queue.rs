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

fn seen_writes(h: &Harness) -> usize {
    h.fake
        .calls()
        .iter()
        .filter(|c| c.starts_with("seen "))
        .count()
}

fn has(flags: &[String], flag: &str) -> bool {
    flags.iter().any(|f| f == flag)
}

#[test]
fn answered_mail_is_filed_unread_until_the_user_approves() {
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
    assert!(has(&flags(&h, "u"), "\\Flagged"));

    let writes = h.fake.write_calls();
    h.sync();
    assert_eq!(h.fake.write_calls(), writes, "waiting is quiet");

    answer(&h, "i");
    let out = h.sync();
    assert_eq!(out["filing"]["reply_exits"], 1);
    assert_eq!(out["filing"]["moved"], 1);
    assert_eq!(place(&h, "i").0, "Transactions");
    assert!(has(&flags(&h, "i"), "\\Answered"));
    assert!(!has(&flags(&h, "i"), "\\Seen"), "unread until approved");
    assert_eq!(place(&h, "u").0, "INBOX", "unanswered mail keeps waiting");

    let writes = h.fake.write_calls();
    h.sync(); // confirms the move's arrival in Transactions
    assert_eq!(
        h.fake.write_calls(),
        writes,
        "nothing is read before approval"
    );
    let replies = h.service().filing_replies("work", false, &[]).unwrap();
    assert_eq!(replies["waiting"], 1, "{replies}");
    let item = &replies["items"][0];
    assert_eq!(item["id"], id_of(&h, "i").as_str());
    assert_eq!(item["folder"], "Transactions");
    assert_eq!(item["answered"], true);

    let approved = h.service().filing_replies("work", true, &[]).unwrap();
    assert_eq!(approved["approved"].as_array().unwrap().len(), 1);
    assert_eq!(approved["approved_pending"], 1);
    let out = h.sync();
    assert_eq!(out["filing"]["reads_applied"], 1, "{out}");
    assert!(has(&flags(&h, "i"), "\\Seen"));
    assert_eq!(seen_writes(&h), 1);
    let replies = h.service().filing_replies("work", false, &[]).unwrap();
    assert_eq!(replies["items"].as_array().unwrap().len(), 0);

    let writes = h.fake.write_calls();
    h.sync();
    assert_eq!(h.fake.write_calls(), writes, "second pass is quiet");
    let status = h.service().filing_status("work").unwrap();
    assert_eq!(status["reply_queue"], true);
    assert_eq!(status["awaiting_reply"], 1);
    assert_eq!(status["read_waiting"], 0);
    assert!(h.service().store.intents("work", true).unwrap().is_empty());
}

#[test]
fn approval_can_name_single_messages_and_refuses_unknown_ones() {
    let h = queued(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("a", "Invoice", "Please pay by next month"));
    h.fake
        .deliver("INBOX", &mail("b", "Invoice", "Payment due next month"));
    h.sync();
    answer(&h, "a");
    answer(&h, "b");
    h.sync();
    let a = id_of(&h, "a");
    assert!(h
        .service()
        .filing_replies("work", true, &["msg_unknown".into()])
        .is_err());
    let approved = h
        .service()
        .filing_replies("work", true, std::slice::from_ref(&a))
        .unwrap();
    assert_eq!(approved["approved"], serde_json::json!([a]));
    h.sync();
    assert!(has(&flags(&h, "a"), "\\Seen"));
    assert!(!has(&flags(&h, "b"), "\\Seen"));
}

#[test]
fn marking_held_mail_done_files_it_unread_for_approval() {
    let h = queued(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("i", "Invoice", "Please pay by next month"));
    h.sync();
    assert_eq!(place(&h, "i").0, "INBOX");
    h.service().review("work", &id_of(&h, "i"), true).unwrap();
    h.sync();
    assert_eq!(place(&h, "i").0, "Transactions");
    assert!(!has(&flags(&h, "i"), "\\Seen"));
    let replies = h.service().filing_replies("work", false, &[]).unwrap();
    assert_eq!(replies["waiting"], 1);
    assert_eq!(replies["items"][0]["answered"], false);
}

#[test]
fn mail_that_needs_no_action_files_as_before_and_stays_unread() {
    let h = queued(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.sync();
    assert_eq!(place(&h, "n").0, "Newsletters");
    assert!(!has(&flags(&h, "n"), "\\Seen"));
    let replies = h.service().filing_replies("work", false, &[]).unwrap();
    assert_eq!(replies["items"].as_array().unwrap().len(), 0);
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
fn a_lost_reply_exit_response_converges_without_a_second_move() {
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
    let moves = h
        .fake
        .calls()
        .iter()
        .filter(|c| c.starts_with("move "))
        .count();
    assert_eq!(moves, 1);
    assert_eq!(place(&h, "i").0, "Transactions");
    assert!(h.service().store.intents("work", true).unwrap().is_empty());
    let replies = h.service().filing_replies("work", false, &[]).unwrap();
    assert_eq!(replies["waiting"], 1);
}

#[test]
fn a_failed_read_write_is_retried_on_the_next_pass() {
    let h = queued(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("i", "Invoice", "Payment due next month"));
    h.sync();
    answer(&h, "i");
    h.sync();
    h.service().filing_replies("work", true, &[]).unwrap();
    h.fake.inject(FakeOp::Seen, Fault::ErrorBefore);
    let out = h.sync();
    assert!(
        out["filing"]["problems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "read_failed:Transactions"),
        "{out}"
    );
    assert!(!has(&flags(&h, "i"), "\\Seen"));
    h.sync();
    assert!(has(&flags(&h, "i"), "\\Seen"));
}
