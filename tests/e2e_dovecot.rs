//! Real IMAP end-to-end test. Run through tests/e2e/run.sh; skipped unless MT_E2E_* is set.
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

struct Env {
    himalaya: String,
    port: String,
    layout: String,
    dir: tempfile::TempDir,
}

fn env() -> Option<Env> {
    Some(Env {
        himalaya: std::env::var("MT_E2E_HIMALAYA").ok()?,
        port: std::env::var("MT_E2E_PORT").ok()?,
        layout: std::env::var("MT_E2E_LAYOUT").ok()?,
        dir: tempfile::tempdir().unwrap(),
    })
}

/// The simulated mail client (tests/e2e/imap_client.py); it must succeed.
fn imap(e: &Env, args: &[&str]) -> Value {
    let out = Command::new("python3")
        .arg("tests/e2e/imap_client.py")
        .arg("--port")
        .arg(&e.port)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "imap_client {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

/// The real mailtriage binary with `--json`: its exit code and JSON output,
/// echoed for the failure report.
fn mt_status(e: &Env, args: &[&str]) -> (Option<i32>, Value) {
    let mut full = vec!["--config", "mailtriage.json"];
    full.extend_from_slice(args);
    full.push("--json");
    let out = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
        .current_dir(e.dir.path())
        .args(&full)
        .output()
        .unwrap();
    eprintln!(
        "mailtriage {args:?} -> {}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout)
    );
    let value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    (out.status.code(), value)
}

/// `mt_status` for a command that must succeed (exit 0).
fn mt(e: &Env, args: &[&str]) -> Value {
    let (code, value) = mt_status(e, args);
    assert_eq!(code, Some(0), "mailtriage {args:?}: {value}");
    value
}

/// One `sync` pass that must be clean: exit 0, not partial, no filing error.
fn sync(e: &Env) -> Value {
    let out = mt(e, &["sync", "--account", "work"]);
    assert_eq!(out["partial"], false, "{out}");
    assert_eq!(out["filing"]["errors"], 0, "{out}");
    out
}

fn folder(e: &Env, name: &str) -> String {
    if e.layout == "prefix" && name != "INBOX" {
        format!("INBOX.{name}")
    } else {
        name.to_string()
    }
}

/// The only copy of a message on the server: (folder, flags). Fails when
/// the message is missing or in more than one place.
fn only_location(e: &Env, mid: &str) -> (String, Vec<String>) {
    let mut found = location(e, mid);
    assert_eq!(
        found.len(),
        1,
        "{mid} must have exactly one copy: {found:?}"
    );
    found.remove(0)
}

fn location(e: &Env, mid: &str) -> Vec<(String, Vec<String>)> {
    let found = imap(e, &["locate", mid]);
    eprintln!("locate {mid} -> {found}");
    found
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r[0].as_str().unwrap().to_string(),
                r[2].as_array()
                    .unwrap()
                    .iter()
                    .map(|f| f.as_str().unwrap().to_string())
                    .collect(),
            )
        })
        .collect()
}

fn write_mail(dir: &Path, name: &str, mid: &str, subject: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, format!("Message-ID: {mid}\r\nFrom: Alex <alex@example.com>\r\nTo: e2e@example.test\r\nSubject: {subject}\r\n\r\n{body}\r\n")).unwrap();
    p
}

fn find_item(e: &Env, subject: &str) -> Value {
    let list = mt(e, &["list", "--account", "work", "--view", "all"]);
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["subject"] == subject)
        .unwrap()
        .clone()
}

