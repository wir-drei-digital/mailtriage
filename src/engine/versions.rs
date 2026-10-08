//! The Himalaya versions mailtriage is tested with. `himalaya-versions.json`
//! is the single source: compiled into the binary, parsed once, it lists each
//! tested version, the SHA-256 of pimalaya's release archive per platform,
//! and the mailbox roles that version's `--mailbox` resolution maps.
use semver::Version;
use serde::Deserialize;
use std::{collections::BTreeMap, fmt, sync::OnceLock};

/// The data file as compiled in.
pub const DATA: &str = include_str!("himalaya-versions.json");
/// pimalaya's platform names, one per platform mailtriage releases for.
pub const PLATFORMS: [&str; 3] = ["aarch64-darwin", "x86_64-linux", "aarch64-linux"];
/// Debug builds only: a data file that replaces the compiled one, so tests
/// can serve archives whose digests they know. Release builds never read it.
pub const TEST_OVERRIDE: &str = "MAILTRIAGE_TEST_HIMALAYA_VERSIONS";

/// One tested Himalaya version.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tested {
    /// `X.Y.Z`, without the `v`.
    pub version: String,
    /// Lowercase role name to the mailbox `--mailbox ROLE` opens on IMAP.
    pub roles: BTreeMap<String, String>,
    /// pimalaya platform to `sha256:<64 lowercase hex>` of
    /// `himalaya.<platform>.tgz`.
    pub assets: BTreeMap<String, String>,
}

impl Tested {
    /// The archive's SHA-256 for `platform`, as 64 lowercase hex digits.
    pub fn sha256(&self, platform: &str) -> Option<&str> {
        self.assets.get(platform)?.strip_prefix("sha256:")
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    versions: Vec<Tested>,
}

/// Parses a data file. Every entry needs an exact `X.Y.Z` version, higher
/// than the one before, lowercase role names, and exactly one well-formed
/// digest for each of `PLATFORMS`.
pub fn parse(text: &str) -> Result<Vec<Tested>, String> {
    let file: File = serde_json::from_str(text).map_err(|e| format!("invalid JSON: {e}"))?;
    if file.versions.is_empty() {
        return Err("no versions".into());
    }
    let mut previous: Option<Version> = None;
    for entry in &file.versions {
        let v = &entry.version;
        let version = crate::update::version::parse_tag(&format!("v{v}"))
            .ok_or_else(|| format!("version {v:?} is not X.Y.Z"))?;
        if previous.as_ref().is_some_and(|p| *p >= version) {
            return Err(format!("version {v} is not higher than the one before"));
        }
        previous = Some(version);
        if let Some(role) = entry.roles.keys().find(|r| r.to_lowercase() != **r) {
            return Err(format!("{v}: role {role:?} is not lowercase"));
        }
        if entry.assets.len() != PLATFORMS.len() {
            return Err(format!("{v}: needs a digest for each of {PLATFORMS:?}"));
        }
        for platform in PLATFORMS {
            let hex = entry
                .sha256(platform)
                .ok_or_else(|| format!("{v}: no sha256 digest for {platform}"))?;
            let lower_hex = |b: u8| b.is_ascii_digit() || (b'a'..=b'f').contains(&b);
            if hex.len() != 64 || !hex.bytes().all(lower_hex) {
                return Err(format!("{v}: the {platform} digest is malformed"));
            }
        }
    }
    Ok(file.versions)
}

/// The tested versions, oldest first.
pub fn tested() -> &'static [Tested] {
    static TESTED: OnceLock<Vec<Tested>> = OnceLock::new();
    TESTED.get_or_init(load)
}

#[cfg(debug_assertions)]
fn load() -> Vec<Tested> {
    if let Some(path) = std::env::var_os(TEST_OVERRIDE) {
        let text = std::fs::read_to_string(&path).expect("the test's Himalaya versions file");
        return parse(&text).expect("the test's Himalaya versions file is valid");
    }
    parse(DATA).expect("the compiled Himalaya versions file is valid")
}

#[cfg(not(debug_assertions))]
fn load() -> Vec<Tested> {
    parse(DATA).expect("the compiled Himalaya versions file is valid")
}

/// The entry for `version` (`X.Y.Z`), when it is tested.
pub fn find(version: &str) -> Option<&'static Tested> {
    tested().iter().find(|t| t.version == version)
}

