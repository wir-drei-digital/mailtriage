//! Coordination between setup mutations and managed startup. The installer
//! holds the exclusive guard until its download child has stopped and the
//! receipt is written. Normal startup holds a shared guard through readiness.
use super::Layout;
use fs2::FileExt;
use std::{fs, io};

const OWNER_ENV: &str = "MAILTRIAGE_KEV_INSTALLATION_OWNER";

fn open(layout: &Layout) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(layout.installation_lock())
}

fn busy(layout: &Layout) -> io::Error {
    io::Error::other(format!("local Kev installation is in progress in {}; wait for setup to finish, then run `mailtriage setup --update` again",layout.root().display()))
}

pub(crate) fn startup(layout: &Layout) -> io::Result<fs::File> {
    let file = open(layout)?;
    FileExt::try_lock_shared(&file).map_err(|e| {
        if e.kind() == io::ErrorKind::WouldBlock {
            busy(layout)
        } else {
            e
        }
    })?;
    Ok(file)
}

pub(crate) struct Guard {
    _file: fs::File,
    layout: Layout,
    owner: String,
}

impl Guard {
    pub(crate) fn acquire(layout: &Layout) -> io::Result<Self> {
        fs::create_dir_all(layout.root())?;
        let file = open(layout)?;
        file.try_lock_exclusive().map_err(|e| {
            if e.kind() == io::ErrorKind::WouldBlock {
                busy(layout)
            } else {
                e
            }
        })?;
        let owner = uuid::Uuid::new_v4().to_string();
        fs::write(layout.installation_owner(), &owner)?;
        Ok(Self {
            _file: file,
            layout: layout.clone(),
            owner,
        })
    }

    /// Inherit the same exclusive open-file lock into the bounded mutation
    /// supervisor. It owns the lock independently if setup disappears.
    pub(crate) fn retain_for_mutation(
        &self,
        command: &mut std::process::Command,
    ) -> io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::{fd::AsRawFd, unix::process::CommandExt};
            let fd = self._file.as_raw_fd();
            command.arg("--guard-fd").arg(fd.to_string());
            // SAFETY: fcntl is async-signal-safe; only the forked child's FD
            // flags change. The parent retains this descriptor until status().
            unsafe {
                command.pre_exec(move || set_inherited(fd, true));
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = command;
            Err(io::Error::other("local Kev mutation requires Unix"))
        }
    }

    pub(crate) fn authorize(
        &self,
        layout: &Layout,
        command: &mut std::process::Command,
    ) -> io::Result<()> {
        if layout != &self.layout {
            return Err(io::Error::other(
                "installation guard belongs to another layout",
            ));
        }
        command.env(OWNER_ENV, &self.owner);
        Ok(())
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.layout.installation_owner());
    }
}

/// Only the download child launched with the current exclusive installer's
/// fresh ticket can start under that guard. A stale ticket never crosses
/// another install, and standalone --allow-download uses the normal guard.
pub(crate) fn supervisor_start(
    layout: &Layout,
    allow_download: bool,
) -> io::Result<Option<fs::File>> {
    if allow_download {
        if let Ok(owner) = std::env::var(OWNER_ENV) {
            validate_owner(layout, &owner)?;
            return Ok(None);
        }
    }
    startup(layout).map(Some)
}

pub(crate) fn clear_authorization(command: &mut std::process::Command) {
    command.env_remove(OWNER_ENV);
}

pub(super) fn validate_owner(layout: &Layout, owner: &str) -> io::Result<()> {
    let current = fs::read_to_string(layout.installation_owner())?;
    if owner != current.as_str() || owner.is_empty() {
        return Err(busy(layout));
    }
    let file = open(layout)?;
    match FileExt::try_lock_shared(&file) {
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(()),
        Err(e) => Err(e),
        Ok(()) => Err(io::Error::other(
            "Kev installer no longer holds its installation guard",
        )),
    }
}

#[cfg(unix)]
pub(super) fn set_inherited(fd: i32, inherited: bool) -> io::Result<()> {
    unsafe extern "C" {
        fn fcntl(fd: i32, command: i32, ...) -> i32;
    }
    // POSIX F_GETFD=1, F_SETFD=2, FD_CLOEXEC=1 on supported Unix targets.
    // SAFETY: calls use a live owned descriptor and integer FD flags only.
    let flags = unsafe { fcntl(fd, 1) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let flags = if inherited { flags & !1 } else { flags | 1 };
    if unsafe { fcntl(fd, 2, flags) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Read-only inspection: exclusive install/mutation ownership, distinct from
/// shared normal startup guards. Missing files are a fresh, idle layout.
pub(crate) fn in_progress(layout: &Layout) -> io::Result<bool> {
    let file = match fs::OpenOptions::new()
        .read(true)
        .open(layout.installation_lock())
    {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("local Kev installation lock is not a regular file; check its path before mailtriage setup --update"));
    }
    match FileExt::try_lock_shared(&file) {
        Ok(()) => Ok(false),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(true),
        Err(e) => Err(e),
    }
}

#[cfg(unix)]
pub(super) fn copy_guard(fd: i32) -> io::Result<fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    unsafe extern "C" {
        fn fcntl(fd: i32, command: i32, ...) -> i32;
    }
    // POSIX F_DUPFD=0. Duplication validates the source descriptor and gives
    // this function a new owned descriptor, without adopting a borrowed FD.
    let owned = unsafe { fcntl(fd, 0, 3) };
    if owned < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fcntl just returned this live descriptor, owned by this call.
    let file = unsafe { fs::File::from_raw_fd(owned) };
    set_inherited(file.as_raw_fd(), false)?;
    set_inherited(fd, false)?;
    Ok(file)
}
