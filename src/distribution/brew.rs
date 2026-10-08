//! Homebrew kegs. Brew keeps each version in
//! `<prefix>/Cellar/mailtriage/<version>/bin/`, and `brew upgrade` deletes
//! the old keg; `<prefix>/opt/mailtriage` always points to the current one.
//! For a program in a keg, mailtriage records and re-executes that `opt`
//! path (the launch path) instead of the keg path.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn a_keg_path_has_an_opt_path() {
        assert_eq!(
            opt_path(Path::new(
                "/opt/homebrew/Cellar/mailtriage/1.2.3/bin/mailtriage"
            )),
            Some(PathBuf::from("/opt/homebrew/opt/mailtriage/bin/mailtriage"))
        );
        assert_eq!(
            opt_path(Path::new(
                "/home/linuxbrew/.linuxbrew/Cellar/mailtriage/1.2.3_1/bin/mailtriage-tray"
            )),
            Some(PathBuf::from(
                "/home/linuxbrew/.linuxbrew/opt/mailtriage/bin/mailtriage-tray"
            ))
        );
        for plain in [
            "/Users/a/.local/bin/mailtriage",
            "/opt/homebrew/Cellar/himalaya/2.2.1/bin/himalaya",
            "/opt/homebrew/Cellar/mailtriage/1.2.3/libexec/mailtriage",
            "/Cellar/mailtriage/bin/mailtriage",
            "mailtriage",
        ] {
            assert_eq!(opt_path(Path::new(plain)), None, "{plain}");
        }
    }

    #[test]
    fn the_launch_path_is_the_opt_path_only_when_it_exists() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let keg = root.join("Cellar/mailtriage/1.2.3/bin/mailtriage");
        fs::create_dir_all(keg.parent().unwrap()).unwrap();
        fs::write(&keg, "").unwrap();
        assert_eq!(launch_path(&keg), keg);
        fs::create_dir_all(root.join("opt")).unwrap();
        std::os::unix::fs::symlink(
            root.join("Cellar/mailtriage/1.2.3"),
            root.join("opt/mailtriage"),
        )
        .unwrap();
        assert_eq!(
            launch_path(&keg),
            root.join("opt/mailtriage/bin/mailtriage")
        );
        let plain = root.join("bin/mailtriage");
        assert_eq!(launch_path(&plain), plain);
    }
}
