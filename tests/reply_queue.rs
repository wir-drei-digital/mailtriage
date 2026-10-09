//! Reply queue spec (`docs/superpowers/specs/2026-10-08-reply-queue-design.md`)
//! through `sync` passes against the fake engine.
mod common;
use common::{mail, Harness};
use mailtriage::{
    domain::FilingMode::{self, DryRun, Live},
    engine::fake::{FakeOp, Fault},
    service::Backfill,
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

fn paused(h: &Harness, folder: &str) -> Option<String> {
    h.service()
        .store
        .folder_record("work", folder)
        .unwrap()
        .unwrap()
        .pause_reason
}

/// The `epoch_race` events of kind `seen`, as their `error`s.
fn seen_races(h: &Harness, mid: &str) -> Vec<String> {
    h.service()
        .store
        .events("work", Some(&id_of(h, mid)), 50)
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "epoch_race" && e["detail"]["kind"] == "seen")
        .map(|e| e["detail"]["error"].as_str().unwrap().to_string())
        .collect()
}

fn has_problem(out: &serde_json::Value, code: &str) -> bool {
    out["filing"]["problems"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p == code)
}

/// Holds and answers `mids` one pass at a time, so they reach
/// Transactions as UIDs 1, 2, ... in order.
fn file_in_order(h: &Harness, mids: &[&str]) {
    h.sync();
    for mid in mids {
        h.fake
            .deliver("INBOX", &mail(mid, "Invoice", "Payment due next month"));
    }
    h.sync();
    for mid in mids {
        answer(h, mid);
        h.sync();
    }
    h.sync(); // confirms the last move's arrival
}

#[test]
fn a_seen_write_in_another_epoch_pauses_the_folder() {
    let h = queued(Live);
    file_in_order(&h, &["x", "a", "b"]);
    assert_eq!(h.fake.uids("Transactions"), vec![1, 2, 3]);
    assert_eq!(place(&h, "a"), ("Transactions".to_string(), 2));
    h.fake.client_delete("Transactions", 1);
    h.sync();
    let a = id_of(&h, "a");
    h.service()
        .filing_replies("work", true, std::slice::from_ref(&a))
        .unwrap();
    // The session renumbers Transactions: UID 2 now names b.
    h.fake.inject(FakeOp::Seen, Fault::EpochRaceBefore);
    let out = h.sync();
    assert!(has_problem(&out, "epoch_race:Transactions"), "{out}");
    assert_eq!(paused(&h, "Transactions").as_deref(), Some("epoch_race"));
    assert_eq!(seen_races(&h, "a"), vec!["epoch_race"]);
    assert!(has(&flags(&h, "b"), "\\Seen"), "the race marked b");
    assert!(!has(&flags(&h, "a"), "\\Seen"));
    let replies = h.service().filing_replies("work", false, &[]).unwrap();
    let approved_at = |mid: &str| {
        let id = id_of(&h, mid);
        let items = replies["items"].as_array().unwrap();
        items.iter().find(|i| i["id"] == id.as_str()).unwrap()["approved_at"].clone()
    };
    assert!(approved_at("b").is_null(), "b still waits: {replies}");
    assert!(!approved_at("a").is_null(), "a stays approved: {replies}");
    let writes = seen_writes(&h);
    h.sync();
    assert_eq!(seen_writes(&h), writes, "a paused folder is not written");
    assert_eq!(seen_races(&h, "a").len(), 1, "the race is reported once");
}

#[test]
fn a_lost_seen_outcome_followed_by_an_epoch_change_is_a_suspected_race() {
    let h = queued(Live);
    file_in_order(&h, &["i"]);
    h.service().filing_replies("work", true, &[]).unwrap();
    h.fake.inject(FakeOp::Seen, Fault::ErrorAfter);
    let out = h.sync();
    assert!(has_problem(&out, "read_failed:Transactions"), "{out}");
    h.fake.reset_epoch("Transactions");
    let out = h.sync();
    assert!(has_problem(&out, "epoch_race:Transactions"), "{out}");
    assert_eq!(paused(&h, "Transactions").as_deref(), Some("epoch_race"));
    assert_eq!(seen_races(&h, "i"), vec!["epoch_race_suspected"]);
    assert_eq!(seen_writes(&h), 1);
    h.sync();
    assert_eq!(seen_races(&h, "i").len(), 1, "the race is reported once");
}

#[test]
fn a_lost_seen_outcome_in_the_same_epoch_converges() {
    let h = queued(Live);
    file_in_order(&h, &["i"]);
    h.service().filing_replies("work", true, &[]).unwrap();
    h.fake.inject(FakeOp::Seen, Fault::ErrorAfter);
    h.sync();
    let out = h.sync();
    assert_eq!(out["filing"]["reads_applied"], 1, "{out}");
    assert_eq!(paused(&h, "Transactions"), None);
    assert!(seen_races(&h, "i").is_empty());
    assert_eq!(seen_writes(&h), 1, "the write had landed");
    let replies = h.service().filing_replies("work", false, &[]).unwrap();
    assert_eq!(replies["items"].as_array().unwrap().len(), 0);
}

#[test]
fn backfilled_mail_that_needs_action_files_and_is_flagged_as_before() {
    let h = queued(Live);
    h.sync();
    h.fake.deliver_at(
        "INBOX",
        &mail("old", "Invoice", "Payment due next month"),
        "2026-01-01T00:00:00+00:00",
    );
    h.sync();
    let mut s = h.service();
    assert_eq!(
        s.filing_backfill("work", Backfill::All, true).unwrap()["applied"],
        1
    );
    let out = h.sync();
    assert_eq!(out["filing"]["flagged"], 1, "{out}");
    assert_eq!(place(&h, "old").0, "Transactions");
    assert!(has(&flags(&h, "old"), "\\Flagged"));
    assert!(out["filing"].get("awaiting_reply").is_none(), "{out}");
}

#[test]
fn answered_mail_is_not_flagged_after_it_leaves_the_queue() {
    let h = queued(Live);
    file_in_order(&h, &["i"]);
    assert_eq!(place(&h, "i").0, "Transactions");
    h.sync();
    h.edit(|c| c.accounts.get_mut("work").unwrap().filing.reply_queue = false);
    h.sync();
    h.sync();
    assert_eq!(flags(&h, "i"), vec!["\\Answered".to_string()]);
    let flag_calls = h
        .fake
        .calls()
        .iter()
        .filter(|c| c.starts_with("flag "))
        .count();
    assert_eq!(flag_calls, 0);
}
