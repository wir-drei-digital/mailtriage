//! The boundary between mailtriage and a mailbox. The service only talks to
//! `MailEngine`; Himalaya is one implementation, a native IMAP engine can be
//! another. No method can delete, expunge, unflag, change \Seen, or delete or
//! rename folders. Writes are limited to creating and subscribing folders,
//! UID MOVE and adding \Flagged.
pub mod fake;
pub mod himalaya;
pub mod raw;

use crate::domain::{EngineConfig, MailboxSnapshot, SourceEnvelope};
use anyhow::Result;
use std::rc::Rc;

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct EngineCapabilities {
    pub move_supported: bool,
    /// COPYUID available.
    pub uidplus: bool,
    /// RFC 6154 advertised.
    pub special_use: bool,
    pub delimiter: Option<char>,
    /// First personal namespace, "" if none.
    pub personal_prefix: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderInfo {
    /// Native name.
    pub name: String,
    /// For example `\Noselect`, `\HasChildren`.
    pub attributes: Vec<String>,
    /// Special-use roles; `None` means unknown.
    pub roles: Option<Vec<String>>,
    pub subscribed: bool,
}

pub use raw::CopyUid;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WriteOutcome {
    /// `a1 SELECT` returned OK.
    pub selected: bool,
    /// UIDVALIDITY seen by that SELECT.
    pub session_epoch: Option<u64>,
    /// `a2` returned OK.
    pub completed: bool,
    /// Kept even when `a2` returned NO.
    pub copyuid: Option<CopyUid>,
}

/// The engine refused to run because its external configuration changed.
#[derive(Debug)]
pub struct ConfigChanged;

impl std::fmt::Display for ConfigChanged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("mail engine configuration changed during operation")
    }
}

impl std::error::Error for ConfigChanged {}

pub const SPECIAL_USE_ROLES: [&str; 7] = [
    "\\Sent",
    "\\Trash",
    "\\Drafts",
    "\\Junk",
    "\\Archive",
    "\\All",
    "\\Flagged",
];

pub trait MailEngine {
    fn version(&self) -> Result<String>;
    /// Stable JSON describing server and login identity, never secrets.
    fn binding_identity(&self) -> Result<serde_json::Value>;
    fn snapshot(&self, folder: &str) -> Result<MailboxSnapshot>;
    /// Envelopes for UIDs in (after, through].
    fn discover(&self, folder: &str, after: u64, through: u64) -> Result<Vec<SourceEnvelope>>;
    /// Raw RFC 5322 bytes; never sets \Seen.
    fn fetch_raw(&self, folder: &str, uid: u64) -> Result<Vec<u8>>;
    fn capabilities(&self) -> Result<EngineCapabilities>;
    fn list_folders(&self) -> Result<Vec<FolderInfo>>;
    fn create_folder(&self, native: &str) -> Result<()>;
    fn subscribe_folder(&self, native: &str) -> Result<()>;
    /// Envelopes for exactly these UIDs; absent UIDs are omitted.
    fn envelopes(&self, folder: &str, uids: &[u64]) -> Result<Vec<SourceEnvelope>>;
    /// One session: SELECT folder; UID MOVE uids target.
    fn move_messages(&self, folder: &str, uids: &[u64], target: &str) -> Result<WriteOutcome>;
    /// One session: SELECT folder; UID STORE uids +FLAGS.SILENT (\Flagged).
    fn add_flagged(&self, folder: &str, uids: &[u64]) -> Result<WriteOutcome>;
    /// Folders beyond the configured sources this engine may touch this pass (category and referenced retired folders). Default: no restriction.
    fn set_watch_scope(&self, _folders: &[String]) {}
    /// Watched folder names that a client-side alias would resolve elsewhere.
    fn alias_conflicts(&self, _folders: &[String]) -> Result<Vec<String>> {
        Ok(vec![])
    }
}

pub fn open(config: &EngineConfig) -> Result<Rc<dyn MailEngine>> {
    match config {
        EngineConfig::Himalaya(h) => Ok(Rc::new(himalaya::Himalaya::new(h)?)),
    }
}

/// The binding source of an engine configuration without opening the
/// engine: stable JSON describing server and login identity, never secrets.
/// It is what `MailEngine::binding_identity` reports for that configuration.
pub fn binding_source(config: &EngineConfig) -> Result<serde_json::Value> {
    match config {
        EngineConfig::Himalaya(h) => himalaya::source_binding(h),
    }
}
