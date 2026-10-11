#![cfg(unix)]
//! Kev spec, "The managed runtime" and the skips: `ensure_running` against
//! the stand-in Kev server (started by the real `mailtriage kev run`), and
//! classification skipped without an attempt while Kev is unavailable.
mod common;
mod kev_support;
use common::{mail, write_tool, Harness};
use kev_support::{free_port, git_script, models_for, wait_until, Stub};
use mailtriage::{
    config,
    domain::{FilingMode, ManagedKev, ProviderConfig},
    kev::{
        runtime::{self, Probe, Runtime},
        Installed, Layout, TAG,
    },
    provider,
};
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::Duration,
};

/// The stand-in Kev server, run by the stand-in `uv` (see `kev_support`).
#[test]
#[ignore = "the stand-in Kev server; the stand-in uv runs it"]
fn kev_standin_server() {
    kev_support::serve_from_env();
}

const CHECKPOINT: &str = "test/kev@v0";

/// A temp root with `data/mailtriage/kev` (the layout `mailtriage` finds
/// with `XDG_DATA_HOME=root/data`), a stand-in `uv` in `bin` and its
/// control directory, and a free port. Dropping it stops any supervisor.
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    layout: Layout,
    uv: PathBuf,
    control: PathBuf,
    port: u16,
    _environment: Environment,
}

impl Fixture {
    fn new() -> Self {
        let mut environment = Environment::new();
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let layout = Layout::new(root.join("data/mailtriage/kev"));
        let (bin, control) = (root.join("bin"), root.join("control"));
        for d in [&bin, &control, &root.join("home")] {
            fs::create_dir_all(d).unwrap();
        }
        environment.set("HOME", root.join("home"));
        environment.set("XDG_DATA_HOME", root.join("data"));
        let exe = std::env::current_exe().unwrap();
        write_tool(&bin, "uv", &kev_support::uv_script(&control, &exe));
        Self {
            uv: bin.join("uv"),
            _dir: dir,
            root,
            layout,
            control,
            port: free_port(),
            _environment: environment,
        }
    }

    /// What a finished setup leaves: the checkout and `installed.json`.
    fn install(&self, checkpoint: &str) {
        fs::create_dir_all(self.layout.src()).unwrap();
        fs::create_dir_all(self.layout.src().join(".venv")).unwrap();
        fs::create_dir_all(self.layout.hf()).unwrap();
        fs::write(self.layout.src().join(".standin-tag"), TAG).unwrap();
        write_tool(&self.root.join("bin"), "git", &git_script(&self.control));
        Installed {
            tag: TAG.into(),
            checkpoint: checkpoint.into(),
            installed_at: "2026-10-11T00:00:00+00:00".into(),
            uv: self.uv.clone(),
        }
        .write(&self.layout)
        .unwrap();
    }

    fn runtime(&self) -> Runtime {
        Runtime {
            layout: self.layout.clone(),
            port: self.port,
            checkpoint: CHECKPOINT.into(),
            exe: PathBuf::from(env!("CARGO_BIN_EXE_mailtriage")),
            start_timeout: Duration::from_secs(30),
            idle: Duration::from_secs(900),
            check: Duration::from_secs(30),
        }
    }

    fn control(&self, name: &str, text: &str) {
        fs::write(self.control.join(name), text).unwrap();
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.control.join(name)).unwrap_or_default()
    }

    /// The `uv run` lines of `uv.log`.
    fn runs(&self) -> Vec<String> {
        self.read("uv.log")
            .lines()
            .filter(|l| l.starts_with("run "))
            .map(str::to_owned)
            .collect()
    }

    fn kev_log(&self) -> String {
        fs::read_to_string(self.layout.log()).unwrap_or_default()
    }

    /// The provider block of a managed Kev on this fixture's port.
    fn managed_config(&self) -> ProviderConfig {
        let mut provider = provider::provider_for("kev").unwrap().new_config();
        provider.endpoint = provider::kev::local_endpoint(self.port);
        provider.managed = Some(ManagedKev {
            checkpoint: CHECKPOINT.into(),
        });
        provider
    }

    fn doctor(&self, provider: &ProviderConfig) -> Value {
        let path = self.root.join("doctor.json");
        let mut c = config::default_config();
        c.provider = provider.clone();
        c.state_dir = self.root.join("doctor-state");
        config::save(&path, &c).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
            .arg("--config")
            .arg(&path)
            .args(["doctor", "--account", "work", "--json"])
            .env("HOME", self.root.join("home"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("bin").display()),
            )
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
}

