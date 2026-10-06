//! The categories digest, `categories apply --expect-digest` and the
//! `changes` of `categories validate --account`.
use mailtriage::{
    categories::{self, Changes},
    config,
    domain::{AppConfig, Category, FilingMode},
    service::{ErrorKind, Service, ServiceError},
};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const DEFAULT_OFF: &str = "v1:fe333646885134da461c3ef57080b76e331c276b50c3e1fa44935bdd418f02ac";
const DEFAULT_ON: &str = "v1:c0fe803aca4c84827ee579f2d8ba4e1ce496e3cd9f0fc493a034cf030dceee69";
const UNICODE_ON: &str = "v1:7733c397ba1df0b0d3ffe42025d7551f2f164d2413ce93c4ef65c1a60b1591f5";

fn defaults() -> Vec<Category> {
    config::default_config().accounts["work"].categories.clone()
}

fn parse(text: &str) -> Vec<Category> {
    serde_json::from_str(text).unwrap()
}

#[test]
fn digests_are_fixed_vectors() {
    assert_eq!(categories::digest(false, &defaults()), DEFAULT_OFF);
    assert_eq!(categories::digest(true, &defaults()), DEFAULT_ON);
    let mut swapped = defaults();
    swapped.swap(0, 1);
    assert_ne!(categories::digest(false, &swapped), DEFAULT_OFF);
    let unicode = parse(
        r#"[{"id":"cafe","name":"Café ☕ Rechnungen","description":"Belege für Kaffee","catch_all":true,"folder":"Cafe"}]"#,
    );
    assert_eq!(categories::digest(true, &unicode), UNICODE_ON);
    let reordered = parse(
        r#"[{"folder":"Cafe","catch_all":true,"description":"Belege für Kaffee","name":"Café ☕ Rechnungen","id":"cafe","examples":[]}]"#,
    );
    assert_eq!(categories::digest(true, &reordered), UNICODE_ON);
    let null_folder =
        parse(r#"[{"id":"x","name":"X","description":"d","catch_all":true,"folder":null}]"#);
    let no_folder = parse(r#"[{"id":"x","name":"X","description":"d","catch_all":true}]"#);
    assert_eq!(
        categories::digest(false, &null_folder),
        categories::digest(false, &no_folder)
    );
}

/// A config with the default `work` account, filing in `mode`.
fn config_with(dir: &Path, mode: FilingMode) -> PathBuf {
    let path = dir.join("mailtriage.json");
    let mut c = config::default_config();
    c.state_dir = dir.join("state");
    c.accounts.get_mut("work").unwrap().filing.mode = mode;
    config::save(&path, &c).unwrap();
    path
}

fn reason(error: anyhow::Error) -> (i32, Option<&'static str>) {
    let e = error.downcast_ref::<ServiceError>().unwrap();
    (e.code, e.kind.reason())
}

#[test]
fn export_and_validate_report_the_digest() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_with(dir.path(), FilingMode::Off);
    let mut service = Service::open(&path).unwrap();
    assert_eq!(service.categories("work").unwrap()["digest"], DEFAULT_OFF);
    let checked = service.validate_categories("work", defaults()).unwrap();
    assert_eq!(checked["digest"], DEFAULT_OFF);
    assert_eq!(
        checked["changes"],
        json!({"added":[],"removed":[],"renamed":[],"folders_changed":[],"edited":[],"reclassifies":false})
    );
}

fn run(dir: &Path, args: &[&str]) -> (i32, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
        .current_dir(dir)
        .args(args)
        .env_remove("MAILTRIAGE_CONFIG")
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null),
    )
}

#[test]
fn apply_checks_the_expected_digest_first() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_with(dir.path(), FilingMode::Off);
    let mut renamed = defaults();
    renamed[3].name = "News Digest".into();
    fs::write(
        dir.path().join("cats.json"),
        json!({ "categories": renamed }).to_string(),
    )
    .unwrap();
    let apply = |digest: &str| {
        run(
            dir.path(),
            &[
                "categories",
                "apply",
                "--account",
                "work",
                "--file",
                "cats.json",
                "--expect-digest",
                digest,
                "--json",
            ],
        )
    };
    // A stale digest: exit 5, nothing written.
    let before = fs::read(&path).unwrap();
    let (code, out) = apply(DEFAULT_ON);
    assert_eq!(code, 5);
    assert_eq!(
        out["error"],
        json!({"code":5,"message":"categories changed since export; export again","reason":"categories_changed"})
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    // The current digest: applied, and the result carries the new one.
    let (code, out) = apply(DEFAULT_OFF);
    assert_eq!(code, 0, "{out}");
    assert_eq!(out["categories"][3]["name"], "News Digest");
    assert_ne!(out["digest"], DEFAULT_OFF);
}

