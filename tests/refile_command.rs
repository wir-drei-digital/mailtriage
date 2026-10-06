//! Refile spec "Command" (`--apply`), "Errors and exit codes" and the CLI.
mod common;
mod refile_support;
use common::{mail, Harness};
use mailtriage::domain::FilingMode::{DryRun, Live};
use refile_support::{
    add_updates, apply, code, deliver_filed, events, filed_in_other_now_updates, id_of, located,
    moves_to, opts, placement, preview, without,
};
use serde_json::{json, Value};
use std::{path::Path, process::Command};

#[test]
fn apply_marks_and_the_next_passes_move_the_mail() {
    let h = Harness::new(Live);
    let id = filed_in_other_now_updates(&h);
    assert_eq!(
        apply(&h, None, None),
        json!({"schema_version": 1, "account": "work", "marked": 1, "waiting_marked": 0})
    );
    assert!(placement(&h, &id).refile_once);
    assert_eq!(events(&h, "refile_marked")[0]["detail"]["marked"], 1);
    h.sync();
    h.sync();
    assert_eq!(located(&h, "u").0, "Updates");
    assert!(!placement(&h, &id).refile_once);
    assert_eq!(events(&h, "moved")[0]["detail"]["reason"], "refile");
    assert_eq!(preview(&h, None, None)["total"], 0, "in place now");
}

#[test]
fn the_limit_shortens_the_list_and_apply_marks_the_whole_set_once() {
    let h = Harness::new(Live);
    h.edit(|c| {
        c.accounts
            .get_mut("work")
            .unwrap()
            .filing
            .max_actions_per_pass = 1
    });
    without(&h, &["updates"]);
    h.sync();
    for i in 0..3 {
        h.fake.deliver(
            "INBOX",
            &mail(
                &format!("u{i}"),
                &format!("System update {i}"),
                "Version 2 is out",
            ),
        );
    }
    for _ in 0..5 {
        h.sync(); // one move per pass
    }
    assert!((0..3).all(|i| located(&h, &format!("u{i}")).0 == "Other"));
    add_updates(&h);
    let mut o = opts(None, None);
    o.limit = 1;
    let v = h.service().filing_refile("work", o).unwrap();
    assert_eq!(
        (
            v["candidates"].as_array().unwrap().len(),
            v["total"].clone()
        ),
        (1, json!(3))
    );
    assert_eq!(
        apply(&h, None, None)["marked"],
        3,
        "--apply marks the whole set"
    );
    assert_eq!(
        apply(&h, None, None)["marked"],
        0,
        "applying again marks nothing new"
    );
    let mut moved = vec![];
    for _ in 0..3 {
        h.sync();
        moved.push(moves_to(&h, "Updates"));
    }
    assert_eq!(
        moved,
        vec![1, 2, 3],
        "max_actions_per_pass spreads the moves"
    );
}

#[test]
fn category_never_marks_waiting_mail_and_folder_does() {
    let h = Harness::new(Live);
    without(&h, &["updates", "promotions"]);
    h.sync();
    let u = deliver_filed(&h, "u", "System update", "Version 2 is out"); // will be `updates`
    let s = deliver_filed(&h, "s", "Big sale", "Everything must go"); // stays `other`
    assert_eq!(
        (located(&h, "u").0, located(&h, "s").0),
        ("Other".into(), "Other".into())
    );
    refile_support::add_category(&h, refile_support::updates_category()); // both wait
    assert_eq!(preview(&h, None, Some("Other"))["waiting"], 2);
    assert_eq!(
        apply(&h, Some("updates"), None),
        json!({"schema_version": 1, "account": "work", "marked": 0, "waiting_marked": 0})
    );
    assert_eq!(
        apply(&h, None, Some("Other")),
        json!({"schema_version": 1, "account": "work", "marked": 0, "waiting_marked": 2})
    );
    h.sync();
    h.sync();
    assert_eq!(located(&h, "u").0, "Updates", "moved once classified");
    assert_eq!(located(&h, "s").0, "Other");
    assert!(!placement(&h, &u).refile_once);
    assert!(
        !placement(&h, &s).refile_once,
        "classified into its own folder: cleared"
    );
    let cleared: Vec<Value> = events(&h, "refile_cleared")
        .into_iter()
        .filter(|e| e["message_id"] == s.as_str())
        .collect();
    assert_eq!(cleared[0]["detail"]["reason"], "in_place");
    assert_eq!(moves_to(&h, "Updates"), 1);
}

#[test]
fn copies_are_skipped_and_never_moved() {
    let h = Harness::new(Live);
    without(&h, &["updates"]);
    h.sync();
    deliver_filed(&h, "a", "System update", "Version 2 is out");
    deliver_filed(&h, "b", "Server update", "Version 3 is out");
    let (folder, uid) = located(&h, "a");
    h.fake.client_copy(&folder, uid, "Transactions"); // copies in two category folders
    add_updates(&h);
    let (folder, uid) = located(&h, "b");
    h.fake.client_copy(&folder, uid, "Updates"); // a copy already in the target
    h.sync();
    let v = preview(&h, None, None);
    assert_eq!(
        (v["total"].clone(), v["skipped"]["multiple_copies"].clone()),
        (json!(0), json!(2))
    );
    assert_eq!(apply(&h, None, None)["marked"], 0);
    h.sync();
    h.sync();
    assert_eq!(moves_to(&h, "Updates"), 0);
}

