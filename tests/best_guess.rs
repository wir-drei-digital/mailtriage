use mailtriage::{
    config,
    domain::AppConfig,
    policy,
    provider::{self, Choice, Decision, Questions},
};
use serde_json::{json, Value};

fn configured(mode: &str) -> AppConfig {
    let mut value = serde_json::to_value(config::default_config()).unwrap();
    value["policy"]["choice_mode"] = json!(mode);
    config::from_bytes(&serde_json::to_vec(&value).unwrap()).unwrap()
}

fn choice(label: &str, confidence: f64, probabilities: &[(&str, f64)]) -> Choice {
    Choice {
        choice: label.into(),
        confidence,
        probabilities: probabilities
            .iter()
            .map(|(k, v)| (k.to_string(), *v))
            .collect(),
    }
}

fn decision(category: Choice, urgency: Choice) -> Decision {
    Decision {
        model: "kev-latest".into(),
        raw: json!({"answers": {"category": {"choice": category.choice, "confidence": category.confidence, "probabilities": category.probabilities}, "urgency": {"choice": urgency.choice, "confidence": urgency.confidence, "probabilities": urgency.probabilities}}}),
        category: Some(category),
        urgency: Some(urgency),
        action_required: Some(0.5),
    }
}

fn category(confidence: f64, first: f64, second: f64) -> Choice {
    choice(
        "updates",
        confidence,
        &[
            ("correspondence", 0.0),
            ("transactions", 0.0),
            ("updates", first),
            ("newsletters", 0.0),
            ("promotions", 0.0),
            ("other", second),
        ],
    )
}

fn urgency(confidence: f64, first: f64, second: f64) -> Choice {
    choice(
        "low",
        confidence,
        &[("low", first), ("medium", second), ("high", 0.0)],
    )
}

fn apply(c: &AppConfig, d: Decision) -> Value {
    let account = &c.accounts["work"];
    provider::check_decision(&d, &Questions::for_account(account)).unwrap();
    serde_json::to_value(policy::apply(d, account, &c.policy, false)).unwrap()
}

#[test]
#[allow(clippy::approx_constant)] // Exact confidence scores captured from live Kev.
fn best_guess_accepts_the_live_kev_examples_and_keeps_the_raw_scores() {
    let c = configured("best_guess");
    for (label, confidence, probabilities, urgency_confidence, urgency_probabilities) in [
        (
            "updates",
            0.5235,
            [0.0109, 0.1784, 0.6029, 0.0266, 0.0341, 0.1471],
            0.5331,
            [0.6887, 0.2575, 0.0538],
        ),
        (
            "updates",
            0.4018,
            [0.03, 0.1081, 0.5015, 0.037, 0.1084, 0.215],
            0.4919,
            [0.6613, 0.2921, 0.0466],
        ),
        (
            "promotions",
            0.3855,
            [0.0511, 0.0044, 0.0281, 0.0248, 0.4879, 0.4037],
            0.3974,
            [0.5982, 0.3716, 0.0301],
        ),
    ] {
        let labels = [
            "correspondence",
            "transactions",
            "updates",
            "newsletters",
            "promotions",
            "other",
        ];
        let cat = choice(
            label,
            confidence,
            &labels.into_iter().zip(probabilities).collect::<Vec<_>>(),
        );
        let urg = choice(
            "low",
            urgency_confidence,
            &[
                ("low", urgency_probabilities[0]),
                ("medium", urgency_probabilities[1]),
                ("high", urgency_probabilities[2]),
            ],
        );
        let d = decision(cat, urg);
        let original = d.raw.clone();
        let result = apply(&c, d);
        assert_eq!(result["category_id"], label);
        assert_eq!(result["urgency"], "low");
        assert_eq!(result["best_guess_fields"], json!(["category", "urgency"]));
        assert_eq!(result["raw"], original);
        assert_eq!(result["action_required"], Value::Null);
        assert_eq!(result["state"], "uncertain");
        assert_eq!(
            result["reasons"],
            json!(["action_required_uncertain", "review_mode"])
        );
    }
}

