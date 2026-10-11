//! The classifier boundary. Setup, `doctor`, the service files and the
//! policy layer reach a classifier only through this module: a
//! `DecisionProvider` answers the three typed questions of `questions` with a
//! `Decision`, `check_decision` holds every `Decision` to one contract, and
//! `policy::apply` turns it into a `Classification`.
//!
//! Only Decisions-style services fit: each choice comes with a confidence and
//! a probability per offered label. To add one:
//!
//! 1. implement `DecisionProvider` in a module next to `openrouter`, `kev` and
//!    `fake`;
//! 2. add its kind to `KINDS` (setup offers the kinds in that order) and to
//!    `provider_for`;
//! 3. give it a fixture in `tests/provider_contract.rs`, whose suite runs
//!    against every kind in `KINDS`.
//!
//! `systemone` holds the request, client and answer parsing every System One
//! service shares.
pub mod fake;
pub mod kev;
pub mod openrouter;
pub mod questions;
pub mod systemone;

pub use openrouter::DECISIONS_ENDPOINT;
pub use questions::{Question, Questions};

use crate::{
    domain::{AccountConfig, Classification, NormalizedMessage, PolicyConfig, ProviderConfig},
    secrets::KeyCache,
};
use anyhow::{bail, Result};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

/// What every provider is asked.
#[derive(Debug, Clone)]
pub struct DecisionRequest {
    pub identity: String,
    pub timezone: String,
    pub brief: String,
    /// RFC 3339, set once per request.
    pub evaluated_at: String,
    pub message: NormalizedMessage,
    pub questions: Questions,
}

impl DecisionRequest {
    pub fn new(account: &AccountConfig, message: &NormalizedMessage, evaluated_at: String) -> Self {
        DecisionRequest {
            identity: account.identity.clone(),
            timezone: account.timezone.clone(),
            brief: account.brief.clone(),
            evaluated_at,
            message: message.clone(),
            questions: Questions::for_account(account),
        }
    }
}

/// One answered choice question.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub choice: String,
    pub confidence: f64,
    pub probabilities: BTreeMap<String, f64>,
}

/// A provider's answer, before policy.
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    /// The model as the response names it.
    pub model: String,
    /// `None`: the provider gave no answer.
    pub category: Option<Choice>,
    pub urgency: Option<Choice>,
    /// The probability of "yes".
    pub action_required: Option<f64>,
    /// The provider's response, stored for audit.
    pub raw: Value,
}

/// The provider cannot answer now, for a reason outside the message: a
/// local server that is not installed or does not start, a server that
/// does not answer. Like a missing key it skips classification: no job is
/// taken, no attempt is used, and the reason is reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderUnavailable(pub String);

impl fmt::Display for ProviderUnavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProviderUnavailable {}

pub trait DecisionProvider: Sync {
    /// Whether the provider can answer now. Called once per command, after
    /// the key check and before any job is taken; `Err` skips
    /// classification like a missing key. Ready by default.
    fn prepare(
        &self,
        _config: &ProviderConfig,
        _key: Option<&str>,
    ) -> Result<(), ProviderUnavailable> {
        Ok(())
    }

    /// Provider-specific rules; the shared rules have already passed.
    fn validate(&self, config: &ProviderConfig) -> Result<()>;
    /// The key store's account name (`openrouter`) when `config` needs a
    /// key; `None` when it needs none (the offline demo, a Kev server on
    /// this machine).
    fn key_account(&self, config: &ProviderConfig) -> Option<&'static str>;
    /// Setup's menu entry for this kind.
    fn label(&self) -> &'static str;
    /// The provider block a new setup writes for this kind.
    fn new_config(&self) -> ProviderConfig;
    fn decide(
        &self,
        config: &ProviderConfig,
        request: &DecisionRequest,
        key: Option<&str>,
    ) -> Result<Decision>;
}

/// Registered kinds, in the order setup offers them.
pub const KINDS: &[&str] = &["openrouter", "kev", "fake"];

pub fn provider_for(kind: &str) -> Result<&'static dyn DecisionProvider> {
    match kind {
        "openrouter" => Ok(&openrouter::OpenRouter),
        "kev" => Ok(&kev::Kev),
        "fake" => Ok(&fake::Fake),
        _ => bail!("unsupported provider kind"),
    }
}

/// Whose key a config needs, for messages: "OpenRouter" or "Kev server".
pub fn key_owner(config: &ProviderConfig) -> &'static str {
    if config.kind == "kev" {
        "Kev server"
    } else {
        "OpenRouter"
    }
}

/// The key store account of `config`'s provider: `None` when it needs no
/// key, and for an unknown kind, which validation refuses.
pub fn key_account(config: &ProviderConfig) -> Option<&'static str> {
    provider_for(&config.kind)
        .ok()
        .and_then(|provider| provider.key_account(config))
}

