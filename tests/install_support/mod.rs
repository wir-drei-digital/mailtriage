#![allow(dead_code)]
//! Shared by the install tests: a fake Himalaya. Nothing here touches the
//! real HOME.
use std::{fs, os::unix::fs::PermissionsExt, path::Path};

/// A fake Himalaya whose `--version` prints `@VERSION@` (`flip`: 2.1.0 the
/// first time, 2.2.2 afterwards). It lists the IMAP account `work`, passes
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

/// Writes an executable file, creating its directory.
pub fn write_exe(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