#[test]
#[ignore]
fn filing_against_real_dovecot() {
    let Some(e) = env() else {
        eprintln!("MT_E2E_* not set; skipping");
        return;
    };
    // 1. Config: init, then engine + live filing + explicit folders (correspondence -> INBOX).
    assert_eq!(mt(&e, &["init"])["schema_version"], 1);
    let template = std::fs::read_to_string("tests/e2e/himalaya.toml.in").unwrap();
    std::fs::write(
        e.dir.path().join("himalaya.toml"),
        template.replace("@PORT@", &e.port),
    )
    .unwrap();
    let cfg_path = e.dir.path().join("mailtriage.json");
    let mut cfg: Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    let account = &mut cfg["accounts"]["work"];
    account["engine"] = serde_json::json!({
        "kind": "himalaya", "binary": e.himalaya, "config": "himalaya.toml", "account": "e2e",
        "mailboxes": ["INBOX"], "expected_version": "2.1.0", "timeout_seconds": 30, "max_output_bytes": 20000000
    });
    account["filing"] =
        serde_json::json!({"mode": "live", "flag": true, "max_actions_per_pass": 200});
    for c in account["categories"].as_array_mut().unwrap() {
        let folder = if c["id"] == "correspondence" {
            "INBOX".to_string()
        } else {
            c["name"].as_str().unwrap().to_string()
        };
        c["folder"] = Value::from(folder);
    }
    std::fs::write(&cfg_path, cfg.to_string()).unwrap();
    assert_eq!(
        mt(&e, &["doctor", "--account", "work"])["filing"]["move_supported"],
        true
    );
    // The first pass sets `enabled_at`; mail appended afterwards is new. The
    // pause absorbs a container clock running slightly behind the host's.
    sync(&e);
    std::thread::sleep(std::time::Duration::from_secs(2));
    let n = write_mail(
        e.dir.path(),
        "n.eml",
        "<n@e2e>",
        "Weekly newsletter",
        "Our newsletter",
    );
    let i = write_mail(
        e.dir.path(),
        "i.eml",
        "<i@e2e>",
        "Invoice",
        "Payment due today",
    );
    imap(&e, &["append", "INBOX", n.to_str().unwrap()]);
    imap(&e, &["append", "INBOX", i.to_str().unwrap()]);
    for _ in 0..2 {
        sync(&e);
    }
    let ln = only_location(&e, "<n@e2e>");
    assert_eq!(ln.0, folder(&e, "Newsletters"));
    assert!(
        !ln.1.contains(&"\\Seen".to_string()),
        "read state preserved"
    );
    let li = only_location(&e, "<i@e2e>");
    assert_eq!(li.0, folder(&e, "Transactions"));
    assert!(li.1.contains(&"\\Flagged".to_string()));
    // 2. Client move = correction.
    imap(
        &e,
        &[
            "move",
            &folder(&e, "Newsletters"),
            "<n@e2e>",
            &folder(&e, "Updates"),
        ],
    );
    for _ in 0..2 {
        sync(&e);
    }
    let item = find_item(&e, "Weekly newsletter");
    assert_eq!(item["classification"]["category_id"], "updates");
    assert_eq!(only_location(&e, "<n@e2e>").0, folder(&e, "Updates"));
    // 3. Client move back to INBOX = pin.
    imap(
        &e,
        &["move", &folder(&e, "Transactions"), "<i@e2e>", "INBOX"],
    );
    for _ in 0..3 {
        sync(&e);
    }
    assert_eq!(only_location(&e, "<i@e2e>").0, "INBOX");
    // 4. Client delete = done.
    imap(&e, &["delete", &folder(&e, "Updates"), "<n@e2e>"]);
    for _ in 0..3 {
        sync(&e);
    }
    assert!(location(&e, "<n@e2e>").is_empty());
    let read = mt(
        &e,
        &[
            "read",
            "--account",
            "work",
            "--id",
            item["id"].as_str().unwrap(),
        ],
    );
    assert_eq!(read["item"]["review_state"], "done");
    // 5. Epoch reset of INBOX keeps the pinned message known and open.
    imap(
        &e,
        &["reset-epoch", "INBOX", &format!("mt-e2e-{}", e.layout)],
    );
    for _ in 0..4 {
        sync(&e);
    }
    let status = mt(&e, &["filing", "status", "--account", "work"]);
    assert_eq!(status["ambiguous"], 0);
    assert_eq!(only_location(&e, "<i@e2e>").0, "INBOX");
    let invoice = find_item(&e, "Invoice");
    assert_eq!(invoice["review_state"], "open");
    assert_eq!(invoice["placement"]["location_state"], "known");
    assert_eq!(invoice["placement"]["folder"], "INBOX");
    assert_eq!(invoice["placement"]["pinned"], true);
}
