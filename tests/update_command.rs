#![cfg(unix)]
//! `mailtriage update` through a copy of the binary in a temporary
//! directory, with a loopback server as GitHub.
mod update_support;
use mailtriage::system_service::{self, Manager, Unit};
use serde_json::{json, Value};
use std::{fs, os::unix::fs::PermissionsExt, path::Path, time::Duration};
use update_support::{
    download_path, fake_binary, platform, release_archive, run, sha256_hex, Reply, Sandbox, Server,
};

const RUNNING: &str = env!("CARGO_PKG_VERSION");

fn original() -> Vec<u8> {
    fs::read(env!("CARGO_BIN_EXE_mailtriage")).unwrap()
}

/// Files of update attempts left next to the binary.
fn leftovers(bin: &Path) -> Vec<String> {
    fs::read_dir(bin.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with(".mailtriage-update-"))
        .collect()
}

/// A marked service file for `account` running `exe`, in the sandbox HOME.
fn service_file(home: &Path, account: &str, exe: &Path) {
    let manager = if cfg!(target_os = "macos") {
        Manager::Launchd
    } else {
        Manager::Systemd
    };
    let unit = Unit {
        account: account.into(),
        exe: exe.to_path_buf(),
        config: home.join("mailtriage.json"),
        interval_seconds: 60,
        limit: 100,
        log_dir: home.join("logs"),
        path_env: None,
    };
    let dir = system_service::unit_dir(manager, home);
    fs::create_dir_all(&dir).unwrap();
    let (name, text) = match manager {
        Manager::Launchd => (
            format!("digital.wirdrei.mailtriage.{account}.plist"),
            system_service::plist(&unit),
        ),
        Manager::Systemd => (
            format!("mailtriage-{account}.service"),
            system_service::systemd_unit(&unit),
        ),
    };
    fs::write(dir.join(name), text).unwrap();
}

/// A hook script: runs `body` when called with `point`.
#[cfg(debug_assertions)]
fn hook(dir: &Path, point: &str, body: &str) -> std::path::PathBuf {
    let path = dir.join("hook.sh");
    fs::write(
        &path,
        format!("#!/bin/sh\n[ \"$1\" = {point} ] || exit 0\n{body}\n"),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn update_installs_a_newer_release_and_lists_the_services() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    let marker = sandbox.root().join("marker");
    server.publish("9.9.9", &release_archive(&fake_binary("9.9.9", &marker)));
    service_file(&sandbox.home, "work", &sandbox.bin);
    service_file(
        &sandbox.home,
        "other",
        Path::new("/opt/elsewhere/mailtriage"),
    );
    let (code, v, stderr) = run(sandbox.command(&server).args(["update", "--json"]));
    assert_eq!(code, Some(0), "{v} {stderr}");
    let u = &v["update"];
    assert_eq!(u["action"], "updated");
    assert_eq!(
        (u["from"].as_str(), u["to"].as_str()),
        (Some(RUNNING), Some("9.9.9"))
    );
    assert_eq!(u["path"], sandbox.bin.to_str().unwrap());
    let previous = format!("{}.previous", sandbox.bin.display());
    assert_eq!(u["previous_path"], previous);
    assert_eq!(u["warnings"], json!([]));
    let services = u["services"].as_array().unwrap();
    assert_eq!(services.len(), 2, "{u}");
    assert_eq!(
        (
            services[0]["account"].as_str(),
            services[0]["same_binary"].as_bool()
        ),
        (Some("other"), Some(false))
    );
    assert_eq!(
        (
            services[1]["account"].as_str(),
            services[1]["same_binary"].as_bool()
        ),
        (Some("work"), Some(true))
    );
    assert_eq!(services[1]["executable"], sandbox.bin.to_str().unwrap());
    assert_eq!(
        fs::read(&sandbox.bin).unwrap(),
        fake_binary("9.9.9", &marker)
    );
    assert_eq!(fs::read(&previous).unwrap(), original());
    assert!(leftovers(&sandbox.bin).is_empty());
    let cache = sandbox.cache();
    assert_eq!(
        cache["installs"][sandbox.bin.to_str().unwrap()]["version"],
        "9.9.9"
    );
    assert_eq!(cache["release"]["version"], "9.9.9");
    assert!(!marker.exists(), "the new binary only ran --version");
}

#[test]
fn update_reports_current_without_downloading() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    server.publish(RUNNING, &release_archive(b"never used"));
    let (code, v, _) = run(sandbox.command(&server).args(["update", "--json"]));
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(
        v["update"],
        json!({"action":"current","from":RUNNING,"to":RUNNING,"path":sandbox.bin,"warnings":[]})
    );
    assert_eq!(server.count("/download"), 0);
}

