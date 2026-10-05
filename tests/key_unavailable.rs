#![cfg(unix)]
//! Final review I2: an unavailable OpenRouter key skips classification
//! before any job is leased, so it consumes no attempts; mail stays queued
//! and is classified once the key works again.
mod common;
use common::{mail, Harness};
use mailtriage::{config, domain::FilingMode, service::Service};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    thread,
};

const KEY_ERROR: &str = "API key command failed (exit 1)";

/// A Decisions endpoint on 127.0.0.1 that answers `count` requests with one
/// fixed decision, then stops.
fn decisions_stub(count: usize) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        for _ in 0..count {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = Vec::new();
            let mut buf = [0u8; 4096];
            let header_end = loop {
                let n = stream.read(&mut buf).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
                if let Some(p) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    break p + 4;
                }
            };
            let headers = String::from_utf8_lossy(&bytes[..header_end]).to_ascii_lowercase();
            assert!(headers.contains("authorization: bearer sk-or-fixed"));
            let length: usize = headers
                .lines()
                .find_map(|l| l.strip_prefix("content-length: ")?.trim().parse().ok())
                .unwrap();
            while bytes.len() - header_end < length {
                let n = stream.read(&mut buf).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
            }
            let response = json!({"model":"typesafe/jev-1.13", "answers":{
                "category":{"type":"choice","choice":"correspondence","confidence":0.98,"probabilities":{"correspondence":0.98,"transactions":0.01,"updates":0.0,"newsletters":0.0,"promotions":0.0,"other":0.01}},
                "urgency":{"type":"choice","choice":"medium","confidence":0.99,"probabilities":{"low":0.01,"medium":0.99,"high":0.0}},
                "action_required":{"type":"noul","noul":0.95}}}).to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).unwrap();
        }
    });
    (
        format!("http://127.0.0.1:{port}/api/alpha/decisions"),
        server,
    )
}

/// A key command that logs each run to `log`, then runs `script`.
fn key_command(log: &Path, script: &str) -> Vec<String> {
    vec![
        "/bin/sh".into(),
        "-c".into(),
        format!("echo run >> '{}'; {script}", log.display()),
    ]
}

fn database(config: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(config.with_file_name("mailtriage-state/mailtriage.sqlite")).unwrap()
}

/// `(state, attempts)` of every job, in message id order.
fn jobs(config: &Path) -> Vec<(String, u32)> {
    let db = database(config);
    let mut stmt = db
        .prepare("SELECT state, attempts FROM jobs ORDER BY message_id")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// Every job is due now, as if its retry wait had passed.
fn make_due(config: &Path) {
    database(config)
        .execute("UPDATE jobs SET next_after='2000-01-01T00:00:00+00:00'", [])
        .unwrap();
}

fn queued(n: usize) -> Vec<(String, u32)> {
    vec![("queued".to_owned(), 0); n]
}

#[test]
fn an_unavailable_key_leaves_mail_queued_until_the_key_works() {
    let h = Harness::new(FilingMode::Off);
    let (endpoint, server) = decisions_stub(2);
    let log = h.dir.path().join("key-runs.log");
    h.edit(|c| {
        c.provider.kind = "openrouter".into();
        c.provider.model = "typesafe/jev-1.13".into();
        c.provider.endpoint = endpoint.clone();
        c.provider.api_key_env = "OPENROUTER_API_KEY".into();
        c.provider.api_key_command = Some(key_command(&log, "exit 1"));
    });
    h.fake
        .deliver("INBOX", &mail("a", "Invoice", "Please pay by Friday."));
    h.fake
        .deliver("INBOX", &mail("b", "Lunch", "Are you free on Monday?"));
    let max_attempts = config::load(&h.path).unwrap().policy.max_attempts as usize;
    let mut passes = Vec::new();
    for _ in 0..=max_attempts {
        passes.push(h.sync());
        make_due(&h.path);
    }
    // No attempt was consumed and nothing went terminal.
    assert_eq!(jobs(&h.path), queued(2));
    for out in &passes {
        assert_eq!(out["partial"], true, "{out}");
        assert_eq!(
            out["classification"],
            json!({"skipped": true, "reason": KEY_ERROR}),
            "{out}"
        );
        assert_eq!(
            (out["failed"].clone(), out["classified"].clone()),
            (json!(0), json!(0))
        );
    }
    assert_eq!(passes[0]["discovered"], 2);
    assert_eq!(passes.last().unwrap()["pending"], 2);
    let beat = h.service().store.heartbeat("work").unwrap().unwrap();
    assert_eq!(
        (beat["exit_code"].clone(), beat["partial"].clone()),
        (json!(4), json!(true))
    );
    // The key is resolved once per pass, however many messages wait.
    assert_eq!(
        fs::read_to_string(&log).unwrap().lines().count(),
        max_attempts + 1
    );

    // A working key changes no generation; the queued mail is classified.
    h.edit(|c| c.provider.api_key_command = Some(key_command(&log, "echo sk-or-fixed")));
    let out = h.sync();
    server.join().unwrap();
    assert_eq!(out["classified"], 2, "{out}");
    assert_eq!(out["partial"], false, "{out}");
    assert!(out.get("classification").is_none(), "{out}");
    assert_eq!(jobs(&h.path), vec![("complete".to_owned(), 1); 2]);
}

#[test]
fn classify_and_reclassify_lease_nothing_without_a_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let log = dir.path().join("key-runs.log");
    let mut c = config::default_config();
    c.provider.kind = "openrouter".into();
    c.provider.model = "typesafe/jev-1.13".into();
    c.provider.endpoint = "http://127.0.0.1:9/api/alpha/decisions".into();
    c.provider.api_key_env = "OPENROUTER_API_KEY".into();
    c.provider.api_key_command = Some(key_command(&log, "exit 1"));
    config::save(&path, &c).unwrap();
    let raw = mail("c", "Contract", "Please sign the attached contract.");
    let out = Service::open(&path)
        .unwrap()
        .classify("work", &raw, "rfc822")
        .unwrap();
    assert_eq!(out["outcome"], "skipped", "{out}");
    assert_eq!(out["partial"], true, "{out}");
    assert_eq!(out["classification"]["reason"], KEY_ERROR, "{out}");
    assert_eq!(out["item"]["classification"]["state"], "pending", "{out}");
    assert_eq!(jobs(&path), queued(1));

    let out = Service::open(&path)
        .unwrap()
        .reclassify("work", None, false, 100)
        .unwrap();
    assert_eq!(out["partial"], true, "{out}");
    assert_eq!(out["failed"], 0, "{out}");
    assert_eq!(out["reclassified"], 0, "{out}");
    assert_eq!(out["classification"]["skipped"], true, "{out}");
    assert_eq!(jobs(&path), queued(1));
    let attempts: Vec<Value> = Service::open(&path).unwrap().export("work").unwrap()["attempts"]
        .as_array()
        .unwrap()
        .clone();
    assert!(attempts.is_empty(), "{attempts:?}");
}
