//! The provider's API key comes from a key command or an environment
//! variable, never from the config file. Only `resolve_key` and
//! `KeyCache::get` return the key, and only to the provider; errors are
//! fixed strings that never contain command output.
use crate::{
    domain::ProviderConfig,
    process::{self, Ending},
};
use anyhow::{anyhow, bail, Result};
use std::{cell::OnceCell, path::Path, time::Duration};

const KEY_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const KEY_COMMAND_MAX_STDOUT: usize = 4096;

/// `"command"` when `api_key_command` is set, else `"env"`.
pub fn key_source(config: &ProviderConfig) -> &'static str {
    if config.api_key_command.is_some() {
        "command"
    } else {
        "env"
    }
}

/// Runs a key command and returns its first stdout line, trimmed.
pub fn run_key_command(command: &[String]) -> Result<String> {
    let could_not_start = || anyhow!("API key command could not start");
    let (program, args) = command
        .split_first()
        .filter(|(program, _)| !program.is_empty())
        .ok_or_else(could_not_start)?;
    let run = process::run_bounded(
        Path::new(program),
        args,
        KEY_COMMAND_TIMEOUT,
        KEY_COMMAND_MAX_STDOUT,
    )
    .map_err(|_| could_not_start())?;
    match run.ending {
        Ending::TimedOut => bail!("API key command timed out"),
        Ending::Overflowed => bail!("API key command printed no key"),
        Ending::Exited(status) if !status.success() => bail!(
            "API key command failed (exit {})",
            status
                .code()
                .map_or_else(|| "signal".to_owned(), |code| code.to_string())
        ),
        Ending::Exited(_) => {}
    }
    let key = std::str::from_utf8(&run.stdout)
        .ok()
        .and_then(|text| text.lines().next())
        .map(str::trim)
        .unwrap_or_default();
    if key.is_empty() {
        bail!("API key command printed no key");
    }
    Ok(key.to_owned())
}

/// The key: from `api_key_command` when set (the only source then), else
/// from the `api_key_env` variable.
pub fn resolve_key(config: &ProviderConfig) -> Result<String> {
    if let Some(command) = &config.api_key_command {
        return run_key_command(command);
    }
    let key = std::env::var(&config.api_key_env)
        .map_err(|_| anyhow!("OpenRouter API key environment variable is missing"))?;
    if key.trim().is_empty() {
        bail!("OpenRouter API key environment variable is empty");
    }
    Ok(key)
}

/// Where setup keeps the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyStore {
    Keychain,
    SecretService,
    Pass,
    Command,
    Env,
}

impl KeyStore {
    pub const ALL: [KeyStore; 5] = [
        KeyStore::Keychain,
        KeyStore::SecretService,
        KeyStore::Pass,
        KeyStore::Command,
        KeyStore::Env,
    ];

    /// The `--key-store` value.
    pub fn flag(self) -> &'static str {
        match self {
            KeyStore::Keychain => "keychain",
            KeyStore::SecretService => "secret-service",
            KeyStore::Pass => "pass",
            KeyStore::Command => "command",
            KeyStore::Env => "env",
        }
    }

    pub fn from_flag(flag: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|store| store.flag() == flag)
    }

    /// The menu text.
    pub fn label(self) -> &'static str {
        match self {
            KeyStore::Keychain => "macOS Keychain",
            KeyStore::SecretService => "Secret Service (secret-tool)",
            KeyStore::Pass => "pass (the standard Unix password manager)",
            KeyStore::Command => "A command that prints the key",
            KeyStore::Env => "An environment variable only",
        }
    }

    /// The program behind a tool-backed store.
    pub fn tool(self) -> Option<&'static str> {
        match self {
            KeyStore::Keychain => Some("security"),
            KeyStore::SecretService => Some("secret-tool"),
            KeyStore::Pass => Some("pass"),
            KeyStore::Command | KeyStore::Env => None,
        }
    }
}

