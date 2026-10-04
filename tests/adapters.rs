use mailtriage::{
    config,
    domain::{Address, HimalayaConfig, NormalizedMessage, Urgency},
    himalaya::Himalaya,
    provider,
};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    thread,
};
use tempfile::TempDir;

fn message() -> NormalizedMessage {
    NormalizedMessage {
        raw_sha256: "test".into(),
        from: vec![Address {
            email: "a@example.test".into(),
            name: None,
        }],
        to: vec![],
        cc: vec![],
        subject: "Can you review this?".into(),
        sent_at: None,
        body: "Please review by tomorrow.".into(),
        incomplete: false,
        warnings: vec![],
    }
}

#[test]
fn fake_provider_is_explicit_and_deterministic() {
    let app = config::default_config();
    let account = &app.accounts["work"];
    let first = provider::classify(&app.provider, account, &message(), &app.policy).unwrap();
    let second = provider::classify(&app.provider, account, &message(), &app.policy).unwrap();
    assert_eq!(first.urgency, Some(Urgency::Medium));
    assert_eq!(first.action_required, Some(true));
    assert_eq!(first.category_id, second.category_id);
    assert_eq!(first.raw["answers"], second.raw["answers"]);
}

#[test]
fn decisions_http_contract_and_typed_response() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut buf = [0u8; 4096];
        let header_end;
        loop {
            let n = stream.read(&mut buf).unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&buf[..n]);
            if let Some(p) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                header_end = p + 4;
                break;
            }
        }
        let headers = std::str::from_utf8(&bytes[..header_end]).unwrap();
        assert!(headers.starts_with("POST /api/alpha/decisions HTTP/1.1"));
        assert!(headers
            .to_ascii_lowercase()
            .contains("authorization: bearer fixture-key"));
        let length: usize = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length: ")
                    .and_then(|s| s.trim().parse().ok())
            })
            .unwrap();
        while bytes.len() - header_end < length {
            let n = stream.read(&mut buf).unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&buf[..n]);
        }
        let request: Value =
            serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
        assert_eq!(request["model"], "typesafe/jev-1.13");
        assert_eq!(request["questions"]["category"]["type"], "choice");
        assert_eq!(request["questions"]["urgency"]["type"], "choice");
        assert_eq!(request["questions"]["action_required"]["type"], "noul");
        assert_eq!(request["state"]["subject"], "Can you review this?");
        assert!(request.get("messages").is_none());
        let response = json!({"model":"typesafe/jev-1.13", "answers":{
            "category":{"type":"choice","choice":"correspondence","confidence":0.98,"probabilities":{"correspondence":0.98,"transactions":0.01,"updates":0.0,"newsletters":0.0,"promotions":0.0,"other":0.01}},
            "urgency":{"type":"choice","choice":"medium","confidence":0.99,"probabilities":{"low":0.01,"medium":0.99,"high":0.0}},
            "action_required":{"type":"noul","noul":0.95}}}).to_string();
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).unwrap();
    });
    let mut app = config::default_config();
    app.provider.kind = "openrouter".into();
    app.provider.model = "typesafe/jev-1.13".into();
    app.provider.endpoint = format!("http://127.0.0.1:{port}/api/alpha/decisions");
    app.provider.api_key_env = "MAILTRIAGE_ADAPTER_TEST_KEY".into();
    std::env::set_var("MAILTRIAGE_ADAPTER_TEST_KEY", "fixture-key");
    let classification = provider::classify(
        &app.provider,
        &app.accounts["work"],
        &message(),
        &app.policy,
    )
    .unwrap();
    std::env::remove_var("MAILTRIAGE_ADAPTER_TEST_KEY");
    server.join().unwrap();
    assert_eq!(
        classification.category_id.as_deref(),
        Some("correspondence")
    );
    assert_eq!(classification.action_required, Some(true));
}

