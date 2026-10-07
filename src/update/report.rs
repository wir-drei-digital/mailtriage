//! The `update` block of `service status` and `doctor`: the service's
//! executable, what it prints for `--version`, the tray next to it, and
//! what the cache says. Reads only; no network.
use super::{
    cache::{self, Cache, CacheFile},
    installed_tray,
    platform::{self, Blocker, FileIdentity},
    release::{self, CachedRelease},
    service_files, version, CLI, TRAY,
};
use crate::{
    domain::UpdateMode,
    system_service::{self, Context, Manager},
};
use semver::Version;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
};

/// The manager and service file path of `account`'s service on this
/// platform; `None` where no service manager is found.
pub fn unit_of(account: &str) -> Option<(Manager, PathBuf)> {
    Context::detect()
        .ok()
        .map(|ctx| (ctx.manager, ctx.unit_path(account)))
}

/// The block for the service whose file is `unit` (manager and path), or,
/// without a decodable service file, for the binary running this command,
/// with what `cache` says (`None`: nothing cached). With a
/// `mailtriage-tray` next to that executable it adds `tray` (`tray_block`).
pub fn update_block(
    mode: UpdateMode,
    unit: Option<(Manager, PathBuf)>,
    cache: Option<Cache>,
) -> Value {
    describe(mode, unit, cache).0
}

/// `doctor`'s block: `update_block` with this user's cache, plus `ready`,
/// false only when the mode is `auto` and the executable is not
/// replaceable; then also `fix`.
pub fn doctor_block(mode: UpdateMode, unit: Option<(Manager, PathBuf)>) -> Value {
    let (mut block, blocked) = describe(mode, unit, Cache::for_user());
    let ready = !(mode == UpdateMode::Auto && block["replaceable"] == false);
    block["ready"] = json!(ready);
    if !ready {
        block["fix"] = json!(match blocked {
            Some((path, blocker)) => blocker.fix(&path),
            None => "set \"updates\" to \"notify\"".to_owned(),
        });
    }
    block
}

/// The block, and the binary and why it may not be replaced.
fn describe(
    mode: UpdateMode,
    unit: Option<(Manager, PathBuf)>,
    cache: Option<Cache>,
) -> (Value, Option<(PathBuf, Blocker)>) {
    let executable = unit.and_then(|(manager, path)| decoded_executable(manager, &path));
    let target = match &executable {
        Some(exe) => Some(fs::canonicalize(exe).unwrap_or_else(|_| exe.clone())),
        None => platform::installation_path().ok(),
    };
    let installed = target.as_deref().and_then(|p| platform::probe(p, CLI).ok());
    let file = cache.map(|c| c.read()).unwrap_or_default();
    let latest = file.release.as_ref().map(|r| r.version.clone());
    let available = match (latest.as_deref().map(semver::Version::parse), &installed) {
        (Some(Ok(latest)), Some(installed)) => version::is_newer(&latest, installed),
        _ => false,
    };
    let last_error = target
        .as_deref()
        .and_then(|p| file.installs.get(&cache::install_key(p)))
        .and_then(|e| e.last_error.clone())
        .or_else(|| file.last_check_error.clone());
    let blocker = target
        .as_deref()
        .and_then(|p| platform::blocker(p, release::platform()));
    let tray = target.as_deref().and_then(installed_tray).map(|tray| {
        let installed = tray_version(&tray, &file);
        tray_block(&tray, installed.as_ref(), file.release.as_ref())
    });
    let mut block = json!({
        "mode": mode.as_str(),
        "executable": executable,
        "installed": installed.map(|v| v.to_string()),
        "latest": latest,
        "available": available,
        "checked_at": file.checked_at,
        "last_error": last_error,
        "replaceable": target.is_some() && blocker.is_none(),
        "reason": blocker.map(Blocker::reason),
    });
    if let Some(tray) = tray {
        block["tray"] = tray;
    }
    (block, target.zip(blocker))
}

/// The tray's installed version, read as `watch` reads it: the version
/// its `installs` entry recorded for this very file (same identity), else
/// its `--version` (`probe_tray`). Nothing is written: status and doctor
/// only read. A running tray asks `service status` every 15 s; this keeps
/// that from starting the tray's binary each time.
fn tray_version(path: &Path, file: &CacheFile) -> Option<Version> {
    let identity = FileIdentity::read(path).ok()?;
    file.installs
        .get(&cache::install_key(path))
        .filter(|entry| entry.identity == Some(identity))
        .and_then(|entry| entry.version.as_deref())
        .and_then(|v| Version::parse(v).ok())
        .or_else(|| probe_tray(path, identity))
}

/// The tray's `--version`, run once per tray file identity and command
/// (process): `service status` describes every account, and their
/// services usually share one executable and so one tray. A file with
/// another identity (a replaced tray) is probed again.
fn probe_tray(path: &Path, identity: FileIdentity) -> Option<Version> {
    type Seen = Vec<(PathBuf, FileIdentity, Option<Version>)>;
    static SEEN: Mutex<Seen> = Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((_, _, found)) = seen.iter().find(|(p, i, _)| p == path && *i == identity) {
        return found.clone();
    }
    let found = platform::probe(path, TRAY).ok();
    seen.push((path.to_owned(), identity, found.clone()));
    found
}

/// `tray` of `update --check`, `service status` and `doctor`: the tray's
/// path, its `--version` (`installed`, `null` when it does not run here)
/// and whether `release` is newer. `available` is also false without a
/// release, and when the release was recorded by a version that did not
/// know the tray (no `mailtriage-tray` key: unknown).
pub fn tray_block(
    path: &Path,
    installed: Option<&Version>,
    release: Option<&CachedRelease>,
) -> Value {
    let available = match (installed, release) {
        (Some(installed), Some(release)) if release.knows(TRAY) => Version::parse(&release.version)
            .is_ok_and(|latest| version::is_newer(&latest, installed)),
        _ => false,
    };
    json!({
        "path": path,
        "installed": installed.map(ToString::to_string),
        "available": available,
    })
}

/// The executable of a service file mailtriage wrote; `None` when there is
/// no such file or it cannot be decoded.
fn decoded_executable(manager: Manager, path: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(path).ok()?;
    if !system_service::is_marked(manager, &text) {
        return None;
    }
    service_files::executable(manager, &text)
}