/// The stores setup offers, the platform's own first: the Keychain on
/// macOS; Secret Service and `pass` when their tools are on `PATH`; a
/// command and an environment variable always.
pub fn key_store_options(macos: bool, has_tool: impl Fn(&str) -> bool) -> Vec<KeyStore> {
    let mut options = Vec::new();
    if macos {
        options.push(KeyStore::Keychain);
    }
    for store in [KeyStore::SecretService, KeyStore::Pass] {
        if store.tool().is_some_and(&has_tool) {
            options.push(store);
        }
    }
    options.extend([KeyStore::Command, KeyStore::Env]);
    options
}

/// The command that prints the key stored for the key store account
/// `account` (a provider's `key_account()`), with the tool's absolute path.
pub fn read_command(store: KeyStore, tool: &Path, account: &str) -> Option<Vec<String>> {
    let args = match store {
        KeyStore::Keychain => strings(&[
            "find-generic-password",
            "-s",
            "mailtriage",
            "-a",
            account,
            "-w",
        ]),
        KeyStore::SecretService => {
            strings(&["lookup", "service", "mailtriage", "provider", account])
        }
        KeyStore::Pass => vec!["show".to_owned(), format!("mailtriage/{account}")],
        KeyStore::Command | KeyStore::Env => return None,
    };
    Some(with_tool(tool, args))
}

/// The command that stores the key for `account`; the tool asks for it on
/// the terminal.
pub fn store_command(store: KeyStore, tool: &Path, account: &str) -> Option<Vec<String>> {
    let args = match store {
        KeyStore::Keychain => strings(&[
            "add-generic-password",
            "-U",
            "-s",
            "mailtriage",
            "-a",
            account,
            "-w",
        ]),
        KeyStore::SecretService => vec![
            "store".to_owned(),
            secret_service_label(account),
            "service".to_owned(),
            "mailtriage".to_owned(),
            "provider".to_owned(),
            account.to_owned(),
        ],
        KeyStore::Pass => vec!["insert".to_owned(), format!("mailtriage/{account}")],
        KeyStore::Command | KeyStore::Env => return None,
    };
    Some(with_tool(tool, args))
}

/// The label Secret Service shows; `openrouter` keeps the one setup has
/// always written.
fn secret_service_label(account: &str) -> String {
    match account {
        "openrouter" => "--label=mailtriage OpenRouter key".to_owned(),
        other => format!("--label=mailtriage {other} key"),
    }
}

