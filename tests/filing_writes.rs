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
    h.sync();
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
    let e = s.sync("work", 100).unwrap_err();
    let code = e
        .downcast_ref::<mailtriage::service::ServiceError>()
        .map(|e| e.code);
    assert_eq!(code, Some(5), "the existing require_unchanged error");
    assert_eq!(move_calls(&h), 0);
    assert!(h.service().store.intents("work", false).unwrap().is_empty());
    h.sync();
    assert_eq!(place(&h, "n")[0].0, "Newsletters");
}

fn placement_of(h: &Harness, subject: &str) -> mailtriage::filing::Placement {
    let s = h.service();
    let id = s
        .store
        .records("work")
        .unwrap()
        .into_iter()
        .find(|r| r.envelope["subject"] == subject)
        .unwrap()
        .id;
    s.store.placement("work", &id).unwrap().unwrap()
}

fn event_kinds(h: &Harness) -> Vec<String> {
    h.service()
        .store
        .events("work", None, 200)
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_string())
        .collect()
}

fn flag_calls(h: &Harness) -> usize {
    h.fake
        .calls()
        .iter()
        .filter(|c| c.starts_with("flag "))
        .count()
}

#[test]
fn crash_after_the_block_before_the_intent_closed_converges() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::PartialCopy);
    h.sync();
    // A crash right after the block was written, before the intent closed.
    {
        let mut s = h.service();
        let mut p = s.store.placements("work").unwrap().remove(0);
        let rev = p.desired_rev;
        p.blocked_reason = Some("duplicate_copy".into());
        assert!(s.store.save_placement(&p, Some(rev)).unwrap());
    }
    assert_eq!(
        intent_states(&h),
        vec![("move".to_string(), "uncertain".to_string())]
    );
    h.sync();
    h.sync();
    assert_eq!(
        intent_states(&h),
        vec![("move".to_string(), "failed".to_string())]
    );
    let p = h.service().store.placements("work").unwrap().remove(0);
    assert_eq!(p.blocked_reason.as_deref(), Some("duplicate_copy"));
    assert_eq!(move_calls(&h), 1, "never a second move");
}

#[test]
fn verification_drops_are_reported_and_existing_flags_consume_the_attempt() {
    use mailtriage::filing::{apply, inputs, observe, planner, FilingSummary, PassContext};
    let h = Harness::new(Live);
    h.sync();
    let n = h
        .fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    let c = h
        .fake
        .deliver("INBOX", &mail("c", "Lunch", "Can you join us?"));
    // Placed and classified without writes.
    h.set_mode(DryRun);
    h.sync();
    h.set_mode(Live);
    let n_id = placement_of(&h, "Weekly newsletter").message_id;
    let mut s = h.service();
    let cfg = s.config.accounts["work"].clone();
    let generation = s.store.records("work").unwrap()[0].generation.clone();
    let verify = || -> anyhow::Result<()> { Ok(()) };
    let ctx = PassContext {
        account: "work",
        cfg: &cfg,
        engine: &h.fake,
        mode: Live,
        generation: &generation,
        now: mailtriage::store::now(),
        max_attempts: 5,
        verify_binding: &verify,
    };
    let mut summary = FilingSummary::default();
    let map = observe::resolve_folders(&mut s.store, &ctx, &mut summary).unwrap();
    let plan = planner::plan(&inputs::plan_input(&s.store, &ctx, &map, false).unwrap());
    assert_eq!(plan.actions.len(), 2, "a flag for c, a move for n");
    // Between planning and applying, n leaves INBOX and the user flags c.
    h.fake.client_delete("INBOX", n);
    h.fake.client_set_flag("INBOX", c, "\\Flagged", true);
    let dropped = apply::apply(&mut s.store, &ctx, &map, &plan, &mut summary).unwrap();
    assert_eq!(dropped, vec![n_id]);
    assert_eq!(move_calls(&h) + flag_calls(&h), 0);
    let p = placement_of(&h, "Lunch");
    assert!(
        p.flag_attempted_at.is_some(),
        "the user's flag consumed the attempt"
    );
    assert!(s.store.intents("work", false).unwrap().is_empty());
}

