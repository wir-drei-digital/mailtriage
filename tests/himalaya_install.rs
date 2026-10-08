#![cfg(unix)]
//! `mailtriage himalaya install` against the update tests' loopback server
//! and a test-only table of digests (debug builds read it from
//! `MAILTRIAGE_TEST_HIMALAYA_VERSIONS`). HOME and XDG_DATA_HOME are
//! temporary; nothing outside them is written.
mod update_support;
use mailtriage::{distribution::himalaya::platform_of, update::release};
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
};
use update_support::{archive, run, sha256_hex, Server};

/// pimalaya's name for this host's platform.
fn platform() -> &'static str {
    platform_of(release::platform()).expect("a platform pimalaya releases for")
}

fn asset_path(version: &str) -> String {
    format!(
        "/pimalaya/himalaya/releases/download/v{version}/himalaya.{}.tgz",
        platform()
    )
}

/// A Himalaya release archive whose `himalaya` prints `version_line`.
fn himalaya_archive(version_line: &str) -> Vec<u8> {
    let script = format!("#!/bin/sh\nprintf '%s\\n' '{version_line}'\n");
    archive(&[
        ("himalaya", script.as_bytes()),
        ("share/man/himalaya.1.gz", b"man"),
        ("share/completions/himalaya.bash", b"complete"),
    ])
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    server: Server,
}

impl Fixture {
    /// Serves `archive` as 2.2.1 for this platform, with `digest` in the
    /// test table (2.1.0 is listed with digests nothing matches).
    fn new(archive: &[u8], digest: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        // Private whatever the umask, as the protected path rule wants.
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        let server = Server::start();
        server.reply(&asset_path("2.2.1"), update_support::Reply::ok(archive));
        let other = "0".repeat(64);
        let assets = |digest: &str| {
            let mut assets = serde_json::Map::new();
            for p in ["aarch64-darwin", "x86_64-linux", "aarch64-linux"] {
                let d = if p == platform() {
                    digest
                } else {
                    other.as_str()
                };
                assets.insert(p.into(), json!(format!("sha256:{d}")));
            }
            Value::Object(assets)
        };
        let table = json!({"versions": [
            {"version": "2.1.0", "roles": {}, "assets": assets(&other)},
            {"version": "2.2.1", "roles": {"inbox": "INBOX"}, "assets": assets(digest)},
        ]});
        fs::write(root.join("versions.json"), table.to_string()).unwrap();
        fs::create_dir_all(root.join("home")).unwrap();
        Self {
            _dir: dir,
            root,
            server,
        }
    }

    fn data(&self) -> PathBuf {
        self.root.join("data")
    }

    fn installed(&self) -> PathBuf {
        self.data().join("mailtriage/himalaya/2.2.1/himalaya")
    }

    /// `mailtriage himalaya install --json ARGS` under umask 077.
    fn install(&self, args: &[&str]) -> (Option<i32>, Value, String) {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "umask 077; exec \"$@\"", "sh"])
            .arg(env!("CARGO_BIN_EXE_mailtriage"))
            .args(["himalaya", "install", "--json"])
            .args(args)
            .current_dir(&self.root)
            .env("HOME", self.root.join("home"))
            .env("XDG_DATA_HOME", self.data())
            .env("MAILTRIAGE_UPDATE_URL", &self.server.base)
            .env(
                "MAILTRIAGE_TEST_HIMALAYA_VERSIONS",
                self.root.join("versions.json"),
            )
            .env_remove("MAILTRIAGE_CONFIG");
        run(&mut command)
    }
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

