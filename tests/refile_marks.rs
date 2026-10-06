//! Refile spec "Each pass" (mark upkeep) and "Intents".
mod common;
mod refile_support;
use common::{mail, Harness};
use mailtriage::{
    domain::FilingMode::Live,
    engine::fake::{FakeOp, Fault},
    filing::{
        apply, inputs, observe, planner,
        refile::{rules, upkeep},
        FilingSummary, PassContext,
    },
    service::RetryTarget,
};
use refile_support::{
    add_category, correspondence_category, deliver_filed, events, filed_in_other_now_updates,
    located, mark, moves_to, pause, placement, review_state, set_catch_all, updates_category,
    without,
};
use serde_json::json;

#[test]
fn upkeep_clears_marks_with_their_reason() {
    type Act = fn(&Harness, &str);
    let cases: [(&str, Act); 5] = [
        ("done", |h, id| {
            h.service().review("work", id, true).unwrap();
        }),
        ("pinned", |h, id| {
            h.service().filing_pin("work", id).unwrap();
        }),
        ("corrected", |h, id| {
            h.service()
                .correct("work", id, json!({"category_id": "transactions"}), None)
                .unwrap();
        }),
        ("multiple_copies", |h, _| {
            let (folder, uid) = located(h, "u");
            h.fake.client_copy(&folder, uid, "Transactions");
        }),
        ("not_filed_by_mailtriage", |h, _| {
            h.fake.reset_epoch("Other")
        }),
    ];
    for (reason, act) in cases {
        let h = Harness::new(Live);
        let id = filed_in_other_now_updates(&h);
        mark(&h, &id);
        act(&h, &id);
        for _ in 0..4 {
            h.sync();
        }
        assert!(!placement(&h, &id).refile_once, "{reason}");
        assert_eq!(events(&h, "refile_cleared")[0]["detail"]["reason"], reason);
        assert_eq!(moves_to(&h, "Updates"), 0, "{reason}");
    }
}

#[test]
fn absent_then_done_clears_the_mark() {
    let h = Harness::new(Live);
    let id = filed_in_other_now_updates(&h);
    let (folder, uid) = located(&h, "u");
    h.fake.client_delete(&folder, uid);
    mark(&h, &id);
    for _ in 0..3 {
        h.sync();
    }
    assert!(!placement(&h, &id).refile_once);
    assert_eq!(
        events(&h, "refile_cleared")[0]["detail"]["reason"],
        "not_filed_by_mailtriage"
    );
    assert_eq!(moves_to(&h, "Updates"), 0);
    assert_eq!(review_state(&h, &id), "done", "done inference still runs");
}

#[test]
fn a_stale_classification_keeps_the_mark_until_the_current_one_decides() {
    let h = Harness::new(Live);
    without(&h, &["updates"]);
    h.sync();
    for i in 0..3 {
        h.fake.deliver(
            "INBOX",
            &mail(&format!("f{i}"), &format!("Lunch {i}"), "See you at noon"),
        );
    }
    h.sync();
    let id = deliver_filed(&h, "u", "System update", "Version 2 is out");
    // Open mail is queued again; u comes after the three others.
    add_category(&h, updates_category());
    // Its stale classification still says `other`: its own folder.
    mark(&h, &id);
    for pass in 0..3 {
        h.service().sync("work", 1).unwrap();
        assert!(placement(&h, &id).refile_once, "pass {pass}");
        assert!(events(&h, "refile_cleared").is_empty(), "pass {pass}");
    }
    h.service().sync("work", 1).unwrap(); // u is classified again: `updates`
    h.sync();
    assert_eq!(located(&h, "u").0, "Updates");
    assert!(!placement(&h, &id).refile_once);
}

#[test]
fn a_client_correction_after_marking_wins() {
    let h = Harness::new(Live);
    let id = filed_in_other_now_updates(&h);
    mark(&h, &id);
    let (folder, uid) = located(&h, "u");
    h.fake.client_move(&folder, uid, "Transactions");
    h.sync();
    h.sync();
    assert_eq!(located(&h, "u").0, "Transactions");
    assert_eq!(moves_to(&h, "Updates"), 0);
    assert!(!placement(&h, &id).refile_once);
    assert_eq!(
        events(&h, "refile_cleared")[0]["detail"]["reason"],
        "corrected"
    );
}

#[test]
fn an_inbox_target_clears_the_mark() {
    let h = Harness::new(Live);
    without(&h, &["correspondence"]);
    h.sync();
    let id = deliver_filed(&h, "l", "Lunch", "See you at noon");
    assert_eq!(located(&h, "l").0, "Other");
    add_category(&h, correspondence_category());
    h.sync();
    mark(&h, &id);
    h.sync();
    h.sync();
    assert_eq!(located(&h, "l").0, "Other", "never refiled into INBOX");
    assert_eq!(moves_to(&h, "INBOX"), 0);
    assert_eq!(
        events(&h, "refile_cleared")[0]["detail"]["reason"],
        "target_inbox_or_source"
    );
}

