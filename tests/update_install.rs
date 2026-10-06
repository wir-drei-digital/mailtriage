#![cfg(unix)]
//! The install transaction against a loopback server, with an installed
//! script standing in for the binary: success, refusals, revalidation and
//! a fault injected before each step of the commit.
mod update_support;
use anyhow::{bail, Result};
use mailtriage::update::{
    cache::Cache,
    github::{Endpoint, Net},
    install::{self, Hooks, Job, NoHooks, Outcome},
    release::{self, CachedRelease},
    CLI, COMPONENTS,
};
use semver::Version;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    time::{Duration, Instant},
};
use update_support::{fake_binary, release_archive, replace_file, sha256_hex, Server};

const OLD: &str = "#!/bin/sh\necho 'mailtriage 0.0.1'\n";

struct Setup {
    _dir: tempfile::TempDir,
    server: Server,
    net: Net,
    cache: Cache,
    path: PathBuf,
    marker: PathBuf,
}

impl Setup {
    /// An installed `mailtriage 0.0.1` script and a server publishing 9.9.9.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let bin = root.join("bin");
        fs::create_dir(&bin).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        let path = bin.join("mailtriage");
        fs::write(&path, OLD).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        let server = Server::start();
        let marker = root.join("marker");
        server.publish("9.9.9", &release_archive(&fake_binary("9.9.9", &marker)));
        let net = Net::new(Endpoint::with_override(Some(&server.base))).unwrap();
        let cache = Cache::new(root.join("cache"));
        Self {
            _dir: dir,
            server,
            net,
            cache,
            path,
            marker,
        }
    }

    fn release(&self) -> CachedRelease {
        let list = self.net.releases().unwrap();
        release::select(&list, release::platform(), COMPONENTS).unwrap()
    }

    fn run(&self, hooks: &dyn Hooks, fallback: &str) -> Result<Outcome> {
        let release = self.release();
        let fallback = Version::parse(fallback).unwrap();
        let job = Job {
            net: &self.net,
            component: CLI,
            path: &self.path,
            release: &release,
            fallback: &fallback,
            cache: Some(&self.cache),
            hooks,
        };
        let lock = install::lock(self.path.parent().unwrap(), Duration::ZERO)
            .unwrap()
            .unwrap();
        install::install(&job, &lock, &mut || Ok(()))
    }

    fn installed(&self) -> String {
        fs::read_to_string(&self.path).unwrap()
    }

    fn previous(&self) -> PathBuf {
        install::previous_path(&self.path)
    }

    /// Temporary files of an attempt left in the directory.
    fn leftovers(&self) -> Vec<String> {
        fs::read_dir(self.path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.starts_with(install::TEMP_PREFIX))
            .collect()
    }
}

