#![cfg(unix)]
//! `install.sh`, run by `sh` against the update tests' loopback server
//! (`MAILTRIAGE_INSTALL_URL`, honoured only for loopback URLs). A fake
//! `uname` picks the platform; HOME and XDG_DATA_HOME are temporary; the
//! archives' `mailtriage` is a script that records how it was handed over,
//! except in the one run that installs the real binary.
mod update_support;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};
use update_support::{archive, Reply, Server};

const VERSION: &str = "1.2.3";

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("install.sh")
}

fn hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The fake `mailtriage`: records its arguments and what it read on stdin
/// in `$MT_MARK`.
const RECORDER: &[u8] =
    b"#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$MT_MARK/args\"\ncat > \"$MT_MARK/stdin\"\nexit \"${MT_EXIT:-0}\"\n";

fn cli_archive(binary: &[u8]) -> Vec<u8> {
    archive(&[
        ("mailtriage", binary),
        ("LICENSE", b"MIT\n"),
        ("README.md", b"# mailtriage\n"),
    ])
}

fn tray_archive() -> Vec<u8> {
    archive(&[("mailtriage-tray", RECORDER), ("LICENSE", b"MIT\n")])
}

/// A gzip tar with a symlink named `mailtriage` besides the other files.
fn archive_with_link() -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    header.set_mode(0o777);
    builder
        .append_link(&mut header, "mailtriage", "/bin/sh")
        .unwrap();
    for (name, data) in [("LICENSE", &b"MIT\n"[..]), ("README.md", b"# m\n")] {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        builder.append_data(&mut header, name, data).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    server: Server,
}

