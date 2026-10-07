//! The `update` block of `service status` and `doctor`: the service's
//! executable, what it prints for `--version`, and what the cache says.
//! Reads only; no network.
use super::{
    cache::{self, Cache},
    platform::{self, Blocker},
    release, service_files, version, CLI,
};
use crate::{
    domain::UpdateMode,
    system_service::{self, Context, Manager},
};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// The manager and service file path of `account`'s service on this
/// platform; `None` where no service manager is found.
pub fn unit_of(account: &str) -> Option<(Manager, PathBuf)> {
    Context::detect()
        .ok()
        .map(|ctx| (ctx.manager, ctx.unit_path(account)))
}

/// The block for the service whose file is `unit` (manager and path), or,
/// without a decodable service file, for the binary running this command.
pub fn update_block(mode: UpdateMode, unit: Option<(Manager, PathBuf)>) -> Value {
    describe(mode, unit).0
}

/// `doctor`'s block: `update_block` plus `ready`, false only when the mode
/// is `auto` and the executable is not replaceable; then also `fix`.
pub fn doctor_block(mode: UpdateMode, unit: Option<(Manager, PathBuf)>) -> Value {
    let (mut block, blocked) = describe(mode, unit);
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
) -> (Value, Option<(PathBuf, Blocker)>) {
    let executable = unit.and_then(|(manager, path)| decoded_executable(manager, &path));
    let target = match &executable {
        Some(exe) => Some(fs::canonicalize(exe).unwrap_or_else(|_| exe.clone())),
        None => platform::installation_path().ok(),
    };
    let installed = target.as_deref().and_then(|p| platform::probe(p, CLI).ok());
    let file = Cache::for_user().map(|c| c.read()).unwrap_or_default();
    let latest = file.release.as_ref().map(|r| r.version.clone());
    let available = match (latest.as_deref().map(semver::Version::parse), &installed) {
        (Some(Ok(latest)), Some(installed)) => version::is_newer(&latest, installed),
        _ => false,
    };
    let last_error = target
        .as_deref()
        .and_then(|p| file.installs.get(&cache::install_key(p)))
        .and_then(|e| e.last_error.clone())
        .or(file.last_check_error);
    let blocker = target
        .as_deref()
        .and_then(|p| platform::blocker(p, release::platform()));
    let block = json!({
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
    (block, target.zip(blocker))
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
