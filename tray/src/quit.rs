//! `mailtriage-tray quit`: ends the running tray of this installation
//! without signalling any process. The tray listens on `tray.sock` next to
//! `tray.lock` in the cache directory (mode 0700); `quit` sends
//! `quit <its canonical path>`, and the tray quits, as its menu's Quit does,
//! only when that path is its own installation's.
use crate::{brew, instances::TRAY_LOCK, paths};
use fs2::FileExt;
use serde_json::json;
use std::{
    fs::{self, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    os::unix::{ffi::OsStrExt, net::UnixListener, net::UnixStream},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

/// The socket's file name in the cache directory.
pub const SOCKET: &str = "tray.sock";
/// How long `quit` waits for an answer, and then for `tray.lock` to be free.
pub const WAIT: Duration = Duration::from_secs(5);
/// What `quit` says when the tray holds its lock but does not answer.
pub const NO_ANSWER: &str = "the tray does not respond; quit it from its menu";

/// Listens on `<cache>/tray.sock` for the tray whose canonical path is
/// `own`, on a thread of its own. Call it only while holding `tray.lock`:
/// that proves no other tray owns an old socket file, which is removed
/// first. `on_quit` runs after the answer to a request from this
/// installation; the thread then stops listening.
pub fn listen(cache: &Path, own: PathBuf, on_quit: impl Fn() + Send + 'static) -> io::Result<()> {
    let path = cache.join(SOCKET);
    match fs::remove_file(&path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let listener = UnixListener::bind(&path)?;
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            if answer(stream, &own) {
                on_quit();
                return;
            }
        }
    });
    Ok(())
}

/// Answers one connection; `true` when it asked this installation to quit.
fn answer(stream: UnixStream, own: &Path) -> bool {
    let _ = stream.set_read_timeout(Some(WAIT));
    let mut line = Vec::new();
    let mut reader = BufReader::new(&stream);
    if reader.read_until(b'\n', &mut line).is_err() {
        return false;
    }
    let line = line.strip_suffix(b"\n").unwrap_or(&line);
    let (reply, quit) = match line.strip_prefix(b"quit ") {
        Some(path) => {
            let theirs = Path::new(std::ffi::OsStr::from_bytes(path));
            if brew::same_installation(own, theirs) {
                ("quit\n", true)
            } else {
                ("other_installation\n", false)
            }
        }
        None => ("unknown\n", false),
    };
    let mut writer = &stream;
    let _ = writer.write_all(reply.as_bytes());
    let _ = writer.flush();
    quit
}

/// What `quit` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The tray of this installation quit and released `tray.lock`.
    Quit,
    /// No tray runs: nothing listens and `tray.lock` is free.
    NotRunning,
    /// A tray of another installation runs; it keeps running.
    OtherInstallation,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Quit => "quit",
            Outcome::NotRunning => "not_running",
            Outcome::OtherInstallation => "other_installation",
        }
    }
}

/// Whether no process holds `<cache>/tray.lock` (a missing file is free).
pub fn lock_free(cache: &Path) -> bool {
    let Ok(file) = OpenOptions::new().read(true).open(cache.join(TRAY_LOCK)) else {
        return true;
    };
    let free = file.try_lock_exclusive().is_ok();
    let _ = FileExt::unlock(&file);
    free
}

