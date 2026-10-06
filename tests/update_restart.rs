#![cfg(unix)]
//! `watch` re-executes itself onto a replaced binary: the real binary, a
//! script that does not run until `chmod +x`, and a failing `exec`.
mod update_support;
use std::{fs, os::unix::fs::PermissionsExt};
use update_support::{fake_binary, replace_file, run, wait_exit, Sandbox, Server};

const RUNNING: &str = env!("CARGO_PKG_VERSION");

#[test]
fn watch_restarts_onto_a_fresh_copy_and_keeps_its_pid() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    let config = sandbox.config("cfg", "off");
    // Passes 3 s apart: a `sync` started right after one ends long before
    // the next, so the two never compete for the account lock.
    let mut watch = sandbox.watch_with_interval(&server, &config, 3);
    watch.wait_passes(2);
    // A fresh copy of the same binary: a new inode at the same path.
    let binary = fs::read(env!("CARGO_BIN_EXE_mailtriage")).unwrap();
    replace_file(&sandbox.bin, &binary, 0o755);
    let restarting = watch.event("restarting");
    assert_eq!(restarting["update"]["pid"], watch.child.id());
    assert_eq!(restarting["update"]["from"], RUNNING);
    assert_eq!(restarting["update"]["to"], RUNNING);
    // The same process passes again, with the config from MAILTRIAGE_CONFIG.
    let before = watch.passes();
    watch.wait_passes(before + 1);
    assert_eq!(watch.seen.last().unwrap()["account"], "work");
    // Right after that pass another worker gets the account lock: the
    // restart left no lock held.
    let (code, out, err) = run(sandbox
        .command(&server)
        .env("MAILTRIAGE_CONFIG", &config)
        .args(["sync", "--account", "work", "--json"]));
    assert_eq!(code, Some(0), "the account lock stayed held: {out} {err}");
    // `watch` keeps passing in the same process.
    watch.wait_passes(before + 2);
    assert_eq!(watch.seen.last().unwrap()["account"], "work");
    assert!(watch.child.try_wait().unwrap().is_none());
    assert_eq!(watch.events("restarting").len(), 1);
    let (code, last) = watch.stop();
    assert_eq!(code, Some(0));
    assert_eq!(last["watch"]["stopped"], true);
    assert!(server.requests().is_empty());
}

#[test]
fn a_replacement_that_does_not_run_is_reported_once_until_it_changes() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    let config = sandbox.config("cfg", "off");
    let marker = sandbox.root().join("marker");
    let mut watch = sandbox.watch(&server, &config);
    watch.wait_passes(1);
    replace_file(&sandbox.bin, &fake_binary("9.9.9", &marker), 0o644);
    let error = watch.event("error");
    let message = error["update"]["message"].as_str().unwrap();
    assert!(
        message.ends_with("does not run: could not start"),
        "{message}"
    );
    let passes = watch.passes();
    watch.wait_passes(passes + 3);
    assert_eq!(watch.events("error").len(), 1);
    // `chmod +x` changes the file's change time and mode: retried at once.
    fs::set_permissions(&sandbox.bin, fs::Permissions::from_mode(0o755)).unwrap();
    let restarting = watch.event("restarting");
    assert_eq!(restarting["update"]["to"], "9.9.9");
    // The script now runs in watch's place with watch's arguments.
    let status = wait_exit(&mut watch.child);
    assert_eq!(status.code(), Some(0));
    let args = fs::read_to_string(&marker).unwrap();
    assert_eq!(
        args.trim(),
        "watch --account work --interval-seconds 1 --json"
    );
}

#[test]
#[cfg(debug_assertions)]
fn a_failed_exec_keeps_the_old_code_running() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    let config = sandbox.config("cfg", "off");
    let hook = sandbox.root().join("hook.sh");
    fs::write(
        &hook,
        format!(
            "#!/bin/sh\n[ \"$1\" = exec ] || exit 0\nrm -f '{}'\n",
            sandbox.bin.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    let mut watch = update_support::Watch::spawn(
        sandbox
            .command(&server)
            .env("MAILTRIAGE_CONFIG", &config)
            .env("MAILTRIAGE_UPDATE_TEST_HOOK", &hook)
            .args([
                "watch",
                "--account",
                "work",
                "--interval-seconds",
                "1",
                "--json",
            ]),
    );
    watch.wait_passes(1);
    replace_file(
        &sandbox.bin,
        &fake_binary("9.9.9", &sandbox.root().join("m")),
        0o755,
    );
    let error = watch.event("error");
    let message = error["update"]["message"].as_str().unwrap();
    assert!(message.starts_with("cannot restart onto "), "{message}");
    let passes = watch.passes();
    watch.wait_passes(passes + 2);
    assert!(watch.child.try_wait().unwrap().is_none());
    assert_eq!(watch.events("error").len(), 1);
    let (code, last) = watch.stop();
    assert_eq!(code, Some(0));
    assert_eq!(last["watch"]["stopped"], true);
}
