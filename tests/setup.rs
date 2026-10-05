#![cfg(unix)]
//! `mailtriage setup` through the binary, with a fake Himalaya first on PATH
//! and HOME in a temp dir. Prompts are driven with `--interactive` and piped
//! stdin; every other run uses `--yes`.
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
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

fn write_tool(dir: &Path, name: &str, script: &str) {
    let path = dir.join(name);
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

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
    /// environment `env`, and `stdin` piped in.
    fn run_with(&self, args: &[&str], stdin: &str, env: &[(&str, &str)]) -> (Output, Value) {
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
