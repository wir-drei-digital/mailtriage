mod common;
use common::{mail, Harness};
use mailtriage::domain::FilingMode;

#[test]
fn sync_through_fake_engine_with_filing_off_is_unchanged() {
    let h = Harness::new(FilingMode::Off);
    h.fake
        .deliver("INBOX", &mail("a", "Weekly newsletter", "Our newsletter"));
    let out = h.sync();
    assert_eq!(out["discovered"], 1);
    assert_eq!(out["classified"], 1);
    assert_eq!(h.fake.write_calls(), 0);
    assert!(out.get("filing").is_none() || out["filing"]["mode"] == "off");
}

use mailtriage::domain::FilingMode::{DryRun, Live, Off};

fn folder_state(h: &Harness, native: &str) -> Option<String> {
    h.service()
        .store
        .folder_record("work", native)
        .unwrap()
        .map(|f| f.state)
}

#[test]
fn dry_run_creates_nothing_and_writes_nothing() {
    let h = Harness::new(DryRun);
    h.fake
        .deliver("INBOX", &mail("a", "Weekly newsletter", "Our newsletter"));
    let out = h.sync();
    assert_eq!(out["filing"]["mode"], "dry_run");
    assert_eq!(h.fake.write_calls(), 0);
    assert!(folder_state(&h, "Newsletters").is_none());
}

#[test]
fn live_creates_and_subscribes_category_folders_once() {
    let h = Harness::new(Live);
    h.sync();
    for name in [
        "Transactions",
        "Updates",
        "Newsletters",
        "Promotions",
        "Other",
    ] {
        assert_eq!(folder_state(&h, name).as_deref(), Some("ok"), "{name}");
        assert!(h.fake.subscribed(name), "{name}");
    }
    assert!(
        folder_state(&h, "Correspondence").is_none(),
        "INBOX-target category has no folder"
    );
    let creates = h
        .fake
        .calls()
        .iter()
        .filter(|c| c.starts_with("create "))
        .count();
    h.sync();
    assert_eq!(
        h.fake
            .calls()
            .iter()
            .filter(|c| c.starts_with("create "))
            .count(),
        creates
    );
}

