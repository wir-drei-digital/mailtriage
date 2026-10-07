#![cfg(unix)]
//! `mailtriage update` and `watch` with a `mailtriage-tray` next to the
//! CLI, against the loopback release server of `update_support`. The binary
//! under test is a copy in a sandbox directory (its installation directory)
//! with the sandbox's HOME and XDG_CACHE_HOME; the trays are scripts, so no
//! real tray starts.
mod update_support;
use mailtriage::update::release::platform;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};
use update_support::{
    archive, download_path, fake_binary, release_archive, run, sha256_hex, Reply, Sandbox, Server,
    LIST,
};

const RUNNING: &str = env!("CARGO_PKG_VERSION");

/// A fake tray: `--version` prints `mailtriage-tray VERSION`; nothing else
/// does anything.
fn tray_script(version: &str) -> String {
    format!("#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'mailtriage-tray {version}'; fi\n")
}

/// Why the tray that does not run here fails, as `--version` reports it.
const NO_LIBRARY: &str = "libayatana-appindicator3.so.1: cannot open shared object file";

/// Puts a tray next to the sandbox's CLI: one printing `version`, or, with
/// `runs` false, one failing the way a tray without its libraries does.
fn put_tray(sandbox: &Sandbox, version: &str, runs: bool) -> PathBuf {
    let path = sandbox.bin.with_file_name("mailtriage-tray");
    let script = if runs {
        tray_script(version)
    } else {
        format!("#!/bin/sh\necho '{NO_LIBRARY}' >&2\nexit 127\n")
    };
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// The tray part of a test release.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TrayAsset {
    Good,
    /// `SHA256SUMS` has a wrong line for it.
    BadSum,
    Missing,
}

/// Publishes `version` as the only release: a fake CLI that logs to
/// `marker`, the tray archive (`mailtriage-tray` and `LICENSE`, as the
/// release workflow builds it) unless `Missing`, and `SHA256SUMS`. The
/// release list is served only with `list`. Returns each asset as the cache
/// records it, by name.
fn publish(
    server: &Server,
    version: &str,
    tray: TrayAsset,
    marker: &Path,
    list: bool,
) -> HashMap<String, Value> {
    let platform = platform().unwrap();
    let mut files = vec![(
        format!("mailtriage-v{version}-{platform}.tar.gz"),
        release_archive(&fake_binary(version, marker)),
    )];
    if tray != TrayAsset::Missing {
        let script = tray_script(version);
        files.push((
            format!("mailtriage-tray-v{version}-{platform}.tar.gz"),
            archive(&[
                ("mailtriage-tray", script.as_bytes()),
                ("LICENSE", b"MIT License\n"),
            ]),
        ));
    }
    let mut sums = String::new();
    for (name, data) in &files {
        let hash = if tray == TrayAsset::BadSum && name.starts_with("mailtriage-tray-") {
            sha256_hex(b"another archive")
        } else {
            sha256_hex(data)
        };
        sums += &format!("{hash}  {name}\n");
    }
    files.push(("SHA256SUMS".to_owned(), sums.into_bytes()));
    for (name, data) in &files {
        server.reply(&download_path(version, name), Reply::ok(data.clone()));
    }
    if list {
        let listing: Vec<(&str, usize)> =
            files.iter().map(|(n, d)| (n.as_str(), d.len())).collect();
        server.list(&[server.release_json(version, &listing)]);
    }
    files
        .iter()
        .map(|(name, data)| {
            let asset = json!({"name": name, "url": server.url(&download_path(version, name)), "size": data.len()});
            (name.clone(), asset)
        })
        .collect()
}

fn cli_archive(version: &str) -> String {
    format!("mailtriage-v{version}-{}.tar.gz", platform().unwrap())
}

fn tray_archive(version: &str) -> String {
    format!("mailtriage-tray-v{version}-{}.tar.gz", platform().unwrap())
}

