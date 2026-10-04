mod common;
use common::{mail, Harness};
use mailtriage::{
    domain::FilingMode::{DryRun, Live},
    engine::fake::{FakeOp, Fault},
};

fn place(h: &Harness, mid: &str) -> Vec<(String, u64)> {
    h.fake.locate(&format!("<{mid}@test>"))
}

#[test]
fn live_files_new_mail_and_flags_actionable_mail() {
    let h = Harness::new(Live);
    h.sync(); // creates folders
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake
        .deliver("INBOX", &mail("i", "Invoice", "Payment due today"));
    h.fake
        .deliver("INBOX", &mail("c", "Lunch", "Can you join us?"));
    let out = h.sync();
    assert_eq!(out["filing"]["moved"], 2);
    assert_eq!(out["filing"]["flagged"], 2);
    assert_eq!(place(&h, "n")[0].0, "Newsletters");
    let (folder, uid) = place(&h, "i")[0].clone();
    assert_eq!(folder, "Transactions");
    assert!(h
        .fake
        .flags(&folder, uid)
        .contains(&"\\Flagged".to_string()));
    let (folder, uid) = place(&h, "c")[0].clone();
    assert_eq!(folder, "INBOX", "correspondence targets INBOX");
    assert!(h
        .fake
        .flags(&folder, uid)
        .contains(&"\\Flagged".to_string()));
    let writes = h.fake.write_calls();
    h.sync();
    assert_eq!(h.fake.write_calls(), writes, "second pass is quiet");
    let s = h.service();
    for p in s.store.placements("work").unwrap() {
        if p.home_folder.as_deref() != Some("INBOX") {
            assert_eq!(p.filed_by.as_deref(), Some("mailtriage"));
        }
    }
    assert!(s.store.intents("work", true).unwrap().is_empty());
}

#[test]
fn dry_run_then_live_files_mail_that_arrived_during_dry_run() {
    let h = Harness::new(DryRun);
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.sync();
    h.sync();
    assert_eq!(h.fake.write_calls(), 0);
    h.set_mode(Live);
    h.sync();
    assert_eq!(place(&h, "n")[0].0, "Newsletters");
}

#[test]
fn backlog_is_untouched() {
    let h = Harness::new(Live);
    h.fake.deliver_at(
        "INBOX",
        &mail(
            "old",
            "Old newsletter",
            "Can you review this newsletter today?",
        ),
        "2026-01-01T00:00:00+00:00",
    );
    h.sync();
    h.sync();
    assert_eq!(place(&h, "old")[0].0, "INBOX");
    assert!(h
        .fake
        .calls()
        .iter()
        .all(|c| !c.starts_with("move ") && !c.starts_with("flag ")));
}

#[test]
fn lost_response_converges_without_a_second_move() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::ErrorAfter);
    h.sync();
    h.sync();
    h.sync();
    assert_eq!(
        h.fake
            .calls()
            .iter()
            .filter(|c| c.starts_with("move "))
            .count(),
        1
    );
    assert_eq!(place(&h, "n")[0].0, "Newsletters");
    assert!(h.service().store.intents("work", true).unwrap().is_empty());
}

#[test]
fn failed_dispatch_is_retried_once_backoff_allows() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::ErrorBefore);
    h.sync();
    assert_eq!(place(&h, "n")[0].0, "INBOX");
    // Make the backoff elapse: retries are allowed when next_after <= now.
    h.service()
        .store
        .expire_intent_backoff_for_tests("work")
        .unwrap();
    h.sync();
    h.sync();
    assert_eq!(place(&h, "n")[0].0, "Newsletters");
}

#[test]
fn partial_copy_blocks_with_duplicate_copy() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::PartialCopy);
    h.sync();
    h.sync();
    h.sync();
    let s = h.service();
    let blocked: Vec<_> = s
        .store
        .placements("work")
        .unwrap()
        .into_iter()
        .filter_map(|p| p.blocked_reason)
        .collect();
    assert_eq!(blocked, vec!["duplicate_copy".to_string()]);
    assert_eq!(
        h.fake
            .calls()
            .iter()
            .filter(|c| c.starts_with("move "))
            .count(),
        1
    );
}

#[test]
fn epoch_race_with_copyuid_is_reverted() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::EpochRaceBefore);
    let out = h.sync();
    assert_eq!(out["filing"]["reverted"], 1);
    assert_eq!(place(&h, "n")[0].0, "INBOX");
    let s = h.service();
    assert!(s
        .store
        .events("work", None, 50)
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "epoch_race_reverted"));
    assert!(s
        .store
        .events("work", None, 50)
        .unwrap()
        .iter()
        .all(|e| e["kind"] != "client_correction"));
}

