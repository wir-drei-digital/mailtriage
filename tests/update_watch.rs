#![cfg(unix)]
//! `watch`'s update step against a loopback server: auto, notify and off,
//! reservations across restarts, stored modes, and contained errors.
mod update_support;
use serde_json::{json, Value};
use std::{fs, path::Path, time::Duration};
use update_support::{
    download_path, fake_binary, release_archive, set_updates, Reply, Sandbox, Server, LIST,
};

const RUNNING: &str = env!("CARGO_PKG_VERSION");

fn original() -> Vec<u8> {
    fs::read(env!("CARGO_BIN_EXE_mailtriage")).unwrap()
}

/// Moves the cache's next check into the past, so the next pass checks.
fn check_now(sandbox: &Sandbox) {
    let mut cache = sandbox.cache();
    cache["next_check_at"] = json!("2000-01-01T00:00:00Z");
    fs::write(sandbox.cache_file(), cache.to_string()).unwrap();
}

#[test]
fn auto_installs_a_newer_release_and_restarts_onto_it() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    let marker = sandbox.root().join("marker");
    server.publish("9.9.9", &release_archive(&fake_binary("9.9.9", &marker)));
    let config = sandbox.config("cfg", "auto");
    let mut watch = sandbox.watch(&server, &config);
    let restarting = watch.event("restarting");
    assert_eq!(restarting["update"]["to"], "9.9.9");
    assert_eq!(restarting["update"]["pid"], watch.child.id());
    assert_eq!(watch.child.wait().unwrap().code(), Some(0));
    assert_eq!(
        fs::read_to_string(&marker).unwrap().trim(),
        "watch --account work --interval-seconds 1 --json"
    );
    assert_eq!(
        fs::read(format!("{}.previous", sandbox.bin.display())).unwrap(),
        original()
    );
    let cache = sandbox.cache();
    let entry = &cache["installs"][sandbox.bin.to_str().unwrap()];
    assert_eq!(entry["version"], "9.9.9");
    assert_eq!(
        (entry["failures"].clone(), entry["next_attempt_at"].clone()),
        (json!(0), Value::Null)
    );
    let config_key = fs::canonicalize(&config).unwrap();
    assert_eq!(
        cache["configs"][config_key.to_str().unwrap()]["mode"],
        "auto"
    );
}

#[test]
fn notify_reports_once_and_changes_nothing() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    server.publish("9.9.9", &release_archive(b"unused"));
    let config = sandbox.config("cfg", "notify");
    let mut watch = sandbox.watch(&server, &config);
    watch.wait_passes(4);
    let available = watch.events("available");
    assert_eq!(available.len(), 1, "{:#?}", watch.seen);
    assert_eq!(
        available[0],
        json!({"schema_version":1,"update":{"event":"available","current":RUNNING,"latest":"9.9.9","release_url":"https://github.com/wir-drei-digital/mailtriage/releases/tag/v9.9.9"}})
    );
    assert_eq!(server.count(LIST), 1);
    assert_eq!(server.count("/download"), 0);
    assert_eq!(fs::read(&sandbox.bin).unwrap(), original());
    let (code, _) = watch.stop();
    assert_eq!(code, Some(0));
}

#[test]
fn off_makes_no_request() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    server.publish("9.9.9", &release_archive(b"unused"));
    let config = sandbox.config("cfg", "off");
    let mut watch = sandbox.watch(&server, &config);
    watch.wait_passes(3);
    watch.stop();
    assert!(server.requests().is_empty());
}

#[test]
fn auto_reports_a_binary_it_may_not_replace() {
    let sandbox = Sandbox::at("opt/homebrew/Cellar/mailtriage/0.1.0/bin");
    let server = Server::start();
    server.publish("9.9.9", &release_archive(b"unused"));
    let config = sandbox.config("cfg", "auto");
    let mut watch = sandbox.watch(&server, &config);
    let available = watch.event("available");
    assert_eq!(
        available["update"]["install"],
        json!({"reason": "managed_by_homebrew", "fix": "run `brew upgrade mailtriage`"})
    );
    watch.wait_passes(2);
    watch.stop();
    assert_eq!(server.count("/download"), 0);
}

