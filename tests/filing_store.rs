use mailtriage::{
    domain::{FilingMode, MailboxSnapshot, SourceEnvelope},
    filing::{LocationState, RescanFilter, StageOptions},
    normalize,
    store::Store,
};
use std::collections::{BTreeMap, BTreeSet};

fn store() -> (tempfile::TempDir, Store) {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("db")).unwrap();
    (d, s)
}
fn env(uid: u64, mid: &str) -> SourceEnvelope {
    SourceEnvelope {
        uid,
        subject: "s".into(),
        message_id: Some(format!("<{mid}@t>")),
        internal_date: Some("2026-10-04T10:00:00+00:00".into()),
        size: Some(100),
        flags: vec!["\\Seen".into()],
        ..Default::default()
    }
}
fn snap(epoch: u64, next: u64) -> MailboxSnapshot {
    MailboxSnapshot {
        uid_validity: epoch,
        uid_next: next,
    }
}
fn opts<'a>(t: &'a BTreeMap<u64, (String, i64)>) -> StageOptions<'a> {
    StageOptions {
        record_arrivals: true,
        known_targets: t,
        rescan_filter: None,
    }
}
const NOW: &str = "2026-10-04T12:00:00+00:00";

#[test]
fn migration_reaches_v3_and_is_idempotent() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    assert_eq!(Store::open(&p).unwrap().schema_version().unwrap(), 3);
    assert_eq!(Store::open(&p).unwrap().schema_version().unwrap(), 3);
}

#[test]
fn filing_mode_transitions_keep_or_reset_enabled_at() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    let a = s
        .sync_filing_mode("work", FilingMode::DryRun, "2026-10-01T00:00:00+00:00")
        .unwrap();
    assert_eq!(a.enabled_at.as_deref(), Some("2026-10-01T00:00:00+00:00"));
    let b = s
        .sync_filing_mode("work", FilingMode::Live, "2026-10-02T00:00:00+00:00")
        .unwrap();
    assert_eq!(b.enabled_at, a.enabled_at);
    let c = s
        .sync_filing_mode("work", FilingMode::Off, "2026-10-03T00:00:00+00:00")
        .unwrap();
    assert_eq!(c.enabled_at, a.enabled_at);
    let e = s
        .sync_filing_mode("work", FilingMode::Live, "2026-10-04T00:00:00+00:00")
        .unwrap();
    assert_eq!(e.enabled_at.as_deref(), Some("2026-10-04T00:00:00+00:00"));
}

#[test]
fn stage_with_records_arrivals_metadata_and_known_targets() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    s.checkpoint("work", "INBOX", &snap(5, 3)).unwrap();
    let none = BTreeMap::new();
    s.stage_with(
        "work",
        "INBOX",
        5,
        2,
        &[env(1, "a"), env(2, "b")],
        "g1",
        true,
        &opts(&none),
    )
    .unwrap();
    let arrivals = s.arrivals("work", Some("pending")).unwrap();
    assert_eq!(arrivals.len(), 2);
    assert_eq!(arrivals[0].rfc_message_id.as_deref(), Some("<a@t>"));
    let id = arrivals[0].message_id.clone();
    let meta = s.message_meta("work", &id).unwrap().unwrap();
    assert_eq!(meta.size, Some(100));
    assert_eq!(meta.flags, vec!["\\Seen".to_string()]);
    // A COPYUID-known target joins the existing message without a new message.
    s.checkpoint("work", "News", &snap(9, 21)).unwrap();
    let targets = BTreeMap::from([(20u64, (id.clone(), 77i64))]);
    s.stage_with(
        "work",
        "News",
        9,
        20,
        &[env(20, "a")],
        "g1",
        true,
        &opts(&targets),
    )
    .unwrap();
    let occ = s.occurrences_of("work", &id).unwrap();
    assert!(occ.contains(&("News".to_string(), 9, 20)));
    let arr = s.arrivals("work", Some("pending")).unwrap();
    assert!(arr
        .iter()
        .any(|a| a.folder == "News" && a.intent_id == Some(77) && a.message_id == id));
}

