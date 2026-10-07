//! Refile spec "Filed home": which mailtriage moves grant it, and what ends it.
mod common;
mod refile_support;
use common::{mail, Harness};
use mailtriage::{
    domain::FilingMode::Live,
    engine::fake::{FakeOp, Fault},
    filing::LocationState,
    service::RetryTarget,
};
use refile_support::{deliver_filed, filed_home, home, id_of, located, pause, placement};

#[test]
fn a_move_proven_by_copyuid_grants_the_filed_home() {
    let h = Harness::new(Live);
    h.sync();
    let id = deliver_filed(&h, "n", "Weekly newsletter", "Our newsletter");
    let p = placement(&h, &id);
    let (folder, uid) = located(&h, "n");
    assert_eq!(folder, "Newsletters");
    assert_eq!(home(&p), Some((folder.clone(), h.fake.epoch(&folder), uid)));
    assert_eq!(filed_home(&p), home(&p));
    assert!(p.at_filed_home());
}

#[test]
fn moves_without_a_matching_copyuid_grant_none() {
    // No UIDPLUS: the moved copy is identified by fingerprint.
    let h = Harness::new(Live);
    h.fake.set_capabilities(true, false, true);
    h.sync();
    let id = deliver_filed(&h, "n", "Weekly newsletter", "Our newsletter");
    h.sync();
    let p = placement(&h, &id);
    assert_eq!(
        (p.home_folder.as_deref(), p.filed_by.as_deref()),
        (Some("Newsletters"), Some("mailtriage"))
    );
    assert_eq!(filed_home(&p), None);

    // A lost response: recovery applies the move by observation.
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::ErrorAfter);
    for _ in 0..4 {
        h.sync();
    }
    let p = placement(&h, &id_of(&h, "n"));
    assert_eq!(p.home_folder.as_deref(), Some("Newsletters"));
    assert_eq!(filed_home(&p), None);
}

#[test]
fn a_move_applied_after_its_targets_epoch_changed_grants_none() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.sync(); // moved; the intent waits for its arrival
    h.fake.reset_epoch("Newsletters");
    for _ in 0..5 {
        h.sync();
    }
    let s = h.service();
    let intent = s
        .store
        .intents("work", false)
        .unwrap()
        .into_iter()
        .find(|i| i.kind == "move")
        .unwrap();
    assert_eq!(intent.state, "applied");
    let p = s
        .store
        .placement("work", &intent.message_id)
        .unwrap()
        .unwrap();
    assert_eq!(p.home_epoch, Some(h.fake.epoch("Newsletters")));
    assert_eq!(filed_home(&p), None);
}

#[test]
fn a_client_move_or_relocation_ends_the_filed_home() {
    let h = Harness::new(Live);
    h.sync();
    let a = deliver_filed(&h, "a", "Weekly newsletter", "Our newsletter");
    let b = deliver_filed(&h, "b", "Second newsletter", "More newsletter news");
    let (folder, uid) = located(&h, "a");
    h.fake.client_move(&folder, uid, "Updates");
    let (folder, uid) = located(&h, "b");
    h.fake.client_move(&folder, uid, &folder); // back into the folder it was in
    h.sync();
    h.sync();
    let a = placement(&h, &a);
    assert_eq!(
        (a.home_folder.as_deref(), filed_home(&a)),
        (Some("Updates"), None)
    );
    let b = placement(&h, &b);
    assert_eq!(b.home_folder.as_deref(), Some("Newsletters"));
    assert_eq!(b.home_uid, Some(located(&h, "b").1));
    assert_eq!(filed_home(&b), None);
}

#[test]
fn an_epoch_reset_ends_it_and_rediscovery_never_restores_it() {
    let h = Harness::new(Live);
    h.sync();
    let id = deliver_filed(&h, "n", "Weekly newsletter", "Our newsletter");
    h.fake.reset_epoch("Newsletters");
    for _ in 0..4 {
        h.sync();
    }
    let p = placement(&h, &id);
    assert_eq!(
        (p.home_folder.as_deref(), p.home_epoch),
        (Some("Newsletters"), Some(h.fake.epoch("Newsletters")))
    );
    assert_eq!(filed_home(&p), None);
}

#[test]
fn a_rescan_in_the_same_epoch_keeps_it() {
    let h = Harness::new(Live);
    h.sync();
    let id = deliver_filed(&h, "n", "Weekly newsletter", "Our newsletter");
    let before = filed_home(&placement(&h, &id));
    assert!(before.is_some());
    pause(&h, "Newsletters");
    h.service()
        .filing_retry("work", RetryTarget::Folder("Newsletters".into()))
        .unwrap();
    for _ in 0..3 {
        h.sync();
    }
    let rec = h
        .service()
        .store
        .folder_record("work", "Newsletters")
        .unwrap()
        .unwrap();
    assert!(rec.rescan_complete);
    assert_eq!(filed_home(&placement(&h, &id)), before);
}

#[test]
fn mail_that_left_every_folder_loses_it() {
    let h = Harness::new(Live);
    h.sync();
    let id = deliver_filed(&h, "n", "Weekly newsletter", "Our newsletter");
    let (folder, uid) = located(&h, "n");
    h.fake.client_delete(&folder, uid);
    h.sync();
    h.sync();
    let p = placement(&h, &id);
    assert_eq!(p.location_state, LocationState::Absent);
    assert_eq!(filed_home(&p), None);
}
