//! `mailtriage-tray quit` through the binary against a tray stand-in: this
//! test process takes `tray.lock` and runs the tray's own listener, whose
//! quit releases the lock as the tray's exit would. No menu bar is needed.
//! The cache lives under /tmp, so the socket path stays short.
use mailtriage_tray::{instances, quit};
use serde_json::Value;
use std::{
    fs,
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Self {
        Self {
            dir: tempfile::Builder::new().tempdir_in("/tmp").unwrap(),
        }
    }

    fn cache(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.dir.path().join("Library/Caches/mailtriage")
        } else {
            self.dir.path().join("cache/mailtriage")
        }
    }

    /// `<program> quit --json` with this HOME and cache.
    fn quit(&self, program: &Path) -> (Option<i32>, Value) {
        let out = Command::new(program)
            .args(["quit", "--json"])
            .env("HOME", self.dir.path())
            .env("XDG_CACHE_HOME", self.dir.path().join("cache"))
            .stdin(Stdio::null())
            .output()
            .unwrap();
        (
            out.status.code(),
            serde_json::from_slice(&out.stdout).unwrap_or(Value::Null),
        )
    }
}

/// A copy of the tray binary at `<root>/<dir>/mailtriage-tray`, canonical.
fn installation(root: &Path, dir: &str) -> PathBuf {
    let path = root.join(dir).join("mailtriage-tray");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_mailtriage-tray"), &path).unwrap();
    fs::canonicalize(path).unwrap()
}

/// A running tray of the installation at `own`: the lock and the listener.
/// The receiver gets a message when it quits.
fn tray(cache: &Path, own: PathBuf) -> mpsc::Receiver<()> {
    let lock = Arc::new(Mutex::new(Some(
        instances::tray_lock(cache)
            .unwrap()
            .expect("the lock is free"),
    )));
    let (sent, quits) = mpsc::channel();
    quit::listen(cache, own, move || {
        // The tray exits: its lock is released.
        lock.lock().unwrap().take();
        let _ = sent.send(());
    })
    .unwrap();
    quits
}

#[test]
fn a_running_tray_of_this_installation_quits() {
    let home = Home::new();
    let root = fs::canonicalize(home.dir.path()).unwrap();
    let own = installation(&root, "a");
    let quits = tray(&home.cache(), own.clone());
    // A categories window: a process the tray started, which keeps running.
    // It guards a property `quit` has by construction (it signals no
    // process); a real window cannot start without the GUI event loop,
    // which tray tests never start, so an unrelated process stands in.
    let mut window = Command::new("/bin/sh")
        .args(["-c", "sleep 30"])
        .spawn()
        .unwrap();
    let (code, v) = home.quit(&own);
    assert_eq!((code, v["quit"].clone()), (Some(0), "quit".into()), "{v}");
    assert!(quits.recv_timeout(Duration::from_secs(1)).is_ok());
    assert!(quit::lock_free(&home.cache()));
    assert!(
        window.try_wait().unwrap().is_none(),
        "the window still runs"
    );
    window.kill().unwrap();
    let _ = window.wait();
    // Nothing runs now: not_running.
    let (code, v) = home.quit(&own);
    assert_eq!((code, v["quit"].clone()), (Some(0), "not_running".into()));
}

#[test]
fn another_installations_tray_keeps_running() {
    let home = Home::new();
    let root = fs::canonicalize(home.dir.path()).unwrap();
    let theirs = installation(&root, "b");
    let quits = tray(&home.cache(), theirs);
    // This test's tray binary is another installation than `b`.
    let (code, v) = home.quit(Path::new(env!("CARGO_BIN_EXE_mailtriage-tray")));
    assert_eq!(
        (code, v["quit"].clone()),
        (Some(0), "other_installation".into()),
        "{v}"
    );
    assert!(quits.recv_timeout(Duration::from_millis(300)).is_err());
    assert!(
        !quit::lock_free(&home.cache()),
        "B's tray still holds its lock"
    );
}

#[test]
fn a_stale_socket_with_a_free_lock_is_not_running() {
    let home = Home::new();
    fs::create_dir_all(home.cache()).unwrap();
    drop(UnixListener::bind(home.cache().join(quit::SOCKET)).unwrap());
    assert!(home.cache().join(quit::SOCKET).exists());
    let (code, v) = home.quit(Path::new(env!("CARGO_BIN_EXE_mailtriage-tray")));
    assert_eq!((code, v["quit"].clone()), (Some(0), "not_running".into()));
}

#[test]
fn a_held_lock_without_an_answer_exits_3() {
    let home = Home::new();
    let _held = instances::tray_lock(&home.cache()).unwrap().unwrap();
    let (code, v) = home.quit(Path::new(env!("CARGO_BIN_EXE_mailtriage-tray")));
    assert_eq!(code, Some(3));
    assert_eq!(
        v["error"]["message"],
        "the tray does not respond; quit it from its menu"
    );
}

#[test]
fn a_new_tray_replaces_an_old_socket_file() {
    let home = Home::new();
    fs::create_dir_all(home.cache()).unwrap();
    fs::write(home.cache().join(quit::SOCKET), "left over").unwrap();
    let root = fs::canonicalize(home.dir.path()).unwrap();
    let own = installation(&root, "a");
    let quits = tray(&home.cache(), own.clone());
    let (code, v) = home.quit(&own);
    assert_eq!((code, v["quit"].clone()), (Some(0), "quit".into()), "{v}");
    assert!(quits.recv_timeout(Duration::from_secs(1)).is_ok());
}

/// A cache path longer than a Unix socket path may be (104 bytes on macOS):
/// the tray runs without its socket, and `quit` fails safe (exit 3) while
/// it holds its lock.
#[test]
fn a_socket_path_that_is_too_long_is_not_fatal() {
    let home = Home::new();
    let deep = home.cache().join("x".repeat(120));
    fs::create_dir_all(&deep).unwrap();
    let _held = instances::tray_lock(&deep).unwrap().unwrap();
    assert!(quit::listen(&deep, PathBuf::from("/a/mailtriage-tray"), || {}).is_err());
    quit::listen_or_warn(&deep, PathBuf::from("/a/mailtriage-tray"), || {});
    assert_eq!(
        quit::quit(&deep, Path::new("/a/mailtriage-tray")),
        Err(quit::NO_ANSWER.to_owned())
    );
}
