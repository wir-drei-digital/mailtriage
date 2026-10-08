//! One tray per user, one categories window per config, and the windows
//! the tray started. Locks live in the cache directory; their file
//! descriptors close on `exec` (Rust opens files close-on-exec).
use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::Path,
};

/// The tray's lock file in the cache directory.
pub const TRAY_LOCK: &str = "tray.lock";

/// The variable that hands the window PIDs to a re-executed tray.
pub const WINDOWS_ENV: &str = "MAILTRIAGE_TRAY_WINDOWS";

/// An exclusive lock on `path`; `None` while another process holds it.
pub fn try_lock(path: &Path) -> io::Result<Option<File>> {
    if let Some(dir) = path.parent() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(dir)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    Ok(file.try_lock_exclusive().ok().map(|()| file))
}

/// The tray's lock, `tray.lock`.
pub fn tray_lock(cache: &Path) -> io::Result<Option<File>> {
    try_lock(&cache.join(TRAY_LOCK))
}

/// `editor-<first 16 hex of SHA-256 of the absolute config path>`.
pub fn editor_name(config: &Path) -> String {
    let digest = Sha256::digest(config.as_os_str().as_encoded_bytes());
    format!("editor-{}", &format!("{digest:x}")[..16])
}

/// The window's lock for one config.
pub enum EditorLock {
    /// This process holds it; dropping the file releases it.
    Taken(File),
    /// Another window holds it; its PID when the `.pid` file has one.
    HeldBy(Option<u32>),
}

/// Takes the window lock for `config` and records this PID next to it.
pub fn editor_lock(cache: &Path, config: &Path) -> io::Result<EditorLock> {
    let name = editor_name(config);
    let pid_file = cache.join(format!("{name}.pid"));
    match try_lock(&cache.join(format!("{name}.lock")))? {
        Some(file) => {
            fs::write(&pid_file, std::process::id().to_string())?;
            Ok(EditorLock::Taken(file))
        }
        None => Ok(EditorLock::HeldBy(
            fs::read_to_string(&pid_file)
                .ok()
                .and_then(|p| p.trim().parse().ok()),
        )),
    }
}

/// Brings the running window with `pid` to the front.
#[cfg(target_os = "macos")]
pub fn bring_to_front(pid: u32) -> bool {
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid as libc::pid_t).is_some_and(
        |app| app.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows),
    )
}

#[cfg(not(target_os = "macos"))]
pub fn bring_to_front(_pid: u32) -> bool {
    false
}

/// The categories windows the tray started. Each refresh reaps exactly
/// these PIDs with `waitpid(pid, WNOHANG)`, never `waitpid(-1, …)`, which
/// could take the exit status of a command a worker waits for.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Windows {
    pids: Vec<u32>,
}

impl Windows {
    /// The PIDs a previous image of the tray passed on. Only `1..=i32::MAX`
    /// count: for `waitpid`, 0 means any child in this process group, and a
    /// larger value turns negative as a `pid_t`, which means any child of
    /// another group (`u32::MAX` becomes -1: any child at all).
    pub fn from_env(value: Option<&str>) -> Self {
        Self {
            pids: value
                .unwrap_or_default()
                .split(',')
                .filter_map(|p| p.trim().parse().ok())
                .filter(|&pid: &u32| i32::try_from(pid).is_ok_and(|pid| pid > 0))
                .collect(),
        }
    }

    pub fn track(&mut self, pid: u32) {
        self.pids.push(pid);
    }

    pub fn pids(&self) -> &[u32] {
        &self.pids
    }

    /// `MAILTRIAGE_TRAY_WINDOWS` for a re-executed tray.
    pub fn env_value(&self) -> String {
        self.pids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Reaps the windows that ended; returns their PIDs.
    pub fn reap(&mut self) -> Vec<u32> {
        let mut ended = vec![];
        self.pids.retain(|&pid| {
            let mut status = 0;
            // SAFETY: waitpid writes only to `status`; WNOHANG never blocks.
            let result = unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG) };
            if result == 0 {
                return true;
            }
            // Ended, or not (any longer) our child.
            ended.push(pid);
            false
        });
        ended
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only PIDs that `waitpid` takes as one process.
    #[test]
    fn only_single_process_pids_come_from_the_environment() {
        let windows = Windows::from_env(Some("12, 0,4294967295,2147483648,2147483647,x,,-5,34"));
        assert_eq!(windows.pids(), [12, 2147483647, 34]);
        assert!(Windows::from_env(None).pids().is_empty());
    }
}