#[test]
fn a_user_flag_satisfies_the_flag_attempt_and_a_later_unflag_is_kept() {
    let h = Harness::new(Live);
    h.sync();
    let c = h
        .fake
        .deliver("INBOX", &mail("c", "Lunch", "Can you join us?"));
    h.fake.client_set_flag("INBOX", c, "\\Flagged", true);
    h.sync();
    assert_eq!(flag_calls(&h), 0);
    assert!(placement_of(&h, "Lunch").flag_attempted_at.is_some());
    assert!(h.service().store.intents("work", false).unwrap().is_empty());
    h.fake.client_set_flag("INBOX", c, "\\Flagged", false);
    h.sync();
    h.sync();
    assert_eq!(flag_calls(&h), 0, "the unflag is never overridden");
    assert!(h.fake.flags("INBOX", c).is_empty());
}

/// Delivers a (then deleted), n and the backlog message b, so that an epoch
/// reset renumbers n to 1 and b to 2: the raced session moves b, not n.
fn deliver_race_victims(h: &Harness) {
    let a = h.fake.deliver("INBOX", &mail("a", "Hello", "hello there"));
    let n = h
        .fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    let b = h.fake.deliver_at(
        "INBOX",
        &mail("b", "Old newsletter", "newsletter"),
        "2026-01-01T00:00:00+00:00",
    );
    assert_eq!((a, n, b), (1, 2, 3));
    h.fake.client_delete("INBOX", a);
    h.fake.inject(FakeOp::Move, Fault::EpochRaceBefore);
}

fn assert_untouched(p: &mailtriage::filing::Placement) {
    assert_eq!(p.filed_at, None);
    assert_eq!(p.filed_by, None);
    assert_eq!(p.desired_target, None);
    assert_eq!(p.desired_rev, 0);
    assert_eq!(p.blocked_reason, None);
}

#[test]
fn epoch_race_that_moved_another_message_reverts_it_without_placement_changes() {
    let h = Harness::new(Live);
    h.sync();
    deliver_race_victims(&h);
    let out = h.sync();
    let calls = h.fake.calls();
    assert!(calls.contains(&"move INBOX 2 -> Newsletters".to_string()));
    assert!(
        calls.contains(&"move Newsletters 1 -> INBOX".to_string()),
        "the revert moved the raced message back"
    );
    assert_eq!(out["filing"]["reverted"], 1);
    assert_eq!(place(&h, "b")[0].0, "INBOX");
    assert_eq!(place(&h, "n")[0].0, "INBOX");
    let b = placement_of(&h, "Old newsletter");
    assert_untouched(&b);
    assert_eq!(b.home_uid, Some(3));
    assert_untouched(&placement_of(&h, "Weekly newsletter"));
    assert_eq!(
        intent_states(&h),
        vec![("move".to_string(), "awaiting_rescan".to_string())]
    );
    h.sync();
    h.sync();
    h.sync();
    assert_eq!(
        place(&h, "n")[0].0,
        "Newsletters",
        "n is filed by its retry"
    );
    assert_eq!(place(&h, "b")[0].0, "INBOX");
    assert_untouched(&placement_of(&h, "Old newsletter"));
    let kinds = event_kinds(&h);
    assert!(kinds.iter().all(|k| k != "client_correction"));
    assert_eq!(kinds.iter().filter(|k| *k == "moved").count(), 1);
}

#[test]
fn epoch_race_that_moved_another_message_without_uidplus_only_pauses() {
    let h = Harness::new(Live);
    h.fake.set_capabilities(true, false, true);
    h.sync();
    deliver_race_victims(&h);
    h.sync();
    assert_eq!(pause_of(&h, "INBOX").as_deref(), Some("epoch_race"));
    assert_eq!(move_calls(&h), 1, "no revert without COPYUID");
    assert!(h.service().store.reverts("work", false).unwrap().is_empty());
    assert_eq!(place(&h, "b")[0].0, "Newsletters");
    assert_eq!(place(&h, "n")[0].0, "INBOX");
    assert_untouched(&placement_of(&h, "Weekly newsletter"));
    h.sync();
    h.sync();
    assert_eq!(move_calls(&h), 1, "the paused source is not written");
    assert_untouched(&placement_of(&h, "Old newsletter"));
    assert_untouched(&placement_of(&h, "Weekly newsletter"));
    assert!(event_kinds(&h).iter().all(|k| k != "client_correction"));
}

