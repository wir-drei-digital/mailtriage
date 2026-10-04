mod common;
use common::{mail, Harness};
use mailtriage::{
    domain::FilingMode::Live,
    engine::fake::{FakeOp, Fault},
    service::ListOptions,
};
use serde_json::json;

fn filed(h: &Harness, mid: &str, subject: &str, body: &str) -> (String, String) {
    h.fake.deliver("INBOX", &mail(mid, subject, body));
    h.sync();
    h.sync();
    let mut s = h.service();
    let all = s
        .list(
            "work",
            ListOptions {
                view: "all".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let id = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["subject"] == subject)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let p = s.store.placement("work", &id).unwrap().unwrap();
    (id, p.home_folder.unwrap())
}
fn at(h: &Harness, mid: &str) -> Vec<(String, u64)> {
    h.fake.locate(&format!("<{mid}@test>"))
}
fn item(h: &Harness, id: &str) -> serde_json::Value {
    h.service().read("work", id).unwrap()["item"].clone()
}

#[test]
fn client_move_to_another_category_is_a_correction() {
    let h = Harness::new(Live);
    h.sync();
    let (id, home) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    assert_eq!(home, "Newsletters");
    let (f, uid) = at(&h, "n")[0].clone();
    h.fake.client_move(&f, uid, "Updates");
    h.sync();
    h.sync();
    let it = item(&h, &id);
    assert_eq!(it["classification"]["category_id"], "updates");
    assert_eq!(it["placement"]["filed_by"], "user");
    assert_eq!(
        at(&h, "n")[0].0,
        "Updates",
        "mailtriage does not move it back"
    );
}

#[test]
fn client_move_back_to_inbox_pins() {
    let h = Harness::new(Live);
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    let (f, uid) = at(&h, "n")[0].clone();
    h.fake.client_move(&f, uid, "INBOX");
    h.sync();
    h.sync();
    h.sync();
    assert_eq!(at(&h, "n")[0].0, "INBOX");
    assert_eq!(item(&h, &id)["placement"]["pinned"], true);
}

#[test]
fn archive_marks_done_and_return_reopens() {
    let h = Harness::new(Live);
    h.fake.add_folder("Archive", &["\\Archive"]);
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    let (f, uid) = at(&h, "n")[0].clone();
    let archived = h.fake.client_move(&f, uid, "Archive");
    for _ in 0..3 {
        h.sync();
    }
    assert_eq!(item(&h, &id)["review_state"], "done");
    h.fake.client_move("Archive", archived, "Newsletters");
    h.sync();
    h.sync();
    let it = item(&h, &id);
    assert_eq!(it["review_state"], "open");
    // A move into its own category only relocates (spec "Arrival resolution").
    assert_eq!(it["overrides"], json!({}));
    assert_eq!(it["placement"]["folder"], "Newsletters");
    let kinds = event_kinds(&h);
    assert!(!kinds.iter().any(|k| k == "client_correction"));
    assert!(kinds.iter().any(|k| k == "relocated"));
    assert!(kinds.iter().any(|k| k == "reopened"));
}

fn event_kinds(h: &Harness) -> Vec<String> {
    h.service()
        .store
        .events("work", None, 500)
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn explicit_done_is_never_reopened_by_observation() {
    let h = Harness::new(Live);
    h.fake.add_folder("Archive", &["\\Archive"]);
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    h.service().review("work", &id, true).unwrap();
    let (f, uid) = at(&h, "n")[0].clone();
    let archived = h.fake.client_move(&f, uid, "Archive");
    for _ in 0..3 {
        h.sync();
    }
    h.fake.client_move("Archive", archived, "Newsletters");
    h.sync();
    h.sync();
    assert_eq!(item(&h, &id)["review_state"], "done");
}

#[test]
fn second_label_is_an_extra_occurrence() {
    let h = Harness::new(Live);
    h.sync();
    let (id, home) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    let (f, uid) = at(&h, "n")[0].clone();
    h.fake.client_copy(&f, uid, "Updates");
    h.sync();
    h.sync();
    let it = item(&h, &id);
    assert_eq!(it["classification"]["category_id"], "newsletters");
    assert_eq!(it["placement"]["folder"], home);
}

#[test]
fn copy_then_delete_relocates_with_correction() {
    let h = Harness::new(Live);
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    let (f, uid) = at(&h, "n")[0].clone();
    h.fake.client_copy(&f, uid, "Updates");
    h.sync();
    h.fake.client_delete(&f, uid);
    for _ in 0..3 {
        h.sync();
    }
    let it = item(&h, &id);
    assert_eq!(it["classification"]["category_id"], "updates");
    assert_eq!(it["placement"]["folder"], "Updates");
}

#[test]
fn several_survivors_are_ambiguous_until_pinned() {
    let h = Harness::new(Live);
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    let (f, uid) = at(&h, "n")[0].clone();
    h.fake.client_copy(&f, uid, "Updates");
    h.fake.client_copy(&f, uid, "Promotions");
    h.sync();
    h.fake.client_delete(&f, uid);
    for _ in 0..3 {
        h.sync();
    }
    assert_eq!(item(&h, &id)["placement"]["location_state"], "ambiguous");
    h.service().filing_pin("work", &id).unwrap();
    h.sync();
    h.sync();
    assert!(at(&h, "n").iter().any(|(f, _)| f == "INBOX"));
}

#[test]
fn cli_correction_moves_and_a_newer_request_survives_an_older_intent() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::ErrorAfter);
    h.sync(); // moved to Newsletters, response lost
    let mut s = h.service();
    let all = s
        .list(
            "work",
            ListOptions {
                view: "all".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let id = all["items"][0]["id"].as_str().unwrap().to_string();
    h.service()
        .correct("work", &id, json!({"category_id": "transactions"}), None)
        .unwrap();
    for _ in 0..4 {
        h.sync();
    }
    assert_eq!(at(&h, "n")[0].0, "Transactions");
    assert_eq!(
        item(&h, &id)["classification"]["category_id"],
        "transactions"
    );
}

#[test]
fn pin_supersedes_an_unsent_move() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::ErrorBefore);
    h.sync();
    let mut s = h.service();
    let id = s
        .list(
            "work",
            ListOptions {
                view: "all".into(),
                ..Default::default()
            },
        )
        .unwrap()["items"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    h.service().filing_pin("work", &id).unwrap();
    h.service()
        .store
        .expire_intent_backoff_for_tests("work")
        .unwrap();
    h.sync();
    h.sync();
    assert_eq!(at(&h, "n")[0].0, "INBOX");
    let s = h.service();
    assert!(s
        .store
        .intents("work", false)
        .unwrap()
        .iter()
        .any(|i| i.state == "superseded"));
    assert!(s
        .store
        .placement("work", &id)
        .unwrap()
        .unwrap()
        .blocked_reason
        .is_none());
}

#[test]
fn mail_delivered_into_a_category_folder_is_user_filed() {
    let h = Harness::new(Live);
    h.sync();
    h.fake.deliver("Updates", &mail("u", "Hello", "Plain mail"));
    h.sync();
    h.sync();
    let mut s = h.service();
    let all = s
        .list(
            "work",
            ListOptions {
                view: "all".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let it = &all["items"][0];
    assert_eq!(it["classification"]["category_id"], "updates");
    assert_eq!(it["placement"]["filed_by"], "user");
    assert_eq!(at(&h, "u")[0].0, "Updates");
}

#[test]
fn category_folder_epoch_reset_reattaches_without_ingesting_or_done() {
    let h = Harness::new(Live);
    h.fake.add_folder("Newsletters", &[]);
    h.fake
        .deliver("Newsletters", &mail("pre", "Pre-existing", "newsletter"));
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    h.fake.reset_epoch("Newsletters");
    for _ in 0..4 {
        h.sync();
    }
    let mut s = h.service();
    let all = s
        .list(
            "work",
            ListOptions {
                view: "all".into(),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(all["total"], 1, "pre-existing content stays out");
    let it = item(&h, &id);
    assert_eq!(it["review_state"], "open");
    assert_eq!(it["placement"]["location_state"], "known");
    assert_eq!(it["placement"]["folder"], "Newsletters");
}

#[test]
fn merge_conflict_blocks_instead_of_losing_edits() {
    let h = Harness::new(Live);
    h.sync();
    let uid = h.fake.deliver("INBOX", &mail("d", "Hello", "Plain mail"));
    h.fake.client_copy("INBOX", uid, "Updates");
    let mut s = h.service();
    s.sync("work", 1).unwrap(); // processes one job; the Updates copy stays provisional
    let all = s
        .list(
            "work",
            ListOptions {
                view: "all".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let provisional = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["classification"]["state"] == "pending")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    s.review("work", &provisional, true).unwrap();
    for _ in 0..3 {
        h.sync();
    }
    let s = h.service();
    assert!(s
        .store
        .events("work", None, 100)
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "merge_conflict"));
    assert!(s
        .store
        .placements("work")
        .unwrap()
        .iter()
        .any(|p| p.blocked_reason.as_deref() == Some("merge_conflict")));
}

// Supplementary coverage: rescan pruning, the watch point, re-evaluation of
// homes found gone, done-inference barriers and the lifting commands.

use mailtriage::filing::{transitions, LocationState};

fn folder(h: &Harness, native: &str) -> mailtriage::filing::FolderRecord {
    h.service()
        .store
        .folder_record("work", native)
        .unwrap()
        .unwrap()
}

#[test]
fn a_completed_rescan_prunes_its_rescan_sets() {
    let h = Harness::new(Live);
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    h.fake.reset_epoch("Newsletters");
    h.sync();
    let f = folder(&h, "Newsletters");
    assert!(f.rescan_complete);
    assert_eq!(
        f.watch_from_uid, None,
        "an epoch reset drops the watch point"
    );
    assert!(h.service().store.rescan_folders("work").unwrap().is_empty());
    assert_eq!(item(&h, &id)["placement"]["folder"], "Newsletters");
}

#[test]
fn an_adopted_category_folder_is_reconciled_from_its_watch_point() {
    let h = Harness::new(Live);
    h.fake.add_folder("Newsletters", &[]);
    for i in 0..3 {
        h.fake.deliver(
            "Newsletters",
            &mail(&format!("old{i}"), "Old newsletter", "newsletter"),
        );
    }
    h.sync();
    h.fake
        .deliver("Newsletters", &mail("u", "Moved by the user", "hello"));
    h.sync();
    h.sync();
    assert_eq!(folder(&h, "Newsletters").watch_from_uid, Some(3));
    let below_watch_point: Vec<String> = h
        .fake
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("discover Newsletters 0.."))
        .collect();
    assert!(below_watch_point.is_empty(), "{below_watch_point:?}");
}

#[test]
fn client_move_during_a_home_folder_epoch_reset_is_corrected_after_the_rescan() {
    let h = Harness::new(Live);
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    let (f, uid) = at(&h, "n")[0].clone();
    h.fake.client_move(&f, uid, "Updates");
    h.fake.reset_epoch("Newsletters");
    for _ in 0..3 {
        h.sync();
    }
    let it = item(&h, &id);
    assert_eq!(it["classification"]["category_id"], "updates");
    assert_eq!(it["placement"]["folder"], "Updates");
    assert_eq!(it["placement"]["filed_by"], "user");
    let s = h.service();
    assert!(s
        .store
        .arrivals("work", Some("pending"))
        .unwrap()
        .is_empty());
}

#[test]
fn a_home_found_gone_on_the_server_is_reevaluated() {
    use mailtriage::domain::FilingMode::DryRun;
    use mailtriage::filing::{arrivals, observe, FilingSummary, PassContext};
    let h = Harness::new(DryRun);
    h.sync();
    let uid = h
        .fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.sync();
    let mut s = h.service();
    let id = s.store.placements("work").unwrap()[0].message_id.clone();
    // Gone on the server, still recorded locally (as after a verification drop).
    h.fake.client_delete("INBOX", uid);
    let cfg = s.config.accounts["work"].clone();
    let generation = s.store.records("work").unwrap()[0].generation.clone();
    let verify = || -> anyhow::Result<()> { Ok(()) };
    let ctx = PassContext {
        account: "work",
        cfg: &cfg,
        engine: &h.fake,
        mode: DryRun,
        generation: &generation,
        now: mailtriage::store::now(),
        max_attempts: 5,
        verify_binding: &verify,
    };
    let mut summary = FilingSummary::default();
    let map = observe::resolve_folders(&mut s.store, &ctx, &mut summary).unwrap();
    let ids = [id.clone(), id.clone()];
    arrivals::reevaluate(&mut s.store, &ctx, &map, &ids, &mut summary).unwrap();
    let p = s.store.placement("work", &id).unwrap().unwrap();
    assert_eq!(p.location_state, LocationState::Absent);
    assert!(s.store.occurrences_of("work", &id).unwrap().is_empty());
    assert_eq!(summary.errors, 0);
}

#[test]
fn a_paused_folder_holds_done_inference_until_released() {
    let h = Harness::new(Live);
    h.fake.add_folder("Archive", &["\\Archive"]);
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    {
        let mut s = h.service();
        let mut rec = s
            .store
            .folder_record("work", "Promotions")
            .unwrap()
            .unwrap();
        rec.pause_reason = Some("epoch_race".into());
        s.store.save_folder(&rec).unwrap();
    }
    let (f, uid) = at(&h, "n")[0].clone();
    h.fake.client_move(&f, uid, "Archive");
    for _ in 0..3 {
        h.sync();
    }
    assert_eq!(item(&h, &id)["review_state"], "open");
    assert_eq!(item(&h, &id)["placement"]["location_state"], "absent");
    {
        let mut s = h.service();
        let now = mailtriage::store::now();
        transitions::release_folder(&mut s.store, "work", "Promotions", &now).unwrap();
        assert!(s
            .store
            .events("work", None, 100)
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "released"));
    }
    assert!(folder(&h, "Promotions").pause_reason.is_none());
    for _ in 0..2 {
        h.sync();
    }
    assert_eq!(item(&h, &id)["review_state"], "done");
    let s = h.service();
    let p = s.store.placement("work", &id).unwrap().unwrap();
    assert!(p.done_inferred);
}

#[test]
fn a_merge_conflict_is_lifted_by_retrying_its_arrival() {
    let h = Harness::new(Live);
    h.sync();
    let uid = h.fake.deliver("INBOX", &mail("d", "Hello", "Plain mail"));
    h.fake.client_copy("INBOX", uid, "Updates");
    let mut s = h.service();
    s.sync("work", 1).unwrap();
    let all = s
        .list(
            "work",
            ListOptions {
                view: "all".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let provisional = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["classification"]["state"] == "pending")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    s.review("work", &provisional, true).unwrap();
    h.sync();
    let mut s = h.service();
    let arrival = s.store.arrivals("work", Some("unresolved")).unwrap()[0].id;
    let now = mailtriage::store::now();
    // The user clears the provisional's edit, then retries the arrival.
    s.review("work", &provisional, false).unwrap();
    let generation = s.store.records("work").unwrap()[0].generation.clone();
    transitions::retry_arrival(&mut s.store, "work", arrival, &generation, &now).unwrap();
    assert!(
        transitions::retry_arrival(&mut s.store, "work", arrival, &generation, &now).is_err(),
        "only an unresolved arrival is retried"
    );
    assert!(s
        .store
        .placements("work")
        .unwrap()
        .iter()
        .all(|p| p.blocked_reason.is_none()));
    h.sync();
    h.sync();
    let mut s = h.service();
    let all = s
        .list(
            "work",
            ListOptions {
                view: "all".into(),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(all["total"], 1, "the copy merged into the original");
    let a = s.store.arrival("work", arrival).unwrap().unwrap();
    assert_eq!(
        (a.state.as_str(), a.kind.as_deref()),
        ("resolved", Some("extra"))
    );
}

#[test]
fn an_unverified_folder_is_adopted_by_command() {
    let h = Harness::new(Live);
    h.fake.set_capabilities(true, true, false);
    h.fake.add_folder("Updates", &[]);
    h.sync();
    assert_eq!(folder(&h, "Updates").state, "needs_confirmation");
    let mut s = h.service();
    let now = mailtriage::store::now();
    transitions::adopt_folder(&mut s.store, "work", "Updates", &now).unwrap();
    assert!(transitions::adopt_folder(&mut s.store, "work", "Nope", &now).is_err());
    h.sync();
    let f = folder(&h, "Updates");
    assert_eq!(
        (f.state.as_str(), f.origin.as_deref()),
        ("ok", Some("adopted"))
    );
}

#[test]
fn filing_commands_on_unplaced_messages_fail_with_the_spec_codes() {
    use mailtriage::service::ServiceError;
    let h = Harness::new(Live);
    h.sync();
    h.fake.deliver("INBOX", &mail("a", "First", "first"));
    h.fake.deliver("Updates", &mail("u", "Hello", "Plain mail"));
    let mut s = h.service();
    s.sync("work", 1).unwrap(); // fetches the INBOX message only
    let code = |e: anyhow::Error| e.downcast_ref::<ServiceError>().map(|s| s.code);
    let all = s
        .list(
            "work",
            ListOptions {
                view: "all".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let provisional = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["classification"]["state"] == "pending")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let e = h.service().filing_pin("work", &provisional).unwrap_err();
    assert_eq!(code(e), Some(5), "identity not yet established");
    let raw = mail("x", "Offline", "offline");
    let id = s.classify("work", &raw, "rfc822").unwrap()["item"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let e = h.service().filing_pin("work", &id).unwrap_err();
    assert_eq!(
        code(e),
        Some(2),
        "fingerprinted but never seen in a mailbox"
    );
    assert_eq!(
        h.service().read("work", &id).unwrap()["item"]["placement"],
        serde_json::Value::Null
    );
}

// Fix round 1: frozen quarantine windows, running rescans, crash safety of
// rescan arrivals and inferred Done, and done-inference barriers.

use mailtriage::filing::{FilingWrite, NewIntent};

fn now() -> String {
    mailtriage::store::now()
}

fn pause(h: &Harness, native: &str) {
    let mut s = h.service();
    let mut rec = s.store.folder_record("work", native).unwrap().unwrap();
    rec.pause_reason = Some("epoch_race".into());
    s.store.save_folder(&rec).unwrap();
}

fn id_of(h: &Harness, mid: &str) -> String {
    let rfc = format!("<{mid}@test>");
    let s = h.service();
    s.store
        .records("work")
        .unwrap()
        .into_iter()
        .find(|r| r.envelope["message_id"] == rfc.as_str())
        .unwrap()
        .id
}

fn arrival_of(h: &Harness, folder: &str, mid: &str) -> mailtriage::filing::Arrival {
    let rfc = format!("<{mid}@test>");
    h.service()
        .store
        .arrivals("work", None)
        .unwrap()
        .into_iter()
        .rfind(|a| a.folder == folder && a.rfc_message_id.as_deref() == Some(rfc.as_str()))
        .unwrap()
}

fn review_state(h: &Harness, id: &str) -> String {
    item(h, id)["review_state"].as_str().unwrap().to_string()
}

/// Archives a filed message; returns its UID in `Archive`.
fn archive(h: &Harness, mid: &str) -> u64 {
    let (f, uid) = at(h, mid)[0].clone();
    h.fake.client_move(&f, uid, "Archive")
}

/// m's move is claimed with an unknown outcome; then x lands in its target
/// and the source is recreated, so the next pass suspects a race. x stays
/// unfetched in that pass (fetch limit 1).
fn suspected_race_with_target_arrival(h: &Harness) {
    h.fake
        .deliver("INBOX", &mail("m", "Another newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::ErrorBefore);
    h.sync();
    let x = h
        .fake
        .deliver("Newsletters", &mail("x", "Invoice", "Payment due"));
    h.fake.reset_epoch("INBOX");
    h.service().sync("work", 1).unwrap();
    assert_eq!(
        folder(h, "INBOX").pause_reason.as_deref(),
        Some("epoch_race_suspected")
    );
    let m = h.service().store.intents("work", true).unwrap().remove(0);
    assert_eq!(
        (m.target_uid_next, m.race_until_uid),
        (Some(x), Some(x + 1)),
        "the window ends at the target's UIDNEXT seen right after the race"
    );
}

#[test]
fn raced_arrivals_stay_quarantined_after_the_pause_is_released() {
    let h = Harness::new(Live);
    h.sync();
    suspected_race_with_target_arrival(&h);
    let mut s = h.service();
    transitions::release_folder(&mut s.store, "work", "INBOX", &now()).unwrap();
    for _ in 0..3 {
        h.sync();
    }
    let a = arrival_of(&h, "Newsletters", "x");
    assert_eq!(
        (a.state.as_str(), a.kind.as_deref()),
        ("resolved", Some("quarantined"))
    );
    let s = h.service();
    let p = s.store.placement("work", &a.message_id).unwrap().unwrap();
    assert_eq!(p.blocked_reason.as_deref(), Some("quarantined"));
    assert!(!event_kinds(&h).iter().any(|k| k == "client_correction"));
}

#[test]
fn an_identified_quarantined_arrival_does_not_hold_done_inference() {
    let h = Harness::new(Live);
    h.fake.add_folder("Archive", &["\\Archive"]);
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    suspected_race_with_target_arrival(&h);
    let mut s = h.service();
    transitions::release_folder(&mut s.store, "work", "INBOX", &now()).unwrap();
    for _ in 0..3 {
        h.sync();
    }
    let x = arrival_of(&h, "Newsletters", "x");
    assert_eq!(x.kind.as_deref(), Some("quarantined"));
    archive(&h, "n");
    for _ in 0..3 {
        h.sync();
    }
    assert_eq!(review_state(&h, &id), "done");
    let s = h.service();
    let still = s
        .store
        .occurrence_at("work", &x.folder, x.epoch, x.uid)
        .unwrap();
    assert!(still.is_some(), "the quarantined occurrence is still there");
}

#[test]
fn a_retried_raced_arrival_is_not_a_correction() {
    let h = Harness::new(Live);
    h.fake.set_capabilities(true, false, true);
    h.sync();
    let a = h.fake.deliver("INBOX", &mail("a", "Hello", "hello there"));
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.deliver_at(
        "INBOX",
        &mail("b", "Invoice", "Payment received"),
        "2026-01-01T00:00:00+00:00",
    );
    h.fake.client_delete("INBOX", a);
    h.fake.inject(FakeOp::Move, Fault::EpochRaceBefore);
    h.sync(); // the race moves b, not n, into Newsletters; INBOX pauses
    assert_eq!(at(&h, "b")[0].0, "Newsletters");
    let mut s = h.service();
    s.sync("work", 1).unwrap(); // b's copy in Newsletters stays provisional
    let all = s
        .list(
            "work",
            ListOptions {
                view: "all".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let provisional = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["classification"]["state"] == "pending")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    s.review("work", &provisional, true).unwrap();
    h.sync(); // merge refused: the raced arrival is unresolved
    let raced = arrival_of(&h, "Newsletters", "b");
    assert_eq!(raced.state, "unresolved");
    let mut s = h.service();
    transitions::release_folder(&mut s.store, "work", "INBOX", &now()).unwrap();
    for _ in 0..2 {
        h.sync();
    }
    // n's retry would rewrite the window's bounds: it waits for the window.
    let intent = h.service().store.intents("work", true).unwrap().remove(0);
    assert_eq!(
        (intent.state.as_str(), intent.error.as_deref()),
        ("awaiting_rescan", Some("epoch_race"))
    );
    let mut s = h.service();
    s.review("work", &provisional, false).unwrap();
    let generation = s.store.records("work").unwrap()[0].generation.clone();
    transitions::retry_arrival(&mut s.store, "work", raced.id, &generation, &now()).unwrap();
    for _ in 0..3 {
        h.sync();
    }
    let raced = h
        .service()
        .store
        .arrival("work", raced.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        (raced.state.as_str(), raced.kind.as_deref()),
        ("resolved", Some("quarantined"))
    );
    let b = id_of(&h, "b");
    let it = item(&h, &b);
    assert_eq!(it["overrides"], json!({}));
    assert_eq!(it["classification"]["category_id"], "transactions");
    assert!(!event_kinds(&h).iter().any(|k| k == "client_correction"));
    for _ in 0..2 {
        h.sync();
    }
    assert_eq!(
        at(&h, "n")[0].0,
        "Newsletters",
        "the retry ran once settled"
    );
}

#[test]
fn releasing_a_pause_keeps_a_running_reset_rescan() {
    let h = Harness::new(Live);
    h.fake.add_folder("Newsletters", &[]);
    for i in 0..2 {
        h.fake.deliver(
            "Newsletters",
            &mail(&format!("pre{i}"), "Old newsletter", "newsletter"),
        );
    }
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    pause(&h, "Newsletters");
    h.fake.reset_epoch("Newsletters");
    h.service().sync("work", 1).unwrap(); // the rescan has only reached UID 1
    let before = folder(&h, "Newsletters");
    assert!(!before.rescan_complete);
    let mut s = h.service();
    transitions::release_folder(&mut s.store, "work", "Newsletters", &now()).unwrap();
    let after = folder(&h, "Newsletters");
    assert_eq!(after.pause_reason, None);
    assert_eq!(
        (
            after.rescan_epoch,
            after.rescan_below_uid,
            after.rescan_complete
        ),
        (before.rescan_epoch, before.rescan_below_uid, false)
    );
    for _ in 0..3 {
        h.sync();
    }
    let mut s = h.service();
    let all = s
        .list(
            "work",
            ListOptions {
                view: "all".into(),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(all["total"], 1, "pre-existing content stays out");
    assert_eq!(item(&h, &id)["placement"]["folder"], "Newsletters");
    assert!(folder(&h, "Newsletters").rescan_complete);
}

#[test]
fn a_message_refound_in_its_home_folders_new_epoch_updates_its_home() {
    use mailtriage::domain::FilingMode::DryRun;
    let h = Harness::new(DryRun);
    h.fake.add_folder("Archive", &["\\Archive"]);
    h.sync();
    h.fake.deliver("INBOX", &mail("m1", "First", "first"));
    let n = h
        .fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.deliver("INBOX", &mail("m2", "Second", "second"));
    h.sync();
    let id = id_of(&h, "n");
    let archived = h.fake.client_move("INBOX", n, "Archive");
    h.fake.reset_epoch("INBOX");
    h.service().sync("work", 1).unwrap(); // the INBOX rescan is still running
    let back = h.fake.client_move("Archive", archived, "INBOX");
    h.sync();
    let p = h.service().store.placement("work", &id).unwrap().unwrap();
    assert_eq!(
        (
            p.location_state,
            p.home_folder.as_deref(),
            p.home_epoch,
            p.home_uid
        ),
        (
            LocationState::Known,
            Some("INBOX"),
            Some(h.fake.epoch("INBOX")),
            Some(back)
        )
    );
    assert!(!p.pinned, "a location update, not a pin");
}

#[test]
fn an_interrupted_rescan_arrival_is_not_a_correction() {
    let h = Harness::new(Live);
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    h.fake.reset_epoch("Newsletters");
    h.sync();
    {
        // A crash after the placement was (re)created at the rescan
        // occurrence, before its arrival was resolved.
        let mut s = h.service();
        let a = arrival_of(&h, "Newsletters", "n");
        assert_eq!(a.kind.as_deref(), Some("rescan"));
        s.store
            .resolve_arrival(a.id, "pending", None, &now())
            .unwrap();
        let mut p = s.store.placement("work", &id).unwrap().unwrap();
        let rev = p.desired_rev;
        p.filed_by = None;
        p.filed_at = None;
        assert!(s.store.save_placement(&p, Some(rev)).unwrap());
    }
    h.sync();
    let a = arrival_of(&h, "Newsletters", "n");
    assert_eq!(
        (a.state.as_str(), a.kind.as_deref()),
        ("resolved", Some("rescan"))
    );
    assert_eq!(item(&h, &id)["overrides"], json!({}));
    assert!(!event_kinds(&h).iter().any(|k| k == "client_correction"));
}

#[test]
fn an_extra_lost_while_the_home_folder_rescans_is_not_a_correction() {
    let h = Harness::new(Live);
    h.fake.add_folder("Newsletters", &[]);
    for i in 0..2 {
        h.fake.deliver(
            "Newsletters",
            &mail(&format!("pre{i}"), "Old newsletter", "newsletter"),
        );
    }
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    let (f, uid) = at(&h, "n")[0].clone();
    h.fake.client_copy(&f, uid, "Updates");
    let promo = h.fake.client_copy(&f, uid, "Promotions");
    h.sync();
    h.fake.reset_epoch("Newsletters");
    h.fake.client_delete("Promotions", promo);
    h.service().sync("work", 1).unwrap(); // Newsletters still rescanning
    assert!(!folder(&h, "Newsletters").rescan_complete);
    let it = item(&h, &id);
    assert_eq!(it["overrides"], json!({}));
    assert_eq!(it["classification"]["category_id"], "newsletters");
    for _ in 0..3 {
        h.sync();
    }
    let it = item(&h, &id);
    assert_eq!(it["overrides"], json!({}));
    assert_eq!(it["placement"]["folder"], "Newsletters");
    assert_eq!(it["placement"]["location_state"], "known");
    assert!(!event_kinds(&h).iter().any(|k| k == "client_correction"));
}

#[test]
fn an_explicit_done_taken_mid_transition_is_never_reopened() {
    let h = Harness::new(Live);
    h.fake.add_folder("Archive", &["\\Archive"]);
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    archive(&h, "n");
    for _ in 0..3 {
        h.sync();
    }
    let mut s = h.service();
    let read = s.store.placement("work", &id).unwrap().unwrap();
    assert!(read.done_inferred);
    // The user marks it done explicitly between a pass's read and its write.
    s.review("work", &id, true).unwrap();
    let mut known = read.clone();
    known.location_state = LocationState::Known;
    known.absent_since = None;
    let generation = s.store.records("work").unwrap()[0].generation.clone();
    let writes = [
        FilingWrite::PlacementFrom {
            placement: &known,
            read: &read,
        },
        FilingWrite::ReopenInferred {
            message_id: &id,
            generation: &generation,
        },
    ];
    assert!(s.store.commit_filing("work", &writes, &now()).unwrap());
    let p = s.store.placement("work", &id).unwrap().unwrap();
    assert!(
        !p.done_inferred,
        "the full-row write never restores done_inferred"
    );
    assert_eq!(review_state(&h, &id), "done");
}

#[test]
fn an_inferred_done_left_on_a_known_placement_is_reopened() {
    let h = Harness::new(Live);
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    // A crash after the reappearance was recorded, before the reopen.
    assert!(h.service().store.infer_done_row("work", &id).unwrap());
    assert_eq!(review_state(&h, &id), "done");
    h.sync();
    assert_eq!(review_state(&h, &id), "open");
    let p = h.service().store.placement("work", &id).unwrap().unwrap();
    assert!(!p.done_inferred);
    assert!(event_kinds(&h).iter().any(|k| k == "reopened"));
}

#[test]
fn a_home_lost_in_an_aborted_pass_is_reevaluated() {
    let h = Harness::new(Live);
    h.fake.add_folder("Archive", &["\\Archive"]);
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    archive(&h, "n");
    {
        // Reconciliation removed the home occurrence, then the pass aborted.
        let mut s = h.service();
        let p = s.store.placement("work", &id).unwrap().unwrap();
        let (folder, epoch, uid) = (
            p.home_folder.unwrap(),
            p.home_epoch.unwrap(),
            p.home_uid.unwrap(),
        );
        s.store
            .remove_occurrence("work", &folder, epoch, uid)
            .unwrap();
    }
    h.sync();
    assert_eq!(item(&h, &id)["placement"]["location_state"], "absent");
}

#[test]
fn done_is_inferred_only_in_a_pass_after_the_absence() {
    let h = Harness::new(Live);
    h.fake.add_folder("Archive", &["\\Archive"]);
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    archive(&h, "n");
    h.sync();
    assert_eq!(item(&h, &id)["placement"]["location_state"], "absent");
    assert_eq!(review_state(&h, &id), "open");
    h.sync();
    assert_eq!(review_state(&h, &id), "done");
}

#[test]
fn a_pending_arrival_holds_done_inference() {
    let h = Harness::new(Live);
    h.fake.add_folder("Archive", &["\\Archive"]);
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    // m is moved with a lost response, so its copy's arrival waits a pass
    // for intent recovery.
    h.fake
        .deliver("INBOX", &mail("m", "Another newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::ErrorAfter);
    archive(&h, "n");
    h.sync(); // n absent
    h.sync(); // m's arrival pending behind its sent intent
    assert!(!h
        .service()
        .store
        .arrivals("work", Some("pending"))
        .unwrap()
        .is_empty());
    assert_eq!(review_state(&h, &id), "open");
    h.sync();
    assert_eq!(review_state(&h, &id), "done");
}

#[test]
fn an_unresolved_arrival_holds_done_inference_until_dismissed() {
    let h = Harness::new(Live);
    h.fake.add_folder("Archive", &["\\Archive"]);
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    let uid = h.fake.deliver("INBOX", &mail("d", "Hello", "Plain mail"));
    h.fake.client_copy("INBOX", uid, "Updates");
    let mut s = h.service();
    s.sync("work", 1).unwrap();
    let all = s
        .list(
            "work",
            ListOptions {
                view: "all".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let provisional = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["classification"]["state"] == "pending")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    s.review("work", &provisional, true).unwrap();
    h.sync(); // merge conflict: unresolved
    archive(&h, "n");
    for _ in 0..3 {
        h.sync();
    }
    assert_eq!(review_state(&h, &id), "open");
    let mut s = h.service();
    let unresolved = s.store.arrivals("work", Some("unresolved")).unwrap()[0].id;
    transitions::dismiss_arrival(&mut s.store, "work", unresolved, &now()).unwrap();
    let generation = s.store.records("work").unwrap()[0].generation.clone();
    assert!(
        transitions::retry_arrival(&mut s.store, "work", unresolved, &generation, &now()).is_err(),
        "a dismissed arrival stays dismissed"
    );
    h.sync();
    assert_eq!(review_state(&h, &id), "done");
}

#[test]
fn an_open_move_intent_holds_done_inference() {
    let h = Harness::new(Live);
    h.fake.add_folder("Archive", &["\\Archive"]);
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    archive(&h, "n");
    h.sync();
    let mut s = h.service();
    let rev = s.store.placement("work", &id).unwrap().unwrap().desired_rev;
    // A sent move whose target can never count as scanned since dispatch.
    let (inbox, news) = (h.fake.epoch("INBOX"), h.fake.epoch("Newsletters"));
    s.store
        .insert_intent(&NewIntent {
            account: "work",
            message_id: &id,
            kind: "move",
            folder: "INBOX",
            epoch: inbox,
            uid: 999,
            target: Some("Newsletters"),
            target_epoch: Some(news),
            target_uid_next: Some(1000),
            desired_rev: rev,
            consumes_eligible: false,
            batch: "test",
            state: "sent",
            now: "2999-01-01T00:00:00+00:00",
        })
        .unwrap();
    for _ in 0..3 {
        h.sync();
    }
    assert_eq!(review_state(&h, &id), "open");
}

#[test]
fn a_running_rescan_holds_done_inference() {
    let h = Harness::new(Live);
    h.fake.add_folder("Archive", &["\\Archive"]);
    h.fake.add_folder("Newsletters", &[]);
    for i in 0..3 {
        h.fake.deliver(
            "Newsletters",
            &mail(&format!("pre{i}"), "Old newsletter", "newsletter"),
        );
    }
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    archive(&h, "n");
    h.sync(); // n absent
    h.fake.reset_epoch("Newsletters");
    for _ in 0..2 {
        h.service().sync("work", 1).unwrap();
        assert!(!folder(&h, "Newsletters").rescan_complete);
        assert_eq!(review_state(&h, &id), "open");
    }
    h.sync();
    assert!(folder(&h, "Newsletters").rescan_complete);
    assert_eq!(review_state(&h, &id), "done");
}

#[test]
fn a_concurrent_block_survives_an_unrelated_transition() {
    let h = Harness::new(Live);
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    let mut s = h.service();
    let read = s.store.placement("work", &id).unwrap().unwrap();
    // A pass blocks the message (no revision change) after `read` was taken.
    let mut blocked = read.clone();
    blocked.blocked_reason = Some("duplicate_copy".into());
    assert!(s
        .store
        .save_placement(&blocked, Some(read.desired_rev))
        .unwrap());
    let mut pinned = read.clone();
    pinned.pinned = true;
    pinned.desired_rev += 1;
    let write = FilingWrite::PlacementFrom {
        placement: &pinned,
        read: &read,
    };
    assert!(s.store.commit_filing("work", &[write], &now()).unwrap());
    let p = s.store.placement("work", &id).unwrap().unwrap();
    assert!(p.pinned);
    assert_eq!(p.blocked_reason.as_deref(), Some("duplicate_copy"));
    // Clearing a block that was replaced since it was read is refused.
    let mut stale_read = p.clone();
    stale_read.blocked_reason = Some("move_failed".into());
    let mut cleared = p.clone();
    cleared.blocked_reason = None;
    cleared.desired_rev += 1;
    let stale = FilingWrite::PlacementFrom {
        placement: &cleared,
        read: &stale_read,
    };
    assert!(!s.store.commit_filing("work", &[stale], &now()).unwrap());
    let p = s.store.placement("work", &id).unwrap().unwrap();
    assert_eq!(p.blocked_reason.as_deref(), Some("duplicate_copy"));
}

#[test]
fn occurrences_in_a_missing_folder_count_as_present() {
    use mailtriage::filing::{arrivals, observe, FilingSummary, PassContext};
    let h = Harness::new(Live);
    h.sync();
    let (id, home) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    h.fake.remove_folder(&home);
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
        now: now(),
        max_attempts: 5,
        verify_binding: &verify,
    };
    let mut summary = FilingSummary::default();
    let map = observe::resolve_folders(&mut s.store, &ctx, &mut summary).unwrap();
    assert_eq!(folder(&h, &home).state, "missing");
    arrivals::reevaluate(
        &mut s.store,
        &ctx,
        &map,
        std::slice::from_ref(&id),
        &mut summary,
    )
    .unwrap();
    let p = s.store.placement("work", &id).unwrap().unwrap();
    assert_eq!(p.location_state, LocationState::Known);
    assert_eq!(s.store.occurrences_of("work", &id).unwrap().len(), 1);
}

// Fix round 2: dismissal lifts a merge conflict, quarantine of a placement
// created at fetch, and a same-category move unpins.

#[test]
fn dismissing_a_merge_conflict_lifts_its_block() {
    use mailtriage::domain::FilingMode::DryRun;
    let h = Harness::new(DryRun);
    h.fake.add_folder("Updates", &[]);
    h.sync();
    let uid = h
        .fake
        .deliver("INBOX", &mail("d", "Weekly newsletter", "Our newsletter"));
    h.fake.client_copy("INBOX", uid, "Updates");
    let mut s = h.service();
    s.sync("work", 1).unwrap(); // the Updates copy stays provisional
    let all = s
        .list(
            "work",
            ListOptions {
                view: "all".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let provisional = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["classification"]["state"] == "pending")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    s.review("work", &provisional, true).unwrap();
    h.sync(); // merge refused: the canonical is blocked
    let d = {
        let s = h.service();
        let records = s.store.records("work").unwrap();
        records
            .into_iter()
            .find(|r| s.store.placement("work", &r.id).unwrap().is_some())
            .unwrap()
            .id
    };
    assert_ne!(d, provisional);
    let blocked = |h: &Harness| {
        h.service()
            .store
            .placement("work", &d)
            .unwrap()
            .unwrap()
            .blocked_reason
    };
    assert_eq!(blocked(&h).as_deref(), Some("merge_conflict"));
    h.set_mode(Live);
    h.sync();
    assert_eq!(at(&h, "d")[0].0, "INBOX", "a blocked message is not filed");
    let mut s = h.service();
    let arrival = s.store.arrivals("work", Some("unresolved")).unwrap()[0].id;
    transitions::dismiss_arrival(&mut s.store, "work", arrival, &now()).unwrap();
    assert_eq!(blocked(&h), None, "the reviewed conflict no longer blocks");
    let released = h.service().store.events("work", Some(&d), 50).unwrap();
    assert!(released.iter().any(|e| e["kind"] == "released"));
    h.sync();
    assert!(
        at(&h, "d").iter().any(|(f, _)| f == "Newsletters"),
        "plannable again: filed"
    );
}

#[test]
fn a_raced_message_placed_in_its_source_target_is_blocked_and_never_filed() {
    let h = Harness::new(Live);
    h.fake.set_capabilities(true, false, true);
    h.sync();
    filed(&h, "a", "Old newsletter", "newsletter");
    let (n, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    let (f, a_uid) = at(&h, "a")[0].clone();
    h.fake.client_delete(&f, a_uid);
    h.service().filing_pin("work", &n).unwrap(); // a move back to INBOX
                                                 // Discovered this pass but not fetched (the INBOX message takes the one
                                                 // fetch); the race renumbers Newsletters and moves r instead of n.
    h.fake.deliver("INBOX", &mail("c", "Hello", "Plain mail"));
    h.fake
        .deliver("Newsletters", &mail("r", "Invoice", "Payment due today"));
    h.fake.inject(FakeOp::Move, Fault::EpochRaceBefore);
    h.service().sync("work", 1).unwrap();
    assert_eq!(at(&h, "r")[0].0, "INBOX", "the race moved r");
    assert_eq!(
        folder(&h, "Newsletters").pause_reason.as_deref(),
        Some("epoch_race")
    );
    for _ in 0..3 {
        h.sync();
    }
    let r = arrival_of(&h, "INBOX", "r");
    assert_eq!(
        (r.state.as_str(), r.kind.as_deref()),
        ("resolved", Some("quarantined"))
    );
    let s = h.service();
    let p = s.store.placement("work", &r.message_id).unwrap().unwrap();
    assert_eq!(p.home_folder.as_deref(), Some("INBOX"));
    assert_eq!(p.blocked_reason.as_deref(), Some("quarantined"));
    assert_eq!(at(&h, "r"), vec![("INBOX".to_string(), r.uid)]);
    assert!(h.fake.flags("INBOX", r.uid).is_empty(), "never flagged");
    assert!(s
        .store
        .intents("work", false)
        .unwrap()
        .iter()
        .all(|i| i.message_id != r.message_id));
}

#[test]
fn a_pinned_message_filed_into_its_own_category_stays_there_unpinned() {
    let h = Harness::new(Live);
    h.sync();
    let (id, home) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    h.service().filing_pin("work", &id).unwrap();
    h.sync();
    h.sync();
    assert_eq!(at(&h, "n")[0].0, "INBOX");
    assert_eq!(item(&h, &id)["placement"]["pinned"], true);
    let (f, uid) = at(&h, "n")[0].clone();
    h.fake.client_move(&f, uid, &home);
    for _ in 0..3 {
        h.sync();
    }
    assert_eq!(at(&h, "n")[0].0, home, "not moved back against the user");
    let it = item(&h, &id);
    assert_eq!(it["placement"]["pinned"], false);
    assert_eq!(it["placement"]["pending_action"], serde_json::Value::Null);
    assert_eq!(it["overrides"], json!({}));
    let kinds = event_kinds(&h);
    assert!(!kinds.iter().any(|k| k == "client_correction"));
    assert!(kinds.iter().any(|k| k == "relocated"));
}
