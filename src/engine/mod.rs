//! The boundary between mailtriage and a mailbox. The service only talks to
//! `MailEngine`; Himalaya is one implementation, a native IMAP engine can be
//! another. No method can delete, expunge, unflag, change \Seen, or delete or
//! rename folders.
pub mod himalaya;

use crate::domain::{EngineConfig, MailboxSnapshot, SourceEnvelope};
use anyhow::Result;
use std::rc::Rc;

pub trait MailEngine {
    fn version(&self) -> Result<String>;
    /// Stable JSON describing server and login identity, never secrets.
    fn binding_identity(&self) -> Result<serde_json::Value>;
    fn snapshot(&self, folder: &str) -> Result<MailboxSnapshot>;
    /// Envelopes for UIDs in (after, through].
    fn discover(&self, folder: &str, after: u64, through: u64) -> Result<Vec<SourceEnvelope>>;
    /// Raw RFC 5322 bytes; never sets \Seen.
    fn fetch_raw(&self, folder: &str, uid: u64) -> Result<Vec<u8>>;
}

pub fn open(config: &EngineConfig) -> Result<Rc<dyn MailEngine>> {
    match config {
        EngineConfig::Himalaya(h) => Ok(Rc::new(himalaya::Himalaya::new(h)?)),
    }
}
