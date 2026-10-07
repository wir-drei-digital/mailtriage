//! Refile spec "Each pass" (Planning): marked mail moves in the next pass.
mod common;
mod refile_support;
use common::Harness;
use mailtriage::domain::FilingMode::{DryRun, Live};
use refile_support::{
    events, filed_home, filed_in_other_now_updates, home, located, mark, moves_to, placement,
};

#[test]
fn a_marked_message_is_refiled_by_the_next_pass() {
    let h = Harness::new(Live);
    let id = filed_in_other_now_updates(&h);
    mark(&h, &id);
    let plan = h.service().filing_plan("work", 50).unwrap();
    assert_eq!(plan["total"], 1);
    let action = &plan["actions"][0];
    assert_eq!(action["message_id"], id.as_str());
    assert_eq!(action["to"], "Updates");
    assert_eq!(action["reason"], "refile");
    h.sync();
    h.sync();
    assert_eq!(located(&h, "u").0, "Updates");
    let p = placement(&h, &id);
    assert!(!p.refile_once, "consumed by its own move");
    assert_eq!(p.home_folder.as_deref(), Some("Updates"));
    assert_eq!(
        filed_home(&p),
        home(&p),
        "a refile move is proven like any other"
    );
    assert_eq!(events(&h, "moved")[0]["detail"]["reason"], "refile");
    let intent = h
        .service()
        .store
        .intents("work", false)
        .unwrap()
        .pop()
        .unwrap();
    assert!(intent.consumes_refile);
    assert_eq!(intent.state, "applied");
    h.sync();
    assert_eq!(moves_to(&h, "Updates"), 1, "moved once");
    assert_eq!(h.service().filing_plan("work", 50).unwrap()["total"], 0);
}

#[test]
fn unmarked_mail_is_not_refiled() {
    let h = Harness::new(Live);
    filed_in_other_now_updates(&h);
    h.sync();
    h.sync();
    assert_eq!(located(&h, "u").0, "Other");
    assert_eq!(moves_to(&h, "Updates"), 0);
}

#[test]
fn a_dry_run_pass_plans_the_refile_and_writes_nothing() {
    let h = Harness::new(Live);
    let id = filed_in_other_now_updates(&h);
    mark(&h, &id);
    h.set_mode(DryRun);
    let writes = h.fake.write_calls();
    let out = h.sync();
    assert_eq!(out["filing"]["planned"], 1);
    assert_eq!(h.fake.write_calls(), writes);
    assert!(placement(&h, &id).refile_once, "the mark waits for live");
}
