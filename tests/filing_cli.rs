mod common;
use common::{mail, Harness};
use mailtriage::{
    domain::{Category, FilingMode},
    service::{Backfill, RetryTarget},
};
use serde_json::Value;
use std::{path::Path, process::Command};

fn run(cwd: &Path, args: &[&str]) -> (i32, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap();
    let v = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    (out.status.code().unwrap(), v)
}

#[test]
fn cli_enable_status_plan_disable_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    assert_eq!(run(d, &["init", "--json"]).0, 0);
    let (code, v) = run(
        d,
        &[
            "filing",
            "enable",
            "--account",
            "work",
            "--mode",
            "dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 2);
    assert!(v["error"]["message"].as_str().unwrap().contains("engine"));
    std::fs::write(
        d.join("h.toml"),
        "[accounts.work]\nimap.server='imaps://x.test'\n",
    )
    .unwrap();
    let mut c: Value =
        serde_json::from_str(&std::fs::read_to_string(d.join("mailtriage.json")).unwrap()).unwrap();
    c["accounts"]["work"]["engine"] = serde_json::json!({"kind":"himalaya","binary":"/nonexistent/himalaya","config":"h.toml","account":"work","mailboxes":["INBOX"],"expected_version":"2.1.0","timeout_seconds":5,"max_output_bytes":100000});
    std::fs::write(d.join("mailtriage.json"), c.to_string()).unwrap();
    let (code, v) = run(
        d,
        &[
            "filing",
            "enable",
            "--account",
            "work",
            "--mode",
            "dry-run",
            "--json",
        ],
    );
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["mode"], "dry_run");
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(d.join("mailtriage.json")).unwrap()).unwrap();
    assert!(saved["accounts"]["work"]["categories"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["folder"].is_string()));
    let (code, v) = run(d, &["filing", "status", "--account", "work", "--json"]);
    assert_eq!((code, v["mode"].clone()), (0, Value::from("dry_run")));
    let (code, v) = run(d, &["filing", "plan", "--account", "work", "--json"]);
    assert_eq!(code, 0);
    assert!(v["folders_to_create"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f == "Newsletters"));
    let (code, _) = run(
        d,
        &[
            "filing",
            "backfill",
            "--account",
            "work",
            "--days",
            "30",
            "--apply",
            "--json",
        ],
    );
    assert_eq!(code, 2, "apply requires live");
    let (code, _) = run(d, &["filing", "backfill", "--account", "work", "--json"]);
    assert_eq!(code, 2, "days or all is required");
    let (code, v) = run(d, &["filing", "log", "--account", "work", "--json"]);
    assert_eq!((code, v["items"].as_array().unwrap().len()), (0, 0));
    let (code, v) = run(d, &["filing", "disable", "--account", "work", "--json"]);
    assert_eq!((code, v["mode"].clone()), (0, Value::from("off")));
}

#[test]
fn rename_keeps_folder_after_enable() {
    let h = Harness::new(FilingMode::Off);
    h.edit(|c| {
        c.accounts
            .get_mut("work")
            .unwrap()
            .categories
            .iter_mut()
            .for_each(|c| c.folder = None)
    });
    h.service().filing_enable("work", FilingMode::Live).unwrap();
    let mut cats: Vec<Category> = h.service().config.accounts["work"].categories.clone();
    for c in cats.iter_mut() {
        if c.id == "newsletters" {
            c.name = "News Digest".into();
        }
        c.folder = None;
    }
    h.service().apply_categories("work", cats).unwrap();
    let news = h.service().config.accounts["work"]
        .categories
        .iter()
        .find(|c| c.id == "newsletters")
        .unwrap()
        .clone();
    assert_eq!(news.effective_folder(), "Newsletters");
    h.sync();
    assert!(h.fake.calls().iter().all(|c| !c.contains("News Digest")));
}