/// What `path --version` prints, trimmed.
fn version_of(path: &Path) -> String {
    let out = Command::new(path).arg("--version").output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn previous(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.previous", path.display()))
}

fn key(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// `mailtriage ARGS` from the sandbox's copy: its exit code and JSON.
fn mailtriage(sandbox: &Sandbox, server: &Server, args: &[&str]) -> (Option<i32>, Value) {
    let (code, v, stderr) = run(sandbox.command(server).args(args));
    (code, json!({"out": v, "stderr": stderr}))
}

/// Files of update attempts left in the installation directory.
fn leftovers(sandbox: &Sandbox) -> Vec<String> {
    fs::read_dir(sandbox.bin.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with(".mailtriage-update-"))
        .collect()
}

/// Writes `update.json` into the sandbox's cache directory.
fn write_cache(sandbox: &Sandbox, cache: &Value) {
    fs::create_dir_all(sandbox.cache_file().parent().unwrap()).unwrap();
    fs::write(sandbox.cache_file(), cache.to_string()).unwrap();
}

/// A cache as `watch` finds it after a check made now: `release` is
/// `version` with `archives` (as given: an older check has no tray key),
/// the next check far away, and `installs` as given.
fn cache_with(
    assets: &HashMap<String, Value>,
    version: &str,
    archives: Value,
    next_check_at: &str,
    installs: Value,
) -> Value {
    let now = chrono::Utc::now().to_rfc3339();
    json!({
        "schema_version": 1,
        "release": {
            "version": version,
            "release_url": format!("https://github.com/wir-drei-digital/mailtriage/releases/tag/v{version}"),
            "published_at": now,
            "archives": archives,
            "sums": assets["SHA256SUMS"],
        },
        "checked_at": now,
        "next_check_at": next_check_at,
        "check_failures": 0,
        "last_check_error": null,
        "configs": {},
        "installs": installs,
    })
}

/// Waits up to 30 s for the tray at `path` to print `mailtriage-tray VERSION`.
fn wait_for_tray(path: &Path, version: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let expected = format!("mailtriage-tray {version}");
    while version_of(path) != expected {
        assert!(Instant::now() < deadline, "the tray was not installed");
        thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn a_tray_next_to_the_cli_is_updated() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    publish(
        &server,
        "9.9.9",
        TrayAsset::Good,
        Path::new("/dev/null"),
        true,
    );
    let tray = put_tray(&sandbox, "0.1.0", true);
    let (code, v) = mailtriage(&sandbox, &server, &["update", "--json"]);
    assert_eq!(code, Some(0), "{v}");
    let out = &v["out"];
    assert_eq!(out["update"]["action"], "updated");
    assert_eq!(
        out["update"]["tray"],
        json!({"action": "updated", "from": "0.1.0", "to": "9.9.9", "error": null})
    );
    assert_eq!(out.get("partial"), None);
    assert_eq!(out["update"]["warnings"], json!([]));
    assert_eq!(version_of(&sandbox.bin), "mailtriage 9.9.9");
    assert_eq!(version_of(&tray), "mailtriage-tray 9.9.9");
    assert_eq!(version_of(&previous(&tray)), "mailtriage-tray 0.1.0");
    assert!(leftovers(&sandbox).is_empty());
    // Full success: one entry each.
    let installs = &sandbox.cache()["installs"];
    assert_eq!(installs[key(&sandbox.bin)]["version"], "9.9.9");
    let entry = &installs[key(&tray)];
    assert_eq!(
        (entry["version"].clone(), entry["failures"].clone()),
        (json!("9.9.9"), json!(0))
    );
    assert!(entry["last_error"].is_null());
}

/// Skipping is not a failure: exit 0, no download, and the tray's entry
/// says why without a backoff (Decision 28: `failures` unchanged).
#[test]
fn a_tray_that_does_not_run_here_is_skipped_without_a_download() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    publish(
        &server,
        "9.9.9",
        TrayAsset::Good,
        Path::new("/dev/null"),
        true,
    );
    let tray = put_tray(&sandbox, "", false);
    write_cache(
        &sandbox,
        &json!({"schema_version": 1, "installs": {key(&tray): {
            "version": "0.0.1", "failures": 2, "next_attempt_at": "2999-01-01T00:00:00Z", "last_error": null
        }}}),
    );
    let (code, v) = mailtriage(&sandbox, &server, &["update", "--json"]);
    assert_eq!(code, Some(0), "{v}");
    let error = format!("mailtriage-tray does not run here: exited with 127; stderr: {NO_LIBRARY}");
    assert_eq!(
        v["out"]["update"]["tray"],
        json!({"action": "skipped", "from": null, "to": null, "error": error})
    );
    assert_eq!(v["out"].get("partial"), None);
    assert_eq!(
        server.count(&download_path("9.9.9", &tray_archive("9.9.9"))),
        0
    );
    // Skip: one entry each.
    let installs = &sandbox.cache()["installs"];
    assert_eq!(installs[key(&sandbox.bin)]["version"], "9.9.9");
    let entry = &installs[key(&tray)];
    assert_eq!(entry["version"], Value::Null);
    assert_eq!(entry["last_error"]["message"], error.as_str());
    assert_eq!(
        (entry["failures"].clone(), entry["next_attempt_at"].clone()),
        (json!(2), json!("2999-01-01T00:00:00Z"))
    );
}

#[test]
fn a_bad_tray_checksum_fails_only_the_tray() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    publish(
        &server,
        "9.9.9",
        TrayAsset::BadSum,
        Path::new("/dev/null"),
        true,
    );
    let tray = put_tray(&sandbox, "0.1.0", true);
    let before = chrono::Utc::now();
    let (code, v) = mailtriage(&sandbox, &server, &["update", "--json"]);
    assert_eq!(code, Some(4), "{v}");
    let out = &v["out"];
    assert_eq!(out["partial"], true);
    assert_eq!(out["update"]["action"], "updated");
    let error = format!("checksum mismatch for {}", tray_archive("9.9.9"));
    assert_eq!(
        out["update"]["tray"],
        json!({"action": "failed", "from": "0.1.0", "to": "9.9.9", "error": error})
    );
    assert_eq!(version_of(&sandbox.bin), "mailtriage 9.9.9");
    assert_eq!(version_of(&tray), "mailtriage-tray 0.1.0");
    assert!(!previous(&tray).exists());
    assert!(leftovers(&sandbox).is_empty());
    // Tray failure: one entry each, the tray's with the update spec's backoff.
    let installs = &sandbox.cache()["installs"];
    assert_eq!(installs[key(&sandbox.bin)]["version"], "9.9.9");
    let entry = &installs[key(&tray)];
    assert_eq!(entry["last_error"]["message"], error.as_str());
    assert_eq!(entry["failures"], 1);
    let next = chrono::DateTime::parse_from_rfc3339(entry["next_attempt_at"].as_str().unwrap())
        .unwrap()
        .with_timezone(&chrono::Utc);
    assert!(next >= before + chrono::Duration::minutes(59), "{next}");
    assert!(
        next <= chrono::Utc::now() + chrono::Duration::hours(1),
        "{next}"
    );
}

#[test]
fn a_release_without_a_tray_archive_fails_the_tray_part_only() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    publish(
        &server,
        "9.9.9",
        TrayAsset::Missing,
        Path::new("/dev/null"),
        true,
    );
    let tray = put_tray(&sandbox, "0.1.0", true);
    let (code, v) = mailtriage(&sandbox, &server, &["update", "--json"]);
    assert_eq!(code, Some(4), "{v}");
    assert_eq!(v["out"]["update"]["action"], "updated");
    assert_eq!(
        v["out"]["update"]["tray"],
        json!({"action": "failed", "from": "0.1.0", "to": "9.9.9",
               "error": format!("release v9.9.9 has no tray archive for {}", platform().unwrap())})
    );
    assert_eq!(version_of(&tray), "mailtriage-tray 0.1.0");
    assert_eq!(sandbox.cache()["installs"][key(&tray)]["failures"], 1);
}