#[test]
fn notify_reports_a_binary_it_may_not_replace_without_an_install_block() {
    let sandbox = Sandbox::at("opt/homebrew/Cellar/mailtriage/0.1.0/bin");
    let server = Server::start();
    server.publish("9.9.9", &release_archive(b"unused"));
    let config = sandbox.config("cfg", "notify");
    let mut watch = sandbox.watch(&server, &config);
    let available = watch.event("available");
    assert_eq!(available["update"].get("install"), None, "{available}");
    watch.stop();
}

#[test]
fn a_failed_check_is_not_retried_after_a_restart() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    server.reply(LIST, Reply::status(500));
    let config = sandbox.config("cfg", "auto");
    let mut watch = sandbox.watch(&server, &config);
    let error = watch.event("error");
    assert_eq!(
        error["update"]["message"],
        "cannot read the release list: GitHub answered 500"
    );
    watch.kill();
    let mut watch = sandbox.watch(&server, &config);
    watch.wait_passes(3);
    watch.stop();
    assert_eq!(server.count(LIST), 1);
    assert_eq!(sandbox.cache()["check_failures"], 1);

    // Killed during the request: the reservation already holds the next one off.
    check_now(&sandbox);
    server.reply(LIST, Reply::status(500).delayed(Duration::from_secs(20)));
    let watch = sandbox.watch(&server, &config);
    server.wait_for(LIST, 2);
    watch.kill();
    let mut watch = sandbox.watch(&server, &config);
    watch.wait_passes(3);
    watch.stop();
    assert_eq!(server.count(LIST), 2);
}

#[test]
fn a_download_killed_midway_is_not_repeated_before_its_time() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    let archive = release_archive(&fake_binary("9.9.9", Path::new("/dev/null")));
    let name = server.publish("9.9.9", &archive);
    let path = download_path("9.9.9", &name);
    server.reply(&path, Reply::ok(archive).delayed(Duration::from_secs(20)));
    let config = sandbox.config("cfg", "auto");
    let watch = sandbox.watch(&server, &config);
    server.wait_for(&path, 1);
    watch.kill();
    let mut watch = sandbox.watch(&server, &config);
    watch.wait_passes(3);
    watch.stop();
    assert_eq!(server.count(&path), 1);
    let entry = &sandbox.cache()["installs"][sandbox.bin.to_str().unwrap()];
    assert!(entry["next_attempt_at"].is_string(), "{entry}");
    assert_eq!(fs::read(&sandbox.bin).unwrap(), original());
}

#[test]
fn a_notify_watcher_does_not_hold_back_an_auto_watcher() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    server.publish(
        "9.9.9",
        &release_archive(&fake_binary("9.9.9", Path::new("/dev/null"))),
    );
    let notify = sandbox.config("a", "notify");
    let auto = sandbox.config("b", "auto");
    let mut watch = sandbox.watch(&server, &notify);
    watch.event("available");
    watch.stop();
    let mut watch = sandbox.watch(&server, &auto);
    let restarting = watch.event("restarting");
    assert_eq!(restarting["update"]["to"], "9.9.9");
    let _ = watch.child.wait();
    // The auto watcher used the release the notify watcher had cached.
    assert_eq!(server.count(LIST), 1);
}

#[test]
fn an_unreadable_config_uses_only_its_own_stored_mode() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    server.publish("9.9.9", &release_archive(b"unused"));
    let config = sandbox.config("cfg", "notify");
    let mut watch = sandbox.watch(&server, &config);
    watch.event("available");
    watch.stop();
    assert_eq!(server.count(LIST), 1);
    // Its updates value turns invalid: the stored notify still checks.
    check_now(&sandbox);
    set_updates(&config, "sometimes");
    let mut watch = sandbox.watch(&server, &config);
    assert_eq!(watch.child.wait().unwrap().code(), Some(2));
    assert_eq!(server.count(LIST), 2);
    // A config with no stored mode makes no request.
    check_now(&sandbox);
    let other = sandbox.config("other", "auto");
    set_updates(&other, "sometimes");
    let mut watch = sandbox.watch(&server, &other);
    assert_eq!(watch.child.wait().unwrap().code(), Some(2));
    assert_eq!(server.count(LIST), 2);
}

#[test]
fn update_errors_never_change_a_pass() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    server.reply(LIST, Reply::status(500));
    let config = sandbox.config("cfg", "notify");
    let mut watch = sandbox.watch(&server, &config);
    watch.event("error");
    watch.wait_passes(3);
    let (code, last) = watch.stop();
    assert_eq!(code, Some(0));
    assert_eq!(last["watch"]["partial_passes"], 0);
    assert_eq!(last["watch"]["skipped_passes"], 0);
    assert_eq!(last["partial"], false);
}

