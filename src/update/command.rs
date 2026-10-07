//! `mailtriage update [--check]`: refresh the release information, then
//! report it or install the candidate: the CLI first, then a
//! `mailtriage-tray` next to it. Needs no config.
use super::{
    cache::{install_key, Cache},
    check::{self, Reservation},
    github::{Endpoint, Net},
    install::{self, Hooks, InstallLock, Job, Outcome},
    installed_tray,
    platform::{self, Blocker, FileIdentity},
    release::{self, CachedRelease},
    report, service_files, version, CLI, TRAY,
};
use crate::service::err;
use anyhow::Result;
use semver::Version;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

const BUSY: &str = "another update is running";

/// Runs `update`; `check_only` is `--check`. Errors carry the exit code:
/// 3 for network, release, archive and replaceability problems of the
/// CLI, 5 when another update held the installation lock. A failed tray
/// part keeps the CLI's result and adds top-level `"partial": true`, which
/// the CLI turns into exit 4.
pub fn run(check_only: bool, hooks: &dyn Hooks) -> Result<Value> {
    let path = platform::installation_path().map_err(|e| err(3, e))?;
    let cache =
        Cache::for_user().ok_or_else(|| err(3, "cannot find the update cache: HOME is not set"))?;
    let net = Net::new(Endpoint::from_env())
        .map_err(|e| err(3, format!("cannot start an HTTPS client: {e}")))?;
    let checked =
        check::refresh(&net, &cache, Reservation::BestEffort).map_err(|e| err(3, e.to_string()))?;
    hooks.at("checked").map_err(|e| err(3, e.to_string()))?;
    let running = version::running();
    let installed = platform::probe(&path, CLI).ok();
    let blocker = platform::blocker(&path, release::platform());
    let tray = installed_tray(&path);
    if check_only {
        let latest = checked.release.as_ref().map(|r| r.version.clone());
        let available = match (&latest, &installed) {
            (Some(latest), Some(installed)) => semver::Version::parse(latest)
                .is_ok_and(|latest| version::is_newer(&latest, installed)),
            _ => false,
        };
        let mut out = json!({"schema_version": 1, "update": {
            "current": version::RUNNING,
            "installed": installed.map(|v| v.to_string()),
            "latest": latest,
            "available": available,
            "release_url": checked.release.as_ref().map(|r| r.release_url.clone()),
            "published_at": checked.release.as_ref().and_then(|r| r.published_at.clone()),
            "checked_at": checked.checked_at,
            "install": install_block(&path, blocker),
            "warnings": checked.warnings,
        }});
        if let Some(tray) = tray {
            let found = platform::probe(&tray, TRAY).ok();
            out["update"]["tray"] =
                report::tray_block(&tray, found.as_ref(), checked.release.as_ref());
        }
        return Ok(out);
    }
    let release = checked
        .release
        .clone()
        .ok_or_else(|| err(3, "no stable release of mailtriage was found"))?;
    let candidate = semver::Version::parse(&release.version)
        .map_err(|_| err(3, "the release version is invalid"))?;
    let dir = path.parent().unwrap_or(Path::new("/"));
    let mut warnings = checked.warnings;
    // Held from the CLI's install through the tray's, when they need it.
    let mut lock = None;
    let baseline = installed.clone().unwrap_or_else(|| running.clone());
    let mut out = if !version::is_newer(&candidate, &baseline) {
        current(&path, &baseline)
    } else {
        if let Some(blocker) = blocker {
            return Err(err(3, blocker.refusal(&path)));
        }
        let held = install::lock(dir, install::update_lock_wait())
            .map_err(|e| err(3, format!("{e:#}")))?
            .ok_or_else(|| err(5, BUSY))?;
        let job = Job {
            net: &net,
            component: CLI,
            path: &path,
            release: &release,
            fallback: &running,
            cache: Some(&cache),
            hooks,
        };
        let outcome =
            install::install(&job, &held, &mut || Ok(())).map_err(|e| err(3, format!("{e:#}")))?;
        lock = Some(held);
        match outcome {
            Outcome::Current { installed } => {
                current(&path, installed.as_ref().unwrap_or(&running))
            }
            Outcome::Installed(done) => {
                warnings.extend(done.warnings);
                json!({"schema_version": 1, "update": {
                    "action": "updated",
                    "from": done.from.unwrap_or(running).to_string(),
                    "to": done.to.to_string(),
                    "path": path,
                    "previous_path": done.previous_path,
                    "services": services(&path),
                }})
            }
        }
    };
    if let Some(tray) = tray {
        let part = TrayPart {
            net: &net,
            cache: &cache,
            release: &release,
            candidate: &candidate,
            path: &tray,
            hooks,
        };
        let result = part.update(lock.take(), &mut warnings)?;
        if result["action"] == "failed" {
            out["partial"] = json!(true);
        }
        out["update"]["tray"] = result;
    }
    drop(lock);
    out["update"]["warnings"] = json!(warnings);
    Ok(out)
}

fn current(path: &Path, version: &semver::Version) -> Value {
    json!({"schema_version": 1, "update": {
        "action": "current",
        "from": version.to_string(),
        "to": version.to_string(),
        "path": path,
    }})
}