/// Fails the step named `.0`.
struct FailAt(&'static str);

impl Hooks for FailAt {
    fn at(&self, point: &str) -> Result<()> {
        if point == self.0 {
            bail!("injected at {point}");
        }
        Ok(())
    }
}

/// Runs `.1` at the step named `.0`.
struct RunAt<F: Fn()>(&'static str, F);

impl<F: Fn()> Hooks for RunAt<F> {
    fn at(&self, point: &str) -> Result<()> {
        if point == self.0 {
            (self.1)();
        }
        Ok(())
    }
}

fn installed(outcome: Outcome) -> install::Installed {
    match outcome {
        Outcome::Installed(done) => done,
        other => panic!("not installed: {other:?}"),
    }
}

#[test]
fn a_newer_release_replaces_the_binary_and_keeps_the_previous_one() {
    use std::os::unix::fs::MetadataExt;
    let s = Setup::new();
    let original = fs::metadata(&s.path).unwrap().ino();
    fs::write(
        s.path
            .parent()
            .unwrap()
            .join(".mailtriage-update-stale.tmp"),
        "old",
    )
    .unwrap();
    let done = installed(s.run(&NoHooks, "0.1.0").unwrap());
    assert_eq!(done.from, Some(Version::new(0, 0, 1)));
    assert_eq!(done.to, Version::new(9, 9, 9));
    assert_eq!(done.previous_path, s.previous());
    assert!(done.warnings.is_empty(), "{:?}", done.warnings);
    assert_eq!(fs::read(&s.path).unwrap(), fake_binary("9.9.9", &s.marker));
    assert_eq!(fs::read_to_string(s.previous()).unwrap(), OLD);
    // Swapped by rename: the old file is the backup, the path a new file.
    assert_eq!(fs::metadata(s.previous()).unwrap().ino(), original);
    assert_ne!(fs::metadata(&s.path).unwrap().ino(), original);
    assert_eq!(
        fs::metadata(&s.path).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert!(s.leftovers().is_empty(), "{:?}", s.leftovers());
    let entry = &s.cache.read().installs[s.path.to_str().unwrap()];
    assert_eq!(entry.version.as_deref(), Some("9.9.9"));
    assert_eq!((entry.failures, entry.next_attempt_at.clone()), (0, None));
    assert!(entry.identity.is_some());
    assert!(s.path.parent().unwrap().join(install::LOCK_FILE).exists());
}

#[test]
fn nothing_is_downloaded_when_the_installed_version_is_current() {
    let s = Setup::new();
    replace_file(&s.path, b"#!/bin/sh\necho 'mailtriage 9.9.9'\n", 0o755);
    assert_eq!(
        s.run(&NoHooks, "0.1.0").unwrap(),
        Outcome::Current {
            installed: Some(Version::new(9, 9, 9))
        }
    );
    // An unreadable installed version is compared with the fallback.
    replace_file(&s.path, b"#!/bin/sh\nexit 1\n", 0o755);
    assert_eq!(
        s.run(&NoHooks, "9.9.9").unwrap(),
        Outcome::Current { installed: None }
    );
    assert_eq!(s.server.count("/download"), 0);
    installed(s.run(&NoHooks, "0.1.0").unwrap());
}

/// Serves a release that must be refused.
type Prepare = fn(&Setup);

#[test]
fn refused_releases_leave_the_binary_and_no_files() {
    let cases: [(&str, Prepare); 4] = [
        ("checksum mismatch for mailtriage-v9.9.9-", |s| {
            let archive = release_archive(&fake_binary("9.9.9", &s.marker));
            let name = format!("mailtriage-v9.9.9-{}.tar.gz", update_support::platform());
            let wrong = format!("{}  {name}\n", sha256_hex(b"something else"));
            s.server.publish_with_sums("9.9.9", &archive, &wrong);
        }),
        (
            "the new binary does not run here: printed version 9.9.8, expected 9.9.9",
            |s| {
                let archive = release_archive(&fake_binary("9.9.8", &s.marker));
                s.server.publish("9.9.9", &archive);
            },
        ),
        (
            "the new binary does not run here: killed by signal 9",
            |s| {
                s.server
                    .publish("9.9.9", &release_archive(b"#!/bin/sh\nkill -9 $$\n"));
            },
        ),
        (
            "release v9.9.9: SHA256SUMS has no line for mailtriage-v9.9.9-",
            |s| {
                let archive = release_archive(&fake_binary("9.9.9", &s.marker));
                s.server.publish_with_sums("9.9.9", &archive, "");
            },
        ),
    ];
    for (expected, prepare) in cases {
        let s = Setup::new();
        prepare(&s);
        let e = s.run(&NoHooks, "0.1.0").unwrap_err().to_string();
        assert!(e.contains(expected), "{expected}: {e}");
        assert_eq!(s.installed(), OLD, "{expected}");
        assert!(!s.previous().exists(), "{expected}");
        assert!(s.leftovers().is_empty(), "{expected}: {:?}", s.leftovers());
        assert!(s.cache.read().installs.is_empty(), "{expected}");
    }
}

#[test]
fn a_release_without_this_platforms_archive_downloads_nothing() {
    let s = Setup::new();
    s.server
        .list(&[s.server.release_json("9.9.9", &[("SHA256SUMS", 10)])]);
    let e = s.run(&NoHooks, "0.1.0").unwrap_err().to_string();
    assert_eq!(
        e,
        format!(
            "release v9.9.9 has no {} archive",
            update_support::platform()
        )
    );
    assert_eq!(s.server.count("/download"), 0);
}

#[test]
fn a_binary_replaced_during_the_update_is_kept() {
    let s = Setup::new();
    let theirs = b"#!/bin/sh\necho 'mailtriage 5.0.0'\n";
    let path = s.path.clone();
    let hooks = RunAt("revalidate", move || replace_file(&path, theirs, 0o755));
    let e = s.run(&hooks, "0.1.0").unwrap_err().to_string();
    assert_eq!(
        e,
        "the installed binary changed during the update; try again"
    );
    assert_eq!(fs::read(&s.path).unwrap(), theirs);
    assert!(s.leftovers().is_empty(), "{:?}", s.leftovers());
}

#[test]
fn a_fault_before_the_commit_point_changes_nothing() {
    for point in ["backup", "commit"] {
        let s = Setup::new();
        fs::write(s.previous(), "older").unwrap();
        let e = s.run(&FailAt(point), "0.1.0").unwrap_err().to_string();
        assert!(e.contains(&format!("injected at {point}")), "{e}");
        assert_eq!(s.installed(), OLD, "{point}");
        assert_eq!(
            fs::read_to_string(s.previous()).unwrap(),
            "older",
            "{point}"
        );
        assert!(s.leftovers().is_empty(), "{point}: {:?}", s.leftovers());
    }
}

#[test]
fn a_fault_after_the_commit_point_is_a_warning_and_nothing_is_reversed() {
    let new = |s: &Setup| fake_binary("9.9.9", &s.marker);

    let s = Setup::new();
    fs::write(s.previous(), "older").unwrap();
    let done = installed(s.run(&FailAt("publish"), "0.1.0").unwrap());
    assert_eq!(fs::read(&s.path).unwrap(), new(&s));
    assert_eq!(fs::read_to_string(s.previous()).unwrap(), "older");
    assert_ne!(done.previous_path, s.previous());
    assert_eq!(fs::read_to_string(&done.previous_path).unwrap(), OLD);
    assert!(
        done.warnings[0].starts_with("installed; the previous binary stays at "),
        "{:?}",
        done.warnings
    );

    let s = Setup::new();
    let done = installed(s.run(&FailAt("sync_dir"), "0.1.0").unwrap());
    assert_eq!(fs::read(&s.path).unwrap(), new(&s));
    assert_eq!(fs::read_to_string(s.previous()).unwrap(), OLD);
    assert!(
        done.warnings[0].starts_with("installed; syncing "),
        "{:?}",
        done.warnings
    );

    let s = Setup::new();
    let done = installed(s.run(&FailAt("record"), "0.1.0").unwrap());
    assert_eq!(fs::read(&s.path).unwrap(), new(&s));
    assert_eq!(
        done.warnings,
        vec!["installed; recording the update failed: injected at record".to_owned()]
    );
    assert!(s.cache.read().installs.is_empty());
}

#[test]
fn a_refused_reservation_stops_before_any_download() {
    let s = Setup::new();
    let release = s.release();
    let fallback = Version::new(0, 1, 0);
    let job = Job {
        net: &s.net,
        component: CLI,
        path: &s.path,
        release: &release,
        fallback: &fallback,
        cache: None,
        hooks: &NoHooks,
    };
    let lock = install::lock(s.path.parent().unwrap(), Duration::ZERO)
        .unwrap()
        .unwrap();
    let e =
        install::install(&job, &lock, &mut || bail!("cannot write the update cache")).unwrap_err();
    assert_eq!(e.to_string(), "cannot write the update cache");
    assert_eq!(s.server.count("/download"), 0);
    assert_eq!(s.installed(), OLD);
}

#[test]
fn the_installation_lock_is_exclusive_and_waits() {
    let dir = tempfile::tempdir().unwrap();
    let held = install::lock(dir.path(), Duration::ZERO).unwrap().unwrap();
    assert!(install::lock(dir.path(), Duration::ZERO).unwrap().is_none());
    let start = Instant::now();
    assert!(install::lock(dir.path(), Duration::from_millis(300))
        .unwrap()
        .is_none());
    assert!(start.elapsed() >= Duration::from_millis(300));
    drop(held);
    assert!(install::lock(dir.path(), Duration::ZERO).unwrap().is_some());
    assert!(dir.path().join(".mailtriage-update.lock").exists());
}

#[test]
fn leftovers_are_removed_but_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        ".mailtriage-update-a.tmp",
        ".mailtriage-update-b.prev",
        ".mailtriage-update.lock",
        "mailtriage",
        "mailtriage.previous",
    ] {
        fs::write(dir.path().join(name), "x").unwrap();
    }
    fs::create_dir(dir.path().join(".mailtriage-update-dir")).unwrap();
    install::remove_leftovers(dir.path());
    let mut names: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            ".mailtriage-update-dir",
            ".mailtriage-update.lock",
            "mailtriage",
            "mailtriage.previous"
        ]
    );
}
