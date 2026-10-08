//! Schema v6 (refile spec "Storage"): the migration, the stored mark and
//! filed home, and the refile flag of a claimed move.
use mailtriage::{
    domain::{MailboxSnapshot, SourceEnvelope},
    filing::{
        planner::{Action, Locator},
        IntentPatch, LocationState, StageOptions,
    },
    normalize,
    store::{Store, LATEST},
};
use std::collections::BTreeMap;

const NOW: &str = "2026-10-06T12:00:00+00:00";

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

/// A fingerprinted, hydrated message at `folder`/`epoch`/`uid`, its placement homed there.
fn placed(s: &mut Store, folder: &str, epoch: u64, uid: u64, mid: &str) -> String {
    if s.checkpoint_state("work", folder).unwrap().is_none() {
        s.checkpoint("work", folder, &snap(epoch, uid + 1)).unwrap();
    }
    let env = SourceEnvelope {
        uid,
        subject: "s".into(),
        message_id: Some(format!("<{mid}@t>")),
        internal_date: Some("2026-10-04T10:00:00+00:00".into()),
        size: Some(100),
        ..Default::default()
    };
    let none = BTreeMap::new();
    let opts = StageOptions {
        record_arrivals: false,
        known_targets: &none,
        rescan_filter: None,
    };
    s.stage_with("work", folder, epoch, uid, &[env], "g1", true, &opts)
        .unwrap();
    let id = s
        .occurrence_at("work", folder, epoch, uid)
        .unwrap()
        .unwrap();
    let raw = format!("Message-ID: <{mid}@t>\r\nSubject: s\r\n\r\nbody {mid}\r\n");
    s.attach(
        "work",
        &id,
        &normalize::rfc822(raw.as_bytes(), 1000).unwrap(),
    )
    .unwrap();
    assert!(s
        .ensure_placement("work", &id, &[folder.to_string()])
        .unwrap());
    id
}

/// Records the placement's current home as its filed home.
fn file_home(s: &mut Store, id: &str) {
    let mut p = s.placement("work", id).unwrap().unwrap();
    p.filed_home_folder = p.home_folder.clone();
    p.filed_home_epoch = p.home_epoch;
    p.filed_home_uid = p.home_uid;
    assert!(s.save_placement(&p, None).unwrap());
}

/// Turns a fresh database back into v5.
const DOWNGRADE_TO_V5: &str = "ALTER TABLE placements DROP COLUMN refile_once;
ALTER TABLE placements DROP COLUMN filed_home_folder;
ALTER TABLE placements DROP COLUMN filed_home_epoch;
ALTER TABLE placements DROP COLUMN filed_home_uid;
ALTER TABLE filing_intents DROP COLUMN consumes_refile;
ALTER TABLE folders DROP COLUMN drain_until_uid;
ALTER TABLE pass_heartbeats DROP COLUMN version;
ALTER TABLE pass_heartbeats DROP COLUMN reason;
ALTER TABLE pass_heartbeats DROP COLUMN reason_at;
DROP TABLE read_approvals;
PRAGMA user_version=5;";

/// One placement per migration case, with its move intents (ids ascend in
/// insertion order, so the last applied move of a message is its newest).
const V5_ROWS: &str = "INSERT INTO accounts VALUES('work','id','g1');
INSERT INTO folders(account,native,category_id,state) VALUES('work','News','news','ok'),('work','Old','old','retired');
INSERT INTO messages(id,account,envelope,status,observed_at,generation) VALUES
 ('proven','work','{}','ready','2026-10-01T00:00:00+00:00','g1'),
 ('later_failure','work','{}','ready','2026-10-01T00:00:00+00:00','g1'),
 ('no_copyuid','work','{}','ready','2026-10-01T00:00:00+00:00','g1'),
 ('newest_unproven','work','{}','ready','2026-10-01T00:00:00+00:00','g1'),
 ('moved_since','work','{}','ready','2026-10-01T00:00:00+00:00','g1'),
 ('retired','work','{}','ready','2026-10-01T00:00:00+00:00','g1'),
 ('no_uid','work','{}','ready','2026-10-01T00:00:00+00:00','g1'),
 ('no_target_epoch','work','{}','ready','2026-10-01T00:00:00+00:00','g1'),
 ('absent','work','{}','ready','2026-10-01T00:00:00+00:00','g1');
INSERT INTO placements(account,message_id,source_folder,home_folder,home_epoch,home_uid,location_state) VALUES
 ('work','proven','INBOX','News',7,3,'known'),
 ('work','later_failure','INBOX','News',7,10,'known'),
 ('work','no_copyuid','INBOX','News',7,4,'known'),
 ('work','newest_unproven','INBOX','News',7,5,'known'),
 ('work','moved_since','INBOX','News',7,9,'known'),
 ('work','retired','INBOX','Old',8,2,'known'),
 ('work','no_uid','INBOX','News',7,NULL,'known'),
 ('work','no_target_epoch','INBOX','News',7,11,'known'),
 ('work','absent','INBOX','News',7,12,'absent');
