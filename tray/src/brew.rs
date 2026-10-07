//! Homebrew kegs, as the CLI's `distribution::brew` sees them (the tray
//! does not depend on the CLI crate; both are tested with the same cases).
//! Brew keeps each version in `<prefix>/Cellar/mailtriage/<version>/bin/`,
//! `brew upgrade` deletes the old keg, and `<prefix>/opt/mailtriage` points
//! to the current one. The tray records, runs and re-executes `opt` paths.
use std::path::{Path, PathBuf};

/// `<prefix>/opt/mailtriage/bin/<name>` for a path shaped
/// `<prefix>/Cellar/mailtriage/<version>/bin/<name>`, by its shape alone.
pub fn opt_path(canonical: &Path) -> Option<PathBuf> {
    let name = canonical.file_name()?;
    let bin = canonical.parent()?;
    let keg = bin.parent()?;
    let formula = keg.parent()?;
    let cellar = formula.parent()?;
    let shaped = bin.file_name()? == "bin"
        && keg.file_name().is_some()
        && formula.file_name()? == "mailtriage"
        && cellar.file_name()? == "Cellar";
    if !shaped {
        return None;
    }
    Some(cellar.parent()?.join("opt/mailtriage/bin").join(name))
}

/// The launch path of the program whose canonical path is `canonical`: its
/// `opt` path when it has one and that exists, else `canonical` itself.
pub fn launch_path(canonical: &Path) -> PathBuf {
    opt_path(canonical)
        .filter(|opt| opt.exists())
        .unwrap_or_else(|| canonical.to_path_buf())
}

/// Whether two canonical paths are one installation: equal, or both in a
/// keg of the same prefix (one may be a deleted older keg).
pub fn same_installation(a: &Path, b: &Path) -> bool {
    a == b || opt_path(a).is_some_and(|opt| opt_path(b) == Some(opt))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_keg_path_has_an_opt_path() {
        assert_eq!(
            opt_path(Path::new(
                "/opt/homebrew/Cellar/mailtriage/1.2.3/bin/mailtriage-tray"
            )),
            Some(PathBuf::from(
                "/opt/homebrew/opt/mailtriage/bin/mailtriage-tray"
            ))
        );
        for plain in [
            "/Users/a/.local/bin/mailtriage-tray",
            "/opt/homebrew/Cellar/himalaya/2.2.1/bin/himalaya",
            "/opt/homebrew/Cellar/mailtriage/1.2.3/libexec/mailtriage-tray",
            "/Cellar/mailtriage/bin/mailtriage-tray",
        ] {
            assert_eq!(opt_path(Path::new(plain)), None, "{plain}");
        }
    }

    #[test]
    fn kegs_of_one_prefix_are_one_installation() {
        let old = Path::new("/opt/homebrew/Cellar/mailtriage/1.2.3/bin/mailtriage-tray");
        let new = Path::new("/opt/homebrew/Cellar/mailtriage/1.2.4/bin/mailtriage-tray");
        let other = Path::new("/usr/local/Cellar/mailtriage/1.2.4/bin/mailtriage-tray");
        let plain = Path::new("/Users/a/.local/bin/mailtriage-tray");
        assert!(same_installation(old, new));
        assert!(same_installation(plain, plain));
        assert!(!same_installation(old, other));
        assert!(!same_installation(
            plain,
            Path::new("/Users/b/.local/bin/mailtriage-tray")
        ));
        assert!(!same_installation(old, plain));
    }
}
