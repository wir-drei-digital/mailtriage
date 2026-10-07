#![cfg(unix)]
//! The `update` block of `service status` and `doctor`, read from the
//! service file, the executable's `--version` and the cache. No network.
mod common;
mod update_support;
use common::{write_tool, LAUNCHCTL, SYSTEMCTL};
use mailtriage::system_service::{self, Manager, Unit};
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};
use update_support::{cache_dir, run, set_updates, Server};

const RUNNING: &str = env!("CARGO_PKG_VERSION");

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    xdg: PathBuf,
    server: Server,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let (home, xdg, bin) = (root.join("home"), root.join("xdg"), root.join("bin"));
        for d in [&home, &xdg, &bin] {
            fs::create_dir_all(d).unwrap();
        }
        write_tool(&bin, "launchctl", LAUNCHCTL);
        write_tool(&bin, "systemctl", SYSTEMCTL);
        let env = Self {
            _dir: dir,
            root,
            home,
            xdg,
            server: Server::start(),
        };
        let (code, _, _) = run(env.command().args(["init", "--json"]));
        assert_eq!(code, Some(0));
        env
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mailtriage"));
        command
            .current_dir(&self.root)
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.xdg)
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("bin").display()),
            )
            .env("MAILTRIAGE_UPDATE_URL", &self.server.base)
            .env_remove("MAILTRIAGE_CONFIG");
        command
    }

    fn status(&self) -> Value {
        let (code, v, stderr) =
            run(self
                .command()
                .args(["service", "status", "--account", "work", "--json"]));
        assert_eq!(code, Some(0), "{v} {stderr}");
        v["service"]["update"].clone()
    }

    fn doctor(&self) -> Value {
        let (code, v, stderr) = run(self
            .command()
            .args(["doctor", "--account", "work", "--json"]));
        assert_eq!(code, Some(0), "{v} {stderr}");
        v
    }

    /// A marked service file for `work` running `exe`.
    fn service_file(&self, exe: &Path) {
        self.service_file_for("work", exe);
    }

    /// A marked service file for `account` running `exe`.
    fn service_file_for(&self, account: &str, exe: &Path) {
        let manager = if cfg!(target_os = "macos") {
            Manager::Launchd
        } else {
            Manager::Systemd
        };
        let unit = Unit {
            account: account.into(),
            exe: exe.to_path_buf(),
            config: self.root.join("mailtriage.json"),
            interval_seconds: 60,
            limit: 100,
            log_dir: self.root.join("logs"),
            path_env: None,
        };
        let dir = system_service::unit_dir(manager, &self.home);
        fs::create_dir_all(&dir).unwrap();
        let (name, text) = match manager {
            Manager::Launchd => (
                format!("digital.wirdrei.mailtriage.{account}.plist"),
                system_service::plist(&unit),
            ),
            Manager::Systemd => (
                format!("mailtriage-{account}.service"),
                system_service::systemd_unit(&unit),
            ),
        };
        fs::write(dir.join(name), text).unwrap();
    }

    /// A script printing `mailtriage VERSION` at `<root>/<relative>/mailtriage`.
    fn executable(&self, relative: &str, version: &str) -> PathBuf {
        let dir = self.root.join(relative);
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        write_tool(
            &dir,
            "mailtriage",
            &format!("#!/bin/sh\necho 'mailtriage {version}'\n"),
        );
        dir.join("mailtriage")
    }

    fn write_cache(&self, cache: &Value) {
        let dir = cache_dir(&self.home, &self.xdg);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("update.json"), cache.to_string()).unwrap();
    }
}

#[test]
fn without_a_service_file_status_describes_the_running_binary() {
    let env = Env::new();
    let u = env.status();
    assert_eq!(u["mode"], "auto");
    assert_eq!(u["executable"], Value::Null);
    assert_eq!(u["installed"], RUNNING);
    // Before the first check.
    assert_eq!(
        (u["latest"].clone(), u["checked_at"].clone()),
        (Value::Null, Value::Null)
    );
    assert_eq!(
        (u["available"].clone(), u["last_error"].clone()),
        (json!(false), Value::Null)
    );
    assert!(u["replaceable"].is_boolean());
    assert!(env.server.requests().is_empty());
}

#[test]
fn status_describes_the_services_executable_and_the_cache() {
    let env = Env::new();
    let exe = env.executable("svc/bin", "0.0.1");
    env.service_file(&exe);
    set_updates(&env.root.join("mailtriage.json"), "notify");
    let release = json!({"version":"9.9.9","release_url":"https://github.com/wir-drei-digital/mailtriage/releases/tag/v9.9.9","published_at":null,"archives":{"mailtriage":null},"sums":null});
    let check_error = json!({"at":"2026-11-03T08:00:00Z","message":"cannot read the release list: GitHub answered 503"});
    let install_error = json!({"at":"2026-11-03T09:00:00Z","message":"checksum mismatch for x"});
    let mut cache = json!({
        "schema_version": 1, "release": release, "checked_at": "2026-11-03T07:00:00Z",
        "next_check_at": null, "check_failures": 1, "last_check_error": check_error,
        "configs": {}, "installs": {exe.to_str().unwrap(): {"version":"0.0.1","at":null,"last_error":install_error,"failures":1,"next_attempt_at":null}}
    });
    env.write_cache(&cache);
    assert_eq!(
        env.status(),
        json!({"mode":"notify","executable":exe,"installed":"0.0.1","latest":"9.9.9","available":true,
               "checked_at":"2026-11-03T07:00:00Z","last_error":install_error,"replaceable":true,"reason":null})
    );
    cache["installs"] = json!({});
    env.write_cache(&cache);
    assert_eq!(env.status()["last_error"], check_error);
    assert!(env.server.requests().is_empty());
}