/// `prepare` of `config`'s provider, with the key when it needs one (the
/// caller has checked that the key resolves). An unknown kind is ready
/// here; validation refuses it.
pub fn prepare(config: &ProviderConfig, key: &KeyCache) -> Result<(), ProviderUnavailable> {
    let Ok(provider) = provider_for(&config.kind) else {
        return Ok(());
    };
    let key = provider
        .key_account(config)
        .and_then(|_| key.get(config).ok());
    provider.prepare(config, key.as_deref())
}

/// The shared rules, then the provider's own.
pub fn validate_configuration(config: &ProviderConfig) -> Result<()> {
    if config.timeout_seconds == 0 || config.timeout_seconds > 300 {
        bail!("provider timeout must be between 1 and 300 seconds");
    }
    if config.model.trim().is_empty() {
        bail!("provider.model cannot be empty");
    }
    let provider = provider_for(&config.kind)?;
    if config.managed.is_some() && config.kind != "kev" {
        bail!("provider.managed is only for kind kev");
    }
    if provider.key_account(config).is_some() {
        match &config.api_key_command {
            Some(command) => {
                if command.first().is_none_or(|program| program.is_empty()) {
                    bail!("provider.api_key_command must name a program");
                }
            }
            None if config.api_key_env.is_empty() => {
                if config.kind == "kev" {
                    bail!("Kev server needs api_key_command or api_key_env")
                }
                bail!("OpenRouter provider needs api_key_command or api_key_env")
            }
            None => {}
        }
        if !config.api_key_env.is_empty() && !valid_env_name(&config.api_key_env) {
            bail!("provider API key environment variable name is invalid");
        }
    }
    provider.validate(config)
}

/// Upper-case ASCII letters, digits and `_`, nonempty.
pub fn valid_env_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

pub fn classify(
    config: &ProviderConfig,
    account: &AccountConfig,
    message: &NormalizedMessage,
    policy: &PolicyConfig,
) -> Result<Classification> {
    classify_with_key(config, account, message, policy, &KeyCache::default())
}

/// `classify`, resolving the key through `key` when the provider needs one.
pub fn classify_with_key(
    config: &ProviderConfig,
    account: &AccountConfig,
    message: &NormalizedMessage,
    policy: &PolicyConfig,
    key: &KeyCache,
) -> Result<Classification> {
    validate_configuration(config)?;
    if account.categories.is_empty() {
        bail!("account has no categories");
    }
    let provider = provider_for(&config.kind)?;
    let request = DecisionRequest::new(account, message, chrono::Utc::now().to_rfc3339());
    let key = match provider.key_account(config) {
        Some(_) => Some(key.get(config)?),
        None => None,
    };
    let decision = provider.decide(config, &request, key.as_deref())?;
    check_decision(&decision, &request.questions)?;
    Ok(crate::policy::apply(
        decision,
        account,
        policy,
        message.incomplete,
    ))
}

/// The contract every `Decision` meets before policy, whichever provider
/// made it. A missing answer passes; policy turns it into a `*_missing`
/// reason.
pub fn check_decision(decision: &Decision, questions: &Questions) -> Result<()> {
    if decision.model.trim().is_empty() {
        bail!("Decisions response missing model");
    }
    if let Some(choice) = &decision.category {
        check_choice(choice, &questions.category, "category")?;
    }
    if let Some(choice) = &decision.urgency {
        check_choice(choice, &questions.urgency, "urgency")?;
    }
    if let Some(yes) = decision.action_required {
        unit(yes, "action_required noul")?;
    }
    Ok(())
}

fn unit(p: f64, label: &str) -> Result<()> {
    if !p.is_finite() || !(0.0..=1.0).contains(&p) {
        bail!("{label} must be between 0 and 1");
    }
    Ok(())
}

fn check_choice(choice: &Choice, question: &Question, label: &str) -> Result<()> {
    let offered: BTreeSet<&str> = question.labels().collect();
    let selected = choice.choice.as_str();
    if !offered.contains(selected) {
        bail!("{label} choice {:?} is not configured", selected);
    }
    unit(choice.confidence, &format!("{label} confidence"))?;
    let distribution = &choice.probabilities;
    if distribution.is_empty() {
        bail!("{label} probabilities cannot be empty");
    }
    if distribution.len() != offered.len() {
        bail!("{label} probabilities must include every configured choice");
    }
    let mut total = 0.0;
    for (class, &p) in distribution {
        if !offered.contains(class.as_str()) {
            bail!("{label} probability class {:?} is not configured", class);
        }
        unit(p, &format!("{label} probability {class}"))?;
        total += p;
    }
    let Some(&selected_probability) = distribution.get(selected) else {
        bail!("{label} probabilities omit chosen class");
    };
    if distribution
        .values()
        .any(|&p| p > selected_probability + 0.01)
    {
        bail!("{label} choice does not match highest probability");
    }
    // The Decisions API rounds probabilities to two decimal places.
    if (total - 1.0).abs() > 0.031 {
        bail!("{label} probabilities must total approximately 1");
    }
    Ok(())
}