/// The newest tested version: what `himalaya install` installs by default.
pub fn newest() -> &'static Tested {
    tested().last().expect("at least one tested version")
}

/// The tested versions for messages, such as `2.1.0, 2.2.1`.
pub fn listed() -> String {
    tested()
        .iter()
        .map(|t| t.version.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// A Himalaya that mailtriage does not accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Untested {
    /// The first line `himalaya --version` printed.
    pub line: String,
    /// The version that line names (`v` dropped), when it names one.
    pub version: Option<String>,
    /// The version is tested, but the build lacks `+imap`.
    pub no_imap: bool,
}

impl fmt::Display for Untested {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.version, self.no_imap) {
            (Some(v), true) => write!(f, "Himalaya {v} was built without IMAP (+imap)"),
            (Some(v), false) => write!(
                f,
                "Himalaya {v} is not a tested version (tested: {})",
                listed()
            ),
            (None, _) => write!(
                f,
                "Himalaya printed no version mailtriage knows (tested: {})",
                listed()
            ),
        }
    }
}

impl std::error::Error for Untested {}

/// The first line of `himalaya --version` and its tested entry. The line
/// must be `himalaya v<tested version> …` with `+imap` among its words; any
/// other version, a newer patch release included, is refused.
pub fn check_version_output(output: &[u8]) -> Result<(String, &'static Tested), Untested> {
    let text = String::from_utf8_lossy(output);
    let line = text.lines().next().unwrap_or_default().trim().to_owned();
    let mut words = line.split_ascii_whitespace();
    let version = match (words.next(), words.next()) {
        (Some("himalaya"), Some(v)) => v.strip_prefix('v').map(str::to_owned),
        _ => None,
    };
    let imap = words.any(|w| w == "+imap");
    match version.as_deref().and_then(find) {
        Some(tested) if imap => Ok((line, tested)),
        Some(_) => Err(Untested {
            line,
            version,
            no_imap: true,
        }),
        None => Err(Untested {
            line,
            version,
            no_imap: false,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The spec's digests of the two versions the feature shipped with. The
    /// tests check properties of the data file, not its exact list, so that
    /// adding a version keeps them passing.
    const PINNED: [(&str, [(&str, &str); 3]); 2] = [
        (
            "2.1.0",
            [
                (
                    "aarch64-darwin",
                    "a5a787b7c4dbf065408e7772908fc75c799626f4cebab8e9c78fafe3e2fa585c",
                ),
                (
                    "x86_64-linux",
                    "683a2ab8e1534f01e6bda3a69e204d564c31fbfbe20511fc7bc60b67f2e85884",
                ),
                (
                    "aarch64-linux",
                    "c41adab4bc220ba816cdbf865a5df8dc3b358b39ec58b4be0ed2f64e46b1d182",
                ),
            ],
        ),
        (
            "2.2.1",
            [
                (
                    "aarch64-darwin",
                    "a5d97a1f7bbca45e58bddde3f9f17325f614c9cd6d5f17fabf38ddcb030869ab",
                ),
                (
                    "x86_64-linux",
                    "5c5ba2724c162f82d0a0c71b6c03224ed44f8bef7b14ace5da0635d3af2665a0",
                ),
                (
                    "aarch64-linux",
                    "1dc21c3dd6d948929e22e5cae49b9d0b3b30ff88fafd4493fd4460c6252e9d90",
                ),
            ],
        ),
    ];

    /// A newer patch release the data does not list: the newest listed
    /// version with its patch number + 1.
    fn untested_patch() -> String {
        let newest = Version::parse(&newest().version).unwrap();
        let untested = format!("{}.{}.{}", newest.major, newest.minor, newest.patch + 1);
        assert!(find(&untested).is_none(), "{untested} is listed");
        untested
    }

    #[test]
    fn the_data_file_lists_well_formed_digests_oldest_first() {
        let tested = parse(DATA).unwrap();
        let versions: Vec<Version> = tested
            .iter()
            .map(|t| Version::parse(&t.version).unwrap())
            .collect();
        assert!(versions.windows(2).all(|w| w[0] < w[1]), "{versions:?}");
        let names: Vec<&str> = tested.iter().map(|t| t.version.as_str()).collect();
        assert_eq!(listed(), names.join(", "));
        assert_eq!(&newest().version, names.last().unwrap());
        for (version, digests) in PINNED {
            let entry = find(version).unwrap_or_else(|| panic!("{version} is not listed"));
            for (platform, hex) in digests {
                assert_eq!(entry.sha256(platform), Some(hex), "{version} {platform}");
            }
        }
    }

    /// The roles each version's resolver maps for IMAP, read from
    /// Himalaya's source: 2.1.0 resolves aliases only; 2.2.1 resolves
    /// aliases, then the `inbox` role (to `INBOX`), then the literal name.
    #[test]
    fn the_roles_tables_are_pinned() {
        assert!(find("2.1.0").unwrap().roles.is_empty());
        assert_eq!(
            find("2.2.1").unwrap().roles,
            BTreeMap::from([("inbox".to_owned(), "INBOX".to_owned())])
        );
    }

    #[test]
    fn malformed_files_are_refused() {
        let entry = |version: &str, assets: &str| {
            format!(r#"{{"versions":[{{"version":"{version}","roles":{{}},"assets":{assets}}}]}}"#)
        };
        let hex = "a".repeat(64);
        let all = format!(
            r#"{{"aarch64-darwin":"sha256:{hex}","x86_64-linux":"sha256:{hex}","aarch64-linux":"sha256:{hex}"}}"#
        );
        assert!(parse(&entry("2.3.0", &all)).is_ok());
        for (version, assets, why) in [
            ("v2.3.0", all.clone(), "not X.Y.Z"),
            ("2.3", all.clone(), "not X.Y.Z"),
            (
                "2.3.0",
                format!(r#"{{"aarch64-darwin":"sha256:{hex}","x86_64-linux":"sha256:{hex}"}}"#),
                "needs a digest",
            ),
            ("2.3.0", all.replace("sha256:", "sha512:"), "no sha256"),
            ("2.3.0", all.replacen(&hex, &"A".repeat(64), 1), "malformed"),
            ("2.3.0", all.replacen(&hex, &"a".repeat(63), 1), "malformed"),
        ] {
            let error = parse(&entry(version, &assets)).unwrap_err();
            assert!(error.contains(why), "{version} {assets}: {error}");
        }
        let two = format!(
            r#"{{"versions":[{{"version":"2.2.1","roles":{{}},"assets":{all}}},{{"version":"2.1.0","roles":{{}},"assets":{all}}}]}}"#
        );
        assert!(parse(&two).unwrap_err().contains("not higher"));
        let upper = entry("2.3.0", &all).replace(r#""roles":{}"#, r#""roles":{"Inbox":"INBOX"}"#);
        assert!(parse(&upper).unwrap_err().contains("not lowercase"));
        assert!(parse(r#"{"versions":[]}"#).is_err());
        assert!(parse(r#"{"versions":[],"extra":1}"#).is_err());
    }

    #[test]
    fn only_tested_versions_with_imap_are_accepted() {
        for line in [
            "himalaya v2.1.0 +imap\n",
            "himalaya v2.2.1 +smtp +imap +jmap\nbuild: macos aarch64\n",
        ] {
            let (first, tested) = check_version_output(line.as_bytes()).unwrap();
            assert_eq!(first, line.lines().next().unwrap());
            assert!(line.contains(&format!("v{} ", tested.version)));
        }
        let refused = |line: &str| check_version_output(line.as_bytes()).unwrap_err();
        let untested = untested_patch();
        let newer = refused(&format!("himalaya v{untested} +imap\n"));
        assert_eq!(newer.version.as_deref(), Some(untested.as_str()));
        assert_eq!(
            newer.to_string(),
            format!(
                "Himalaya {untested} is not a tested version (tested: {})",
                listed()
            )
        );
        assert!(find("1.2.0").is_none());
        assert_eq!(
            refused("himalaya v1.2.0 +imap\n").to_string(),
            format!(
                "Himalaya 1.2.0 is not a tested version (tested: {})",
                listed()
            )
        );
        let no_imap = refused("himalaya v2.2.1 +smtp\n");
        assert!(no_imap.no_imap);
        assert_eq!(
            no_imap.to_string(),
            "Himalaya 2.2.1 was built without IMAP (+imap)"
        );
        for odd in ["himalaya 2.1.0 +imap\n", "", "himalaya-cli v2.1.0 +imap\n"] {
            let e = refused(odd);
            assert!(!e.no_imap, "{odd:?}");
        }
        // The version is on the first line only.
        assert!(check_version_output(b"something\nhimalaya v2.1.0 +imap\n").is_err());
    }
}
