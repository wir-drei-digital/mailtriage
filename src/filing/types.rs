//! Typed rows of the filing tables (schema v3) and store call parameters.
use crate::domain::{FilingMode, SourceEnvelope};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocationState {
    Known,
    Ambiguous,
    Absent,
}
impl LocationState {
    pub fn as_str(self) -> &'static str {
        match self {
            LocationState::Known => "known",
            LocationState::Ambiguous => "ambiguous",
            LocationState::Absent => "absent",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "known" => Some(LocationState::Known),
            "ambiguous" => Some(LocationState::Ambiguous),
            "absent" => Some(LocationState::Absent),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Placement {
    pub account: String,
    pub message_id: String,
    pub source_folder: String,
    pub home_folder: Option<String>,
    pub home_epoch: Option<u64>,
    pub home_uid: Option<u64>,
    pub location_state: LocationState,
    pub absent_since: Option<String>,
    /// Category id or `"@source"`; `None` means automatic.
    pub desired_target: Option<String>,
    pub pinned: bool,
    pub eligible_once: bool,
    pub desired_rev: i64,
    pub filed_at: Option<String>,
    /// `"mailtriage"` or `"user"`.
    pub filed_by: Option<String>,
    pub flag_attempted_at: Option<String>,
    pub flagged_at: Option<String>,
    pub done_inferred: bool,
    /// `move_failed`, `duplicate_copy`, `quarantined` or `merge_conflict`.
    pub blocked_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct FolderRecord {
    pub account: String,
    pub native: String,
    pub configured: Option<String>,
    /// `None` for source folders.
    pub category_id: Option<String>,
    /// `created`, `adopted` or `source`.
    pub origin: Option<String>,
    /// `ok`, `missing`, `special_use`, `noselect`, `needs_confirmation`, `retired` or `error`.
    pub state: String,
    pub role_verified: bool,
    pub confirmed: bool,
    pub subscribed: bool,
    /// `epoch_race` or `epoch_race_suspected`; cleared only by an explicit release.
    pub pause_reason: Option<String>,
    pub epoch: Option<u64>,
    pub watch_from_uid: Option<u64>,
    pub rescan_epoch: Option<u64>,
    pub rescan_below_uid: Option<u64>,
    pub rescan_complete: bool,
    pub checked_at: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Arrival {
    pub id: i64,
    pub account: String,
    pub folder: String,
    pub epoch: u64,
    pub uid: u64,
    pub message_id: String,
    pub rfc_message_id: Option<String>,
    /// `pending`, `resolved`, `vanished`, `unresolved` or `dismissed`.
    pub state: String,
    /// `own_move`, `user_move`, `user_pin`, `extra`, `new`, `rescan`, `reverted` or `quarantined`.
    pub kind: Option<String>,
    pub intent_id: Option<i64>,
    pub created_at: String,
    pub resolved_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Intent {
    pub id: i64,
    pub account: String,
    pub message_id: String,
    /// `move` or `flag`.
    pub kind: String,
    pub folder: String,
    pub epoch: u64,
    pub uid: u64,
    pub target: Option<String>,
    pub target_epoch: Option<u64>,
    pub target_uid_next: Option<u64>,
    pub target_uid: Option<u64>,
    pub desired_rev: i64,
    pub consumes_eligible: bool,
    pub batch: Option<String>,
    /// `in_flight`, `sent`, `uncertain`, `awaiting_rescan`, `applied`, `lost`, `failed` or `superseded`.
    pub state: String,
    pub attempts: u32,
    pub next_after: Option<String>,
    pub dispatched_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub error: Option<String>,
}
pub const OPEN_INTENT_STATES: [&str; 4] = ["in_flight", "sent", "uncertain", "awaiting_rescan"];

/// `OPEN_INTENT_STATES` as an SQL list for `state IN ...`.
pub(crate) fn open_states_sql() -> String {
    format!("('{}')", OPEN_INTENT_STATES.join("','"))
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Revert {
    pub id: i64,
    pub account: String,
    pub parent_intent: i64,
    /// COPYUID destination locator of the message the epoch race moved.
    pub folder: String,
    pub folder_epoch: u64,
    pub uid: u64,
    /// Original source folder and the session epoch the race ran in.
    pub target: String,
    pub target_epoch: u64,
    /// `pending`, `in_flight`, `applied` or `failed`.
    pub state: String,
    pub target_uid: Option<u64>,
    pub created_at: String,
    pub updated_at: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct FilingStateRow {
    pub account: String,
    pub mode: FilingMode,
    pub enabled_at: Option<String>,
    pub bootstrap_done: bool,
    pub last_pass: Option<serde_json::Value>,
}

/// A folder's checkpoint: (epoch, last UID, complete, scanned_at).
pub type CheckpointState = (u64, u64, bool, Option<String>);

/// Transport metadata of a message.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct MessageMeta {
    pub rfc_message_id: Option<String>,
    pub size: Option<u64>,
    pub internal_date: Option<String>,
    pub flags: Vec<String>,
    pub fingerprinted: bool,
    pub source_managed: bool,
}

/// Placements of one home folder awaiting hydration, in that folder's checkpoint epoch.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct HydrationBatch {
    pub folder: String,
    pub epoch: u64,
    /// (home UID, message id), lowest UID first.
    pub members: Vec<(u64, String)>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RescanFilter {
    pub below_uid: u64,
    pub rfc_ids: BTreeSet<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct StageOptions<'a> {
    pub record_arrivals: bool,
    /// target UID -> (message id, intent id), from epoch-bound COPYUID.
    pub known_targets: &'a BTreeMap<u64, (String, i64)>,
    pub rescan_filter: Option<&'a RescanFilter>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct NewIntent<'a> {
    pub account: &'a str,
    pub message_id: &'a str,
    pub kind: &'a str,
    pub folder: &'a str,
    pub epoch: u64,
    pub uid: u64,
    pub target: Option<&'a str>,
    pub target_epoch: Option<u64>,
    pub target_uid_next: Option<u64>,
    pub desired_rev: i64,
    pub consumes_eligible: bool,
    pub batch: &'a str,
    pub state: &'a str,
    pub now: &'a str,
}

/// Fields that are `Some` overwrite the stored value; `None` keeps it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, Default)]
pub struct IntentPatch {
    pub target_uid: Option<u64>,
    pub attempts: Option<u32>,
    pub next_after: Option<String>,
    pub dispatched_at: Option<String>,
    pub error: Option<String>,
}

/// The stored form of an envelope's Message-ID: trimmed, case-sensitive,
/// `None` when missing or blank (spec "Identity and placements").
pub fn rfc_message_id(env: &SourceEnvelope) -> Option<String> {
    env.message_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}