#[test]
fn a_tested_release_is_installed_privately_then_reported_current() {
    let data = himalaya_archive("himalaya v2.2.1 +smtp +imap");
    let f = Fixture::new(&data, &sha256_hex(&data));
    let (code, v, err) = f.install(&[]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(
        v,
        json!({"schema_version": 1, "himalaya": {
            "action": "installed", "version": "2.2.1",
            "path": f.installed().to_str().unwrap(),
        }})
    );
    assert_eq!(mode(&f.installed()), 0o755);
    // The directory chain is 0755 whatever the umask.
    for dir in [
        "",
        "mailtriage",
        "mailtriage/himalaya",
        "mailtriage/himalaya/2.2.1",
    ] {
        assert_eq!(mode(&f.data().join(dir)), 0o755, "{dir}");
    }
    // Only the binary was unpacked.
    let names: Vec<String> = fs::read_dir(f.installed().parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(
        !names.iter().any(|n| n == "share" || n.ends_with(".tmp")),
        "{names:?}"
    );
    let downloads = f.server.count("/pimalaya/");
    let (code, v, _) = f.install(&["--version", "2.2.1"]);
    assert_eq!(code, Some(0));
    assert_eq!(v["himalaya"]["action"], "current");
    assert_eq!(
        f.server.count("/pimalaya/"),
        downloads,
        "nothing downloaded"
    );
}

#[test]
fn a_checksum_mismatch_installs_nothing() {
    let data = himalaya_archive("himalaya v2.2.1 +imap");
    let f = Fixture::new(&data, &"a".repeat(64));
    let (code, v, _) = f.install(&[]);
    assert_eq!(code, Some(3), "{v}");
    assert!(v["error"]["message"]
        .as_str()
        .unwrap()
        .starts_with("checksum mismatch for himalaya."));
    assert!(!f.installed().exists());
}

#[test]
fn an_archive_without_himalaya_or_a_binary_that_does_not_run_installs_nothing() {
    for (data, why) in [
        (
            archive(&[("bin/himalaya", b"#!/bin/sh\n")]),
            "has no top-level himalaya",
        ),
        (
            himalaya_archive("himalaya v2.2.2 +imap"),
            "the downloaded Himalaya does not run here: Himalaya 2.2.2 is not a tested version",
        ),
        (
            himalaya_archive("himalaya v2.2.1 +smtp"),
            "built without IMAP",
        ),
    ] {
        let f = Fixture::new(&data, &sha256_hex(&data));
        let (code, v, _) = f.install(&[]);
        assert_eq!(code, Some(3), "{v}");
        let message = v["error"]["message"].as_str().unwrap();
        assert!(message.contains(why), "{message}");
        let dir = f.installed().parent().unwrap().to_path_buf();
        let left: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n != ".mailtriage-update.lock")
            .collect();
        assert!(left.is_empty(), "{left:?}");
    }
}

#[test]
fn an_unlisted_version_exits_2_before_any_download() {
    let data = himalaya_archive("himalaya v2.2.1 +imap");
    let f = Fixture::new(&data, &sha256_hex(&data));
    let (code, v, _) = f.install(&["--version", "2.2.2"]);
    assert_eq!(code, Some(2));
    assert_eq!(
        v["error"]["message"],
        "Himalaya 2.2.2 is not a tested version (tested: 2.1.0, 2.2.1)"
    );
    assert_eq!(f.server.count("/"), 0);
    assert!(!f.data().exists(), "nothing created");
}

#[test]
fn a_shared_writable_data_directory_is_refused() {
    let data = himalaya_archive("himalaya v2.2.1 +imap");
    let f = Fixture::new(&data, &sha256_hex(&data));
    fs::create_dir_all(f.data().join("mailtriage")).unwrap();
    // Safe whatever the umask until the change made on purpose.
    for dir in [f.root.clone(), f.data(), f.data().join("mailtriage")] {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::set_permissions(
        f.data().join("mailtriage"),
        fs::Permissions::from_mode(0o775),
    )
    .unwrap();
    let (code, v, _) = f.install(&[]);
    assert_eq!(code, Some(3), "{v}");
    assert_eq!(v["error"]["reason"], "unsafe_permissions");
    let message = v["error"]["message"].as_str().unwrap();
    assert!(
        message.contains(&format!(
            "{} is writable by group or others",
            f.data().join("mailtriage").display()
        )),
        "{message}"
    );
    assert!(
        !f.data().join("mailtriage/himalaya").exists(),
        "nothing created inside"
    );
    assert_eq!(f.server.count("/"), 0);
}

#[test]
fn a_symlinked_version_directory_is_refused() {
    let data = himalaya_archive("himalaya v2.2.1 +imap");
    let f = Fixture::new(&data, &sha256_hex(&data));
    let elsewhere = f.root.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::create_dir_all(f.data().join("mailtriage/himalaya")).unwrap();
    // Safe whatever the umask until the change made on purpose.
    for dir in [
        f.root.clone(),
        elsewhere.clone(),
        f.data(),
        f.data().join("mailtriage"),
        f.data().join("mailtriage/himalaya"),
    ] {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
    }
    symlink(&elsewhere, f.data().join("mailtriage/himalaya/2.2.1")).unwrap();
    let (code, v, _) = f.install(&[]);
    assert_eq!(code, Some(3), "{v}");
    assert_eq!(v["error"]["reason"], "unsafe_permissions");
    assert!(v["error"]["message"]
        .as_str()
        .unwrap()
        .contains("is a symlink"));
    assert_eq!(
        fs::read_dir(&elsewhere).unwrap().count(),
        0,
        "nothing written there"
    );
}

/// pimalaya's archives carry man pages, completions and JSON schemas: 111
/// entries in 2.2.1, far above the 16 a mailtriage release may hold.
#[test]
fn an_archive_shaped_like_pimalayas_installs() {
    let script = "#!/bin/sh\nprintf '%s\\n' 'himalaya v2.2.1 +imap'\n";
    let docs: Vec<(String, Vec<u8>)> = (0..120)
        .map(|i| (format!("share/schemas/himalaya-{i}.json"), b"{}".to_vec()))
        .collect();
    let mut entries: Vec<(&str, &[u8])> = vec![("himalaya", script.as_bytes())];
    entries.extend(docs.iter().map(|(n, d)| (n.as_str(), d.as_slice())));
    let data = archive(&entries);
    let f = Fixture::new(&data, &sha256_hex(&data));
    let (code, v, err) = f.install(&[]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["himalaya"]["action"], "installed");
}
