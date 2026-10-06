#![allow(dead_code)]
//! A fake `mailtriage` that records each call and answers from files:
//! `<key>.json` is printed and `<key>.code` is the exit code, where the key
//! is the first two words (`service-status`, `categories-apply`, …) plus
//! `-apply` with `--apply`. `hold-<key>` or `hold-draft-<N>` makes the call
//! wait until the file is removed.
use std::{
    fs,
    path::{Path, PathBuf},
};

pub const FAKE_MAILTRIAGE: &str = r#"#!/bin/sh
dir="$(dirname "$0")"
sep="$(printf '\037')"
line=""
for a in "$@"; do line="$line$a$sep"; done
# One write per call, so concurrent calls never interleave.
printf '%s\n' "$line" >> "$dir/calls.log"
if [ "$1" = "--version" ]; then cat "$dir/version" 2>/dev/null || echo "mailtriage 0.1.0"; exit 0; fi
key="$1-$2"; hold=""
for a in "$@"; do
  case "$a" in
    --apply) key="$key-apply" ;;
    */draft-*.json) hold="$(basename "$a" .json)" ;;
  esac
done
while [ -n "$hold" ] && [ -f "$dir/hold-$hold" ]; do sleep 0.02; done
while [ -f "$dir/hold-$key" ]; do sleep 0.02; done
code=0
if [ -f "$dir/$key.code" ]; then code=$(cat "$dir/$key.code"); fi
if [ -f "$dir/$key.json" ]; then cat "$dir/$key.json"; fi
exit "$code"
"#;

pub struct FakeCli {
    pub dir: tempfile::TempDir,
    /// The fake's canonical path (macOS temporary directories are symlinks).
    pub program: PathBuf,
}

pub fn write_script(path: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A file of `tests/fixtures`, with `@CONFIG@` replaced by `config`.
pub fn fixture(name: &str, config: &Path) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("fixture {name}"))
        .replace("@CONFIG@", &config.display().to_string())
}

impl FakeCli {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let program = fs::canonicalize(dir.path()).unwrap().join("mailtriage");
        write_script(&program, FAKE_MAILTRIAGE);
        Self { dir, program }
    }

    /// A config file next to the fake, canonical.
    pub fn config(&self) -> PathBuf {
        let path = self.dir.path().join("mailtriage.json");
        if !path.exists() {
            fs::write(&path, "{}").unwrap();
        }
        fs::canonicalize(path).unwrap()
    }

    pub fn respond(&self, key: &str, json: &str) {
        fs::write(self.dir.path().join(format!("{key}.json")), json).unwrap();
        let _ = fs::remove_file(self.dir.path().join(format!("{key}.code")));
    }

    pub fn respond_fixture(&self, key: &str, name: &str) {
        self.respond(key, &fixture(name, &self.config()));
    }

    /// Answer `key` with a JSON error object and its exit code.
    pub fn fail(&self, key: &str, code: i32, message: &str, reason: Option<&str>) {
        let mut error = serde_json::json!({"code": code, "message": message});
        if let Some(reason) = reason {
            error["reason"] = reason.into();
        }
        fs::write(
            self.dir.path().join(format!("{key}.json")),
            serde_json::json!({"schema_version": 1, "error": error}).to_string(),
        )
        .unwrap();
        fs::write(
            self.dir.path().join(format!("{key}.code")),
            code.to_string(),
        )
        .unwrap();
    }

    pub fn hold(&self, name: &str) {
        fs::write(self.dir.path().join(format!("hold-{name}")), "").unwrap();
    }

    pub fn release(&self, name: &str) {
        let _ = fs::remove_file(self.dir.path().join(format!("hold-{name}")));
    }

    /// Every call so far, each as its argument list.
    pub fn calls(&self) -> Vec<Vec<String>> {
        fs::read_to_string(self.dir.path().join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(|line| {
                line.split('\u{1f}')
                    .filter(|a| !a.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .collect()
    }

    /// The calls whose first two words are `first second`.
    pub fn calls_of(&self, first: &str, second: &str) -> Vec<Vec<String>> {
        self.calls()
            .into_iter()
            .filter(|c| c.len() > 1 && c[0] == first && c[1] == second)
            .collect()
    }
}