#[test]
fn a_current_tray_keeps_its_own_entry_and_no_tray_means_no_tray_key() {
    // CLI-only success: the CLI is updated and the current tray gets its own entry.
    let sandbox = Sandbox::new();
    let server = Server::start();
    publish(
        &server,
        "9.9.9",
        TrayAsset::Good,
        Path::new("/dev/null"),
        true,
    );
    let tray = put_tray(&sandbox, "9.9.9", true);
    let (code, v) = mailtriage(&sandbox, &server, &["update", "--json"]);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["out"]["update"]["action"], "updated");
    assert_eq!(
        v["out"]["update"]["tray"],
        json!({"action": "current", "from": "9.9.9", "to": "9.9.9", "error": null})
    );
    let installs = &sandbox.cache()["installs"];
    assert_eq!(installs[key(&sandbox.bin)]["version"], "9.9.9");
    assert_eq!(installs[key(&tray)]["version"], "9.9.9");
    assert!(installs[key(&tray)]["identity"].is_object(), "{installs}");
    assert_eq!(
        server.count(&download_path("9.9.9", &tray_archive("9.9.9"))),
        0
    );

    // R11-2: a current tray never fails for a release without its archive.
    let sandbox = Sandbox::new();
    let server = Server::start();
    publish(
        &server,
        "9.9.9",
        TrayAsset::Missing,
        Path::new("/dev/null"),
        true,
    );
    put_tray(&sandbox, "9.9.9", true);
    let (code, v) = mailtriage(&sandbox, &server, &["update", "--json"]);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["out"]["update"]["tray"]["action"], "current");

    // Without a tray file there is no tray key, and no tray entry.
    let sandbox = Sandbox::new();
    let server = Server::start();
    publish(
        &server,
        "9.9.9",
        TrayAsset::Good,
        Path::new("/dev/null"),
        true,
    );
    let (code, v) = mailtriage(&sandbox, &server, &["update", "--check", "--json"]);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["out"]["update"].get("tray"), None, "{v}");
    let (code, v) = mailtriage(&sandbox, &server, &["update", "--json"]);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["out"]["update"].get("tray"), None, "{v}");
    let installs = sandbox.cache()["installs"].clone();
    assert_eq!(
        installs.as_object().unwrap().keys().collect::<Vec<_>>(),
        [key(&sandbox.bin)]
    );
}

