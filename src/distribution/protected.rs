//! The protected path rule: a directory that holds a program mailtriage
//! installs and runs must not be changeable by anyone but this user or root.
//! It and every ancestor up to `/`, each read without following symlinks,
//! must be no symlink, owned by the user or root, and not writable by group
//! or others. A world-writable directory with the sticky bit (such as
//! `/tmp`) passes when the next directory down the path belongs to the user.
use crate::setup::shell_line;
use anyhow::{Context, Result};
use std::{
    fmt, fs, io,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

/// Why a directory fails the rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    Symlink,
    NotDirectory,
    /// Owned by this other uid.
    Owner(u32),
    /// Owned by this uid (root included) where only the user's own will do:
    /// `self install`'s directory.
    NotYours(u32),
    /// Writable by group or others; its permission bits.
    Writable(u32),
}

/// A directory that fails the rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsafe {
    pub dir: PathBuf,
    pub why: Why,
}

impl fmt::Display for Unsafe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let dir = self.dir.display();
        match self.why {
            Why::Symlink => write!(f, "{dir} is a symlink"),
            Why::NotDirectory => write!(f, "{dir} is not a directory"),
            Why::Owner(uid) => write!(f, "{dir} belongs to uid {uid}, not to you or root"),
            Why::NotYours(uid) => write!(f, "{dir} belongs to uid {uid}, not to you"),
            Why::Writable(mode) => write!(
                f,
                "{dir} is writable by group or others (mode {:o})",
                mode & 0o7777
            ),
        }
    }
}

impl std::error::Error for Unsafe {}

impl Unsafe {
    /// What makes the directory pass.
    pub fn fix(&self) -> String {
        match self.why {
            Why::Writable(_) => format!(
                "run {}",
                shell_line(&[Path::new("chmod"), Path::new("go-w"), &self.dir])
            ),
            Why::Symlink => "use the directory the link points to".to_owned(),
            Why::NotDirectory | Why::Owner(_) | Why::NotYours(_) => {
                "use a directory you own".to_owned()
            }
        }
    }
}

/// What the rule reads of a path, without following a final symlink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Meta {
    pub symlink: bool,
    pub dir: bool,
    pub uid: u32,
    pub mode: u32,
}

fn lstat(path: &Path) -> io::Result<Meta> {
    let meta = fs::symlink_metadata(path)?;
    Ok(Meta {
        symlink: meta.file_type().is_symlink(),
        dir: meta.is_dir(),
        uid: meta.uid(),
        mode: meta.mode(),
    })
}

fn uid() -> u32 {
    crate::system_service::current_uid()
}

/// Checks `path` (absolute and existing) and every ancestor for this user.
pub fn check(path: &Path) -> Result<()> {
    check_with(path, uid(), lstat, false)
}

/// The rule for `uid`, reading each path with `stat`, from `/` down; an
/// error reading one is returned as is. With `creating`, the caller is about
/// to create a directory in `path`, so a sticky, world-writable `path`
/// passes for now: its next directory down will be the user's.
pub fn check_with(
    path: &Path,
    uid: u32,
    stat: impl Fn(&Path) -> io::Result<Meta>,
    creating: bool,
) -> Result<()> {
    let mut chain: Vec<&Path> = path.ancestors().collect();
    chain.reverse();
    for (at, dir) in chain.iter().enumerate() {
        let meta = stat(dir).with_context(|| format!("cannot read {}", dir.display()))?;
        let fail = |why| -> Result<()> {
            Err(Unsafe {
                dir: dir.to_path_buf(),
                why,
            }
            .into())
        };
        if meta.symlink {
            return fail(Why::Symlink);
        }
        if !meta.dir {
            return fail(Why::NotDirectory);
        }
        if meta.uid != uid && meta.uid != 0 {
            return fail(Why::Owner(meta.uid));
        }
        if meta.mode & 0o022 == 0 {
            continue;
        }
        let sticky_world = meta.mode & 0o1002 == 0o1002;
        let child_is_users = match chain.get(at + 1) {
            Some(child) => stat(child).is_ok_and(|c| c.uid == uid && !c.symlink),
            None => creating,
        };
        if !(sticky_world && child_is_users) {
            return fail(Why::Writable(meta.mode & 0o7777));
        }
    }
    Ok(())
}