#[test]
fn status_plan_backfill_and_log_report_state() {
    let h = Harness::new(FilingMode::Live);
    h.sync();
    h.fake.deliver_at(
        "INBOX",
        &mail("old", "Old newsletter", "newsletter"),
        "2026-01-01T00:00:00+00:00",
    );
    h.sync();
    let mut s = h.service();
    let plan = s.filing_plan("work", 50).unwrap();
    assert_eq!(plan["total"], 0, "backlog is not planned");
    let b = s.filing_backfill("work", Backfill::All, false).unwrap();
    assert_eq!(b["matched"], 1);
    assert_eq!(
        s.filing_backfill("work", Backfill::All, true).unwrap()["applied"],
        1
    );
    assert_eq!(s.filing_plan("work", 50).unwrap()["total"], 1);
    h.sync();
    assert_eq!(h.fake.locate("<old@test>")[0].0, "Newsletters");
    h.sync(); // the next pass observes the arrival: the move is applied, event `moved`
    let status = h.service().filing_status("work").unwrap();
    assert_eq!(status["mode"], "live");
    assert!(status["folders"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["native"] == "Newsletters" && f["state"] == "ok"));
    let log = h.service().filing_log("work", None, 50).unwrap();
    assert!(log["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "moved"));
}

#[test]
fn unpin_retry_folder_and_adopt() {
    let h = Harness::new(FilingMode::Live);
    h.fake.set_capabilities(true, true, false);
    h.fake.add_folder("Updates", &[]);
    h.sync();
    assert_eq!(
        h.service().filing_status("work").unwrap()["folders"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["native"] == "Updates")
            .unwrap()["state"],
        "needs_confirmation"
    );
    h.service().filing_adopt("work", "Updates").unwrap();
    h.sync();
    let f = h
        .service()
        .store
        .folder_record("work", "Updates")
        .unwrap()
        .unwrap();
    assert_eq!(f.state, "ok");
    let mut rec = f.clone();
    rec.pause_reason = Some("epoch_race".into());
    h.service().store.save_folder(&rec).unwrap();
    h.service()
        .filing_retry("work", RetryTarget::Folder("Updates".into()))
        .unwrap();
    assert!(h
        .service()
        .store
        .folder_record("work", "Updates")
        .unwrap()
        .unwrap()
        .pause_reason
        .is_none());
}

#[test]
fn placement_is_null_with_filing_off_and_doctor_reports_filing() {
    let h = Harness::new(FilingMode::Off);
    h.fake.deliver("INBOX", &mail("a", "Hello", "Plain"));
    h.sync();
    let mut s = h.service();
    let all = s
        .list(
            "work",
            mailtriage::service::ListOptions {
                view: "all".into(),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(all["items"][0]["placement"].is_null());
    h.set_mode(FilingMode::Live);
    let d = h.service().doctor("work").unwrap();
    assert_eq!(d["filing"]["move_supported"], true);
}

// Supplementary coverage: specific folder errors, stale requests, arrival
// commands, backfill scopes, offline reads and the CLI argument groups.

fn code(e: anyhow::Error) -> Option<i32> {
    e.downcast_ref::<mailtriage::service::ServiceError>()
        .map(|s| s.code)
}

fn message(e: anyhow::Error) -> String {
    e.downcast_ref::<mailtriage::service::ServiceError>()
        .map(|s| s.message.clone())
        .unwrap_or_default()
}

fn id_of(h: &Harness, mid: &str) -> String {
    let s = h.service();
    let records = s.store.records("work").unwrap();
    records
        .into_iter()
        .find(|r| {
            s.store
                .message_meta("work", &r.id)
                .unwrap()
                .and_then(|m| m.rfc_message_id)
                .as_deref()
                == Some(&format!("<{mid}@test>"))
        })
        .unwrap()
        .id
}

#[test]
fn folder_errors_name_the_categories_that_need_a_folder() {
    let h = Harness::new(FilingMode::Off);
    h.edit(|c| {
        let a = c.accounts.get_mut("work").unwrap();
        for cat in &mut a.categories {
            cat.folder = None;
            if cat.id == "newsletters" {
                cat.name = "News & Views".into();
            }
        }
    });
    let e = h
        .service()
        .filing_enable("work", FilingMode::DryRun)
        .unwrap_err();
    assert_eq!(
        (code(e), h.service().config.accounts["work"].filing.mode),
        (Some(2), FilingMode::Off),
        "refused before saving"
    );
    let e = h
        .service()
        .filing_enable("work", FilingMode::DryRun)
        .unwrap_err();
    assert_eq!(message(e), "categories need a valid folder: newsletters");

    let h = Harness::new(FilingMode::Live);
    let mut cats = h.service().config.accounts["work"].categories.clone();
    for c in cats.iter_mut() {
        if c.id == "updates" || c.id == "promotions" {
            c.folder = Some("Shared".into());
        }
    }
    let e = h.service().apply_categories("work", cats).unwrap_err();
    assert_eq!(
        message(e),
        "categories need a valid folder: promotions",
        "the second user of a folder is named"
    );
    let first = h.service().filing_enable("work", FilingMode::Live).unwrap();
    let again = h.service().filing_enable("work", FilingMode::Live).unwrap();
    assert_eq!(first, again, "idempotent");
    assert_eq!(first["folders"]["correspondence"], "INBOX");
}

#[test]
fn status_reports_counts_and_stale_requests_without_engine_calls() {
    let h = Harness::new(FilingMode::Live);
    h.sync();
    h.fake.deliver_at(
        "INBOX",
        &mail("old", "Old newsletter", "newsletter"),
        "2026-01-01T00:00:00+00:00",
    );
    h.sync();
    let id = id_of(&h, "old");
    let mut s = h.service();
    let mut p = s.store.placement("work", &id).unwrap().unwrap();
    p.desired_target = Some("removed-category".into());
    s.store.save_placement(&p, None).unwrap();
    // The account binding check reads the engine's identity (Himalaya: its
    // TOML); nothing else may reach the engine.
    let mailbox_calls = || {
        h.fake
            .calls()
            .into_iter()
            .filter(|c| c != "binding_identity")
            .count()
    };
    let calls = mailbox_calls();
    let status = s.filing_status("work").unwrap();
    s.filing_plan("work", 10).unwrap();
    s.filing_backfill("work", Backfill::Days(30), false)
        .unwrap();
    s.filing_log("work", Some(&id), 10).unwrap();
    assert_eq!(mailbox_calls(), calls, "offline commands");
    assert_eq!(status["stale_requests"]["count"], 1);
    assert_eq!(status["stale_requests"]["ids"][0], id.as_str());
    assert_eq!(status["state_mode"], "live");
    assert_eq!(status["bootstrap_done"], true);
    assert_eq!(status["capabilities"]["move_supported"], true);
    assert_eq!(status["eligible_unfiled"], 0, "backlog is not eligible");
    assert_eq!(status["paused_categories"], serde_json::json!([]));
    assert_eq!(status["last_pass"]["mode"], "live");
    for key in ["blocked", "quarantined", "ambiguous", "unresolved_arrivals"] {
        assert_eq!(status[key], 0, "{key}");
    }
    let e = s.filing_plan("work", 0).unwrap_err();
    assert_eq!(code(e), Some(2));
    let e = s.filing_log("work", None, 501).unwrap_err();
    assert_eq!(code(e), Some(2));
}

#[test]
fn backfill_days_uses_the_internal_date_and_apply_is_idempotent() {
    let h = Harness::new(FilingMode::Live);
    h.sync();
    h.fake.deliver_at(
        "INBOX",
        &mail("old", "Old newsletter", "newsletter"),
        "2026-01-01T00:00:00+00:00",
    );
    let recent = (chrono::Utc::now() - chrono::Duration::days(2)).to_rfc3339();
    h.fake.deliver_at(
        "INBOX",
        &mail("recent", "Recent newsletter", "newsletter"),
        &recent,
    );
    h.sync();
    let mut s = h.service();
    let all = s.filing_backfill("work", Backfill::All, false).unwrap();
    assert_eq!(all["matched"], 2);
    let days = s
        .filing_backfill("work", Backfill::Days(30), false)
        .unwrap();
    assert_eq!(days["matched"], 1);
    assert_eq!(days["items"][0], id_of(&h, "recent").as_str());
    assert_eq!(
        s.filing_backfill("work", Backfill::Days(30), true).unwrap()["applied"],
        1
    );
    assert_eq!(
        s.filing_backfill("work", Backfill::Days(30), true).unwrap()["applied"],
        0,
        "already eligible"
    );
    assert_eq!(
        code(
            s.filing_backfill("work", Backfill::Days(0), false)
                .unwrap_err()
        ),
        Some(2)
    );
    let log = s.filing_log("work", None, 50).unwrap();
    assert!(log["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"] == "backfill"));
    h.sync();
    assert_eq!(h.fake.locate("<recent@test>")[0].0, "Newsletters");
    assert_eq!(h.fake.locate("<old@test>")[0].0, "INBOX");
}

#[test]
fn arrival_commands_are_reported_and_lift_their_blocks() {
    let h = Harness::new(FilingMode::DryRun);
    h.fake.add_folder("Updates", &[]);
    h.sync();
    let uid = h
        .fake
        .deliver("INBOX", &mail("d", "Weekly newsletter", "Our newsletter"));
    h.fake.client_copy("INBOX", uid, "Updates");
    let mut s = h.service();
    s.sync("work", 1).unwrap(); // the Updates copy stays provisional
    let all = s
        .list(
            "work",
            mailtriage::service::ListOptions {
                view: "all".into(),
                ..Default::default()
            },
        )
        .unwrap();
    let provisional = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["classification"]["state"] == "pending")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    s.review("work", &provisional, true).unwrap();
    h.sync(); // merge refused: unresolved arrival, canonical blocked
    let status = h.service().filing_status("work").unwrap();
    assert_eq!(status["unresolved_arrivals"], 1);
    assert_eq!(status["blocked"], 1);
    let arrival = status["unresolved_arrival_items"][0]["id"]
        .as_i64()
        .unwrap();
    let e = h
        .service()
        .filing_retry("work", RetryTarget::Arrival(arrival + 1000))
        .unwrap_err();
    assert_eq!(code(e), Some(2), "only an unresolved arrival is retried");
    let out = h.service().filing_dismiss("work", arrival).unwrap();
    assert_eq!(
        (out["ok"].clone(), out["arrival"]["state"].clone()),
        (Value::from(true), Value::from("dismissed"))
    );
    let status = h.service().filing_status("work").unwrap();
    assert_eq!(
        (
            status["unresolved_arrivals"].clone(),
            status["blocked"].clone()
        ),
        (0.into(), 0.into())
    );
    let e = h.service().filing_dismiss("work", arrival).unwrap_err();
    assert_eq!(code(e), Some(2), "already dismissed");
    let e = h
        .service()
        .filing_retry("work", RetryTarget::Arrival(arrival))
        .unwrap_err();
    assert_eq!(code(e), Some(2), "a dismissed arrival stays dismissed");
}

#[test]
fn message_commands_return_the_item() {
    let h = Harness::new(FilingMode::Live);
    h.sync();
    h.fake.deliver_at(
        "INBOX",
        &mail("old", "Old newsletter", "newsletter"),
        "2026-01-01T00:00:00+00:00",
    );
    h.sync();
    let id = id_of(&h, "old");
    let mut s = h.service();
    let pinned = s.filing_pin("work", &id).unwrap();
    assert_eq!(pinned["item"]["placement"]["pinned"], true);
    let unpinned = s.filing_unpin("work", &id).unwrap();
    assert_eq!(unpinned["item"]["placement"]["pinned"], false);
    let retried = s
        .filing_retry("work", RetryTarget::Message(id.clone()))
        .unwrap();
    assert_eq!(retried["item"]["id"], id.as_str());
    h.sync();
    assert_eq!(
        h.fake.locate("<old@test>")[0].0,
        "Newsletters",
        "unpin makes backlog eligible once"
    );
    let e = h.service().filing_adopt("work", "Nope").unwrap_err();
    assert_eq!(code(e), Some(2));
    let e = h
        .service()
        .filing_retry("work", RetryTarget::Folder("Nope".into()))
        .unwrap_err();
    assert_eq!(code(e), Some(2));
}

#[test]
fn filing_off_keeps_doctor_and_plan_quiet() {
    let h = Harness::new(FilingMode::Off);
    h.sync();
    let mut s = h.service();
    assert!(s.doctor("work").unwrap().get("filing").is_none());
    let plan = s.filing_plan("work", 50).unwrap();
    assert_eq!(
        (plan["mode"].clone(), plan["total"].clone()),
        (Value::from("off"), Value::from(0))
    );
    assert_eq!(plan["folders_to_create"], serde_json::json!([]));
    let e = s.filing_backfill("work", Backfill::All, true).unwrap_err();
    assert_eq!(code(e), Some(2));
    h.set_mode(FilingMode::DryRun);
    let d = h.service().doctor("work").unwrap();
    assert_eq!(d["filing"]["mode"], "dry_run");
    assert!(d["filing"]["folders"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["name"] == "INBOX"));
    assert_eq!(d["filing"]["problems"], serde_json::json!([]));
}

#[test]
fn cli_retry_takes_exactly_one_target() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    assert_eq!(run(d, &["init", "--json"]).0, 0);
    let (code, v) = run(
        d,
        &[
            "filing",
            "retry",
            "--account",
            "work",
            "--id",
            "x",
            "--folder",
            "y",
            "--json",
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(v["error"]["code"], 2);
    let (code, _) = run(d, &["filing", "retry", "--account", "work", "--json"]);
    assert_eq!(code, 2);
    let (code, _) = run(
        d,
        &[
            "filing",
            "enable",
            "--account",
            "work",
            "--mode",
            "off",
            "--json",
        ],
    );
    assert_eq!(code, 2, "enable takes dry-run or live");
    let (code, _) = run(
        d,
        &[
            "filing",
            "backfill",
            "--account",
            "work",
            "--days",
            "0",
            "--json",
        ],
    );
    assert_eq!(code, 2, "days is 1..=3650");
    let (code, _) = run(
        d,
        &[
            "filing",
            "backfill",
            "--account",
            "work",
            "--days",
            "3",
            "--all",
            "--json",
        ],
    );
    assert_eq!(code, 2, "days and all are exclusive");
    let (code, v) = run(d, &["filing", "status", "--account", "work", "--json"]);
    assert_eq!((code, v["mode"].clone()), (0, Value::from("off")));
    assert_eq!(v["engine_configured"], false);
}

#[test]
fn cli_enable_and_disable_keep_the_file_as_written() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    assert_eq!(run(d, &["init", "--json"]).0, 0);
    std::fs::write(
        d.join("h.toml"),
        "[accounts.work]\nimap.server='imaps://x.test'\n",
    )
    .unwrap();
    let path = d.join("mailtriage.json");
    let mut c: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    c["accounts"]["work"]["engine"] = serde_json::json!({"kind":"himalaya","binary":"/nonexistent/himalaya","config":"h.toml","account":"work","mailboxes":["INBOX"],"expected_version":"2.1.0","timeout_seconds":5,"max_output_bytes":100000});
    std::fs::write(&path, c.to_string()).unwrap();
    for args in [
        &[
            "filing",
            "enable",
            "--account",
            "work",
            "--mode",
            "live",
            "--json",
        ][..],
        &["filing", "disable", "--account", "work", "--json"][..],
    ] {
        let (code, v) = run(d, args);
        assert_eq!(code, 0, "{v}");
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["state_dir"], ".state");
        assert_eq!(saved["accounts"]["work"]["engine"]["config"], "h.toml");
    }
    let (code, v) = run(d, &["doctor", "--account", "work", "--json"]);
    assert_eq!(
        (code, v.get("filing").is_none()),
        (0, true),
        "filing off: no block"
    );
}

// Fix round 1: ids behind the status counts, `categories validate --account`
// and open move intents.

#[test]
fn status_lists_the_ids_behind_its_counts() {
    let h = Harness::new(FilingMode::Live);
    h.sync();
    for mid in ["a", "b", "c"] {
        h.fake.deliver_at(
            "INBOX",
            &mail(mid, &format!("Old newsletter {mid}"), "newsletter"),
            "2026-01-01T00:00:00+00:00",
        );
    }
    h.sync();
    let (a, b, c) = (id_of(&h, "a"), id_of(&h, "b"), id_of(&h, "c"));
    let mut s = h.service();
    for (id, block) in [
        (&a, Some("move_failed")),
        (&b, Some("quarantined")),
        (&c, None),
    ] {
        let mut p = s.store.placement("work", id).unwrap().unwrap();
        p.blocked_reason = block.map(str::to_string);
        if block.is_none() {
            p.location_state = mailtriage::filing::LocationState::Ambiguous;
        }
        s.store.save_placement(&p, None).unwrap();
    }
    let status = s.filing_status("work").unwrap();
    assert_eq!(
        (
            status["blocked"].clone(),
            status["quarantined"].clone(),
            status["ambiguous"].clone()
        ),
        (1.into(), 1.into(), 1.into())
    );
    assert_eq!(
        status["blocked_ids"],
        serde_json::json!([{"id": a, "blocked_reason": "move_failed"}])
    );
    assert_eq!(status["quarantined_ids"], serde_json::json!([b]));
    assert_eq!(status["ambiguous_ids"], serde_json::json!([c]));
}

#[test]
fn a_moved_but_unobserved_message_is_neither_backfilled_nor_eligible() {
    let h = Harness::new(FilingMode::Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.sync(); // moved; the intent stays `sent` until the next pass sees the arrival
    assert_eq!(h.fake.locate("<n@test>")[0].0, "Newsletters");
    let id = id_of(&h, "n");
    let mut s = h.service();
    assert!(s
        .store
        .intents("work", true)
        .unwrap()
        .iter()
        .any(|i| i.message_id == id && i.kind == "move"));
    let b = s.filing_backfill("work", Backfill::All, false).unwrap();
    assert_eq!(b["matched"], 0, "{b}");
    assert_eq!(s.filing_status("work").unwrap()["eligible_unfiled"], 0);
}

#[test]
fn cli_categories_validate_with_an_account_applies_the_folder_rules() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    assert_eq!(run(d, &["init", "--json"]).0, 0);
    std::fs::write(
        d.join("h.toml"),
        "[accounts.work]\nimap.server='imaps://x.test'\n",
    )
    .unwrap();
    let path = d.join("mailtriage.json");
    let mut c: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    c["accounts"]["work"]["engine"] = serde_json::json!({"kind":"himalaya","binary":"/nonexistent/himalaya","config":"h.toml","account":"work","mailboxes":["INBOX"],"expected_version":"2.1.0","timeout_seconds":5,"max_output_bytes":100000});
    std::fs::write(&path, c.to_string()).unwrap();
    let validate = |file: &str, account: bool| {
        let mut args = vec!["categories", "validate", "--file", file, "--json"];
        if account {
            args.extend(["--account", "work"]);
        }
        run(d, &args)
    };
    let mut cats = c["accounts"]["work"]["categories"].clone();
    for cat in cats.as_array_mut().unwrap() {
        if cat["id"] == "newsletters" {
            cat["folder"] = "A&B".into();
        }
    }
    std::fs::write(d.join("bad.json"), cats.to_string()).unwrap();
    let mut renamed = c["accounts"]["work"]["categories"].clone();
    for cat in renamed.as_array_mut().unwrap() {
        if cat["id"] == "newsletters" {
            cat["name"] = "News & Views".into();
        }
    }
    std::fs::write(d.join("renamed.json"), renamed.to_string()).unwrap();

    // Filing off: folders are not checked, with or without the account.
    assert_eq!(validate("bad.json", false).0, 0);
    assert_eq!(validate("bad.json", true).0, 0);
    assert_eq!(
        run(
            d,
            &[
                "filing",
                "enable",
                "--account",
                "work",
                "--mode",
                "dry-run",
                "--json"
            ]
        )
        .0,
        0
    );
    let (code, v) = validate("bad.json", false);
    assert_eq!(code, 0, "without --account the rules stay as before: {v}");
    let (code, v) = validate("bad.json", true);
    assert_eq!(code, 2);
    assert_eq!(
        v["error"]["message"],
        "categories need a valid folder: newsletters"
    );
    let (code, v) = validate("renamed.json", true);
    assert_eq!(
        (code, v["valid"].clone()),
        (0, Value::from(true)),
        "a renamed category keeps its folder: {v}"
    );
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        saved["accounts"]["work"]["categories"],
        c["accounts"]["work"]["categories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|cat| {
                let mut cat = cat.clone();
                cat["folder"] = cat["name"].clone();
                cat
            })
            .collect::<Value>(),
        "validate writes nothing"
    );
}

/// Final review M1: `filing enable` names a configured source mailbox that
/// filing cannot write to safely.
#[test]
fn enable_names_unsafe_source_mailboxes() {
    use mailtriage::domain::EngineConfig;
    let h = Harness::new(FilingMode::Off);
    h.edit(|c| {
        let Some(EngineConfig::Himalaya(e)) = &mut c.accounts.get_mut("work").unwrap().engine
        else {
            panic!("harness has a Himalaya engine config");
        };
        e.mailboxes.push("Lists\\Work".into());
    });
    let e = h
        .service()
        .filing_enable("work", FilingMode::DryRun)
        .unwrap_err();
    assert_eq!(
        message(e),
        "source mailboxes are not safe to file from: \"Lists\\\\Work\"; use printable ASCII without \\, \" or & and no leading -"
    );
    assert_eq!(
        h.service().config.accounts["work"].filing.mode,
        FilingMode::Off
    );
}