#[test]
fn check_reports_the_tray_without_changing_it() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    publish(
        &server,
        "9.9.9",
        TrayAsset::Good,
        Path::new("/dev/null"),
        true,
    );
    let tray = put_tray(&sandbox, "0.1.0", true);
    let (code, v) = mailtriage(&sandbox, &server, &["update", "--check", "--json"]);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(
        v["out"]["update"]["tray"],
        json!({"path": key(&tray), "installed": "0.1.0", "available": true})
    );
    // The CLI's `available` keeps meaning the CLI alone.
    assert_eq!(v["out"]["update"]["available"], true);
    put_tray(&sandbox, "9.9.9", true);
    let (_, v) = mailtriage(&sandbox, &server, &["update", "--check", "--json"]);
    assert_eq!(v["out"]["update"]["tray"]["available"], false, "{v}");
    put_tray(&sandbox, "", false);
    let (_, v) = mailtriage(&sandbox, &server, &["update", "--check", "--json"]);
    assert_eq!(
        v["out"]["update"]["tray"],
        json!({"path": key(&tray), "installed": null, "available": false})
    );
    assert_eq!(server.count("/download/"), 0);
    assert_eq!(version_of(&sandbox.bin), format!("mailtriage {RUNNING}"));
}

/// R11-4: the tray part runs under the installation lock even when the
/// CLI is current.
#[test]
fn a_current_cli_still_updates_an_older_tray() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    publish(
        &server,
        RUNNING,
        TrayAsset::Good,
        Path::new("/dev/null"),
        true,
    );
    let tray = put_tray(&sandbox, "0.0.1", true);
    let (code, v) = mailtriage(&sandbox, &server, &["update", "--json"]);
    assert_eq!(code, Some(0), "{v}");
    let update = &v["out"]["update"];
    assert_eq!(
        (update["action"].clone(), update["from"].clone()),
        (json!("current"), json!(RUNNING))
    );
    assert_eq!(
        update["tray"],
        json!({"action": "updated", "from": "0.0.1", "to": RUNNING, "error": null})
    );
    assert_eq!(version_of(&tray), format!("mailtriage-tray {RUNNING}"));
    assert_eq!(
        server.count(&download_path(RUNNING, &cli_archive(RUNNING))),
        0
    );
    assert_eq!(
        fs::read(&sandbox.bin).unwrap(),
        fs::read(env!("CARGO_BIN_EXE_mailtriage")).unwrap()
    );
    assert!(sandbox
        .bin
        .with_file_name(".mailtriage-update.lock")
        .exists());
}

