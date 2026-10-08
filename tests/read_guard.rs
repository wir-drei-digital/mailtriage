//! The read check in every filing mode: a folder whose effective target is
//! another mailbox is never read, and a mail engine configuration that
//! cannot be read or parsed reads nothing.
mod common;
use common::{mail, Harness};
use mailtriage::domain::FilingMode::{DryRun, Live, Off};

fn fetched_from(h: &Harness, folder: &str) -> bool {
    h.fake
        .calls()
        .iter()
        .any(|c| c.starts_with(&format!("fetch {folder}")))
}

#[test]
fn with_filing_off_a_source_that_leads_elsewhere_is_not_read() {
    let h = Harness::new(Off);
    h.fake.deliver("INBOX", &mail("a", "Hello", "body"));
    h.fake.set_alias_conflicts(&["INBOX"]);
    let blocked = h.sync();
    assert_eq!(blocked["discovered"], 1);
    assert_eq!(blocked["fetched"], 0);
    assert_eq!(blocked["pending"], 1, "the message stays queued");
    assert!(!fetched_from(&h, "INBOX"));
    assert!(h.fake.calls().iter().any(|c| c == "alias_conflicts INBOX"));
    h.fake.set_alias_conflicts(&[]);
    let freed = h.sync();
    assert_eq!(freed["fetched"], 1, "no attempt was burned");
    assert!(fetched_from(&h, "INBOX"));
}

#[test]
fn an_unreadable_engine_configuration_reads_nothing_in_any_mode() {
    for mode in [Off, DryRun, Live] {
        let h = Harness::new(mode);
        h.fake.deliver("INBOX", &mail("a", "Hello", "body"));
        h.fake.fail_alias_check(true);
        let out = h.sync();
        assert_eq!(out["fetched"], 0, "{mode:?}: {out}");
        assert_eq!(out["pending"], 1, "{mode:?}");
        assert!(!fetched_from(&h, "INBOX"), "{mode:?}");
        h.fake.fail_alias_check(false);
        assert_eq!(h.sync()["fetched"], 1, "{mode:?}");
    }
}

#[test]
fn a_category_folder_that_leads_elsewhere_is_not_read() {
    let h = Harness::new(Live);
    h.sync();
    // Mail that arrived in a category folder is fetched from there.
    h.fake
        .deliver("Newsletters", &mail("n", "Weekly newsletter", "news"));
    h.fake.set_alias_conflicts(&["Newsletters"]);
    let blocked = h.sync();
    assert!(!fetched_from(&h, "Newsletters"), "{blocked}");
    h.fake.set_alias_conflicts(&[]);
    h.sync();
    assert!(fetched_from(&h, "Newsletters"));
}

#[test]
fn doctor_reports_source_conflicts_and_an_unreadable_configuration() {
    for mode in [Off, Live] {
        let h = Harness::new(mode);
        h.fake.set_alias_conflicts(&["INBOX"]);
        let report = h.service().doctor("work").unwrap();
        assert_eq!(
            report["transport"]["alias_conflicts"],
            serde_json::json!(["INBOX"])
        );
        assert_eq!(report["transport"]["ready"], true, "{mode:?}");
        h.fake.set_alias_conflicts(&[]);
        h.fake.fail_alias_check(true);
        let report = h.service().doctor("work").unwrap();
        let t = &report["transport"];
        assert_eq!(t["ready"], false, "{mode:?}: {t}");
        assert_eq!(
            t["error"],
            "cannot read the Himalaya configuration: it is not valid TOML"
        );
        assert_eq!(report["ready"], false);
    }
}
