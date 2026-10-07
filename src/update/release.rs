//! Choosing the candidate release, the platform's archive name and the
//! `SHA256SUMS` line for it.
use super::{
    github::{AssetJson, ReleaseJson},
    version, Component,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The release information the cache keeps: the candidate and, per
/// component, its archive for this platform (`None` when the release has
/// none, exactly one being required).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedRelease {
    pub version: String,
    pub release_url: String,
    pub published_at: Option<String>,
    pub archives: BTreeMap<String, Option<Asset>>,
    pub sums: Option<Asset>,
}

impl CachedRelease {
    /// Whether the check that recorded this release knew `component`. A
    /// check records an entry for every component it knows (`None` when
    /// the release lacks its archive), so a missing key means an older
    /// version made the check and says nothing about the release.
    pub fn knows(&self, component: Component) -> bool {
        self.archives.contains_key(component.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// The platform part of archive names for this build, or `None` for a
/// target without release archives.
pub fn platform() -> Option<&'static str> {
    platform_for(
        std::env::consts::ARCH,
        std::env::consts::OS,
        cfg!(target_env = "gnu"),
    )
}

/// `linux-amd64` (x86_64 Linux GNU), `linux-arm64` (aarch64 Linux GNU),
/// `macos-arm64` (aarch64 macOS); nothing else.
pub fn platform_for(arch: &str, os: &str, gnu: bool) -> Option<&'static str> {
    match (arch, os, gnu) {
        ("x86_64", "linux", true) => Some("linux-amd64"),
        ("aarch64", "linux", true) => Some("linux-arm64"),
        ("aarch64", "macos", _) => Some("macos-arm64"),
        _ => None,
    }
}

/// The highest stable release: drafts, prereleases and tags that are not
/// exactly `vX.Y.Z` are ignored. `None` when no release remains.
pub fn select(
    releases: &[ReleaseJson],
    platform: Option<&str>,
    components: &[Component],
) -> Option<CachedRelease> {
    let (version, release) = releases
        .iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter_map(|r| Some((version::parse_tag(&r.tag_name)?, r)))
        .max_by(|(a, _), (b, _)| a.cmp_precedence(b))?;
    let unique = |name: &str| -> Option<Asset> {
        let mut found = release.assets.iter().filter(|a| a.name == name);
        match (found.next(), found.next()) {
            (Some(asset), None) => Some(asset_of(asset)),
            _ => None,
        }
    };
    let archives = components
        .iter()
        .map(|c| {
            let asset = platform.and_then(|p| unique(&c.archive_name(&version, p)));
            (c.name.to_owned(), asset)
        })
        .collect();
    Some(CachedRelease {
        version: version.to_string(),
        release_url: release.html_url.clone(),
        published_at: release.published_at.clone(),
        archives,
        sums: unique("SHA256SUMS"),
    })
}

fn asset_of(asset: &AssetJson) -> Asset {
    Asset {
        name: asset.name.clone(),
        url: asset.browser_download_url.clone(),
        size: asset.size,
    }
}

/// The SHA-256 `SHA256SUMS` lists for `name`. Every non-empty line must be
/// `<64 lowercase hex>  <name>`, and exactly one must name `name`.
pub fn expected_sha256(sums: &str, name: &str) -> Result<String, String> {
    let mut found = Vec::new();
    for line in sums.lines().filter(|l| !l.is_empty()) {
        let (hash, file) = line
            .split_once("  ")
            .filter(|(hash, file)| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    && !file.is_empty()
            })
            .ok_or_else(|| "SHA256SUMS has a malformed line".to_owned())?;
        if file == name {
            found.push(hash.to_owned());
        }
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(format!("SHA256SUMS has no line for {name}")),
        _ => Err(format!("SHA256SUMS has several lines for {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::{CLI, COMPONENTS, TRAY};

    fn release(tag: &str, draft: bool, prerelease: bool, assets: &[&str]) -> ReleaseJson {
        ReleaseJson {
            tag_name: tag.into(),
            draft,
            prerelease,
            html_url: format!("https://github.com/r/releases/tag/{tag}"),
            published_at: Some("2026-11-02T09:00:00Z".into()),
            assets: assets
                .iter()
                .map(|name| AssetJson {
                    name: (*name).into(),
                    browser_download_url: format!(
                        "https://github.com/r/releases/download/{tag}/{name}"
                    ),
                    size: 10,
                })
                .collect(),
        }
    }

    #[test]
    fn each_supported_target_has_an_archive_name() {
        assert_eq!(platform_for("x86_64", "linux", true), Some("linux-amd64"));
        assert_eq!(platform_for("aarch64", "linux", true), Some("linux-arm64"));
        assert_eq!(platform_for("aarch64", "macos", false), Some("macos-arm64"));
        for (arch, os, gnu) in [
            ("x86_64", "macos", false),
            ("x86_64", "linux", false),
            ("x86_64", "windows", false),
            ("arm", "linux", true),
            ("aarch64", "freebsd", false),
        ] {
            assert_eq!(platform_for(arch, os, gnu), None, "{arch} {os}");
        }
        let v = semver::Version::new(0, 3, 0);
        assert_eq!(
            CLI.archive_name(&v, "macos-arm64"),
            "mailtriage-v0.3.0-macos-arm64.tar.gz"
        );
    }

    #[test]
    fn the_highest_stable_release_wins_wherever_it_is_listed() {
        let list = vec![
            release("v1.2.3-rc.1", false, true, &[]),
            release("v9.0.0", true, false, &[]),
            release("v8.0.0", false, true, &[]),
            release("1.2.3", false, false, &[]),
            release("v1.2", false, false, &[]),
            release("v01.2.3", false, false, &[]),
            release("v0.9.9", false, false, &[]),
            release(
                "v0.10.0",
                false,
                false,
                &["mailtriage-v0.10.0-linux-amd64.tar.gz", "SHA256SUMS"],
            ),
            release("v0.2.0", false, false, &[]),
        ];
        let chosen = select(&list, Some("linux-amd64"), &[CLI]).unwrap();
        assert_eq!(chosen.version, "0.10.0");
        assert_eq!(
            chosen.release_url,
            "https://github.com/r/releases/tag/v0.10.0"
        );
        assert_eq!(
            chosen.archives["mailtriage"].as_ref().unwrap().name,
            "mailtriage-v0.10.0-linux-amd64.tar.gz"
        );
        assert_eq!(chosen.sums.as_ref().unwrap().name, "SHA256SUMS");
        assert!(select(&list[..6], Some("linux-amd64"), &[CLI]).is_none());
    }

    #[test]
    fn a_missing_or_doubled_asset_is_recorded_as_none() {
        let doubled = release(
            "v1.0.0",
            false,
            false,
            &[
                "mailtriage-v1.0.0-macos-arm64.tar.gz",
                "mailtriage-v1.0.0-macos-arm64.tar.gz",
                "SHA256SUMS",
            ],
        );
        let chosen = select(&[doubled], Some("macos-arm64"), &[CLI]).unwrap();
        assert_eq!(chosen.archives["mailtriage"], None);
        assert!(chosen.sums.is_some());
        let bare = release("v1.0.0", false, false, &[]);
        let chosen = select(std::slice::from_ref(&bare), Some("macos-arm64"), &[CLI]).unwrap();
        assert_eq!(
            (chosen.archives["mailtriage"].clone(), chosen.sums),
            (None, None)
        );
        let unsupported = select(&[bare], None, &[CLI]).unwrap();
        assert_eq!(unsupported.archives["mailtriage"], None);
    }

    /// Ruling (a): every known component gets an entry, `None` when the
    /// release has no archive for it; a key no check wrote is unknown.
    #[test]
    fn every_component_gets_an_entry() {
        let both = release(
            "v1.0.0",
            false,
            false,
            &[
                "mailtriage-v1.0.0-linux-arm64.tar.gz",
                "mailtriage-tray-v1.0.0-linux-arm64.tar.gz",
                "SHA256SUMS",
            ],
        );
        let chosen = select(&[both], Some("linux-arm64"), COMPONENTS).unwrap();
        assert_eq!(
            chosen.archives["mailtriage-tray"].as_ref().unwrap().name,
            "mailtriage-tray-v1.0.0-linux-arm64.tar.gz"
        );
        assert_eq!(
            chosen.archives["mailtriage"].as_ref().unwrap().name,
            "mailtriage-v1.0.0-linux-arm64.tar.gz"
        );
        assert!(chosen.knows(TRAY) && chosen.knows(CLI));
        let cli_only = release(
            "v1.0.0",
            false,
            false,
            &["mailtriage-v1.0.0-linux-arm64.tar.gz", "SHA256SUMS"],
        );
        let chosen = select(&[cli_only], Some("linux-arm64"), COMPONENTS).unwrap();
        assert_eq!(chosen.archives.get("mailtriage-tray"), Some(&None));
        assert!(chosen.knows(TRAY));
        let older = CachedRelease {
            archives: [("mailtriage".to_owned(), None)].into_iter().collect(),
            ..chosen
        };
        assert!(older.knows(CLI) && !older.knows(TRAY));
    }

    #[test]
    fn sha256sums_needs_exactly_one_well_formed_line() {
        let hash = "a".repeat(64);
        let name = "mailtriage-v1.0.0-linux-amd64.tar.gz";
        let other = format!("{}  mailtriage-v1.0.0-macos-arm64.tar.gz\n", "b".repeat(64));
        let line = format!("{hash}  {name}\n");
        assert_eq!(
            expected_sha256(&format!("{other}{line}"), name),
            Ok(hash.clone())
        );
        assert_eq!(
            expected_sha256(&other, name),
            Err(format!("SHA256SUMS has no line for {name}"))
        );
        assert_eq!(
            expected_sha256(&format!("{line}{line}"), name),
            Err(format!("SHA256SUMS has several lines for {name}"))
        );
        for bad in [
            format!("{}  {name}\n", "A".repeat(64)),
            format!("{}  {name}\n", "a".repeat(63)),
            format!("{hash} {name}\n"),
            format!("{hash}  \n"),
            format!("{line}garbage\n"),
        ] {
            assert_eq!(
                expected_sha256(&bad, name),
                Err("SHA256SUMS has a malformed line".to_owned()),
                "{bad:?}"
            );
        }
    }
}
