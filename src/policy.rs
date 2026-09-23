use crate::domain::{AccountConfig, Classification, PolicyConfig, Urgency};
use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde_json::Value;
use std::collections::BTreeSet;

fn probability(value: &Value, label: &str) -> Result<f64> {
    let p = value
        .as_f64()
        .with_context(|| format!("{label} must be a number"))?;
    if !p.is_finite() || !(0.0..=1.0).contains(&p) {
        bail!("{label} must be between 0 and 1");
    }
    Ok(p)
}

fn choice(
    answer: Option<&Value>,
    allowed: &BTreeSet<&str>,
    confidence_min: f64,
    label: &str,
    reasons: &mut Vec<String>,
) -> Result<Option<String>> {
    let Some(answer) = answer else {
        reasons.push(format!("{label}_missing"));
        return Ok(None);
    };
    if answer.get("type").and_then(Value::as_str) != Some("choice") {
        bail!("{label} answer must have type choice");
    }
    let selected = answer
        .get("choice")
        .and_then(Value::as_str)
        .with_context(|| format!("{label} choice missing"))?;
    if !allowed.contains(selected) {
        bail!("{label} choice {:?} is not configured", selected);
    }
    let confidence = probability(
        answer
            .get("confidence")
            .context("choice confidence missing")?,
        &format!("{label} confidence"),
    )?;
    let distribution = answer
        .get("probabilities")
        .and_then(Value::as_object)
        .with_context(|| format!("{label} probabilities missing"))?;
    if distribution.is_empty() {
        bail!("{label} probabilities cannot be empty");
    }
    if distribution.len() != allowed.len() {
        bail!("{label} probabilities must include every configured choice");
    }
    let mut total = 0.0;
    for (key, value) in distribution {
        if !allowed.contains(key.as_str()) {
            bail!("{label} probability class {:?} is not configured", key);
        }
        total += probability(value, &format!("{label} probability {key}"))?;
    }
    if !distribution.contains_key(selected) {
        bail!("{label} probabilities omit chosen class");
    }
    let selected_probability = probability(
        &distribution[selected],
        &format!("{label} selected probability"),
    )?;
    if distribution
        .values()
        .filter_map(Value::as_f64)
        .any(|p| p > selected_probability + 0.01)
    {
        bail!("{label} choice does not match highest probability");
    }
    // Decisions API rounds probabilities to two decimal places.
    if (total - 1.0).abs() > 0.031 {
        bail!("{label} probabilities must total approximately 1");
    }
    if confidence < confidence_min {
        reasons.push(format!("{label}_low_confidence"));
        return Ok(None);
    }
    Ok(Some(selected.to_owned()))
}

pub fn decode_response(
    raw: &Value,
    account: &AccountConfig,
    policy: &PolicyConfig,
    incomplete: bool,
) -> Result<Classification> {
    let model = raw
        .get("model")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .context("Decisions response missing model")?;
    let answers = raw
        .get("answers")
        .and_then(Value::as_object)
        .context("Decisions response missing answers object")?;
    let configured: BTreeSet<&str> = account.categories.iter().map(|c| c.id.as_str()).collect();
    if configured.is_empty() {
        bail!("account has no configured categories");
    }
    let urgency_set: BTreeSet<&str> = ["low", "medium", "high"].into_iter().collect();
    let mut reasons = Vec::new();
    let category_id = choice(
        answers.get("category"),
        &configured,
        policy.choice_confidence_min,
        "category",
        &mut reasons,
    )?;
    let urgency = choice(
        answers.get("urgency"),
        &urgency_set,
        policy.choice_confidence_min,
        "urgency",
        &mut reasons,
    )?
    .map(|s| match s.as_str() {
        "low" => Urgency::Low,
        "medium" => Urgency::Medium,
        "high" => Urgency::High,
        _ => unreachable!(),
    });
    let action_required = match answers.get("action_required") {
        None => {
            reasons.push("action_required_missing".into());
            None
        }
        Some(answer) => {
            if answer.get("type").and_then(Value::as_str) != Some("noul") {
                bail!("action_required answer must have type noul");
            }
            let yes = probability(
                answer.get("noul").context("action_required noul missing")?,
                "action_required noul",
            )?;
            if yes >= policy.action_yes_min {
                Some(true)
            } else if yes <= policy.action_no_max {
                Some(false)
            } else {
                reasons.push("action_required_uncertain".into());
                None
            }
        }
    };
    if incomplete {
        reasons.push("input_incomplete".into());
    }
    if policy.review_mode {
        reasons.push("review_mode".into());
    }
    let state = if reasons.is_empty() {
        "ready"
    } else {
        "uncertain"
    };
    Ok(Classification {
        state: state.into(),
        urgency,
        category_id,
        action_required,
        taxonomy_revision: account.taxonomy_revision,
        model: model.into(),
        classified_at: Utc::now().to_rfc3339(),
        raw: raw.clone(),
        reasons,
    })
}

pub fn attention(classification: &Classification, review_state: &str) -> Vec<String> {
    if review_state.eq_ignore_ascii_case("done") {
        return Vec::new();
    }
    let mut reasons = Vec::new();
    match classification.state.as_str() {
        "pending" | "failed" | "error" | "uncertain" => reasons.push(classification.state.clone()),
        "ready" => {}
        _ => reasons.push("unknown_state".into()),
    }
    if classification.urgency == Some(Urgency::High) {
        reasons.push("high_urgency".into());
    } else if classification.urgency == Some(Urgency::Medium) {
        reasons.push("medium_urgency".into());
    }
    if classification.action_required == Some(true) {
        reasons.push("action_required".into());
    }
    if classification.urgency.is_none()
        || classification.category_id.is_none()
        || classification.action_required.is_none()
    {
        reasons.push("incomplete_decision".into());
    }
    if review_state.eq_ignore_ascii_case("review")
        || review_state.eq_ignore_ascii_case("needs_review")
    {
        reasons.push("needs_review".into());
    }
    if classification.reasons.iter().any(|r| r == "review_mode") {
        reasons.push("review_mode".into());
    }
    reasons
}
