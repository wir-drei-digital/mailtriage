#![cfg(unix)]
//! The Himalaya engine computes each folder's effective target from the
//! merged alias map and the reported version's roles, and never runs
//! `message read` for a folder that leads elsewhere or with a configuration
//! it cannot parse.
use mailtriage::{
    domain::HimalayaConfig,
    engine::{himalaya::Himalaya, MailEngine},
};
use std::{fs, os::unix::fs::PermissionsExt};

/// Logs every call to `calls.log`; prints the `version` file for
/// `--version`, a fixed status, and a message for `message read`.
const FAKE: &str = r#"#!/bin/sh
dir="$(dirname "$0")"
printf '%s\n' "$*" >> "$dir/calls.log"
for a in "$@"; do last="$a"; done
case "$last" in
  --version) cat "$dir/version"; exit 0 ;;
esac
case "$*" in
  *"imap status"*) printf '{"uid_validity":9,"uid_next":44}' ;;
  *"message read"*) printf 'Subject: hi\r\n\r\nbody' ;;
  *) exit 7 ;;
esac
"#;

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new(version: &str, toml: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("himalaya");
        fs::write(&bin, FAKE).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            dir.path().join("version"),
            format!("himalaya v{version} +imap\n"),
        )
        .unwrap();
        fs::write(dir.path().join("h.toml"), toml).unwrap();
        Self { dir }
    }

    fn engine(&self, mailboxes: &[&str]) -> Himalaya {
        Himalaya::new(&HimalayaConfig {
            binary: self.dir.path().join("himalaya"),
            config: self.dir.path().join("h.toml"),
            account: "work".into(),
            mailboxes: mailboxes.iter().map(|m| m.to_string()).collect(),
            expected_version: "2.1.0".into(),
            timeout_seconds: 5,
            max_output_bytes: 100_000,
        })
        .unwrap()
    }

    fn reads(&self) -> usize {
        fs::read_to_string(self.dir.path().join("calls.log"))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains("message read"))
            .count()
    }
}

fn folders(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| n.to_string()).collect()
}

const TOML: &str = r#"
[mailbox.alias]
news = "Lists/News"
sent = "Sent Items"

[accounts.work]
imap.server = "imaps://x.test"

[accounts.work.mailbox.alias]
SENT = "Sent"
"#;

#[test]
fn global_aliases_count_and_the_account_overrides_them() {
    for version in ["2.1.0", "2.2.1"] {
        let f = Fixture::new(version, TOML);
        let h = f.engine(&["INBOX"]);
        let conflicts = h
            .alias_conflicts(&folders(&["INBOX", "News", "Sent", "Archive"]))
            .unwrap();
        // `news` is global; `sent` is overridden by the account's `SENT`,
        // which leads to `Sent` itself.
        assert_eq!(conflicts, folders(&["News"]), "{version}");
    }
}

#[test]
fn the_inbox_role_of_2_2_1_is_not_a_conflict() {
    let f = Fixture::new(
        "2.2.1",
        "[accounts.work]\nimap.server = \"imaps://x.test\"\n",
    );
    let h = f.engine(&["INBOX"]);
    assert!(h
        .alias_conflicts(&folders(&["INBOX", "Inbox", "inbox"]))
        .unwrap()
        .is_empty());
}

#[test]
fn a_folder_that_leads_elsewhere_is_never_read() {
    let f = Fixture::new(
        "2.2.1",
        "[accounts.work]\nimap.server = \"imaps://x.test\"\n[accounts.work.mailbox.alias]\ninbox = \"Archive\"\n",
    );
    let h = f.engine(&["INBOX"]);
    let error = h.fetch("INBOX", 42).unwrap_err();
    assert!(
        error.to_string().starts_with("alias_conflict:INBOX"),
        "{error}"
    );
    assert_eq!(f.reads(), 0, "message read must not run");
}

#[test]
fn a_configuration_himalaya_cannot_parse_reads_nothing() {
    let f = Fixture::new(
        "2.1.0",
        "[accounts.work]\nimap.server = \"imaps://x.test\"\n[accounts.work.mailbox]\nalias = 3\n",
    );
    let h = f.engine(&["INBOX"]);
    assert!(h.alias_conflicts(&folders(&["INBOX"])).is_err());
    assert!(h.fetch("INBOX", 42).is_err());
    assert_eq!(f.reads(), 0);
    // A readable configuration reads as before.
    fs::write(
        f.dir.path().join("h.toml"),
        "[accounts.work]\nimap.server = \"imaps://x.test\"\n",
    )
    .unwrap();
    let h = f.engine(&["INBOX"]);
    assert_eq!(h.fetch("INBOX", 42).unwrap(), b"Subject: hi\r\n\r\nbody");
    assert_eq!(f.reads(), 1);
}
