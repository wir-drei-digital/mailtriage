//! TypeSafe's System One wire format, which OpenRouter's Decisions API and
//! a Kev server share: the `{model, state, questions}` request, the client
//! (timeout, no redirects, 1 MiB response limit) and the typed answers.
//! `name` goes into every error text, so OpenRouter keeps its texts
//! ("OpenRouter Decisions returned HTTP 429") and Kev gets its own ("Kev
//! server returned HTTP 429"). Response bodies never reach an error.
use super::{Choice, Decision, DecisionRequest, Question};
use crate::domain::ProviderConfig;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Map, Value};
use std::{collections::BTreeMap, io::Read, time::Duration};

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// Today's `{model, state, questions}` body. Key order and escaping come
/// from `serde_json`'s sorted maps, so the bytes on the wire are pinned by
/// `tests/provider_contract.rs`.
pub fn request_body(config: &ProviderConfig, request: &DecisionRequest) -> Value {
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

/// POSTs `body` to `url`: Bearer `key` when given, the config's timeout, no
/// redirects, at most 1 MiB back.
pub fn send(
    config: &ProviderConfig,
    url: &str,
    body: &Value,
    key: Option<&str>,
    name: &str,
) -> Result<Value> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(config.timeout_seconds))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("failed to build provider client")?;
    let mut post = client.post(url);
    if let Some(key) = key {
        post = post.bearer_auth(key);
    }
    let response = post
        .json(body)
        .send()
        .map_err(|_| anyhow!("{name} request failed"))?;
    if !response.status().is_success() {
        bail!("{name} returned HTTP {}", response.status().as_u16());
    }
    if response
        .content_length()
        .is_some_and(|len| len > MAX_RESPONSE_BYTES as u64)
    {
        bail!("{name} response is too large");
    }
    let mut limited = response.take((MAX_RESPONSE_BYTES + 1) as u64);
    let mut bytes = Vec::new();
    limited
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow!("failed to read {name} response"))?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        bail!("{name} response is too large");
    }
    serde_json::from_slice(&bytes).with_context(|| format!("invalid {name} JSON"))
}

/// The typed answers as a `Decision`; `raw` is the whole response. A
/// missing answer is `None`; a wrong `type` or a non-number is an error.
pub fn parse(response: Value, name: &str) -> Result<Decision> {
    let answers = response
        .get("answers")
        .and_then(Value::as_object)
        .with_context(|| format!("{name} response lacks typed answers"))?;
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