#[test]
fn epoch_race_without_copyuid_pauses_the_source() {
    let h = Harness::new(Live);
    h.fake.set_capabilities(true, false, true);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::EpochRaceBefore);
    h.sync();
    h.sync();
    let f = h
        .service()
        .store
        .folder_record("work", "INBOX")
        .unwrap()
        .unwrap();
    assert_eq!(f.pause_reason.as_deref(), Some("epoch_race"));
    let moves = h
        .fake
        .calls()
        .iter()
        .filter(|c| c.starts_with("move "))
        .count();
    h.fake
        .deliver("INBOX", &mail("m", "Another newsletter", "newsletter"));
    h.sync();
    assert_eq!(
        h.fake
            .calls()
            .iter()
            .filter(|c| c.starts_with("move "))
            .count(),
        moves,
        "paused source is not written"
    );
}

#[test]
fn without_uidplus_identity_comes_from_the_fingerprint() {
    let h = Harness::new(Live);
    h.fake.set_capabilities(true, false, true);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.sync();
    h.sync();
    h.sync();
    let mut s = h.service();
    let p = &s.store.placements("work").unwrap()[0];
    assert_eq!(p.home_folder.as_deref(), Some("Newsletters"));
    assert_eq!(p.filed_by.as_deref(), Some("mailtriage"));
    assert!(s.store.intents("work", true).unwrap().is_empty());
    let all = s
        .list(
            "work",
            mailtriage::service::ListOptions {
                view: "all".into(),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(all["total"], 1, "the moved copy merged into the original");
}

#[test]
fn flag_is_attempted_once_and_never_retried() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("c", "Lunch", "Can you join us?"));
    h.fake.inject(FakeOp::Flag, Fault::ErrorBefore);
    h.sync();
    h.sync();
    h.sync();
    assert_eq!(
        h.fake
            .calls()
            .iter()
            .filter(|c| c.starts_with("flag "))
            .count(),
        1
    );
    let s = h.service();
    assert!(s
        .store
        .events("work", None, 50)
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "flag_failed"));
}

// Supplementary coverage: further rows of the intent-recovery table, flag and
// revert races, and the write gates.

fn intent_states(h: &Harness) -> Vec<(String, String)> {
    h.service()
        .store
        .intents("work", false)
        .unwrap()
        .into_iter()
        .map(|i| (i.kind, i.state))
        .collect()
}

fn move_calls(h: &Harness) -> usize {
    h.fake
        .calls()
        .iter()
        .filter(|c| c.starts_with("move "))
        .count()
}

fn pause_of(h: &Harness, folder: &str) -> Option<String> {
    h.service()
        .store
        .folder_record("work", folder)
        .unwrap()
        .unwrap()
        .pause_reason
}

#[test]
fn flag_epoch_race_fails_the_flag_and_pauses_the_folder() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("i", "Invoice", "Payment due today"));
    h.fake.inject(FakeOp::Flag, Fault::EpochRaceBefore);
    let out = h.sync();
    assert_eq!(out["filing"]["flagged"], 0);
    assert_eq!(out["partial"], true);
    assert_eq!(pause_of(&h, "INBOX").as_deref(), Some("epoch_race"));
    assert_eq!(
        intent_states(&h),
        vec![("flag".to_string(), "failed".to_string())]
    );
    assert_eq!(move_calls(&h), 0, "no move from a folder paused this pass");
    h.sync();
    assert_eq!(move_calls(&h), 0);
}

#[test]
fn failed_revert_pauses_the_source() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::EpochRaceBefore);
    // Consumed by the revert's own move.
    h.fake.inject(FakeOp::Move, Fault::ErrorBefore);
    let out = h.sync();
    assert_eq!(out["filing"]["reverted"], 0);
    assert_eq!(pause_of(&h, "INBOX").as_deref(), Some("epoch_race"));
    let s = h.service();
    let reverts = s.store.reverts("work", false).unwrap();
    assert_eq!(reverts.len(), 1);
    assert_eq!(reverts[0].state, "failed");
    assert_eq!(move_calls(&h), 2, "the race and its single revert attempt");
    h.sync();
    assert_eq!(move_calls(&h), 2, "a failed revert is never retried");
}

