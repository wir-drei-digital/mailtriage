//! The offline demo: fixed keyword rules, no network request, no key.
//! Deterministic, so tests and `init` configs give the same answers.
use super::{Choice, Decision, DecisionProvider, DecisionRequest};
use crate::domain::ProviderConfig;
use anyhow::{bail, Result};
use serde_json::json;
use std::collections::BTreeMap;

pub struct Fake;

impl DecisionProvider for Fake {
    fn validate(&self, _config: &ProviderConfig) -> Result<()> {
        Ok(())
    }

    fn key_account(&self) -> Option<&'static str> {
        None
    }

    fn label(&self) -> &'static str {
        "Offline demo (fake; keyword rules, no key)"
    }

    /// The `init` provider: model `fake/offline`.
    fn new_config(&self) -> ProviderConfig {
        crate::config::default_config().provider
    }

    fn decide(
        &self,
        config: &ProviderConfig,
        request: &DecisionRequest,
        _key: Option<&str>,
    ) -> Result<Decision> {
        let message = &request.message;
        let questions = &request.questions;
        let haystack = format!("{} {}", message.subject, message.body).to_lowercase();
        let keyword = if haystack.contains("invoice")
            || haystack.contains("receipt")
            || haystack.contains("payment")
        {
            "transactions"
        } else if haystack.contains("newsletter") {
            "newsletters"
        } else if haystack.contains("sale") || haystack.contains("discount") {
            "promotions"
        } else if haystack.contains("update") {
            "updates"
        } else {
            "correspondence"
        };
        let labels: Vec<&str> = questions.category.labels().collect();
        let Some(&first) = labels.first() else {
            bail!("account has no categories");
        };
        let category = labels
            .iter()
            .copied()
            .find(|&id| id == keyword)
            .or(questions.catch_all.as_deref())
            .unwrap_or(first);
        let urgency = if haystack.contains("urgent")
            || haystack.contains("today")
            || haystack.contains("deadline")
        {
            "high"
        } else if haystack.contains("tomorrow") || haystack.contains("soon") {
            "medium"
        } else {
            "low"
        };
        let action = haystack.contains("please reply")
            || haystack.contains("can you")
            || haystack.contains("please review")
            || haystack.contains("payment due")
            || haystack.contains("please pay");
        let category_probabilities = one_hot(&labels, category);
        let urgency_probabilities = one_hot(&["low", "medium", "high"], urgency);
        let yes = if action { 1.0 } else { 0.0 };
        let raw = json!({
            "model": config.model,
            "provider": "fake",
            "answers": {
                "category": { "type": "choice", "choice": category, "confidence": 1.0, "probabilities": category_probabilities },
                "urgency": { "type": "choice", "choice": urgency, "confidence": 1.0, "probabilities": urgency_probabilities },
                "action_required": { "type": "noul", "noul": yes }
            }
        });
        Ok(Decision {
            model: config.model.clone(),
            category: Some(Choice {
                choice: category.to_owned(),
                confidence: 1.0,
                probabilities: category_probabilities,
            }),
            urgency: Some(Choice {
                choice: urgency.to_owned(),
                confidence: 1.0,
                probabilities: urgency_probabilities,
            }),
            action_required: Some(yes),
            raw,
        })
    }
}

fn one_hot(labels: &[&str], chosen: &str) -> BTreeMap<String, f64> {
    labels
        .iter()
        .map(|&label| (label.to_owned(), if label == chosen { 1.0 } else { 0.0 }))
        .collect()
}
