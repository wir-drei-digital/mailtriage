//! Automatic updates from GitHub Releases (spec:
//! docs/superpowers/specs/2026-10-06-auto-update-design.md): release
//! information, installing a release, restarting `watch` onto a replaced
//! binary, and reporting what is installed.
pub mod archive;
pub mod cache;
pub mod check;
pub mod command;
pub mod events;
pub mod github;
pub mod install;
pub mod platform;
pub mod release;
pub mod report;
pub mod restart;
pub mod schedule;
pub mod service_files;
pub mod version;
pub mod watch;

/// The repository releases come from.
pub const REPO: &str = "wir-drei-digital/mailtriage";

/// A program the updater installs. This spec defines `mailtriage`; the tray
/// spec adds `mailtriage-tray`, installed the same way next to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Component {
    /// The key in the cache's `release.archives`, the archive name prefix,
    /// the file name inside the archive and next to the CLI, and the first
    /// word of its `--version` output.
    pub name: &'static str,
}

/// The `mailtriage` CLI.
pub const CLI: Component = Component { name: "mailtriage" };

/// The tray app, installed next to the CLI when its file is there.
pub const TRAY: Component = Component {
    name: "mailtriage-tray",
};

/// Every component a release check records archives for.
pub const COMPONENTS: &[Component] = &[CLI, TRAY];

impl Component {
    /// `NAME-vX.Y.Z-PLATFORM.tar.gz`.
    pub fn archive_name(self, version: &semver::Version, platform: &str) -> String {
        format!("{}-v{version}-{platform}.tar.gz", self.name)
    }

    /// Why this component is skipped: its `--version` failed with `cause`.
    pub fn does_not_run(self, cause: &str) -> String {
        format!("{} does not run here: {cause}", self.name)
    }

    /// Why release `version` cannot be installed for this component: it has
    /// no archive for `platform`. The tray says which archive is missing.
    pub fn missing_archive(self, version: &semver::Version, platform: &str) -> String {
        if self == TRAY {
            format!("release v{version} has no tray archive for {platform}")
        } else {
            format!("release v{version} has no {platform} archive")
        }
    }
}

/// The tray's installation path: `mailtriage-tray` in the directory of
/// the CLI's canonical path.
pub fn tray_path(cli: &std::path::Path) -> std::path::PathBuf {
    cli.with_file_name(TRAY.name)
}

/// `tray_path(cli)` when it is a regular file (not a link, so the path is
/// canonical too): only then is there a tray to describe or update.
pub fn installed_tray(cli: &std::path::Path) -> Option<std::path::PathBuf> {
    let path = tray_path(cli);
    std::fs::symlink_metadata(&path)
        .is_ok_and(|meta| meta.is_file())
        .then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn the_tray_is_the_regular_file_next_to_the_cli() {
        let dir = tempfile::tempdir().unwrap();
        let cli = dir.path().join("mailtriage");
        assert_eq!(tray_path(&cli), dir.path().join("mailtriage-tray"));
        assert_eq!(installed_tray(&cli), None);
        fs::create_dir(dir.path().join("mailtriage-tray")).unwrap();
        assert_eq!(installed_tray(&cli), None, "a directory");
        fs::remove_dir(dir.path().join("mailtriage-tray")).unwrap();
        fs::write(dir.path().join("elsewhere"), "").unwrap();
        std::os::unix::fs::symlink(
            dir.path().join("elsewhere"),
            dir.path().join("mailtriage-tray"),
        )
        .unwrap();
        assert_eq!(installed_tray(&cli), None, "a link");
        fs::remove_file(dir.path().join("mailtriage-tray")).unwrap();
        fs::write(dir.path().join("mailtriage-tray"), "").unwrap();
        assert_eq!(
            installed_tray(&cli),
            Some(dir.path().join("mailtriage-tray"))
        );
        let v = semver::Version::new(9, 9, 9);
        assert_eq!(
            TRAY.archive_name(&v, "linux-amd64"),
            "mailtriage-tray-v9.9.9-linux-amd64.tar.gz"
        );
        assert_eq!(
            TRAY.missing_archive(&v, "linux-amd64"),
            "release v9.9.9 has no tray archive for linux-amd64"
        );
        assert_eq!(
            CLI.missing_archive(&v, "linux-amd64"),
            "release v9.9.9 has no linux-amd64 archive"
        );
        assert_eq!(
            TRAY.does_not_run("exited with 1"),
            "mailtriage-tray does not run here: exited with 1"
        );
    }
}
