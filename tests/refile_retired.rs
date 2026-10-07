//! Refile spec "Retired folders": retention, draining and freezing.
mod common;
mod refile_support;
use common::{mail, Harness};
use mailtriage::{
    domain::{FilingMode::Live, MailboxSnapshot, SourceEnvelope},
    filing::{FolderRecord, LocationState, StageOptions},
    store::Store,
};
use refile_support::{
    apply, deliver_filed, events, filed_home, home, located, placement, preview, remove_category,
    review_state, scoped,
};
use serde_json::json;
use std::collections::BTreeMap;

fn snapshot_calls(h: &Harness, folder: &str) -> usize {
    let call = format!("snapshot {folder}");
    h.fake.calls().iter().filter(|c| **c == call).count()
}

#[test]
fn a_repointed_folder_stays_watched_until_its_mail_moved_then_freezes() {
    let h = Harness::new(Live);
    h.sync();
    let id = deliver_filed(&h, "n", "Weekly newsletter", "Our newsletter");
    let mut categories = h.service().config.accounts["work"].categories.clone();
    categories
        .iter_mut()
        .find(|c| c.id == "newsletters")
        .unwrap()
        .folder = Some("News".into());
    let out = h.service().apply_categories("work", categories).unwrap();
    assert!(out["hint"].is_string());
    h.sync(); // News is created; Newsletters is retired and kept by n
    assert!(scoped(&h, "Newsletters"));
    let v = preview(&h, None, Some("Newsletters"));
    assert_eq!(
        v["candidates"],
        json!([{"id": id, "folder": "Newsletters", "target": "News", "category": "newsletters", "reason": "folder_retired"}])
    );
    assert_eq!(apply(&h, None, Some("Newsletters"))["marked"], 1);
    for _ in 0..4 {
        h.sync();
    }
    assert_eq!(located(&h, "n").0, "News");
    let p = placement(&h, &id);
    assert_eq!(filed_home(&p), home(&p));
    assert!(!scoped(&h, "Newsletters"), "drained and frozen");
    assert!(!h
        .service()
        .store
        .drain_states("work")
        .unwrap()
        .contains_key("Newsletters"));
    assert!(
        h.fake.uids("Newsletters").is_empty(),
        "the emptied folder stays on the server"
    );
}

#[test]
fn removing_a_category_moves_its_waiting_mail_once_classified() {
    let h = Harness::new(Live);
    h.sync();
    let id = deliver_filed(&h, "n", "Weekly newsletter", "Our newsletter");
    remove_category(&h, "newsletters");
    let v = preview(&h, None, Some("Newsletters"));
    assert_eq!(
        (v["total"].clone(), v["waiting"].clone()),
        (json!(0), json!(1))
    );
    assert_eq!(
        v["folders"],
        json!([{"folder": "Newsletters", "native": "Newsletters", "retired": true, "candidates": 0, "waiting": 1}])
    );
    assert_eq!(
        apply(&h, None, Some("Newsletters")),
        json!({"schema_version": 1, "account": "work", "marked": 0, "waiting_marked": 1})
    );
    for _ in 0..5 {
        h.sync();
    }
    assert_eq!(located(&h, "n").0, "Other");
    assert!(!placement(&h, &id).refile_once);
    assert!(!scoped(&h, "Newsletters"));
}

#[test]
fn a_draining_folder_is_watched_until_mail_moved_into_it_is_found() {
    let h = Harness::new(Live);
    h.sync();
    deliver_filed(&h, "a", "Weekly newsletter", "Our newsletter");
    let b = deliver_filed(&h, "b", "System update", "Version 2 is out");
    let fillers: Vec<u64> = (0..3)
        .map(|i| {
            h.fake.deliver(
                "INBOX",
                &mail(&format!("f{i}"), &format!("Lunch {i}"), "See you at noon"),
            )
        })
        .collect();
    h.sync();
    remove_category(&h, "newsletters");
    h.sync(); // Newsletters is retired and kept by a, classified again into `other`
    assert!(scoped(&h, "Newsletters"));
    assert_eq!(apply(&h, None, Some("Newsletters"))["marked"], 1);
    // Three messages, then b, go into Newsletters: b lies beyond one pass's
    // discovery window.
    for uid in fillers {
        h.fake.client_move("INBOX", uid, "Newsletters");
    }
    let (folder, uid) = located(&h, "b");
    h.fake.client_move(&folder, uid, "Newsletters");
    for pass in 0..40 {
        h.service().sync("work", 1).unwrap();
        assert_eq!(
            review_state(&h, &b),
            "open",
            "pass {pass}: b is never inferred done"
        );
        if !scoped(&h, "Newsletters") {
            break;
        }
    }
    assert!(!scoped(&h, "Newsletters"), "frozen once drained");
    let p = placement(&h, &b);
    assert_eq!(
        (p.home_folder.as_deref(), p.location_state),
        (Some("Newsletters"), LocationState::Known)
    );
    assert_eq!(located(&h, "a").0, "Other");
}

#[test]
fn done_mail_alone_does_not_keep_a_retired_folder_watched() {
    let h = Harness::new(Live);
    h.sync();
    let id = deliver_filed(&h, "d", "Weekly newsletter", "Our newsletter");
    h.service().review("work", &id, true).unwrap();
    remove_category(&h, "newsletters");
    let before = snapshot_calls(&h, "Newsletters");
    h.sync();
    h.sync();
    assert_eq!(snapshot_calls(&h, "Newsletters"), before, "frozen at once");
    assert_eq!(
        filed_home(&placement(&h, &id)),
        None,
        "a frozen folder holds no filed home"
    );
    h.service().review("work", &id, false).unwrap();
    h.sync();
    h.sync();
    assert_eq!(
        snapshot_calls(&h, "Newsletters"),
        before,
        "never retained again"
    );
    assert_eq!(
        preview(&h, None, Some("Newsletters"))["skipped"]["retired_frozen"],
        1
    );
}

