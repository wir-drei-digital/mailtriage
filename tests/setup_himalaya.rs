#![cfg(unix)]
//! Setup's step 2 with a Himalaya that is missing or untested: it offers a
//! private copy (`himalaya install`, served by the update tests' loopback
//! server with a test-only digest table), `--himalaya-install` answers the
//! offer without prompts, and a Homebrew keg gets the `brew pin` note.
mod install_support;
mod update_support;
use install_support::{fake_himalaya as fake, write_exe};
use mailtriage::{distribution::himalaya::platform_of, update::release};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    os::unix::fs::symlink,
    path::PathBuf,
    process::{Command, Output, Stdio},
};
use update_support::{archive, sha256_hex, Reply, Server};

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    server: Server,
}

impl Fixture {
    /// A home with a Himalaya config for `work`, an empty `bin/` on PATH,
    /// and 2.2.1 for this platform served as a fake that prints
    /// `himalaya v2.2.1 +imap`.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        for d in ["bin", "home/.config/himalaya", "state"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        fs::write(
            root.join("home/.config/himalaya/config.toml"),
            "[accounts.work]\nemail = \"work@example.test\"\nimap.server = \"imaps://mail.example.test\"\n",
        )
        .unwrap();
        let server = Server::start();
        let platform = platform_of(release::platform()).unwrap();
        let tgz = archive(&[("himalaya", fake("himalaya v2.2.1 +imap").as_bytes())]);
        server.reply(
            &format!("/pimalaya/himalaya/releases/download/v2.2.1/himalaya.{platform}.tgz"),
            Reply::ok(tgz.clone()),
        );
        let none = format!("sha256:{}", "0".repeat(64));
        let assets = |digest: &str| {
            let mut assets = serde_json::Map::new();
            for p in ["aarch64-darwin", "x86_64-linux", "aarch64-linux"] {
                let d = if p == platform { digest } else { none.as_str() };
                assets.insert(p.into(), json!(d));
            }
            Value::Object(assets)
        };
        let table = json!({"versions": [
            {"version": "2.1.0", "roles": {}, "assets": assets(&none)},
            {"version": "2.2.1", "roles": {"inbox": "INBOX"},
             "assets": assets(&format!("sha256:{}", sha256_hex(&tgz)))},
        ]});
        fs::write(root.join("versions.json"), table.to_string()).unwrap();
        Self {
            _dir: dir,
            root,
            server,
        }
    }

    fn private(&self) -> PathBuf {
        self.root.join("data/mailtriage/himalaya/2.2.1/himalaya")
    }

    fn config(&self) -> Value {
        serde_json::from_slice(
            &fs::read(self.root.join("home/.config/mailtriage/mailtriage.json")).unwrap(),
        )
        .unwrap()
    }

    fn setup(&self, args: &[&str], stdin: &str) -> (Output, Value) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
            .arg("setup")
            .args(args)
            .args(["--json", "--provider", "fake", "--service", "skip"])
            .current_dir(&self.root)
            .env("HOME", self.root.join("home"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("bin").display()),
            )
            .env("TZ", "Europe/Berlin")
            .env("MT_FAKE_DIR", self.root.join("state"))
            .env("MAILTRIAGE_UPDATE_URL", &self.server.base)
            .env(
                "MAILTRIAGE_TEST_HIMALAYA_VERSIONS",
                self.root.join("versions.json"),
            )
            .env_remove("MAILTRIAGE_CONFIG")
            .env_remove("HIMALAYA_CONFIG")
            .env_remove("XDG_CONFIG_HOME")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
        let out = child.wait_with_output().unwrap();
        let value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
        (out, value)
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn prompts_offer_a_private_himalaya_for_an_untested_one() {
    let f = Fixture::new();
    write_exe(&f.root.join("bin/himalaya"), &fake("himalaya v2.2.2 +imap"));
    // Yes to the private copy, then every default.
    let (out, v) = f.setup(&["--interactive"], &"\n".repeat(12));
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("Install Himalaya 2.2.1 for mailtriage? [Y/n]"),
        "{err}"
    );
    assert!(
        err.contains("Himalaya 2.2.2 is not a tested version"),
        "{err}"
    );
    let engine = &f.config()["accounts"]["work"]["engine"];
    assert_eq!(engine["binary"], f.private().to_str().unwrap());
    assert_eq!(engine["expected_version"], "2.2.1");
}