/// R11-4: a held installation lock is exit 5 when only the tray needs it.
#[test]
#[cfg(debug_assertions)]
fn a_held_lock_is_exit_5_when_only_the_tray_needs_it() {
    use fs2::FileExt;
    let sandbox = Sandbox::new();
    let server = Server::start();
    publish(
        &server,
        RUNNING,
        TrayAsset::Good,
        Path::new("/dev/null"),
        true,
    );
    let tray = put_tray(&sandbox, "0.0.1", true);
    let lock = fs::File::create(sandbox.bin.with_file_name(".mailtriage-update.lock")).unwrap();
    lock.lock_exclusive().unwrap();
    let (code, v, _) = run(sandbox
        .command(&server)
        .env("MAILTRIAGE_UPDATE_TEST_LOCK_WAIT_MS", "300")
        .args(["update", "--json"]));
    assert_eq!(code, Some(5), "{v}");
    assert_eq!(v["error"]["message"], "another update is running");
    assert_eq!(version_of(&tray), "mailtriage-tray 0.0.1");
}

/// R11-4: when the CLI part fails, the run fails as before and the tray is
/// not tried.
#[test]
fn a_failed_cli_update_leaves_the_tray_untried() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    publish(
        &server,
        "9.9.9",
        TrayAsset::Good,
        Path::new("/dev/null"),
        true,
    );
    server.reply(
        &download_path("9.9.9", &cli_archive("9.9.9")),
        Reply::redirect("https://evil.example/a.tar.gz"),
    );
    let tray = put_tray(&sandbox, "0.1.0", true);
    let (code, v) = mailtriage(&sandbox, &server, &["update", "--json"]);
    assert_eq!(code, Some(3), "{v}");
    assert!(
        v["out"]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("a redirect left GitHub"),
        "{v}"
    );
    assert_eq!(version_of(&tray), "mailtriage-tray 0.1.0");
    assert_eq!(
        server.count(&download_path("9.9.9", &tray_archive("9.9.9"))),
        0
    );
    assert!(sandbox.cache()["installs"].get(key(&tray)).is_none());
}

/// R11-4b: a skip whose entry cannot be written is a warning, and the run
/// still succeeds.
#[test]
fn a_skip_that_cannot_be_recorded_is_a_warning() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    publish(
        &server,
        RUNNING,
        TrayAsset::Good,
        Path::new("/dev/null"),
        true,
    );
    let tray = put_tray(&sandbox, "", false);
    // The cache directory is a file: nothing can be written into it.
    let dir = sandbox.cache_file().parent().unwrap().to_path_buf();
    fs::create_dir_all(dir.parent().unwrap()).unwrap();
    fs::write(&dir, "").unwrap();
    let (code, v) = mailtriage(&sandbox, &server, &["update", "--json"]);
    assert_eq!(code, Some(0), "{v}");
    let update = &v["out"]["update"];
    assert_eq!(update["action"], "current");
    assert_eq!(update["tray"]["action"], "skipped");
    let warnings: Vec<&str> = update["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap())
        .collect();
    assert!(
        warnings
            .iter()
            .any(|w| w.starts_with("cannot write the update cache") && w.contains(key(&tray))),
        "{warnings:?}"
    );
}

/// Makes the tray file group-writable: `platform::blocker` then refuses the
/// tray (`unsafe_permissions`) while the CLI next to it, 0755 in a 0755
/// directory, stays replaceable. Returns the refusal both parts report.
fn make_unsafe(tray: &Path) -> String {
    fs::set_permissions(tray, fs::Permissions::from_mode(0o775)).unwrap();
    format!(
        "{} cannot be replaced (unsafe_permissions): install mailtriage into a directory that only this user owns and can write, such as ~/.local/bin, or set \"updates\" to \"notify\" (now: {})",
        tray.display(),
        tray.display()
    )
}

/// Spec step 3 for the tray: a tray the replaceability check refuses fails
/// the tray part like any other tray failure (exit 4, the update spec's
/// backoff), with the CLI current and replaceable, and nothing downloaded.
#[test]
fn a_tray_that_may_not_be_replaced_fails_the_tray_part_with_a_backoff() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    publish(
        &server,
        RUNNING,
        TrayAsset::Good,
        Path::new("/dev/null"),
        true,
    );
    let tray = put_tray(&sandbox, "0.0.1", true);
    let refusal = make_unsafe(&tray);
    let (code, v) = mailtriage(&sandbox, &server, &["update", "--check", "--json"]);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["out"]["update"]["install"]["replaceable"], true, "{v}");
    let before = chrono::Utc::now();
    let (code, v) = mailtriage(&sandbox, &server, &["update", "--json"]);
    assert_eq!(code, Some(4), "{v}");
    let out = &v["out"];
    assert_eq!(out["partial"], true);
    assert_eq!(out["update"]["action"], "current");
    assert_eq!(
        out["update"]["tray"],
        json!({"action": "failed", "from": "0.0.1", "to": RUNNING, "error": refusal})
    );
    assert_eq!(server.count("/download/"), 0, "{:?}", server.requests());
    assert_eq!(version_of(&tray), "mailtriage-tray 0.0.1");
    assert!(!previous(&tray).exists());
    let entry = &sandbox.cache()["installs"][key(&tray)];
    assert_eq!(entry["last_error"]["message"], refusal.as_str());
    assert_eq!(entry["failures"], 1);
    let next = chrono::DateTime::parse_from_rfc3339(entry["next_attempt_at"].as_str().unwrap())
        .unwrap()
        .with_timezone(&chrono::Utc);
    assert!(next >= before + chrono::Duration::minutes(59), "{next}");
}

