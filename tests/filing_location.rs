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
    assert_eq!(item(&h, &id)["review_state"], "open");
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
    transitions::dismiss_arrival(&mut s.store, "work", arrival, &now).unwrap();
    assert!(transitions::dismiss_arrival(&mut s.store, "work", arrival, &now).is_err());
    // The user clears the provisional's edit, then retries the arrival.
    s.review("work", &provisional, false).unwrap();
    let generation = s.store.records("work").unwrap()[0].generation.clone();
    transitions::retry_arrival(&mut s.store, "work", arrival, &generation, &now).unwrap();
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
