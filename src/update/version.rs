//! Versions: SemVer precedence, release tags and `--version` output.
use semver::Version;
use std::cmp::Ordering;

/// The version compiled into this process.
pub const RUNNING: &str = env!("CARGO_PKG_VERSION");

/// `RUNNING`, parsed.
pub fn running() -> Version {
    Version::parse(RUNNING).expect("the package version is SemVer")
}

/// The version of a stable release tag: exactly `vX.Y.Z`, decimal numbers
/// without leading zeros. Anything else (`v1.2.3-rc.1`, `1.2.3`, `v1.2`,
/// `v01.2.3`) is not a stable release.
pub fn parse_tag(tag: &str) -> Option<Version> {
    let parts: Vec<&str> = tag.strip_prefix('v')?.split('.').collect();
    let [major, minor, patch] = parts[..] else {
        return None;
    };
    let number = |part: &str| -> Option<u64> {
        let decimal = !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
        (decimal && (part == "0" || !part.starts_with('0')))
            .then(|| part.parse().ok())
            .flatten()
    };
    Some(Version::new(number(major)?, number(minor)?, number(patch)?))
}

/// The version `program --version` printed: exactly `<program> <semver>`,
/// optionally followed by one newline.
pub fn parse_version_output(program: &str, stdout: &[u8]) -> Option<Version> {
    let text = std::str::from_utf8(stdout).ok()?;
    let line = text.strip_suffix('\n').unwrap_or(text);
    let version = line.strip_prefix(program)?.strip_prefix(' ')?;
    if version.contains(char::is_whitespace) {
        return None;
    }
    Version::parse(version).ok()
}

/// Whether `candidate` replaces `installed`: strictly greater by SemVer
/// precedence (build metadata ignored), so never a downgrade or a reinstall.
pub fn is_newer(candidate: &Version, installed: &Version) -> bool {
    candidate.cmp_precedence(installed) == Ordering::Greater
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn precedence_follows_semver() {
        assert!(is_newer(&v("0.10.0"), &v("0.9.9")));
        assert!(is_newer(&v("0.3.0"), &v("0.3.0-rc.1")));
        assert!(is_newer(&v("0.3.1"), &v("0.3.0")));
        // A running release candidate never installs an older stable release.
        assert!(!is_newer(&v("0.3.0"), &v("0.4.0-rc.1")));
        // Equal and older versions are never installed.
        assert!(!is_newer(&v("0.3.0"), &v("0.3.0")));
        assert!(!is_newer(&v("0.2.9"), &v("0.3.0")));
        assert!(!is_newer(&v("0.3.0"), &v("0.3.0+build.7")));
    }

    #[test]
    fn only_exact_stable_tags_are_releases() {
        assert_eq!(parse_tag("v1.2.3"), Some(v("1.2.3")));
        assert_eq!(parse_tag("v0.10.0"), Some(v("0.10.0")));
        assert_eq!(parse_tag("v0.0.0"), Some(v("0.0.0")));
        for tag in [
            "v1.2.3-rc.1",
            "1.2.3",
            "v1.2",
            "v01.2.3",
            "v1.02.3",
            "v1.2.3.4",
            "v1.2.3+b",
            "v1..3",
            "v-1.2.3",
            "V1.2.3",
            "v1.2.3 ",
        ] {
            assert_eq!(parse_tag(tag), None, "{tag}");
        }
    }

    #[test]
    fn version_output_is_one_exact_line() {
        assert_eq!(
            parse_version_output("mailtriage", b"mailtriage 0.3.0\n"),
            Some(v("0.3.0"))
        );
        assert_eq!(
            parse_version_output("mailtriage", b"mailtriage 0.4.0-rc.1"),
            Some(v("0.4.0-rc.1"))
        );
        for bad in [
            &b"mailtriage 0.3.0\n\n"[..],
            b"mailtriage  0.3.0\n",
            b"mailtriage-tray 0.3.0\n",
            b"mailtriage 0.3\n",
            b"mailtriage 0.3.0\r\n",
            b"Mailtriage 0.3.0\n",
            b"",
        ] {
            assert_eq!(parse_version_output("mailtriage", bad), None, "{bad:?}");
        }
    }
}