#[test]
fn decisions_errors_are_bounded_and_do_not_expose_response_bodies() {
    for (status, body, expected) in [
        (
            "429 Too Many Requests",
            "provider secret diagnostic",
            "HTTP 429",
        ),
        ("200 OK", "not-json", "invalid OpenRouter Decisions JSON"),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut incoming = [0u8; 8192];
            let _ = stream.read(&mut incoming).unwrap();
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let mut app = config::default_config();
        app.provider.kind = "openrouter".into();
        app.provider.model = "typesafe/jev-1.13".into();
        app.provider.endpoint = format!("http://127.0.0.1:{port}/api/alpha/decisions");
        app.provider.api_key_env = "MAILTRIAGE_ADAPTER_ERROR_TEST_KEY".into();
        std::env::set_var("MAILTRIAGE_ADAPTER_ERROR_TEST_KEY", "fixture-key");
        let error = provider::classify(
            &app.provider,
            &app.accounts["work"],
            &message(),
            &app.policy,
        )
        .unwrap_err()
        .to_string();
        std::env::remove_var("MAILTRIAGE_ADAPTER_ERROR_TEST_KEY");
        server.join().unwrap();
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains("provider secret diagnostic"));
    }
}

/// A fake Himalaya binary with a timeout generous enough for a loaded test
/// machine; only the timeout subcases use `fake_himalaya_timeout` directly.
#[cfg(unix)]
fn fake_himalaya(script: &str) -> (TempDir, Himalaya) {
    fake_himalaya_timeout(script, 5)
}

#[cfg(unix)]
fn fake_himalaya_timeout(script: &str, timeout_seconds: u64) -> (TempDir, Himalaya) {
    use std::os::unix::fs::PermissionsExt;
    let temp = TempDir::new().unwrap();
    let binary = temp.path().join("himalaya");
    fs::write(&binary, script).unwrap();
    let mut permissions = fs::metadata(&binary).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&binary, permissions).unwrap();
    let config_path = temp.path().join("config.toml");
    fs::write(&config_path, "").unwrap();
    let adapter = Himalaya::new(&HimalayaConfig {
        binary,
        config: config_path,
        account: "work".into(),
        mailboxes: vec!["INBOX".into()],
        expected_version: "2.1.0".into(),
        timeout_seconds,
        max_output_bytes: 4096,
    })
    .unwrap();
    (temp, adapter)
}

#[cfg(unix)]
#[test]
fn himalaya_uid_discovery_and_binary_raw_fetch() {
    let script = "#!/bin/sh\ncase \" $* \" in *' --seen '*) exit 9 ;; esac\ncase \"$*\" in\n  *--version*) printf 'himalaya v2.1.0 +imap +smtp\\nbuild: macos aarch64\\n' ;;\n  *'imap status INBOX'*) printf '{\"uid_validity\":9,\"uid_next\":44}' ;;\n  *'imap fetch --mailbox INBOX --envelope --flags --internal-date --size 42:43'*) printf '{\"messages\":[{\"uid\":42,\"envelope\":{\"subject\":\"Hello\",\"from\":[\"A <a@example.test>\"],\"date\":\"Tue, 1 Sep 2026 10:00:00 +0000\"}}]}' ;;\n  *'message read --mailbox INBOX --raw 42'*) printf 'Subject: binary\\r\\n\\r\\n\\377\\000' ;;\n  *) exit 7 ;;\nesac\n";
    let (_temp, adapter) = fake_himalaya(script);
    assert_eq!(adapter.version().unwrap(), "himalaya v2.1.0 +imap +smtp");
    assert_eq!(adapter.snapshot("INBOX").unwrap().uid_validity, 9);
    let envelopes = adapter.discover("INBOX", 41, 43).unwrap();
    assert_eq!(envelopes.len(), 1);
    assert_eq!(envelopes[0].uid, 42);
    assert_eq!(envelopes[0].from[0].email, "a@example.test");
    assert!(adapter.fetch("INBOX", 42).unwrap().ends_with(&[255, 0]));
    assert!(adapter.fetch("Trash", 42).is_err());
}

