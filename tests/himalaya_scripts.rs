#![cfg(unix)]
//! The CI helpers that read and extend `src/engine/himalaya-versions.json`:
//! the end-to-end matrix and `scripts/add-himalaya-version.sh`, which the
//! weekly check runs. They run on copies; the real data file is only read.
use mailtriage::engine::versions;
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn data_file() -> PathBuf {
    repo().join("src/engine/himalaya-versions.json")
}

#[test]
fn the_e2e_matrix_lists_every_tested_version_with_its_linux_digest() {
    let out = Command::new("python3")
        .arg(repo().join(".github/scripts/himalaya_matrix.py"))
        .arg(data_file())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let matrix: Value = serde_json::from_slice(&out.stdout).unwrap();
    let expected: Vec<Value> = versions::parse(versions::DATA)
        .unwrap()
        .iter()
        .map(|t| json!({"version": t.version, "sha256": t.sha256("x86_64-linux").unwrap()}))
        .collect();
    assert_eq!(matrix, Value::from(expected));
}

/// A release of pimalaya/himalaya as the API returns it, with a digest for
/// each platform except those in `without`.
fn release(version: &str, without: &[&str]) -> Value {
    let assets: Vec<Value> = [
        "aarch64-darwin",
        "x86_64-linux",
        "aarch64-linux",
        "x86_64-darwin",
        "x86_64-windows",
    ]
    .iter()
    .enumerate()
    .map(|(i, platform)| {
        let mut asset = json!({"name": format!("himalaya.{platform}.tgz"), "size": 6_000_000});
        if !without.contains(platform) {
            asset["digest"] = json!(format!("sha256:{}", format!("{i}").repeat(64)));
        }
        asset
    })
    .collect();
    json!({"tag_name": format!("v{version}"), "draft": false, "prerelease": false, "assets": assets})
}

struct Copy {
    dir: tempfile::TempDir,
}

impl Copy {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::copy(data_file(), dir.path().join("versions.json")).unwrap();
        Self { dir }
    }

    fn path(&self) -> PathBuf {
        self.dir.path().join("versions.json")
    }

    fn text(&self) -> String {
        fs::read_to_string(self.path()).unwrap()
    }

    fn add(&self, version: &str, release: &Value) -> Output {
        let fixture = self.dir.path().join("release.json");
        fs::write(&fixture, release.to_string()).unwrap();
        Command::new("bash")
            .arg(repo().join("scripts/add-himalaya-version.sh"))
            .arg(version)
            .env("HIMALAYA_VERSIONS_FILE", self.path())
            .env("HIMALAYA_RELEASE_JSON", &fixture)
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .output()
            .unwrap()
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn the_helper_adds_the_release_digests_and_the_previous_roles() {
    let copy = Copy::new();
    let before = copy.text();
    let out = copy.add("2.3.0", &release("2.3.0", &[]));
    assert!(out.status.success(), "{}", stderr(&out));
    let after = copy.text();
    // The listed entries keep their bytes; the new one comes last.
    let kept = before.strip_suffix("\n]}\n").unwrap();
    assert!(after.starts_with(&format!("{kept},\n")), "{after}");
    let tested = versions::parse(&after).unwrap();
    let new = tested.last().unwrap();
    assert_eq!(new.version, "2.3.0");
    assert_eq!(new.roles, tested[tested.len() - 2].roles);
    assert_eq!(new.sha256("aarch64-darwin"), Some(&*"0".repeat(64)));
    assert_eq!(new.sha256("x86_64-linux"), Some(&*"1".repeat(64)));
    assert_eq!(new.sha256("aarch64-linux"), Some(&*"2".repeat(64)));
    assert!(String::from_utf8_lossy(&out.stdout).contains("roles copied from"));
}

#[test]
fn a_missing_digest_fails_and_changes_nothing() {
    let copy = Copy::new();
    let before = copy.text();
    let out = copy.add("2.3.0", &release("2.3.0", &["aarch64-linux"]));
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("no sha256 digest for himalaya.aarch64-linux.tgz"),
        "{}",
        stderr(&out)
    );
    assert_eq!(copy.text(), before);
}

#[test]
fn a_listed_lower_or_unstable_version_is_refused() {
    let copy = Copy::new();
    let before = copy.text();
    for (version, release, why) in [
        ("2.2.1", release("2.2.1", &[]), "already listed"),
        ("2.2.0", release("2.2.0", &[]), "not higher"),
        ("2.3.0", release("2.3.1", &[]), "not v2.3.0"),
        (
            "2.3.0",
            {
                let mut r = release("2.3.0", &[]);
                r["prerelease"] = json!(true);
                r
            },
            "prerelease",
        ),
    ] {
        let out = copy.add(version, &release);
        assert_eq!(out.status.code(), Some(1), "{version}");
        assert!(stderr(&out).contains(why), "{version}: {}", stderr(&out));
    }
    let out = copy.add("v2.3.0", &release("2.3.0", &[]));
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(copy.text(), before);
}

#[test]
fn a_file_that_cannot_be_written_back_is_left_unchanged() {
    // An existing entry without one platform cannot be rendered in the
    // file's layout; the helper must fail before it touches the file.
    let copy = Copy::new();
    let mut data: Value = serde_json::from_str(&copy.text()).unwrap();
    data["versions"][0]["assets"]
        .as_object_mut()
        .unwrap()
        .remove("aarch64-linux");
    fs::write(copy.path(), data.to_string()).unwrap();
    let before = copy.text();
    let out = copy.add("2.3.0", &release("2.3.0", &[]));
    assert!(!out.status.success(), "{}", stderr(&out));
    assert_eq!(copy.text(), before);
    let mut left: Vec<String> = fs::read_dir(copy.dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, ["release.json", "versions.json"]);
}
