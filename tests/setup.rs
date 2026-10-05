#![cfg(unix)]
//! `mailtriage setup` through the binary, with a fake Himalaya first on PATH
//! and HOME in a temp dir. Prompts are driven with `--interactive` and piped
//! stdin; every other run uses `--yes`.
mod common;
use common::{write_tool, LAUNCHCTL, SYSTEMCTL};
use mailtriage::secrets;
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Output, Stdio},
};

/// Fake Himalaya v2.1.0. Accounts come from `accounts.json` next to the
/// script; accounts listed in `failing.json` fail `account check`;
/// `configure` adds the account `fresh`. Every call is logged to `calls.log`.
const HIMALAYA: &str = r#"#!/usr/bin/env python3
import json, os, sys
here = os.path.dirname(os.path.abspath(__file__))
argv = sys.argv[1:]
with open(os.path.join(here, "calls.log"), "a") as log:
    log.write(json.dumps(argv) + "\n")
def out(text):
    sys.stdout.write(text)
    sys.stdout.flush()
def load(name, default):
    try:
        with open(os.path.join(here, name)) as f:
            return json.load(f)
    except FileNotFoundError:
        return default
if argv and argv[-1] == "--version":
    out("himalaya v2.1.0 +smtp +imap +maildir\nbuild: test\n")
    sys.exit(0)
opts, rest, i = {}, [], 0
while i < len(argv):
    if argv[i] in ("--config", "--account", "--backend"):
        opts[argv[i]] = argv[i + 1]
        i += 2
    elif argv[i] == "--json":
        i += 1
    else:
        rest.append(argv[i])
        i += 1
accounts = load("accounts.json", [])
if rest == ["account", "list"]:
    out(json.dumps({"accounts": accounts}))
elif rest == ["account", "check"]:
    ok = opts.get("--account") not in load("failing.json", [])
    out(json.dumps({"account": opts.get("--account"), "backends": [
        {"backend": "imap", "ok": ok, "error": None if ok else "SECRET-DETAIL refused"}]}))
elif rest == ["configure"]:
    accounts.append({"name": "fresh", "default": False, "backends": ["imap"]})
    with open(os.path.join(here, "accounts.json"), "w") as f:
        json.dump(accounts, f)
    toml = opts.get("--config") or os.path.join(os.environ["HOME"], ".config", "himalaya", "config.toml")
    os.makedirs(os.path.dirname(toml), exist_ok=True)
    with open(toml, "a") as f:
        f.write('\n[accounts.fresh]\nemail = "fresh@example.test"\nimap.server = "imaps://fresh.example.test"\n')
    out("Account fresh configured.\n")
elif rest[:2] == ["imap", "raw"]:
    text = rest[-1]
    if "CAPABILITY" in text:
        out('* CAPABILITY IMAP4rev1 MOVE UIDPLUS SPECIAL-USE\r\na1 OK done\r\n* NAMESPACE (("" "/")) NIL NIL\r\na2 OK done\r\n')
    elif "RETURN (SPECIAL-USE)" in text:
        out('* LIST (\\HasNoChildren) "/" "INBOX"\r\n* LIST (\\HasNoChildren \\Sent) "/" "Sent"\r\na1 OK done\r\n')
    else:
        sys.exit(5)
elif rest[:2] == ["imap", "list"]:
    rows = [{"name": "INBOX", "delimiter": "/", "attributes": []},
            {"name": "Sent", "delimiter": "/", "attributes": ["\\Sent"]},
            {"name": "Lists", "delimiter": "/", "attributes": ["\\Noselect"]},
            {"name": "Lists/News", "delimiter": "/", "attributes": []},
            {"name": "Caf&AOk-", "delimiter": "/", "attributes": []}]
    out(json.dumps({"mailboxes": rows}))
else:
    sys.exit(7)
"#;