#[test]
fn existing_folder_is_adopted_and_its_content_not_ingested() {
    let h = Harness::new(Live);
    h.fake.add_folder("Newsletters", &[]);
    for i in 0..3 {
        h.fake.deliver(
            "Newsletters",
            &mail(&format!("old{i}"), "Old newsletter", "newsletter"),
        );
    }
    h.sync();
    let f = h
        .service()
        .store
        .folder_record("work", "Newsletters")
        .unwrap()
        .unwrap();
    assert_eq!(f.origin.as_deref(), Some("adopted"));
    let all = h
        .service()
        .list(
            "work",
            mailtriage::service::ListOptions {
                view: "all".into(),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(all["total"], 0);
}

#[test]
fn special_use_and_unverified_folders_are_not_used() {
    let h = Harness::new(Live);
    h.fake.add_folder("Promotions", &["\\Junk"]);
    h.sync();
    assert_eq!(
        folder_state(&h, "Promotions").as_deref(),
        Some("special_use")
    );
    let h = Harness::new(Live);
    h.fake.set_capabilities(true, true, false);
    h.fake.add_folder("Updates", &[]);
    h.sync();
    assert_eq!(
        folder_state(&h, "Updates").as_deref(),
        Some("needs_confirmation")
    );
    assert_eq!(
        folder_state(&h, "Newsletters").as_deref(),
        Some("ok"),
        "created folders are verified"
    );
}

#[test]
fn personal_namespace_prefix_is_applied() {
    let h = Harness::new(Live);
    h.fake.set_prefix("INBOX.", '.');
    h.sync();
    assert_eq!(folder_state(&h, "INBOX.Newsletters").as_deref(), Some("ok"));
}

#[test]
fn category_folder_colliding_with_a_source_is_reported() {
    let h = Harness::new(Live);
    h.fake.add_folder("Newsletters", &[]);
    h.edit(|c| {
        if let Some(mailtriage::domain::EngineConfig::Himalaya(e)) =
            &mut c.accounts.get_mut("work").unwrap().engine
        {
            e.mailboxes.push("Newsletters".into());
        }
    });
    let out = h.sync();
    assert!(out["filing"]["problems"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p == "folder_collides_with_source:newsletters"));
}

#[test]
fn missing_move_capability_blocks_all_writes() {
    let h = Harness::new(Live);
    h.fake.set_capabilities(false, true, true);
    let out = h.sync();
    assert_eq!(h.fake.write_calls(), 0);
    assert!(out["filing"]["problems"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p == "move_unsupported"));
}

#[test]
fn enabling_on_large_existing_mailbox_moves_nothing_and_hydrates_in_batches() {
    let h = Harness::new(Off);
    h.fake.set_rich_discovery(false);
    for i in 0..250 {
        h.fake.deliver_at(
            "INBOX",
            &mail(
                &format!("m{i}"),
                "Can you review this newsletter?",
                "please review",
            ),
            "2026-01-01T00:00:00+00:00",
        );
    }
    let mut s = h.service();
    for _ in 0..3 {
        s.sync("work", 100).unwrap();
    }
    h.set_mode(Live);
    let first = h.sync();
    assert_eq!(first["filing"]["hydrated"], 100);
    assert_eq!(h.sync()["filing"]["hydrated"], 100);
    assert_eq!(h.sync()["filing"]["hydrated"], 50);
    assert!(h
        .fake
        .calls()
        .iter()
        .all(|c| !c.starts_with("move ") && !c.starts_with("flag ")));
    let placements = h.service().store.placements("work").unwrap();
    assert_eq!(placements.len(), 250);
}

// Supplementary coverage: failure, retirement, rescan and denylist paths.

fn set_folder(h: &Harness, category: &str, folder: &str) {
    h.edit(|c| {
        let account = c.accounts.get_mut("work").unwrap();
        let cat = account
            .categories
            .iter_mut()
            .find(|x| x.id == category)
            .unwrap();
        cat.folder = Some(folder.into());
    });
}

#[test]
fn failed_create_marks_the_folder_error_and_retries_next_pass() {
    use mailtriage::engine::fake::{FakeOp, Fault};
    let h = Harness::new(Live);
    h.fake.inject(FakeOp::Create, Fault::ErrorBefore);
    let out = h.sync();
    assert_eq!(out["partial"], true);
    assert_eq!(out["filing"]["errors"], 1);
    assert_eq!(folder_state(&h, "Transactions").as_deref(), Some("error"));
    assert_eq!(h.sync()["filing"]["errors"], 0);
    assert_eq!(folder_state(&h, "Transactions").as_deref(), Some("ok"));
    assert!(h.fake.subscribed("Transactions"));
}

#[test]
fn renamed_category_retires_its_old_folder_and_stops_watching_it() {
    let h = Harness::new(Live);
    h.sync();
    set_folder(&h, "newsletters", "News");
    h.sync();
    assert_eq!(folder_state(&h, "Newsletters").as_deref(), Some("retired"));
    assert_eq!(folder_state(&h, "News").as_deref(), Some("ok"));
    let watched = |h: &Harness| {
        h.fake
            .calls()
            .iter()
            .filter(|c| *c == "snapshot Newsletters")
            .count()
    };
    let before = watched(&h);
    h.sync();
    assert_eq!(
        watched(&h),
        before,
        "an unreferenced retired folder is not watched"
    );
}

#[test]
fn denylisted_category_folder_is_never_created() {
    let h = Harness::new(Live);
    set_folder(&h, "newsletters", "Junk");
    let out = h.sync();
    assert!(!h.fake.calls().contains(&"create Junk".to_string()));
    assert_eq!(folder_state(&h, "Junk").as_deref(), Some("special_use"));
    assert_eq!(out["filing"]["errors"], 0);
    h.sync();
    assert_eq!(
        folder_state(&h, "Junk").as_deref(),
        Some("special_use"),
        "an absent denylisted folder never decays to missing"
    );
    assert!(!h.fake.calls().contains(&"create Junk".to_string()));
    let kinds: Vec<String> = h
        .service()
        .store
        .events("work", None, 100)
        .unwrap()
        .iter()
        .filter(|e| e["folder"] == "Junk")
        .map(|e| e["kind"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(kinds, vec!["folder_special_use".to_string()]);
}

#[test]
fn mail_engine_configuration_change_aborts_the_pass_with_exit_5() {
    use mailtriage::service::ServiceError;
    for from_call in ["list", "discover INBOX"] {
        let h = Harness::new(Live);
        h.fake.deliver("INBOX", &mail("a", "Hello", "hello"));
        h.fake.fail_with_config_changed(from_call);
        let e = h.service().sync("work", 100).unwrap_err();
        let code = e.downcast_ref::<ServiceError>().map(|s| s.code);
        assert_eq!(code, Some(5), "{from_call}");
        let state = h.service().store.filing_state("work").unwrap();
        assert!(state.last_pass.is_none(), "{from_call}: no summary stored");
        assert!(
            !h.fake.calls().iter().any(|c| c.starts_with("fetch ")),
            "{from_call}: the pass stopped before fetching"
        );
    }
}

#[test]
fn alias_conflict_stops_fetching_from_that_folder() {
    let h = Harness::new(Live);
    h.fake.add_folder("Work", &[]);
    h.edit(|c| {
        if let Some(mailtriage::domain::EngineConfig::Himalaya(e)) =
            &mut c.accounts.get_mut("work").unwrap().engine
        {
            e.mailboxes.push("Work".into());
        }
    });
    h.fake.deliver("INBOX", &mail("first", "First", "first"));
    h.fake.deliver("Work", &mail("second", "Second", "second"));
    // Limit 1: both folders stage one message, only the older one is fetched.
    let first = h.service().sync("work", 1).unwrap();
    assert_eq!(
        (first["discovered"].as_u64(), first["fetched"].as_u64()),
        (Some(2), Some(1))
    );
    let fetched_from_work =
        |h: &Harness| h.fake.calls().iter().any(|c| c.starts_with("fetch Work"));
    assert!(!fetched_from_work(&h));
    h.fake.set_alias_conflicts(&["Work"]);
    let blocked = h.service().sync("work", 100).unwrap();
    assert!(blocked["filing"]["problems"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p == "alias_conflict:Work"));
    assert!(
        !fetched_from_work(&h),
        "no fetch through a conflicting alias"
    );
    assert_eq!(blocked["fetched"], 0);
    assert_eq!(blocked["pending"], 1, "the message stays queued");
    h.fake.set_alias_conflicts(&[]);
    let freed = h.service().sync("work", 100).unwrap();
    assert!(fetched_from_work(&h));
    assert_eq!(
        freed["fetched"], 1,
        "fetched without a burned attempt once the conflict is gone"
    );
}

#[test]
fn category_folder_epoch_reset_rescans_only_known_mail() {
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
        .deliver("Newsletters", &mail("moved", "Moved by the user", "hello"));
    assert_eq!(h.sync()["discovered"], 1);
    h.fake.reset_epoch("Newsletters");
    let out = h.sync();
    assert_eq!(out["discovered"], 1, "only the rescan-set member is staged");
    let all = h
        .service()
        .list(
            "work",
            mailtriage::service::ListOptions {
                view: "all".into(),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        all["total"], 1,
        "the rescanned copy merged into the known message"
    );
    let f = h
        .service()
        .store
        .folder_record("work", "Newsletters")
        .unwrap()
        .unwrap();
    assert_eq!(f.rescan_epoch, Some(h.fake.epoch("Newsletters")));
    assert!(f.rescan_complete);
}
