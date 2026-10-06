//! `mailtriage update [--check]`: refresh the release information, then
//! report it or install the candidate. Needs no config.
use super::{
    cache::Cache,
    check::{self, Reservation},
    github::{Endpoint, Net},
    install::{self, Hooks, Job, Outcome},
    platform::{self, Blocker},
    release, service_files, version, CLI,
};
use crate::service::err;
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Runs `update`; `check_only` is `--check`. Errors carry the exit code:
/// 3 for network, release, archive and replaceability problems, 5 when
/// another update held the installation lock.
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
    if check_only {
        let latest = checked.release.as_ref().map(|r| r.version.clone());
        let available = match (&latest, &installed) {
            (Some(latest), Some(installed)) => semver::Version::parse(latest)
                .is_ok_and(|latest| version::is_newer(&latest, installed)),
            _ => false,
        };
        return Ok(json!({"schema_version": 1, "update": {
            "current": version::RUNNING,
            "installed": installed.map(|v| v.to_string()),
            "latest": latest,
            "available": available,
            "release_url": checked.release.as_ref().map(|r| r.release_url.clone()),
            "published_at": checked.release.as_ref().and_then(|r| r.published_at.clone()),
            "checked_at": checked.checked_at,
            "install": install_block(&path, blocker),
            "warnings": checked.warnings,
        }}));
    }
    let release = checked
        .release
        .clone()
        .ok_or_else(|| err(3, "no stable release of mailtriage was found"))?;
    let candidate = semver::Version::parse(&release.version)
        .map_err(|_| err(3, "the release version is invalid"))?;
    let baseline = installed.clone().unwrap_or_else(|| running.clone());
    if !version::is_newer(&candidate, &baseline) {
        return Ok(current(&path, &baseline, checked.warnings));
    }
    if let Some(blocker) = blocker {
        return Err(err(
            3,
            format!(
                "{} cannot be replaced ({}): {}",
                path.display(),
                blocker.reason(),
                blocker.fix(&path)
            ),
        ));
    }
    let dir = path.parent().unwrap_or(Path::new("/"));
    let lock = install::lock(dir, install::update_lock_wait())
        .map_err(|e| err(3, format!("{e:#}")))?
        .ok_or_else(|| err(5, "another update is running"))?;
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
        install::install(&job, &lock, &mut || Ok(())).map_err(|e| err(3, format!("{e:#}")))?;
    drop(lock);
    match outcome {
        Outcome::Current { installed } => Ok(current(
            &path,
            installed.as_ref().unwrap_or(&running),
            checked.warnings,
        )),
        Outcome::Installed(done) => {
            let mut warnings = checked.warnings;
            warnings.extend(done.warnings);
            Ok(json!({"schema_version": 1, "update": {
                "action": "updated",
                "from": done.from.unwrap_or(running).to_string(),
                "to": done.to.to_string(),
                "path": path,
                "previous_path": done.previous_path,
                "warnings": warnings,
                "services": services(&path),
            }}))
        }
    }
}

fn current(path: &Path, version: &semver::Version, warnings: Vec<String>) -> Value {
    json!({"schema_version": 1, "update": {
        "action": "current",
        "from": version.to_string(),
        "to": version.to_string(),
        "path": path,
        "warnings": warnings,
    }})
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