#[test]
fn an_unwritable_cache_stops_network_work_with_one_event() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    server.publish("9.9.9", &release_archive(b"unused"));
    let cache_dir = sandbox.cache_file().parent().unwrap().to_path_buf();
    fs::create_dir_all(cache_dir.parent().unwrap()).unwrap();
    fs::write(&cache_dir, "a file where the cache directory belongs").unwrap();
    let config = sandbox.config("cfg", "auto");
    let mut watch = sandbox.watch(&server, &config);
    watch.wait_passes(3);
    let errors = watch.events("error");
    watch.stop();
    assert!(server.requests().is_empty());
    assert_eq!(errors.len(), 1, "{errors:#?}");
    let message = errors[0]["update"]["message"].as_str().unwrap();
    assert!(
        message.starts_with("cannot write the update cache: "),
        "{message}"
    );
}

/// Gives a directory its owner's write permission back when dropped, so
/// the sandbox around it can be removed.
struct Writable(std::path::PathBuf);

impl Drop for Writable {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
    }
}

#[test]
fn a_cache_that_can_be_read_but_not_written_is_reported_once() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let sandbox = Sandbox::new();
    // Permissions do not bind root.
    if fs::metadata(&sandbox.home).unwrap().uid() == 0 {
        return;
    }
    let server = Server::start();
    server.publish(
        "9.9.9",
        &release_archive(&fake_binary("9.9.9", Path::new("/dev/null"))),
    );
    // One successful check while the cache can still be written.
    let first = sandbox.config("first", "notify");
    let mut watch = sandbox.watch(&server, &first);
    watch.event("available");
    watch.stop();
    let cache_dir = sandbox.cache_file().parent().unwrap().to_path_buf();
    let _writable = Writable(cache_dir.clone());
    fs::set_permissions(&cache_dir, fs::Permissions::from_mode(0o500)).unwrap();
    // Configs the cache has never seen: storing their mode, `notify`'s
    // `notified_version` and `auto`'s install reservation all fail.
    for mode in ["notify", "auto"] {
        let config = sandbox.config(mode, mode);
        let mut watch = sandbox.watch(&server, &config);
        watch.wait_passes(4);
        let errors = watch.events("error");
        let available = watch.events("available");
        let (code, _) = watch.stop();
        assert_eq!(code, Some(0));
        assert_eq!(errors.len(), 1, "{mode}: {errors:#?}");
        let message = errors[0]["update"]["message"].as_str().unwrap();
        assert!(
            message.starts_with("cannot write the update cache: "),
            "{mode}: {message}"
        );
        let expected = usize::from(mode == "notify");
        assert_eq!(available.len(), expected, "{mode}: {available:#?}");
    }
    assert_eq!(server.count(LIST), 1);
    assert_eq!(server.count("/download"), 0);
    assert_eq!(fs::read(&sandbox.bin).unwrap(), original());
}

#[test]
fn a_check_whose_result_cannot_be_recorded_is_reported_once() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let sandbox = Sandbox::new();
    // Permissions do not bind root.
    if fs::metadata(&sandbox.home).unwrap().uid() == 0 {
        return;
    }
    let server = Server::start();
    server.publish("9.9.9", &release_archive(b"unused"));
    let cache_dir = sandbox.cache_file().parent().unwrap().to_path_buf();
    let _writable = Writable(cache_dir.clone());
    // The reservation is written before the request; during the request
    // the cache turns read-only, so the result cannot be recorded.
    server.on_request(LIST, move || {
        let _ = fs::set_permissions(&cache_dir, fs::Permissions::from_mode(0o500));
    });
    let config = sandbox.config("cfg", "notify");
    let mut watch = sandbox.watch(&server, &config);
    watch.wait_passes(3);
    let errors = watch.events("error");
    let available = watch.events("available");
    let (code, _) = watch.stop();
    assert_eq!(code, Some(0));
    assert_eq!(errors.len(), 1, "{errors:#?}");
    let message = errors[0]["update"]["message"].as_str().unwrap();
    assert!(
        message.starts_with("cannot write the update cache: ")
            && message.ends_with("; the check's result was not recorded"),
        "{message}"
    );
    assert!(available.is_empty(), "{available:#?}");
    // The reservation holds the next request off.
    assert_eq!(server.count(LIST), 1);
}
