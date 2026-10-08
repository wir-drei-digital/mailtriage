use mailtriage::{
    config,
    domain::{EngineConfig, FilingMode, HimalayaConfig},
};
use serde_json::json;
use std::fs;

fn legacy_json(dir: &std::path::Path) -> serde_json::Value {
    fs::write(
        dir.join("h.toml"),
        "[accounts.work]\nimap.server='imaps://x.test'\n",
    )
    .unwrap();
    let mut c = serde_json::to_value(config::default_config()).unwrap();
    c["schema_version"] = json!(1);
    c["accounts"]["work"]["himalaya"] = json!({
        "binary": "himalaya", "config": "h.toml", "account": "work",
        "mailboxes": ["INBOX"], "expected_version": "2.1.0",
        "timeout_seconds": 30, "max_output_bytes": 1000000
    });
    c["accounts"]["work"]
        .as_object_mut()
        .unwrap()
        .remove("engine");
    c["accounts"]["work"]
        .as_object_mut()
        .unwrap()
        .remove("filing");
    c
}

#[test]
fn legacy_himalaya_config_loads_as_engine_and_saves_as_v3() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.json");
    fs::write(&path, legacy_json(dir.path()).to_string()).unwrap();
    let loaded = config::load(&path).unwrap();
    assert_eq!(loaded.schema_version, 3);
    let a = &loaded.accounts["work"];
    assert!(a.himalaya.is_none());
    assert!(matches!(a.engine, Some(EngineConfig::Himalaya(ref h)) if h.mailboxes == ["INBOX"]));
    assert_eq!(a.filing.mode, FilingMode::Off);
    config::save(&path, &loaded).unwrap();
    let on_disk: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(on_disk["schema_version"], 3);
    assert_eq!(on_disk["accounts"]["work"]["engine"]["kind"], "himalaya");
    assert!(on_disk["accounts"]["work"].get("himalaya").is_none());
    assert_eq!(on_disk["accounts"]["work"]["filing"]["mode"], "off");
}

#[test]
fn both_engine_and_legacy_himalaya_is_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.json");
    let mut c = legacy_json(dir.path());
    c["accounts"]["work"]["engine"] = json!({"kind": "himalaya", "binary": "himalaya", "config": "h.toml", "account": "work", "mailboxes": ["INBOX"], "expected_version": "2.1.0", "timeout_seconds": 30, "max_output_bytes": 1000000});
    fs::write(&path, c.to_string()).unwrap();
    assert!(config::load(&path).is_err());
}

#[test]
fn legacy_category_names_load_while_filing_is_off_and_fail_when_on() {
    let mut c = config::default_config();
    c.accounts.get_mut("work").unwrap().categories[1].name = "Receipts / Orders".into();
    config::validate(&c).unwrap();
    c.accounts.get_mut("work").unwrap().filing.mode = FilingMode::DryRun;
    let err = config::validate(&c).unwrap_err().to_string();
    assert!(err.contains("folder"), "{err}");
    c.accounts.get_mut("work").unwrap().categories[1].folder = Some("Receipts".into());
    config::validate(&c).unwrap();
}

#[test]
fn folder_rules_apply_when_filing_is_on() {
    let mut c = config::default_config();
    let a = c.accounts.get_mut("work").unwrap();
    a.filing.mode = FilingMode::Live;
    for bad in [
        "",
        " News",
        "News ",
        "a/b",
        "a.b",
        "x*",
        "x%",
        "q\"",
        "b\\s",
        "a&b",
        "tab\there",
        "Inbox",
        "inbox",
        "Grüße",
        "-x",
    ] {
        c.accounts.get_mut("work").unwrap().categories[0].folder = Some(bad.into());
        assert!(config::validate(&c).is_err(), "accepted {bad:?}");
    }
    c.accounts.get_mut("work").unwrap().categories[0].folder = None;
    let a = c.accounts.get_mut("work").unwrap();
    a.categories[0].folder = Some("INBOX".into());
    a.categories[1].folder = Some("INBOX".into());
    config::validate(&c).unwrap();
    let a = c.accounts.get_mut("work").unwrap();
    a.categories[2].folder = Some("Bills and Receipts".into());
    a.categories[3].folder = Some("bills and receipts".into());
    assert!(
        config::validate(&c).is_err(),
        "case-insensitive duplicate accepted"
    );
    c.accounts.get_mut("work").unwrap().categories[3].folder = Some("Newsletters".into());
    config::validate(&c).unwrap();
    c.accounts
        .get_mut("work")
        .unwrap()
        .filing
        .max_actions_per_pass = 0;
    assert!(config::validate(&c).is_err());
}