/// Asks the tray listening in `cache` to quit on behalf of `own` (this
/// program's canonical path). `Err` is the exit-3 message: the tray holds
/// its lock but does not answer, or did not stop within `WAIT`.
pub fn quit(cache: &Path, own: &Path) -> Result<Outcome, String> {
    let Ok(mut stream) = UnixStream::connect(cache.join(SOCKET)) else {
        return if lock_free(cache) {
            Ok(Outcome::NotRunning)
        } else {
            Err(NO_ANSWER.to_owned())
        };
    };
    let bytes = own.as_os_str().as_bytes();
    if bytes.contains(&b'\n') {
        return Err(format!("{} cannot be sent to the tray", own.display()));
    }
    let _ = stream.set_read_timeout(Some(WAIT));
    let mut request = b"quit ".to_vec();
    request.extend_from_slice(bytes);
    request.push(b'\n');
    if stream.write_all(&request).is_err() {
        return Err(NO_ANSWER.to_owned());
    }
    let mut reply = String::new();
    let _ = BufReader::new(&stream).read_line(&mut reply);
    match reply.trim_end() {
        "other_installation" => Ok(Outcome::OtherInstallation),
        "quit" => {
            let deadline = Instant::now() + WAIT;
            while !lock_free(cache) {
                if Instant::now() >= deadline {
                    return Err(NO_ANSWER.to_owned());
                }
                thread::sleep(Duration::from_millis(50));
            }
            Ok(Outcome::Quit)
        }
        _ => Err(NO_ANSWER.to_owned()),
    }
}

/// `mailtriage-tray quit [--json]`: prints the outcome and returns the exit
/// code (0, or 3 when the tray did not stop).
pub fn run(json_mode: bool) -> i32 {
    let env = paths::Env::current();
    let result = match (
        paths::cache_dir(env.home.as_deref(), env.xdg_cache_home.as_deref()),
        env.own_exe,
    ) {
        (Some(cache), Some(own)) => quit(&cache, &own),
        (None, _) => Err("HOME is not set".to_owned()),
        (_, None) => Err("cannot find this program's path".to_owned()),
    };
    match result {
        Ok(outcome) => {
            let value = json!({"schema_version": 1, "quit": outcome.as_str()});
            if json_mode {
                println!("{value}");
            } else {
                println!("{}", outcome.as_str());
            }
            0
        }
        Err(message) => {
            if json_mode {
                println!(
                    "{}",
                    json!({"schema_version": 1, "error": {"code": 3, "message": message}})
                );
            } else {
                eprintln!("mailtriage-tray: {message}");
            }
            3
        }
    }
}

/// Starts the listener for a tray that holds `tray.lock` in `cache`; a tray
/// whose socket cannot be created (for example a path longer than the
/// system allows) runs without it and says so.
pub fn listen_or_warn(cache: &Path, own: PathBuf, on_quit: impl Fn() + Send + 'static) {
    if let Err(e) = listen(cache, own, on_quit) {
        eprintln!(
            "mailtriage-tray: cannot listen on {}: {e}; mailtriage-tray quit will not reach this tray",
            cache.join(SOCKET).display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instances;

    #[test]
    fn requests_are_answered_by_installation() {
        let own = Path::new("/opt/homebrew/Cellar/mailtriage/1.2.3/bin/mailtriage-tray");
        for (theirs, reply, quits) in [
            (own.to_path_buf(), "quit\n", true),
            (
                PathBuf::from("/opt/homebrew/Cellar/mailtriage/1.2.4/bin/mailtriage-tray"),
                "quit\n",
                true,
            ),
            (
                PathBuf::from("/home/a/.local/bin/mailtriage-tray"),
                "other_installation\n",
                false,
            ),
        ] {
            let (mut client, server) = UnixStream::pair().unwrap();
            let mut request = b"quit ".to_vec();
            request.extend_from_slice(theirs.as_os_str().as_bytes());
            request.push(b'\n');
            client.write_all(&request).unwrap();
            assert_eq!(answer(server, own), quits);
            let mut got = String::new();
            BufReader::new(&client).read_line(&mut got).unwrap();
            assert_eq!(got, reply);
        }
    }

    #[test]
    fn instances_share_the_lock_file_name() {
        let dir = tempfile::Builder::new().tempdir_in("/tmp").unwrap();
        assert!(lock_free(dir.path()));
        let held = instances::tray_lock(dir.path()).unwrap().unwrap();
        assert!(!lock_free(dir.path()));
        drop(held);
        assert!(lock_free(dir.path()));
    }
}