#[test]
fn doctor_adds_readiness_only_auto_needs() {
    let env = Env::new();
    let exe = env.executable("svc/bin", "0.0.1");
    env.service_file(&exe);
    let d = env.doctor();
    assert_eq!(d["update"]["ready"], true);
    assert!(d["update"].get("fix").is_none());

    let brew = env.executable("brew/Cellar/mailtriage/0.0.1/bin", "0.0.1");
    env.service_file(&brew);
    let d = env.doctor();
    assert_eq!(d["update"]["executable"], brew.to_str().unwrap());
    assert_eq!(
        (
            d["update"]["replaceable"].clone(),
            d["update"]["reason"].clone()
        ),
        (json!(false), json!("managed_by_homebrew"))
    );
    assert_eq!(
        (d["update"]["ready"].clone(), d["update"]["fix"].clone()),
        (json!(false), json!("run `brew upgrade mailtriage`"))
    );
    // The top-level `ready` keeps its meaning.
    assert_eq!(d["ready"], true);

    set_updates(&env.root.join("mailtriage.json"), "notify");
    let d = env.doctor();
    assert_eq!(d["update"]["ready"], true);
    assert_eq!(d["update"]["mode"], "notify");
    assert!(env.server.requests().is_empty());
}

/// A tray script next to `exe` printing `mailtriage-tray VERSION`; each
/// `--version` run appends a line to `probes` next to it.
fn tray_next_to(exe: &Path, version: &str) -> PathBuf {
    let dir = exe.parent().unwrap();
    write_tool(
        dir,
        "mailtriage-tray",
        &format!(
            "#!/bin/sh\necho probe >> '{}'\necho 'mailtriage-tray {version}'\n",
            dir.join("probes").display()
        ),
    );
    dir.join("mailtriage-tray")
}

/// The tray next to the service's executable (a sandbox script, never
/// `target/debug/mailtriage-tray`): its `--version`, and `available` from
/// the cached release, which says nothing when an older version recorded
/// it (no `mailtriage-tray` key). `doctor` shares the block.
#[test]
fn status_and_doctor_describe_the_tray_next_to_the_services_executable() {
    let env = Env::new();
    let exe = env.executable("svc/bin", "0.0.1");
    env.service_file(&exe);
    let u = env.status();
    assert_eq!(u.get("tray"), None, "no tray file: {u}");

    let tray = tray_next_to(&exe, "0.0.1");
    let release = |archives: Value| json!({"version":"9.9.9","release_url":"u","published_at":null,"archives":archives,"sums":null});
    let cache = |archives: Value| json!({"schema_version": 1, "release": release(archives), "checked_at": "2026-11-03T07:00:00Z"});
    env.write_cache(&cache(json!({"mailtriage": null, "mailtriage-tray": null})));
    let expected = json!({"path": tray, "installed": "0.0.1", "available": true});
    assert_eq!(env.status()["tray"], expected);
    assert_eq!(env.doctor()["update"]["tray"], expected);

    env.write_cache(&cache(json!({"mailtriage": null})));
    assert_eq!(
        env.status()["tray"],
        json!({"path": tray, "installed": "0.0.1", "available": false})
    );
    env.write_cache(&json!({"schema_version": 1}));
    assert_eq!(env.status()["tray"]["available"], false);
    tray_next_to(&exe, "9.9.9");
    env.write_cache(&cache(json!({"mailtriage": null, "mailtriage-tray": null})));
    assert_eq!(
        env.status()["tray"],
        json!({"path": tray, "installed": "9.9.9", "available": false})
    );
    write_tool(
        exe.parent().unwrap(),
        "mailtriage-tray",
        "#!/bin/sh\nexit 1\n",
    );
    assert_eq!(
        env.status()["tray"],
        json!({"path": tray, "installed": null, "available": false})
    );
    assert!(env.server.requests().is_empty());
}

/// R11-8: `service status` for every account probes a tray they share once.
#[test]
fn every_accounts_status_probes_a_shared_tray_once() {
    let env = Env::new();
    let path = env.root.join("mailtriage.json");
    let mut config: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    config["accounts"]["home"] = config["accounts"]["work"].clone();
    fs::write(&path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    let exe = env.executable("svc/bin", "0.0.1");
    env.service_file_for("work", &exe);
    env.service_file_for("home", &exe);
    let tray = tray_next_to(&exe, "0.0.1");
    let (code, v, stderr) = run(env.command().args(["service", "status", "--json"]));
    assert_eq!(code, Some(0), "{v} {stderr}");
    let services = v["services"].as_array().unwrap();
    assert_eq!(services.len(), 2, "{v}");
    for s in services {
        assert_eq!(s["update"]["tray"]["path"], json!(tray), "{s}");
        assert_eq!(s["update"]["tray"]["installed"], "0.0.1", "{s}");
    }
    let probes = fs::read_to_string(exe.parent().unwrap().join("probes")).unwrap();
    assert_eq!(probes.lines().count(), 1, "{probes}");
}

/// Phase B: the version that ran the last pass.
#[test]
fn status_shows_the_version_of_the_last_pass() {
    let env = Env::new();
    let (code, _, _) = run(env.command().args(["sync", "--account", "work", "--json"]));
    assert_eq!(code, Some(0));
    let (code, v, stderr) =
        run(env
            .command()
            .args(["service", "status", "--account", "work", "--json"]));
    assert_eq!(code, Some(0), "{v} {stderr}");
    assert_eq!(v["service"]["last_pass"]["version"], RUNNING);
}