#[test]
fn applied_move_with_a_stale_revision_does_not_consume_a_newer_request() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::ErrorAfter);
    h.sync();
    {
        let mut s = h.service();
        let mut p = s.store.placements("work").unwrap().remove(0);
        let rev = p.desired_rev;
        p.desired_target = Some("transactions".into());
        p.desired_rev += 1;
        assert!(s.store.save_placement(&p, Some(rev)).unwrap());
    }
    h.sync(); // the copy is not identified yet: sent
    h.sync(); // identified: the old intent applies; the newer request stays and is dispatched
    let p = h.service().store.placements("work").unwrap().remove(0);
    assert_eq!(p.home_folder.as_deref(), Some("Newsletters"));
    assert_eq!(p.filed_by.as_deref(), Some("mailtriage"));
    assert_eq!(p.desired_target.as_deref(), Some("transactions"));
    assert_eq!(p.desired_rev, 1);
    h.sync();
    assert_eq!(place(&h, "n")[0].0, "Transactions");
    let intents = h.service().store.intents("work", false).unwrap();
    let states: Vec<_> = intents
        .iter()
        .map(|i| (i.state.as_str(), i.desired_rev))
        .collect();
    assert_eq!(states, vec![("applied", 0), ("applied", 1)]);
    let p = h.service().store.placements("work").unwrap().remove(0);
    assert_eq!(p.desired_target, None, "consumed by its own move");
}

#[test]
fn claims_without_dispatch_are_recovered() {
    use mailtriage::filing::planner::{Action, Locator};
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("i", "Invoice", "Payment due today"));
    h.set_mode(DryRun);
    h.sync();
    h.set_mode(Live);
    {
        let mut s = h.service();
        let p = s.store.placements("work").unwrap().remove(0);
        let at = Locator {
            folder: "INBOX".into(),
            epoch: p.home_epoch.unwrap(),
            uid: p.home_uid.unwrap(),
        };
        let now = mailtriage::store::now();
        let flag = Action::Flag {
            message_id: p.message_id.clone(),
            at: at.clone(),
        };
        assert!(s
            .store
            .claim_flag("work", &flag, "crash", &now)
            .unwrap()
            .is_some());
        let mv = Action::Move {
            message_id: p.message_id.clone(),
            from: at,
            to: "Transactions".into(),
            desired_rev: p.desired_rev,
            consumes_eligible: false,
        };
        let epoch = h.fake.epoch("Transactions");
        assert!(s
            .store
            .claim_move("work", &mv, epoch, 1, "crash", &now)
            .unwrap()
            .is_some());
    }
    h.sync();
    assert_eq!(
        flag_calls(&h),
        0,
        "an interrupted flag claim is never dispatched later"
    );
    assert_eq!(move_calls(&h), 1, "the unsent move is retried once");
    assert_eq!(
        intent_states(&h),
        vec![
            ("flag".to_string(), "failed".to_string()),
            ("move".to_string(), "sent".to_string())
        ]
    );
    assert!(event_kinds(&h).iter().any(|k| k == "flag_failed"));
    h.sync();
    let (folder, uid) = place(&h, "i")[0].clone();
    assert_eq!(folder, "Transactions");
    assert!(h.fake.flags(&folder, uid).is_empty());
    assert_eq!(flag_calls(&h), 0);
}

