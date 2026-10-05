//! The OpenRouter API key comes from a key command or an environment
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

/// Where setup keeps the key. Task 4 adds the tool-backed stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyStore {
    Command,
    Env,
}

impl KeyStore {
    pub const ALL: [KeyStore; 2] = [KeyStore::Command, KeyStore::Env];

    /// The `--key-store` value.
    pub fn flag(self) -> &'static str {
        match self {
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
            KeyStore::Command => "A command that prints the key",
            KeyStore::Env => "An environment variable only",
        }
    }
}

/// The stores setup offers, the platform's own first.
pub fn key_store_options(_macos: bool, _has_tool: impl Fn(&str) -> bool) -> Vec<KeyStore> {
    vec![KeyStore::Command, KeyStore::Env]
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