#[test]
fn uncertain_intent_superseded_by_a_newer_request_is_not_retried() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::ErrorBefore);
    h.sync();
    {
        let mut s = h.service();
        let mut p = s.store.placements("work").unwrap().remove(0);
        let rev = p.desired_rev;
        p.desired_target = Some("transactions".into());
        p.desired_rev += 1;
        assert!(s.store.save_placement(&p, Some(rev)).unwrap());
    }
    h.sync();
    h.sync();
    assert_eq!(place(&h, "n")[0].0, "Transactions");
    assert_eq!(
        intent_states(&h),
        vec![
            ("move".to_string(), "superseded".to_string()),
            ("move".to_string(), "applied".to_string())
        ]
    );
    let p = h.service().store.placements("work").unwrap().remove(0);
    assert_eq!(
        p.desired_target, None,
        "the newer request was consumed by its own move"
    );
}

#[test]
fn exhausted_retries_fail_with_move_failed() {
    let h = Harness::new(Live);
    h.edit(|c| c.policy.max_attempts = 1);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::ErrorBefore);
    h.fake.inject(FakeOp::Move, Fault::ErrorBefore);
    h.sync();
    h.sync(); // the one retry, failing again
    h.service()
        .store
        .expire_intent_backoff_for_tests("work")
        .unwrap();
    h.sync();
    assert_eq!(move_calls(&h), 2);
    assert_eq!(
        intent_states(&h),
        vec![("move".to_string(), "failed".to_string())]
    );
    let s = h.service();
    let p = s.store.placements("work").unwrap().remove(0);
    assert_eq!(p.blocked_reason.as_deref(), Some("move_failed"));
    assert!(s
        .store
        .events("work", None, 50)
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "move_failed"));
    h.sync();
    assert_eq!(move_calls(&h), 2, "a blocked message is not written");
}

#[test]
fn sent_move_whose_copy_vanished_becomes_lost() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.sync();
    let (folder, uid) = place(&h, "n")[0].clone();
    assert_eq!(folder, "Newsletters");
    h.fake.client_delete(&folder, uid);
    h.sync();
    assert_eq!(
        intent_states(&h),
        vec![("move".to_string(), "lost".to_string())]
    );
    let p = h.service().store.placements("work").unwrap().remove(0);
    assert_eq!(p.location_state, mailtriage::filing::LocationState::Absent);
}

#[test]
fn target_epoch_reset_after_a_lost_response_is_a_suspected_race_resolved_by_rescan() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::ErrorAfter);
    h.sync();
    h.fake.reset_epoch("Newsletters");
    h.sync();
    assert_eq!(
        pause_of(&h, "INBOX").as_deref(),
        Some("epoch_race_suspected")
    );
    assert_eq!(
        intent_states(&h),
        vec![("move".to_string(), "awaiting_rescan".to_string())]
    );
    h.sync();
    assert_eq!(
        intent_states(&h),
        vec![("move".to_string(), "applied".to_string())]
    );
    let p = h.service().store.placements("work").unwrap().remove(0);
    assert_eq!(p.home_folder.as_deref(), Some("Newsletters"));
    assert_eq!(p.home_epoch, Some(h.fake.epoch("Newsletters")));
    assert_eq!(move_calls(&h), 1);
}

#[test]
fn dry_run_recovery_observes_but_never_retries() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::ErrorBefore);
    h.sync();
    let writes = h.fake.write_calls();
    h.set_mode(DryRun);
    h.service()
        .store
        .expire_intent_backoff_for_tests("work")
        .unwrap();
    h.sync();
    h.sync();
    assert_eq!(h.fake.write_calls(), writes);
    assert_eq!(
        intent_states(&h),
        vec![("move".to_string(), "uncertain".to_string())]
    );
    h.set_mode(Live);
    h.sync();
    h.sync();
    assert_eq!(place(&h, "n")[0].0, "Newsletters");
}

#[test]
fn changed_configuration_stops_writes_for_the_pass() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    // Placed and classified without writes, so the next pass only plans and applies.
    h.set_mode(DryRun);
    h.sync();
    h.set_mode(Live);
    let mut s = h.service();
    // Rewrite the configuration: its bytes change, its meaning does not.
    let bytes = std::fs::read(&h.path).unwrap();
    std::fs::write(&h.path, [bytes.as_slice(), b"\n"].concat()).unwrap();
    let out = s.sync("work", 100).unwrap();
    assert_eq!(out["partial"], true);
    assert_eq!(out["filing"]["planned"], 1);
    assert!(out["filing"]["problems"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p == "config_changed"));
    assert_eq!(move_calls(&h), 0);
    assert!(h.service().store.intents("work", false).unwrap().is_empty());
    h.sync();
    assert_eq!(place(&h, "n")[0].0, "Newsletters");
}