#[test]
fn flag_with_a_lost_response_is_applied_by_observation() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("c", "Lunch", "Can you join us?"));
    h.fake.inject(FakeOp::Flag, Fault::ErrorAfter);
    h.sync();
    assert_eq!(
        intent_states(&h),
        vec![("flag".to_string(), "uncertain".to_string())]
    );
    h.sync();
    assert_eq!(
        intent_states(&h),
        vec![("flag".to_string(), "applied".to_string())]
    );
    assert!(placement_of(&h, "Lunch").flagged_at.is_some());
    assert!(event_kinds(&h).iter().any(|k| k == "flagged"));
    assert_eq!(flag_calls(&h), 1);
}

#[test]
fn suspected_flag_race_pauses_and_is_never_retried() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("c", "Lunch", "Can you join us?"));
    h.fake.inject(FakeOp::Flag, Fault::ErrorBefore);
    h.sync();
    h.fake.reset_epoch("INBOX");
    h.sync();
    assert_eq!(pause_of(&h, "INBOX").as_deref(), Some("epoch_race"));
    assert_eq!(
        intent_states(&h),
        vec![("flag".to_string(), "failed".to_string())]
    );
    h.sync();
    assert_eq!(flag_calls(&h), 1);
}

#[test]
fn awaiting_rescan_retries_when_only_the_source_holds_the_message() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::EpochRaceBefore);
    h.sync(); // race, reverted
    h.sync(); // INBOX rescan
    assert_eq!(
        intent_states(&h),
        vec![("move".to_string(), "awaiting_rescan".to_string())]
    );
    h.sync(); // in F only: retried
    h.sync(); // applied
    assert_eq!(place(&h, "n")[0].0, "Newsletters");
    let intent = h.service().store.intents("work", false).unwrap().remove(0);
    assert_eq!((intent.state.as_str(), intent.attempts), ("applied", 1));
    assert_eq!(move_calls(&h), 3, "race, revert, retry");
}

#[test]
fn awaiting_rescan_with_the_message_in_neither_end_is_lost() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::ErrorAfter);
    h.sync();
    let (folder, uid) = place(&h, "n")[0].clone();
    assert_eq!(folder, "Newsletters");
    h.fake.client_delete(&folder, uid);
    h.fake.reset_epoch("Newsletters");
    h.sync();
    assert_eq!(
        intent_states(&h),
        vec![("move".to_string(), "awaiting_rescan".to_string())]
    );
    h.sync();
    assert_eq!(
        intent_states(&h),
        vec![("move".to_string(), "lost".to_string())]
    );
    let p = h.service().store.placements("work").unwrap().remove(0);
    assert_eq!(p.location_state, mailtriage::filing::LocationState::Absent);
    assert_eq!(move_calls(&h), 1);
}

#[test]
fn noselect_writes_nothing_and_resolves_as_still_in_the_source() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::NoSelect);
    h.sync();
    assert_eq!(place(&h, "n")[0].0, "INBOX");
    let intent = h.service().store.intents("work", false).unwrap().remove(0);
    assert_eq!(
        (intent.state.as_str(), intent.error.as_deref()),
        ("uncertain", Some("not_selected"))
    );
    h.sync();
    assert_eq!(move_calls(&h), 2, "observed in F, then retried");
    assert_eq!(place(&h, "n")[0].0, "Newsletters");
}

#[test]
fn unrelated_arrivals_in_a_busy_target_do_not_hold_a_lost_move() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.sync();
    let (folder, uid) = place(&h, "n")[0].clone();
    assert_eq!(folder, "Newsletters");
    h.fake.client_delete(&folder, uid);
    h.fake
        .deliver(&folder, &mail("x", "Unrelated", "something else"));
    h.sync();
    assert_eq!(
        intent_states(&h),
        vec![("move".to_string(), "lost".to_string())]
    );
}

// Final review C1: a target folder whose first discovery failed.

fn review_state_of(h: &Harness, subject: &str) -> String {
    h.service()
        .store
        .records("work")
        .unwrap()
        .into_iter()
        .find(|r| r.envelope["subject"] == subject)
        .unwrap()
        .review_state
}

