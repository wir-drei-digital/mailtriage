//! A tray in a Homebrew keg (`<prefix>/Cellar/mailtriage/<version>/bin/`,
//! `<prefix>/opt/mailtriage` linked to it): it runs, records and
//! re-executes `opt` paths, which outlive `brew upgrade`.
mod support;
use mailtriage_tray::{
    instances::Windows,
    paths::{self, Env, Resolved},
    restart::{self, Restarter},
};
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};
use support::{write_script, AUTOSTART_LAUNCHCTL};

/// `<prefix>/Cellar/mailtriage/<version>/bin` with a `mailtriage` script and
/// a `mailtriage-tray` script printing that version.
fn keg(prefix: &Path, version: &str) -> PathBuf {
    let bin = prefix.join("Cellar/mailtriage").join(version).join("bin");
    fs::create_dir_all(&bin).unwrap();
    write_script(
        &bin.join("mailtriage"),
        &format!("#!/bin/sh\necho 'mailtriage {version}'\n"),
    );
    write_script(
        &bin.join("mailtriage-tray"),
        &format!("#!/bin/sh\necho 'mailtriage-tray {version}'\n"),
    );
    bin
}

fn link_opt(prefix: &Path, version: &str) -> PathBuf {
    fs::create_dir_all(prefix.join("opt")).unwrap();
    let opt = prefix.join("opt/mailtriage");
    let _ = fs::remove_file(&opt);
    symlink(prefix.join("Cellar/mailtriage").join(version), &opt).unwrap();
    opt.join("bin")
}

fn root() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    (dir, root)
}

#[test]
fn the_cli_path_is_the_opt_path_however_it_was_found() {
    let (_dir, root) = root();
    let bin = keg(&root, "1.2.3");
    let opt = link_opt(&root, "1.2.3");
    let env = Env {
        cwd: root.clone(),
        own_exe: Some(bin.join("mailtriage-tray")),
        ..Env::default()
    };
    let expected = opt.join("mailtriage");
    assert_eq!(
        paths::resolve_cli(Some(&bin.join("mailtriage")), &env),
        Ok(expected.clone())
    );
    assert_eq!(paths::resolve_cli(None, &env), Ok(expected.clone()));
    let on_path = Env {
        cwd: root.clone(),
        path: Some(bin.clone().into_os_string()),
        ..Env::default()
    };
    assert_eq!(paths::resolve_cli(None, &on_path), Ok(expected.clone()));
    // The restart passes it on as resolved.
    let resolved = Resolved {
        cli: expected.clone(),
        config: root.join("mailtriage.json"),
    };
    let command = restart::command(&opt.join("mailtriage-tray"), &resolved, &Windows::default());
    let args: Vec<_> = command.get_args().map(|a| a.to_owned()).collect();
    assert!(args.contains(&expected.into_os_string()));
}

#[test]
fn a_tray_in_a_keg_follows_opt_to_the_new_keg() {
    let (_dir, root) = root();
    let old = keg(&root, "1.2.3").join("mailtriage-tray");
    let opt = link_opt(&root, "1.2.3").join("mailtriage-tray");
    let mut r = Restarter::new(old.clone(), restart::identity(&old).unwrap());
    assert_eq!(r.launch_path(), opt);
    let t = Instant::now();
    assert_eq!(r.check(t), None);
    keg(&root, "1.2.4");
    link_opt(&root, "1.2.4");
    fs::remove_dir_all(root.join("Cellar/mailtriage/1.2.3")).unwrap();
    assert_eq!(r.check(t), Some(opt));
}

#[test]
fn start_at_login_records_the_opt_paths() {
    let (_dir, root) = root();
    let bin = keg(&root, "1.2.3");
    // The real tray binary in the keg, next to the fake CLI.
    fs::copy(
        env!("CARGO_BIN_EXE_mailtriage-tray"),
        bin.join("mailtriage-tray"),
    )
    .unwrap();
    let opt = link_opt(&root, "1.2.3");
    write_script(&root.join("launchctl"), AUTOSTART_LAUNCHCTL);
    fs::write(root.join("mailtriage.json"), "{}").unwrap();
    let out = Command::new(bin.join("mailtriage-tray"))
        .args(["autostart", "enable", "--json", "--config"])
        .arg(root.join("mailtriage.json"))
        .arg("--mailtriage")
        .arg(bin.join("mailtriage"))
        .current_dir(&root)
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("xdg"))
        .env("PATH", format!("{}:/usr/bin:/bin", root.display()))
        .env_remove("MAILTRIAGE_CONFIG")
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    assert_eq!(out.status.code(), Some(0), "{v}");
    assert_eq!(
        v["autostart"]["mailtriage"],
        opt.join("mailtriage").to_str().unwrap()
    );
    let item = fs::read_to_string(v["autostart"]["path"].as_str().unwrap()).unwrap();
    let tray = opt.join("mailtriage-tray");
    assert!(item.contains(tray.to_str().unwrap()), "{item}");
    assert!(!item.contains("/Cellar/"), "{item}");
}