INSERT INTO filing_intents(account,message_id,kind,folder,epoch,uid,target,target_epoch,target_uid,state,created_at,updated_at) VALUES
 ('work','proven','move','INBOX',1,1,'News',7,3,'applied','t','t'),
 ('work','proven','flag','News',7,3,NULL,NULL,NULL,'applied','t','t'),
 ('work','later_failure','move','INBOX',1,2,'News',7,10,'applied','t','t'),
 ('work','later_failure','move','News',7,10,'Other',4,NULL,'failed','t','t'),
 ('work','no_copyuid','move','INBOX',1,3,'News',7,NULL,'applied','t','t'),
 ('work','newest_unproven','move','INBOX',1,4,'News',7,5,'applied','t','t'),
 ('work','newest_unproven','move','INBOX',1,4,'News',7,NULL,'applied','t','t'),
 ('work','moved_since','move','INBOX',1,5,'News',7,6,'applied','t','t'),
 ('work','retired','move','INBOX',1,6,'Old',8,2,'applied','t','t'),
 ('work','no_uid','move','INBOX',1,7,'News',7,8,'applied','t','t'),
 ('work','no_target_epoch','move','INBOX',1,8,'News',NULL,11,'applied','t','t'),
 ('work','absent','move','INBOX',1,9,'News',7,12,'applied','t','t');";

#[test]
fn a_fresh_database_is_at_the_latest_schema() {
    let (_d, s) = store();
    assert_eq!(s.schema_version().unwrap(), LATEST);
}

#[test]
fn migration_v6_fills_filed_homes_only_from_a_proven_newest_move() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("db");
    drop(Store::open(&path).unwrap());
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(DOWNGRADE_TO_V5).unwrap();
    db.execute_batch(V5_ROWS).unwrap();
    drop(db);
    let s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().unwrap(), LATEST);
    let filed = |id: &str| {
        let p = s.placement("work", id).unwrap().unwrap();
        (p.filed_home_folder, p.filed_home_epoch, p.filed_home_uid)
    };
    assert_eq!(
        filed("proven"),
        (Some("News".to_string()), Some(7), Some(3))
    );
    assert_eq!(
        filed("later_failure"),
        (Some("News".to_string()), Some(7), Some(10)),
        "the newest applied move counts, not a later failed one"
    );
    for id in [
        "no_copyuid",
        "newest_unproven",
        "moved_since",
        "retired",
        "no_uid",
        "no_target_epoch",
        "absent",
    ] {
        assert_eq!(filed(id), (None, None, None), "{id}");
    }
    assert!(s.placements("work").unwrap().iter().all(|p| !p.refile_once));
    assert!(s
        .intents("work", false)
        .unwrap()
        .iter()
        .all(|i| !i.consumes_refile));
    let drains: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM folders WHERE drain_until_uid IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(drains, 0, "every retired folder starts frozen");
}

#[test]
fn the_filed_home_is_stored_only_while_it_is_the_known_home() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    let id = placed(&mut s, "News", 3, 7, "a");
    let fresh = s.placement("work", &id).unwrap().unwrap();
    assert!(!fresh.refile_once);
    assert_eq!(fresh.filed_home_folder, None);
    assert!(!fresh.at_filed_home());
    let mut p = fresh.clone();
    p.refile_once = true;
    p.filed_home_folder = Some("News".into());
    p.filed_home_epoch = Some(3);
    p.filed_home_uid = Some(7);
    assert!(s.save_placement(&p, None).unwrap());
    let stored = s.placement("work", &id).unwrap().unwrap();
    assert_eq!(stored, p);
    assert!(stored.at_filed_home());

    let mut elsewhere = stored.clone();
    elsewhere.home_uid = Some(8);
    assert!(s.save_placement(&elsewhere, None).unwrap());
    let stored = s.placement("work", &id).unwrap().unwrap();
    assert_eq!(
        (
            stored.filed_home_folder.as_deref(),
            stored.filed_home_epoch,
            stored.filed_home_uid
        ),
        (None, None, None),
        "another home never carries the filed home along"
    );
    assert!(stored.refile_once, "the mark is written as given");

    let mut absent = p.clone();
    absent.location_state = LocationState::Absent;
    assert!(s.save_placement(&absent, None).unwrap());
    assert_eq!(
        s.placement("work", &id).unwrap().unwrap().filed_home_folder,
        None,
        "only a known home is a filed home"
    );
}

#[test]
fn an_epoch_reset_clears_the_filed_homes_in_that_folder_only() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    let a = placed(&mut s, "News", 3, 7, "a");
    let b = placed(&mut s, "Other", 5, 2, "b");
    file_home(&mut s, &a);
    file_home(&mut s, &b);
    s.checkpoint("work", "News", &snap(4, 9)).unwrap();
    assert_eq!(
        s.placement("work", &a).unwrap().unwrap().filed_home_folder,
        None
    );
    assert_eq!(
        s.placement("work", &b)
            .unwrap()
            .unwrap()
            .filed_home_folder
            .as_deref(),
        Some("Other")
    );
}

#[test]
fn a_refile_claim_records_that_it_consumes_the_mark() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    let id = placed(&mut s, "Other", 5, 1, "a");
    let action = Action::Move {
        message_id: id.clone(),
        from: Locator {
            folder: "Other".into(),
            epoch: 5,
            uid: 1,
        },
        to: "Updates".into(),
        desired_rev: 0,
        consumes_eligible: false,
    };
    let refile = s
        .claim_move_with("work", &action, (9, 1), "b1", NOW, true)
        .unwrap()
        .unwrap();
    let intent = s.intent(refile).unwrap().unwrap();
    assert!(intent.consumes_refile);
    assert_eq!(
        (intent.target_epoch, intent.target_uid_next),
        (Some(9), Some(1))
    );
    s.update_intent(refile, "superseded", IntentPatch::default(), NOW)
        .unwrap();
    let plain = s
        .claim_move("work", &action, 9, 1, "b2", NOW)
        .unwrap()
        .unwrap();
    assert!(!s.intent(plain).unwrap().unwrap().consumes_refile);
}
