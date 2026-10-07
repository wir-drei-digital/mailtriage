//! Refile spec "Command" (the preview) and "Visibility".
mod common;
mod refile_support;
use common::{mail, Harness};
use mailtriage::{
    domain::FilingMode::{Live, Off},
    engine::fake::{FakeOp, Fault},
    filing::{transitions, FolderRecord},
};
use refile_support::{
    add_category, add_updates, code, correspondence_category, deliver_filed,
    filed_in_other_now_updates, id_of, located, mark, opts, placement, preview, remove_category,
    updates_category, without,
};
use serde_json::{json, Value};

/// Every engine call but the binding check (the account binding reads the
/// engine's identity; nothing else may reach the mailbox).
fn mailbox_calls(h: &Harness) -> usize {
    h.fake
        .calls()
        .iter()
        .filter(|c| *c != "binding_identity")
        .count()
}

fn retired_record(native: &str, configured: &str) -> FolderRecord {
    FolderRecord {
        account: "work".into(),
        native: native.into(),
        configured: Some(configured.into()),
        category_id: Some("deals".into()),
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
fn the_preview_lists_a_changed_category_with_its_target_and_folder() {
    let h = Harness::new(Live);
    let id = filed_in_other_now_updates(&h);
    let calls = mailbox_calls(&h);
    let v = preview(&h, None, None);
    assert_eq!(mailbox_calls(&h), calls, "no mailbox calls");
    assert_eq!(
        v["candidates"],
        json!([{"id": id, "folder": "Other", "target": "Updates", "category": "updates", "reason": "category_changed"}])
    );
    assert_eq!(
        (v["total"].clone(), v["waiting"].clone()),
        (json!(1), json!(0))
    );
    assert_eq!(
        v["folders"],
        json!([{"folder": "Other", "native": "Other", "retired": false, "candidates": 1, "waiting": 0}])
    );
    let skipped = v["skipped"].as_object().unwrap();
    assert_eq!(skipped.len(), 12);
    assert!(skipped.values().all(|n| *n == 0));
    assert_eq!(v["mode"], "live");
    assert!(!placement(&h, &id).refile_once, "a preview marks nothing");
}

#[test]
fn mail_not_yet_classified_again_is_waiting_in_its_folder() {
    let h = Harness::new(Live);
    without(&h, &["updates"]);
    h.sync();
    deliver_filed(&h, "u", "System update", "Version 2 is out");
    add_category(&h, updates_category()); // stale until the next pass
    let v = preview(&h, None, None);
    assert_eq!(
        (v["total"].clone(), v["waiting"].clone()),
        (json!(0), json!(1))
    );
    assert_eq!(
        v["folders"],
        json!([{"folder": "Other", "native": "Other", "retired": false, "candidates": 0, "waiting": 1}])
    );
    assert_eq!(
        preview(&h, Some("updates"), None)["waiting"],
        1,
        "--category still reports it"
    );
}

#[test]
fn a_target_folder_not_created_yet_is_unusable() {
    // Spec rule 7: the target must be a category folder in state `ok`. The
    // live pass after `categories apply` fails to create Updates (it stays
    // `error`, so a later pass would create it) but classifies `u` again.
    let h = Harness::new(Live);
    without(&h, &["updates"]);
    h.sync();
    let id = deliver_filed(&h, "u", "System update", "Version 2 is out");
    add_category(&h, updates_category());
    h.fake.inject(FakeOp::Create, Fault::ErrorBefore);
    h.sync();
    let folder = h
        .service()
        .store
        .folder_record("work", "Updates")
        .unwrap()
        .unwrap();
    assert_eq!(folder.state, "error", "the create failed");
    let v = preview(&h, None, None);
    assert_eq!(
        (v["total"].clone(), v["waiting"].clone()),
        (json!(0), json!(0))
    );
    assert_eq!(v["candidates"], json!([]));
    assert_eq!(v["skipped"]["target_unusable"], 1);
    // The next pass creates Updates: the same classification now moves it.
    h.sync();
    let v = preview(&h, None, None);
    assert_eq!(v["candidates"][0]["id"], json!(id));
    assert_eq!(
        (v["total"].clone(), v["skipped"]["target_unusable"].clone()),
        (json!(1), json!(0))
    );
}

#[test]
fn folder_names_match_configured_and_native_names() {
    let h = Harness::new(Live);
    h.fake.set_prefix("INBOX.", '.');
    let id = filed_in_other_now_updates(&h);
    assert_eq!(located(&h, "u").0, "INBOX.Other");
    let by_name = preview(&h, None, Some("Other"));
    assert_eq!(by_name, preview(&h, None, Some("INBOX.Other")));
    assert_eq!(
        by_name["candidates"],
        json!([{"id": id, "folder": "INBOX.Other", "target": "INBOX.Updates", "category": "updates", "reason": "category_changed"}])
    );
    assert_eq!(
        by_name["folders"],
        json!([{"folder": "Other", "native": "INBOX.Other", "retired": false, "candidates": 1, "waiting": 0}])
    );
    assert_eq!(
        preview(&h, None, Some("INBOX.Updates"))["total"],
        0,
        "another folder"
    );
    for bad in ["INBOX", "Nope", "inbox.other"] {
        let e = h
            .service()
            .filing_refile("work", opts(None, Some(bad)))
            .unwrap_err();
        assert_eq!(code(&e), Some(2), "{bad}");
    }
}

#[test]
fn a_retired_folder_keeps_its_configured_name() {
    let h = Harness::new(Live);
    h.sync();
    let id = deliver_filed(&h, "n", "Weekly newsletter", "Our newsletter");
    remove_category(&h, "newsletters");
    h.sync(); // Newsletters is retired; n is classified again into `other`
    let v = preview(&h, None, Some("Newsletters"));
    assert_eq!(
        v["candidates"],
        json!([{"id": id, "folder": "Newsletters", "target": "Other", "category": "other", "reason": "folder_retired"}])
    );
    assert_eq!(
        v["folders"],
        json!([{"folder": "Newsletters", "native": "Newsletters", "retired": true, "candidates": 1, "waiting": 0}])
    );
}

#[test]
fn a_native_name_wins_and_a_shared_configured_name_is_refused() {
    let h = Harness::new(Live);
    h.sync();
    let mut s = h.service();
    for native in ["INBOX.Deals", "Archive.Deals"] {
        s.store
            .save_folder(&retired_record(native, "Deals"))
            .unwrap();
    }
    // Recorded under the prefix that came later, configured as `Promotions`.
    s.store
        .save_folder(&retired_record("INBOX.Promotions", "Promotions"))
        .unwrap();
    let e = s
        .filing_refile("work", opts(None, Some("Deals")))
        .unwrap_err();
    assert_eq!(code(&e), Some(2));
    assert!(e.to_string().contains("Archive.Deals, INBOX.Deals"), "{e}");
    assert_eq!(
        s.filing_refile("work", opts(None, Some("INBOX.Deals")))
            .unwrap()["total"],
        0
    );
    // `Promotions` is a native name (the category's own folder): no question asked.
    assert!(s
        .filing_refile("work", opts(None, Some("Promotions")))
        .is_ok());
}

#[test]
fn mail_mailtriage_did_not_put_there_is_never_a_candidate() {
    // Moved back into its folder by the user, and delivered straight into it.
    let h = Harness::new(Live);
    without(&h, &["updates"]);
    h.sync();
    deliver_filed(&h, "a", "System update", "Version 2 is out");
    let (folder, uid) = located(&h, "a");
    h.fake.client_move(&folder, uid, &folder);
    h.fake
        .deliver("Other", &mail("b", "Server update", "Version 3 is out"));
    h.sync();
    h.sync();
    add_updates(&h);
    let v = preview(&h, None, None);
    assert_eq!(v["total"], 0);
    assert_eq!(v["skipped"]["not_filed_by_mailtriage"], 2);

    // Placed by a rescan after an epoch reset.
    let h = Harness::new(Live);
    without(&h, &["updates"]);
    h.sync();
    deliver_filed(&h, "c", "System update", "Version 2 is out");
    h.fake.reset_epoch("Other");
    for _ in 0..3 {
        h.sync();
    }
    add_updates(&h);
    let v = preview(&h, None, None);
    assert_eq!(
        (
            v["total"].clone(),
            v["skipped"]["not_filed_by_mailtriage"].clone()
        ),
        (json!(0), json!(1))
    );

    // Moved without COPYUID, so applied by recovery.
    let h = Harness::new(Live);
    h.fake.set_capabilities(true, false, true);
    without(&h, &["updates"]);
    h.sync();
    let id = deliver_filed(&h, "d", "System update", "Version 2 is out");
    h.sync();
    add_updates(&h);
    assert_eq!(placement(&h, &id).filed_by.as_deref(), Some("mailtriage"));
    let v = preview(&h, None, None);
    assert_eq!(
        (
            v["total"].clone(),
            v["skipped"]["not_filed_by_mailtriage"].clone()
        ),
        (json!(0), json!(1))
    );
}

#[test]
fn a_quarantined_copy_that_became_the_home_is_not_a_candidate() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("m", "Another newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::ErrorBefore);
    h.sync();
    // x lands in Newsletters while INBOX is recreated: a suspected race.
    h.fake
        .deliver("Newsletters", &mail("x", "Invoice", "Payment due"));
    h.fake.reset_epoch("INBOX");
    h.service().sync("work", 1).unwrap();
    let mut s = h.service();
    transitions::release_folder(&mut s.store, "work", "INBOX", &mailtriage::store::now()).unwrap();
    for _ in 0..3 {
        h.sync();
    }
    let x = id_of(&h, "x");
    assert_eq!(
        placement(&h, &x).blocked_reason.as_deref(),
        Some("quarantined")
    );
    h.service()
        .filing_retry("work", mailtriage::service::RetryTarget::Message(x.clone()))
        .unwrap();
    // x, classified `transactions`, sits in Newsletters: it would be a candidate.
    let v = preview(&h, None, None);
    assert_eq!(
        (
            v["total"].clone(),
            v["skipped"]["not_filed_by_mailtriage"].clone()
        ),
        (json!(0), json!(1))
    );
}

#[test]
fn an_outstanding_request_for_a_removed_category_is_explicit_target() {
    let h = Harness::new(Live);
    h.sync();
    let id = deliver_filed(&h, "n", "Weekly newsletter", "Our newsletter");
    let mut s = h.service();
    s.correct("work", &id, json!({"category_id": "promotions"}), None)
        .unwrap();
    // Clearing the correction leaves a request for the model's `newsletters`.
    s.correct("work", &id, json!({}), Some("category_id"))
        .unwrap();
    drop(s);
    remove_category(&h, "newsletters");
    let v = preview(&h, None, None);
    assert_eq!(v["total"], 0);
    assert_eq!(v["skipped"]["explicit_target"], 1);
}

#[test]
fn corrected_pinned_done_and_inbox_bound_mail_is_skipped() {
    let h = Harness::new(Live);
    without(&h, &["updates", "correspondence"]);
    h.sync();
    let corrected = deliver_filed(&h, "u1", "System update 1", "Version 2 is out");
    let pinned = deliver_filed(&h, "u2", "System update 2", "Version 2 is out");
    let done = deliver_filed(&h, "u3", "System update 3", "Version 2 is out");
    deliver_filed(&h, "l", "Lunch", "See you at noon");
    add_category(&h, correspondence_category());
    add_updates(&h);
    let mut s = h.service();
    s.correct(
        "work",
        &corrected,
        json!({"category_id": "transactions"}),
        None,
    )
    .unwrap();
    s.filing_pin("work", &pinned).unwrap();
    s.review("work", &done, true).unwrap();
    drop(s);
    let v = preview(&h, None, None);
    assert_eq!(v["total"], 0);
    for key in ["corrected", "pinned", "done", "target_inbox_or_source"] {
        assert_eq!(v["skipped"][key], 1, "{key}");
    }
    let only_updates = preview(&h, Some("updates"), None);
    assert_eq!(
        (
            only_updates["skipped"]["pinned"].clone(),
            only_updates["skipped"]["target_inbox_or_source"].clone()
        ),
        (json!(1), json!(0)),
        "--category counts the skips of that category only"
    );
}

#[test]
fn status_counts_marks_and_candidates() {
    let h = Harness::new(Live);
    let id = filed_in_other_now_updates(&h);
    let st = h.service().filing_status("work").unwrap();
    assert_eq!(
        (st["refile_marked"].clone(), st["refile_candidates"].clone()),
        (json!(0), json!(1))
    );
    mark(&h, &id);
    assert_eq!(
        h.service().filing_status("work").unwrap()["refile_marked"],
        1
    );
}

#[test]
fn categories_apply_hints_at_refile_while_filing_is_on() {
    let h = Harness::new(Live);
    h.sync();
    let categories = h.service().config.accounts["work"].categories.clone();
    let out = h
        .service()
        .apply_categories("work", categories.clone())
        .unwrap();
    assert_eq!(
        out["hint"],
        "Open mail is classified again over the next sync passes. Once they have run, `mailtriage filing refile --account work` shows which filed mail would move."
    );
    let h = Harness::new(Off);
    let out = h.service().apply_categories("work", categories).unwrap();
    assert_eq!(out["hint"], Value::Null);
}

#[test]
fn bad_arguments_exit_2() {
    let h = Harness::new(Live);
    h.sync();
    let mut s = h.service();
    let cases = [
        ("work", opts(Some("nope"), None)),
        ("work", opts(None, Some("INBOX"))),
        ("work", with_limit(0)),
        ("work", with_limit(501)),
        ("nope", opts(None, None)),
    ];
    for (account, o) in cases {
        let e = s.filing_refile(account, o.clone()).unwrap_err();
        assert_eq!(code(&e), Some(2), "{account} {o:?}");
    }
}

fn with_limit(limit: usize) -> mailtriage::service::RefileOptions {
    mailtriage::service::RefileOptions {
        limit,
        ..opts(None, None)
    }
}