/// The installed file is newer than the running process: the hook swaps
/// it right after the check, as another updater would.
#[test]
#[cfg(debug_assertions)]
fn the_installed_version_decides_not_the_running_one() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    let marker = sandbox.root().join("marker");
    server.publish("9.9.9", &release_archive(&fake_binary("9.9.9", &marker)));
    let newer = sandbox.root().join("newer");
    fs::write(&newer, fake_binary("9.9.9", &marker)).unwrap();
    fs::set_permissions(&newer, fs::Permissions::from_mode(0o755)).unwrap();
    let script = hook(
        &sandbox.root(),
        "checked",
        &format!("mv '{}' '{}'", newer.display(), sandbox.bin.display()),
    );
    let (code, v, stderr) = run(sandbox
        .command(&server)
        .env("MAILTRIAGE_UPDATE_TEST_HOOK", &script)
        .args(["update", "--json"]));
    assert_eq!(code, Some(0), "{v} {stderr}");
    assert_eq!(v["update"]["action"], "current");
    assert_eq!(v["update"]["from"], "9.9.9");
    assert_eq!(server.count("/download"), 0);
}

/// Serves a release `update` must refuse; the fake binary logs to `marker`.
type Prepare = fn(&Server, &Path);

#[test]
fn refused_updates_exit_3_and_leave_the_binary() {
    let cases: [(&str, Prepare); 3] = [
        (
            "checksum mismatch for mailtriage-v9.9.9-",
            |server, marker| {
                let archive = release_archive(&fake_binary("9.9.9", marker));
                let name = format!("mailtriage-v9.9.9-{}.tar.gz", platform());
                let wrong = format!("{}  {name}\n", sha256_hex(b"x"));
                server.publish_with_sums("9.9.9", &archive, &wrong);
            },
        ),
        (
            "the new binary does not run here: printed version 9.9.8, expected 9.9.9",
            |server, marker| {
                server.publish("9.9.9", &release_archive(&fake_binary("9.9.8", marker)));
            },
        ),
        ("a redirect left GitHub", |server, marker| {
            let name = server.publish("9.9.9", &release_archive(&fake_binary("9.9.9", marker)));
            let away = Reply::redirect("https://evil.example/a.tar.gz");
            server.reply(&download_path("9.9.9", &name), away);
        }),
    ];
    for (expected, prepare) in cases {
        let sandbox = Sandbox::new();
        let server = Server::start();
        prepare(&server, &sandbox.root().join("marker"));
        let (code, v, _) = run(sandbox.command(&server).args(["update", "--json"]));
        assert_eq!(code, Some(3), "{expected}: {v}");
        let message = v["error"]["message"].as_str().unwrap();
        assert!(message.contains(expected), "{expected}: {message}");
        assert_eq!(fs::read(&sandbox.bin).unwrap(), original(), "{expected}");
        assert!(leftovers(&sandbox.bin).is_empty(), "{expected}");
    }
}

#[test]
fn a_binary_that_is_not_replaceable_is_refused_with_its_fix() {
    let brew = Sandbox::at("opt/homebrew/Cellar/mailtriage/0.1.0/bin");
    let server = Server::start();
    server.publish(
        "9.9.9",
        &release_archive(&fake_binary("9.9.9", Path::new("/dev/null"))),
    );
    let (code, v, _) = run(brew.command(&server).args(["update", "--json"]));
    assert_eq!(code, Some(3), "{v}");
    let message = v["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("(managed_by_homebrew): run `brew upgrade mailtriage`"),
        "{message}"
    );
    assert_eq!(fs::read(&brew.bin).unwrap(), original());

    let (code, v, _) = run(brew.command(&server).args(["update", "--check", "--json"]));
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(
        v["update"]["install"],
        json!({"path": brew.bin, "replaceable": false, "reason": "managed_by_homebrew", "fix": "run `brew upgrade mailtriage`"})
    );
    assert_eq!(v["update"]["available"], true);

    let locked = Sandbox::new();
    let dir = locked.bin.parent().unwrap().to_path_buf();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    let (code, v, _) = run(locked.command(&server).args(["update", "--json"]));
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    if mailtriage_is_root() {
        return;
    }
    assert_eq!(code, Some(3), "{v}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("(not_writable): make "),
        "{v}"
    );
    assert_eq!(fs::read(&locked.bin).unwrap(), original());
    assert!(leftovers(&locked.bin).is_empty());
}