/// The tool-backed store whose read command for `account` `command` is.
pub fn store_of(command: &[String], account: &str) -> Option<KeyStore> {
    let tool = Path::new(command.first()?);
    [KeyStore::Keychain, KeyStore::SecretService, KeyStore::Pass]
        .into_iter()
        .find(|&store| {
            tool.file_name().and_then(|name| name.to_str()) == store.tool()
                && read_command(store, tool, account).as_deref() == Some(command)
        })
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

fn with_tool(tool: &Path, args: Vec<String>) -> Vec<String> {
    std::iter::once(tool.display().to_string())
        .chain(args)
        .collect()
}

/// The key, or the reason it is missing, resolved at most once. A `Service`
/// holds one, so each `watch` pass resolves the key again. No `Debug`: it
/// holds the key.
#[derive(Default)]
pub struct KeyCache(OnceCell<std::result::Result<String, String>>);

impl KeyCache {
    pub fn get(&self, config: &ProviderConfig) -> Result<String> {
        self.0
            .get_or_init(|| resolve_key(config).map_err(|e| e.to_string()))
            .clone()
            .map_err(|message| anyhow!(message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_platform_store_comes_first() {
        assert_eq!(
            key_store_options(true, |_| false),
            vec![KeyStore::Keychain, KeyStore::Command, KeyStore::Env]
        );
        assert_eq!(
            key_store_options(false, |_| true),
            vec![
                KeyStore::SecretService,
                KeyStore::Pass,
                KeyStore::Command,
                KeyStore::Env
            ]
        );
        assert_eq!(
            key_store_options(false, |tool| tool == "pass"),
            vec![KeyStore::Pass, KeyStore::Command, KeyStore::Env]
        );
        assert_eq!(
            key_store_options(false, |_| false),
            vec![KeyStore::Command, KeyStore::Env]
        );
        for store in KeyStore::ALL {
            assert_eq!(KeyStore::from_flag(store.flag()), Some(store));
        }
    }

    #[test]
    fn read_and_store_commands_use_the_tool_path() {
        let tool = Path::new("/usr/bin/security");
        assert_eq!(
            read_command(KeyStore::Keychain, tool, "openrouter").unwrap(),
            [
                "/usr/bin/security",
                "find-generic-password",
                "-s",
                "mailtriage",
                "-a",
                "openrouter",
                "-w"
            ]
        );
        assert_eq!(
            store_command(KeyStore::Keychain, tool, "openrouter").unwrap(),
            [
                "/usr/bin/security",
                "add-generic-password",
                "-U",
                "-s",
                "mailtriage",
                "-a",
                "openrouter",
                "-w"
            ]
        );
        let tool = Path::new("/usr/bin/secret-tool");
        assert_eq!(
            read_command(KeyStore::SecretService, tool, "openrouter").unwrap(),
            [
                "/usr/bin/secret-tool",
                "lookup",
                "service",
                "mailtriage",
                "provider",
                "openrouter"
            ]
        );
        assert_eq!(
            store_command(KeyStore::SecretService, tool, "openrouter").unwrap(),
            [
                "/usr/bin/secret-tool",
                "store",
                "--label=mailtriage OpenRouter key",
                "service",
                "mailtriage",
                "provider",
                "openrouter"
            ]
        );
        let tool = Path::new("/usr/bin/pass");
        assert_eq!(
            read_command(KeyStore::Pass, tool, "openrouter").unwrap(),
            ["/usr/bin/pass", "show", "mailtriage/openrouter"]
        );
        assert_eq!(
            store_command(KeyStore::Pass, tool, "openrouter").unwrap(),
            ["/usr/bin/pass", "insert", "mailtriage/openrouter"]
        );
        assert_eq!(read_command(KeyStore::Env, tool, "openrouter"), None);
        assert_eq!(store_command(KeyStore::Command, tool, "openrouter"), None);
    }

    #[test]
    fn the_key_account_names_the_stored_entry() {
        let tool = Path::new("/usr/bin/secret-tool");
        assert_eq!(
            store_command(KeyStore::SecretService, tool, "acme").unwrap(),
            [
                "/usr/bin/secret-tool",
                "store",
                "--label=mailtriage acme key",
                "service",
                "mailtriage",
                "provider",
                "acme"
            ]
        );
        assert_eq!(
            read_command(KeyStore::Pass, Path::new("/usr/bin/pass"), "acme").unwrap(),
            ["/usr/bin/pass", "show", "mailtriage/acme"]
        );
        assert_eq!(
            read_command(KeyStore::Keychain, Path::new("/usr/bin/security"), "acme").unwrap()[5],
            "acme"
        );
    }

    #[test]
    fn a_read_command_names_its_store() {
        for (store, tool) in [
            (KeyStore::Keychain, "/usr/bin/security"),
            (KeyStore::SecretService, "/usr/bin/secret-tool"),
            (KeyStore::Pass, "/opt/homebrew/bin/pass"),
        ] {
            let command = read_command(store, Path::new(tool), "openrouter").unwrap();
            assert_eq!(store_of(&command, "openrouter"), Some(store));
            assert_eq!(store_of(&command, "other"), None);
        }
        let mut other =
            read_command(KeyStore::Pass, Path::new("/usr/bin/pass"), "openrouter").unwrap();
        other[2] = "other/key".into();
        assert_eq!(store_of(&other, "openrouter"), None);
        let renamed =
            read_command(KeyStore::Pass, Path::new("/usr/bin/gopass"), "openrouter").unwrap();
        assert_eq!(store_of(&renamed, "openrouter"), None);
        assert_eq!(
            store_of(
                &["/bin/sh".into(), "-c".into(), "cat k".into()],
                "openrouter"
            ),
            None
        );
        assert_eq!(store_of(&[], "openrouter"), None);
    }
}
