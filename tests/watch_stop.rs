//! A pass asked to stop (SIGTERM or Ctrl-C in `watch`) takes no new message
//! and skips its filing steps; the next pass resumes from stored state.
mod common;
use common::{mail, Harness};
use mailtriage::domain::FilingMode::Live;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[test]
fn a_pass_asked_to_stop_classifies_and_files_nothing_more() {
    let h = Harness::new(Live);
    h.sync(); // creates folders
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake
        .deliver("INBOX", &mail("i", "Invoice", "Payment due today"));
    let stop = Arc::new(AtomicBool::new(true));
    let writes = h.fake.write_calls();
    let out = h
        .service()
        .with_stop(Arc::clone(&stop))
        .sync("work", 100)
        .unwrap();
    assert_eq!(out["stopped"], true, "{out}");
    assert_eq!(out["partial"], true, "{out}");
    assert_eq!(out["classified"], 0, "{out}");
    assert_eq!(h.fake.write_calls(), writes, "no filing writes");
    stop.store(false, Ordering::SeqCst);
    let out = h
        .service()
        .with_stop(Arc::clone(&stop))
        .sync("work", 100)
        .unwrap();
    assert!(out.get("stopped").is_none(), "{out}");
    assert_eq!(out["classified"], 2, "{out}");
    assert_eq!(out["filing"]["moved"], 2, "{out}");
}
