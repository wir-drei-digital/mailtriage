#![cfg(unix)]
//! Exact-argument contract for the Himalaya engine. The fixtures set
//! process-wide environment variables, so every case runs in sequence from the
//! single `contract` test.
use mailtriage::{
    domain::HimalayaConfig,
    engine::{himalaya::Himalaya, ConfigChanged, MailEngine},
};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};
use tempfile::TempDir;

const SCRIPT: &str = r#"#!/usr/bin/env python3
import sys, json, os, time
a = sys.argv[1:]
open(os.environ["MT_LOG"], "a").write(json.dumps(a) + "\n")
i = a.index("--backend") + 2
cmd = a[i:]
if cmd and cmd[0] == "--json": cmd = cmd[1:]
mode = os.environ.get("MT_MODE", "")
def out(s): sys.stdout.write(s); sys.stdout.flush()
if a[-1] == "--version": out("himalaya v2.1.0 +imap\n")
elif cmd[:2] == ["imap", "raw"]:
    text = cmd[-1]
    if "CAPABILITY" in text:
        out('* CAPABILITY IMAP4rev1 MOVE UIDPLUS SPECIAL-USE\r\na1 OK done\r\n* NAMESPACE (("" "/")) NIL NIL\r\na2 OK done\r\n')
    elif "RETURN (SPECIAL-USE)" in text:
        out('* LIST (\\HasNoChildren) "/" "INBOX"\r\n* LIST (\\HasNoChildren \\Sent) "/" "Sent"\r\n* LIST () "/" "Newsletters"\r\na1 OK\r\n')
    elif "UID MOVE" in text:
        if mode == "crash": sys.exit(3)
        if mode == "noselect":
            out("a1 NO no such mailbox\r\na2 BAD no mailbox selected\r\n"); sys.exit(1)
        if mode == "noselect_timeout":
            out("a1 NO no such mailbox\r\n"); time.sleep(5)
        if mode == "timeout":
            out("* OK [UIDVALIDITY 7] ok\r\na1 OK done\r\n"); time.sleep(5)
        out("* OK [UIDVALIDITY 7] ok\r\na1 OK done\r\n* OK [COPYUID 9 4,5 20:21] moved\r\na2 OK done\r\n")
    elif "UID STORE" in text:
        out("* OK [UIDVALIDITY 7] ok\r\na1 OK done\r\na2 OK done\r\n")
    else: sys.exit(5)
elif cmd[:2] == ["imap", "list"]:
    rows = [{"name": "INBOX", "delimiter": "/", "attributes": []},
            {"name": "Sent", "delimiter": "/", "attributes": ["\\Sent"]},
            {"name": "Newsletters", "delimiter": "/", "attributes": []}]
    if "--all" not in cmd: rows = rows[:2]
    out(json.dumps(rows if mode == "array" else {"preset": "x", "mailboxes": rows}))
elif cmd[:2] in (["imap", "create"], ["imap", "subscribe"]): out("ok\n")
elif cmd[:2] == ["imap", "fetch"]:
    out(json.dumps({"messages": [{"uid": 4, "flags": ["\\Seen", "\\Flagged"], "internal_date": "2026-10-04T08:00:00+00:00", "size": 1234,
        "envelope": {"subject": "Hi", "from": ["A <a@x.test>"], "date": None, "message_id": "<m1@x.test>"}}]}))
elif cmd[:2] == ["imap", "status"]: out('{"uid_validity":7,"uid_next":10}')
else: sys.exit(7)
"#;

struct Fixture {
    _dir: TempDir,
    log: PathBuf,
    toml: PathBuf,
    engine: Himalaya,
}

fn fixture(mode: &str, timeout: u64) -> Fixture {
    let dir = TempDir::new().unwrap();
    let binary = dir.path().join("himalaya");
    fs::write(&binary, SCRIPT).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    let toml = dir.path().join("h.toml");
    fs::write(&toml, "[accounts.work]\nimap.server='imaps://x.test'\n[accounts.work.mailbox.alias]\nnewsletters = \"Lists/News\"\ninbox = \"INBOX\"\n").unwrap();
    let log = dir.path().join("log");
    std::env::set_var("MT_LOG", &log);
    std::env::set_var("MT_MODE", mode);
    let engine = Himalaya::new(&HimalayaConfig {
        binary,
        config: toml.clone(),
        account: "work".into(),
        mailboxes: vec!["INBOX".into()],
        expected_version: "2.1.0".into(),
        timeout_seconds: timeout,
        max_output_bytes: 100_000,
    })
    .unwrap();
    engine.set_watch_scope(&["Bills and Receipts".into(), "News".into()]);
    Fixture {
        _dir: dir,
        log,
        toml,
        engine,
    }
}

