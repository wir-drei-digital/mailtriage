//! `mailtriage himalaya install`: a tested pimalaya release, installed
//! privately for mailtriage into `<data>/mailtriage/himalaya/<version>/`.
//! It never touches any other `himalaya`.
use super::protected;
use crate::{
    engine::versions::{self, Tested},
    process::Ending,
    service::{err, err_kind, ErrorKind},
    update::{
        archive,
        github::{self, Endpoint, Net},
        install::{self, Scratch},
        platform::{self, FileIdentity},
        release,
    },
};
use anyhow::Result;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

/// `$XDG_DATA_HOME` when it is absolute, else `~/.local/share` (on macOS
/// too); `None` without either.
pub fn data_dir(xdg_data_home: Option<&OsStr>, home: Option<&Path>) -> Option<PathBuf> {
    match xdg_data_home.map(Path::new).filter(|p| p.is_absolute()) {
        Some(xdg) => Some(xdg.to_path_buf()),
        None => home
            .filter(|h| !h.as_os_str().is_empty())
            .map(|h| h.join(".local/share")),
    }
}

/// This user's data directory, from `XDG_DATA_HOME` and `HOME`.
pub fn default_data_dir() -> Option<PathBuf> {
    data_dir(
        std::env::var_os("XDG_DATA_HOME").as_deref(),
        std::env::var_os("HOME").map(PathBuf::from).as_deref(),
    )
}

/// The directories below the data directory that hold one version.
fn tail(version: &str) -> [&str; 3] {
    ["mailtriage", "himalaya", version]
}

/// Where `himalaya install` puts `version`: `<data>/mailtriage/himalaya/<version>/himalaya`.
pub fn binary_path(data: &Path, version: &str) -> PathBuf {
    let mut path = data.to_path_buf();
    path.extend(tail(version));
    path.join("himalaya")
}

/// pimalaya's platform name for mailtriage's release platform.
pub fn platform_of(platform: Option<&str>) -> Option<&'static str> {
    match platform? {
        "macos-arm64" => Some("aarch64-darwin"),
        "linux-amd64" => Some("x86_64-linux"),
        "linux-arm64" => Some("aarch64-linux"),
        _ => None,
    }
}

/// What `install` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// `installed`, or `current` when a matching copy was already there.
    pub action: &'static str,
    pub version: String,
    pub path: PathBuf,
}

impl Installed {
    pub fn json(&self) -> Value {
        json!({"schema_version": 1, "himalaya": {
            "action": self.action,
            "version": self.version,
            "path": self.path,
        }})
    }
}

/// `mailtriage himalaya install [--version X.Y.Z]` with this user's data
/// directory and the release endpoint (the loopback override in tests).
pub fn run(version: Option<&str>) -> Result<Value> {
    let data = default_data_dir().ok_or_else(|| {
        err(
            3,
            "cannot find the data directory: set HOME, or XDG_DATA_HOME to an absolute path",
        )
    })?;
    let net = Net::new(Endpoint::from_env())
        .map_err(|e| err(3, format!("cannot start an HTTPS client: {e}")))?;
    Ok(install(version, &net, &data)?.json())
}

