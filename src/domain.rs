use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub schema_version: u32,
    /// What `watch` does about new releases; a config without it means `auto`.
    #[serde(default)]
    pub updates: UpdateMode,
    pub state_dir: PathBuf,
    pub provider: ProviderConfig,
    pub policy: PolicyConfig,
    pub accounts: BTreeMap<String, AccountConfig>,
}

/// The `updates` setting: `auto` installs new releases in the background,
/// `notify` only reports them, `off` makes no network calls.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum UpdateMode {
    #[default]
    Auto,
    Notify,
    Off,
}
impl UpdateMode {
    pub const ALL: [UpdateMode; 3] = [UpdateMode::Auto, UpdateMode::Notify, UpdateMode::Off];
    pub fn as_str(self) -> &'static str {
        match self {
            UpdateMode::Auto => "auto",
            UpdateMode::Notify => "notify",
            UpdateMode::Off => "off",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.as_str() == value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub kind: String,
    pub model: String,
    pub endpoint: String,
    /// Argument list that prints the API key; when set it is the only key
    /// source. Serialized only when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_command: Option<Vec<String>>,
    #[serde(default)]
    pub api_key_env: String,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyConfig {
    pub choice_confidence_min: f64,
    pub action_yes_min: f64,
    pub action_no_max: f64,
    pub review_mode: bool,
    pub max_body_chars: usize,
    pub max_attempts: u32,
    pub freshness_hours: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountConfig {
    pub identity: String,
    pub timezone: String,
    pub brief: String,
    pub taxonomy_revision: u64,
    pub categories: Vec<Category>,
    /// Schema 1 location of the Himalaya settings. Read on load, moved into
    /// `engine` by `config::normalize`, never serialized.
    #[serde(default, skip_serializing)]
    pub himalaya: Option<HimalayaConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<EngineConfig>,
    #[serde(default)]
    pub filing: FilingConfig,
}
impl AccountConfig {
    pub fn engine_config(&self) -> Option<EngineConfig> {
        self.engine
            .clone()
            .or_else(|| self.himalaya.clone().map(EngineConfig::Himalaya))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum EngineConfig {
    Himalaya(HimalayaConfig),
}
impl EngineConfig {
    pub fn mailboxes(&self) -> &[String] {
        match self {
            EngineConfig::Himalaya(h) => &h.mailboxes,
        }
    }
    pub fn timeout_seconds(&self) -> u64 {
        match self {
            EngineConfig::Himalaya(h) => h.timeout_seconds,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FilingMode {
    #[default]
    Off,
    DryRun,
    Live,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FilingConfig {
    #[serde(default)]
    pub mode: FilingMode,
    #[serde(default = "default_true")]
    pub flag: bool,
    #[serde(default = "default_max_actions")]
    pub max_actions_per_pass: usize,
    /// Reply queue spec: new mail that needs action stays in its source
    /// folder until it is answered (or marked done), then gets `\Seen` and
    /// moves to its category folder.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reply_queue: bool,
}
impl Default for FilingConfig {
    fn default() -> Self {
        Self {
            mode: FilingMode::Off,
            flag: true,
            max_actions_per_pass: 200,
            reply_queue: false,
        }
    }
}
fn default_true() -> bool {
    true
}
fn default_max_actions() -> usize {
    200
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Category {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub examples: Vec<String>,
    #[serde(default)]
    pub catch_all: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}
impl Category {
    pub fn effective_folder(&self) -> &str {
        self.folder.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HimalayaConfig {
    pub binary: PathBuf,
    pub config: PathBuf,
    pub account: String,
    pub mailboxes: Vec<String>,
    pub expected_version: String,
    pub timeout_seconds: u64,
    pub max_output_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Address {
    pub email: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedMessage {
    pub raw_sha256: String,
    #[serde(default)]
    pub from: Vec<Address>,
    #[serde(default)]
    pub to: Vec<Address>,
    #[serde(default)]
    pub cc: Vec<Address>,
    pub subject: String,
    pub sent_at: Option<String>,
    pub body: String,
    pub incomplete: bool,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Urgency {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Classification {
    pub state: String,
    pub urgency: Option<Urgency>,
    pub category_id: Option<String>,
    pub action_required: Option<bool>,
    pub taxonomy_revision: u64,
    pub model: String,
    pub classified_at: String,
    pub raw: serde_json::Value,
    #[serde(default)]
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailboxSnapshot {
    pub uid_validity: u64,
    pub uid_next: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SourceEnvelope {
    pub uid: u64,
    pub subject: String,
    pub from: Vec<Address>,
    pub sent_at: Option<String>,
    #[serde(default)]
    pub message_id: Option<String>,
    #[serde(default)]
    pub internal_date: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub flags: Vec<String>,
}
