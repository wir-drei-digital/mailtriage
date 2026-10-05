//! Config resolution through the binary: `--config`, `MAILTRIAGE_CONFIG`,
//! `./mailtriage.json`, then `~/.config/mailtriage/mailtriage.json`.
use mailtriage::config;
use serde_json::Value;
use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn mailtriage(cwd: &Path, home: Option<&Path>, env_config: Option<&Path>, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mailtriage"));
    command
        .current_dir(cwd)
        .args(args)
        .env_remove("MAILTRIAGE_CONFIG")
        .env_remove("HOME");
    if let Some(home) = home {
        command.env("HOME", home);
    }
    if let Some(path) = env_config {
        command.env("MAILTRIAGE_CONFIG", path);
    }
    command.output().unwrap()
}

/// The state directory `doctor` reports, which tells configs apart.
fn doctor_state(cwd: &Path, home: Option<&Path>, env: Option<&Path>, extra: &[&str]) -> String {
    let mut args = vec!["doctor", "--account", "work", "--json"];
    args.extend_from_slice(extra);
    let out = mailtriage(cwd, home, env, &args);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    v["state_dir"].as_str().unwrap().to_owned()
}

#[test]
fn commands_find_the_config_in_the_documented_order() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    let home = root.path().join("home");
    for dir in [&a, &b, &home] {
        fs::create_dir_all(dir).unwrap();
    }
    // `init` writes ./mailtriage.json and ignores MAILTRIAGE_CONFIG.
    let elsewhere = b.join("elsewhere.json");
    assert!(
        mailtriage(&a, Some(&home), Some(&elsewhere), &["init", "--json"])
            .status
            .success()
    );
    assert!(a.join("mailtriage.json").exists());
    assert!(!elsewhere.exists());
    // Nothing in b and nothing under HOME: exit 2, pointing at setup.
    let out = mailtriage(
        &b,
        Some(&home),
        None,
        &["doctor", "--account", "work", "--json"],
    );
    assert_eq!(out.status.code(), Some(2));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["error"]["message"]
        .as_str()
        .unwrap()
        .contains("mailtriage setup"));
    // 4. The home config.
    let home_config = home.join(".config/mailtriage/mailtriage.json");
    let made = mailtriage(
        &b,
        Some(&home),
        None,
        &["init", "--json", "--config", home_config.to_str().unwrap()],
    );
    assert!(made.status.success());
    assert!(doctor_state(&b, Some(&home), None, &[]).ends_with(".config/mailtriage/.state"));
    // 3. ./mailtriage.json beats the home config.
    assert!(doctor_state(&a, Some(&home), None, &[]).ends_with("a/.state"));
    // 2. MAILTRIAGE_CONFIG beats ./mailtriage.json.
    assert!(doctor_state(&a, Some(&home), Some(&home_config), &[])
        .ends_with(".config/mailtriage/.state"));
    // 1. --config beats MAILTRIAGE_CONFIG.
    let local = a.join("mailtriage.json");
    assert!(doctor_state(
        &b,
        Some(&home),
        Some(&home_config),
        &["--config", local.to_str().unwrap()]
    )
    .ends_with("a/.state"));
}

#[test]
fn without_home_the_error_names_config() {
    let dir = tempfile::tempdir().unwrap();
    let out = mailtriage(
        dir.path(),
        None,
        None,
        &["doctor", "--account", "work", "--json"],
    );
    assert_eq!(out.status.code(), Some(2));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        v["error"]["message"].as_str().unwrap().contains("--config"),
        "{v}"
    );
}

#[test]
fn an_empty_environment_value_counts_as_unset() {
    let cwd = Path::new("/nonexistent-cwd");
    let home = Path::new("/h");
    assert_eq!(
        config::resolve_path(None, Some(OsStr::new("")), cwd, Some(home)).unwrap(),
        PathBuf::from("/h/.config/mailtriage/mailtriage.json")
    );
    assert_eq!(
        config::setup_path(None, Some(OsStr::new("/x.json")), Some(home)).unwrap(),
        PathBuf::from("/x.json")
    );
    assert!(config::setup_path(None, None, None).is_err());
}
