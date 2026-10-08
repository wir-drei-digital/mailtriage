#![cfg(unix)]
//! A mailtriage in a Homebrew keg: `<prefix>/Cellar/mailtriage/<version>/bin/`
//! with `<prefix>/opt/mailtriage` linked to it, as brew lays it out. The
//! service records the `opt` path, the updater only notifies, and a running
//! `watch` follows `opt` to the new keg after `brew upgrade`. The real
//! binary is copied into the fake kegs; launchctl and systemctl are fakes.
mod common;
mod update_support;
use common::{write_tool, LAUNCHCTL, SYSTEMCTL};
use mailtriage::{system_service::Manager, update::service_files};
use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
};
use update_support::{run, Sandbox, Server};

/// Copies the binary under test into the keg of `version` under `prefix`.
fn keg(prefix: &Path, version: &str) -> PathBuf {
    let bin = prefix.join("Cellar/mailtriage").join(version).join("bin");
    fs::create_dir_all(&bin).unwrap();
    let exe = bin.join("mailtriage");
    fs::copy(env!("CARGO_BIN_EXE_mailtriage"), &exe).unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
    exe
}

/// Points `<prefix>/opt/mailtriage` at the keg of `version`; returns the
/// `opt` path of the binary.
fn link_opt(prefix: &Path, version: &str) -> PathBuf {
    fs::create_dir_all(prefix.join("opt")).unwrap();
    let opt = prefix.join("opt/mailtriage");
    let _ = fs::remove_file(&opt);
    symlink(prefix.join("Cellar/mailtriage").join(version), &opt).unwrap();
    opt.join("bin/mailtriage")
}

fn manager() -> Manager {
    service_files::platform_manager().expect("macOS or Linux")
}

#[test]
fn service_install_records_the_opt_path() {
    let sandbox = Sandbox::new();
    let root = sandbox.root();
    let prefix = root.join("brew");
    let keg_exe = keg(&prefix, "1.2.3");
    let opt_exe = link_opt(&prefix, "1.2.3");
    let tools = root.join("tools");
    fs::create_dir_all(&tools).unwrap();
    write_tool(&tools, "launchctl", LAUNCHCTL);
    write_tool(&tools, "systemctl", SYSTEMCTL);
    let config = sandbox.config("cfg", "off");
    for started_as in [&opt_exe, &keg_exe] {
        let (code, out, err) = run(std::process::Command::new(started_as)
            .args([
                "service",
                "install",
                "--account",
                "work",
                "--json",
                "--config",
            ])
            .arg(&config)
            .env("HOME", &sandbox.home)
            .env("XDG_CACHE_HOME", &sandbox.xdg)
            .env("MT_FAKE_HOME", &sandbox.home)
            .env("PATH", format!("{}:/usr/bin:/bin", tools.display()))
            .env_remove("MAILTRIAGE_CONFIG"));
        assert_eq!(code, Some(0), "{out} {err}");
        let files = service_files::list(manager(), &sandbox.home);
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].executable.as_deref(),
            Some(opt_exe.as_path()),
            "started as {}",
            started_as.display()
        );
    }
}

#[test]
fn the_updater_still_only_notifies_for_a_keg() {
    let sandbox = Sandbox::new();
    let prefix = sandbox.root().join("brew");
    let keg_exe = keg(&prefix, "1.2.3");
    let opt_exe = link_opt(&prefix, "1.2.3");
    let server = Server::start();
    server.list(&[]);
    let (code, out, err) = run(std::process::Command::new(&opt_exe)
        .args(["update", "--check", "--json"])
        .env("HOME", &sandbox.home)
        .env("XDG_CACHE_HOME", &sandbox.xdg)
        .env("MAILTRIAGE_UPDATE_URL", &server.base)
        .env_remove("MAILTRIAGE_CONFIG"));
    assert_eq!(code, Some(0), "{out} {err}");
    let install = &out["update"]["install"];
    assert_eq!(install["reason"], "managed_by_homebrew");
    // The cache key stays the canonical keg path.
    assert_eq!(install["path"], keg_exe.to_str().unwrap());
}

#[test]
fn watch_follows_opt_to_the_new_keg_when_the_old_one_is_deleted() {
    let sandbox = Sandbox::new();
    let prefix = sandbox.root().join("brew");
    keg(&prefix, "1.2.3");
    let opt_exe = link_opt(&prefix, "1.2.3");
    let server = Server::start();
    let config = sandbox.config("cfg", "off");
    let mut watch = update_support::Watch::spawn(
        std::process::Command::new(&opt_exe)
            .args([
                "watch",
                "--account",
                "work",
                "--interval-seconds",
                "1",
                "--json",
            ])
            .current_dir(sandbox.root())
            .env("HOME", &sandbox.home)
            .env("XDG_CACHE_HOME", &sandbox.xdg)
            .env("MAILTRIAGE_UPDATE_URL", &server.base)
            .env("MAILTRIAGE_CONFIG", &config)
            .env("PATH", "/usr/bin:/bin"),
    );
    watch.wait_passes(1);
    // `brew upgrade`: a new keg, `opt` retargeted, the old keg deleted.
    keg(&prefix, "1.2.4");
    link_opt(&prefix, "1.2.4");
    fs::remove_dir_all(prefix.join("Cellar/mailtriage/1.2.3")).unwrap();
    let restarting = watch.event("restarting");
    assert_eq!(restarting["update"]["pid"], watch.child.id());
    let before = watch.passes();
    watch.wait_passes(before + 1);
    assert!(watch.events("error").is_empty(), "{:#?}", watch.seen);
    assert_eq!(watch.events("restarting").len(), 1, "{:#?}", watch.seen);
    watch.stop();
}
