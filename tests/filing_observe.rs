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
