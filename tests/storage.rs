use mailtriage::{
    config,
    domain::{MailboxSnapshot, SourceEnvelope},
    normalize,
    service::{ListOptions, Service},
    store::Store,
};
use serde_json::json;
use std::fs;

fn setup() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let mut c = config::default_config();
    c.policy.review_mode = false;
    config::save(&path, &c).unwrap();
    (dir, path)
}
fn mail(subject: &str) -> Vec<u8> {
    format!("From: Alex <alex@example.com>\r\nTo: work@example.com\r\nSubject: {subject}\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nPlease reply when convenient.").into_bytes()
}
fn ingest(s: &mut Service, subject: &str) -> String {
    s.classify("work", &mail(subject), "rfc822").unwrap()["item"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn persistence_dedup_overrides_and_done_survive_reclassification() {
    let (_d, p) = setup();
    let mut s = Service::open(&p).unwrap();
    let id = ingest(&mut s, "Question");
    assert_eq!(id, ingest(&mut s, "Question"));
    s.correct(
        "work",
        &id,
        json!({"urgency":"high","action_required":true}),
        None,
    )
    .unwrap();
    s.review("work", &id, true).unwrap();
    s.reclassify("work", None, false, 100).unwrap();
    drop(s);
    let mut s = Service::open(&p).unwrap();
    let item = &s.read("work", &id).unwrap()["item"];
    assert_eq!(item["classification"]["urgency"], "high");
    assert_eq!(item["review_state"], "done");
    assert_eq!(item["attention"], false);
    s.review("work", &id, false).unwrap();
    assert_eq!(s.list("work", ListOptions::default()).unwrap()["total"], 1);
}
#[test]
fn account_binding_and_reads_are_scoped() {
    let (_d, p) = setup();
    let mut c = config::load(&p).unwrap();
    let mut second = c.accounts["work"].clone();
    second.identity = "second@example.com".into();
    c.accounts.insert("other".into(), second);
    config::save(&p, &c).unwrap();
    let mut s = Service::open(&p).unwrap();
    let id = ingest(&mut s, "Scoped");
    assert!(s.read("other", &id).is_err());
    assert_eq!(s.list("other", ListOptions::default()).unwrap()["total"], 0);
    drop(s);
    let mut c = config::load(&p).unwrap();
    c.accounts.get_mut("work").unwrap().identity = "rebound@example.com".into();
    config::save(&p, &c).unwrap();
    assert!(Service::open(&p)
        .unwrap()
        .list("work", ListOptions::default())
        .is_err());
}
#[test]
fn cursor_filters_and_revision_conflicts() {
    let (_d, p) = setup();
    let mut s = Service::open(&p).unwrap();
    let first = ingest(&mut s, "One");
    ingest(&mut s, "Two");
    ingest(&mut s, "Three");
    let opts = ListOptions {
        view: "all".into(),
        limit: 1,
        ..Default::default()
    };
    let page = s.list("work", opts.clone()).unwrap();
    assert_eq!(page["total"], 3);
    let cursor = page["next_cursor"].as_str().unwrap().to_string();
    let next = s
        .list(
            "work",
            ListOptions {
                cursor: Some(cursor.clone()),
                ..opts.clone()
            },
        )
        .unwrap();
    assert_ne!(page["items"][0]["id"], next["items"][0]["id"]);
    assert!(s
        .list(
            "work",
            ListOptions {
                view: "attention".into(),
                cursor: Some(cursor.clone()),
                ..opts.clone()
            }
        )
        .is_err());
    s.review("work", &first, true).unwrap();
    assert!(s
        .list(
            "work",
            ListOptions {
                cursor: Some(cursor),
                ..opts
            }
        )
        .is_err());
}
#[test]
fn category_labels_preserve_revision_definitions_invalidate_and_manual_removal_refused() {
    let (_d, p) = setup();
    let mut s = Service::open(&p).unwrap();
    let id = ingest(&mut s, "Taxonomy");
    let mut cats = s.config.accounts["work"].categories.clone();
    cats[0].name = "Conversations".into();
    let result = s.apply_categories("work", cats.clone()).unwrap();
    assert_eq!(result["taxonomy_revision"], 1);
    assert_eq!(
        s.read("work", &id).unwrap()["item"]["classification"]["state"],
        "ready"
    );
    cats[0].description = "Changed meaning".into();
    assert_eq!(
        s.apply_categories("work", cats.clone()).unwrap()["taxonomy_revision"],
        2
    );
    assert_eq!(
        s.read("work", &id).unwrap()["item"]["classification"]["state"],
        "pending"
    );
    s.correct("work", &id, json!({"category_id":cats[0].id}), None)
        .unwrap();
    cats.remove(0);
    assert!(s.apply_categories("work", cats).is_err());
}
#[test]
fn durable_discovery_epoch_reset_alias_and_stale_generation() {
    let d = tempfile::tempdir().unwrap();
    let mut st = Store::open(&d.path().join("db")).unwrap();
    st.ensure_account("work", "identity", "g1").unwrap();
    let snap = MailboxSnapshot {
        uid_validity: 5,
        uid_next: 3,
    };
    assert_eq!(st.checkpoint("work", "INBOX", &snap).unwrap(), 0);
    let env = SourceEnvelope {
        uid: 1,
        subject: "Pending".into(),
        from: vec![],
        sent_at: None,
        ..Default::default()
    };
    st.stage("work", "INBOX", 5, 2, &[env], "g1", true).unwrap();
    assert_eq!(st.checkpoint("work", "INBOX", &snap).unwrap(), 2);
    let candidate = st.records("work").unwrap()[0].id.clone();
    assert!(st.lease(&candidate, "g1", 30).unwrap());
    assert!(!st.lease(&candidate, "g1", 30).unwrap());
    let msg = normalize::rfc822(&mail("same"), 1000).unwrap();
    let canonical = st.ingest("work", &msg, "g1").unwrap();
    assert_eq!(st.attach("work", &candidate, &msg).unwrap(), canonical);
    assert_eq!(
        st.record("work", &candidate).unwrap().unwrap().id,
        canonical
    );
    assert!(st.lease(&canonical, "g1", 30).unwrap());
    st.ensure_account("work", "identity", "g2").unwrap();
    assert!(!st
        .finish(&canonical, "g1", &json!({"state":"ready"}))
        .unwrap());
    assert_eq!(
        st.record("work", &canonical).unwrap().unwrap().status,
        "pending"
    );
    assert_eq!(
        st.checkpoint(
            "work",
            "INBOX",
            &MailboxSnapshot {
                uid_validity: 6,
                uid_next: 2
            }
        )
        .unwrap(),
        0
    );
    assert!(st.locator("work", &canonical).unwrap().is_none());
}
#[test]
fn failed_job_backoff_and_expired_lease_recovery() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut st = Store::open(&p).unwrap();
    st.ensure_account("work", "identity", "g").unwrap();
    let msg = normalize::rfc822(&mail("retry"), 1000).unwrap();
    let id = st.ingest("work", &msg, "g").unwrap();
    assert!(st.lease(&id, "g", 0).unwrap());
    drop(st);
    let mut st = Store::open(&p).unwrap();
    assert_eq!(st.queued("work", 10).unwrap(), vec![id.clone()]);
    assert!(st.lease(&id, "g", 30).unwrap());
    st.fail(&id, "g", "safe failure", 2).unwrap();
    assert!(st.queued("work", 10).unwrap().is_empty());
    assert_eq!(st.record("work", &id).unwrap().unwrap().status, "failed");
    assert_eq!(st.coverage("work").unwrap()["complete"], false);
    st.requeue("work", std::slice::from_ref(&id), "g").unwrap();
    assert_eq!(st.queued("work", 10).unwrap(), vec![id]);
}
#[test]
fn config_change_is_not_silently_overwritten() {
    let (_d, p) = setup();
    let mut s = Service::open(&p).unwrap();
    let cats = s.config.accounts["work"].categories.clone();
    let mut c = config::load(&p).unwrap();
    c.accounts.get_mut("work").unwrap().brief = "external edit".into();
    config::save(&p, &c).unwrap();
    assert!(s.apply_categories("work", cats).is_err());
    assert!(fs::read_to_string(&p).unwrap().contains("external edit"));
}

#[test]
fn stale_correction_is_rejected_and_done_is_not_automatically_requeued() {
    let (_d, p) = setup();
    let mut s = Service::open(&p).unwrap();
    let id = ingest(&mut s, "Concurrency");
    let old = s.store.record("work", &id).unwrap().unwrap().overrides;
    assert!(s
        .store
        .update_overrides("work", &id, &old, &json!({"urgency":"high"}))
        .unwrap());
    assert!(!s
        .store
        .update_overrides("work", &id, &old, &json!({"action_required":true}))
        .unwrap());
    s.review("work", &id, true).unwrap();
    let mut cats = s.config.accounts["work"].categories.clone();
    cats[0].description = "New definition".into();
    s.apply_categories("work", cats).unwrap();
    assert_eq!(
        s.read("work", &id).unwrap()["item"]["classification"]["state"],
        "ready"
    );
    assert_eq!(s.sync("work", 100).unwrap()["classified"], 0);
    s.review("work", &id, false).unwrap();
    assert_eq!(
        s.read("work", &id).unwrap()["item"]["classification"]["state"],
        "pending"
    );
}
#[test]
fn label_change_expires_existing_cursor() {
    let (_d, p) = setup();
    let mut s = Service::open(&p).unwrap();
    ingest(&mut s, "First");
    ingest(&mut s, "Second");
    let opts = ListOptions {
        view: "all".into(),
        limit: 1,
        ..Default::default()
    };
    let cursor = s.list("work", opts.clone()).unwrap()["next_cursor"]
        .as_str()
        .unwrap()
        .to_string();
    let mut cats = s.config.accounts["work"].categories.clone();
    cats[0].name = "Renamed".into();
    s.apply_categories("work", cats).unwrap();
    assert!(s
        .list(
            "work",
            ListOptions {
                cursor: Some(cursor),
                ..opts
            }
        )
        .is_err());
}
#[test]
fn source_identity_changes_reject_but_password_rotation_is_allowed() {
    use mailtriage::domain::HimalayaConfig;
    let (_d, p) = setup();
    let hpath = p.with_extension("toml");
    let source = |user: &str, password: &str| {
        format!("[accounts.work]\nimap.server = 'imaps://example.test'\nimap.sasl.plain.username = '{user}'\nimap.sasl.plain.password.raw = '{password}'\n")
    };
    fs::write(&hpath, source("alice", "first-secret")).unwrap();
    let mut c = config::load(&p).unwrap();
    c.accounts.get_mut("work").unwrap().himalaya = Some(HimalayaConfig {
        binary: "himalaya".into(),
        config: hpath.clone(),
        account: "work".into(),
        mailboxes: vec!["INBOX".into()],
        expected_version: "2.1.0".into(),
        timeout_seconds: 1,
        max_output_bytes: 4096,
    });
    config::save(&p, &c).unwrap();
    let mut s = Service::open(&p).unwrap();
    ingest(&mut s, "Binding");
    drop(s);
    fs::write(&hpath, source("alice", "rotated-secret")).unwrap();
    assert!(Service::open(&p)
        .unwrap()
        .list("work", ListOptions::default())
        .is_ok());
    fs::write(&hpath, source("bob", "rotated-secret")).unwrap();
    assert!(Service::open(&p)
        .unwrap()
        .list("work", ListOptions::default())
        .is_err());
}
#[test]
fn vanished_unfetched_source_remains_reviewable() {
    let (_d, p) = setup();
    let mut s = Service::open(&p).unwrap();
    s.list("work", ListOptions::default()).unwrap();
    let epoch = MailboxSnapshot {
        uid_validity: 1,
        uid_next: 2,
    };
    s.store.checkpoint("work", "INBOX", &epoch).unwrap();
    s.store
        .stage(
            "work",
            "INBOX",
            1,
            1,
            &[SourceEnvelope {
                uid: 1,
                subject: "Pending fetch".into(),
                from: vec![],
                sent_at: None,
                ..Default::default()
            }],
            "g",
            true,
        )
        .unwrap();
    s.store
        .reconcile_range("work", "INBOX", 1, 0, 1, &[], true)
        .unwrap();
    let result = s.list("work", ListOptions::default()).unwrap();
    assert_eq!(result["total"], 1);
    assert_eq!(result["items"][0]["classification"]["state"], "failed");
}

#[test]
fn reclassification_queues_remainder_for_later_sync() {
    let (_d, p) = setup();
    let mut s = Service::open(&p).unwrap();
    for subject in ["Batch1", "Batch2", "Batch3"] {
        ingest(&mut s, subject);
    }
    let result = s.reclassify("work", None, false, 1).unwrap();
    assert_eq!(result["matched"], 3);
    assert_eq!(result["reclassified"], 1);
    assert_eq!(result["pending"], 2);
    assert_eq!(s.sync("work", 100).unwrap()["classified"], 2);
}
#[test]
fn smaller_body_policy_is_enforced_on_cached_normalized_content() {
    let (_d, p) = setup();
    let mut s = Service::open(&p).unwrap();
    let id = ingest(&mut s, "Body limit");
    drop(s);
    let mut c = config::load(&p).unwrap();
    c.policy.max_body_chars = 5;
    config::save(&p, &c).unwrap();
    let mut s = Service::open(&p).unwrap();
    s.sync("work", 100).unwrap();
    let read = s.read("work", &id).unwrap();
    assert_eq!(read["item"]["classification"]["state"], "uncertain");
    assert!(read["item"]["classification"]["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r == "input_incomplete"));
}
