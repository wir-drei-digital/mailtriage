//! Every sync pass that holds the account lock records how it ended.
mod common;
use common::Harness;
use mailtriage::{domain::FilingMode, store::Store};

#[test]
fn each_pass_records_a_heartbeat() {
    let h = Harness::new(FilingMode::DryRun);
    assert!(h.service().store.heartbeat("work").unwrap().is_none());
    h.sync();
    let beat = h.service().store.heartbeat("work").unwrap().unwrap();
    assert_eq!(beat["exit_code"], 0);
    assert_eq!(beat["partial"], false);
    assert_eq!(beat["mode"], "dry_run");
    assert!(chrono::DateTime::parse_from_rfc3339(beat["finished_at"].as_str().unwrap()).is_ok());

    h.fake.fail_with_config_changed("version");
    assert!(h.service().sync("work", 100).is_err());
    let beat = h.service().store.heartbeat("work").unwrap().unwrap();
    assert_eq!(beat["exit_code"], 5);
    assert_eq!(beat["partial"], false);
}

#[test]
fn schema_5_adds_the_heartbeat_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let store = Store::open(&path).unwrap();
    store.record_heartbeat("work", true, 4, "off").unwrap();
    store.record_heartbeat("work", false, 0, "live").unwrap();
    let beat = store.heartbeat("work").unwrap().unwrap();
    assert_eq!(
        (
            beat["partial"].clone(),
            beat["exit_code"].clone(),
            beat["mode"].clone()
        ),
        (false.into(), 0.into(), "live".into())
    );
    drop(store);
    let version: u32 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 5);
}