impl Fixture {
    /// Release 1.2.3 for linux-amd64 with `cli` and the tray, as the newest.
    fn new(cli: &[u8]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        for d in ["fakebin", "home", "data", "mark"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        let uname = root.join("fakebin/uname");
        fs::write(
            &uname,
            "#!/bin/sh\ntouch \"$MT_MARK/uname-ran\"\ncase \"$1\" in -s) echo \"$MT_OS\" ;; -m) echo \"$MT_ARCH\" ;; esac\n",
        )
        .unwrap();
        fs::set_permissions(&uname, fs::Permissions::from_mode(0o755)).unwrap();
        let f = Self {
            _dir: dir,
            root,
            server: Server::start(),
        };
        f.latest(&format!("/releases/tag/v{VERSION}"));
        f.publish(&cli_archive(cli), &tray_archive(), None);
        f
    }

    /// `/releases/latest` redirects to `target`.
    fn latest(&self, target: &str) {
        self.server.reply(
            "/releases/latest",
            Reply::redirect(&self.server.url(target)),
        );
        self.server.reply(target, Reply::ok("release page"));
    }

    /// Serves both archives and a `SHA256SUMS` (computed unless given).
    fn publish(&self, cli: &[u8], tray: &[u8], sums: Option<String>) {
        let download = format!("/releases/download/v{VERSION}");
        let cli_name = format!("mailtriage-v{VERSION}-linux-amd64.tar.gz");
        let tray_name = format!("mailtriage-tray-v{VERSION}-linux-amd64.tar.gz");
        let sums = sums.unwrap_or_else(|| {
            format!(
                "{}  {cli_name}\n{}  {tray_name}\n{}  mailtriage-v{VERSION}-macos-arm64.tar.gz\n",
                hex(cli),
                hex(tray),
                "0".repeat(64)
            )
        });
        self.server
            .reply(&format!("{download}/{cli_name}"), Reply::ok(cli.to_vec()));
        self.server
            .reply(&format!("{download}/{tray_name}"), Reply::ok(tray.to_vec()));
        self.server
            .reply(&format!("{download}/SHA256SUMS"), Reply::ok(sums));
    }

    fn mark(&self, name: &str) -> Option<String> {
        fs::read_to_string(self.root.join("mark").join(name)).ok()
    }

    fn args(&self) -> Vec<String> {
        self.mark("args")
            .map(|a| a.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    /// `sh SCRIPT ARGS` with the fake uname for `os arch`, the server as
    /// GitHub, `tty` as the terminal, and `env`.
    fn run_script(&self, script: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut command = Command::new("sh");
        command
            .arg(script)
            .args(args)
            .current_dir(&self.root)
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("fakebin").display()),
            )
            .env("HOME", self.root.join("home"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("MT_MARK", self.root.join("mark"))
            .env("MT_OS", "Linux")
            .env("MT_ARCH", "x86_64")
            .env("MAILTRIAGE_INSTALL_URL", &self.server.base)
            .env("MAILTRIAGE_INSTALL_TTY", self.root.join("no-tty"))
            .stdin(Stdio::null());
        for (key, value) in env {
            command.env(key, value);
        }
        command.output().unwrap()
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        self.run_script(&script(), args, env)
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn a_fresh_install_hands_over_to_self_install() {
    let f = Fixture::new(RECORDER);
    fs::write(f.root.join("tty"), "answers from the terminal").unwrap();
    let tty = f.root.join("tty");
    let out = f.run(&[], &[("MAILTRIAGE_INSTALL_TTY", tty.to_str().unwrap())]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let dir = f.root.join("home/.local/bin");
    assert_eq!(
        f.args(),
        ["self", "install", "--dir", dir.to_str().unwrap()]
    );
    assert_eq!(
        f.mark("stdin").as_deref(),
        Some("answers from the terminal")
    );
    // Linux without a display: no tray.
    let tray = format!("/releases/download/v{VERSION}/mailtriage-tray-");
    assert_eq!(f.server.count(&tray), 0);
    // With a display, the tray comes along, from the temporary directory,
    // which is gone afterwards.
    let out = f.run(
        &["--no-setup", "--yes"],
        &[
            ("DISPLAY", ":0"),
            ("MAILTRIAGE_INSTALL_TTY", tty.to_str().unwrap()),
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let args = f.args();
    assert_eq!(
        args[..4],
        ["self", "install", "--dir", dir.to_str().unwrap()]
    );
    assert_eq!(args[4], "--tray-file");
    assert!(args[5].ends_with("/mailtriage-tray"));
    assert!(
        !Path::new(&args[5]).exists(),
        "the temporary directory is removed"
    );
    assert_eq!(args[6..], ["--no-setup", "--yes"]);
    assert_eq!(
        f.mark("stdin").as_deref(),
        Some(""),
        "--yes reads /dev/null"
    );
    assert_eq!(f.server.count(&tray), 1);
}

#[test]
fn without_a_terminal_the_hand_over_reads_dev_null_and_its_exit_code_is_kept() {
    let f = Fixture::new(RECORDER);
    let out = f.run(
        &["--version", VERSION, "--dir", "/tmp/x y"],
        &[("MT_EXIT", "5")],
    );
    assert_eq!(out.status.code(), Some(5), "{}", stderr(&out));
    assert_eq!(f.args(), ["self", "install", "--dir", "/tmp/x y"]);
    assert_eq!(f.mark("stdin").as_deref(), Some(""));
    assert_eq!(
        f.server.count("/releases/latest"),
        0,
        "--version needs no lookup"
    );
}

#[test]
fn the_tray_archive_is_fetched_only_when_wanted() {
    let f = Fixture::new(RECORDER);
    let tray = format!("/releases/download/v{VERSION}/mailtriage-tray-");
    let out = f.run(&["--no-tray"], &[("DISPLAY", ":0")]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(f.server.count(&tray), 0);
    let out = f.run(&["--tray"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(f.server.count(&tray), 1);
    // macOS wants the tray by default.
    let out = f.run(&[], &[("MT_OS", "Darwin"), ("MT_ARCH", "arm64")]);
    assert_eq!(out.status.code(), Some(1), "macos-arm64 is not served here");
    assert!(
        f.server.count(&format!(
            "/releases/download/v{VERSION}/mailtriage-v{VERSION}-macos-arm64"
        )) == 1
    );
}

#[test]
fn an_unexpected_latest_url_exits_1() {
    for target in [
        "/releases/tag/v1.2",
        "/releases/tag/v01.2.3",
        "/releases/tag/1.2.3",
        "/elsewhere/tag/v1.2.3",
    ] {
        let f = Fixture::new(RECORDER);
        f.latest(target);
        let out = f.run(&[], &[]);
        assert_eq!(out.status.code(), Some(1), "{target}: {}", stderr(&out));
        assert!(stderr(&out).contains("unexpected URL"), "{target}");
        assert_eq!(f.mark("args"), None);
    }
}

#[test]
fn a_checksum_mismatch_or_a_bad_sums_file_installs_nothing() {
    let cli = cli_archive(RECORDER);
    let tray = tray_archive();
    let name = format!("mailtriage-v{VERSION}-linux-amd64.tar.gz");
    for (sums, why) in [
        (format!("{}  {name}\n", "a".repeat(64)), "checksum mismatch"),
        (
            format!("{}  {name}\n{}  {name}\n", hex(&cli), hex(&cli)),
            "exactly one line",
        ),
        (String::new(), "exactly one line"),
        (format!("{} {name}\n", hex(&cli)), "malformed"),
        (
            format!("{}  {name}\n", hex(&cli).to_uppercase()),
            "malformed",
        ),
    ] {
        let f = Fixture::new(RECORDER);
        f.publish(&cli, &tray, Some(sums.clone()));
        let out = f.run(&["--no-tray"], &[]);
        assert_eq!(out.status.code(), Some(1), "{sums:?}: {}", stderr(&out));
        assert!(stderr(&out).contains(why), "{sums:?}: {}", stderr(&out));
        assert_eq!(f.mark("args"), None);
    }
}

#[test]
fn an_archive_that_is_not_the_release_layout_installs_nothing() {
    for (archive, why) in [
        (
            archive(&[
                ("mailtriage", RECORDER),
                ("LICENSE", b"MIT\n"),
                ("README.md", b"r\n"),
                ("extra", b"x\n"),
            ]),
            "unexpected entry: extra",
        ),
        (archive_with_link(), "a link"),
        (
            archive(&[
                ("mailtriage", RECORDER),
                ("mailtriage", b"#!/bin/sh\n"),
                ("LICENSE", b"MIT\n"),
                ("README.md", b"r\n"),
            ]),
            "mailtriage twice",
        ),
        (
            archive(&[("mailtriage", RECORDER), ("LICENSE", b"MIT\n")]),
            "has no README.md",
        ),
        (
            archive(&[
                ("bin/mailtriage", RECORDER),
                ("LICENSE", b"MIT\n"),
                ("README.md", b"r\n"),
            ]),
            "unexpected entry",
        ),
    ] {
        let f = Fixture::new(RECORDER);
        f.publish(&archive, &tray_archive(), None);
        let out = f.run(&["--no-tray"], &[]);
        assert_eq!(out.status.code(), Some(1), "{why}: {}", stderr(&out));
        assert!(stderr(&out).contains(why), "{why}: {}", stderr(&out));
        assert_eq!(f.mark("args"), None);
    }
}

#[test]
fn unsupported_platforms_and_invalid_options_exit_2() {
    let f = Fixture::new(RECORDER);
    let out = f.run(&[], &[("MT_OS", "FreeBSD"), ("MT_ARCH", "amd64")]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains(
        "mailtriage has no build for FreeBSD amd64; see the guide's Install section to build from source"
    ));
    let out = f.run(&[], &[("MT_OS", "Darwin"), ("MT_ARCH", "x86_64")]);
    assert_eq!(out.status.code(), Some(2));
    for (args, env) in [
        (vec!["--version", "1.2"], vec![]),
        (vec!["--version", "v1.2.3"], vec![]),
        (vec!["--bogus"], vec![]),
        (vec!["--dir"], vec![]),
        (vec![], vec![("MAILTRIAGE_TRAY", "yes")]),
        (
            vec![],
            vec![("MAILTRIAGE_INSTALL_URL", "http://example.com")],
        ),
        (
            vec![],
            vec![("MAILTRIAGE_INSTALL_URL", "http://127.0.0.1.evil.example/")],
        ),
        (
            vec![],
            vec![("MAILTRIAGE_INSTALL_URL", "http://u@127.0.0.1:1/")],
        ),
    ] {
        let out = f.run(&args, &env);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{args:?} {env:?}: {}",
            stderr(&out)
        );
    }
    assert_eq!(f.server.count("/"), 0, "nothing was requested");
}

#[test]
fn uninstall_hands_over_to_self_uninstall_without_a_download() {
    let f = Fixture::new(RECORDER);
    let dir = f.root.join("bin");
    let out = f.run(&["--uninstall", "--dir", dir.to_str().unwrap()], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains(&format!("nothing is installed in {}", dir.display())));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("mailtriage"), RECORDER).unwrap();
    fs::set_permissions(dir.join("mailtriage"), fs::Permissions::from_mode(0o755)).unwrap();
    let out = f.run(
        &["--uninstall", "--dir", dir.to_str().unwrap(), "--yes"],
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        f.args(),
        ["self", "uninstall", "--dir", dir.to_str().unwrap(), "--yes"]
    );
    assert_eq!(f.server.count("/"), 0);
}

/// A literal `~` (a quoted `--dir '~/bin'` or `MAILTRIAGE_INSTALL_DIR`)
/// means HOME, also for `--uninstall`; a `~` elsewhere stays as it is.
#[test]
fn a_literal_tilde_in_the_directory_means_home() {
    let f = Fixture::new(RECORDER);
    let home = f.root.join("home");
    let home = home.to_str().unwrap();
    for (args, env, dir) in [
        (vec!["--dir", "~/bin"], vec![], format!("{home}/bin")),
        (vec!["--dir=~/a b"], vec![], format!("{home}/a b")),
        (
            vec![],
            vec![("MAILTRIAGE_INSTALL_DIR", "~")],
            home.to_owned(),
        ),
        (
            vec![],
            vec![("MAILTRIAGE_INSTALL_DIR", "~/opt/mt")],
            format!("{home}/opt/mt"),
        ),
        (vec!["--dir", "/tmp/~/x"], vec![], "/tmp/~/x".to_owned()),
    ] {
        let out = f.run(&[&["--no-tray"][..], &args].concat(), &env);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{args:?} {env:?}: {}",
            stderr(&out)
        );
        assert_eq!(
            f.args(),
            ["self", "install", "--dir", dir.as_str()],
            "{args:?} {env:?}"
        );
    }
    let out = f.run(&["--uninstall", "--dir", "~/gone"], &[]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(&format!("nothing is installed in {home}/gone")),
        "{}",
        stderr(&out)
    );
}

/// Exit 126 (the shell could not run the program, as from a temporary
/// directory mounted noexec) is kept and explained.
#[test]
fn a_program_that_cannot_run_is_explained() {
    let f = Fixture::new(b"#!/bin/sh\nexit 126\n");
    let out = f.run(&["--no-tray"], &[]);
    assert_eq!(out.status.code(), Some(126), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("could not run the downloaded mailtriage from the temporary directory"),
        "{err}"
    );
    assert!(err.contains("may be mounted noexec"), "{err}");
    assert!(
        err.contains("TMPDIR=<a directory that allows running programs>"),
        "{err}"
    );
    // The same exit from --uninstall's run names the installed file.
    let dir = f.root.join("bin");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("mailtriage"), b"#!/bin/sh\nexit 126\n").unwrap();
    fs::set_permissions(dir.join("mailtriage"), fs::Permissions::from_mode(0o755)).unwrap();
    let out = f.run(&["--uninstall", "--dir", dir.to_str().unwrap()], &[]);
    assert_eq!(out.status.code(), Some(126), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains(&format!("could not run {}/mailtriage", dir.display())),
        "{err}"
    );
    assert!(err.contains("may be mounted noexec"), "{err}");
    // Other exit codes pass through without it.
    let out = f.run(&["--no-tray"], &[]);
    assert_eq!(out.status.code(), Some(126));
    let ok = Fixture::new(RECORDER);
    let out = ok.run(&["--no-tray"], &[("MT_EXIT", "3")]);
    assert_eq!(out.status.code(), Some(3));
    assert!(!stderr(&out).contains("noexec"), "{}", stderr(&out));
}

#[test]
fn a_truncated_script_runs_nothing() {
    let f = Fixture::new(RECORDER);
    let whole = fs::read(script()).unwrap();
    let cut_script = f.root.join("cut.sh");
    for cut in whole.len() - 64..whole.len() {
        let head = &whole[..cut];
        // Dropping only the trailing newline leaves the whole script.
        if head.trim_ascii_end() == whole.trim_ascii_end() {
            continue;
        }
        fs::write(&cut_script, head).unwrap();
        let out = f.run_script(&cut_script, &["--version", "9.9.9"], &[]);
        assert_ne!(out.status.code(), Some(0), "cut at {cut}");
        assert_eq!(f.mark("uname-ran"), None, "cut at {cut} ran");
    }
    assert_eq!(f.server.count("/"), 0);
    // The whole script runs and keeps its arguments.
    let out = f.run_script(&script(), &["--version", "9.9.9", "--no-tray"], &[]);
    assert_eq!(out.status.code(), Some(1), "v9.9.9 is not served");
    assert!(f.mark("uname-ran").is_some());
    assert_eq!(
        f.server
            .count("/releases/download/v9.9.9/mailtriage-v9.9.9-linux-amd64.tar.gz"),
        1
    );
}

#[test]
fn the_real_binary_installs_itself() {
    let binary = fs::read(env!("CARGO_BIN_EXE_mailtriage")).unwrap();
    let f = Fixture::new(&binary);
    let dir = f.root.join("bin");
    let out = f.run(
        &[
            "--yes",
            "--no-setup",
            "--no-tray",
            "--dir",
            dir.to_str().unwrap(),
        ],
        &[("XDG_CACHE_HOME", f.root.join("cache").to_str().unwrap())],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["self_install"]["cli"]["action"], "installed");
    let version = Command::new(dir.join("mailtriage"))
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&version.stdout),
        format!("mailtriage {}\n", env!("CARGO_PKG_VERSION"))
    );
}