#[test]
fn rescan_filter_skips_unrelated_old_content() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    s.checkpoint("work", "News", &snap(3, 10)).unwrap();
    let none = BTreeMap::new();
    let filter = RescanFilter {
        below_uid: 10,
        rfc_ids: BTreeSet::from([Some("<keep@t>".to_string())]),
    };
    let o = StageOptions {
        record_arrivals: true,
        known_targets: &none,
        rescan_filter: Some(&filter),
    };
    s.stage_with(
        "work",
        "News",
        3,
        11,
        &[env(1, "keep"), env(2, "other"), env(11, "new")],
        "g1",
        true,
        &o,
    )
    .unwrap();
    let ids: Vec<_> = s
        .arrivals("work", None)
        .unwrap()
        .into_iter()
        .map(|a| a.uid)
        .collect();
    assert_eq!(ids, vec![1, 11]);
}

#[test]
fn placement_creation_selection_and_revision_cas() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    let raw = b"Message-ID: <a@t>\r\nSubject: s\r\n\r\nbody";
    let msg = normalize::rfc822(raw, 1000).unwrap();
    s.checkpoint("work", "News", &snap(9, 5)).unwrap();
    s.checkpoint("work", "INBOX", &snap(5, 5)).unwrap();
    let none = BTreeMap::new();
    s.stage_with(
        "work",
        "News",
        9,
        4,
        &[env(4, "a")],
        "g1",
        true,
        &opts(&none),
    )
    .unwrap();
    let id = s.arrivals("work", None).unwrap()[0].message_id.clone();
    let sources = vec!["INBOX".to_string(), "Work".to_string()];
    assert!(
        !s.ensure_placement("work", &id, &sources).unwrap(),
        "no fingerprint yet"
    );
    s.attach("work", &id, &msg).unwrap();
    assert!(s.ensure_placement("work", &id, &sources).unwrap());
    let p = s.placement("work", &id).unwrap().unwrap();
    assert_eq!(p.home_folder.as_deref(), Some("News"));
    assert_eq!(p.source_folder, "INBOX");
    assert_eq!(p.location_state, LocationState::Known);
    let mut q = p.clone();
    q.pinned = true;
    q.desired_rev += 1;
    assert!(s.save_placement(&q, Some(p.desired_rev)).unwrap());
    assert!(
        !s.save_placement(&q, Some(p.desired_rev)).unwrap(),
        "stale revision must not write"
    );
}

#[test]
fn filing_aware_merge_moves_arrivals_and_keeps_metadata() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    let raw = b"Message-ID: <a@t>\r\nSubject: s\r\n\r\nbody";
    let msg = normalize::rfc822(raw, 1000).unwrap();
    let canonical = s.ingest("work", &msg, "g1").unwrap();
    s.checkpoint("work", "INBOX", &snap(5, 5)).unwrap();
    let none = BTreeMap::new();
    s.stage_with(
        "work",
        "INBOX",
        5,
        1,
        &[env(1, "a")],
        "g1",
        true,
        &opts(&none),
    )
    .unwrap();
    let provisional = s.arrivals("work", None).unwrap()[0].message_id.clone();
    assert_eq!(s.attach("work", &provisional, &msg).unwrap(), canonical);
    let arrival = &s.arrivals("work", None).unwrap()[0];
    assert_eq!(arrival.message_id, canonical);
    let meta = s.message_meta("work", &canonical).unwrap().unwrap();
    assert_eq!(meta.rfc_message_id.as_deref(), Some("<a@t>"));
    assert_eq!(meta.size, Some(100));
}

#[test]
fn epoch_reset_captures_rescan_set_and_vanishes_pending_arrivals() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    s.checkpoint("work", "News", &snap(3, 5)).unwrap();
    let none = BTreeMap::new();
    s.stage_with(
        "work",
        "News",
        3,
        4,
        &[env(4, "a")],
        "g1",
        true,
        &opts(&none),
    )
    .unwrap();
    let id = s.arrivals("work", None).unwrap()[0].message_id.clone();
    s.checkpoint("work", "News", &snap(4, 2)).unwrap();
    let members = s.rescan_members("work", "News", 4).unwrap();
    assert_eq!(members, vec![(id, Some("<a@t>".to_string()))]);
    assert_eq!(s.arrivals("work", Some("vanished")).unwrap().len(), 1);
}

