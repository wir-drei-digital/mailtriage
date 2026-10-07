//! Every sync pass of a configured account records how it ended, including
//! a pass that finds another worker holding the account lock.
mod common;
use common::{write_tool, Harness, LAUNCHCTL, SYSTEMCTL};
use fs2::FileExt;
use mailtriage::{
    domain::FilingMode,
    store::{Store, LATEST},
};
use serde_json::Value;
use sha2::Digest;
use std::{fs, path::Path, process::Command};

#[test]
fn each_pass_records_a_heartbeat() {
    let h = Harness::new(FilingMode::DryRun);
    assert!(h.service().store.heartbeat("work").unwrap().is_none());
    h.sync();
    let beat = h.service().store.heartbeat("work").unwrap().unwrap();
    assert_eq!(beat["exit_code"], 0);
    assert_eq!(beat["partial"], false);
    assert_eq!(beat["mode"], "dry_run");
    assert_eq!(beat["version"], env!("CARGO_PKG_VERSION"));
    assert!(chrono::DateTime::parse_from_rfc3339(beat["finished_at"].as_str().unwrap()).is_ok());

    h.fake.fail_with_config_changed("version");
    assert!(h.service().sync("work", 100).is_err());
    let beat = h.service().store.heartbeat("work").unwrap().unwrap();
    assert_eq!(beat["exit_code"], 5);
    assert_eq!(beat["partial"], false);
    assert_eq!(beat["version"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn schema_5_adds_the_heartbeat_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let store = Store::open(&path).unwrap();
    store
        .record_heartbeat("work", true, 4, "off", None)
        .unwrap();
    store
        .record_heartbeat("work", false, 0, "live", None)
        .unwrap();
    let beat = store.heartbeat("work").unwrap().unwrap();
    assert_eq!(
        (
            beat["partial"].clone(),
            beat["exit_code"].clone(),
            beat["mode"].clone()
        ),
        (false.into(), 0.into(), "live".into())
    );
    drop(store);
    let version: u32 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, LATEST);
}

/// Holds `account`'s worker lock in `state_dir`, as another worker does.
fn hold_worker_lock(state_dir: &Path, account: &str) -> fs::File {
    let lock = state_dir.join(format!(
        "worker-{:x}.lock",
        sha2::Sha256::digest(account.as_bytes())
    ));
    let held = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock)
        .unwrap();
    held.try_lock_exclusive().unwrap();
    held
}

fn last_reason(h: &Harness) -> (i64, Value) {
    let beat = h.service().store.heartbeat("work").unwrap().unwrap();
    (beat["exit_code"].as_i64().unwrap(), beat["reason"].clone())
}

/// Error heartbeats name the error's reason. Health reads it to tell the
/// exit 5 of a configuration change, which `watch` survives, and of a busy
/// worker, which ends `watch` until the service manager restarts it, from
/// the exit 5 of a binding conflict, which needs the user.
#[test]
fn error_heartbeats_carry_their_reason() {
    let h = Harness::new(FilingMode::DryRun);
    h.sync();
    assert_eq!(last_reason(&h), (0, Value::Null));
    // Another worker holds the account's lock.
    let held = hold_worker_lock(&h.dir.path().join("mailtriage-state"), "work");
    assert!(h.service().sync("work", 100).is_err());
    assert_eq!(last_reason(&h), (5, "account_busy".into()));
    drop(held);
    // The engine's configuration changed during the pass.
    h.fake.fail_with_config_changed("version");
    assert!(h.service().sync("work", 100).is_err());
    assert_eq!(last_reason(&h), (5, "config_changed".into()));
    // A clean pass clears the reason.
    h.fake.reload_config();
    h.sync();
    assert_eq!(last_reason(&h), (0, Value::Null));

    // The mailbox behind the account changed.
    let h = Harness::new(FilingMode::DryRun);
    h.sync();
    h.fake.set_binding("another mailbox");
    assert!(h.service().sync("work", 100).is_err());
    assert_eq!(last_reason(&h), (5, "binding_conflict".into()));
}

/// Runs the CLI in `root` with its own HOME and XDG_CACHE_HOME, the fake
/// `launchctl` and `systemctl` first on PATH, release checks pointed at a
/// closed loopback port, and no inherited config or update test hook;
/// returns the exit code and stdout as JSON.
fn run(root: &Path, args: &[&str]) -> (Option<i32>, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
        .current_dir(root)
        .args(args)
        .env("HOME", root.join("home"))
        .env("XDG_CACHE_HOME", root.join("xdg"))
        .env("MT_FAKE_HOME", root.join("home"))
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.join("bin").display()),
        )
        .env("MAILTRIAGE_UPDATE_URL", "http://127.0.0.1:9")
        .env_remove("MAILTRIAGE_CONFIG")
        .env_remove("MAILTRIAGE_UPDATE_TEST_HOOK")
        .env_remove("MAILTRIAGE_UPDATE_TEST_LOCK_WAIT_MS")
        .output()
        .unwrap();
    (
        out.status.code(),
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null),
    )
}

/// `service status` reports the reason as `last_pass.reason`.
#[test]
fn status_shows_the_reason_of_the_last_pass() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    for d in ["home", "xdg", "bin"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    write_tool(&root.join("bin"), "launchctl", LAUNCHCTL);
    write_tool(&root.join("bin"), "systemctl", SYSTEMCTL);
    let status = || {
        let (code, v) = run(&root, &["service", "status", "--account", "work", "--json"]);
        assert_eq!(code, Some(0), "{v}");
        v["service"]["last_pass"].clone()
    };
    assert_eq!(run(&root, &["init", "--json"]).0, Some(0));
    let (code, v) = run(&root, &["sync", "--account", "work", "--json"]);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(status()["reason"], Value::Null);

    let held = hold_worker_lock(&root.join(".state"), "work");
    let (code, v) = run(&root, &["sync", "--account", "work", "--json"]);
    assert_eq!(
        (code, v["error"]["reason"].clone()),
        (Some(5), "account_busy".into())
    );
    let pass = status();
    assert_eq!(
        (pass["exit_code"].clone(), pass["reason"].clone()),
        (5.into(), "account_busy".into())
    );
    drop(held);
}
