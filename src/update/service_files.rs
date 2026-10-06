//! Reading the service files mailtriage wrote: the exact inverse of
//! `system_service::plist` and `system_service::systemd_unit`.
use crate::{
    config,
    system_service::{self, Manager, LABEL_PREFIX},
};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// A service file mailtriage wrote (its marker is present).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceFile {
    pub account: String,
    pub manager: Manager,
    pub unit_path: PathBuf,
    /// Decoded from the file; `None` when it cannot be decoded.
    pub executable: Option<PathBuf>,
}

/// The manager whose files this platform uses, without asking for the tool.
pub fn platform_manager() -> Option<Manager> {
    if cfg!(target_os = "macos") {
        Some(Manager::Launchd)
    } else if cfg!(target_os = "linux") {
        Some(Manager::Systemd)
    } else {
        None
    }
}

/// Every marked service file in `manager`'s directory under `home`, by
/// account name.
pub fn list(manager: Manager, home: &Path) -> Vec<ServiceFile> {
    let Ok(entries) = fs::read_dir(system_service::unit_dir(manager, home)) else {
        return vec![];
    };
    let mut files: Vec<ServiceFile> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let account = account_of(manager, &name)?;
            let text = fs::read_to_string(entry.path()).ok()?;
            system_service::is_marked(manager, &text).then(|| ServiceFile {
                account,
                manager,
                unit_path: entry.path(),
                executable: executable(manager, &text),
            })
        })
        .collect();
    files.sort_by_key(|f| f.account.clone());
    files
}

/// The account a service file name belongs to.
fn account_of(manager: Manager, file_name: &str) -> Option<String> {
    let account = match manager {
        Manager::Launchd => file_name
            .strip_prefix(LABEL_PREFIX)?
            .strip_prefix('.')?
            .strip_suffix(".plist")?,
        Manager::Systemd => file_name
            .strip_prefix("mailtriage-")?
            .strip_suffix(".service")?,
    };
    config::valid_account_name(account).then(|| account.to_owned())
}

/// The executable a service file runs: the first program argument.
pub fn executable(manager: Manager, text: &str) -> Option<PathBuf> {
    let arguments = match manager {
        Manager::Launchd => plist_arguments(text)?,
        Manager::Systemd => unit_arguments(text)?,
    };
    arguments
        .into_iter()
        .next()
        .filter(|exe| Path::new(exe).is_absolute())
        .map(PathBuf::from)
}

/// The `<string>`s of `ProgramArguments`, XML entities decoded.
pub fn plist_arguments(text: &str) -> Option<Vec<String>> {
    let (_, rest) = text.split_once("<key>ProgramArguments</key>")?;
    let mut rest = rest.trim_start().strip_prefix("<array>")?;
    let mut arguments = Vec::new();
    loop {
        rest = rest.trim_start();
        if rest.starts_with("</array>") {
            return Some(arguments);
        }
        let (value, after) = rest.strip_prefix("<string>")?.split_once("</string>")?;
        arguments.push(xml_decode(value)?);
        rest = after;
    }
}

/// Undoes `system_service`'s `xml`, which escapes `&`, `<` and `>`: decodes
/// `&amp;`, `&lt;` and `&gt;`, so everything it writes round-trips. A raw
/// `<` anywhere in `text`, an unknown entity or an unterminated one gives
/// `None`. Input `xml` never writes is otherwise accepted: a raw `>`, and
/// `&quot;` and `&apos;`, decode too.
fn xml_decode(text: &str) -> Option<String> {
    if text.contains('<') {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let (entity, after) = rest[at + 1..].split_once(';')?;
        out.push(match entity {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            _ => return None,
        });
        rest = after;
    }
    out.push_str(rest);
    Some(out)
}

/// The words of the unit's `ExecStart=`, `systemd_arg`'s quoting and its
/// `%`/`$` doubling undone.
pub fn unit_arguments(text: &str) -> Option<Vec<String>> {
    let line = text.lines().find_map(|l| l.strip_prefix("ExecStart="))?;
    let mut words = Vec::new();
    let mut chars = line.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        if chars.peek().is_none() {
            return Some(words);
        }
        let mut word = String::new();
        if chars.next_if_eq(&'"').is_some() {
            loop {
                match chars.next()? {
                    '\\' => word.push(chars.next()?),
                    '"' => break,
                    c => word.push(c),
                }
            }
            if chars.peek().is_some_and(|c| !c.is_whitespace()) {
                return None;
            }
        } else {
            while let Some(c) = chars.next_if(|c| !c.is_whitespace()) {
                if matches!(c, '"' | '\'' | '\\' | ';') {
                    return None;
                }
                word.push(c);
            }
        }
        words.push(undouble(&word)?);
    }
}

