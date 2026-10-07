#![cfg(unix)]
//! The tray's fixtures (`tray/tests/fixtures/*.json`) against what this CLI
//! really prints. The tray does not depend on this crate, so its fixtures
//! and its parser were written from the specs; here every key path a fixture
//! holds must exist in a real output of the same command with the same JSON
//! type, and `null` only where the real output can be null. In a path `[]`
//! stands for every array element, so a fixture element is compared with
//! the shapes of all real elements.
//!
//! The real outputs come from the commands themselves: `filing refile` and
//! `categories export`/`validate` in-process over the fake engine (the CLI
//! prints these values unchanged), and `service status --json` through a
//! sandbox copy of the binary with its own HOME and XDG_CACHE_HOME, fake
//! `launchctl`/`systemctl` first on PATH and the loopback release server as
//! GitHub. Nothing reads or writes the real HOME, cache or service manager,
//! and nothing depends on a built `mailtriage-tray`.
mod common;
mod refile_support;
mod update_support;
use common::{write_tool, Harness, LAUNCHCTL, SYSTEMCTL};
use mailtriage::{
    domain::{Category, FilingMode::Live},
    system_service::{self, Manager, Unit},
};
use refile_support::{apply, deliver_filed, filed_in_other_now_updates, preview, remove_category};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::Command,
};
use update_support::{cache_dir, run, Sandbox, Server};

/// The command whose output a fixture stands for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Output {
    /// `categories export --account A --json`
    Export,
    /// `categories validate --account A --file F --json`
    Validate,
    /// `filing refile --account A [--folder NATIVE] --json`
    Preview,
    /// `filing refile --account A [--folder NATIVE] --apply --json`
    Marked,
    /// `service status --json` (every account)
    Status,
}

/// Every fixture of the tray, with the command it stands for.
const FIXTURES: [(&str, Output); 9] = [
    ("export-daniel.json", Output::Export),
    ("export-info.json", Output::Export),
    ("validate-ok.json", Output::Validate),
    ("validate-reclassify.json", Output::Validate),
    ("refile-preview.json", Output::Preview),
    ("refile-marked.json", Output::Marked),
    ("refile-marked-folder.json", Output::Marked),
    ("status.json", Output::Status),
    ("status-old.json", Output::Status),
];

fn fixture_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tray/tests/fixtures")
}

/// The JSON types seen at each key path.
type Shapes = BTreeMap<String, BTreeSet<&'static str>>;

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn collect(value: &Value, path: &str, shapes: &mut Shapes) {
    shapes
        .entry(if path.is_empty() { "/" } else { path }.to_owned())
        .or_default()
        .insert(kind(value));
    match value {
        Value::Array(items) => {
            for item in items {
                collect(item, &format!("{path}/[]"), shapes);
            }
        }
        Value::Object(fields) => {
            for (key, item) in fields {
                collect(item, &format!("{path}/{key}"), shapes);
            }
        }
        _ => {}
    }
}

fn shapes(values: &[Value]) -> Shapes {
    let mut shapes = Shapes::new();
    for value in values {
        collect(value, "", &mut shapes);
    }
    shapes
}

/// Whether `path` lies inside an array that every real output left empty,
/// such as `log_paths` on systemd: an empty array says nothing about the
/// shape of its items.
fn inside_empty_array(path: &str, real: &Shapes) -> bool {
    let mut prefix = String::new();
    for segment in path.split('/').skip(1) {
        let parent = if prefix.is_empty() {
            "/".to_owned()
        } else {
            prefix.clone()
        };
        prefix.push('/');
        prefix.push_str(segment);
        if segment == "[]"
            && !real.contains_key(&prefix)
            && real
                .get(&parent)
                .is_some_and(|kinds| kinds.contains("array"))
        {
            return true;
        }
    }
    false
}

/// Each key path and type of `fixture` that no real output has.
fn drift(fixture: &Value, real: &Shapes) -> Vec<String> {
    let mut drift = vec![];
    for (path, kinds) in shapes(std::slice::from_ref(fixture)) {
        if inside_empty_array(&path, real) {
            continue;
        }
        let seen = real.get(&path).cloned().unwrap_or_default();
        for k in kinds.difference(&seen) {
            drift.push(format!("{path}: {k} (real: {seen:?})"));
        }
    }
    drift
}

/// Checks every fixture that stands for `output` against `real`.
fn check(output: Output, real: &[Value]) {
    let real = shapes(real);
    let mut checked = 0;
    for (name, _) in FIXTURES.iter().filter(|(_, o)| *o == output) {
        let text = fs::read_to_string(fixture_dir().join(name)).unwrap();
        let fixture: Value =
            serde_json::from_str(&text.replace("@CONFIG@", "/abs/mailtriage.json")).unwrap();
        let drift = drift(&fixture, &real);
        assert!(
            drift.is_empty(),
            "{name} differs from {output:?}: {drift:#?}"
        );
        checked += 1;
    }
    assert!(checked > 0, "no fixture for {output:?}");
}