/// Creates the directory `path` with mode 0755 whatever the umask; an
/// existing entry is left as it is (the rule checks it).
fn make_dir(path: &Path) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) => fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("cannot set the mode of {}", path.display())),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e).with_context(|| format!("cannot create {}", path.display())),
    }
}

/// Makes the absolute directory `dir` exist under the rule and returns its
/// canonical path: its deepest existing ancestor is canonicalized and
/// checked before anything is created in it; the missing directories below
/// it are created one at a time with mode 0755 whatever the umask; then the
/// whole canonical path is checked.
pub fn prepare(dir: &Path) -> Result<PathBuf> {
    prepare_dir(dir, false)
}

/// `prepare`; with `creating`, the caller creates more directories in
/// `dir` and checks the full path afterwards.
fn prepare_dir(dir: &Path, creating: bool) -> Result<PathBuf> {
    let mut existing = dir;
    let mut missing = Vec::new();
    while fs::metadata(existing).is_err() {
        missing.push(
            existing
                .file_name()
                .with_context(|| format!("cannot create {}", dir.display()))?,
        );
        existing = existing
            .parent()
            .with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let mut path = fs::canonicalize(existing)
        .with_context(|| format!("cannot resolve {}", existing.display()))?;
    check_with(&path, uid(), lstat, creating || !missing.is_empty())?;
    for part in missing.iter().rev() {
        path.push(part);
        make_dir(&path)?;
    }
    check_with(&path, uid(), lstat, creating)?;
    Ok(path)
}

/// Makes `base.join(tail…)` exist under the rule and returns it: `base`
/// (absolute) as `prepare` does, then each component of `tail`, read
/// without following a symlink, is checked when it exists, before anything
/// is created in it, and created with mode 0755 when it is missing; then
/// the whole path is checked.
pub fn prepare_below(base: &Path, tail: &[&str]) -> Result<PathBuf> {
    let mut dir = prepare_dir(base, !tail.is_empty())?;
    for part in tail {
        dir.push(part);
        if fs::symlink_metadata(&dir).is_ok() {
            check_with(&dir, uid(), lstat, true)?;
        } else {
            make_dir(&dir)?;
        }
    }
    check(&dir)?;
    Ok(dir)
}

/// The `Unsafe` inside `error`, when the rule refused a directory.
pub fn unsafe_dir(error: &anyhow::Error) -> Option<&Unsafe> {
    error.downcast_ref::<Unsafe>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const ME: u32 = 501;

    fn meta(uid: u32, mode: u32) -> Meta {
        Meta {
            symlink: false,
            dir: true,
            uid,
            mode,
        }
    }

    fn rule(path: &str, table: &[(&str, Meta)]) -> Result<(), Unsafe> {
        let table: HashMap<PathBuf, Meta> =
            table.iter().map(|(p, m)| (PathBuf::from(p), *m)).collect();
        let stat = |p: &Path| {
            table
                .get(p)
                .copied()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        };
        check_with(Path::new(path), ME, stat, false)
            .map_err(|e| e.downcast::<Unsafe>().expect("an Unsafe"))
    }

    fn root() -> (&'static str, Meta) {
        ("/", meta(0, 0o755))
    }

    #[test]
    fn a_private_chain_passes() {
        assert_eq!(
            rule(
                "/home/me/.local/bin",
                &[
                    root(),
                    ("/home", meta(0, 0o755)),
                    ("/home/me", meta(ME, 0o750)),
                    ("/home/me/.local", meta(ME, 0o700)),
                    ("/home/me/.local/bin", meta(ME, 0o755)),
                ]
            ),
            Ok(())
        );
    }

    #[test]
    fn a_group_writable_ancestor_fails_with_its_directory() {
        let error = rule(
            "/opt/shared/bin",
            &[
                root(),
                ("/opt", meta(0, 0o755)),
                ("/opt/shared", meta(ME, 0o775)),
                ("/opt/shared/bin", meta(ME, 0o755)),
            ],
        )
        .unwrap_err();
        assert_eq!(error.dir, PathBuf::from("/opt/shared"));
        assert_eq!(error.why, Why::Writable(0o775));
        assert_eq!(
            error.to_string(),
            "/opt/shared is writable by group or others (mode 775)"
        );
        assert_eq!(error.fix(), "run chmod go-w /opt/shared");
    }

    #[test]
    fn a_symlink_or_a_stranger_in_the_chain_fails() {
        let mut link = meta(ME, 0o777);
        link.symlink = true;
        let error = rule(
            "/home/me/bin",
            &[root(), ("/home", meta(0, 0o755)), ("/home/me", link)],
        )
        .unwrap_err();
        assert_eq!((error.dir, error.why), ("/home/me".into(), Why::Symlink));
        let error = rule(
            "/home/other/bin",
            &[
                root(),
                ("/home", meta(0, 0o755)),
                ("/home/other", meta(1000, 0o755)),
            ],
        )
        .unwrap_err();
        assert_eq!(error.why, Why::Owner(1000));
    }

    #[test]
    fn a_sticky_world_writable_directory_passes_only_above_the_users_own() {
        let tmp = ("/tmp", meta(0, 0o1777));
        assert_eq!(
            rule("/tmp/mine", &[root(), tmp, ("/tmp/mine", meta(ME, 0o755))]),
            Ok(())
        );
        // Someone else's directory below the sticky one.
        let error = rule(
            "/tmp/theirs/x",
            &[
                root(),
                tmp,
                ("/tmp/theirs", meta(0, 0o755)),
                ("/tmp/theirs/x", meta(ME, 0o755)),
            ],
        )
        .unwrap_err();
        assert_eq!(error.dir, PathBuf::from("/tmp"));
        // The sticky directory itself as the target.
        assert_eq!(
            rule("/tmp", &[root(), tmp]).unwrap_err().dir,
            PathBuf::from("/tmp")
        );
        // World-writable without the sticky bit, or sticky but only
        // group-writable.
        let open = ("/tmp", meta(0, 0o777));
        assert!(rule("/tmp/mine", &[root(), open, ("/tmp/mine", meta(ME, 0o755))]).is_err());
        let group = ("/tmp", meta(0, 0o1775));
        assert!(rule(
            "/tmp/mine",
            &[root(), group, ("/tmp/mine", meta(ME, 0o755))]
        )
        .is_err());
    }

    #[test]
    fn real_directories_are_created_0755_and_checked() {
        let tmp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap();
        // Private whatever the umask, so only the directories below matter.
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        // A sticky, world-writable directory passes above the user's own.
        let sticky = root.join("sticky");
        fs::create_dir(&sticky).unwrap();
        fs::set_permissions(&sticky, fs::Permissions::from_mode(0o1777)).unwrap();
        let made = prepare(&sticky.join("mine/bin")).unwrap();
        assert_eq!(made, sticky.join("mine/bin"));
        for dir in [sticky.join("mine"), sticky.join("mine/bin")] {
            let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o7777;
            assert_eq!(mode, 0o755, "{}", dir.display());
        }
        let error = check(&sticky).unwrap_err();
        assert_eq!(unsafe_dir(&error).unwrap().dir, sticky);
        // A group-writable ancestor: refused before anything is created in it.
        let group = root.join("group");
        fs::create_dir(&group).unwrap();
        fs::set_permissions(&group, fs::Permissions::from_mode(0o775)).unwrap();
        let error = prepare(&group.join("x")).unwrap_err();
        assert_eq!(unsafe_dir(&error).unwrap().dir, group);
        assert!(!group.join("x").exists());
        // A symlink below the base is refused, not followed.
        fs::create_dir(root.join("real")).unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("link")).unwrap();
        let error = prepare_below(&root, &["link", "x"]).unwrap_err();
        assert_eq!(unsafe_dir(&error).unwrap().why, Why::Symlink);
        assert!(!root.join("real/x").exists());
    }
}