#[test]
fn doctor_reports_local_kev_without_starting_it() {
    let f = Fixture::new();
    let provider = f.managed_config();
    let v = f.doctor(&provider);
    let kev = &v["provider"]["kev"];
    assert_eq!(kev["mode"], "managed", "{v}");
    assert_eq!(kev["installed"], false);
    assert_eq!(kev["ready"], false);
    assert_eq!(
        kev["error"],
        "local Kev is not installed; run mailtriage setup --update"
    );
    assert_eq!(kev["fix"], "run `mailtriage setup --update`");
    assert_eq!(v["ready"], false);
    assert_eq!(v["live_checks_performed"], true);
    assert!(!f.layout.root().exists(), "doctor created Kev files");
    f.install(CHECKPOINT);
    let v = f.doctor(&provider);
    let kev = &v["provider"]["kev"];
    assert_eq!(kev["installed"], true, "{v}");
    assert_eq!(kev["tag"], TAG);
    assert_eq!(kev["checkpoint"], CHECKPOINT);
    assert_eq!(kev["running"], false);
    assert_eq!(kev["ready"], true);
    assert_eq!(v["ready"], true);
    assert!(
        f.runs().is_empty(),
        "doctor started Kev: {}",
        f.read("uv.log")
    );
    runtime::ensure_running_with(&f.runtime()).unwrap();
    assert_eq!(f.doctor(&provider)["provider"]["kev"]["running"], true);
}

#[test]
fn doctor_reports_missing_artifacts_and_abandoned_ownership() {
    let f = Fixture::new();
    let provider = f.managed_config();
    f.install(CHECKPOINT);
    for missing in [
        f.layout.src(),
        f.layout.hf(),
        f.layout.src().join(".venv"),
        f.uv.clone(),
    ] {
        let parked = missing.with_extension("parked");
        fs::rename(&missing, &parked).unwrap();
        let kev = f.doctor(&provider)["provider"]["kev"].clone();
        assert_eq!(kev["installed"], false, "{kev}");
        assert_eq!(kev["ready"], false, "{kev}");
        assert_eq!(kev["fix"], "run `mailtriage setup --update`", "{kev}");
        fs::rename(&parked, &missing).unwrap();
    }
    fs::write(f.layout.src().join(".standin-tag"), "other-tag").unwrap();
    assert_eq!(f.doctor(&provider)["provider"]["kev"]["installed"], false);
    fs::write(f.layout.src().join(".standin-tag"), TAG).unwrap();
    let mut changed = provider.clone();
    changed.managed.as_mut().unwrap().checkpoint = "other/checkpoint@v1".into();
    let kev = f.doctor(&changed)["provider"]["kev"].clone();
    assert_eq!(kev["installed"], false);
    assert_eq!(kev["installed_checkpoint"], CHECKPOINT);
    fs::write(f.layout.ownership(), "corrupt").unwrap();
    let kev = f.doctor(&provider)["provider"]["kev"].clone();
    assert_eq!(kev["ready"], false, "{kev}");
    assert!(
        kev["error"].as_str().unwrap().contains("unconfirmed"),
        "{kev}"
    );
    assert!(kev["fix"].as_str().unwrap().contains("verify"), "{kev}");
    assert_eq!(fs::read_to_string(f.layout.ownership()).unwrap(), "corrupt");
}

#[test]
fn doctor_reports_installation_busy_before_inspecting_server_lock() {
    use fs2::FileExt;
    let f = Fixture::new();
    f.install(CHECKPOINT);
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(f.layout.installation_lock())
        .unwrap();
    lock.lock_exclusive().unwrap();
    let kev = f.doctor(&f.managed_config())["provider"]["kev"].clone();
    assert_eq!(kev["ready"], false, "{kev}");
    assert!(
        kev["error"].as_str().unwrap().contains("in progress"),
        "{kev}"
    );
    assert_eq!(kev["running"], false, "{kev}");
    assert!(!f.layout.lock().exists(), "doctor created a server lock");
}