/// The same refusal in `watch`: an `error` event, the tray's own backoff,
/// no download; the passes go on.
#[test]
fn a_tray_that_may_not_be_replaced_is_an_error_event_in_watch() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    let assets = publish(
        &server,
        RUNNING,
        TrayAsset::Good,
        Path::new("/dev/null"),
        false,
    );
    let tray = put_tray(&sandbox, "0.0.1", true);
    let refusal = make_unsafe(&tray);
    let config = sandbox.config("cfg", "auto");
    write_cache(
        &sandbox,
        &cache_with(
            &assets,
            RUNNING,
            json!({"mailtriage": assets[&cli_archive(RUNNING)], "mailtriage-tray": assets[&tray_archive(RUNNING)]}),
            "2999-01-01T00:00:00Z",
            json!({}),
        ),
    );
    let mut watch = sandbox.watch(&server, &config);
    let error = watch.event("error");
    assert_eq!(
        error["update"]["message"],
        format!("installing the mailtriage-tray update failed: {refusal}")
    );
    watch.wait_passes(3);
    assert_eq!(watch.events("error").len(), 1, "{:#?}", watch.seen);
    let (code, _) = watch.stop();
    assert_eq!(code, Some(0));
    assert_eq!(server.count("/download/"), 0, "{:?}", server.requests());
    assert_eq!(version_of(&tray), "mailtriage-tray 0.0.1");
    let entry = &sandbox.cache()["installs"][key(&tray)];
    assert_eq!(entry["last_error"]["message"], refusal.as_str());
    assert_eq!(entry["failures"], 1);
    assert!(entry["next_attempt_at"].is_string(), "{entry}");
}

/// The update spec's background install for the tray: a later `watch` pass
/// installs the still-older tray once its backoff has passed, from the
/// persisted cache alone (the server serves no release list). The CLI is
/// current, so `watch` does not restart.
#[test]
fn watch_installs_an_older_tray_from_the_cache_after_its_backoff() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    let assets = publish(
        &server,
        RUNNING,
        TrayAsset::Good,
        Path::new("/dev/null"),
        false,
    );
    let tray = put_tray(&sandbox, "0.0.1", true);
    let config = sandbox.config("cfg", "auto");
    let now = chrono::Utc::now().to_rfc3339();
    write_cache(
        &sandbox,
        &cache_with(
            &assets,
            RUNNING,
            json!({"mailtriage": assets[&cli_archive(RUNNING)], "mailtriage-tray": assets[&tray_archive(RUNNING)]}),
            "2999-01-01T00:00:00Z",
            json!({
                key(&sandbox.bin): {"version": RUNNING, "at": now, "last_error": null, "failures": 0, "next_attempt_at": null},
                key(&tray): {
                    "version": "0.0.1", "at": now,
                    "last_error": {"at": now, "message": "an earlier failure"},
                    "failures": 1, "next_attempt_at": "2000-01-01T00:00:00Z"
                }
            }),
        ),
    );
    let mut watch = sandbox.watch(&server, &config);
    wait_for_tray(&tray, RUNNING);
    watch.wait_passes(2);
    assert!(watch.events("restarting").is_empty(), "{:#?}", watch.seen);
    assert!(watch.events("error").is_empty(), "{:#?}", watch.seen);
    let (code, _) = watch.stop();
    assert_eq!(code, Some(0));
    assert_eq!(server.count(LIST), 0, "{:?}", server.requests());
    let entry = &sandbox.cache()["installs"][key(&tray)];
    assert_eq!(entry["version"], RUNNING);
    assert_eq!(
        (entry["last_error"].clone(), entry["failures"].clone()),
        (Value::Null, json!(0))
    );
    assert_eq!(version_of(&previous(&tray)), "mailtriage-tray 0.0.1");
}

