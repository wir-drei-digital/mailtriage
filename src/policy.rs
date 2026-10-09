use crate::{
    domain::{AccountConfig, Classification, PolicyConfig, Urgency},
    provider::{Choice, Decision},
};
use chrono::Utc;

/// The policy thresholds applied to a `Decision` that passed
/// `provider::check_decision`.
pub fn apply(
    decision: Decision,
    account: &AccountConfig,
    policy: &PolicyConfig,
    incomplete: bool,
) -> Classification {
    let mut reasons = Vec::new();
    let category_id = confident(
        decision.category,
        policy.choice_confidence_min,
        "category",
        &mut reasons,
    );
    let urgency = confident(
        decision.urgency,
        policy.choice_confidence_min,
        "urgency",
        &mut reasons,
    )
    .and_then(|label| match label.as_str() {
        "low" => Some(Urgency::Low),
        "medium" => Some(Urgency::Medium),
        "high" => Some(Urgency::High),
        // The contract check offers only these three labels.
        _ => None,
    });
    let action_required = match decision.action_required {
        None => {
            reasons.push("action_required_missing".into());
            None
        }
        Some(yes) if yes >= policy.action_yes_min => Some(true),
        Some(yes) if yes <= policy.action_no_max => Some(false),
        Some(_) => {
            reasons.push("action_required_uncertain".into());
            None
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
    Classification {
        state: state.into(),
        urgency,
        category_id,
        action_required,
        taxonomy_revision: account.taxonomy_revision,
        model: decision.model,
        classified_at: Utc::now().to_rfc3339(),
        raw: decision.raw,
        reasons,
    }
}

/// The chosen label when the provider answered with enough confidence;
/// otherwise `None` and the reason.
fn confident(
    choice: Option<Choice>,
    confidence_min: f64,
    label: &str,
    reasons: &mut Vec<String>,
) -> Option<String> {
    match choice {
        None => {
            reasons.push(format!("{label}_missing"));
            None
        }
        Some(choice) if choice.confidence < confidence_min => {
            reasons.push(format!("{label}_low_confidence"));
            None
        }
        Some(choice) => Some(choice.choice),
    }
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