#[cfg(unix)]
#[test]
fn himalaya_version_requires_pinned_release_and_imap_feature() {
    for output in [
        "himalaya 2.1.0\n",
        "himalaya v2.1.0 +smtp\n",
        "himalaya v2.0.0 +imap\n",
    ] {
        let script = format!("#!/bin/sh\nprintf '{}'\n", output.replace('\n', "\\n"));
        let (_temp, adapter) = fake_himalaya(&script);
        assert!(adapter.version().is_err(), "accepted {output:?}");
    }
}

#[cfg(unix)]
#[test]
fn himalaya_discovery_allows_missing_sender_but_rejects_wrong_type() {
    let script = "#!/bin/sh\ncase \"$*\" in\n *'imap status INBOX'*) printf '{\"uid_validity\":9,\"uid_next\":3}' ;;\n *'imap fetch --mailbox INBOX --envelope --flags --internal-date --size 1:2'*) printf '{\"messages\":[{\"uid\":1,\"envelope\":{\"subject\":\"No sender\"}},{\"uid\":2,\"envelope\":{\"subject\":\"Null sender\",\"from\":null}}]}' ;;\n *) exit 7 ;;\nesac\n";
    let (_fixture, adapter) = fake_himalaya(script);
    let envelopes = adapter.discover("INBOX", 0, 2).unwrap();
    assert_eq!(envelopes.len(), 2);
    assert!(envelopes.iter().all(|e| e.from.is_empty()));

    let script = "#!/bin/sh\ncase \"$*\" in\n *'imap status INBOX'*) printf '{\"uid_validity\":9,\"uid_next\":2}' ;;\n *'imap fetch --mailbox INBOX --envelope --flags --internal-date --size 1:1'*) printf '{\"messages\":[{\"uid\":1,\"envelope\":{\"from\":\"not an array\"}}]}' ;;\n *) exit 7 ;;\nesac\n";
    let (_fixture, adapter) = fake_himalaya(script);
    assert!(adapter
        .discover("INBOX", 0, 1)
        .unwrap_err()
        .to_string()
        .contains("envelope.from"));
}

#[cfg(unix)]
#[test]
fn himalaya_rejects_epoch_reset_timeout_and_large_output() {
    let temp = TempDir::new().unwrap();
    let counter = temp.path().join("counter");
    let script = format!("#!/bin/sh\ncase \"$*\" in\n *'imap status INBOX'*) n=$(cat '{}'); n=$((n+1)); printf '%s' \"$n\" > '{}'; if [ \"$n\" -eq 1 ]; then printf '{{\"uid_validity\":1,\"uid_next\":3}}'; else printf '{{\"uid_validity\":2,\"uid_next\":3}}'; fi ;;\n *'message read --mailbox INBOX --raw 1'*) printf 'abc' ;;\n *) exit 7 ;;\nesac\n", counter.display(), counter.display());
    fs::write(&counter, "0").unwrap();
    let (_fixture, adapter) = fake_himalaya(&script);
    assert!(adapter
        .fetch("INBOX", 1)
        .unwrap_err()
        .to_string()
        .contains("epoch changed"));

    let (_fixture, delayed) = fake_himalaya_timeout("#!/bin/sh\nsleep 3\n", 1);
    assert!(delayed
        .snapshot("INBOX")
        .unwrap_err()
        .to_string()
        .contains("timed out"));

    let (_fixture, huge) = fake_himalaya("#!/bin/sh\nhead -c 10000 /dev/zero\n");
    assert!(huge
        .snapshot("INBOX")
        .unwrap_err()
        .to_string()
        .contains("output limit"));

    let (_fixture, inherited_pipe) = fake_himalaya_timeout("#!/bin/sh\n(sleep 3) &\nexit 0\n", 1);
    let started = std::time::Instant::now();
    assert!(inherited_pipe
        .snapshot("INBOX")
        .unwrap_err()
        .to_string()
        .contains("timed out"));
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}
