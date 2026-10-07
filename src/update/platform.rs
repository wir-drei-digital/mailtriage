//! Facts about an installed file: its identity, whether mailtriage may
//! replace it, and what its `--version` prints.
use super::{version, Component};
use crate::process::{self, Ending};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Component as PathPart, Path},
    time::Duration,
};

/// How long a `--version` run may take, and how much output it may print.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_MAX_STDOUT: usize = 4096;
const PROBE_MAX_STDERR: usize = 4096;

/// A file's identity: device, inode, size, modification time, change time
/// and mode. A rename over the path, a rewrite or a `chmod` changes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileIdentity {
    pub dev: u64,
    pub ino: u64,
    pub size: u64,
    pub mtime: i64,
    pub mtime_nsec: i64,
    pub ctime: i64,
    pub ctime_nsec: i64,
    pub mode: u32,
}

impl FileIdentity {
    pub fn of(meta: &fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            dev: meta.dev(),
            ino: meta.ino(),
            size: meta.size(),
            mtime: meta.mtime(),
            mtime_nsec: meta.mtime_nsec(),
            ctime: meta.ctime(),
            ctime_nsec: meta.ctime_nsec(),
            mode: meta.mode(),
        }
    }

    /// The identity of the file `path` names (symlinks followed).
    pub fn read(path: &Path) -> std::io::Result<Self> {
        fs::metadata(path).map(|meta| Self::of(&meta))
    }
}

/// The installation path: the running executable's canonical path. On
/// Linux, when that file was replaced since the start (its link then ends
/// in ` (deleted)`), `argv[0]` when it is absolute and exists.
pub fn installation_path() -> Result<std::path::PathBuf, String> {
    if let Ok(path) = std::env::current_exe().and_then(fs::canonicalize) {
        return Ok(path);
    }
    std::env::args_os()
        .next()
        .map(std::path::PathBuf::from)
        .filter(|argv0| argv0.is_absolute())
        .and_then(|argv0| fs::canonicalize(argv0).ok())
        .ok_or_else(|| "cannot find the file this mailtriage runs from".to_owned())
}

/// Why mailtriage will not replace a binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocker {
    UnsupportedPlatform,
    ManagedByHomebrew,
    ManagedByNix,
    UnsafePermissions,
    NotWritable,
}

impl Blocker {
    /// `install.reason` in output.
    pub fn reason(self) -> &'static str {
        match self {
            Blocker::UnsupportedPlatform => "unsupported_platform",
            Blocker::ManagedByHomebrew => "managed_by_homebrew",
            Blocker::ManagedByNix => "managed_by_nix",
            Blocker::UnsafePermissions => "unsafe_permissions",
            Blocker::NotWritable => "not_writable",
        }
    }

    /// `install.fix`: one sentence for the binary at `path`.
    pub fn fix(self, path: &Path) -> String {
        let dir = path.parent().unwrap_or(Path::new("/")).display();
        match self {
            Blocker::UnsupportedPlatform => {
                "releases have no archive for this platform; build mailtriage from source, or set \"updates\" to \"off\"".into()
            }
            Blocker::ManagedByHomebrew => "run `brew upgrade mailtriage`".into(),
            Blocker::ManagedByNix => {
                "update mailtriage through nix (for example `nix profile upgrade`)".into()
            }
            Blocker::UnsafePermissions => format!(
                "install mailtriage into a directory that only this user owns and can write, such as ~/.local/bin, or set \"updates\" to \"notify\" (now: {})",
                path.display()
            ),
            Blocker::NotWritable => {
                format!("make {dir} writable for this user, or set \"updates\" to \"notify\"")
            }
        }
    }
}

