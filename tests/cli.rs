use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn run(cwd: &Path, args: &[&str]) -> (Output, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
        .current_dir(cwd)
        .args(args)
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