/// Mail filed while the target had no discovery checkpoint was never
/// discovered there, became `lost`, then was marked done while it sat in
/// the target. The move now waits for the target's checkpoint.
#[test]
fn a_failed_first_snapshot_of_the_target_never_loses_or_finishes_filed_mail() {
    use mailtriage::filing::LocationState::{Absent, Known};
    let h = Harness::new(DryRun);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.sync();
    h.set_mode(Live);
    // The pass that creates Newsletters cannot read it once.
    h.fake.fail_next_snapshot("Newsletters");
    for pass in 0..5 {
        h.sync();
        let p = placement_of(&h, "Weekly newsletter");
        assert_ne!(p.location_state, Absent, "pass {pass}");
        assert_eq!(
            review_state_of(&h, "Weekly newsletter"),
            "open",
            "pass {pass}"
        );
        let kinds = event_kinds(&h);
        assert!(
            !kinds.iter().any(|k| k == "lost" || k == "archived_done"),
            "pass {pass}: {kinds:?}"
        );
    }
    let p = placement_of(&h, "Weekly newsletter");
    assert_eq!(p.location_state, Known);
    assert_eq!(p.home_folder.as_deref(), Some("Newsletters"));
    assert_eq!(p.filed_by.as_deref(), Some("mailtriage"));
    assert_eq!(place(&h, "n").len(), 1, "exactly one copy");
    assert_eq!(place(&h, "n")[0].0, "Newsletters");
    assert_eq!(
        intent_states(&h),
        vec![("move".to_string(), "applied".to_string())]
    );
    assert_eq!(move_calls(&h), 1);
}

/// A move journaled into a folder discovery has not established yet (state
/// an older release could leave behind): the first watch starts below the
/// move's `target_uid_next`, so the moved message is found, never `lost`.
#[test]
fn a_move_into_a_folder_watched_later_is_discovered_and_applied() {
    use mailtriage::filing::{LocationState::Known, NewIntent};
    let h = Harness::new(DryRun);
    h.sync();
    let uid = h
        .fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.sync();
    h.set_mode(Live);
    h.fake.fail_next_snapshot("Newsletters");
    h.sync(); // creates Newsletters; its discovery fails once
    assert_eq!(place(&h, "n")[0].0, "INBOX", "no move without a checkpoint");
    let id = placement_of(&h, "Weekly newsletter").message_id;
    let target_epoch = h.fake.epoch("Newsletters");
    // The move ran (client_move stands in for it) before the target was watched.
    h.fake.client_move("INBOX", uid, "Newsletters");
    let mut s = h.service();
    let now = mailtriage::store::now();
    let intent = s
        .store
        .insert_intent(&NewIntent {
            account: "work",
            message_id: &id,
            kind: "move",
            folder: "INBOX",
            epoch: h.fake.epoch("INBOX"),
            uid,
            target: Some("Newsletters"),
            target_epoch: Some(target_epoch),
            target_uid_next: Some(1),
            desired_rev: placement_of(&h, "Weekly newsletter").desired_rev,
            consumes_eligible: false,
            batch: "legacy",
            state: "sent",
            now: &now,
        })
        .unwrap();
    s.store
        .remove_occurrence("work", "INBOX", h.fake.epoch("INBOX"), uid)
        .unwrap();
    drop(s);
    for pass in 0..4 {
        h.sync();
        let kinds = event_kinds(&h);
        assert!(
            !kinds.iter().any(|k| k == "lost" || k == "archived_done"),
            "pass {pass}: {kinds:?}"
        );
    }
    let s = h.service();
    assert_eq!(s.store.intent(intent).unwrap().unwrap().state, "applied");
    let p = placement_of(&h, "Weekly newsletter");
    assert_eq!(p.location_state, Known);
    assert_eq!(p.home_folder.as_deref(), Some("Newsletters"));
    assert_eq!(place(&h, "n").len(), 1);
}

