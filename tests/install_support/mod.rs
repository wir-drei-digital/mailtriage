#![allow(dead_code)]
//! Shared by the install tests: fake Himalaya and tray programs, and
//! starting a copied binary. Nothing here touches the real HOME.
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Child, Command, Output},
    thread,
    time::Duration,
};

/// `ETXTBSY` (26 on Linux and macOS): another test thread forked while this
/// test's copy of a program was open for writing, and that child holds the
/// handle until it execs. Starting the program is retried briefly, as
/// `src/update/platform.rs` does.
const TEXT_FILE_BUSY: i32 = 26;
const BUSY_RETRIES: u32 = 20;
const BUSY_PAUSE: Duration = Duration::from_millis(50);

/// `command.spawn()`, retried while the program is busy.
pub fn spawn(command: &mut Command) -> Child {
    for _ in 0..BUSY_RETRIES {
        match command.spawn() {
            Err(e) if e.raw_os_error() == Some(TEXT_FILE_BUSY) => thread::sleep(BUSY_PAUSE),
            result => return result.unwrap(),
        }
    }
    command.spawn().unwrap()
}

/// `command.output()`, retried while the program is busy, or while the
/// program a shell `exec`s is (exit 126, "Text file busy"): it never ran.
pub fn output(command: &mut Command) -> Output {
    for _ in 0..BUSY_RETRIES {
        match command.output() {
            Err(e) if e.raw_os_error() == Some(TEXT_FILE_BUSY) => {}
            Ok(out)
                if out.status.code() == Some(126)
                    && String::from_utf8_lossy(&out.stderr).contains("Text file busy") => {}
            result => return result.unwrap(),
        }
        thread::sleep(BUSY_PAUSE);
    }
    command.output().unwrap()
}

/// A fake Himalaya whose `--version` prints `@VERSION@` (`flip`: 2.1.0 the
/// first time, 2.2.2 afterwards). `flip` is for tests that run under their
/// own `MAILTRIAGE_TEST_HIMALAYA_VERSIONS` table, which lists 2.1.0 and not
/// 2.2.2, so the compiled data file may list 2.2.2 one day; tests on the
/// compiled data derive their untested version from it instead (see
/// `tests/himalaya_versions.rs`). It lists the IMAP account `work`, passes
/// `account check`, reports capabilities without SPECIAL-USE and lists
/// INBOX. Its state lives in `MT_FAKE_DIR`, so a copy installed elsewhere
/// behaves the same.
const FAKE_HIMALAYA: &str = r#"#!/usr/bin/env python3
import json, os, sys
state = os.environ["MT_FAKE_DIR"]
argv = sys.argv[1:]
with open(os.path.join(state, "calls.log"), "a") as log:
    log.write(json.dumps([sys.argv[0]] + argv) + "\n")
def out(text):
    sys.stdout.write(text)
    sys.stdout.flush()
if argv and argv[-1] == "--version":
    line = "@VERSION@"
    if line == "flip":
        # Tested at the first call, untested afterwards.
        flag = os.path.join(state, "flipped")
        line = "himalaya v2.2.2 +imap" if os.path.exists(flag) else "himalaya v2.1.0 +imap"
        open(flag, "a").close()
    out(line + "\nbuild: test\n")
    sys.exit(0)
words = " ".join(argv)
if "account list" in words:
    out(json.dumps({"accounts": [{"name": "work", "default": True, "backends": ["imap"]}]}))
elif "account check" in words:
    out(json.dumps({"account": "work", "backends": [{"backend": "imap", "ok": True}]}))
elif "imap raw" in words:
    out('* CAPABILITY IMAP4rev1 MOVE\r\na1 OK done\r\n* NAMESPACE (("" "/")) NIL NIL\r\na2 OK done\r\n')
elif "imap list" in words:
    out(json.dumps({"mailboxes": [{"name": "INBOX", "delimiter": "/", "attributes": []}]}))
else:
    sys.exit(7)
"#;

/// The fake Himalaya printing `version_line` (or `flip`).
pub fn fake_himalaya(version_line: &str) -> String {
    FAKE_HIMALAYA.replace("@VERSION@", version_line)
}

/// A fake tray: `--version` prints `mailtriage-tray VERSION`; any other
/// call appends its arguments to `tray.log` next to the file it runs from.
/// `quit` prints `quit.json` from there (default: the result `quit`) and
/// exits with `quit.code` (default 0).
pub fn fake_tray(version: &str) -> String {
    format!(
        r#"#!/bin/sh
dir="$(dirname "$0")"
if [ "$1" = --version ]; then echo 'mailtriage-tray {version}'; exit 0; fi
echo "$*" >> "$dir/tray.log"
if [ "$1" = quit ]; then
  if [ -f "$dir/quit.json" ]; then cat "$dir/quit.json"; else echo '{{"schema_version":1,"quit":"quit"}}'; fi
  exit "$(cat "$dir/quit.code" 2>/dev/null || echo 0)"
fi
"#
    )
}

/// Writes an executable file, creating its directory.
pub fn write_exe(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