#[test]
fn apply_needs_live_and_marks_nothing_otherwise() {
    let h = Harness::new(Live);
    let id = filed_in_other_now_updates(&h);
    h.set_mode(DryRun);
    let e = h
        .service()
        .filing_refile_apply("work", opts(None, None))
        .unwrap_err();
    assert_eq!(code(&e), Some(2));
    assert_eq!(e.to_string(), "refile --apply requires filing mode live");
    assert!(!placement(&h, &id).refile_once);
    assert_eq!(
        preview(&h, None, None)["total"],
        1,
        "the preview works in dry_run"
    );
}

#[test]
fn a_configuration_change_during_apply_exits_5() {
    let h = Harness::new(Live);
    let id = filed_in_other_now_updates(&h);
    let mut s = h.service();
    h.edit(|c| c.accounts.get_mut("work").unwrap().brief = "changed".into());
    let e = s.filing_refile_apply("work", opts(None, None)).unwrap_err();
    assert_eq!(code(&e), Some(5));
    let kind = e
        .downcast_ref::<mailtriage::service::ServiceError>()
        .unwrap()
        .kind;
    assert_eq!(kind.reason(), Some("config_changed"));
    assert!(!placement(&h, &id).refile_once);
}

#[test]
fn filed_mail_made_actionable_is_flagged_and_the_refile_writes_no_flag() {
    let h = Harness::new(Live);
    let id = filed_in_other_now_updates(&h);
    // The fake classifier's action decision depends on the text only, so the
    // decision changes through a correction of `action_required`.
    h.service()
        .correct("work", &id, json!({"action_required": true}), None)
        .unwrap();
    apply(&h, None, None);
    h.sync();
    h.sync();
    let flags: Vec<String> = h
        .fake
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("flag "))
        .collect();
    assert_eq!(flags.len(), 1, "{flags:?}");
    assert!(
        flags[0].starts_with("flag Other "),
        "flagged where it was, by the flag rule"
    );
    let (folder, uid) = located(&h, "u");
    assert_eq!(folder, "Updates");
    assert!(h
        .fake
        .flags(&folder, uid)
        .contains(&"\\Flagged".to_string()));
    assert_eq!(id_of(&h, "u"), id);
}

fn run(cwd: &Path, args: &[&str]) -> (i32, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
        .current_dir(cwd)
        .args(args)
        .env_remove("MAILTRIAGE_CONFIG")
        .output()
        .unwrap();
    let v = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    (out.status.code().unwrap(), v)
}

/// `init` plus a Himalaya engine that is never started: the refile commands
/// make no mailbox calls.
fn offline_account(d: &Path) {
    assert_eq!(run(d, &["init", "--json"]).0, 0);
    std::fs::write(
        d.join("h.toml"),
        "[accounts.work]\nimap.server='imaps://x.test'\n",
    )
    .unwrap();
    let path = d.join("mailtriage.json");
    let mut c: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    c["accounts"]["work"]["engine"] = json!({"kind":"himalaya","binary":"/nonexistent/himalaya","config":"h.toml","account":"work","mailboxes":["INBOX"],"expected_version":"2.1.0","timeout_seconds":5,"max_output_bytes":100000});
    std::fs::write(&path, c.to_string()).unwrap();
}

fn no_skips() -> Value {
    let keys = [
        "not_filed_by_mailtriage",
        "corrected",
        "pinned",
        "blocked",
        "done",
        "open_intent",
        "explicit_target",
        "multiple_copies",
        "incomplete_input",
        "retired_frozen",
        "target_unusable",
        "target_inbox_or_source",
    ];
    Value::Object(keys.iter().map(|k| (k.to_string(), json!(0))).collect())
}

#[test]
fn cli_refile_prints_the_preview_and_apply_shapes_and_exit_codes() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    offline_account(d);
    let enable = |mode| {
        run(
            d,
            &[
                "filing",
                "enable",
                "--account",
                "work",
                "--mode",
                mode,
                "--json",
            ],
        )
        .0
    };
    assert_eq!(enable("dry-run"), 0);
    let (code, v) = run(d, &["filing", "refile", "--account", "work", "--json"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(
        v,
        json!({"schema_version": 1, "account": "work", "mode": "dry_run", "candidates": [], "total": 0, "folders": [], "waiting": 0, "skipped": no_skips()})
    );
    let (code, v) = run(
        d,
        &["filing", "refile", "--account", "work", "--apply", "--json"],
    );
    assert_eq!(
        (code, v["error"]["message"].clone()),
        (2, json!("refile --apply requires filing mode live"))
    );
    for extra in [
        &["--category", "nope"][..],
        &["--folder", "Nope"][..],
        &["--folder", "INBOX"][..],
        &["--limit", "0"][..],
        &["--limit", "501"][..],
    ] {
        let mut args = vec!["filing", "refile", "--account", "work", "--json"];
        args.extend_from_slice(extra);
        assert_eq!(run(d, &args).0, 2, "{extra:?}");
    }
    assert_eq!(
        run(d, &["filing", "refile", "--account", "nope", "--json"]).0,
        2
    );
    assert_eq!(enable("live"), 0);
    let (code, v) = run(
        d,
        &["filing", "refile", "--account", "work", "--apply", "--json"],
    );
    assert_eq!(
        (code, v),
        (
            0,
            json!({"schema_version": 1, "account": "work", "marked": 0, "waiting_marked": 0})
        )
    );
}

#[test]
fn cli_refile_reports_an_unavailable_state_database_with_exit_3() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    offline_account(d);
    std::fs::create_dir_all(d.join(".state").join("mailtriage.sqlite")).unwrap();
    assert_eq!(
        run(d, &["filing", "refile", "--account", "work", "--json"]).0,
        3
    );
}