#[test]
fn best_guess_withholds_low_confidence_and_close_or_tied_choices_independently() {
    let c = configured("best_guess");
    for (confidence, first, second, reason) in [
        (0.2499, 0.8, 0.2, "low_confidence"),
        (0.8, 0.52, 0.48, "ambiguous"),
        (0.8, 0.5, 0.5, "ambiguous"),
        (0.8, 0.496, 0.504, "ambiguous"),
    ] {
        let result = apply(
            &c,
            decision(category(confidence, first, second), urgency(0.6, 0.8, 0.2)),
        );
        assert_eq!(result["category_id"], Value::Null);
        assert_eq!(result["urgency"], "low");
        assert!(result["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!(format!("category_{reason}"))));
        let result = apply(
            &c,
            decision(category(0.6, 0.8, 0.2), urgency(confidence, first, second)),
        );
        assert_eq!(result["category_id"], "updates");
        assert_eq!(result["urgency"], Value::Null);
        assert!(result["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!(format!("urgency_{reason}"))));
    }
}

#[test]
fn best_guess_accepts_threshold_boundaries_and_still_withholds_missing_answers() {
    let c = configured("best_guess");
    let result = apply(
        &c,
        decision(category(0.25, 0.525, 0.475), urgency(0.25, 0.525, 0.475)),
    );
    assert_eq!(result["category_id"], "updates");
    assert_eq!(result["urgency"], "low");
    let mut d = decision(category(0.9, 0.8, 0.2), urgency(0.9, 0.8, 0.2));
    d.category = None;
    d.urgency = None;
    let result = apply(&c, d);
    assert_eq!(result["category_id"], Value::Null);
    assert_eq!(result["urgency"], Value::Null);
    assert_eq!(
        result["reasons"],
        json!([
            "category_missing",
            "urgency_missing",
            "action_required_uncertain",
            "review_mode"
        ])
    );
}

#[test]
fn legacy_configs_keep_strict_behavior_and_serialization() {
    let mut original = serde_json::to_value(config::default_config()).unwrap();
    original["policy"]
        .as_object_mut()
        .unwrap()
        .remove("choice_mode");
    original["schema_version"] = json!(3);
    let loaded = config::from_bytes(&serde_json::to_vec(&original).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(&loaded).unwrap(), original);
    for c in [loaded, configured("strict")] {
        let result = apply(
            &c,
            decision(category(0.6, 0.8, 0.2), urgency(0.6, 0.8, 0.2)),
        );
        assert_eq!(result["category_id"], Value::Null);
        assert_eq!(result["urgency"], Value::Null);
        assert!(result.get("best_guess_fields").is_none());
    }
}

#[test]
fn new_configs_default_to_best_guess_for_category_and_urgency() {
    let c = config::default_config();
    let result = apply(
        &c,
        decision(category(0.4, 0.8, 0.2), urgency(0.4, 0.8, 0.2)),
    );
    assert_eq!(result["category_id"], "updates");
    assert_eq!(result["urgency"], "low");
}

#[test]
fn account_mode_overrides_the_global_mode() {
    for (global, account, accepted) in [
        ("strict", "best_guess", true),
        ("best_guess", "strict", false),
    ] {
        let mut v = serde_json::to_value(configured(global)).unwrap();
        v["accounts"]["work"]["choice_mode"] = json!(account);
        let c = config::from_bytes(&serde_json::to_vec(&v).unwrap()).unwrap();
        let result = apply(
            &c,
            decision(category(0.6, 0.8, 0.2), urgency(0.6, 0.8, 0.2)),
        );
        assert_eq!(result["category_id"].is_string(), accepted);
        assert_eq!(result["urgency"].is_string(), accepted);
    }
}

#[test]
fn best_guess_limits_are_configurable_and_validated() {
    let mut v = serde_json::to_value(configured("best_guess")).unwrap();
    v["policy"]["best_guess_confidence_min"] = json!(0.5);
    v["policy"]["best_guess_margin_min"] = json!(0.2);
    let c = config::from_bytes(&serde_json::to_vec(&v).unwrap()).unwrap();
    let result = apply(
        &c,
        decision(category(0.49, 0.8, 0.2), urgency(0.6, 0.55, 0.45)),
    );
    assert_eq!(result["category_id"], Value::Null);
    assert_eq!(result["urgency"], Value::Null);
    for field in ["best_guess_confidence_min", "best_guess_margin_min"] {
        for invalid in [-0.01, 1.01] {
            let mut bad = v.clone();
            bad["policy"][field] = json!(invalid);
            assert!(
                config::from_bytes(&serde_json::to_vec(&bad).unwrap()).is_err(),
                "{field}={invalid}"
            );
        }
    }
    v["policy"]["choice_mode"] = json!("guess");
    assert!(config::from_bytes(&serde_json::to_vec(&v).unwrap()).is_err());
}

#[test]
fn writing_choice_settings_uses_a_schema_that_older_binaries_refuse() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    config::save(&path, &configured("best_guess")).unwrap();
    let written: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(written["schema_version"], 5);
    assert_eq!(written["policy"]["choice_mode"], "best_guess");
    let loaded = config::load(&path).unwrap();
    let result = apply(
        &loaded,
        decision(category(0.6, 0.8, 0.2), urgency(0.6, 0.8, 0.2)),
    );
    assert_eq!(result["category_id"], "updates");

    // Even with a strict global mode, account overrides and custom limits
    // must survive a save and must not be silently lost to older binaries.
    for (section, field, value) in [
        ("account", "choice_mode", json!("best_guess")),
        ("policy", "best_guess_confidence_min", json!(0.4)),
        ("policy", "best_guess_margin_min", json!(0.1)),
    ] {
        let mut value_json = serde_json::to_value(configured("strict")).unwrap();
        let target = if section == "account" {
            &mut value_json["accounts"]["work"]
        } else {
            &mut value_json["policy"]
        };
        target[field] = value.clone();
        let c = config::from_bytes(&serde_json::to_vec(&value_json).unwrap()).unwrap();
        config::save(&path, &c).unwrap();
        let written: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["schema_version"], 5);
        let loaded = serde_json::to_value(config::load(&path).unwrap()).unwrap();
        let settings = if section == "account" {
            &loaded["accounts"]["work"]
        } else {
            &loaded["policy"]
        };
        assert_eq!(settings[field], value);
    }
}

mod common;

#[test]
fn changing_account_mode_reclassifies_cached_messages_and_exposes_best_guess_fields() {
    let h = common::Harness::new(mailtriage::domain::FilingMode::Off);
    h.edit(|c| c.policy.choice_mode = mailtriage::domain::ChoiceMode::Strict);
    let bytes = common::mail("mode-change", "Your receipt", "Thanks for your payment.");
    let first = h.service().classify("work", &bytes, "rfc822").unwrap();
    assert_eq!(first["outcome"], "classified");
    h.edit(|c| {
        let mut value = serde_json::to_value(&*c).unwrap();
        value["accounts"]["work"]["choice_mode"] = json!("best_guess");
        *c = config::from_bytes(&serde_json::to_vec(&value).unwrap()).unwrap();
    });
    let second = h.service().classify("work", &bytes, "rfc822").unwrap();
    assert_eq!(second["outcome"], "classified");
    assert_eq!(
        second["item"]["classification"]["best_guess_fields"],
        json!(["category", "urgency"])
    );

    let id = second["item"]["id"].as_str().unwrap();
    let mut service = h.service();
    service
        .correct("work", id, json!({"category_id":"updates"}), None)
        .unwrap();
    let read = service.read("work", id).unwrap();
    assert_eq!(
        read["item"]["classification"]["best_guess_fields"],
        json!(["urgency"])
    );
    assert_eq!(
        read["item"]["model_decision"]["best_guess_fields"],
        json!(["category", "urgency"])
    );
    service
        .correct("work", id, json!({"urgency":"high"}), None)
        .unwrap();
    let read = service.read("work", id).unwrap();
    assert!(read["item"]["classification"]
        .get("best_guess_fields")
        .is_none());
    service
        .correct("work", id, json!({}), Some("category_id"))
        .unwrap();
    let read = service.read("work", id).unwrap();
    assert_eq!(
        read["item"]["classification"]["best_guess_fields"],
        json!(["category"])
    );
    service
        .correct("work", id, json!({}), Some("urgency"))
        .unwrap();
    let read = service.read("work", id).unwrap();
    assert_eq!(
        read["item"]["classification"]["best_guess_fields"],
        json!(["category", "urgency"])
    );
}

#[test]
fn zero_margin_still_rejects_ties_and_strict_accepts_its_confidence_boundary() {
    let mut value = serde_json::to_value(configured("best_guess")).unwrap();
    value["policy"]["best_guess_margin_min"] = json!(0.0);
    let c = config::from_bytes(&serde_json::to_vec(&value).unwrap()).unwrap();
    let result = apply(
        &c,
        decision(category(0.9, 0.5, 0.5), urgency(0.9, 0.5, 0.5)),
    );
    assert_eq!(result["category_id"], Value::Null);
    assert_eq!(result["urgency"], Value::Null);
    let c = configured("strict");
    let result = apply(
        &c,
        decision(category(0.75, 0.8, 0.2), urgency(0.75, 0.8, 0.2)),
    );
    assert_eq!(result["category_id"], "updates");
    assert_eq!(result["urgency"], "low");
}