const TOML: &str = "[accounts.home]\nemail = \"home@example.test\"\nimap.server = \"imaps://home.example.test\"\n\n[accounts.work]\ndefault = true\nemail = \"work@example.test\"\nimap.server = \"imaps://mail.example.test\"\nimap.sasl.plain.username = \"work@example.test\"\nimap.sasl.plain.password.raw = \"fixture-pass\"\n";

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn message(v: &Value) -> String {
    v["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

struct Fixture {
    _dir: tempfile::TempDir,
    home: PathBuf,
    bin: PathBuf,
    cwd: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let bin = dir.path().join("bin");
        let cwd = dir.path().join("cwd");
        for d in [&home, &bin, &cwd] {
            fs::create_dir_all(d).unwrap();
        }
        write_tool(&bin, "himalaya", HIMALAYA);
        let f = Self {
            _dir: dir,
            home,
            bin,
            cwd,
        };
        f.set_accounts(json!([
            {"name": "home", "default": false, "backends": ["imap"]},
            {"name": "work", "default": true, "backends": ["imap"]}
        ]));
        fs::create_dir_all(f.himalaya_toml().parent().unwrap()).unwrap();
        fs::write(f.himalaya_toml(), TOML).unwrap();
        // Fake key tools first on PATH, so no test reaches a real keychain.
        f.add_key_tools();
        f
    }

    fn himalaya_toml(&self) -> PathBuf {
        self.home.join(".config/himalaya/config.toml")
    }

    fn set_accounts(&self, accounts: Value) {
        fs::write(self.bin.join("accounts.json"), accounts.to_string()).unwrap();
    }

    fn fail_check(&self, account: &str) {
        fs::write(self.bin.join("failing.json"), json!([account]).to_string()).unwrap();
    }

    fn calls(&self) -> String {
        fs::read_to_string(self.bin.join("calls.log")).unwrap_or_default()
    }

    fn config_path(&self) -> PathBuf {
        self.home.join(".config/mailtriage/mailtriage.json")
    }

    fn config(&self) -> Value {
        serde_json::from_slice(&fs::read(self.config_path()).unwrap()).unwrap()
    }

    /// Runs mailtriage with this fixture's HOME, PATH and time zone, extra
    /// environment `env`, and `stdin` piped in; prompted runs without
    /// `--service` get `--service skip`.
    fn run_with(&self, args: &[&str], stdin: &str, env: &[(&str, &str)]) -> (Output, Value) {
        let mut args = args.to_vec();
        if args.contains(&"--interactive") && !args.contains(&"--service") {
            args.extend(["--service", "skip"]);
        }
        self.run_exact(&args, stdin, env)
    }

    /// `run_with` without the automatic `--service skip`.
    fn run_exact(&self, args: &[&str], stdin: &str, env: &[(&str, &str)]) -> (Output, Value) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mailtriage"));
        command
            .current_dir(&self.cwd)
            .args(args)
            .env("HOME", &self.home)
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("TZ", "Europe/Berlin")
            .env_remove("MAILTRIAGE_CONFIG")
            .env_remove("HIMALAYA_CONFIG")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("OPENROUTER_API_KEY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = command.spawn().unwrap();
        let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
        let out = child.wait_with_output().unwrap();
        let value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
        (out, value)
    }

    fn run(&self, args: &[&str], stdin: &str) -> (Output, Value) {
        self.run_with(args, stdin, &[])
    }
}

const WORK_ENV: [&str; 7] = [
    "setup",
    "--yes",
    "--json",
    "--himalaya-account",
    "work",
    "--key-store",
    "env",
];

#[test]
fn flags_alone_write_a_ready_config() {
    let f = Fixture::new();
    let (out, v) = f.run_with(&WORK_ENV, "", &[("OPENROUTER_API_KEY", "sk-or-fixture")]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let s = &v["setup"];
    assert_eq!(s["account"], "work");
    assert_eq!(s["mailboxes"], json!(["INBOX"]));
    assert_eq!(s["provider"], "openrouter");
    assert_eq!(s["model"], "typesafe/jev-1.13");
    assert_eq!(s["key_source"], "env");
    assert_eq!(s["key_store"], "env");
    assert_eq!(s["filing"], "dry_run");
    assert_eq!(s["doctor"]["ready"], true, "{}", s["doctor"]);
    assert_eq!(
        s["config"],
        fs::canonicalize(f.config_path()).unwrap().to_str().unwrap()
    );
    let c = f.config();
    assert_eq!(c["state_dir"], "state");
    assert_eq!(c["provider"]["api_key_env"], "OPENROUTER_API_KEY");
    assert!(c["provider"].get("api_key_command").is_none());
    let a = &c["accounts"]["work"];
    assert_eq!(a["identity"], "work@example.test");
    assert_eq!(a["timezone"], "Europe/Berlin");
    assert_eq!(a["filing"]["mode"], "dry_run");
    let e = &a["engine"];
    assert_eq!(e["kind"], "himalaya");
    assert_eq!(e["account"], "work");
    assert_eq!(e["binary"], f.bin.join("himalaya").to_str().unwrap());
    assert_eq!(e["config"], f.himalaya_toml().to_str().unwrap());
    assert_eq!(e["mailboxes"], json!(["INBOX"]));
    assert_eq!(e["expected_version"], "2.1.0");
    let categories = a["categories"].as_array().unwrap();
    assert_eq!(categories.len(), 6);
    assert!(categories.iter().all(|c| c["folder"] == c["name"]));
    let mode = |p: PathBuf| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(f.config_path()), 0o600);
    assert_eq!(mode(f.config_path().with_file_name("state")), 0o700);
    assert!(!stdout(&out).contains("sk-or-fixture"));
    assert!(!stderr(&out).contains("sk-or-fixture"));
    // No IMAP changes, no wizard.
    for forbidden in [
        "\"configure\"",
        "\"create\"",
        "\"subscribe\"",
        "MOVE",
        "STORE",
    ] {
        assert!(!f.calls().contains(forbidden), "{forbidden}");
    }
    // Other commands now find the home config without --config.
    let (out, v) = f.run(&["doctor", "--account", "work", "--json"], "");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(v["provider"]["kind"], "openrouter");
}

#[test]
fn without_prompts_missing_answers_name_their_flag() {
    let f = Fixture::new();
    let cases: [(&[&str], &str); 4] = [
        (
            &["setup", "--yes", "--json", "--key-store", "env"],
            "--himalaya-account",
        ),
        (
            &[
                "setup",
                "--yes",
                "--json",
                "--himalaya-account",
                "nobody",
                "--key-store",
                "env",
            ],
            "--himalaya-account",
        ),
        (
            &[
                "setup",
                "--yes",
                "--json",
                "--himalaya-account",
                "work",
                "--key-store",
                "command",
            ],
            "--key-command",
        ),
        (
            &[
                "setup",
                "--yes",
                "--json",
                "--himalaya-account",
                "work",
                "--key-store",
                "env",
                "--account",
                "my work",
            ],
            "--account",
        ),
    ];
    for (args, flag) in cases {
        let (out, v) = f.run(args, "");
        assert_eq!(out.status.code(), Some(2), "{args:?}: {v}");
        assert!(message(&v).contains(flag), "{args:?}: {v}");
    }
    assert!(!f.config_path().exists());
}

#[test]
fn an_existing_config_needs_update_and_keeps_other_accounts() {
    let f = Fixture::new();
    assert_eq!(f.run(&WORK_ENV, "").0.status.code(), Some(0));
    // The user's own changes to the work account.
    let mut c = f.config();
    c["accounts"]["work"]["categories"][0]["description"] = json!("People I write with");
    c["accounts"]["work"]["filing"]["mode"] = json!("live");
    fs::write(f.config_path(), serde_json::to_vec_pretty(&c).unwrap()).unwrap();
    let before = f.config();

    let (out, v) = f.run(&WORK_ENV, "");
    assert_eq!(out.status.code(), Some(5));
    assert!(message(&v).contains("--update"), "{v}");
    assert_eq!(f.config(), before);

    let (out, v) = f.run(
        &[
            "setup",
            "--yes",
            "--update",
            "--json",
            "--himalaya-account",
            "home",
            "--account",
            "home",
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(v["setup"]["key_store"], Value::Null);
    let after = f.config();
    assert_eq!(after["accounts"]["work"], before["accounts"]["work"]);
    assert_eq!(after["provider"], before["provider"]);
    assert_eq!(after["accounts"]["home"]["identity"], "home@example.test");

    // Updating work keeps its categories and its live mode.
    let (out, _) = f.run(
        &[
            "setup",
            "--yes",
            "--update",
            "--json",
            "--himalaya-account",
            "work",
            "--brief",
            "Runs a bakery",
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let work = &f.config()["accounts"]["work"];
    assert_eq!(work["brief"], "Runs a bakery");
    assert_eq!(work["filing"]["mode"], "live");
    assert_eq!(work["categories"][0]["description"], "People I write with");
}

/// Final review I1: an update that would change a bound account's identity,
/// Himalaya account or IMAP server is refused before anything is written,
/// and so is a fresh setup over a state directory that outlived its config.
#[test]
fn a_binding_changing_update_is_refused_before_writing() {
    let f = Fixture::new();
    assert_eq!(f.run(&WORK_ENV, "").0.status.code(), Some(0));
    let before = fs::read(f.config_path()).unwrap();
    let update = ["setup", "--yes", "--update", "--json", "--account", "work"];
    for extra in [
        ["--identity", "other@example.test"],
        ["--himalaya-account", "home"],
    ] {
        let (out, v) = f.run(&[&update[..], &extra[..]].concat(), "");
        assert_eq!(out.status.code(), Some(5), "{extra:?}: {v}");
        let m = message(&v);
        assert!(
            m.starts_with(
                "step 3 (account): account work is bound to its previous mailbox (identity, Himalaya account or IMAP server changed); "
            ),
            "{m}"
        );
        assert!(m.contains("--account NEW"), "{m}");
        assert_eq!(fs::read(f.config_path()).unwrap(), before, "{extra:?}");
    }
    // An update that keeps the binding still works.
    let (out, _) = f.run(&[&update[..], &["--brief", "Runs a bakery"]].concat(), "");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(f.config()["accounts"]["work"]["brief"], "Runs a bakery");
    // The config is gone, its state is not.
    fs::remove_file(f.config_path()).unwrap();
    let (out, v) = f.run(
        &[&WORK_ENV[..], &["--identity", "other@example.test"]].concat(),
        "",
    );
    assert_eq!(out.status.code(), Some(5), "{v}");
    assert!(message(&v).starts_with("step 3 (account): "), "{v}");
    assert!(!f.config_path().exists());
    let (out, _) = f.run(&WORK_ENV, "");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
}

/// Final review I1: a service whose every pass would fail is not installed.
#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn a_failed_state_check_installs_no_service() {
    let f = Fixture::new();
    write_tool(&f.bin, "launchctl", LAUNCHCTL);
    write_tool(&f.bin, "systemctl", SYSTEMCTL);
    assert_eq!(f.run(&WORK_ENV, "").0.status.code(), Some(0));
    // A state database from a newer mailtriage cannot be opened.
    let db = f.config_path().with_file_name("state/mailtriage.sqlite");
    rusqlite::Connection::open(&db)
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();
    let (out, v) = f.run(
        &[
            "setup",
            "--yes",
            "--update",
            "--json",
            "--account",
            "work",
            "--service",
            "install",
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let items = v["setup"]["doctor"]["items"].as_array().unwrap();
    assert!(
        items
            .iter()
            .any(|i| i["check"] == "state" && i["ready"] == false),
        "{items:?}"
    );
    assert_eq!(v["setup"]["service"], Value::Null);
    assert!(
        stderr(&out).contains("Not installing the background service"),
        "{}",
        stderr(&out)
    );
    assert!(!f.bin.join("launchctl.log").exists());
    assert!(!f.bin.join("systemctl.log").exists());
}

#[test]
fn prompts_offer_defaults_and_ask_again_after_invalid_input() {
    let f = Fixture::new();
    // Himalaya account (9 is out of range, then Enter for work); name ("my
    // work" is invalid, then Enter); identity; time zone; brief; folders 1
    // and 3; provider; model; key variable; filing.
    let input = "9\n\nmy work\n\n\n\nRuns a bakery\n1,3\n\n\n\n\n";
    let (out, v) = f.run(
        &["setup", "--interactive", "--json", "--key-store", "env"],
        input,
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("Please enter a number from 1 to 3."), "{err}");
    assert!(
        err.contains("Use 1 to 64 ASCII letters, digits, - or _."),
        "{err}"
    );
    assert!(err.contains("Time zone [Europe/Berlin]"), "{err}");
    assert!(
        !err.contains(") Lists\n"),
        "a \\Noselect folder is offered: {err}"
    );
    let a = &f.config()["accounts"]["work"];
    assert_eq!(a["brief"], "Runs a bakery");
    assert_eq!(a["engine"]["mailboxes"], json!(["INBOX", "Lists/News"]));
    assert_eq!(v["setup"]["filing"], "dry_run");
}

#[test]
fn end_of_input_aborts_without_writing() {
    let f = Fixture::new();
    let (out, v) = f.run(&["setup", "--interactive", "--json"], "");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(message(&v), "setup aborted: input ended");
    assert!(!f.config_path().exists());
    // Ending after the account menu and the name is no different.
    let (out, _) = f.run(&["setup", "--interactive", "--json"], "\n\n");
    assert_eq!(out.status.code(), Some(2));
    assert!(!f.config_path().exists());
}

#[test]
fn choosing_abort_changes_nothing() {
    let f = Fixture::new();
    assert_eq!(f.run(&WORK_ENV, "").0.status.code(), Some(0));
    let before = fs::read(f.config_path()).unwrap();
    let (out, v) = f.run(&["setup", "--interactive", "--json"], "3\n");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(message(&v), "setup aborted; nothing was changed");
    assert_eq!(fs::read(f.config_path()).unwrap(), before);
}

#[test]
fn without_an_account_setup_runs_himalaya_configure() {
    let f = Fixture::new();
    fs::remove_file(f.himalaya_toml()).unwrap();
    f.set_accounts(json!([]));
    let (out, v) = f.run(&WORK_ENV, "");
    assert_eq!(out.status.code(), Some(3));
    assert!(message(&v).contains("himalaya configure"), "{v}");
    assert!(!f.calls().contains("\"configure\""));
    // Confirm creating one, take it (the default), then every default.
    let input = "\n".repeat(11);
    let (out, v) = f.run(
        &["setup", "--interactive", "--json", "--key-store", "env"],
        &input,
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(f.calls().contains("\"configure\""));
    assert_eq!(v["setup"]["account"], "fresh");
    assert_eq!(
        f.config()["accounts"]["fresh"]["identity"],
        "fresh@example.test"
    );
}

#[test]
fn a_failed_account_check_exits_3_without_himalaya_output() {
    let f = Fixture::new();
    f.fail_check("work");
    let (out, v) = f.run(&WORK_ENV, "");
    assert_eq!(out.status.code(), Some(3));
    assert!(message(&v).contains("account check"), "{v}");
    assert!(!stdout(&out).contains("SECRET-DETAIL"));
    assert!(!stderr(&out).contains("SECRET-DETAIL"));
    assert!(!f.config_path().exists());
}

#[test]
fn a_key_command_is_verified_and_stored_as_sh_c() {
    let f = Fixture::new();
    let args = [
        "setup",
        "--yes",
        "--json",
        "--himalaya-account",
        "work",
        "--key-command",
    ];
    let (out, v) = f.run(&[&args[..], &["printf 'sk-or-cmd-123\\n'"]].concat(), "");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(v["setup"]["key_store"], "command");
    assert_eq!(v["setup"]["key_source"], "command");
    assert_eq!(
        v["setup"]["doctor"]["ready"], true,
        "{}",
        v["setup"]["doctor"]
    );
    assert_eq!(
        f.config()["provider"]["api_key_command"],
        json!(["/bin/sh", "-c", "printf 'sk-or-cmd-123\\n'"])
    );
    assert_eq!(f.config()["provider"]["api_key_env"], "OPENROUTER_API_KEY");
    assert!(!stdout(&out).contains("sk-or-cmd-123"));
    assert!(!stderr(&out).contains("sk-or-cmd-123"));

    fs::remove_file(f.config_path()).unwrap();
    let (out, v) = f.run(&[&args[..], &["exit 1"]].concat(), "");
    assert_eq!(out.status.code(), Some(3));
    assert!(
        message(&v).contains("API key command failed (exit 1)"),
        "{v}"
    );
    assert!(!f.config_path().exists());
}

#[test]
fn watched_folders_must_exist_and_be_safe_to_file_from() {
    let f = Fixture::new();
    let with = |extra: &[&str]| f.run(&[&WORK_ENV[..], extra].concat(), "");
    let (out, v) = with(&["--mailbox", "Nope"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(message(&v).contains("Nope"), "{v}");
    let (out, v) = with(&["--mailbox", "Caf&AOk-"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(message(&v).contains("--filing off"), "{v}");
    let (out, v) = with(&[
        "--mailbox",
        "INBOX",
        "--mailbox",
        "Caf&AOk-",
        "--filing",
        "off",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(v["setup"]["mailboxes"], json!(["INBOX", "Caf&AOk-"]));
    assert_eq!(v["setup"]["filing"], "off");
}

#[test]
fn the_offline_classifier_needs_no_key() {
    let f = Fixture::new();
    let (out, v) = f.run(
        &[
            "setup",
            "--yes",
            "--json",
            "--himalaya-account",
            "work",
            "--provider",
            "fake",
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(v["setup"]["provider"], "fake");
    assert_eq!(v["setup"]["key_source"], Value::Null);
    assert_eq!(
        v["setup"]["doctor"]["ready"], true,
        "{}",
        v["setup"]["doctor"]
    );
    assert_eq!(f.config()["provider"]["kind"], "fake");
}

#[test]
fn doctor_items_that_are_not_ready_carry_a_fix() {
    let f = Fixture::new();
    let (out, v) = f.run(&WORK_ENV, "");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doctor = &v["setup"]["doctor"];
    assert_eq!(doctor["ready"], false);
    let key = doctor["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["check"] == "key")
        .unwrap();
    assert_eq!(key["ready"], false);
    assert!(
        key["fix"]
            .as_str()
            .unwrap()
            .contains("export OPENROUTER_API_KEY="),
        "{key}"
    );
    assert!(stderr(&out).contains("export OPENROUTER_API_KEY="));
}

#[test]
fn setup_writes_the_home_config_unless_told_otherwise() {
    let f = Fixture::new();
    // An older ./mailtriage.json is left alone.
    assert!(f.run(&["init", "--json"], "").0.status.success());
    let local = fs::read(f.cwd.join("mailtriage.json")).unwrap();
    assert_eq!(f.run(&WORK_ENV, "").0.status.code(), Some(0));
    assert!(f.config_path().exists());
    assert_eq!(fs::read(f.cwd.join("mailtriage.json")).unwrap(), local);
    // MAILTRIAGE_CONFIG, then --config (a directory with a space).
    let env_path = f.cwd.join("env.json");
    let (out, _) = f.run_with(
        &WORK_ENV,
        "",
        &[("MAILTRIAGE_CONFIG", env_path.to_str().unwrap())],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(env_path.exists());
    let flag_path = f.cwd.join("Some Dir/flag.json");
    let (out, v) = f.run(
        &[&WORK_ENV[..], &["--config", flag_path.to_str().unwrap()]].concat(),
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(flag_path.exists());
    assert!(v["setup"]["config"]
        .as_str()
        .unwrap()
        .ends_with("Some Dir/flag.json"));
}

#[test]
fn a_himalaya_config_path_with_spaces_is_kept_whole() {
    let f = Fixture::new();
    let toml = f
        .home
        .join("Library/Application Support/himalaya/config.toml");
    fs::create_dir_all(toml.parent().unwrap()).unwrap();
    fs::rename(f.himalaya_toml(), &toml).unwrap();
    let (out, _) = f.run(
        &[
            &WORK_ENV[..],
            &["--himalaya-config", toml.to_str().unwrap()],
        ]
        .concat(),
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        f.config()["accounts"]["work"]["engine"]["config"],
        toml.to_str().unwrap()
    );
    let quoted = serde_json::to_string(toml.to_str().unwrap()).unwrap();
    assert!(f.calls().contains(&quoted), "{}", f.calls());
}

#[test]
fn changing_the_key_store_keeps_the_rest_of_the_classifier() {
    let f = Fixture::new();
    assert_eq!(f.run(&WORK_ENV, "").0.status.code(), Some(0));
    let mut c = f.config();
    c["provider"]["api_key_env"] = json!("MY_OPENROUTER_KEY");
    c["provider"]["model"] = json!("~typesafe/jev-latest");
    c["provider"]["endpoint"] = json!("http://127.0.0.1:9/api/alpha/decisions");
    c["provider"]["timeout_seconds"] = json!(45);
    fs::write(f.config_path(), serde_json::to_vec_pretty(&c).unwrap()).unwrap();
    let before = f.config()["provider"].clone();
    let update = [
        "setup",
        "--yes",
        "--update",
        "--json",
        "--himalaya-account",
        "work",
    ];
    // To a key command: only api_key_command is added.
    let (out, v) = f.run(
        &[&update[..], &["--key-command", "printf 'k\\n'"]].concat(),
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(v["setup"]["key_store"], "command");
    let mut provider = f.config()["provider"].clone();
    assert_eq!(
        provider["api_key_command"],
        json!(["/bin/sh", "-c", "printf 'k\\n'"])
    );
    provider.as_object_mut().unwrap().remove("api_key_command");
    assert_eq!(provider, before);
    // Back to the variable: the provider is the original again.
    let (out, v) = f.run(&[&update[..], &["--key-store", "env"]].concat(), "");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(v["setup"]["key_store"], "env");
    assert_eq!(f.config()["provider"], before);
}

#[test]
fn updating_an_account_interactively_keeps_its_mailbox() {
    let f = Fixture::new();
    assert_eq!(f.run(&WORK_ENV, "").0.status.code(), Some(0));
    let (out, _) = f.run(
        &[
            "setup",
            "--yes",
            "--update",
            "--json",
            "--himalaya-account",
            "home",
            "--account",
            "home",
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let before = f.config();
    // Update (1), home (1), then Enter for the Himalaya account, identity,
    // time zone, brief, folders, keeping the classifier and filing.
    let input = format!("1\n1\n{}", "\n".repeat(7));
    let (out, _) = f.run(&["setup", "--interactive", "--json"], &input);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let after = f.config();
    assert_eq!(after["accounts"]["home"]["engine"]["account"], "home");
    assert_eq!(after["accounts"]["home"], before["accounts"]["home"]);
    assert_eq!(after["accounts"]["work"], before["accounts"]["work"]);
}

#[test]
fn updating_an_account_by_name_keeps_its_himalaya_settings() {
    let f = Fixture::new();
    assert_eq!(f.run(&WORK_ENV, "").0.status.code(), Some(0));
    // home uses a Himalaya binary and TOML that discovery would not find.
    let alt = f.home.join("Other Mail");
    fs::create_dir_all(&alt).unwrap();
    write_tool(&alt, "himalaya", HIMALAYA);
    fs::copy(f.bin.join("accounts.json"), alt.join("accounts.json")).unwrap();
    let binary = alt.join("himalaya");
    let toml = alt.join("himalaya.toml");
    fs::copy(f.himalaya_toml(), &toml).unwrap();
    let (out, _) = f.run(
        &[
            "setup",
            "--yes",
            "--update",
            "--json",
            "--account",
            "home",
            "--himalaya-account",
            "home",
            "--himalaya-binary",
            binary.to_str().unwrap(),
            "--himalaya-config",
            toml.to_str().unwrap(),
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let before = f.config();
    assert_eq!(
        before["accounts"]["home"]["engine"]["config"],
        toml.to_str().unwrap()
    );
    fs::remove_file(alt.join("calls.log")).unwrap();
    fs::remove_file(f.bin.join("calls.log")).unwrap();

    let (out, v) = f.run(
        &["setup", "--yes", "--update", "--json", "--account", "home"],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(v["setup"]["account"], "home");
    let after = f.config();
    assert_eq!(after["accounts"]["home"], before["accounts"]["home"]);
    assert_eq!(after["accounts"]["work"], before["accounts"]["work"]);
    // The stored binary ran the checks; the one on PATH was never asked.
    let calls = fs::read_to_string(alt.join("calls.log")).unwrap();
    assert!(calls.contains("\"check\""), "{calls}");
    assert!(!f.bin.join("calls.log").exists(), "{}", f.calls());
}

#[test]
fn an_account_flag_skips_the_config_menu() {
    let f = Fixture::new();
    assert_eq!(f.run(&WORK_ENV, "").0.status.code(), Some(0));
    let before = f.config();
    // A new name adds that account: Himalaya account 1 (home), then Enter
    // for identity, time zone, brief, folders, keeping the classifier and
    // filing.
    let input = format!("1\n{}", "\n".repeat(6));
    let (out, v) = f.run(
        &["setup", "--interactive", "--json", "--account", "home"],
        &input,
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(!stderr(&out).contains("What would you like to do?"));
    assert_eq!(v["setup"]["account"], "home");
    let added = f.config();
    assert_eq!(added["accounts"]["home"]["engine"]["account"], "home");
    assert_eq!(added["accounts"]["work"], before["accounts"]["work"]);
    // An existing name updates it: Enter for the Himalaya account and the
    // six questions after it.
    let (out, _) = f.run(
        &["setup", "--interactive", "--json", "--account", "work"],
        &"\n".repeat(7),
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(!err.contains("What would you like to do?"), "{err}");
    assert!(!err.contains("Which account?"), "{err}");
    let updated = f.config();
    assert_eq!(updated["accounts"]["work"], before["accounts"]["work"]);
    assert_eq!(updated["accounts"]["home"], added["accounts"]["home"]);
}

#[test]
fn every_error_names_its_step_and_a_fix() {
    let f = Fixture::new();
    f.add_key_tools();
    let w = ["--himalaya-account", "work"];
    let we = ["--himalaya-account", "work", "--key-store", "env"];
    let cases: Vec<(Vec<&str>, &str)> = vec![
        (vec!["--key-store", "env"], "step 2 (Himalaya): "),
        (
            vec!["--himalaya-account", "nobody", "--key-store", "env"],
            "step 2 (Himalaya): ",
        ),
        (
            [&we[..], &["--himalaya-config", "missing.toml"]].concat(),
            "step 2 (Himalaya): --himalaya-config: ",
        ),
        (
            [&we[..], &["--account", "my work"]].concat(),
            "step 3 (account): --account: ",
        ),
        (
            [&we[..], &["--identity", " "]].concat(),
            "step 3 (account): --identity: ",
        ),
        (
            [&we[..], &["--timezone", "Europe Berlin"]].concat(),
            "step 3 (account): --timezone: ",
        ),
        (
            [&we[..], &["--mailbox", "Nope"]].concat(),
            "step 4 (folders): --mailbox: ",
        ),
        (
            [&we[..], &["--model", "gpt-4"]].concat(),
            "step 5 (classifier): --model: ",
        ),
        (
            [&w[..], &["--key-store", "command"]].concat(),
            "step 5 (key): --key-command ",
        ),
        (
            [&w[..], &["--key-command", "x", "--key-env", "FOO"]].concat(),
            "step 5 (key): ",
        ),
        (
            [&we[..], &["--key-command", "x"]].concat(),
            "step 5 (key): ",
        ),
        (
            [&w[..], &["--key-env", "bad-name"]].concat(),
            "step 5 (key): --key-env: ",
        ),
        (
            [&w[..], &["--key-command", "exit 1"]].concat(),
            "step 5 (key): ",
        ),
        (
            [&w[..], &["--key-store", "pass"]].concat(),
            "step 5 (key): ",
        ),
        (
            [&w[..], &["--key-store", "pass", "--key-stored"]].concat(),
            "step 5 (key): ",
        ),
        (
            [&we[..], &["--mailbox", "Caf&AOk-"]].concat(),
            "step 7 (filing): ",
        ),
    ];
    for (extra, prefix) in cases {
        let (out, v) = f.run(&[&["setup", "--yes", "--json"][..], &extra].concat(), "");
        assert_ne!(out.status.code(), Some(0), "{extra:?}");
        let m = message(&v);
        assert!(m.starts_with(prefix), "{extra:?}: {m}");
        assert!(m.contains("--") || m.contains('`'), "{extra:?}: {m}");
    }
    // Without HOME, step 1 names --config.
    let (out, v) = f.run_with(&WORK_ENV, "", &[("HOME", "")]);
    assert_eq!(out.status.code(), Some(2));
    assert!(message(&v).starts_with("step 1 (config): "), "{v}");
    assert!(message(&v).contains("--config"), "{v}");
    assert!(!f.config_path().exists());
    // An existing config without --update.
    assert_eq!(f.run(&WORK_ENV, "").0.status.code(), Some(0));
    let (out, v) = f.run(&WORK_ENV, "");
    assert_eq!(out.status.code(), Some(5));
    assert!(message(&v).starts_with("step 1 (config): "), "{v}");
}

/// A fake key tool. The store verb reads one line from stdin, as the real
/// tools read the key from the terminal; the read verb prints it, or exits
/// 44 when nothing is stored. Calls are logged to `tools.log`.
fn key_tool(store_verb: &str, read_verb: &str, file: &str) -> String {
    format!(
        "#!/bin/sh\ndir=\"$(dirname \"$0\")\"\necho \"$*\" >> \"$dir/tools.log\"\ncase \"$1\" in\n  {store_verb}) IFS= read -r key || exit 1; printf '%s\\n' \"$key\" > \"$dir/{file}\" ;;\n  {read_verb}) [ -f \"$dir/{file}\" ] && cat \"$dir/{file}\" || exit 44 ;;\n  *) exit 64 ;;\nesac\n"
    )
}

impl Fixture {
    fn add_key_tools(&self) {
        write_tool(
            &self.bin,
            "security",
            &key_tool(
                "add-generic-password",
                "find-generic-password",
                "keychain.key",
            ),
        );
        write_tool(
            &self.bin,
            "secret-tool",
            &key_tool("store", "lookup", "secret-service.key"),
        );
        write_tool(&self.bin, "pass", &key_tool("insert", "show", "pass.key"));
    }

    fn stored(&self, file: &str) -> Option<String> {
        fs::read_to_string(self.bin.join(file))
            .ok()
            .map(|s| s.trim().to_owned())
    }

    fn tool_calls(&self) -> String {
        fs::read_to_string(self.bin.join("tools.log")).unwrap_or_default()
    }
}

#[test]
fn the_store_tool_asks_for_the_key_on_the_shared_stdin() {
    let f = Fixture::new();
    f.add_key_tools();
    // Defaults for account, name, identity, time zone, brief, folders,
    // provider and model; the line secret-tool reads; then filing.
    let input = format!("{}sk-or-stored-1\n\n", "\n".repeat(8));
    let (out, v) = f.run(
        &[
            "setup",
            "--interactive",
            "--json",
            "--key-store",
            "secret-service",
        ],
        &input,
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        f.stored("secret-service.key").as_deref(),
        Some("sk-or-stored-1")
    );
    assert_eq!(
        f.config()["provider"]["api_key_command"],
        json!([
            f.bin.join("secret-tool").to_str().unwrap(),
            "lookup",
            "service",
            "mailtriage",
            "provider",
            "openrouter"
        ])
    );
    assert_eq!(v["setup"]["key_store"], "secret-service");
    assert_eq!(
        v["setup"]["doctor"]["ready"], true,
        "{}",
        v["setup"]["doctor"]
    );
    assert!(!stdout(&out).contains("sk-or-stored-1"));
    assert!(!stderr(&out).contains("sk-or-stored-1"));
}

#[test]
fn a_stored_key_is_reused_without_prompts() {
    let f = Fixture::new();
    f.add_key_tools();
    fs::write(f.bin.join("pass.key"), "sk-or-old\n").unwrap();
    let (out, v) = f.run(
        &[
            "setup",
            "--yes",
            "--json",
            "--himalaya-account",
            "work",
            "--key-store",
            "pass",
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(v["setup"]["key_store"], "pass");
    assert!(!f.tool_calls().contains("insert"));
    assert_eq!(
        f.config()["provider"]["api_key_command"],
        json!([
            f.bin.join("pass").to_str().unwrap(),
            "show",
            "mailtriage/openrouter"
        ])
    );
}

#[test]
fn without_a_terminal_the_store_step_needs_key_stored() {
    let f = Fixture::new();
    f.add_key_tools();
    let base = [
        "setup",
        "--yes",
        "--json",
        "--himalaya-account",
        "work",
        "--key-store",
        "pass",
    ];
    let (out, v) = f.run(&base, "");
    assert_eq!(out.status.code(), Some(2));
    assert!(message(&v).contains("--key-stored"), "{v}");
    let (out, v) = f.run(&[&base[..], &["--key-stored"]].concat(), "");
    assert_eq!(out.status.code(), Some(3));
    assert!(message(&v).contains("no key found"), "{v}");
    assert!(!f.config_path().exists());
    assert!(!f.tool_calls().contains("insert"));
}

#[test]
fn a_stored_key_can_be_replaced_when_prompting() {
    let f = Fixture::new();
    f.add_key_tools();
    fs::write(f.bin.join("pass.key"), "sk-or-old\n").unwrap();
    // Eight defaults, decline reuse, the new key for pass, then filing.
    let input = format!("{}n\nsk-or-new\n\n", "\n".repeat(8));
    let (out, _) = f.run(
        &["setup", "--interactive", "--json", "--key-store", "pass"],
        &input,
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(f.stored("pass.key").as_deref(), Some("sk-or-new"));
}

#[test]
fn the_key_store_menu_starts_with_the_platform_default() {
    let f = Fixture::new();
    f.add_key_tools();
    let options =
        secrets::key_store_options(cfg!(target_os = "macos"), |tool| f.bin.join(tool).exists());
    // Eight defaults, the last menu entry (env), its variable, then filing.
    let input = format!("{}{}\n\n\n", "\n".repeat(8), options.len());
    let (out, v) = f.run_with(
        &["setup", "--interactive", "--json"],
        &input,
        &[("OPENROUTER_API_KEY", "sk-or-env")],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let first = if cfg!(target_os = "macos") {
        "keychain"
    } else {
        "secret-service"
    };
    assert_eq!(options[0].flag(), first);
    assert!(
        stderr(&out).contains(&format!("  1) {} (default)", options[0].label())),
        "{}",
        stderr(&out)
    );
    assert_eq!(v["setup"]["key_store"], "env");
}

#[test]
#[cfg(target_os = "macos")]
fn keychain_is_the_default_on_macos() {
    let f = Fixture::new();
    f.add_key_tools();
    fs::write(f.bin.join("keychain.key"), "sk-or-keychain\n").unwrap();
    let (out, v) = f.run(
        &["setup", "--yes", "--json", "--himalaya-account", "work"],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(v["setup"]["key_store"], "keychain");
    assert_eq!(
        f.config()["provider"]["api_key_command"],
        json!([
            f.bin.join("security").to_str().unwrap(),
            "find-generic-password",
            "-s",
            "mailtriage",
            "-a",
            "openrouter",
            "-w"
        ])
    );
}

#[test]
#[cfg(target_os = "linux")]
fn secret_service_is_the_default_on_linux_when_available() {
    let f = Fixture::new();
    f.add_key_tools();
    fs::write(f.bin.join("secret-service.key"), "sk-or-linux\n").unwrap();
    let (out, v) = f.run(
        &["setup", "--yes", "--json", "--himalaya-account", "work"],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(v["setup"]["key_store"], "secret-service");
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn setup_can_install_the_background_service() {
    let f = Fixture::new();
    write_tool(&f.bin, "launchctl", LAUNCHCTL);
    write_tool(&f.bin, "systemctl", SYSTEMCTL);
    let (out, v) = f.run(
        &[
            &WORK_ENV[..],
            &["--service", "install", "--interval-seconds", "300"],
        ]
        .concat(),
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let service = &v["setup"]["service"];
    assert_eq!(service["action"], "installed");
    let text = fs::read_to_string(service["unit_path"].as_str().unwrap()).unwrap();
    assert!(text.contains("300"), "{text}");
    // The key comes from an environment variable the service does not have.
    assert!(service["note"]
        .as_str()
        .unwrap()
        .contains("OPENROUTER_API_KEY"));
    if cfg!(target_os = "linux") {
        assert!(stderr(&out).contains("loginctl enable-linger"));
    }
    // Final review I3: the note names this platform's store and the
    // account; next steps leave the lock to the service.
    let note = service["note"].as_str().unwrap();
    let store = if cfg!(target_os = "macos") {
        "--key-store keychain`"
    } else {
        "--key-store secret-service`"
    };
    assert!(note.contains(" --account work --key-store "), "{note}");
    assert!(note.contains(store), "{note}");
    assert!(note.contains("setup --update --config "), "{note}");
    assert!(note.contains("`service install` rewrites it"), "{note}");
    let err = stderr(&out);
    assert!(
        err.contains("`mailtriage service status --account work`"),
        "{err}"
    );
    assert!(err.contains("`mailtriage list --account work`"), "{err}");
    assert!(!err.contains("`mailtriage sync"), "{err}");
    assert!(!err.contains("`mailtriage watch"), "{err}");
}

/// Final review I3: every command setup prints finds the written config.
#[test]
fn printed_commands_name_the_config_unless_it_is_found_without() {
    let f = Fixture::new();
    // The home config: found from anywhere, so no --config.
    let (out, _) = f.run(&WORK_ENV, "");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("`mailtriage sync --account work`"), "{err}");
    assert!(
        err.contains("`mailtriage categories export --account work > categories.json`"),
        "{err}"
    );
    assert!(!err.contains("--config"), "{err}");
    // Another path: every printed command carries it, quoted.
    let other = f.cwd.join("Some Dir/x.json");
    let (out, v) = f.run(
        &[&WORK_ENV[..], &["--config", other.to_str().unwrap()]].concat(),
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let flag = format!("--config '{}'", v["setup"]["config"].as_str().unwrap());
    let err = stderr(&out);
    for command in [
        "mailtriage categories export",
        "mailtriage categories validate",
        "mailtriage categories apply",
        "mailtriage sync",
        "mailtriage watch",
        "mailtriage filing plan",
        "mailtriage filing enable",
    ] {
        assert!(
            err.contains(&format!("`{command} {flag} --account work")),
            "{command}: {err}"
        );
    }
    assert!(!err.contains("takes precedence"), "{err}");
}

/// Final review I3: a `./mailtriage.json` in the working directory would
/// win over the written config, so setup warns and passes --config.
#[test]
fn a_shadowing_local_config_is_named_in_a_warning() {
    let f = Fixture::new();
    assert!(f.run(&["init", "--json"], "").0.status.success());
    let (out, v) = f.run(&WORK_ENV, "");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let written = v["setup"]["config"].as_str().unwrap();
    let err = stderr(&out);
    assert_eq!(err.matches("takes precedence over").count(), 1, "{err}");
    assert!(
        err.contains(&format!(
            "`mailtriage sync --config {written} --account work`"
        )),
        "{err}"
    );
}

/// Final review I3: the doctor's key fix keeps the store the key comes
/// from, and asks for it explicitly, so it does not keep the broken key.
#[test]
fn the_doctor_key_fix_names_the_current_store() {
    let f = Fixture::new();
    fs::write(f.bin.join("pass.key"), "sk-or-old\n").unwrap();
    let (out, _) = f.run(
        &[
            "setup",
            "--yes",
            "--json",
            "--himalaya-account",
            "work",
            "--key-store",
            "pass",
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let key_fix = |v: &Value| {
        v["setup"]["doctor"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["check"] == "key")
            .unwrap()["fix"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    let update = ["setup", "--yes", "--update", "--json", "--account", "work"];
    fs::remove_file(f.bin.join("pass.key")).unwrap();
    let (out, v) = f.run(&update, "");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        key_fix(&v).contains("`mailtriage setup --update --account work --key-store pass`"),
        "{v}"
    );
    // A command of the user's own.
    let key = f.home.join("key.txt");
    fs::write(&key, "sk-or-file\n").unwrap();
    let cat = format!("cat '{}'", key.display());
    let (out, _) = f.run(&[&update[..], &["--key-command", &cat]].concat(), "");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    fs::remove_file(&key).unwrap();
    let (out, v) = f.run(&update, "");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        key_fix(&v).contains("`mailtriage setup --update --account work --key-store command`"),
        "{v}"
    );
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn prompting_offers_the_service_with_yes_as_default() {
    let f = Fixture::new();
    write_tool(&f.bin, "launchctl", LAUNCHCTL);
    write_tool(&f.bin, "systemctl", SYSTEMCTL);
    // Eight defaults, filing, then Enter for the service.
    let input = "\n".repeat(10);
    let (out, v) = f.run_exact(
        &[
            "setup",
            "--interactive",
            "--json",
            "--key-command",
            "printf 'sk-or-k\\n'",
        ],
        &input,
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stderr(&out).contains("seconds)? [Y/n]"), "{}", stderr(&out));
    assert_eq!(v["setup"]["service"]["action"], "installed");
}

/// Final review I2: the service does not inherit the variable an `env` key
/// comes from, so the prompted question defaults to no and says why.
#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn the_service_question_defaults_to_no_for_a_key_from_the_environment() {
    let f = Fixture::new();
    write_tool(&f.bin, "launchctl", LAUNCHCTL);
    write_tool(&f.bin, "systemctl", SYSTEMCTL);
    // Eight defaults, key variable, filing, then Enter for the service.
    let (out, v) = f.run_exact(
        &["setup", "--interactive", "--json", "--key-store", "env"],
        &"\n".repeat(11),
        &[("OPENROUTER_API_KEY", "sk-or-env")],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(v["setup"]["service"], Value::Null);
    let err = stderr(&out);
    assert!(
        err.contains("OPENROUTER_API_KEY, which the background service does not inherit"),
        "{err}"
    );
    assert!(err.contains("seconds)? [y/N]"), "{err}");
    assert!(!f.bin.join("launchctl.log").exists());
    assert!(!f.bin.join("systemctl.log").exists());
}

#[test]
fn without_service_flags_nothing_is_installed() {
    let f = Fixture::new();
    let (out, v) = f.run(&WORK_ENV, "");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(v["setup"]["service"], Value::Null);
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn service_errors_name_step_10_and_keep_their_exit_code() {
    let install = [&WORK_ENV[..], &["--service", "install"]].concat();
    // An unmarked file at the unit path: exit 5, and the file stays.
    let f = Fixture::new();
    write_tool(&f.bin, "launchctl", LAUNCHCTL);
    write_tool(&f.bin, "systemctl", SYSTEMCTL);
    let unit = if cfg!(target_os = "macos") {
        f.home
            .join("Library/LaunchAgents/digital.wirdrei.mailtriage.work.plist")
    } else {
        f.home.join(".config/systemd/user/mailtriage-work.service")
    };
    fs::create_dir_all(unit.parent().unwrap()).unwrap();
    fs::write(&unit, "the user's own file\n").unwrap();
    let (out, v) = f.run(&install, "");
    assert_eq!(out.status.code(), Some(5), "{v}");
    let m = message(&v);
    assert!(m.starts_with("step 10 (service): "), "{m}");
    assert!(
        m.contains("then run `mailtriage service install --config "),
        "{m}"
    );
    assert!(
        m.ends_with(" --account work --interval-seconds 60 --limit 100`"),
        "{m}"
    );
    assert_eq!(fs::read_to_string(&unit).unwrap(), "the user's own file\n");
    assert!(f.config_path().exists());
    // A failing manager tool: exit 3.
    let f = Fixture::new();
    write_tool(&f.bin, "launchctl", "#!/bin/sh\nexit 1\n");
    write_tool(&f.bin, "systemctl", "#!/bin/sh\nexit 1\n");
    let (out, v) = f.run(&install, "");
    assert_eq!(out.status.code(), Some(3), "{v}");
    assert!(message(&v).starts_with("step 10 (service): "), "{v}");
}