#[test]
fn turning_filing_off_between_export_and_apply_is_a_change() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_with(dir.path(), FilingMode::DryRun);
    let digest = Service::open(&path).unwrap().categories("work").unwrap()["digest"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(digest, categories::digest(true, &defaults()));
    let mut c: AppConfig = config::load(&path).unwrap();
    c.accounts.get_mut("work").unwrap().filing.mode = FilingMode::Off;
    config::save(&path, &c).unwrap();
    let error = Service::open(&path)
        .unwrap()
        .apply_categories_expecting("work", defaults(), Some(&digest))
        .unwrap_err();
    assert_eq!(reason(error), (5, Some("categories_changed")));
}

/// The test hook for `config_changed` is the library: the file changes
/// between the command's read and its write.
#[test]
fn a_config_rewritten_during_apply_is_config_changed() {
    let dir = tempfile::tempdir().unwrap();
    let path = config_with(dir.path(), FilingMode::Off);
    let mut service = Service::open(&path).unwrap();
    let mut c = config::load(&path).unwrap();
    c.accounts.get_mut("work").unwrap().brief = "changed".into();
    config::save(&path, &c).unwrap();
    let error = service
        .apply_categories_expecting("work", defaults(), Some(DEFAULT_OFF))
        .unwrap_err();
    assert_eq!(reason(error), (5, ErrorKind::ConfigChanged.reason()));
}

fn changes_for(mode: FilingMode, edit: impl FnOnce(&mut Vec<Category>)) -> Changes {
    let dir = tempfile::tempdir().unwrap();
    let path = config_with(dir.path(), mode);
    let service = Service::open(&path).unwrap();
    let mut next = defaults();
    edit(&mut next);
    let previous = &service.config.accounts["work"];
    categories::changes(
        &previous.categories,
        &categories::normalized(previous, next),
    )
}

#[test]
fn changes_name_each_kind() {
    let changes = changes_for(FilingMode::DryRun, |next| {
        next.retain(|c| c.id != "promotions");
        next[1].folder = Some("Receipts".into());
        next[2].description = "Status mail".into();
        next[3].name = "News Digest".into();
        next.push(Category {
            id: "travel".into(),
            name: "Travel".into(),
            description: "Trips".into(),
            examples: vec![],
            catch_all: false,
            folder: None,
        });
    });
    assert_eq!(
        serde_json::to_value(&changes).unwrap(),
        json!({
            "added": ["travel"],
            "removed": [{"id":"promotions","folder":"Promotions"}],
            "renamed": [{"id":"newsletters","from":"Newsletters","to":"News Digest"}],
            "folders_changed": [{"id":"transactions","from":"Transactions","to":"Receipts"}],
            "edited": ["updates"],
            "reclassifies": true
        })
    );
}

#[test]
fn a_rename_keeps_its_folder_only_with_filing_on() {
    let rename = |next: &mut Vec<Category>| next[3].name = "News Digest".into();
    let on = changes_for(FilingMode::DryRun, rename);
    assert_eq!(on.renamed.len(), 1);
    assert!(on.folders_changed.is_empty());
    assert!(!on.reclassifies);
    let off = changes_for(FilingMode::Off, rename);
    assert_eq!(off.folders_changed.len(), 1);
    assert_eq!(off.folders_changed[0].to, "News Digest");
}

type Edit = Box<dyn Fn(&mut Vec<Category>)>;

#[test]
fn reclassifies_matches_whether_apply_advances_the_revision() {
    let edits: Vec<(&str, Edit)> = vec![
        ("rename", Box::new(|n| n[3].name = "News Digest".into())),
        (
            "description",
            Box::new(|n| n[2].description = "Status mail".into()),
        ),
        (
            "examples",
            Box::new(|n| n[2].examples = vec!["Build passed".into()]),
        ),
        ("reorder", Box::new(|n| n.swap(0, 1))),
        (
            "folder",
            Box::new(|n| n[1].folder = Some("Receipts".into())),
        ),
        ("remove", Box::new(|n| n.retain(|c| c.id != "promotions"))),
    ];
    for (what, edit) in edits {
        let dir = tempfile::tempdir().unwrap();
        let path = config_with(dir.path(), FilingMode::DryRun);
        let mut next = defaults();
        edit(&mut next);
        let mut service = Service::open(&path).unwrap();
        let checked = service.validate_categories("work", next.clone()).unwrap();
        let before = service.config.accounts["work"].taxonomy_revision;
        let applied = service.apply_categories("work", next).unwrap();
        assert_eq!(
            checked["changes"]["reclassifies"],
            applied["taxonomy_revision"].as_u64().unwrap() > before,
            "{what}"
        );
    }
}