/// `%%` → `%` and `$$` → `$`; a single one was not written by mailtriage.
fn undouble(word: &str) -> Option<String> {
    let mut out = String::with_capacity(word.len());
    let mut chars = word.chars();
    while let Some(c) = chars.next() {
        if matches!(c, '%' | '$') && chars.next()? != c {
            return None;
        }
        out.push(c);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system_service::{plist, systemd_unit, Unit};

    fn unit(exe: &str) -> Unit {
        Unit {
            account: "work".into(),
            exe: PathBuf::from(exe),
            config: PathBuf::from("/Users/a & b/.config/mailtriage/mailtriage.json"),
            interval_seconds: 60,
            limit: 100,
            log_dir: PathBuf::from("/tmp/logs"),
            path_env: Some("/usr/bin:/bin".into()),
        }
    }

    #[test]
    fn service_files_decode_back_to_their_arguments() {
        for exe in [
            "/usr/local/bin/mailtriage",
            "/opt/mail triage/bin/mailtriage",
            "/opt/50% off/mailtriage",
            "/opt/$HOME/mailtriage",
            "/opt/say \"hi\"/mailtriage",
            "/opt/it's/mailtriage",
            "/opt/back\\slash/mailtriage",
            "/opt/a & b/<x>/mailtriage",
            "/opt/semi;colon/mailtriage",
            "/opt/%%$$/mailtriage",
        ] {
            let unit = unit(exe);
            let expected = unit.arguments();
            assert_eq!(
                plist_arguments(&plist(&unit)),
                Some(expected.clone()),
                "{exe}"
            );
            assert_eq!(
                unit_arguments(&systemd_unit(&unit)),
                Some(expected),
                "{exe}"
            );
            assert_eq!(
                executable(Manager::Launchd, &plist(&unit)),
                Some(PathBuf::from(exe))
            );
            assert_eq!(
                executable(Manager::Systemd, &systemd_unit(&unit)),
                Some(PathBuf::from(exe))
            );
        }
    }

    #[test]
    fn files_that_cannot_be_decoded_give_no_executable() {
        assert_eq!(executable(Manager::Launchd, "<plist></plist>"), None);
        assert_eq!(
            executable(
                Manager::Launchd,
                "<key>ProgramArguments</key><array><string>/a&bogus;b</string></array>"
            ),
            None
        );
        // A raw `<` anywhere in a value, before or after an entity: `xml`
        // never writes one.
        for value in ["/a<b", "/a<b&amp;c", "/a&amp;b<c"] {
            let text =
                format!("<key>ProgramArguments</key><array><string>{value}</string></array>");
            assert_eq!(plist_arguments(&text), None, "{value}");
        }
        assert_eq!(executable(Manager::Systemd, "[Service]\n"), None);
        assert_eq!(
            executable(Manager::Systemd, "ExecStart=\"/unterminated\n"),
            None
        );
        assert_eq!(executable(Manager::Systemd, "ExecStart=/a%b watch\n"), None);
        assert_eq!(
            executable(Manager::Systemd, "ExecStart=relative watch\n"),
            None
        );
    }

    #[test]
    fn listed_files_are_marked_and_named_for_an_account() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        for manager in [Manager::Launchd, Manager::Systemd] {
            let units = system_service::unit_dir(manager, home);
            fs::create_dir_all(&units).unwrap();
            let text = |u: &Unit| match manager {
                Manager::Launchd => plist(u),
                Manager::Systemd => systemd_unit(u),
            };
            let name = |account: &str| match manager {
                Manager::Launchd => format!("digital.wirdrei.mailtriage.{account}.plist"),
                Manager::Systemd => format!("mailtriage-{account}.service"),
            };
            fs::write(units.join(name("work")), text(&unit("/opt/a/mailtriage"))).unwrap();
            fs::write(units.join(name("home")), text(&unit("/opt/b/mailtriage"))).unwrap();
            fs::write(units.join(name("theirs")), "not written by mailtriage").unwrap();
            fs::write(
                units.join("digital.wirdrei.mailtriage-tray.plist"),
                text(&unit("/x")),
            )
            .unwrap();
            fs::write(units.join("other.service"), text(&unit("/x"))).unwrap();
            let found = list(manager, home);
            let accounts: Vec<_> = found.iter().map(|f| f.account.as_str()).collect();
            assert_eq!(accounts, ["home", "work"], "{manager:?}");
            assert_eq!(
                found[1].executable,
                Some(PathBuf::from("/opt/a/mailtriage"))
            );
            assert_eq!(found[1].unit_path, units.join(name("work")));
        }
        assert!(list(Manager::Launchd, &home.join("missing")).is_empty());
    }
}