#[test]
fn doctor_names_nonwritable_server_lock_without_claiming_ready_or_resetting_it() {
    let f = Fixture::new();
    f.install(CHECKPOINT);
    fs::write(f.layout.lock(), "fixture lock\n").unwrap();
    fs::set_permissions(f.layout.lock(), fs::Permissions::from_mode(0o400)).unwrap();
    let before = fs::read(f.layout.lock()).unwrap();
    let kev = f.doctor(&f.managed_config())["provider"]["kev"].clone();
    assert_eq!(kev["ready"], false, "{kev}");
    assert!(
        kev["error"]
            .as_str()
            .unwrap()
            .contains(f.layout.lock().to_str().unwrap()),
        "{kev}"
    );
    let fix = kev["fix"].as_str().unwrap();
    assert!(
        fix.contains("ownership") && fix.contains("permissions"),
        "{kev}"
    );
    assert!(fix.contains("mailtriage doctor"), "{kev}");
    assert!(!fix.contains("backup"), "{kev}");
    assert_eq!(fs::read(f.layout.lock()).unwrap(), before);
    assert_eq!(
        fs::metadata(f.layout.lock()).unwrap().permissions().mode() & 0o777,
        0o400
    );
    assert!(f.runs().is_empty(), "doctor started Kev");
}

#[test]
fn doctor_reports_whether_your_kev_server_answers() {
    let f = Fixture::new();
    let server = |endpoint: String| {
        let mut p = provider::provider_for("kev").unwrap().new_config();
        p.managed = None;
        p.endpoint = endpoint;
        p
    };
    let good = Stub::start(models_for("any"), None);
    let v = f.doctor(&server(good.endpoint()));
    assert_eq!(
        v["provider"]["kev"],
        json!({"mode":"server","endpoint":good.endpoint(),"reachable":true,"ready":true})
    );
    let other = Stub::start(json!({"models":[{"name":"jev-latest"}]}), None);
    let v = f.doctor(&server(other.endpoint()));
    assert_eq!(v["provider"]["kev"]["reachable"], true);
    assert_eq!(v["provider"]["kev"]["ready"], false);
    assert_eq!(
        v["provider"]["kev"]["error"],
        "the Kev server does not serve kev-latest"
    );
    let port = free_port();
    let v = f.doctor(&server(provider::kev::local_endpoint(port)));
    let kev = &v["provider"]["kev"];
    assert_eq!(kev["reachable"], false);
    assert_eq!(kev["ready"], false);
    assert_eq!(
        kev["error"],
        format!("the Kev server did not answer at http://127.0.0.1:{port}/v1/models")
    );
    assert_eq!(
        kev["fix"],
        "start your Kev server, or check provider.endpoint and its key"
    );
    assert_eq!(v["ready"], false);
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = runtime::stop(&self.layout, runtime::STOP_WAIT);
    }
}

#[test]
fn the_first_caller_starts_kev_and_the_next_reuses_it() {
    let f = Fixture::new();
    f.install(CHECKPOINT);
    let rt = f.runtime();
    runtime::ensure_running_with(&rt).unwrap();
    runtime::ensure_running_with(&rt).unwrap();
    let runs = f.runs();
    assert_eq!(runs.len(), 1, "{}", f.read("uv.log"));
    let run = &runs[0];
    assert!(
        run.starts_with(&format!(
            "run --frozen --extra serve python -m kev.serve --run {CHECKPOINT} --host 127.0.0.1 --port {}|",
            f.port
        )),
        "{run}"
    );
    assert!(
        run.contains(&format!("|cwd={}|", f.layout.src().display())),
        "{run}"
    );
    assert!(
        run.contains(&format!("|HF_HOME={}|", f.layout.hf().display())),
        "{run}"
    );
    assert!(run.contains("|HF_HUB_OFFLINE=1|"), "{run}");
    assert!(
        run.contains(&format!(
            "|UV_CACHE_DIR={}|",
            f.layout.root().join("uv-cache").display()
        )),
        "{run}"
    );
    assert!(
        run.ends_with(&format!(
            "|UV_PYTHON_INSTALL_DIR={}",
            f.layout.root().join("python").display()
        )),
        "{run}"
    );
    assert!(f.layout.last_used().exists());
    assert_eq!(runtime::probe(&rt), Probe::Ready);
    assert!(runtime::supervisor_running(&f.layout));
    assert!(
        f.kev_log().contains(&format!(
            "starting Kev: --run {CHECKPOINT} on 127.0.0.1:{}",
            f.port
        )),
        "{}",
        f.kev_log()
    );
}

#[test]
fn without_a_matching_install_kev_is_not_installed() {
    let f = Fixture::new();
    let not_installed = "local Kev is not installed; run mailtriage setup --update";
    let e = runtime::ensure_running_with(&f.runtime()).unwrap_err();
    assert_eq!(e.to_string(), not_installed);
    f.install("jaredpalmer/kev-9b@v1.0");
    let e = runtime::ensure_running_with(&f.runtime()).unwrap_err();
    assert_eq!(e.to_string(), not_installed);
    assert!(f.runs().is_empty());
}