#[test]
fn claims_check_revision_and_consume_flag_attempt() {
    use mailtriage::filing::planner::{Action, Locator};
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    let raw = b"Message-ID: <a@t>\r\nSubject: s\r\n\r\nbody";
    let msg = normalize::rfc822(raw, 1000).unwrap();
    s.checkpoint("work", "INBOX", &snap(5, 5)).unwrap();
    let none = BTreeMap::new();
    s.stage_with(
        "work",
        "INBOX",
        5,
        1,
        &[env(1, "a")],
        "g1",
        true,
        &opts(&none),
    )
    .unwrap();
    let id = s.arrivals("work", None).unwrap()[0].message_id.clone();
    s.attach("work", &id, &msg).unwrap();
    s.ensure_placement("work", &id, &["INBOX".to_string()])
        .unwrap();
    let at = Locator {
        folder: "INBOX".into(),
        epoch: 5,
        uid: 1,
    };
    let stale = Action::Move {
        message_id: id.clone(),
        from: at.clone(),
        to: "News".into(),
        desired_rev: 9,
        consumes_eligible: false,
    };
    assert!(s
        .claim_move("work", &stale, 1, 1, "b1", NOW)
        .unwrap()
        .is_none());
    let fresh = Action::Move {
        message_id: id.clone(),
        from: at.clone(),
        to: "News".into(),
        desired_rev: 0,
        consumes_eligible: false,
    };
    assert!(s
        .claim_move("work", &fresh, 1, 1, "b1", NOW)
        .unwrap()
        .is_some());
    assert!(
        s.claim_move("work", &fresh, 1, 1, "b2", NOW)
            .unwrap()
            .is_none(),
        "one open move intent per message"
    );
    let flag = Action::Flag {
        message_id: id.clone(),
        at,
    };
    assert!(s.claim_flag("work", &flag, "b3", NOW).unwrap().is_some());
    assert!(
        s.claim_flag("work", &flag, "b4", NOW).unwrap().is_none(),
        "one flag attempt ever"
    );
    assert!(s
        .placement("work", &id)
        .unwrap()
        .unwrap()
        .flag_attempted_at
        .is_some());
}

#[test]
fn events_are_newest_first() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    s.record_event(
        "work",
        None,
        Some("News"),
        "folder_created",
        serde_json::json!({}),
        "2026-10-04T10:00:00+00:00",
    )
    .unwrap();
    s.record_event(
        "work",
        Some("m1"),
        None,
        "moved",
        serde_json::json!({"to": "News"}),
        "2026-10-04T11:00:00+00:00",
    )
    .unwrap();
    let e = s.events("work", None, 10).unwrap();
    assert_eq!(e[0]["kind"], "moved");
    assert_eq!(s.events("work", Some("m1"), 10).unwrap().len(), 1);
}

// Supplementary coverage for store contracts later tasks rely on.

fn id_at(s: &Store, uid: u64) -> String {
    s.records("work")
        .unwrap()
        .into_iter()
        .find(|r| r.envelope["uid"] == uid)
        .unwrap()
        .id
}

#[test]
fn newer_schema_is_rejected() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    drop(Store::open(&p).unwrap());
    rusqlite::Connection::open(&p)
        .unwrap()
        .pragma_update(None, "user_version", 4)
        .unwrap();
    assert!(Store::open(&p).is_err());
}

#[test]
fn first_watch_checkpoint_and_epoch_reset_mark_folder_for_rescan() {
    use mailtriage::filing::FolderRecord;
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    assert_eq!(
        s.checkpoint_start_at("work", "News", &snap(3, 8), 7)
            .unwrap(),
        7
    );
    assert_eq!(
        s.checkpoint_start_at("work", "News", &snap(3, 9), 8)
            .unwrap(),
        7,
        "existing checkpoint wins"
    );
    let rec = FolderRecord {
        account: "work".into(),
        native: "News".into(),
        configured: Some("News".into()),
        category_id: Some("news".into()),
        origin: Some("created".into()),
        state: "ok".into(),
        role_verified: true,
        confirmed: false,
        subscribed: true,
        pause_reason: None,
        epoch: Some(3),
        watch_from_uid: Some(7),
        rescan_epoch: None,
        rescan_below_uid: None,
        rescan_complete: true,
        checked_at: Some(NOW.into()),
        error: None,
    };
    s.save_folder(&rec).unwrap();
    assert_eq!(s.folder_record("work", "News").unwrap().unwrap(), rec);
    assert_eq!(
        s.checkpoint_start_at("work", "News", &snap(4, 12), 11)
            .unwrap(),
        0,
        "epoch reset rescans from UID 1"
    );
    let f = s.folder_record("work", "News").unwrap().unwrap();
    assert_eq!(
        (
            f.epoch,
            f.rescan_epoch,
            f.rescan_below_uid,
            f.rescan_complete
        ),
        (Some(4), Some(4), Some(12), false)
    );
    assert_eq!(s.folder_records("work").unwrap().len(), 1);
}

