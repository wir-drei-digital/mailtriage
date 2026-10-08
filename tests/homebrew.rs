#![cfg(unix)]
//! The Homebrew formula: `render.sh` fills the template from a release's
//! `SHA256SUMS`, and `publish.sh` updates a tap checkout without ever
//! moving it backwards. The tap here is a local bare repository; git runs
//! with no global or system configuration.
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn digest(n: u8) -> String {
    format!("{n:x}").repeat(64)
}

/// A `SHA256SUMS` listing the four archives of `version` (digests 1 to 4)
/// and a few others.
fn sums(version: &str) -> String {
    [
        (
            digest(1),
            format!("mailtriage-v{version}-macos-arm64.tar.gz"),
        ),
        (
            digest(2),
            format!("mailtriage-tray-v{version}-macos-arm64.tar.gz"),
        ),
        (
            digest(3),
            format!("mailtriage-v{version}-linux-amd64.tar.gz"),
        ),
        (
            digest(4),
            format!("mailtriage-v{version}-linux-arm64.tar.gz"),
        ),
        (
            digest(5),
            format!("mailtriage-tray-v{version}-linux-amd64.tar.gz"),
        ),
    ]
    .iter()
    .map(|(d, n)| format!("{d}  {n}\n"))
    .collect()
}

fn render(dir: &Path, version: &str, sums: &str) -> Output {
    let file = dir.join("SHA256SUMS");
    fs::write(&file, sums).unwrap();
    Command::new("sh")
        .arg(repo().join("packaging/homebrew/render.sh"))
        .arg(version)
        .arg(&file)
        .output()
        .unwrap()
}

