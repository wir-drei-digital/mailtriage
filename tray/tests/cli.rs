//! The `cli` module against a fake `mailtriage`, and path resolution.
mod support;
use mailtriage_tray::{
    cli::{self, Cli, Ending, FilingMode, Request},
    paths::{self, Env, Problem},
};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use support::{write_script, FakeCli};

fn cli(fake: &FakeCli) -> Cli {
    Cli {
        program: fake.program.clone(),
        config: Some(fake.config()),
    }
}

#[test]
fn every_request_is_its_documented_command_with_the_config_last() {
    let c = Cli {
        program: PathBuf::from("/bin/mailtriage"),
        config: Some(PathBuf::from("/home/a b/mailtriage.json")),
    };
    let args = |r: Request| c.invocation(&r).args.join(" ");
    let tail = "--config /home/a b/mailtriage.json";
    assert_eq!(args(Request::Version), "--version");
    assert_eq!(
        args(Request::Status),
        format!("service status --json {tail}")
    );
    assert_eq!(
        args(Request::Start("d".into())),
        format!("service start --account d --json {tail}")
    );
    assert_eq!(
        args(Request::Stop("d".into())),
        format!("service stop --account d --json {tail}")
    );
    assert_eq!(
        args(Request::Install("d".into())),
        format!("service install --account d --json {tail}")
    );
    assert_eq!(
        args(Request::Export("d".into())),
        format!("categories export --account d --json {tail}")
    );
    assert_eq!(
        args(Request::Validate {
            account: "d".into(),
            file: "/t/draft-3.json".into()
        }),
        format!("categories validate --account d --file /t/draft-3.json --json {tail}")
    );
    assert_eq!(
        args(Request::Apply {
            account: "d".into(),
            file: "/t/draft-3.json".into(),
            digest: "v1:ab".into()
        }),
        format!(
            "categories apply --account d --file /t/draft-3.json --expect-digest v1:ab --json {tail}"
        )
    );
    assert_eq!(
        args(Request::Refile {
            account: "d".into(),
            folder: Some("INBOX.Promotions".into()),
            apply: true
        }),
        format!("filing refile --account d --folder INBOX.Promotions --apply --json {tail}")
    );
    assert_eq!(
        args(Request::Refile {
            account: "d".into(),
            folder: None,
            apply: false
        }),
        format!("filing refile --account d --json {tail}")
    );
    assert_eq!(
        c.invocation(&Request::Status).line(),
        "/bin/mailtriage service status --json --config '/home/a b/mailtriage.json'"
    );
}

#[test]
fn status_is_read_with_the_fields_of_later_releases() {
    let fake = FakeCli::new();
    fake.respond_fixture("service-status", "status.json");
    let status = cli::status(&cli(&fake).run(&Request::Status)).unwrap();
    assert_eq!(status.config, fake.config());
    let daniel = &status.services[0];
    assert_eq!(daniel.account, "daniel");
    assert_eq!(daniel.filing_mode, FilingMode::Live);
    assert_eq!(daniel.config_matches, Some(true));
    let pass = daniel.last_pass.as_ref().unwrap();
    assert_eq!(
        (pass.exit_code, pass.version.as_deref()),
        (4, Some("0.3.0"))
    );
    assert_eq!(
        daniel.update.as_ref().unwrap().installed.as_deref(),
        Some("0.3.0")
    );
    assert_eq!(status.services[1].last_pass, None);
    assert_eq!(status.services[1].interval_seconds, None);
}

#[test]
fn a_cli_without_the_new_fields_asks_for_an_update() {
    let fake = FakeCli::new();
    fake.respond_fixture("service-status", "status-old.json");
    let failure = cli::status(&cli(&fake).run(&Request::Status)).unwrap_err();
    assert_eq!(failure.message, cli::OUTDATED);
    assert!(failure
        .details
        .output
        .starts_with("missing field /services/0/config_matches"));
}

/// mailtriage's own parser error with `--json`, as `src/cli.rs` prints it:
/// clap's text (it starts with `error: `) as the message, code and exit 2.
/// This is what the CLI of 34b4213 answers to `service status --json`.
const OLD_STATUS_USAGE: &str = "error: the following required arguments were not provided:\n  --account <ACCOUNT>\n\nUsage: mailtriage service status --account <ACCOUNT> --json\n\nFor more information, try '--help'.";