/// Review focus 2: another program on the port, even one that serves
/// `kev-latest` from another checkpoint, is never used.
#[test]
fn another_program_on_the_port_is_named() {
    let f = Fixture::new();
    f.install(CHECKPOINT);
    let other = Stub::start(models_for("jaredpalmer/kev-9b@v1.0"), None);
    let mut rt = f.runtime();
    rt.port = other.port;
    let e = runtime::ensure_running_with(&rt).unwrap_err();
    assert_eq!(
        e.to_string(),
        format!("port {} is used by another program", other.port)
    );
    assert!(f.runs().is_empty());
    assert!(other
        .requests()
        .iter()
        .all(|r| r.starts_with("GET /v1/models")));
}

#[test]
fn two_callers_at_once_leave_one_server() {
    let f = Fixture::new();
    f.install(CHECKPOINT);
    let rt = f.runtime();
    thread::scope(|s| {
        let a = s.spawn(|| runtime::ensure_running_with(&rt));
        let b = s.spawn(|| runtime::ensure_running_with(&rt));
        a.join().unwrap().unwrap();
        b.join().unwrap().unwrap();
    });
    assert_eq!(f.runs().len(), 1, "{}", f.read("uv.log"));
}

#[test]
fn kev_stops_after_the_idle_time_unless_it_is_used() {
    let f = Fixture::new();
    f.install(CHECKPOINT);
    let mut rt = f.runtime();
    rt.idle = Duration::from_secs(3);
    rt.check = Duration::from_secs(1);
    runtime::ensure_running_with(&rt).unwrap();
    // A request every second keeps it running past the idle time.
    for _ in 0..5 {
        thread::sleep(Duration::from_secs(1));
        runtime::ensure_running_with(&rt).unwrap();
    }
    assert!(runtime::supervisor_running(&f.layout));
    assert!(wait_until(Duration::from_secs(20), || {
        !runtime::supervisor_running(&f.layout)
    }));
    assert_eq!(runtime::probe(&rt), Probe::Nothing);
    assert!(
        f.kev_log()
            .contains("stopping Kev after 3 seconds without a request"),
        "{}",
        f.kev_log()
    );
    assert_eq!(f.runs().len(), 1);
}

#[test]
fn a_server_that_does_not_answer_in_time_is_reported() {
    let f = Fixture::new();
    f.install(CHECKPOINT);
    f.control("delay", "60");
    let mut rt = f.runtime();
    rt.start_timeout = Duration::from_secs(3);
    let e = runtime::ensure_running_with(&rt).unwrap_err();
    assert_eq!(
        e.to_string(),
        format!("local Kev did not start; see {}", f.layout.log().display())
    );
}

#[test]
fn kev_exiting_ends_the_supervisor_and_is_logged() {
    let f = Fixture::new();
    f.install(CHECKPOINT);
    f.control("exit-now", "");
    let mut rt = f.runtime();
    rt.start_timeout = Duration::from_secs(3);
    assert!(runtime::ensure_running_with(&rt).is_err());
    assert!(wait_until(Duration::from_secs(10), || {
        !runtime::supervisor_running(&f.layout)
    }));
    assert!(
        f.kev_log().contains("Kev exited (exit status: 3)"),
        "{}",
        f.kev_log()
    );
}

/// Review focus 1: the supervisor leads its own session, so a service
/// manager that ends `watch`'s process group leaves Kev running.
#[test]
fn the_supervisor_runs_in_its_own_session() {
    let f = Fixture::new();
    f.install(CHECKPOINT);
    runtime::ensure_running_with(&f.runtime()).unwrap();
    let pid = fs::read_to_string(f.layout.lock())
        .unwrap()
        .trim()
        .to_owned();
    let out = Command::new("ps")
        .args(["-o", "pgid=", "-p", &pid])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), pid);
    let args = Command::new("ps")
        .args(["-o", "command=", "-p", &pid])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&args.stdout).contains("--stop-grace-seconds 10"));
}