#[test]
fn a_folder_frozen_before_v6_is_reported_and_never_watched_again() {
    let h = Harness::new(Live);
    h.sync();
    let id = deliver_filed(&h, "n", "Weekly newsletter", "Our newsletter");
    {
        // As migration v6 leaves mail in a folder retired before it: no filed home.
        let mut s = h.service();
        let mut p = s.store.placement("work", &id).unwrap().unwrap();
        p.filed_home_folder = None;
        assert!(s.store.save_placement(&p, None).unwrap());
    }
    remove_category(&h, "newsletters");
    let before = snapshot_calls(&h, "Newsletters");
    for _ in 0..3 {
        h.sync();
    }
    let v = preview(&h, None, Some("Newsletters"));
    assert_eq!(
        (v["total"].clone(), v["skipped"]["retired_frozen"].clone()),
        (json!(0), json!(1))
    );
    assert_eq!(
        apply(&h, None, Some("Newsletters")),
        json!({"schema_version": 1, "account": "work", "marked": 0, "waiting_marked": 0})
    );
    h.sync();
    h.sync();
    assert_eq!(located(&h, "n").0, "Newsletters");
    assert_eq!(
        snapshot_calls(&h, "Newsletters"),
        before,
        "never watched again"
    );
}

#[test]
fn a_retired_folder_deleted_on_the_server_freezes_and_clears_its_marks() {
    let h = Harness::new(Live);
    h.sync();
    let id = deliver_filed(&h, "n", "Weekly newsletter", "Our newsletter");
    remove_category(&h, "newsletters");
    h.sync();
    assert_eq!(apply(&h, None, Some("Newsletters"))["marked"], 1);
    h.fake.remove_folder("Newsletters");
    let out = h.sync();
    assert_eq!(out["filing"]["errors"], 0, "{out}");
    assert!(!placement(&h, &id).refile_once);
    assert_eq!(
        events(&h, "refile_cleared")[0]["detail"]["reason"],
        "not_filed_by_mailtriage"
    );
    assert!(!h
        .fake
        .calls()
        .iter()
        .any(|c| c.starts_with("move Newsletters")));
    h.sync();
    assert!(!scoped(&h, "Newsletters"));
}

fn store() -> (tempfile::TempDir, Store) {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("db")).unwrap();
    (d, s)
}

fn snap(epoch: u64, next: u64) -> MailboxSnapshot {
    MailboxSnapshot {
        uid_validity: epoch,
        uid_next: next,
    }
}

fn retired(native: &str) -> FolderRecord {
    FolderRecord {
        account: "work".into(),
        native: native.into(),
        configured: Some(native.into()),
        category_id: Some("old".into()),
        origin: Some("created".into()),
        state: "retired".into(),
        role_verified: true,
        confirmed: false,
        subscribed: true,
        pause_reason: None,
        epoch: None,
        watch_from_uid: None,
        rescan_epoch: None,
        rescan_below_uid: None,
        rescan_complete: true,
        checked_at: None,
        error: None,
    }
}

#[test]
fn draining_ends_once_discovery_passed_the_snapshot_and_nothing_waits() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    s.save_folder(&retired("Old")).unwrap();
    s.checkpoint_start_at("work", "Old", &snap(3, 8), 7)
        .unwrap();
    s.set_drain_until_uid("work", "Old", Some(10)).unwrap();
    assert!(
        !s.drain_finished("work", "Old").unwrap(),
        "discovery is at UID 7"
    );
    let none = BTreeMap::new();
    let opts = StageOptions {
        record_arrivals: true,
        known_targets: &none,
        rescan_filter: None,
    };
    let env = SourceEnvelope {
        uid: 9,
        subject: "s".into(),
        message_id: Some("<b@t>".into()),
        size: Some(10),
        ..Default::default()
    };
    s.stage_with("work", "Old", 3, 9, &[env], "g1", true, &opts)
        .unwrap();
    assert!(
        !s.drain_finished("work", "Old").unwrap(),
        "an arrival it found is pending"
    );
    let arrival = s.arrivals("work", Some("pending")).unwrap().remove(0);
    s.resolve_arrival(
        arrival.id,
        "resolved",
        Some("extra"),
        "2026-10-06T12:00:00+00:00",
    )
    .unwrap();
    assert!(s.drain_finished("work", "Old").unwrap());
    assert_eq!(
        s.drain_states("work").unwrap(),
        BTreeMap::from([("Old".to_string(), 10)])
    );
    s.freeze_retired("work", "Old").unwrap();
    assert!(s.drain_states("work").unwrap().is_empty());
}

#[test]
fn an_epoch_reset_while_draining_records_a_new_snapshot() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    for native in ["Draining", "Kept"] {
        s.save_folder(&retired(native)).unwrap();
        s.checkpoint_start_at("work", native, &snap(3, 8), 7)
            .unwrap();
    }
    s.set_drain_until_uid("work", "Draining", Some(8)).unwrap();
    s.set_drain_until_uid("work", "Kept", Some(0)).unwrap();
    s.checkpoint("work", "Draining", &snap(4, 12)).unwrap();
    s.checkpoint("work", "Kept", &snap(5, 20)).unwrap();
    assert_eq!(
        s.drain_states("work").unwrap(),
        BTreeMap::from([("Draining".to_string(), 12), ("Kept".to_string(), 0)])
    );
    assert!(
        !s.drain_finished("work", "Draining").unwrap(),
        "the reset rescan runs first"
    );
}