/// In `watch` a tray failure is an `error` event with its own backoff; the
/// passes go on.
#[test]
fn a_tray_failure_in_watch_is_an_error_event_with_a_backoff() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    let assets = publish(
        &server,
        RUNNING,
        TrayAsset::BadSum,
        Path::new("/dev/null"),
        false,
    );
    let tray = put_tray(&sandbox, "0.0.1", true);
    let config = sandbox.config("cfg", "auto");
    write_cache(
        &sandbox,
        &cache_with(
            &assets,
            RUNNING,
            json!({"mailtriage": assets[&cli_archive(RUNNING)], "mailtriage-tray": assets[&tray_archive(RUNNING)]}),
            "2999-01-01T00:00:00Z",
            json!({}),
        ),
    );
    let mut watch = sandbox.watch(&server, &config);
    let error = watch.event("error");
    assert_eq!(
        error["update"]["message"],
        format!(
            "installing the mailtriage-tray update failed: checksum mismatch for {}",
            tray_archive(RUNNING)
        )
    );
    watch.wait_passes(3);
    assert_eq!(watch.events("error").len(), 1, "{:#?}", watch.seen);
    let (code, _) = watch.stop();
    assert_eq!(code, Some(0));
    assert_eq!(version_of(&tray), "mailtriage-tray 0.0.1");
    let path = download_path(RUNNING, &tray_archive(RUNNING));
    assert_eq!(server.count(&path), 1, "the backoff holds the next try off");
    let entry = &sandbox.cache()["installs"][key(&tray)];
    assert_eq!(entry["failures"], 1);
    assert!(entry["next_attempt_at"].is_string(), "{entry}");
}

/// Ruling (a) and R11-1: a release an older version recorded has no tray
/// key, which is unknown rather than "no archive": `watch` refreshes before
/// the daily deadline, then installs the tray.
#[test]
fn watch_refreshes_a_release_recorded_without_the_tray_then_installs_it() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    let assets = publish(
        &server,
        RUNNING,
        TrayAsset::Good,
        Path::new("/dev/null"),
        true,
    );
    let tray = put_tray(&sandbox, "0.0.1", true);
    let config = sandbox.config("cfg", "auto");
    let tomorrow = (chrono::Utc::now() + chrono::Duration::hours(24)).to_rfc3339();
    write_cache(
        &sandbox,
        &cache_with(
            &assets,
            RUNNING,
            json!({"mailtriage": assets[&cli_archive(RUNNING)]}),
            &tomorrow,
            json!({}),
        ),
    );
    let mut watch = sandbox.watch(&server, &config);
    wait_for_tray(&tray, RUNNING);
    watch.wait_passes(2);
    watch.stop();
    assert_eq!(server.count(LIST), 1);
    assert_eq!(
        sandbox.cache()["release"]["archives"]["mailtriage-tray"]["name"],
        tray_archive(RUNNING)
    );
}

/// R11-1: a watcher that cannot reach GitHub asks once for a release without
/// the tray key, then waits for the failed check's backoff.
#[test]
fn an_offline_watcher_does_not_refresh_a_release_without_the_tray_every_pass() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    let assets = publish(
        &server,
        RUNNING,
        TrayAsset::Good,
        Path::new("/dev/null"),
        false,
    );
    server.reply(LIST, Reply::status(503));
    let tray = put_tray(&sandbox, "0.0.1", true);
    let config = sandbox.config("cfg", "auto");
    let tomorrow = (chrono::Utc::now() + chrono::Duration::hours(24)).to_rfc3339();
    write_cache(
        &sandbox,
        &cache_with(
            &assets,
            RUNNING,
            json!({"mailtriage": assets[&cli_archive(RUNNING)]}),
            &tomorrow,
            json!({}),
        ),
    );
    let mut watch = sandbox.watch(&server, &config);
    watch.wait_passes(4);
    watch.stop();
    assert_eq!(server.count(LIST), 1);
    assert_eq!(server.count("/download/"), 0);
    assert_eq!(version_of(&tray), "mailtriage-tray 0.0.1");
    assert_eq!(sandbox.cache()["check_failures"], 1);
}
