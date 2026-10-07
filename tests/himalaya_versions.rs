#![cfg(unix)]
//! Tested Himalaya versions: any listed version is accepted whatever
//! `expected_version` says, and an untested one is reported by `doctor`,
//! refused by passes (exit 3) and by setup.
use mailtriage::{domain::HimalayaConfig, engine::himalaya::Himalaya};
use serde_json::{json, Value};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, process::Command};

/// A fake Himalaya that prints the `version` file next to it for
/// `--version`, lists the account `work`, passes `account check`, reports
/// capabilities without SPECIAL-USE, lists INBOX, and fails everything else.
const FAKE: &str = r#"#!/bin/sh
dir="$(dirname "$0")"
for a in "$@"; do last="$a"; done
case "$last" in
  --version) cat "$dir/version"; exit 0 ;;
esac
case "$*" in
  *"account list"*) printf '{"accounts":[{"name":"work","default":true,"backends":["imap"]}]}' ;;
  *"account check"*) printf '{"account":"work","backends":[{"backend":"imap","ok":true}]}' ;;
  *"imap raw"*) printf '* CAPABILITY IMAP4rev1 MOVE\r\na1 OK done\r\n* NAMESPACE (("" "/")) NIL NIL\r\na2 OK done\r\n' ;;
  *"imap list"*) printf '{"mailboxes":[{"name":"INBOX","delimiter":"/","attributes":[]}]}' ;;
  *) exit 7 ;;
esac
"#;

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new(version_line: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let f = Self { dir };
        let bin = f.bin();
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("himalaya"), FAKE).unwrap();
        fs::set_permissions(bin.join("himalaya"), fs::Permissions::from_mode(0o755)).unwrap();
        f.set_version(version_line);
        fs::create_dir_all(f.home().join(".config/himalaya")).unwrap();
        fs::write(
            f.toml(),
            "[accounts.work]\nemail = \"work@example.test\"\nimap.server = \"imaps://mail.example.test\"\n",
        )
        .unwrap();
        f
    }

    fn root(&self) -> PathBuf {
        fs::canonicalize(self.dir.path()).unwrap()
    }
    fn bin(&self) -> PathBuf {
        self.root().join("bin")
    }
    fn home(&self) -> PathBuf {
        self.root().join("home")
    }
    fn toml(&self) -> PathBuf {
        self.home().join(".config/himalaya/config.toml")
    }
    fn set_version(&self, line: &str) {
        fs::write(self.bin().join("version"), line).unwrap();
    }

    /// `init` plus an engine for the fake, expecting `expected`.
    fn config(&self, expected: &str) -> PathBuf {
        let path = self.root().join("mailtriage.json");
        let (code, v) = self.run(&["init", "--json", "--config", path.to_str().unwrap()]);
        assert_eq!(code, Some(0), "{v}");
        let mut c: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        c["accounts"]["work"]["engine"] = json!({
            "kind": "himalaya", "binary": self.bin().join("himalaya"), "config": self.toml(),
            "account": "work", "mailboxes": ["INBOX"], "expected_version": expected,
            "timeout_seconds": 5, "max_output_bytes": 100000
        });
        fs::write(&path, c.to_string()).unwrap();
        path
    }

    fn run(&self, args: &[&str]) -> (Option<i32>, Value) {
        let out = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
            .args(args)
            .current_dir(self.root())
            .env("HOME", self.home())
            .env("XDG_CACHE_HOME", self.root().join("cache"))
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin().display()))
            .env_remove("MAILTRIAGE_CONFIG")
            .env_remove("HIMALAYA_CONFIG")
            .env_remove("XDG_CONFIG_HOME")
            .output()
            .unwrap();
        (
            out.status.code(),
            serde_json::from_slice(&out.stdout).unwrap_or(Value::Null),
        )
    }
}

fn engine(f: &Fixture, expected: &str) -> Himalaya {
    Himalaya::new(&HimalayaConfig {
        binary: f.bin().join("himalaya"),
        config: f.toml(),
        account: "work".into(),
        mailboxes: vec!["INBOX".into()],
        expected_version: expected.into(),
        timeout_seconds: 5,
        max_output_bytes: 100_000,
    })
    .unwrap()
}

#[test]
fn a_config_that_expects_2_1_0_accepts_2_2_1() {
    let f = Fixture::new("himalaya v2.2.1 +smtp +imap\nbuild: test\n");
    let h = engine(&f, "2.1.0");
    assert_eq!(h.version().unwrap(), "himalaya v2.2.1 +smtp +imap");
    assert_eq!(h.tested().unwrap().version, "2.2.1");
    // A value no binary ever wrote is not compared either.
    assert!(engine(&f, "9.9.9").version().is_ok());
}

#[test]
fn doctor_reports_whether_the_version_is_tested() {
    let f = Fixture::new("himalaya v2.2.1 +imap\n");
    let config = f.config("2.1.0");
    let config = config.to_str().unwrap();
    let (code, v) = f.run(&["doctor", "--account", "work", "--json", "--config", config]);
    assert_eq!(code, Some(0), "{v}");
    let t = &v["transport"];
    assert_eq!(
        (
            t["ready"].clone(),
            t["tested"].clone(),
            t["version"].clone()
        ),
        (json!(true), json!(true), json!("himalaya v2.2.1 +imap"))
    );
    f.set_version("himalaya v2.2.2 +imap\n");
    let (code, v) = f.run(&["doctor", "--account", "work", "--json", "--config", config]);
    assert_eq!(code, Some(0), "{v}");
    let t = &v["transport"];
    assert_eq!(t["ready"], false, "{v}");
    assert_eq!(t["tested"], false);
    assert_eq!(t["version"], "himalaya v2.2.2 +imap");
    assert_eq!(
        t["error"],
        "Himalaya 2.2.2 is not a tested version (tested: 2.1.0, 2.2.1)"
    );
    assert_eq!(v["ready"], false);
}

#[test]
fn a_pass_with_an_untested_version_exits_3_naming_it() {
    let f = Fixture::new("himalaya v2.2.2 +imap\n");
    let config = f.config("2.2.1");
    let (code, v) = f.run(&[
        "sync",
        "--account",
        "work",
        "--json",
        "--config",
        config.to_str().unwrap(),
    ]);
    assert_eq!(code, Some(3), "{v}");
    assert_eq!(
        v["error"]["message"],
        "Himalaya 2.2.2 is not a tested version (tested: 2.1.0, 2.2.1)"
    );
}

#[test]
fn setup_accepts_any_tested_version_and_writes_it() {
    let f = Fixture::new("himalaya v2.2.1 +imap\n");
    let setup = [
        "setup",
        "--yes",
        "--json",
        "--himalaya-account",
        "work",
        "--provider",
        "fake",
    ];
    let (code, v) = f.run(&setup);
    assert_eq!(code, Some(0), "{v}");
    let written: Value = serde_json::from_slice(
        &fs::read(f.home().join(".config/mailtriage/mailtriage.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        written["accounts"]["work"]["engine"]["expected_version"],
        "2.2.1"
    );
    fs::remove_file(f.home().join(".config/mailtriage/mailtriage.json")).unwrap();
    f.set_version("himalaya v2.2.2 +imap\n");
    let (code, v) = f.run(&setup);
    assert_eq!(code, Some(3), "{v}");
    let message = v["error"]["message"].as_str().unwrap();
    assert!(message.starts_with("step 2 (Himalaya): "), "{message}");
    assert!(
        message.contains("Himalaya 2.2.2 is not a tested version (tested: 2.1.0, 2.2.1)"),
        "{message}"
    );
}