/// `(state, attempts)` of every job of the Harness config at `config`.
fn jobs(config: &Path) -> Vec<(String, u32)> {
    let db =
        rusqlite::Connection::open(config.with_file_name("mailtriage-state/mailtriage.sqlite"))
            .unwrap();
    let mut stmt = db
        .prepare("SELECT state, attempts FROM jobs ORDER BY message_id")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// Kev spec, "Skips": an uninstalled local Kev skips classification like a
/// missing key: the mail stays queued and no attempt is counted. The
/// checkpoint is one no real install has, so the user's data dir is only
/// read.
#[test]
fn an_uninstalled_local_kev_skips_classification_without_an_attempt() {
    let _fixture = Fixture::new();
    let h = Harness::new(FilingMode::Off);
    h.edit(|c| {
        c.provider = provider::provider_for("kev").unwrap().new_config();
        c.provider.managed = Some(ManagedKev {
            checkpoint: "test/never-installed@v0".into(),
        });
    });
    h.fake
        .deliver("INBOX", &mail("a", "Invoice", "Please pay by Friday."));
    let out = h.sync();
    assert_eq!(
        out["classification"],
        json!({"skipped": true, "reason": "local Kev is not installed; run mailtriage setup --update"}),
        "{out}"
    );
    assert_eq!(out["partial"], true);
    assert_eq!(jobs(&h.path), vec![("queued".to_owned(), 0)]);
}

fn kev_server(h: &Harness, endpoint: &str) {
    h.edit(|c| {
        c.provider = provider::provider_for("kev").unwrap().new_config();
        c.provider.managed = None;
        c.provider.endpoint = endpoint.into();
    });
}

#[test]
fn an_unreachable_kev_server_skips_classification() {
    let _fixture = Fixture::new();
    let h = Harness::new(FilingMode::Off);
    let port = free_port();
    kev_server(&h, &provider::kev::local_endpoint(port));
    h.fake
        .deliver("INBOX", &mail("a", "Invoice", "Please pay by Friday."));
    let out = h.sync();
    assert_eq!(
        out["classification"]["reason"],
        format!("the Kev server did not answer at http://127.0.0.1:{port}/v1/models"),
        "{out}"
    );
    assert_eq!(jobs(&h.path), vec![("queued".to_owned(), 0)]);
}

#[test]
fn a_kev_server_classifies_and_its_http_errors_count_an_attempt() {
    let _fixture = Fixture::new();
    let h = Harness::new(FilingMode::Off);
    let good = Stub::start(models_for("any"), None);
    kev_server(&h, &good.endpoint());
    h.fake
        .deliver("INBOX", &mail("a", "Lunch", "Are you free on Monday?"));
    let out = h.sync();
    assert_eq!(out["classified"], 1, "{out}");
    assert!(out.get("classification").is_none(), "{out}");
    assert!(good
        .requests()
        .contains(&"POST /v1/systemone auth=no".to_owned()));
    // Another account state: a provider change would queue "a" again.
    let h = Harness::new(FilingMode::Off);
    let failing = Stub::start(models_for("any"), Some(500));
    kev_server(&h, &failing.endpoint());
    h.fake
        .deliver("INBOX", &mail("b", "Invoice", "Please pay by Friday."));
    let out = h.sync();
    assert_eq!(out["failed"], 1, "{out}");
    assert_eq!(jobs(&h.path), vec![("retry".to_owned(), 1)]);
}

/// Through the binary, with an empty data dir: `classify` skips and
/// `export` lists no attempt.
#[test]
fn classify_skips_while_local_kev_is_not_installed() {
    let f = Fixture::new();
    let path = f.root.join("config.json");
    let mut c = config::default_config();
    c.provider = f.managed_config();
    c.state_dir = f.root.join("state");
    config::save(&path, &c).unwrap();
    let run = |args: &[&str]| -> Value {
        let out = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
            .arg("--config")
            .arg(&path)
            .args(args)
            .env("HOME", f.root.join("home"))
            .env("XDG_DATA_HOME", f.root.join("data"))
            .output()
            .unwrap();
        serde_json::from_slice(&out.stdout).unwrap()
    };
    let receipt = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/receipt.eml");
    let v = run(&[
        "classify",
        "--account",
        "work",
        "--input",
        receipt,
        "--format",
        "rfc822",
        "--json",
    ]);
    assert_eq!(v["outcome"], "skipped", "{v}");
    assert_eq!(
        v["classification"]["reason"],
        "local Kev is not installed; run mailtriage setup --update"
    );
    let export = run(&["export", "--account", "work", "--json"]);
    assert_eq!(export["attempts"], json!([]), "{export}");
    assert!(f.runs().is_empty());
}

static ENVIRONMENT: std::sync::Mutex<()> = std::sync::Mutex::new(());
struct Environment {
    _guard: std::sync::MutexGuard<'static, ()>,
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
}
impl Environment {
    fn new() -> Self {
        Self {
            _guard: ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner()),
            previous: vec![],
        }
    }
    fn set(&mut self, key: &'static str, value: impl AsRef<std::ffi::OsStr>) {
        self.previous.push((key, std::env::var_os(key)));
        std::env::set_var(key, value);
    }
}
impl Drop for Environment {
    fn drop(&mut self) {
        for (key, value) in self.previous.drain(..).rev() {
            if let Some(value) = value {
                std::env::set_var(key, value)
            } else {
                std::env::remove_var(key)
            }
        }
    }
}