fn calls(f: &Fixture) -> Vec<Vec<String>> {
    fs::read_to_string(&f.log)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str::<Vec<String>>(l).unwrap())
        .collect()
}

fn tail(call: &[String]) -> Vec<String> {
    let i = call.iter().position(|a| a == "--backend").unwrap() + 2;
    call[i..].to_vec()
}

fn assert_no_forbidden(f: &Fixture) {
    for call in calls(f) {
        let joined = call.join(" ");
        for bad in [
            "expunge", "EXPUNGE", "delete", "DELETE", "rename", "RENAME", "--seq", "--seen",
            "-FLAGS", "--action",
        ] {
            assert!(!joined.contains(bad), "forbidden {bad:?} in {joined:?}");
        }
    }
}

fn move_quotes_names_with_spaces() {
    let f = fixture("", 5);
    let out = f
        .engine
        .move_messages("INBOX", &[4, 5], "Bills and Receipts")
        .unwrap();
    assert!(out.selected && out.completed);
    assert_eq!(out.session_epoch, Some(7));
    assert_eq!(out.copyuid.unwrap().pairs, vec![(4, 20), (5, 21)]);
    let last = calls(&f).pop().unwrap();
    assert_eq!(
        tail(&last),
        vec![
            "imap",
            "raw",
            "--",
            "a1 SELECT \"INBOX\"\r\na2 UID MOVE 4,5 \"Bills and Receipts\"\r\n"
        ]
    );
    assert_no_forbidden(&f);
}

fn flag_store_text() {
    let f = fixture("", 5);
    let out = f.engine.add_flagged("INBOX", &[4]).unwrap();
    assert!(out.selected && out.completed && out.copyuid.is_none());
    let last = calls(&f).pop().unwrap();
    assert_eq!(
        tail(&last),
        vec![
            "imap",
            "raw",
            "--",
            "a1 SELECT \"INBOX\"\r\na2 UID STORE 4 +FLAGS.SILENT (\\Flagged)\r\n"
        ]
    );
    assert_no_forbidden(&f);
}

fn timeout_keeps_partial_select_result() {
    let f = fixture("timeout", 1);
    let out = f.engine.move_messages("INBOX", &[4], "News").unwrap();
    assert!(out.selected);
    assert_eq!(out.session_epoch, Some(7));
    assert!(!out.completed);
}

fn capabilities_and_namespace() {
    let f = fixture("", 5);
    let caps = f.engine.capabilities().unwrap();
    assert!(caps.move_supported && caps.uidplus && caps.special_use);
    assert_eq!(caps.personal_prefix, "");
    assert_eq!(caps.delimiter, Some('/'));
    let last = calls(&f).pop().unwrap();
    assert_eq!(
        tail(&last),
        vec!["imap", "raw", "--", "a1 CAPABILITY\r\na2 NAMESPACE\r\n"]
    );
}

fn list_folders_accepts_object_or_array_json() {
    for mode in ["", "array"] {
        let f = fixture(mode, 5);
        let folders = f.engine.list_folders().unwrap();
        let sent = folders.iter().find(|x| x.name == "Sent").unwrap();
        assert_eq!(sent.roles.as_deref(), Some(&["\\Sent".to_string()][..]));
        assert!(sent.subscribed);
        let news = folders.iter().find(|x| x.name == "Newsletters").unwrap();
        assert_eq!(news.roles.as_deref(), Some(&[][..]));
        assert!(!news.subscribed);
    }
}

fn create_and_subscribe_args() {
    let f = fixture("", 5);
    f.engine.create_folder("Bills and Receipts").unwrap();
    f.engine.subscribe_folder("Bills and Receipts").unwrap();
    assert!(f.engine.create_folder("A&B").is_err());
    let c = calls(&f);
    assert_eq!(
        tail(&c[c.len() - 2]),
        vec!["imap", "create", "Bills and Receipts"]
    );
    assert_eq!(
        tail(&c[c.len() - 1]),
        vec!["imap", "subscribe", "Bills and Receipts"]
    );
}