#[test]
fn a_paused_target_or_home_keeps_the_mark_until_released() {
    for paused in ["Updates", "Other"] {
        let h = Harness::new(Live);
        let id = filed_in_other_now_updates(&h);
        pause(&h, paused);
        mark(&h, &id);
        h.sync();
        h.sync();
        assert!(placement(&h, &id).refile_once, "{paused}");
        assert_eq!(moves_to(&h, "Updates"), 0, "{paused}");
        h.service()
            .filing_retry("work", RetryTarget::Folder(paused.into()))
            .unwrap();
        for _ in 0..3 {
            h.sync();
        }
        assert_eq!(located(&h, "u").0, "Updates", "{paused}");
        assert!(!placement(&h, &id).refile_once, "{paused}");
    }
}

#[test]
fn done_between_planning_and_claim_supersedes_the_refile_intent() {
    let h = Harness::new(Live);
    let id = filed_in_other_now_updates(&h);
    mark(&h, &id);
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
    let input = inputs::plan_input(&s.store, &ctx, &map, false).unwrap();
    let gone = rules::gone(&s.store, "work", &map.listed).unwrap();
    let refile = rules::input(&s.store, "work", &cfg, &gone).unwrap();
    let plan = planner::plan_with_refile(&input, &refile);
    assert!(plan.refile_moves.contains(&id));
    s.store.review("work", &id, true).unwrap();
    apply::apply(&mut s.store, &ctx, &map, &plan, &mut summary).unwrap();
    assert_eq!(moves_to(&h, "Updates"), 0, "never dispatched");
    let intent = s.store.intents("work", false).unwrap().pop().unwrap();
    assert_eq!(
        (intent.state.as_str(), intent.consumes_refile),
        ("superseded", true)
    );
    assert_eq!(
        events(&h, "refile_cancelled")[0]["detail"]["reason"],
        "done"
    );
    drop(s);
    h.sync();
    assert!(
        !placement(&h, &id).refile_once,
        "upkeep clears the mark of done mail"
    );
    assert_eq!(moves_to(&h, "Updates"), 0);
}

#[test]
fn a_category_change_after_a_failed_dispatch_supersedes_and_retargets() {
    let h = Harness::new(Live);
    without(&h, &["correspondence"]);
    h.sync();
    let id = deliver_filed(&h, "l", "Lunch", "See you at noon"); // the catch-all: Other
    set_catch_all(&h, "updates");
    h.sync(); // now `updates`
    mark(&h, &id);
    h.fake.inject(FakeOp::Move, Fault::ErrorBefore);
    h.sync(); // the dispatch to Updates fails
    assert_eq!(moves_to(&h, "Updates"), 1);
    assert_eq!(located(&h, "l").0, "Other");
    set_catch_all(&h, "promotions");
    h.sync();
    h.sync();
    assert_eq!(located(&h, "l").0, "Promotions");
    assert_eq!(
        moves_to(&h, "Updates"),
        1,
        "the intent to Updates is never retried"
    );
    let refiles: Vec<(String, Option<String>)> = h
        .service()
        .store
        .intents("work", false)
        .unwrap()
        .into_iter()
        .filter(|i| i.consumes_refile)
        .map(|i| (i.state, i.target))
        .collect();
    assert_eq!(
        refiles,
        vec![
            ("superseded".to_string(), Some("Updates".to_string())),
            ("applied".to_string(), Some("Promotions".to_string())),
        ]
    );
    assert_eq!(
        events(&h, "refile_cancelled")[0]["detail"]["reason"],
        "waiting"
    );
    assert!(!placement(&h, &id).refile_once);
}

#[test]
fn a_changed_source_occurrence_supersedes_the_intent() {
    let h = Harness::new(Live);
    let id = filed_in_other_now_updates(&h);
    mark(&h, &id);
    h.fake.inject(FakeOp::Move, Fault::ErrorBefore);
    h.sync(); // the dispatch fails; the intent is uncertain
    h.fake.reset_epoch("Other"); // the source occurrence changes
    for _ in 0..6 {
        h.sync();
    }
    assert_eq!(moves_to(&h, "Updates"), 1, "never retried");
    let s = h.service();
    let intent = s
        .store
        .intents("work", false)
        .unwrap()
        .into_iter()
        .find(|i| i.consumes_refile)
        .unwrap();
    assert_eq!(intent.message_id, id);
    assert_eq!(intent.state, "superseded");
    assert_eq!(
        events(&h, "refile_cancelled")[0]["detail"]["reason"],
        "source_changed"
    );
}

#[test]
fn a_pass_without_a_folder_map_clears_no_mark() {
    let h = Harness::new(Live);
    let id = filed_in_other_now_updates(&h);
    h.service().review("work", &id, true).unwrap(); // a reason to clear
    mark(&h, &id);
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
    let map = observe::FolderMap::sources_only(&cfg); // resolution failed
    let input = inputs::plan_input(&s.store, &ctx, &map, false).unwrap();
    let gone = rules::gone(&s.store, "work", &map.listed).unwrap();
    let mut refile = rules::input(&s.store, "work", &cfg, &gone).unwrap();
    upkeep::run(&mut s.store, &ctx, &map, &input, &mut refile).unwrap();
    assert!(s.store.placement("work", &id).unwrap().unwrap().refile_once);
}
