//! IMAP category filing.
pub mod apply;
pub mod arrivals;
pub mod done;
pub mod inputs;
pub mod observe;
pub mod planner;
pub mod recover;
pub mod store;
pub mod transitions;
pub mod types;

pub use types::*;

use crate::domain::{AccountConfig, FilingMode};
use crate::engine::MailEngine;

/// The `filing` object of a sync response; also stored as the last pass.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct FilingSummary {
    pub mode: String,
    pub planned: usize,
    pub moved: usize,
    pub flagged: usize,
    pub reverted: usize,
    pub client_corrections: usize,
    pub pinned: usize,
    pub archived_done: usize,
    pub quarantined: usize,
    pub unresolved: usize,
    pub folders_created: usize,
    pub hydrated: usize,
    pub errors: usize,
    /// Codes only, never message text.
    pub problems: Vec<String>,
}

/// What every filing step of one pass needs. It borrows nothing from
/// `Service`, so steps can take the store mutably alongside it.
pub struct PassContext<'a> {
    pub account: &'a str,
    pub cfg: &'a AccountConfig,
    pub engine: &'a dyn MailEngine,
    pub mode: FilingMode,
    pub generation: &'a str,
    pub now: String,
    pub max_attempts: u32,
    /// Re-verifies the account binding; called before write batches and recovery observations.
    /// Build it from owned clones (account config, the stored identity string, the engine `Rc`)
    /// so it never borrows `Service` while the store is mutably borrowed.
    pub verify_binding: &'a dyn Fn() -> anyhow::Result<()>,
}

/// Whether the mail engine refused because its configuration changed; such an
/// error aborts the pass instead of counting as a filing error.
pub(crate) fn is_config_changed(e: &anyhow::Error) -> bool {
    e.downcast_ref::<crate::engine::ConfigChanged>().is_some()
}

pub fn mode_str(mode: FilingMode) -> &'static str {
    match mode {
        FilingMode::Off => "off",
        FilingMode::DryRun => "dry_run",
        FilingMode::Live => "live",
    }
}