/// Final review C1(d): a lost move is recorded in the audit log.
#[test]
fn a_lost_move_records_a_lost_event() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.sync();
    let (folder, uid) = place(&h, "n")[0].clone();
    h.fake.client_delete(&folder, uid);
    h.sync();
    let id = placement_of(&h, "Weekly newsletter").message_id;
    let intent = h.service().store.intents("work", false).unwrap().remove(0);
    let events = h.service().store.events("work", Some(&id), 50).unwrap();
    let lost: Vec<_> = events.iter().filter(|e| e["kind"] == "lost").collect();
    assert_eq!(lost.len(), 1, "{events:?}");
    assert_eq!(lost[0]["folder"], "Newsletters");
    assert_eq!(lost[0]["detail"]["intent_id"], intent.id);
}

/// Final review C1(b): a watch that started above a sent move's
/// `target_uid_next` (state a release without C1(c) could leave behind)
/// never covers the move, so the move is never declared lost and the message
/// is never marked done.
#[test]
fn a_watch_that_started_above_a_move_never_declares_it_lost() {
    use mailtriage::engine::MailEngine;
    use mailtriage::filing::{LocationState::Absent, NewIntent};
    let h = Harness::new(DryRun);
    h.sync();
    let uid = h
        .fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.sync();
    h.set_mode(Live);
    h.fake.fail_next_snapshot("Newsletters");
    h.sync(); // creates Newsletters; its discovery fails once
    let id = placement_of(&h, "Weekly newsletter").message_id;
    let target_epoch = h.fake.epoch("Newsletters");
    h.fake.client_move("INBOX", uid, "Newsletters");
    let mut s = h.service();
    // The first watch started at the tip, above the moved message.
    let snapshot = h.fake.snapshot("Newsletters").unwrap();
    s.store
        .checkpoint_start_at("work", "Newsletters", &snapshot, snapshot.uid_next - 1)
        .unwrap();
    let now = mailtriage::store::now();
    let intent = s
        .store
        .insert_intent(&NewIntent {
            account: "work",
            message_id: &id,
            kind: "move",
            folder: "INBOX",
            epoch: h.fake.epoch("INBOX"),
            uid,
            target: Some("Newsletters"),
            target_epoch: Some(target_epoch),
            target_uid_next: Some(1),
            desired_rev: placement_of(&h, "Weekly newsletter").desired_rev,
            consumes_eligible: false,
            batch: "legacy",
            state: "sent",
            now: &now,
        })
        .unwrap();
    s.store
        .remove_occurrence("work", "INBOX", h.fake.epoch("INBOX"), uid)
        .unwrap();
    drop(s);
    for pass in 0..4 {
        h.sync();
        let kinds = event_kinds(&h);
        assert!(
            !kinds.iter().any(|k| k == "lost" || k == "archived_done"),
            "pass {pass}: {kinds:?}"
        );
        assert_ne!(placement_of(&h, "Weekly newsletter").location_state, Absent);
        assert_eq!(review_state_of(&h, "Weekly newsletter"), "open");
    }
    let s = h.service();
    assert_eq!(s.store.intent(intent).unwrap().unwrap().state, "sent");
}

/// Final review I1: how `watch` tells a pass to skip from one that stops it.
/// A pass whose `mailtriage.json` or Himalaya TOML changed mid-pass fails as
/// a configuration change, and the next pass (a fresh open, as `watch`
/// does) continues; a changed account binding is not a configuration change.
#[test]
fn passes_after_a_configuration_change_mid_pass_continue() {
    use mailtriage::service::{is_config_change, ServiceError};
    let code = |e: &anyhow::Error| e.downcast_ref::<ServiceError>().map(|s| s.code);
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    // `mailtriage.json` changes after the pass opened it.
    let mut s = h.service();
    let bytes = std::fs::read(&h.path).unwrap();
    std::fs::write(&h.path, [bytes.as_slice(), b"\n"].concat()).unwrap();
    let e = s.sync("work", 100).unwrap_err();
    assert!(is_config_change(&e), "{e}");
    assert_eq!(code(&e), Some(5));
    // ... and while a classification result is pending.
    let mut s = h.service();
    std::fs::write(&h.path, &bytes).unwrap();
    let e = s
        .classify("work", &mail("c", "Lunch", "Can you join us?"), "rfc822")
        .unwrap_err();
    assert!(is_config_change(&e), "{e}");
    assert_eq!(code(&e), Some(5));
    // The Himalaya TOML changes mid-pass.
    h.fake.fail_with_config_changed("discover INBOX");
    let e = h.service().sync("work", 100).unwrap_err();
    assert!(is_config_change(&e), "{e}");
    assert_eq!(code(&e), Some(5));
    // The next pass opens the engine afresh and continues.
    h.fake.reload_config();
    h.sync();
    assert_eq!(place(&h, "n")[0].0, "Newsletters");
    // A changed account binding is no configuration change: `watch` stops.
    h.fake.set_binding("another mailbox");
    let e = h.service().sync("work", 100).unwrap_err();
    assert!(!is_config_change(&e), "{e}");
    assert_eq!(code(&e), Some(5));
}