#[test]
fn reconcile_ids_hydrate_and_attach_keep_transport_metadata() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    s.checkpoint("work", "INBOX", &snap(5, 4)).unwrap();
    let bare = SourceEnvelope {
        uid: 3,
        subject: "s".into(),
        ..Default::default()
    };
    s.stage(
        "work",
        "INBOX",
        5,
        3,
        &[env(1, "a"), env(2, "b"), bare],
        "g1",
        true,
    )
    .unwrap();
    assert!(
        s.arrivals("work", None).unwrap().is_empty(),
        "plain stage records no arrivals"
    );
    let (a, b, c) = (id_at(&s, 1), id_at(&s, 2), id_at(&s, 3));
    assert_eq!(
        s.reconcile_range_ids("work", "INBOX", 5, 0, 3, &[2, 3], true)
            .unwrap(),
        vec![a]
    );
    let msg = normalize::rfc822(b"Message-ID: <b@t>\r\nSubject: real\r\n\r\nbody", 1000).unwrap();
    s.attach("work", &b, &msg).unwrap();
    let envelope = s.record("work", &b).unwrap().unwrap().envelope;
    assert_eq!(
        (
            envelope["subject"].as_str(),
            envelope["message_id"].as_str()
        ),
        (Some("real"), Some("<b@t>"))
    );
    assert_eq!(s.message_meta("work", &c).unwrap().unwrap().size, None);
    let full = SourceEnvelope {
        flags: vec!["\\Flagged".into()],
        ..env(3, "c")
    };
    s.hydrate("work", &c, &full).unwrap();
    let meta = s.message_meta("work", &c).unwrap().unwrap();
    assert_eq!(
        (meta.rfc_message_id.as_deref(), meta.size, meta.flags),
        (Some("<c@t>"), Some(100), vec!["\\Flagged".to_string()])
    );
    assert_eq!(
        s.record("work", &c).unwrap().unwrap().envelope["subject"],
        "s"
    );
    s.remove_occurrence("work", "INBOX", 5, 3).unwrap();
    assert!(s.occurrences_of("work", &c).unwrap().is_empty());
}

#[test]
fn intent_and_revert_updates_overwrite_only_given_fields() {
    use mailtriage::filing::{IntentPatch, NewIntent, Revert};
    let (_d, mut s) = store();
    let i = s
        .insert_intent(&NewIntent {
            account: "work",
            message_id: "m1",
            kind: "move",
            folder: "INBOX",
            epoch: 5,
            uid: 1,
            target: Some("News"),
            target_epoch: Some(9),
            target_uid_next: Some(20),
            desired_rev: 2,
            consumes_eligible: true,
            batch: "b1",
            state: "sent",
            now: NOW,
        })
        .unwrap();
    assert_eq!(s.intents("work", true).unwrap().len(), 1);
    s.update_intent(
        i,
        "uncertain",
        IntentPatch {
            attempts: Some(2),
            error: Some("timeout".into()),
            ..Default::default()
        },
        NOW,
    )
    .unwrap();
    s.update_intent(
        i,
        "applied",
        IntentPatch {
            target_uid: Some(20),
            ..Default::default()
        },
        NOW,
    )
    .unwrap();
    let all = s.intents("work", false).unwrap();
    assert_eq!(
        (
            all[0].state.as_str(),
            all[0].attempts,
            all[0].error.as_deref(),
            all[0].target_uid
        ),
        ("applied", 2, Some("timeout"), Some(20))
    );
    assert!(s.intents("work", true).unwrap().is_empty());
    let r = s
        .insert_revert(&Revert {
            id: 0,
            account: "work".into(),
            parent_intent: i,
            folder: "News".into(),
            folder_epoch: 9,
            uid: 20,
            target: "INBOX".into(),
            target_epoch: 5,
            state: "pending".into(),
            target_uid: None,
            created_at: NOW.into(),
            updated_at: NOW.into(),
            error: None,
        })
        .unwrap();
    assert_eq!(s.reverts("work", true).unwrap().len(), 1);
    s.update_revert(r, "applied", Some(7), None, NOW).unwrap();
    assert!(s.reverts("work", true).unwrap().is_empty());
    assert_eq!(s.reverts("work", false).unwrap()[0].target_uid, Some(7));
}