#[test]
fn every_tray_fixture_is_checked() {
    let mut found: Vec<String> = fs::read_dir(fixture_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".json"))
        .collect();
    found.sort();
    let mut listed: Vec<String> = FIXTURES.iter().map(|(n, _)| n.to_string()).collect();
    listed.sort();
    assert_eq!(found, listed, "add a new fixture to FIXTURES");
}

/// Preview and marks with and without `--folder`: mail waiting in a retired
/// folder, and mail that can move now from a folder still in use.
#[test]
fn refile_fixtures_match_the_real_preview_and_marks() {
    let waiting = Harness::new(Live);
    waiting.sync();
    deliver_filed(&waiting, "n", "Weekly newsletter", "Our newsletter");
    remove_category(&waiting, "newsletters");
    let moving = Harness::new(Live);
    filed_in_other_now_updates(&moving);
    let previews = [
        preview(&waiting, None, None),
        preview(&waiting, None, Some("Newsletters")),
        preview(&moving, None, None),
    ];
    assert_eq!(
        previews[1]["folders"][0]["retired"], true,
        "{}",
        previews[1]
    );
    assert_eq!(previews[1]["waiting"], 1, "{}", previews[1]);
    assert_eq!(
        previews[2]["folders"][0]["candidates"], 1,
        "{}",
        previews[2]
    );
    check(Output::Preview, &previews);
    let marks = [
        apply(&waiting, None, Some("Newsletters")),
        apply(&moving, None, None),
    ];
    assert_eq!(
        marks,
        [
            json!({"schema_version": 1, "account": "work", "marked": 0, "waiting_marked": 1}),
            json!({"schema_version": 1, "account": "work", "marked": 1, "waiting_marked": 0}),
        ]
    );
    check(Output::Marked, &marks);
}

/// `export` of categories with examples and folders, and a check of a
/// draft with every kind of change.
#[test]
fn categories_fixtures_match_the_real_export_and_check() {
    let h = Harness::new(Live);
    let mut categories = h.service().config.accounts["work"].categories.clone();
    categories[0].examples = vec!["Weekly report".into()];
    h.service()
        .apply_categories("work", categories.clone())
        .unwrap();
    let export = h.service().categories("work").unwrap();
    check(Output::Export, &[export]);

    let mut draft: Vec<Category> = categories
        .into_iter()
        .filter(|c| c.id != "transactions")
        .collect();
    for c in &mut draft {
        match c.id.as_str() {
            "newsletters" => c.name = "News".into(),
            "promotions" => c.folder = Some("Deals".into()),
            "updates" => c.description = "Status mail".into(),
            _ => {}
        }
    }
    draft.push(Category {
        id: "travel".into(),
        name: "Travel".into(),
        description: "Trips".into(),
        examples: vec![],
        catch_all: false,
        folder: Some("Travel".into()),
    });
    let checked = h.service().validate_categories("work", draft).unwrap();
    // Every kind, in the shape the tray's `Changes` reads (the fixtures
    // leave `removed` and `folders_changed` empty).
    assert_eq!(
        checked["changes"],
        json!({
            "added": ["travel"],
            "removed": [{"id": "transactions", "folder": "Transactions"}],
            "renamed": [{"id": "newsletters", "from": "Newsletters", "to": "News"}],
            "folders_changed": [{"id": "promotions", "from": "Promotions", "to": "Deals"}],
            "edited": ["updates"],
            "reclassifies": true,
        })
    );
    check(Output::Validate, &[checked]);
}