/// Final review I2: `filing retry --id` on a `duplicate_copy` block is refused
/// while both copies are recorded; once the user removed one copy and a sync
/// observed it, the retry lifts the block without making the message
/// eligible once, and no second copy is ever created.
#[test]
fn retry_of_a_duplicate_copy_waits_for_one_copy_to_be_removed() {
    use mailtriage::service::{RetryTarget, ServiceError};
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::PartialCopy);
    h.sync();
    h.sync();
    let p = placement_of(&h, "Weekly newsletter");
    assert_eq!(p.blocked_reason.as_deref(), Some("duplicate_copy"));
    assert_eq!(place(&h, "n").len(), 2);
    let retry = |h: &Harness| {
        h.service()
            .filing_retry("work", RetryTarget::Message(p.message_id.clone()))
    };
    let e = retry(&h).unwrap_err();
    let e = e.downcast_ref::<ServiceError>().unwrap();
    assert_eq!(
        (e.code, e.message.as_str()),
        (5, "remove one copy first, then sync and retry")
    );
    let after = placement_of(&h, "Weekly newsletter");
    assert_eq!(after.blocked_reason.as_deref(), Some("duplicate_copy"));
    assert_eq!(after.desired_rev, p.desired_rev, "nothing written");
    h.sync();
    assert_eq!(place(&h, "n").len(), 2, "still blocked");
    // The user deletes the copy left in the source folder; a sync observes it.
    let inbox = place(&h, "n")
        .into_iter()
        .find(|(f, _)| f == "INBOX")
        .unwrap();
    h.fake.client_delete(&inbox.0, inbox.1);
    h.sync();
    retry(&h).unwrap();
    let p = placement_of(&h, "Weekly newsletter");
    assert_eq!(p.blocked_reason, None);
    assert!(
        !p.eligible_once,
        "lifting duplicate_copy grants no eligibility"
    );
    let moves = move_calls(&h);
    for _ in 0..3 {
        h.sync();
        assert_eq!(place(&h, "n").len(), 1, "never a second copy");
    }
    assert_eq!(place(&h, "n")[0].0, "Newsletters");
    assert_eq!(move_calls(&h), moves, "the remaining copy is already filed");
}

/// Final review I2, the other copy: with the category folder's copy removed,
/// the lifted message is still new mail in the source folder and is filed
/// once more, as its only copy.
#[test]
fn retry_after_removing_the_filed_copy_files_the_remaining_one_once() {
    use mailtriage::service::RetryTarget;
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::PartialCopy);
    h.sync();
    h.sync();
    let id = placement_of(&h, "Weekly newsletter").message_id;
    let filed = place(&h, "n")
        .into_iter()
        .find(|(f, _)| f == "Newsletters")
        .unwrap();
    h.fake.client_delete(&filed.0, filed.1);
    h.sync();
    h.service()
        .filing_retry("work", RetryTarget::Message(id))
        .unwrap();
    for _ in 0..3 {
        h.sync();
        assert_eq!(place(&h, "n").len(), 1, "never a second copy");
    }
    assert_eq!(place(&h, "n")[0].0, "Newsletters");
    assert_eq!(placement_of(&h, "Weekly newsletter").blocked_reason, None);
}