#[test]
fn managed_children_discard_authentication_and_enforce_uv_offline() {
    let mut f = Fixture::new();
    f.install(CHECKPOINT);
    f._environment.set("KEV_API_KEY", "test-auth-sentinel");
    runtime::ensure_running_with(&f.runtime()).unwrap();
    assert!(f
        .read("uv.log")
        .contains("|UV_OFFLINE=1|KEV_API_KEY=unset|"));
    assert!(!f.read("uv.log").contains("test-auth-sentinel"));
}

#[test]
fn first_download_survives_normal_idle_and_discards_authentication() {
    let mut f = Fixture::new();
    f.install(CHECKPOINT);
    f.control("delay", "3");
    f._environment.set("KEV_API_KEY", "test-auth-sentinel");
    f._environment
        .set("MAILTRIAGE_TEST_KEV", r#"{"download_seconds":5}"#);
    f._environment.set("UV_OFFLINE", "1");
    f._environment.set("HF_HUB_OFFLINE", "1");
    let mut rt = f.runtime();
    rt.idle = Duration::from_secs(1);
    rt.check = Duration::from_secs(1);
    runtime::spawn_supervisor(&rt, &f.uv, true).unwrap();
    assert!(
        wait_until(Duration::from_secs(8), || runtime::probe(&rt)
            == Probe::Ready),
        "{}",
        f.kev_log()
    );
    assert!(f
        .read("uv.log")
        .contains("|UV_OFFLINE=unset|KEV_API_KEY=unset|"));
}

#[test]
fn stopping_is_confirmed_and_controlled_exit_is_logged() {
    let f = Fixture::new();
    f.install(CHECKPOINT);
    assert_eq!(
        runtime::stop(&f.layout, Duration::from_secs(1)).unwrap(),
        runtime::StopOutcome::AlreadyStopped
    );
    runtime::ensure_running_with(&f.runtime()).unwrap();
    assert_eq!(
        runtime::stop(&f.layout, runtime::STOP_WAIT).unwrap(),
        runtime::StopOutcome::Stopped
    );
    assert_eq!(runtime::probe(&f.runtime()), Probe::Nothing);
    assert!(f.kev_log().contains("Kev exited ("), "{}", f.kev_log());
}

#[test]
fn stop_reports_lock_errors() {
    let f = Fixture::new();
    fs::create_dir_all(f.layout.root()).unwrap();
    fs::create_dir(f.layout.lock()).unwrap();
    assert!(runtime::stop(&f.layout, Duration::ZERO).is_err());
    assert!(runtime::try_supervisor_running(&f.layout).is_err());
}

#[test]
fn stop_times_out_and_cleans_a_term_resistant_descendant_after_its_leader_exits() {
    let f = Fixture::new();
    f.install(CHECKPOINT);
    write_tool(
        f.uv.parent().unwrap(),
        "uv",
        &format!(
            r#"#!/bin/sh
/bin/sh -c 'trap "" TERM; echo $$ > "$1"; while :; do sleep 1; done' sh '{}' &
wait
"#,
            f.control.join("descendant.pid").display()
        ),
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
        .args(["kev", "run", "--dir"])
        .arg(f.layout.root())
        .args([
            "--port",
            &f.port.to_string(),
            "--checkpoint",
            CHECKPOINT,
            "--uv",
        ])
        .arg(&f.uv)
        .args(["--stop-grace-seconds", "1"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    assert!(wait_until(Duration::from_secs(5), || f
        .control
        .join("descendant.pid")
        .exists()));
    let pid = f.read("descendant.pid");
    assert_eq!(
        runtime::stop(&f.layout, Duration::ZERO).unwrap(),
        runtime::StopOutcome::TimedOut
    );
    assert!(child.wait().unwrap().success());
    let out = Command::new("ps")
        .args(["-o", "stat=", "-p", pid.trim()])
        .output()
        .unwrap();
    let stopped = String::from_utf8_lossy(&out.stdout).trim().is_empty();
    if !stopped {
        let _ = Command::new("kill").args(["-KILL", pid.trim()]).status();
    }
    assert!(stopped, "descendant remains alive");
    assert!(f.kev_log().contains("Kev exited ("));
}

#[test]
fn local_prepare_discards_an_explicit_key() {
    let f = Fixture::new();
    let stub = Stub::start(models_for(CHECKPOINT), None);
    let mut config = f.managed_config();
    config.managed = None;
    config.endpoint = stub.endpoint();
    provider::provider_for("kev")
        .unwrap()
        .prepare(&config, Some("discard-me"))
        .unwrap();
    assert_eq!(stub.requests(), vec!["GET /v1/models auth=no"]);
}

fn disappearing_install(f: &Fixture) -> Stub {
    f.install(CHECKPOINT);
    let installed = f.layout.installed();
    Stub::start_with_hook(models_for(CHECKPOINT), None, move || {
        let _ = fs::remove_file(&installed);
    })
}

#[test]
fn sync_stops_after_readiness_without_losing_earlier_successes_or_later_jobs() {
    let f = Fixture::new();
    let h = Harness::new(FilingMode::Off);
    let stub = disappearing_install(&f);
    h.edit(|c| {
        c.provider = f.managed_config();
        c.provider.endpoint = stub.endpoint();
    });
    for id in ["first", "second", "third"] {
        h.fake.deliver("INBOX", &mail(id, "Invoice", "Please pay."));
    }
    let out = h.sync();
    assert_eq!(out["classified"], 1, "{out}");
    assert_eq!(out["failed"], 0, "{out}");
    assert_eq!(out["pending"], 2, "{out}");
    assert_eq!(out["classification"]["skipped"], true, "{out}");
    assert_eq!(out["partial"], true, "{out}");
    let mut state = jobs(&h.path);
    state.sort();
    assert_eq!(
        state,
        vec![
            ("complete".into(), 1),
            ("queued".into(), 0),
            ("queued".into(), 0)
        ]
    );
    assert_eq!(
        h.service().export("work").unwrap()["attempts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn classify_skips_after_an_earlier_success_with_the_same_prepared_service() {
    let f = Fixture::new();
    let h = Harness::new(FilingMode::Off);
    let stub = disappearing_install(&f);
    h.edit(|c| {
        c.provider = f.managed_config();
        c.provider.endpoint = stub.endpoint();
    });
    let mut service = h.service();
    assert_eq!(
        service
            .classify("work", &mail("a", "Invoice", "Pay."), "rfc822")
            .unwrap()["outcome"],
        "classified"
    );
    let out = service
        .classify("work", &mail("b", "Invoice", "Pay."), "rfc822")
        .unwrap();
    assert_eq!(out["outcome"], "skipped", "{out}");
    assert_eq!(out["partial"], true);
    assert_eq!(
        service.export("work").unwrap()["attempts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let mut state = jobs(&h.path);
    state.sort();
    assert_eq!(state, vec![("complete".into(), 1), ("queued".into(), 0)]);
}

#[test]
fn reclassify_stops_after_readiness_and_preserves_existing_audit_history() {
    let f = Fixture::new();
    let h = Harness::new(FilingMode::Off);
    let mut service = h.service();
    for id in ["a", "b", "c"] {
        service
            .classify("work", &mail(id, "Invoice", "Pay."), "rfc822")
            .unwrap();
    }
    let stub = disappearing_install(&f);
    h.edit(|c| {
        c.provider = f.managed_config();
        c.provider.endpoint = stub.endpoint();
    });
    let out = h.service().reclassify("work", None, false, 100).unwrap();
    assert_eq!(out["reclassified"], 1, "{out}");
    assert_eq!(out["failed"], 0);
    assert_eq!(out["pending"], 2);
    assert_eq!(out["classification"]["skipped"], true);
    assert_eq!(out["partial"], true);
    assert_eq!(
        h.service().export("work").unwrap()["attempts"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
}

#[test]
fn handler_registration_failure_is_an_error_before_starting_kev() {
    let f = Fixture::new();
    f.install(CHECKPOINT);
    ctrlc::set_handler(|| {}).unwrap();
    let args = mailtriage::kev::supervisor::RunArgs {
        dir: f.layout.root().to_owned(),
        port: f.port,
        checkpoint: CHECKPOINT.into(),
        uv: f.uv.clone(),
        allow_download: false,
        idle: Duration::from_secs(900),
        check: Duration::from_secs(30),
        grace: Duration::from_secs(10),
    };
    assert!(mailtriage::kev::supervisor::run(&args).is_err());
    assert!(f.runs().is_empty());
    assert!(!runtime::try_supervisor_running(&f.layout).unwrap());
}

#[test]
fn an_unpublished_owner_cannot_claim_shutdown() {
    use fs2::FileExt;
    let f = Fixture::new();
    fs::create_dir_all(f.layout.root()).unwrap();
    let lock = fs::File::create(f.layout.lock()).unwrap();
    lock.lock_exclusive().unwrap();
    assert_eq!(
        runtime::stop(&f.layout, Duration::ZERO).unwrap(),
        runtime::StopOutcome::TimedOut
    );
}

#[test]
fn supervisor_disappearance_cannot_confirm_shutdown_while_its_child_survives() {
    let f = Fixture::new();
    f.install(CHECKPOINT);
    let script = kev_support::uv_script(&f.control, &std::env::current_exe().unwrap()).replace(
        "  run)\n",
        &format!(
            "  run)\n    echo $$ > '{}'\n",
            f.control.join("leader.pid").display()
        ),
    );
    write_tool(f.uv.parent().unwrap(), "uv", &script);
    runtime::ensure_running_with(&f.runtime()).unwrap();
    let supervisor = fs::read_to_string(f.layout.lock()).unwrap();
    let leader: i32 = f.read("leader.pid").trim().parse().unwrap();
    Command::new("kill")
        .args(["-KILL", supervisor.trim()])
        .status()
        .unwrap();
    assert!(wait_until(Duration::from_secs(5), || {
        !runtime::supervisor_running(&f.layout)
    }));
    assert_eq!(runtime::probe(&f.runtime()), Probe::Ready);
    let outcome = runtime::stop(&f.layout, Duration::from_secs(1));
    let ready = runtime::ensure_running_with(&f.runtime());
    let spawned = runtime::spawn_supervisor(&f.runtime(), &f.uv, false);
    kill_fixture_group(leader);
    assert!(
        outcome.is_err(),
        "a surviving owned group was reported as {outcome:?}"
    );
    assert!(
        ready.is_err(),
        "abandoned ownership must not regain managed readiness"
    );
    assert!(
        spawned.is_err(),
        "abandoned ownership must not start another supervisor"
    );
}

#[test]
fn a_new_owner_over_a_stale_valid_pid_does_not_signal_the_unrelated_process() {
    use fs2::FileExt;
    let f = Fixture::new();
    fs::create_dir_all(f.layout.root()).unwrap();
    let mut unrelated = Command::new("/bin/sh")
        .args(["-c", "exec sleep 60"])
        .spawn()
        .unwrap();
    fs::write(f.layout.lock(), unrelated.id().to_string()).unwrap();
    // A new owner holds the persistent lock, paused before PID publication.
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(f.layout.lock())
        .unwrap();
    lock.lock_exclusive().unwrap();
    let outcome = runtime::stop(&f.layout, Duration::from_millis(300));
    let untouched = unrelated.try_wait().unwrap().is_none();
    let _ = unrelated.kill();
    let _ = unrelated.wait();
    drop(lock);
    assert!(untouched, "stop sent a signal to the stale, unrelated PID");
    assert!(!matches!(
        outcome,
        Ok(runtime::StopOutcome::Stopped | runtime::StopOutcome::AlreadyStopped)
    ));
}

fn kill_fixture_group(group: i32) {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    unsafe {
        kill(-group, 9);
    }
}

#[test]
fn installation_busy_inspection_is_read_only_and_distinguishes_shared_startup() {
    use fs2::FileExt;
    let f = Fixture::new();
    assert!(!runtime::installation_in_progress(&f.layout).unwrap());
    assert!(
        !f.layout.root().exists(),
        "inspection creates no layout or lock file"
    );
    f.install(CHECKPOINT);
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(f.layout.installation_lock())
        .unwrap();
    file.lock_exclusive().unwrap();
    assert!(runtime::installation_in_progress(&f.layout).unwrap());
    assert!(
        !f.layout.lock().exists(),
        "inspection must not touch server.lock"
    );
    FileExt::unlock(&file).unwrap();
    FileExt::lock_shared(&file).unwrap();
    assert!(
        !runtime::installation_in_progress(&f.layout).unwrap(),
        "normal shared startup is not mutation"
    );
    FileExt::unlock(&file).unwrap();
    drop(file);
    fs::remove_file(f.layout.installation_lock()).unwrap();
    fs::create_dir(f.layout.installation_lock()).unwrap();
    assert!(
        runtime::installation_in_progress(&f.layout).is_err(),
        "malformed guard is uncertainty, not idle"
    );
}