#[test]
fn enabling_restarts_bootstrap_and_arrivals_resolve() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    assert_eq!(s.filing_state("work").unwrap().mode, FilingMode::Off);
    s.sync_filing_mode("work", FilingMode::DryRun, NOW).unwrap();
    s.set_bootstrap_done("work", true).unwrap();
    s.set_last_pass("work", &serde_json::json!({"moved": 1}))
        .unwrap();
    assert!(
        s.sync_filing_mode("work", FilingMode::Live, NOW)
            .unwrap()
            .bootstrap_done,
        "dry_run -> live keeps it"
    );
    s.sync_filing_mode("work", FilingMode::Off, NOW).unwrap();
    let on = s.sync_filing_mode("work", FilingMode::Live, NOW).unwrap();
    assert!(!on.bootstrap_done, "off -> on restarts the bootstrap");
    assert_eq!(on.last_pass, Some(serde_json::json!({"moved": 1})));
    s.checkpoint("work", "INBOX", &snap(5, 2)).unwrap();
    let none = BTreeMap::new();
    s.stage_with(
        "work",
        "INBOX",
        5,
        1,
        &[env(1, "a")],
        "g1",
        true,
        &opts(&none),
    )
    .unwrap();
    let a = s.arrivals("work", Some("pending")).unwrap()[0].id;
    s.resolve_arrival(a, "resolved", Some("new"), NOW).unwrap();
    let r = &s.arrivals("work", None).unwrap()[0];
    assert_eq!(
        (
            r.state.as_str(),
            r.kind.as_deref(),
            r.resolved_at.as_deref()
        ),
        ("resolved", Some("new"), Some(NOW))
    );
}

#[test]
fn scan_error_placeholder_is_a_first_watch_not_an_epoch_reset() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    s.scan_error("work", "News").unwrap();
    assert_eq!(
        s.checkpoint_start_at("work", "News", &snap(3, 8), 7)
            .unwrap(),
        7,
        "the placeholder starts at the tip"
    );
    let scans = s.coverage("work").unwrap()["scans"].clone();
    assert_eq!(
        (
            scans[0]["uid_validity"].as_u64(),
            scans[0]["last_uid"].as_u64(),
            scans[0]["error"].is_null()
        ),
        (Some(3), Some(7), true)
    );
    s.scan_error("work", "News").unwrap();
    assert_eq!(
        s.checkpoint_start_at("work", "News", &snap(3, 9), 8)
            .unwrap(),
        7,
        "a failed scan keeps a real checkpoint"
    );
    assert_eq!(
        s.checkpoint_start_at("work", "Empty", &snap(5, 1), 0)
            .unwrap(),
        0
    );
    assert_eq!(
        s.checkpoint_start_at("work", "Empty", &snap(5, 4), 3)
            .unwrap(),
        0,
        "a real checkpoint at UID 0 is not a placeholder"
    );
}

#[test]
fn hydrate_never_nulls_known_transport_metadata() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    s.checkpoint("work", "INBOX", &snap(5, 2)).unwrap();
    s.stage("work", "INBOX", 5, 1, &[env(1, "a")], "g1", true)
        .unwrap();
    let id = id_at(&s, 1);
    let sparse = SourceEnvelope {
        uid: 1,
        subject: "s".into(),
        message_id: Some("  ".into()),
        flags: vec!["\\Flagged".into()],
        ..Default::default()
    };
    s.hydrate("work", &id, &sparse).unwrap();
    let meta = s.message_meta("work", &id).unwrap().unwrap();
    assert_eq!(
        (
            meta.rfc_message_id.as_deref(),
            meta.size,
            meta.internal_date.as_deref(),
            meta.flags
        ),
        (
            Some("<a@t>"),
            Some(100),
            Some("2026-10-04T10:00:00+00:00"),
            vec!["\\Flagged".to_string()]
        )
    );
    let envelope = s.record("work", &id).unwrap().unwrap().envelope;
    assert_eq!(
        (
            envelope["message_id"].as_str(),
            envelope["size"].as_u64(),
            envelope["internal_date"].as_str()
        ),
        (Some("<a@t>"), Some(100), Some("2026-10-04T10:00:00+00:00"))
    );
}
