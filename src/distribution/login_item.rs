//! The tray's login item as `mailtriage-tray autostart enable` writes it
//! (tray/src/autostart.rs): a launchd agent on macOS, an XDG autostart
//! entry on Linux, both marked. `self uninstall` reads and removes it
//! itself, so it needs no tray binary.
use crate::update::service_files::plist_arguments;
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

/// The launchd label of the tray's login item.
pub const LABEL: &str = "digital.wirdrei.mailtriage-tray";
const PLIST_MARKER: &str = "<key>XMailtriageManaged</key>";
const DESKTOP_MARKER: &str = "# managed by mailtriage";

/// Where the login item lives: `~/Library/LaunchAgents/<label>.plist` on
/// macOS; on Linux `$XDG_CONFIG_HOME/autostart/mailtriage-tray.desktop`
/// when `XDG_CONFIG_HOME` is absolute, else under `~/.config`.
pub fn path(macos: bool, home: &Path, xdg_config_home: Option<&OsStr>) -> PathBuf {
    if macos {
        return home
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist"));
    }
    xdg_config_home
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"))
        .join("autostart/mailtriage-tray.desktop")
}

/// The tray program a login item's `text` starts: its first argument.
/// `None` without mailtriage's marker or when it cannot be decoded.
pub fn tray_of(macos: bool, text: &str) -> Option<PathBuf> {
    let arguments = if macos {
        if !text.contains(PLIST_MARKER) {
            return None;
        }
        plist_arguments(text)?
    } else {
        if text.lines().next() != Some(DESKTOP_MARKER) {
            return None;
        }
        desktop_arguments(text.lines().find_map(|l| l.strip_prefix("Exec="))?)?
    };
    arguments.into_iter().next().map(PathBuf::from)
}

/// The arguments of an `Exec=` value: the inverse of the tray's
/// `desktop_arg` (the string-level `\` escape, then the Desktop Entry
/// quoting, then `%%` for `%`).
pub fn desktop_arguments(value: &str) -> Option<Vec<String>> {
    let mut unescaped = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            unescaped.push(chars.next()?);
        } else {
            unescaped.push(c);
        }
    }
    let mut args = vec![];
    let mut chars = unescaped.chars().peekable();
    loop {
        while chars.peek() == Some(&' ') {
            chars.next();
        }
        let Some(&first) = chars.peek() else {
            break;
        };
        let mut arg = String::new();
        if first == '"' {
            chars.next();
            loop {
                match chars.next()? {
                    '\\' => arg.push(chars.next()?),
                    '"' => break,
                    c => arg.push(c),
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c == ' ' {
                    break;
                }
                arg.push(c);
                chars.next();
            }
        }
        args.push(arg.replace("%%", "%"));
    }
    Some(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact files tray/tests/autostart.rs pins for the tray's writers.
    #[test]
    fn the_trays_login_items_decode_to_the_tray() {
        let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>XMailtriageManaged</key>
  <true/>
  <key>Label</key>
  <string>digital.wirdrei.mailtriage-tray</string>
  <key>ProgramArguments</key>
  <array>
    <string>/Apps/mail tray/mailtriage-tray</string>
    <string>--config</string>
    <string>/Users/a &amp; b/50% $HOME "x"/mailtriage.json</string>
    <string>--mailtriage</string>
    <string>/opt/mail triage/mailtriage</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>/usr/bin:/bin</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>LimitLoadToSessionType</key>
  <string>Aqua</string>
</dict>
</plist>
"#;
        assert_eq!(
            tray_of(true, plist),
            Some(PathBuf::from("/Apps/mail tray/mailtriage-tray"))
        );
        assert_eq!(
            plist_arguments(plist).unwrap(),
            [
                "/Apps/mail tray/mailtriage-tray",
                "--config",
                "/Users/a & b/50% $HOME \"x\"/mailtriage.json",
                "--mailtriage",
                "/opt/mail triage/mailtriage"
            ]
        );
        let desktop = "# managed by mailtriage\n[Desktop Entry]\nType=Application\nName=mailtriage\nExec=\"/Apps/mail tray/mailtriage-tray\" --config \"/Users/a & b/50%% \\\\$HOME \\\\\"x\\\\\"/mailtriage.json\" --mailtriage \"/opt/mail triage/mailtriage\"\nNoDisplay=true\n";
        assert_eq!(
            tray_of(false, desktop),
            Some(PathBuf::from("/Apps/mail tray/mailtriage-tray"))
        );
        let exec = desktop
            .lines()
            .find_map(|l| l.strip_prefix("Exec="))
            .unwrap();
        assert_eq!(
            desktop_arguments(exec).unwrap(),
            [
                "/Apps/mail tray/mailtriage-tray",
                "--config",
                "/Users/a & b/50% $HOME \"x\"/mailtriage.json",
                "--mailtriage",
                "/opt/mail triage/mailtriage"
            ]
        );
        // Files mailtriage did not write are not its login item.
        assert_eq!(
            tray_of(true, &plist.replace("XMailtriageManaged", "Other")),
            None
        );
        assert_eq!(
            tray_of(false, &desktop.replacen("# managed by mailtriage\n", "", 1)),
            None
        );
    }

    #[test]
    fn the_login_item_path_follows_the_platform() {
        let home = Path::new("/h");
        assert_eq!(
            path(true, home, None),
            PathBuf::from("/h/Library/LaunchAgents/digital.wirdrei.mailtriage-tray.plist")
        );
        assert_eq!(
            path(false, home, Some(OsStr::new("/x"))),
            PathBuf::from("/x/autostart/mailtriage-tray.desktop")
        );
        assert_eq!(
            path(false, home, Some(OsStr::new("rel"))),
            PathBuf::from("/h/.config/autostart/mailtriage-tray.desktop")
        );
    }
}