/// The tray's part of `update`, after the CLI's: the tray's own version is
/// its baseline, never the CLI's.
struct TrayPart<'a> {
    net: &'a Net,
    cache: &'a Cache,
    release: &'a CachedRelease,
    candidate: &'a Version,
    /// The tray's installation path (canonical).
    path: &'a Path,
    hooks: &'a dyn Hooks,
}

impl TrayPart<'_> {
    /// `update.tray`: `{action, from, to, error}`.
    /// - `skipped` when `--version` fails (nothing downloaded; `from` and
    ///   `to` are `null`);
    /// - `current` when the release is not newer (`from` = `to` = its
    ///   version), whatever the release holds;
    /// - `updated` from the version read under the installation lock (else
    ///   the one read just before) to the release;
    /// - `failed` from the tray's version to the release, with the error,
    ///   recorded with the update spec's backoff.
    ///
    /// `lock` is the installation lock the CLI's install already holds; when
    /// it is `None` the tray takes it, and a busy lock is exit 5. Cache
    /// writes that fail add a warning starting with `cannot write the
    /// update cache`.
    fn update(&self, lock: Option<InstallLock>, warnings: &mut Vec<String>) -> Result<Value> {
        let key = install_key(self.path);
        let unrecorded = |e: anyhow::Error| {
            format!(
                "cannot write the update cache: {e:#}; the entry of {} was not recorded",
                self.path.display()
            )
        };
        let installed = match platform::probe(self.path, TRAY) {
            Ok(found) => found,
            Err(cause) => {
                let message = TRAY.does_not_run(&cause);
                let identity = FileIdentity::read(self.path).ok();
                if let Err(e) = self.cache.record_skip(&key, &message, identity) {
                    warnings.push(unrecorded(e));
                }
                return Ok(result("skipped", None, None, Some(&message)));
            }
        };
        let read = |version: &Version, warnings: &mut Vec<String>| {
            let identity = FileIdentity::read(self.path).ok();
            if let Err(e) = self.cache.record_version(&key, Some(version), identity) {
                warnings.push(unrecorded(e));
            }
            result("current", Some(version), Some(version), None)
        };
        if !version::is_newer(self.candidate, &installed) {
            return Ok(read(&installed, warnings));
        }
        let failed = |message: String, warnings: &mut Vec<String>| {
            if let Err(e) = self.cache.record_install_failure(&key, &message) {
                warnings.push(unrecorded(e));
            }
            result(
                "failed",
                Some(&installed),
                Some(self.candidate),
                Some(&message),
            )
        };
        if let Some(blocker) = platform::blocker(self.path, release::platform()) {
            return Ok(failed(blocker.refusal(self.path), warnings));
        }
        let held = match lock {
            Some(held) => held,
            None => {
                let dir = self.path.parent().unwrap_or(Path::new("/"));
                match install::lock(dir, install::update_lock_wait()) {
                    Ok(Some(held)) => held,
                    Ok(None) => return Err(err(5, BUSY)),
                    Err(e) => return Ok(failed(format!("{e:#}"), warnings)),
                }
            }
        };
        let job = Job {
            net: self.net,
            component: TRAY,
            path: self.path,
            release: self.release,
            fallback: &installed,
            cache: Some(self.cache),
            hooks: self.hooks,
        };
        let outcome = install::install(&job, &held, &mut || Ok(()));
        drop(held);
        Ok(match outcome {
            Ok(Outcome::Installed(done)) => {
                warnings.extend(
                    done.warnings
                        .into_iter()
                        .map(|w| format!("{}: {w}", TRAY.name)),
                );
                let from = done.from.unwrap_or(installed);
                result("updated", Some(&from), Some(&done.to), None)
            }
            Ok(Outcome::Current { installed: found }) => {
                read(&found.unwrap_or(installed), warnings)
            }
            Err(e) => failed(format!("{e:#}"), warnings),
        })
    }
}

fn result(
    action: &str,
    from: Option<&Version>,
    to: Option<&Version>,
    error: Option<&str>,
) -> Value {
    json!({
        "action": action,
        "from": from.map(ToString::to_string),
        "to": to.map(ToString::to_string),
        "error": error,
    })
}

/// `install` of `update --check`: whether the binary at `path` is replaceable.
pub fn install_block(path: &Path, blocker: Option<Blocker>) -> Value {
    json!({
        "path": path,
        "replaceable": blocker.is_none(),
        "reason": blocker.map(Blocker::reason),
        "fix": blocker.map(|b| b.fix(path)),
    })
}

/// Every service file mailtriage wrote for this user, and whether it runs
/// the binary at `path` (canonical paths compared).
fn services(path: &Path) -> Vec<Value> {
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from);
    let (Some(manager), Some(home)) = (service_files::platform_manager(), home) else {
        return vec![];
    };
    service_files::list(manager, &home)
        .into_iter()
        .map(|file| {
            let same_binary = file
                .executable
                .as_deref()
                .and_then(|exe| fs::canonicalize(exe).ok())
                .is_some_and(|exe| exe == path);
            json!({
                "account": file.account,
                "manager": manager.name(),
                "unit_path": file.unit_path,
                "executable": file.executable,
                "same_binary": same_binary,
            })
        })
        .collect()
}