#[test]
fn the_template_renders_every_checksum_and_parses() {
    let dir = tempfile::tempdir().unwrap();
    let out = render(dir.path(), "1.2.3", &sums("1.2.3"));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let formula = String::from_utf8(out.stdout).unwrap();
    assert!(!formula.contains('@'), "{formula}");
    assert!(formula.contains("  version \"1.2.3\"\n"));
    for n in 1..=4 {
        assert!(
            formula.contains(&format!("sha256 \"{}\"", digest(n))),
            "{n}"
        );
    }
    assert!(!formula.contains(&digest(5)));
    assert!(formula.contains("/releases/download/v1.2.3/mailtriage-tray-v1.2.3-macos-arm64.tar.gz"));
    assert!(formula.contains("depends_on arch: :arm64"));
    assert!(formula.contains("assert_match \"mailtriage #{version}\""));
    // `ruby -c` where Ruby is installed (the macOS CI job runs it anyway).
    let path = dir.path().join("mailtriage.rb");
    fs::write(&path, &formula).unwrap();
    if let Ok(out) = Command::new("ruby").arg("-c").arg(&path).output() {
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn a_missing_doubled_or_malformed_archive_fails_and_prints_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let all = sums("1.2.3");
    let tray = "mailtriage-tray-v1.2.3-macos-arm64.tar.gz";
    let without: String = all
        .lines()
        .filter(|l| !l.ends_with(tray))
        .map(|l| format!("{l}\n"))
        .collect();
    let doubled = format!("{all}{}  {tray}\n", digest(9));
    let malformed = all.replace(
        &format!("{}  {tray}", digest(2)),
        &format!("{} {tray}", digest(2)),
    );
    for (sums, why) in [
        (without, format!("0 lines for {tray}")),
        (doubled, format!("2 lines for {tray}")),
        (malformed, format!("malformed line for {tray}")),
    ] {
        let out = render(dir.path(), "1.2.3", &sums);
        assert_eq!(out.status.code(), Some(1));
        assert!(out.stdout.is_empty());
        assert!(String::from_utf8_lossy(&out.stderr).contains(&why), "{why}");
    }
    let out = render(dir.path(), "v1.2.3", &all);
    assert_eq!(out.status.code(), Some(2));
}

/// git with no global or system configuration and a fixed identity.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.test")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.test")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

struct Tap {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Tap {
    /// A bare "remote" whose main branch holds the formula of `version`, and
    /// a clone of it in `tap/`.
    fn new(version: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        git(&root, &["init", "-q", "--bare", "-b", "main", "remote.git"]);
        git(&root, &["clone", "-q", "remote.git", "seed"]);
        let tap = Tap { _dir: dir, root };
        tap.push_from("seed", version);
        git(&tap.root, &["clone", "-q", "remote.git", "tap"]);
        tap
    }

    fn formula(&self, version: &str) -> PathBuf {
        let path = self.root.join(format!("mailtriage-{version}.rb"));
        let out = render(&self.root, version, &sums(version));
        assert!(out.status.success());
        fs::write(&path, out.stdout).unwrap();
        path
    }

    /// Commits the formula of `version` in the clone `name` and pushes it.
    fn push_from(&self, name: &str, version: &str) {
        let clone = self.root.join(name);
        fs::create_dir_all(clone.join("Formula")).unwrap();
        fs::copy(self.formula(version), clone.join("Formula/mailtriage.rb")).unwrap();
        git(&clone, &["add", "Formula/mailtriage.rb"]);
        git(
            &clone,
            &["commit", "-q", "-m", &format!("mailtriage {version}")],
        );
        git(&clone, &["push", "-q", "origin", "HEAD:main"]);
    }

    fn publish(&self, version: &str) -> Output {
        Command::new("bash")
            .arg(repo().join("packaging/homebrew/publish.sh"))
            .arg(self.root.join("tap"))
            .arg(self.formula(version))
            .arg(version)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap()
    }

    /// The remote's commit subjects, newest first.
    fn log(&self) -> Vec<String> {
        git(
            &self.root.join("remote.git"),
            &["log", "--format=%s", "main"],
        )
        .lines()
        .map(str::to_owned)
        .collect()
    }

    /// Installs a git hook that always fails, as `hooks/NAME` of the
    /// repository in `repository` (relative to the root).
    fn failing_hook(&self, repository: &str, name: &str) {
        let hooks = self.root.join(repository).join("hooks");
        fs::create_dir_all(&hooks).unwrap();
        let hook = hooks.join(name);
        fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn a_newer_release_is_committed_and_pushed() {
    let tap = Tap::new("1.2.3");
    let out = tap.publish("1.2.4");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(tap.log(), ["mailtriage 1.2.4", "mailtriage 1.2.3"]);
}

#[test]
fn the_tap_never_moves_backwards_and_an_identical_formula_is_not_committed() {
    let tap = Tap::new("1.2.3");
    let out = tap.publish("1.2.2");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("higher than 1.2.2"));
    let out = tap.publish("1.2.3");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("already has this formula"));
    let out = tap.publish("1.10.0");
    assert!(out.status.success(), "1.10.0 is higher than 1.2.3");
    assert_eq!(tap.log(), ["mailtriage 1.10.0", "mailtriage 1.2.3"]);
}

#[test]
fn a_push_conflict_is_fetched_decided_again_and_retried_once() {
    let tap = Tap::new("1.2.3");
    // Another job pushed 1.2.4 after this checkout was made.
    git(&tap.root, &["clone", "-q", "remote.git", "other"]);
    tap.push_from("other", "1.2.4");
    let out = tap.publish("1.2.5");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        tap.log(),
        ["mailtriage 1.2.5", "mailtriage 1.2.4", "mailtriage 1.2.3"]
    );
    // When the other job pushed a higher one, nothing is pushed.
    let other = tap.root.join("other");
    git(&other, &["fetch", "-q", "origin", "main"]);
    git(&other, &["reset", "-q", "--hard", "origin/main"]);
    tap.push_from("other", "1.3.0");
    let out = tap.publish("1.2.6");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(tap.log()[0], "mailtriage 1.3.0");
}

#[test]
fn a_failed_step_or_a_second_refused_push_fails_and_leaves_the_tap_alone() {
    // A commit that fails fails the run, without a retry: pushing the
    // unchanged checkout must not count as success.
    let tap = Tap::new("1.2.3");
    tap.failing_hook("tap/.git", "pre-commit");
    let out = tap.publish("1.2.4");
    assert!(!out.status.success());
    assert!(!String::from_utf8_lossy(&out.stdout).contains("trying once more"));
    assert_eq!(tap.log(), ["mailtriage 1.2.3"]);
    // A push the tap refuses again after the retry fails the run.
    let tap = Tap::new("1.2.3");
    tap.failing_hook("remote.git", "pre-receive");
    let out = tap.publish("1.2.4");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("trying once more"));
    assert_eq!(tap.log(), ["mailtriage 1.2.3"]);
}