/// Why the binary at `path` (canonical) may not be replaced on `platform`
/// (`None`: no release archives for this build), checked in this order:
/// no archive, Homebrew, nix, ownership and permissions, writability.
pub fn blocker(path: &Path, platform: Option<&str>) -> Option<Blocker> {
    if platform.is_none() {
        return Some(Blocker::UnsupportedPlatform);
    }
    if let Some(manager) = package_manager(path) {
        return Some(manager);
    }
    let dir = path.parent()?;
    let uid = crate::system_service::current_uid();
    if !safe(path, uid, false) || !safe(dir, uid, true) {
        return Some(Blocker::UnsafePermissions);
    }
    (!writable(dir)).then_some(Blocker::NotWritable)
}

/// Homebrew (a path component `Cellar` followed by `mailtriage`) or nix
/// (under `/nix/store/`).
pub fn package_manager(path: &Path) -> Option<Blocker> {
    if path.starts_with("/nix/store") {
        return Some(Blocker::ManagedByNix);
    }
    let parts: Vec<_> = path.components().collect();
    parts
        .windows(2)
        .any(|pair| {
            pair[0] == PathPart::Normal("Cellar".as_ref())
                && pair[1] == PathPart::Normal("mailtriage".as_ref())
        })
        .then_some(Blocker::ManagedByHomebrew)
}

/// Owned by `uid`, the right kind, and not writable by group or others.
fn safe(path: &Path, uid: u32, directory: bool) -> bool {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path).is_ok_and(|meta| {
        let kind = if directory {
            meta.is_dir()
        } else {
            meta.is_file()
        };
        kind && meta.uid() == uid && meta.mode() & 0o022 == 0
    })
}

/// Whether this process may create files in `dir` (access(2), so nothing
/// is created).
fn writable(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    unsafe extern "C" {
        fn access(path: *const std::ffi::c_char, mode: i32) -> i32;
    }
    const W_OK: i32 = 2;
    const X_OK: i32 = 1;
    let Ok(path) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `path` is a valid NUL-terminated string for the call's duration.
    unsafe { access(path.as_ptr(), W_OK | X_OK) == 0 }
}

/// Runs `program --version` with a 10 s limit and returns the version it
/// printed as `<component> <semver>`. The error names the cause: `could
/// not start`, `killed by signal N`, `timed out`, `exited with N`, or
/// `printed "…"`, each with the first line of stderr when there is one.
pub fn probe(program: &Path, component: Component) -> Result<Version, String> {
    let out = process::run_captured(
        program,
        &["--version"],
        PROBE_TIMEOUT,
        PROBE_MAX_STDOUT,
        PROBE_MAX_STDERR,
    )
    .map_err(|_| "could not start".to_owned())?;
    let cause = match out.ending {
        Ending::TimedOut => "timed out".to_owned(),
        Ending::Overflowed => "printed too much".to_owned(),
        Ending::Exited(status) => {
            use std::os::unix::process::ExitStatusExt;
            if let Some(signal) = status.signal() {
                format!("killed by signal {signal}")
            } else if status.success() {
                match version::parse_version_output(component.name, &out.stdout) {
                    Some(found) => return Ok(found),
                    None => format!(
                        "printed {:?}",
                        shorten(&String::from_utf8_lossy(&out.stdout), 80)
                    ),
                }
            } else {
                format!("exited with {}", status.code().unwrap_or(-1))
            }
        }
    };
    let stderr = String::from_utf8_lossy(&out.stderr);
    match stderr.lines().map(str::trim).find(|l| !l.is_empty()) {
        Some(line) => Err(format!("{cause}; stderr: {}", shorten(line, 200))),
        None => Err(cause),
    }
}