/// Installs `version` (default: the newest tested one) into `data`. Errors
/// carry exit codes: 2 for a version that is not tested or a platform
/// without a release, 3 for an unsafe directory (reason
/// `unsafe_permissions`), the network, the checksum, the archive or a
/// binary that does not run.
pub fn install(version: Option<&str>, net: &Net, data: &Path) -> Result<Installed> {
    let tested = match version {
        Some(v) => versions::find(v).ok_or_else(|| {
            err(
                2,
                format!(
                    "Himalaya {v} is not a tested version (tested: {})",
                    versions::listed()
                ),
            )
        })?,
        None => versions::newest(),
    };
    let platform = platform_of(release::platform()).ok_or_else(|| {
        err(
            2,
            "pimalaya publishes no Himalaya mailtriage can use for this platform; install a tested Himalaya yourself",
        )
    })?;
    let dir = protected::prepare_below(data, &tail(&tested.version)).map_err(unsafe_or_io)?;
    let lock = install::lock(&dir, install::update_lock_wait())
        .map_err(|e| err(3, format!("{e:#}")))?
        .ok_or_else(|| err(3, "another himalaya install is running; try again"))?;
    install::remove_leftovers(&dir);
    let path = dir.join("himalaya");
    let installed = |action| Installed {
        action,
        version: tested.version.clone(),
        path: path.clone(),
    };
    let existing = fs::symlink_metadata(&path).is_ok_and(|m| m.is_file());
    if existing && probe(&path, tested).is_ok() {
        return Ok(installed("current"));
    }
    let asset = format!("himalaya.{platform}.tgz");
    let url = net.endpoint().himalaya_asset_url(&tested.version, &asset);
    let bytes = net
        .download(url.as_str(), 0, github::MAX_ASSET_BYTES)
        .map_err(|e| {
            err(
                3,
                format!("cannot download {asset} of v{}: {e}", tested.version),
            )
        })?;
    let expected = tested
        .sha256(platform)
        .ok_or_else(|| err(3, format!("no digest for {asset}")))?;
    if install::hex(&Sha256::digest(&bytes)) != expected {
        return Err(err(
            3,
            format!("checksum mismatch for {asset} of v{}", tested.version),
        ));
    }
    let binary = archive::extract(&bytes, "himalaya", &archive::HIMALAYA)
        .map_err(|e| err(3, format!("{asset}: {e}")))?;
    drop(bytes);
    let mut scratch = Scratch::default();
    let staged = scratch.add(dir.join(format!(
        "{}{}.tmp",
        install::TEMP_PREFIX,
        uuid::Uuid::new_v4()
    )));
    install::write_staged(&staged, &binary)
        .map_err(|e| err(3, format!("cannot write {}: {e}", staged.display())))?;
    let identity = FileIdentity::read(&staged)?;
    probe(&staged, tested).map_err(|cause| {
        err(
            3,
            format!("the downloaded Himalaya does not run here: {cause}"),
        )
    })?;
    if FileIdentity::read(&staged).ok() != Some(identity) {
        return Err(err(
            3,
            "the downloaded Himalaya changed before it was installed; try again",
        ));
    }
    fs::rename(&staged, &path)
        .map_err(|e| err(3, format!("cannot install {}: {e}", path.display())))?;
    scratch.keep();
    if let Ok(dir) = fs::File::open(&dir) {
        let _ = dir.sync_all();
    }
    drop(lock);
    Ok(installed("installed"))
}

/// The rule's refusal as exit 3 with reason `unsafe_permissions`; any other
/// problem with the directory as exit 3.
fn unsafe_or_io(error: anyhow::Error) -> anyhow::Error {
    match protected::unsafe_dir(&error) {
        Some(found) => err_kind(
            3,
            ErrorKind::UnsafePermissions,
            format!("unsafe_permissions: {found}; {}", found.fix()),
        ),
        None => err(3, format!("{error:#}")),
    }
}

/// `program --version` names `tested` with IMAP; otherwise why not.
fn probe(program: &Path, tested: &Tested) -> Result<(), String> {
    let out = platform::run_version(program).map_err(|_| "could not start".to_owned())?;
    match out.ending {
        Ending::TimedOut => Err("timed out".into()),
        Ending::Overflowed => Err("printed too much".into()),
        Ending::Exited(status) if !status.success() => {
            Err(format!("exited with {}", status.code().unwrap_or(-1)))
        }
        Ending::Exited(_) => match versions::check_version_output(&out.stdout) {
            Ok((_, found)) if found.version == tested.version => Ok(()),
            Ok((line, _)) => Err(format!("printed {line:?}")),
            Err(untested) => Err(untested.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_data_directory_follows_xdg_on_every_platform() {
        let home = Some(Path::new("/h"));
        assert_eq!(
            data_dir(Some(OsStr::new("/x")), home),
            Some(PathBuf::from("/x"))
        );
        assert_eq!(
            data_dir(Some(OsStr::new("rel")), home),
            Some(PathBuf::from("/h/.local/share"))
        );
        assert_eq!(data_dir(None, home), Some(PathBuf::from("/h/.local/share")));
        assert_eq!(data_dir(None, Some(Path::new(""))), None);
        assert_eq!(data_dir(None, None), None);
        assert_eq!(
            binary_path(Path::new("/x"), "2.2.1"),
            PathBuf::from("/x/mailtriage/himalaya/2.2.1/himalaya")
        );
    }

    #[test]
    fn each_release_platform_has_a_pimalaya_platform() {
        assert_eq!(platform_of(Some("macos-arm64")), Some("aarch64-darwin"));
        assert_eq!(platform_of(Some("linux-amd64")), Some("x86_64-linux"));
        assert_eq!(platform_of(Some("linux-arm64")), Some("aarch64-linux"));
        assert_eq!(platform_of(None), None);
        for platform in ["macos-arm64", "linux-amd64", "linux-arm64"] {
            let name = platform_of(Some(platform)).unwrap();
            assert!(versions::PLATFORMS.contains(&name), "{name}");
        }
    }
}