/// An older CLI that rejects the tray's arguments asks for an update, with
/// its parser's text in the details; a code-2 error of mailtriage's own
/// (a bad value, an unknown account) keeps its message.
#[test]
fn a_cli_that_rejects_the_arguments_asks_for_an_update() {
    let fake = FakeCli::new();
    fake.fail("service-status", 2, OLD_STATUS_USAGE, None);
    let failure = cli::status(&cli(&fake).run(&Request::Status)).unwrap_err();
    assert_eq!(failure.message, cli::OUTDATED);
    assert_eq!(failure.details.exit_code, Some(2));
    assert!(
        failure.details.output.starts_with(OLD_STATUS_USAGE),
        "{}",
        failure.details.output
    );
    // The tray's summary and the window's first load come from here.
    let Err(Problem::Status(failure)) =
        paths::resolve(Some(&fake.program), None, &env_in(fake.dir.path()))
    else {
        panic!("a status problem")
    };
    assert_eq!(failure.message, cli::OUTDATED);
    fake.fail(
        "categories-export",
        2,
        "error: unrecognized subcommand 'categories'\n\nUsage: mailtriage [OPTIONS] <COMMAND>\n\nFor more information, try '--help'.",
        None,
    );
    let failure = cli::categories(&cli(&fake).run(&Request::Export("daniel".into()))).unwrap_err();
    assert_eq!(failure.message, cli::OUTDATED);
    // mailtriage's own input errors are not parser errors.
    fake.fail("categories-export", 2, "unknown account", None);
    let failure = cli::categories(&cli(&fake).run(&Request::Export("daniel".into()))).unwrap_err();
    assert_eq!(failure.message, "unknown account");
    fake.fail(
        "service-status",
        2,
        "configuration not found; run `mailtriage setup` or pass --config",
        None,
    );
    assert_eq!(
        paths::resolve(Some(&fake.program), None, &env_in(fake.dir.path())),
        Err(Problem::NotSetUp(None))
    );
}

#[test]
fn json_errors_keep_their_reason_and_details() {
    let fake = FakeCli::new();
    fake.fail(
        "categories-apply",
        5,
        "categories changed since export; export again",
        Some("categories_changed"),
    );
    let finished = cli(&fake).run(&Request::Apply {
        account: "daniel".into(),
        file: "/t/draft-1.json".into(),
        digest: "v1:aa".into(),
    });
    let failure = cli::done(&finished).unwrap_err();
    assert_eq!(failure.code, Some(5));
    assert!(failure.has_reason("categories_changed"));
    let text = failure.details.text();
    assert!(text.starts_with(&format!(
        "$ {} categories apply --account daniel",
        fake.program.display()
    )));
    assert!(text.contains("exit code: 5\n"));
    assert!(text.contains("\"reason\":\"categories_changed\""));
}

#[test]
fn failures_without_json() {
    let fake = FakeCli::new();
    fake.respond("categories-export", "not json");
    let failure = cli::categories(&cli(&fake).run(&Request::Export("d".into()))).unwrap_err();
    assert_eq!(
        failure.message,
        "mailtriage gave an answer the tray cannot read"
    );

    let missing = Cli {
        program: fake.dir.path().join("nothing-here"),
        config: None,
    };
    let finished = missing.run(&Request::Status);
    assert!(matches!(finished.ending, Ending::CouldNotStart(_)));
    assert!(cli::done(&finished)
        .unwrap_err()
        .message
        .starts_with("mailtriage could not start"));

    fake.hold("service-status");
    let finished = cli::run(
        &cli(&fake).invocation(&Request::Status),
        Duration::from_millis(300),
    );
    fake.release("service-status");
    assert_eq!(
        finished.ending,
        Ending::TimedOut(Duration::from_millis(300))
    );
    assert_eq!(
        cli::done(&finished).unwrap_err().message,
        "mailtriage took too long to answer"
    );
}

#[test]
fn version_and_typed_results() {
    let fake = FakeCli::new();
    assert_eq!(
        cli::version(&cli(&fake).run(&Request::Version)).as_deref(),
        Some("0.1.0")
    );
    fake.respond_fixture("categories-export", "export-daniel.json");
    let exported = cli::categories(&cli(&fake).run(&Request::Export("daniel".into()))).unwrap();
    assert_eq!(exported.digest, "v1:aaaa");
    assert_eq!(exported.categories.len(), 4);
    fake.respond_fixture("filing-refile", "refile-preview.json");
    let preview = cli::refile_preview(&cli(&fake).run(&Request::Refile {
        account: "daniel".into(),
        folder: None,
        apply: false,
    }))
    .unwrap();
    assert_eq!((preview.total, preview.waiting), (38, 15));
    assert_eq!(preview.folders[1].name(), "Promotions");
    assert_eq!(preview.skipped["corrected"], 3);
    fake.respond_fixture("filing-refile-apply", "refile-marked.json");
    let marked = cli::refile_marked(&cli(&fake).run(&Request::Refile {
        account: "daniel".into(),
        folder: None,
        apply: true,
    }))
    .unwrap();
    assert_eq!((marked.marked, marked.waiting_marked), (38, 0));
}

fn env_in(cwd: &Path) -> Env {
    Env {
        cwd: cwd.to_path_buf(),
        ..Env::default()
    }
}