fn shorten(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::CLI;
    use std::os::unix::fs::PermissionsExt;

    fn script(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn package_managers_are_recognised_by_path() {
        assert_eq!(
            package_manager(Path::new(
                "/opt/homebrew/Cellar/mailtriage/0.3.0/bin/mailtriage"
            )),
            Some(Blocker::ManagedByHomebrew)
        );
        assert_eq!(
            package_manager(Path::new(
                "/usr/local/Cellar/mailtriage/0.3.0/bin/mailtriage"
            )),
            Some(Blocker::ManagedByHomebrew)
        );
        assert_eq!(
            package_manager(Path::new("/nix/store/abc-mailtriage-0.3.0/bin/mailtriage")),
            Some(Blocker::ManagedByNix)
        );
        for plain in [
            "/Users/alice/.local/bin/mailtriage",
            "/opt/Cellar/other/mailtriage",
            "/opt/mailtriage/Cellar/bin/mailtriage",
            "/nix/storefront/mailtriage",
        ] {
            assert_eq!(package_manager(Path::new(plain)), None, "{plain}");
        }
    }

    #[test]
    fn a_private_directory_is_replaceable_and_a_group_writable_one_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir(&bin).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        let exe = script(&bin, "mailtriage", "echo mailtriage 0.1.0");
        let exe = fs::canonicalize(exe).unwrap();
        assert_eq!(blocker(&exe, Some("linux-amd64")), None);
        assert_eq!(blocker(&exe, None), Some(Blocker::UnsupportedPlatform));
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o775)).unwrap();
        assert_eq!(
            blocker(&exe, Some("linux-amd64")),
            Some(Blocker::UnsafePermissions)
        );
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o757)).unwrap();
        assert_eq!(
            blocker(&exe, Some("linux-amd64")),
            Some(Blocker::UnsafePermissions)
        );
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        if crate::system_service::current_uid() != 0 {
            fs::set_permissions(&bin, fs::Permissions::from_mode(0o555)).unwrap();
            assert_eq!(
                blocker(&exe, Some("linux-amd64")),
                Some(Blocker::NotWritable)
            );
            fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(
            Blocker::NotWritable.fix(Path::new("/opt/mailtriage/mailtriage")),
            "make /opt/mailtriage writable for this user, or set \"updates\" to \"notify\""
        );
        assert_eq!(
            Blocker::ManagedByHomebrew.fix(&exe),
            "run `brew upgrade mailtriage`"
        );
    }

    #[test]
    fn identity_changes_with_content_and_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = script(dir.path(), "a", "true");
        let first = FileIdentity::read(&path).unwrap();
        assert_eq!(FileIdentity::read(&path).unwrap(), first);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let chmodded = FileIdentity::read(&path).unwrap();
        assert_ne!(chmodded, first);
        let other = script(dir.path(), "b", "true");
        fs::rename(&other, &path).unwrap();
        assert_ne!(FileIdentity::read(&path).unwrap().ino, chmodded.ino);
    }

    #[test]
    fn probes_name_why_a_binary_does_not_run() {
        let dir = tempfile::tempdir().unwrap();
        let ok = script(dir.path(), "ok", "echo 'mailtriage 0.3.0'");
        assert_eq!(probe(&ok, CLI), Ok(Version::new(0, 3, 0)));
        let wrong = script(
            dir.path(),
            "wrong",
            "echo 'mailtriage 0.3'; echo 'some detail' >&2",
        );
        assert_eq!(
            probe(&wrong, CLI),
            Err("printed \"mailtriage 0.3\\n\"; stderr: some detail".into())
        );
        let failing = script(
            dir.path(),
            "failing",
            "echo '/lib/libc.so.6: version GLIBC_2.39 not found' >&2; exit 1",
        );
        assert_eq!(
            probe(&failing, CLI),
            Err("exited with 1; stderr: /lib/libc.so.6: version GLIBC_2.39 not found".into())
        );
        let killed = script(dir.path(), "killed", "kill -9 $$");
        assert_eq!(probe(&killed, CLI), Err("killed by signal 9".into()));
        let plain = dir.path().join("plain");
        fs::write(&plain, "not a program").unwrap();
        assert_eq!(probe(&plain, CLI), Err("could not start".into()));
        assert_eq!(
            probe(&dir.path().join("missing"), CLI),
            Err("could not start".into())
        );
    }
}