/// Decision 5: a binary that may not be replaced but is already current
/// is `current` (exit 0); the refusal (exit 3) applies only when there is
/// something to install. Nothing is downloaded and no lock file is made.
#[test]
fn a_binary_that_is_not_replaceable_but_current_is_current() {
    let brew = Sandbox::at("opt/homebrew/Cellar/mailtriage/0.1.0/bin");
    // Group-writable, as `unsafe_permissions` reports a root-owned install.
    let shared = Sandbox::new();
    let dir = shared.bin.parent().unwrap().to_path_buf();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o775)).unwrap();
    for (sandbox, reason) in [
        (&brew, "managed_by_homebrew"),
        (&shared, "unsafe_permissions"),
    ] {
        let server = Server::start();
        server.publish(RUNNING, &release_archive(b"never used"));
        let (code, v, stderr) = run(sandbox.command(&server).args(["update", "--json"]));
        assert_eq!(code, Some(0), "{reason}: {v} {stderr}");
        assert_eq!(
            v["update"],
            json!({"action":"current","from":RUNNING,"to":RUNNING,"path":sandbox.bin,"warnings":[]}),
            "{reason}"
        );
        assert_eq!(server.count(update_support::LIST), 1, "{reason}");
        assert_eq!(server.count("/download"), 0, "{reason}");
        assert_eq!(fs::read(&sandbox.bin).unwrap(), original(), "{reason}");
        let dir = sandbox.bin.parent().unwrap();
        assert!(!dir.join(".mailtriage-update.lock").exists(), "{reason}");
        assert!(leftovers(&sandbox.bin).is_empty(), "{reason}");
        // The binary really is one `update` may not replace.
        let (code, v, _) = run(sandbox
            .command(&server)
            .args(["update", "--check", "--json"]));
        assert_eq!(code, Some(0), "{reason}: {v}");
        assert_eq!(v["update"]["install"]["reason"], reason);
        assert_eq!(v["update"]["available"], false, "{reason}");
    }
}

fn mailtriage_is_root() -> bool {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
}

#[test]
fn check_reports_without_changing_the_binary() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    server.list(&[]);
    let (code, v, _) = run(sandbox
        .command(&server)
        .args(["update", "--check", "--json"]));
    assert_eq!(code, Some(0), "{v}");
    let u = &v["update"];
    assert_eq!(
        (u["current"].as_str(), u["installed"].as_str()),
        (Some(RUNNING), Some(RUNNING))
    );
    assert_eq!(
        (u["latest"].clone(), u["available"].clone()),
        (Value::Null, json!(false))
    );
    assert!(u["checked_at"].as_str().unwrap().ends_with('Z'));

    server.publish("9.9.9", &release_archive(b"unused"));
    let (code, v, _) = run(sandbox
        .command(&server)
        .args(["update", "--check", "--json"]));
    assert_eq!(code, Some(0), "{v}");
    let u = &v["update"];
    assert_eq!(
        (u["latest"].as_str(), u["available"].as_bool()),
        (Some("9.9.9"), Some(true))
    );
    assert_eq!(
        u["release_url"],
        "https://github.com/wir-drei-digital/mailtriage/releases/tag/v9.9.9"
    );
    assert_eq!(u["published_at"], "2026-11-02T09:00:00Z");
    assert_eq!(
        u["install"],
        json!({"path": sandbox.bin, "replaceable": true, "reason": null, "fix": null})
    );
    assert_eq!(server.count("/download"), 0);
    assert_eq!(fs::read(&sandbox.bin).unwrap(), original());
    assert_eq!(sandbox.cache()["release"]["version"], "9.9.9");

    server.reply(update_support::LIST, Reply::status(503));
    let (code, v, _) = run(sandbox
        .command(&server)
        .args(["update", "--check", "--json"]));
    assert_eq!(code, Some(3), "{v}");
    assert_eq!(
        v["error"]["message"],
        "cannot read the release list: GitHub answered 503"
    );
}

#[test]
fn update_needs_a_release_and_a_cache_directory() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    server.list(&[]);
    let (code, v, _) = run(sandbox.command(&server).args(["update", "--json"]));
    assert_eq!(
        (code, v["error"]["message"].as_str()),
        (Some(3), Some("no stable release of mailtriage was found"))
    );
    let (code, v, _) = run(sandbox
        .command(&server)
        .env_remove("HOME")
        .env_remove("XDG_CACHE_HOME")
        .args(["update", "--json"]));
    assert_eq!(code, Some(3), "{v}");
    assert!(
        v["error"]["message"].as_str().unwrap().contains("HOME"),
        "{v}"
    );
}