/// Final review M1: with filing on, the configured source mailboxes are
/// written to as raw IMAP text too, so they follow the same safe-name rules.
#[test]
fn source_mailboxes_are_checked_when_filing_is_on() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = config::default_config();
    let with_source = |c: &mut mailtriage::domain::AppConfig, source: &str, mode| {
        let a = c.accounts.get_mut("work").unwrap();
        a.engine = Some(EngineConfig::Himalaya(HimalayaConfig {
            binary: "himalaya".into(),
            config: dir.path().join("h.toml"),
            account: "work".into(),
            mailboxes: vec!["INBOX".into(), source.into()],
            expected_version: "2.1.0".into(),
            timeout_seconds: 30,
            max_output_bytes: 1_000_000,
        }));
        a.filing.mode = mode;
    };
    for bad in ["a\\b", "-x", "Grüße", "a&b", "q\"", "tab\there"] {
        with_source(&mut c, bad, FilingMode::Off);
        config::validate(&c).unwrap_or_else(|e| panic!("filing off refused {bad:?}: {e}"));
        with_source(&mut c, bad, FilingMode::DryRun);
        let err = config::validate(&c).unwrap_err().to_string();
        assert!(err.contains("source mailbox"), "{bad:?}: {err}");
    }
    for good in ["INBOX.Lists", "Lists/Work", "Bills and Receipts"] {
        with_source(&mut c, good, FilingMode::Live);
        config::validate(&c).unwrap_or_else(|e| panic!("refused {good:?}: {e}"));
    }
}

#[test]
fn unsupported_schema_versions_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.json");
    for version in [0, 5] {
        let mut c = serde_json::to_value(config::default_config()).unwrap();
        c["schema_version"] = json!(version);
        fs::write(&path, c.to_string()).unwrap();
        let err = config::load(&path).unwrap_err().to_string();
        assert!(err.contains("schema_version"), "schema {version}: {err}");
        let mut typed = config::default_config();
        typed.schema_version = version;
        assert!(
            config::save(&path, &typed).is_err(),
            "saved schema {version}"
        );
    }
}

#[test]
fn the_reply_queue_alone_makes_a_config_schema_4() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.json");
    let mut c = config::default_config();
    config::save(&path, &c).unwrap();
    let on_disk: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(on_disk["schema_version"], 3);
    assert!(on_disk["accounts"]["work"]["filing"]
        .get("reply_queue")
        .is_none());
    c.accounts.get_mut("work").unwrap().filing.reply_queue = true;
    config::save(&path, &c).unwrap();
    let on_disk: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(on_disk["schema_version"], 4);
    assert_eq!(on_disk["accounts"]["work"]["filing"]["reply_queue"], true);
    let loaded = config::load(&path).unwrap();
    assert!(loaded.accounts["work"].filing.reply_queue);
}

#[test]
fn himalaya_struct_field_still_works_for_existing_callers() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("h.toml"),
        "[accounts.work]\nimap.server='imaps://x.test'\n",
    )
    .unwrap();
    let mut c = config::default_config();
    c.accounts.get_mut("work").unwrap().himalaya = Some(HimalayaConfig {
        binary: "himalaya".into(),
        config: dir.path().join("h.toml"),
        account: "work".into(),
        mailboxes: vec!["INBOX".into()],
        expected_version: "2.1.0".into(),
        timeout_seconds: 3,
        max_output_bytes: 1000,
    });
    let path = dir.path().join("c.json");
    config::save(&path, &c).unwrap();
    assert!(config::load(&path).unwrap().accounts["work"]
        .engine
        .is_some());
}