#[test]
fn the_cli_is_the_flag_then_next_to_the_tray_then_on_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    for d in ["app", "bin", "flag"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    let mut env = env_in(&root);
    env.own_exe = Some(root.join("app/mailtriage-tray"));
    env.path = Some(root.join("bin").into_os_string());
    assert_eq!(
        paths::resolve_cli(None, &env),
        Err(vec![
            root.join("app/mailtriage").display().to_string(),
            "PATH".into()
        ])
    );
    write_script(&root.join("bin/mailtriage"), "#!/bin/sh\n");
    assert_eq!(
        paths::resolve_cli(None, &env),
        Ok(root.join("bin/mailtriage"))
    );
    write_script(&root.join("app/mailtriage"), "#!/bin/sh\n");
    assert_eq!(
        paths::resolve_cli(None, &env),
        Ok(root.join("app/mailtriage"))
    );
    write_script(&root.join("flag/mt"), "#!/bin/sh\n");
    assert_eq!(
        paths::resolve_cli(Some(Path::new("flag/mt")), &env),
        Ok(root.join("flag/mt"))
    );
    assert_eq!(
        paths::resolve_cli(Some(Path::new("flag/none")), &env),
        Err(vec!["flag/none".into()])
    );
}

/// A config given as a symlink is resolved once; retargeting the symlink
/// later does not change the commands' `--config`.
#[test]
fn a_symlinked_config_is_canonicalized_once() {
    let fake = FakeCli::new();
    let root = fs::canonicalize(fake.dir.path()).unwrap();
    fs::write(root.join("a.json"), "{}").unwrap();
    fs::write(root.join("b.json"), "{}").unwrap();
    std::os::unix::fs::symlink(root.join("a.json"), root.join("link.json")).unwrap();
    let resolved = paths::resolve(
        Some(&fake.program),
        Some(Path::new("link.json")),
        &env_in(&root),
    )
    .unwrap();
    assert_eq!(resolved.config, root.join("a.json"));
    fs::remove_file(root.join("link.json")).unwrap();
    std::os::unix::fs::symlink(root.join("b.json"), root.join("link.json")).unwrap();
    let args = resolved.cli().invocation(&Request::Status).args;
    assert_eq!(
        args[args.len() - 1],
        root.join("a.json").display().to_string()
    );
}

#[test]
fn the_config_comes_from_the_first_status_when_not_given() {
    let fake = FakeCli::new();
    fake.respond_fixture("service-status", "status.json");
    let resolved = paths::resolve(Some(&fake.program), None, &env_in(fake.dir.path())).unwrap();
    assert_eq!(resolved.config, fake.config());
    // That first status names no config; every later command does.
    assert_eq!(fake.calls(), vec![vec!["service", "status", "--json"]]);

    fake.fail(
        "service-status",
        2,
        "configuration not found; run `mailtriage setup` or pass --config",
        None,
    );
    assert_eq!(
        paths::resolve(Some(&fake.program), None, &env_in(fake.dir.path())),
        Err(Problem::NotSetUp(None))
    );
    let missing = paths::resolve(
        Some(&fake.program),
        Some(Path::new("nope.json")),
        &env_in(fake.dir.path()),
    )
    .unwrap_err();
    assert_eq!(
        missing,
        Problem::NotSetUp(Some(fake.dir.path().join("nope.json")))
    );
    // `mailtriage setup` would write the default config, not this one:
    // the text names the file and the command that creates it.
    assert_eq!(
        missing.text(),
        format!(
            "Not set up: no config at {path}. Run `mailtriage setup --config {path}` in a terminal.",
            path = fake.dir.path().join("nope.json").display()
        )
    );
    assert_eq!(
        Problem::NotSetUp(None).text(),
        "Not set up. Run `mailtriage setup` in a terminal."
    );
    // A path a shell would split is quoted in the command.
    assert_eq!(
        Problem::NotSetUp(Some(PathBuf::from("/home/a b/mailtriage.json"))).text(),
        "Not set up: no config at /home/a b/mailtriage.json. Run `mailtriage setup --config '/home/a b/mailtriage.json'` in a terminal."
    );
}

#[test]
fn the_cache_directory_follows_the_update_rules() {
    let home = Some(std::ffi::OsStr::new("/home/a"));
    let xdg = Some(std::ffi::OsStr::new("/xdg"));
    let relative = Some(std::ffi::OsStr::new("xdg"));
    if cfg!(target_os = "macos") {
        assert_eq!(
            paths::cache_dir(home, xdg),
            Some(PathBuf::from("/home/a/Library/Caches/mailtriage"))
        );
    } else {
        assert_eq!(
            paths::cache_dir(home, xdg),
            Some(PathBuf::from("/xdg/mailtriage"))
        );
        assert_eq!(
            paths::cache_dir(home, relative),
            Some(PathBuf::from("/home/a/.cache/mailtriage"))
        );
    }
    assert_eq!(paths::cache_dir(None, xdg), None);
}

#[test]
fn the_tray_version_is_the_workspace_version() {
    let root =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../Cargo.toml")).unwrap();
    let version = root
        .lines()
        .find_map(|l| l.strip_prefix("version = \""))
        .and_then(|v| v.strip_suffix('"'))
        .unwrap();
    assert_eq!(version, env!("CARGO_PKG_VERSION"));
}