#[test]
#[cfg(debug_assertions)]
fn a_held_installation_lock_is_exit_5() {
    use fs2::FileExt;
    let sandbox = Sandbox::new();
    let server = Server::start();
    server.publish(
        "9.9.9",
        &release_archive(&fake_binary("9.9.9", Path::new("/dev/null"))),
    );
    let lock = fs::File::create(
        sandbox
            .bin
            .parent()
            .unwrap()
            .join(".mailtriage-update.lock"),
    )
    .unwrap();
    lock.lock_exclusive().unwrap();
    let (code, v, _) = run(sandbox
        .command(&server)
        .env("MAILTRIAGE_UPDATE_TEST_LOCK_WAIT_MS", "300")
        .args(["update", "--json"]));
    assert_eq!(code, Some(5), "{v}");
    assert_eq!(v["error"]["message"], "another update is running");
    assert_eq!(fs::read(&sandbox.bin).unwrap(), original());
}

/// Two caches (different HOME and XDG_CACHE_HOME) against one binary: the
/// installation lock serializes them, and the second finds it current.
#[test]
fn updaters_with_different_caches_serialize_on_the_installation_lock() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    let archive = release_archive(&fake_binary("9.9.9", &sandbox.root().join("marker")));
    let name = server.publish("9.9.9", &archive);
    let path = download_path("9.9.9", &name);
    server.reply(&path, Reply::ok(archive).delayed(Duration::from_secs(1)));
    let (home2, xdg2) = (sandbox.root().join("home2"), sandbox.root().join("xdg2"));
    fs::create_dir_all(&home2).unwrap();
    fs::create_dir_all(&xdg2).unwrap();
    let spawn = |command: &mut std::process::Command| {
        command
            .args(["update", "--json"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    };
    let first = spawn(&mut sandbox.command(&server));
    let second = spawn(
        sandbox
            .command(&server)
            .env("HOME", &home2)
            .env("XDG_CACHE_HOME", &xdg2),
    );
    let mut actions: Vec<String> = [first, second]
        .into_iter()
        .map(|child| {
            let out = child.wait_with_output().unwrap();
            assert_eq!(out.status.code(), Some(0));
            let v: Value = serde_json::from_slice(&out.stdout).unwrap();
            v["update"]["action"].as_str().unwrap().to_owned()
        })
        .collect();
    actions.sort();
    assert_eq!(actions, ["current", "updated"]);
    assert_eq!(server.count(&path), 1);
}

#[test]
#[cfg(debug_assertions)]
fn a_binary_replaced_during_the_update_is_kept() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    server.publish(
        "9.9.9",
        &release_archive(&fake_binary("9.9.9", Path::new("/dev/null"))),
    );
    let theirs = sandbox.root().join("theirs");
    fs::write(&theirs, "#!/bin/sh\necho 'mailtriage 5.0.0'\n").unwrap();
    // Renamed into place, as installers do: on Linux, writing into the
    // file of the running binary fails (ETXTBSY).
    let script = hook(
        &sandbox.root(),
        "revalidate",
        &format!(
            "cp '{0}' '{0}.new' && mv '{0}.new' '{1}'",
            theirs.display(),
            sandbox.bin.display()
        ),
    );
    let (code, v, _) = run(sandbox
        .command(&server)
        .env("MAILTRIAGE_UPDATE_TEST_HOOK", &script)
        .args(["update", "--json"]));
    assert_eq!(code, Some(3), "{v}");
    assert_eq!(
        v["error"]["message"],
        "the installed binary changed during the update; try again"
    );
    assert_eq!(fs::read(&sandbox.bin).unwrap(), fs::read(&theirs).unwrap());
    assert!(leftovers(&sandbox.bin).is_empty());
}

#[test]
#[cfg(debug_assertions)]
fn faults_through_the_test_hook() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    let marker = sandbox.root().join("marker");
    server.publish("9.9.9", &release_archive(&fake_binary("9.9.9", &marker)));
    let script = hook(&sandbox.root(), "commit", "exit 1");
    let (code, v, _) = run(sandbox
        .command(&server)
        .env("MAILTRIAGE_UPDATE_TEST_HOOK", &script)
        .args(["update", "--json"]));
    assert_eq!(code, Some(3), "{v}");
    assert_eq!(fs::read(&sandbox.bin).unwrap(), original());
    assert!(leftovers(&sandbox.bin).is_empty());

    let script = hook(&sandbox.root(), "record", "exit 1");
    let (code, v, _) = run(sandbox
        .command(&server)
        .env("MAILTRIAGE_UPDATE_TEST_HOOK", &script)
        .args(["update", "--json"]));
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(
        v["update"]["warnings"],
        json!(["installed; recording the update failed: the test hook failed at record"])
    );
    assert_eq!(
        fs::read(&sandbox.bin).unwrap(),
        fake_binary("9.9.9", &marker)
    );
}
