use fs2::FileExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn run(cwd: &Path, args: &[&str]) -> (Output, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
        .current_dir(cwd)
        .args(args)
        .env_remove("MAILTRIAGE_CONFIG")
        .output()
        .expect("run mailtriage");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "expected JSON stdout; status {:?}; stderr {}; stdout {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        )
    });
    (output, value)
}

#[test]
fn offline_cli_round_trip_and_exit_codes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (output, init) = run(root, &["init", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(init["provider"], "fake");
    assert!(root.join("mailtriage.json").exists());

    let (output, _) = run(root, &["doctor", "--account", "work", "--json"]);
    assert_eq!(output.status.code(), Some(0));

    let (output, error) = run(root, &["init", "--json"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(error["error"]["code"], 2);

    let message = root.join("test.eml");
    fs::write(&message, b"From: Alex <alex@example.com>\r\nTo: Demo <work@example.invalid>\r\nSubject: Please reply\r\nContent-Type: text/plain; charset=UTF-8\r\n\r\nPlease send me feedback this week.\r\n").unwrap();
    let (output, result) = run(
        root,
        &[
            "classify",
            "--account",
            "work",
            "--input",
            "test.eml",
            "--format",
            "rfc822",
            "--json",
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{result:?}");
    let id = result["item"]["id"]
        .as_str()
        .expect("classified message id");

    let (output, listed) = run(
        root,
        &["list", "--account", "work", "--view", "all", "--json"],
    );
    assert_eq!(output.status.code(), Some(0));
    assert!(listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["id"] == id));

    let (output, read) = run(root, &["read", "--account", "work", "--id", id, "--json"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(read.to_string().contains("Please send me feedback"));

    let (output, _) = run(
        root,
        &[
            "correct",
            "--account",
            "work",
            "--id",
            id,
            "--action-required",
            "true",
            "--json",
        ],
    );
    assert_eq!(output.status.code(), Some(0));
    let (output, _) = run(root, &["done", "--account", "work", "--id", id, "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let (output, _) = run(root, &["reopen", "--account", "work", "--id", id, "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let (output, _) = run(root, &["export", "--account", "work", "--json"]);
    assert_eq!(output.status.code(), Some(0));

    let (output, categories) = run(
        root,
        &["categories", "export", "--account", "work", "--json"],
    );
    assert_eq!(output.status.code(), Some(0));
    fs::write(
        root.join("categories.json"),
        serde_json::to_vec(&categories).unwrap(),
    )
    .unwrap();
    let (output, validated) = run(
        root,
        &[
            "categories",
            "validate",
            "--file",
            "categories.json",
            "--json",
        ],
    );
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(validated["valid"], true);
    let (output, _) = run(
        root,
        &[
            "categories",
            "apply",
            "--account",
            "work",
            "--file",
            "categories.json",
            "--json",
        ],
    );
    assert_eq!(output.status.code(), Some(0));
    let (output, _) = run(
        root,
        &["reclassify", "--account", "work", "--dry-run", "--json"],
    );
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn invalid_cli_flags_use_json_error_envelope() {
    let dir = tempfile::tempdir().unwrap();
    let (output, value) = run(
        dir.path(),
        &["list", "--account", "work", "--limit", "0", "--json"],
    );
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["error"]["code"], 2);
    assert!(String::from_utf8_lossy(&output.stderr).is_empty());
}

/// The state directory `init` configured, created as the first command would.
fn state_dir(root: &Path) -> PathBuf {
    let config: Value =
        serde_json::from_slice(&fs::read(root.join("mailtriage.json")).unwrap()).unwrap();
    let dir = root.join(config["state_dir"].as_str().unwrap());
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Holds an exclusive lock on `path`, as another mailtriage process would.
fn hold(path: &Path) -> File {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    file.try_lock_exclusive().unwrap();
    file
}

#[test]
fn conflicts_name_their_reason_in_the_json_error_object() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    assert_eq!(run(root, &["init", "--json"]).0.status.code(), Some(0));
    fs::write(root.join("test.eml"), b"From: Alex <alex@example.com>\r\nTo: Demo <work@example.invalid>\r\nSubject: Please reply\r\nContent-Type: text/plain; charset=UTF-8\r\n\r\nPlease send me feedback this week.\r\n").unwrap();
    let classify = [
        "classify",
        "--account",
        "work",
        "--input",
        "test.eml",
        "--format",
        "rfc822",
        "--json",
    ];

    // A second worker for the account.
    let worker = hold(&state_dir(root).join(format!("worker-{:x}.lock", Sha256::digest(b"work"))));
    let (output, value) = run(root, &classify);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(
        value["error"],
        json!({"code":5,"message":"an account worker is already running","reason":"account_busy"})
    );
    drop(worker);

    // Another command is editing the configuration.
    let editing = hold(&root.join("mailtriage.lock"));
    let (output, value) = run(root, &["filing", "disable", "--account", "work", "--json"]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(
        value["error"],
        json!({"code":5,"message":"configuration is being edited","reason":"config_busy"})
    );
    // A command that only reads the configuration under its shared lock.
    let (output, value) = run(
        root,
        &[
            "correct",
            "--account",
            "work",
            "--id",
            "any",
            "--action-required",
            "true",
            "--json",
        ],
    );
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(value["error"]["reason"], "config_busy", "{value}");
    drop(editing);

    // The account's binding changed after the first command stored it.
    let (output, _) = run(root, &classify);
    assert_eq!(output.status.code(), Some(0));
    let mut config: Value =
        serde_json::from_slice(&fs::read(root.join("mailtriage.json")).unwrap()).unwrap();
    config["accounts"]["work"]["identity"] = json!("other@example.invalid");
    fs::write(root.join("mailtriage.json"), config.to_string()).unwrap();
    let (output, value) = run(root, &classify);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(value["error"]["reason"], "binding_conflict");
}

#[test]
fn an_error_without_a_kind_has_no_reason() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    assert_eq!(run(root, &["init", "--json"]).0.status.code(), Some(0));
    let (output, value) = run(root, &["doctor", "--account", "nosuch", "--json"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        value,
        json!({"schema_version":1,"error":{"code":2,"message":"unknown account"}})
    );
    let (output, value) = run(root, &["init", "--json"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(value["error"].get("reason").is_none(), "{value}");
}
