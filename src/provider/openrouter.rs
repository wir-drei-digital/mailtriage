//! OpenRouter's Decisions API, with any Decisions model. The endpoint is
//! pinned, so the key is sent nowhere else.
use super::{Choice, Decision, DecisionProvider, DecisionRequest, Question};
use crate::domain::ProviderConfig;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Map, Value};
use std::{collections::BTreeMap, io::Read, time::Duration};

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub const DECISIONS_ENDPOINT: &str = "https://openrouter.ai/api/alpha/decisions";
/// What a new setup writes: an alias OpenRouter moves to the newest Jev.
pub const DEFAULT_MODEL: &str = "typesafe/jev-latest";
pub const DEFAULT_KEY_ENV: &str = "OPENROUTER_API_KEY";

pub struct OpenRouter;

impl DecisionProvider for OpenRouter {
    fn validate(&self, config: &ProviderConfig) -> Result<()> {
        if config.endpoint.trim().is_empty() {
            bail!("OpenRouter Decisions endpoint is required");
        }
        let url = reqwest::Url::parse(&config.endpoint)
            .context("invalid OpenRouter Decisions endpoint")?;
        if url.as_str() != DECISIONS_ENDPOINT
            && !(url.scheme() == "http"
                && url.host_str() == Some("127.0.0.1")
                && url.path() == "/api/alpha/decisions")
        {
            bail!("provider endpoint must be the OpenRouter Decisions endpoint");
        }
        Ok(())
    }

    fn key_account(&self) -> Option<&'static str> {
        Some("openrouter")
    }

    fn label(&self) -> &'static str {
        "OpenRouter (a Decisions model, Jev by default; needs an API key)"
    }

    fn new_config(&self) -> ProviderConfig {
        ProviderConfig {
            kind: "openrouter".to_owned(),
            model: DEFAULT_MODEL.to_owned(),
            endpoint: DECISIONS_ENDPOINT.to_owned(),
            api_key_command: None,
            api_key_env: DEFAULT_KEY_ENV.to_owned(),
            timeout_seconds: 30,
        }
    }

    fn decide(
        &self,
        config: &ProviderConfig,
        request: &DecisionRequest,
        key: Option<&str>,
    ) -> Result<Decision> {
        let key = key.ok_or_else(|| anyhow!("OpenRouter API key is missing"))?;
        parse(send(config, &request_body(config, request), key)?)
    }
}

/// Today's `{model, state, questions}` body. Key order and escaping come
/// from `serde_json`'s sorted maps, so the bytes on the wire are pinned by
/// `tests/provider_contract.rs`.
fn request_body(config: &ProviderConfig, request: &DecisionRequest) -> Value {
    let message = &request.message;
    let questions = &request.questions;
    json!({
        "model": config.model,
        "state": {
            "account_identity": request.identity,
            "recipient_timezone": request.timezone,
            "recipient_brief": request.brief,
            "evaluated_at": request.evaluated_at,
            "from": message.from,
            "to": message.to,
            "cc": message.cc,
            "subject": message.subject,
            "sent_at": message.sent_at,
            "body": message.body,
            "incomplete": message.incomplete,
            "warnings": message.warnings,
        },
        "questions": {
            "category": question("choice", &questions.category),
            "urgency": question("choice", &questions.urgency),
            "action_required": question("noul", &questions.action_required),
        }
    })
}

fn question(kind: &str, question: &Question) -> Value {
    let criteria: Map<String, Value> = question
        .criteria
        .iter()
        .map(|(label, text)| (label.clone(), Value::String(text.clone())))
        .collect();
    json!({
        "type": kind,
        "instructions": question.instructions,
        "criteria": criteria,
    })
}

fn send(config: &ProviderConfig, body: &Value, key: &str) -> Result<Value> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(config.timeout_seconds))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("failed to build provider client")?;
    let response = client
        .post(&config.endpoint)
        .bearer_auth(key)
        .json(body)
        .send()
        .map_err(|_| anyhow!("OpenRouter Decisions request failed"))?;
    if !response.status().is_success() {
        bail!(
            "OpenRouter Decisions returned HTTP {}",
            response.status().as_u16()
        );
    }
    if response
        .content_length()
        .is_some_and(|len| len > MAX_RESPONSE_BYTES as u64)
    {
        bail!("OpenRouter Decisions response is too large");
    }
    let mut limited = response.take((MAX_RESPONSE_BYTES + 1) as u64);
    let mut bytes = Vec::new();
    limited
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow!("failed to read OpenRouter Decisions response"))?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        bail!("OpenRouter Decisions response is too large");
    }
    serde_json::from_slice(&bytes).context("invalid OpenRouter Decisions JSON")
}

/// The typed answers as a `Decision`; `raw` is the whole response. A
/// missing answer is `None`; a wrong `type` or a non-number is an error.
fn parse(response: Value) -> Result<Decision> {
    let answers = response
        .get("answers")
        .and_then(Value::as_object)
        .context("OpenRouter Decisions response lacks typed answers")?;
    let category = choice(answers.get("category"), "category")?;
    let urgency = choice(answers.get("urgency"), "urgency")?;
    let action_required = match answers.get("action_required") {
        None => None,
        Some(answer) => {
            if answer.get("type").and_then(Value::as_str) != Some("noul") {
                bail!("action_required answer must have type noul");
            }
            Some(number(
                answer.get("noul").context("action_required noul missing")?,
                "action_required noul",
            )?)
        }
    };
    let model = response
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Ok(Decision {
        model,
        category,
        urgency,
        action_required,
        raw: response,
    })
}

fn choice(answer: Option<&Value>, label: &str) -> Result<Option<Choice>> {
    let Some(answer) = answer else {
        return Ok(None);
    };
    if answer.get("type").and_then(Value::as_str) != Some("choice") {
        bail!("{label} answer must have type choice");
    }
    let selected = answer
        .get("choice")
        .and_then(Value::as_str)
        .with_context(|| format!("{label} choice missing"))?;
    let confidence = number(
        answer
            .get("confidence")
            .context("choice confidence missing")?,
        &format!("{label} confidence"),
    )?;
    let distribution = answer
        .get("probabilities")
        .and_then(Value::as_object)
        .with_context(|| format!("{label} probabilities missing"))?;
    let mut probabilities = BTreeMap::new();
    for (class, p) in distribution {
        probabilities.insert(
            class.clone(),
            number(p, &format!("{label} probability {class}"))?,
        );
    }
    Ok(Some(Choice {
        choice: selected.to_owned(),
        confidence,
        probabilities,
    }))
}

fn number(value: &Value, label: &str) -> Result<f64> {
    value
        .as_f64()
        .with_context(|| format!("{label} must be a number"))
}