/// `service status --json` for two accounts: `work` with a service file
/// (its executable a script with a tray script beside it) after one pass,
/// `info` without either; with the manager answering and failing, and with
/// and without a cached release.
#[test]
fn status_fixtures_match_the_real_status_of_every_account() {
    let server = Server::start();
    let sandbox = Sandbox::new();
    let root = sandbox.root();
    let tools = root.join("tools");
    fs::create_dir_all(&tools).unwrap();
    write_tool(&tools, "launchctl", LAUNCHCTL);
    write_tool(&tools, "systemctl", SYSTEMCTL);
    let config = sandbox.config("cfg", "auto");
    let mut c: Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
    c["accounts"]["info"] = c["accounts"]["work"].clone();
    fs::write(&config, serde_json::to_vec_pretty(&c).unwrap()).unwrap();
    let mailtriage = |args: &[&str]| -> Value {
        let (code, v, stderr) = run(sandbox
            .command(&server)
            .env("PATH", format!("{}:/usr/bin:/bin", tools.display()))
            .args(args)
            .arg("--config")
            .arg(&config));
        assert_eq!(code, Some(0), "{args:?}: {v} {stderr}");
        v
    };
    mailtriage(&["sync", "--account", "work", "--json"]);

    let svc = root.join("svc/bin");
    fs::create_dir_all(&svc).unwrap();
    fs::set_permissions(&svc, fs::Permissions::from_mode(0o755)).unwrap();
    write_tool(&svc, "mailtriage", "#!/bin/sh\necho 'mailtriage 0.0.1'\n");
    write_tool(
        &svc,
        "mailtriage-tray",
        "#!/bin/sh\necho 'mailtriage-tray 0.0.1'\n",
    );
    let manager = if cfg!(target_os = "macos") {
        Manager::Launchd
    } else {
        Manager::Systemd
    };
    let unit = Unit {
        account: "work".into(),
        exe: svc.join("mailtriage"),
        config: config.clone(),
        interval_seconds: 60,
        limit: 100,
        log_dir: root.join("cfg/logs"),
        path_env: None,
    };
    let unit_dir = system_service::unit_dir(manager, &sandbox.home);
    fs::create_dir_all(&unit_dir).unwrap();
    // The fake manager's own commands load the job, as `service install` would.
    let fake = |args: &[&str]| {
        let tool = match manager {
            Manager::Launchd => "launchctl",
            Manager::Systemd => "systemctl",
        };
        let status = Command::new(tools.join(tool))
            .args(args)
            .env("MT_FAKE_HOME", &sandbox.home)
            .status()
            .unwrap();
        assert!(status.success(), "{tool} {args:?}");
    };
    let status = || mailtriage(&["service", "status", "--json"]);
    let mut real = vec![];
    match manager {
        Manager::Launchd => {
            let plist = unit_dir.join("digital.wirdrei.mailtriage.work.plist");
            fs::write(&plist, system_service::plist(&unit)).unwrap();
            fake(&["bootstrap", "gui/501", plist.to_str().unwrap()]);
            // Loaded and running.
            real.push(status());
        }
        Manager::Systemd => {
            fs::write(
                unit_dir.join("mailtriage-work.service"),
                system_service::systemd_unit(&unit),
            )
            .unwrap();
            fake(&["--user", "daemon-reload"]);
            fake(&["--user", "enable", "mailtriage-work.service"]);
            // Loaded, not running: the config comes from `ExecStart`.
            real.push(status());
            // Running as this test's own process, whose command line names
            // no config.
            fs::write(tools.join("mainpid"), std::process::id().to_string()).unwrap();
            fs::write(tools.join("active"), "").unwrap();
            real.push(status());
        }
    }
    // A cached release newer than both.
    let dir = cache_dir(&sandbox.home, &sandbox.xdg);
    fs::create_dir_all(&dir).unwrap();
    let release = json!({"version": "9.9.9", "release_url": "https://github.com/wir-drei-digital/mailtriage/releases/tag/v9.9.9", "published_at": null, "archives": {"mailtriage": null, "mailtriage-tray": null}, "sums": null});
    fs::write(
        dir.join("update.json"),
        json!({"schema_version": 1, "release": release, "checked_at": "2026-11-03T07:00:00Z"})
            .to_string(),
    )
    .unwrap();
    real.push(status());
    // The manager's query fails.
    fs::write(tools.join("print-fails"), "").unwrap();
    fs::write(tools.join("show-fails"), "").unwrap();
    real.push(status());

    for v in &real {
        let services = v["services"].as_array().unwrap();
        assert_eq!(services.len(), 2, "{v}");
        assert_eq!(services[0]["account"], "info", "{v}");
        assert_eq!(services[1]["update"]["tray"]["installed"], "0.0.1", "{v}");
        // What the tray parses beyond the JSON type.
        for s in services {
            assert!(["launchd", "systemd", "none"].contains(&s["manager"].as_str().unwrap()));
            assert!(["off", "dry_run", "live"].contains(&s["filing_mode"].as_str().unwrap()));
            if let Some(at) = s["last_pass"]["finished_at"].as_str() {
                chrono::DateTime::parse_from_rfc3339(at).unwrap();
            }
        }
    }
    check(Output::Status, &real);
    assert!(server.requests().is_empty(), "status asks no server");
}

#[test]
fn an_empty_real_array_accepts_any_items() {
    let real = shapes(&[serde_json::json!({"log_paths": []})]);
    let fixture = serde_json::json!({"log_paths": ["/a.log"]});
    assert!(drift(&fixture, &real).is_empty());
    let real = shapes(&[serde_json::json!({"log_paths": [1]})]);
    assert_eq!(drift(&fixture, &real).len(), 1);
}
