//! Config schema 3 and its `updates` key.
use mailtriage::{config, domain::UpdateMode};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn write(dir: &Path, value: &Value) -> PathBuf {
    let path = dir.join("mailtriage.json");
    fs::write(&path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    path
}

fn on_disk(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn default_json() -> Value {
    serde_json::to_value(config::default_config()).unwrap()
}

fn mailtriage(dir: &Path, args: &[&str]) -> (Option<i32>, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
        .current_dir(dir)
        .args(args)
        .env("HOME", dir)
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env_remove("MAILTRIAGE_CONFIG")
        .output()
        .unwrap();
    (
        out.status.code(),
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null),
    )
}

#[test]
fn schema_3_is_written_with_an_explicit_updates() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mailtriage.json");
    config::save(&path, &config::default_config()).unwrap();
    let written = on_disk(&path);
    assert_eq!(written["schema_version"], 3);
    assert_eq!(written["updates"], "auto");
    let mut c = config::default_config();
    c.updates = UpdateMode::Off;
    config::save(&path, &c).unwrap();
    assert_eq!(on_disk(&path)["updates"], "off");
    assert_eq!(config::load(&path).unwrap().updates, UpdateMode::Off);
}

#[test]
fn schemas_1_and_2_read_as_auto() {
    let dir = tempfile::tempdir().unwrap();
    for version in [1, 2] {
        let mut c = default_json();
        c["schema_version"] = json!(version);
        c.as_object_mut().unwrap().remove("updates");
        let path = write(dir.path(), &c);
        assert_eq!(config::load(&path).unwrap().updates, UpdateMode::Auto);
        assert_eq!(config::read_updates_mode(&path).unwrap(), UpdateMode::Auto);
    }
}

#[test]
fn an_invalid_updates_value_is_refused_with_its_rule() {
    let dir = tempfile::tempdir().unwrap();
    for bad in [json!("sometimes"), json!("AUTO"), json!(null), json!(true)] {
        let mut c = default_json();
        c["updates"] = bad.clone();
        let path = write(dir.path(), &c);
        let e = config::load(&path).unwrap_err();
        assert!(
            e.downcast_ref::<config::InvalidUpdates>().is_some(),
            "{bad}"
        );
        assert!(config::read_updates_mode(&path).is_err(), "{bad}");
    }
    let (code, v) = mailtriage(dir.path(), &["sync", "--account", "work", "--json"]);
    assert_eq!(code, Some(2), "{v}");
    assert_eq!(v["error"]["message"], "updates must be auto, notify or off");
}

#[test]
fn the_updates_mode_is_read_without_the_rest_of_validation() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = default_json();
    c["updates"] = json!("notify");
    c["accounts"] = json!({});
    let path = write(dir.path(), &c);
    assert!(config::load(&path).is_err());
    assert_eq!(
        config::read_updates_mode(&path).unwrap(),
        UpdateMode::Notify
    );
    c["schema_version"] = json!(5);
    let path = write(dir.path(), &c);
    assert!(config::read_updates_mode(&path).is_err());
    assert!(config::read_updates_mode(&dir.path().join("missing.json")).is_err());
}

#[test]
fn init_and_categories_apply_write_schema_3_and_keep_updates() {
    let dir = tempfile::tempdir().unwrap();
    let (code, _) = mailtriage(dir.path(), &["init", "--json"]);
    assert_eq!(code, Some(0));
    let path = dir.path().join("mailtriage.json");
    assert_eq!(on_disk(&path)["schema_version"], 3);
    assert_eq!(on_disk(&path)["updates"], "auto");

    let mut c = on_disk(&path);
    c["updates"] = json!("off");
    write(dir.path(), &c);
    let (_, export) = mailtriage(
        dir.path(),
        &["categories", "export", "--account", "work", "--json"],
    );
    let mut categories = export["categories"].clone();
    categories[0]["description"] = json!("People I write with");
    let file = dir.path().join("categories.json");
    fs::write(&file, categories.to_string()).unwrap();
    let (code, v) = mailtriage(
        dir.path(),
        &[
            "categories",
            "apply",
            "--account",
            "work",
            "--file",
            file.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(code, Some(0), "{v}");
    let written = on_disk(&path);
    assert_eq!(written["updates"], "off");
    assert_eq!(written["schema_version"], 3);
    assert_eq!(
        written["accounts"]["work"]["categories"][0]["description"],
        "People I write with"
    );
}
