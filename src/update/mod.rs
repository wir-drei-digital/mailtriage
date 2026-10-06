//! Automatic updates from GitHub Releases (spec:
//! docs/superpowers/specs/2026-10-06-auto-update-design.md): release
//! information, installing a release, restarting `watch` onto a replaced
//! binary, and reporting what is installed.
pub mod cache;
pub mod check;
pub mod github;
pub mod release;
pub mod schedule;
pub mod version;

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

/// Every component a release check records archives for.
pub const COMPONENTS: &[Component] = &[CLI];

impl Component {
    /// `NAME-vX.Y.Z-PLATFORM.tar.gz`.
    pub fn archive_name(self, version: &semver::Version, platform: &str) -> String {
        format!("{}-v{version}-{platform}.tar.gz", self.name)
    }
}
