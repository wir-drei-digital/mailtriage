use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub schema_version: u32,
    pub state_dir: PathBuf,
    pub provider: ProviderConfig,
    pub policy: PolicyConfig,
    pub accounts: BTreeMap<String, AccountConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub kind: String,
    pub model: String,
    pub endpoint: String,
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
    pub himalaya: Option<HimalayaConfig>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceEnvelope {
    pub uid: u64,
    pub subject: String,
    pub from: Vec<Address>,
    pub sent_at: Option<String>,
}