fn envelopes_parse_new_fields_and_args() {
    let f = fixture("", 5);
    let env = f.engine.envelopes("INBOX", &[4, 9]).unwrap();
    assert_eq!(env[0].message_id.as_deref(), Some("<m1@x.test>"));
    assert_eq!(env[0].size, Some(1234));
    assert_eq!(
        env[0].internal_date.as_deref(),
        Some("2026-10-04T08:00:00+00:00")
    );
    assert!(env[0].flags.iter().any(|x| x == "\\Flagged"));
    let last = calls(&f).pop().unwrap();
    assert_eq!(
        tail(&last),
        vec![
            "--json",
            "imap",
            "fetch",
            "--mailbox",
            "INBOX",
            "--envelope",
            "--flags",
            "--internal-date",
            "--size",
            "4,9"
        ]
    );
}

fn config_change_is_refused() {
    let f = fixture("", 5);
    f.engine.capabilities().unwrap();
    fs::write(
        &f.toml,
        "[accounts.work]\nimap.server='imaps://other.test'\n",
    )
    .unwrap();
    // capabilities() is cached; list_folders() must spawn and therefore re-check the TOML.
    let err = f.engine.list_folders().unwrap_err();
    assert!(err.downcast_ref::<ConfigChanged>().is_some());
}

fn alias_conflicts_detected() {
    let f = fixture("", 5);
    let conflicts = f
        .engine
        .alias_conflicts(&["Newsletters".into(), "INBOX".into(), "Receipts".into()])
        .unwrap();
    assert_eq!(conflicts, vec!["Newsletters".to_string()]);
    // INBOX is case-insensitive in IMAP: a server that lists it as "Inbox"
    // names the mailbox the alias `inbox = "INBOX"` resolves to.
    let conflicts = f
        .engine
        .alias_conflicts(&["Inbox".into(), "inbox".into()])
        .unwrap();
    assert!(conflicts.is_empty(), "{conflicts:?}");
}

// A session whose SELECT result was never captured is an error (outcome
// unknown). A captured SELECT failure is a definite `selected: false`, even on a
// non-zero exit or when the process then had to be killed.
fn write_errors_only_without_select_result() {
    let f = fixture("crash", 5);
    assert!(f.engine.move_messages("INBOX", &[4], "News").is_err());
    let f = fixture("noselect_timeout", 1);
    let out = f.engine.move_messages("INBOX", &[4], "News").unwrap();
    assert!(!out.selected && !out.completed);
    let f = fixture("noselect", 5);
    let out = f.engine.move_messages("INBOX", &[4], "News").unwrap();
    assert!(!out.selected && !out.completed);
    assert!(f.engine.add_flagged("INBOX", &[]).is_err());
    let too_many: Vec<u64> = (1..=101).collect();
    assert!(f.engine.move_messages("INBOX", &too_many, "News").is_err());
    assert_eq!(calls(&f).len(), 1, "invalid UID lists must not spawn");
}

fn out_of_scope_folders_are_refused_without_spawning() {
    let f = fixture("", 5);
    assert!(f.engine.snapshot("Archive").is_err());
    assert!(f.engine.discover("Archive", 0, 4).is_err());
    assert!(f.engine.envelopes("Archive", &[4]).is_err());
    assert!(f.engine.fetch_raw("Archive", 4).is_err());
    assert!(f.engine.move_messages("INBOX", &[4], "Archive").is_err());
    assert!(f.engine.move_messages("Archive", &[4], "News").is_err());
    assert!(f.engine.add_flagged("Archive", &[4]).is_err());
    assert!(f.engine.create_folder("Archive").is_err());
    assert!(f.engine.subscribe_folder("Archive").is_err());
    assert!(calls(&f).is_empty(), "out-of-scope folders must not spawn");
    f.engine.set_watch_scope(&["Archive".into()]);
    assert_eq!(f.engine.envelopes("Archive", &[4]).unwrap()[0].uid, 4);
    assert!(
        f.engine
            .move_messages("INBOX", &[4], "Archive")
            .unwrap()
            .completed
    );
    assert_eq!(calls(&f).len(), 2);
    // Each call replaces the previous scope.
    f.engine.set_watch_scope(&[]);
    assert!(f.engine.envelopes("Archive", &[4]).is_err());
    assert!(f.engine.move_messages("INBOX", &[4], "News").is_err());
    assert_eq!(calls(&f).len(), 2);
}

#[test]
fn contract() {
    move_quotes_names_with_spaces();
    flag_store_text();
    timeout_keeps_partial_select_result();
    capabilities_and_namespace();
    list_folders_accepts_object_or_array_json();
    create_and_subscribe_args();
    envelopes_parse_new_fields_and_args();
    config_change_is_refused();
    alias_conflicts_detected();
    write_errors_only_without_select_result();
    out_of_scope_folders_are_refused_without_spawning();
}