#[test]
fn declining_the_private_himalaya_fails_with_the_fix() {
    let f = Fixture::new();
    write_exe(&f.root.join("bin/himalaya"), &fake("himalaya v2.2.2 +imap"));
    let (out, v) = f.setup(&["--interactive"], "n\n");
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    let message = v["error"]["message"].as_str().unwrap();
    assert!(
        message.ends_with(&format!(
            "run mailtriage himalaya install, then mailtriage setup --update --himalaya-binary {}, or pass --himalaya-install",
            f.private().display()
        )),
        "{message}"
    );
    assert!(!f.private().exists());
    assert_eq!(f.server.count("/pimalaya/"), 0);
}

#[test]
fn himalaya_install_answers_the_offer_without_prompts() {
    let f = Fixture::new();
    // Nothing on PATH: missing counts like untested.
    let (out, v) = f.setup(&["--yes", "--himalaya-account", "work"], "");
    assert_eq!(out.status.code(), Some(3), "{v}");
    let message = v["error"]["message"].as_str().unwrap();
    assert!(
        message.starts_with("step 2 (Himalaya): himalaya is not on PATH; "),
        "{message}"
    );
    let (out, v) = f.setup(
        &["--yes", "--himalaya-account", "work", "--himalaya-install"],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    assert_eq!(
        f.config()["accounts"]["work"]["engine"]["binary"],
        f.private().to_str().unwrap()
    );
    // A tested Himalaya found on PATH is used as it is.
    fs::remove_file(f.root.join("home/.config/mailtriage/mailtriage.json")).unwrap();
    write_exe(&f.root.join("bin/himalaya"), &fake("himalaya v2.1.0 +imap"));
    let (out, _) = f.setup(
        &["--yes", "--himalaya-account", "work", "--himalaya-install"],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        f.config()["accounts"]["work"]["engine"]["binary"],
        f.root.join("bin/himalaya").to_str().unwrap()
    );
}

#[test]
fn a_himalaya_in_a_homebrew_keg_gets_the_pin_note() {
    let f = Fixture::new();
    let keg = f.root.join("brew/Cellar/himalaya/2.1.0/bin/himalaya");
    write_exe(&keg, &fake("himalaya v2.1.0 +imap"));
    fs::create_dir_all(f.root.join("brew/bin")).unwrap();
    symlink(&keg, f.root.join("brew/bin/himalaya")).unwrap();
    let linked = f.root.join("brew/bin/himalaya");
    let args = ["--yes", "--himalaya-account", "work", "--himalaya-binary"];
    let (out, _) = f.setup(&[&args[..], &[linked.to_str().unwrap()]].concat(), "");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let note = "Homebrew may upgrade Himalaya to a version mailtriage has not tested; \"brew pin himalaya\" holds it, or run mailtriage himalaya install for a private copy.";
    assert_eq!(stderr(&out).matches(note).count(), 1, "{}", stderr(&out));
    // Not for a Himalaya outside a keg.
    fs::remove_file(f.root.join("home/.config/mailtriage/mailtriage.json")).unwrap();
    write_exe(&f.root.join("bin/himalaya"), &fake("himalaya v2.1.0 +imap"));
    let (out, _) = f.setup(&["--yes", "--himalaya-account", "work"], "");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(!stderr(&out).contains("brew pin"), "{}", stderr(&out));
}

#[test]
fn the_mail_item_names_the_private_himalaya_for_an_untested_version() {
    let f = Fixture::new();
    write_exe(&f.root.join("bin/himalaya"), &fake("flip"));
    let (out, v) = f.setup(&["--yes", "--himalaya-account", "work"], "");
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    let items = v["setup"]["doctor"]["items"].as_array().unwrap();
    let mail = items.iter().find(|i| i["check"] == "mail").unwrap();
    assert_eq!(mail["ready"], false);
    assert_eq!(
        mail["error"],
        "Himalaya 2.2.2 is not a tested version (tested: 2.1.0, 2.2.1)"
    );
    assert_eq!(
        mail["fix"],
        format!(
            "run mailtriage himalaya install, then mailtriage setup --update --himalaya-binary {}",
            f.private().display()
        )
    );
}
