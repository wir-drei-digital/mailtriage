use mailtriage::{
    config,
    domain::AccountConfig,
    normalize, policy,
    provider::{self, Choice, Decision, Questions},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// `labels` with `chosen` at 0.9 and the rest of 0.1 spread evenly.
fn distribution(labels: &[&str], chosen: &str) -> BTreeMap<String, f64> {
    let rest = 0.1 / (labels.len() - 1) as f64;
    labels
        .iter()
        .map(|&l| (l.to_owned(), if l == chosen { 0.9 } else { rest }))
        .collect()
}

const CATEGORIES: [&str; 6] = [
    "correspondence",
    "transactions",
    "updates",
    "newsletters",
    "promotions",
    "other",
];
const URGENCIES: [&str; 3] = ["low", "medium", "high"];

/// A decision of the shape the old `response` fixture had: the chosen
/// labels at 0.9, both choices at `confidence`.
fn decision(category: &str, urgency: &str, action: f64, confidence: f64) -> Decision {
    Decision {
        model: "typesafe/jev-1.13-20260917".into(),
        category: Some(Choice {
            choice: category.into(),
            confidence,
            probabilities: distribution(&CATEGORIES, category),
        }),
        urgency: Some(Choice {
            choice: urgency.into(),
            confidence,
            probabilities: distribution(&URGENCIES, urgency),
        }),
        action_required: Some(action),
        raw: json!({"fixture": true}),
    }
}

fn questions(account: &AccountConfig) -> Questions {
    Questions::for_account(account)
}

/// `decision`, after it passed the contract check as `classify` runs it.
fn checked(d: Decision, account: &AccountConfig) -> Decision {
    provider::check_decision(&d, &questions(account)).unwrap();
    d
}

fn contract_error(d: &Decision, account: &AccountConfig) -> String {
    provider::check_decision(d, &questions(account))
        .unwrap_err()
        .to_string()
}

#[test]
fn default_config_round_trips_and_is_demo_only() {
    let config = config::default_config();
    assert_eq!(config.provider.kind, "fake");
    assert!(config.policy.review_mode);
    assert!(config.accounts.get("work").unwrap().himalaya.is_none());
    config::validate(&config).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/config.json");
    config::save(&path, &config).unwrap();
    let loaded = config::load(&path).unwrap();
    assert_eq!(loaded.accounts["work"].identity, "work@example.invalid");
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(!on_disk.contains("API_KEY="));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn config_rejects_unsafe_thresholds_and_taxonomy() {
    let mut c = config::default_config();
    c.policy.action_no_max = 0.9;
    assert!(config::validate(&c).is_err());
    c.policy.action_no_max = 0.2;
    c.accounts.get_mut("work").unwrap().categories[0].id = "other".into();
    assert!(config::validate(&c).is_err());
    c.accounts.get_mut("work").unwrap().categories[0].id = "correspondence".into();
    c.provider.kind = "openrouter".into();
    c.provider.endpoint = "http://localhost".into();
    assert!(config::validate(&c).is_err());
}

#[test]
fn mixed_mime_preserves_recipients_and_ignores_attachment() {
    let raw = b"From: =?UTF-8?Q?Alice_M=C3=BCller?= <alice@example.org>\r\nTo: Bob <bob@example.org>, jane@example.org\r\nCc: team@example.org\r\nSubject: Important\r\nDate: Wed, 23 Sep 2026 09:00:00 +0200\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=abc\r\n\r\n--abc\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nPlease respond.\r\n--abc\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=payload.bin\r\nContent-Transfer-Encoding: base64\r\n\r\nc2VjcmV0LWF0dGFjaG1lbnQ=\r\n--abc--\r\n";
    let m = normalize::rfc822(raw, 10_000).unwrap();
    assert_eq!(m.raw_sha256, format!("{:x}", Sha256::digest(raw)));
    assert_eq!(m.from[0].email, "alice@example.org");
    assert_eq!(m.to.len(), 2);
    assert_eq!(m.cc[0].email, "team@example.org");
    assert!(m.body.contains("Please respond."));
    assert!(!m.body.contains("secret-attachment"));
    assert!(!m.incomplete);
}

#[test]
fn normalization_marks_missing_and_truncated_bodies() {
    let raw = br#"{"from":"A <a@example.org>","subject":"Hi","body":"abcdefghijklmnop","raw_sha256":"forged"}"#;
    let m = normalize::json(raw, 5).unwrap();
    assert_eq!(m.body, "abcde");
    assert!(m.incomplete);
    assert_ne!(m.raw_sha256, "forged");
    assert_eq!(m.from[0].name.as_deref(), Some("A"));
    let m = normalize::json(br#"{"subject":"No body"}"#, 100).unwrap();
    assert!(m.incomplete);
    assert!(normalize::json(b"{broken", 100).is_err());
}

#[test]
fn html_only_mail_is_readable_without_fetching_remote_content() {
    let raw = b"From: sender@example.org\r\nTo: work@example.invalid\r\nSubject: HTML\r\nMIME-Version: 1.0\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<html><body><p>Review the proposal today.</p><img src=\"https://example.org/pixel\"></body></html>";
    let m = normalize::rfc822(raw, 1000).unwrap();
    assert!(m.body.contains("Review the proposal today"));
    assert!(!m.incomplete);
}

/// An edit that breaks one contract rule.
type Change = fn(&mut Decision);

#[test]
fn the_contract_check_keeps_todays_rules_and_texts() {
    let c = config::default_config();
    let a = &c.accounts["work"];
    assert!(
        provider::check_decision(&decision("other", "high", 0.99, 0.99), &questions(a)).is_ok()
    );
    // Missing answers are policy's business, not a contract failure.
    let empty = Decision {
        category: None,
        urgency: None,
        action_required: None,
        ..decision("other", "high", 0.99, 0.99)
    };
    assert!(provider::check_decision(&empty, &questions(a)).is_ok());

    let cases: Vec<(Change, &str)> = vec![
        (
            |d| d.model = "  ".into(),
            "Decisions response missing model",
        ),
        (
            |d| d.category.as_mut().unwrap().choice = "unconfigured".into(),
            "category choice \"unconfigured\" is not configured",
        ),
        (
            |d| d.urgency.as_mut().unwrap().choice = "urgent".into(),
            "urgency choice \"urgent\" is not configured",
        ),
        (
            |d| d.category.as_mut().unwrap().confidence = 1.5,
            "category confidence must be between 0 and 1",
        ),
        (
            |d| d.urgency.as_mut().unwrap().confidence = f64::NAN,
            "urgency confidence must be between 0 and 1",
        ),
        (
            |d| d.category.as_mut().unwrap().probabilities.clear(),
            "category probabilities cannot be empty",
        ),
        (
            |d| {
                d.category
                    .as_mut()
                    .unwrap()
                    .probabilities
                    .remove("promotions");
            },
            "category probabilities must include every configured choice",
        ),
        (
            |d| {
                let p = &mut d.category.as_mut().unwrap().probabilities;
                let v = p.remove("promotions").unwrap();
                p.insert("spam".into(), v);
            },
            "category probability class \"spam\" is not configured",
        ),
        (
            |d| {
                let p = &mut d.urgency.as_mut().unwrap().probabilities;
                p.insert("low".into(), -0.05);
                p.insert("medium".into(), 0.15);
            },
            "urgency probability low must be between 0 and 1",
        ),
        (
            |d| {
                let p = &mut d.category.as_mut().unwrap().probabilities;
                p.insert("other".into(), 0.2);
                p.insert("correspondence".into(), 0.72);
            },
            "category choice does not match highest probability",
        ),
        (
            |d| {
                d.category
                    .as_mut()
                    .unwrap()
                    .probabilities
                    .insert("other".into(), 0.85);
            },
            "category probabilities must total approximately 1",
        ),
        (
            |d| d.action_required = Some(1.5),
            "action_required noul must be between 0 and 1",
        ),
    ];
    for (change, expected) in cases {
        let mut d = decision("other", "high", 0.99, 0.99);
        change(&mut d);
        assert_eq!(contract_error(&d, a), expected);
    }
    // A choice may trail the highest probability by 0.01, and the total
    // may miss 1 by up to 0.031 (two-decimal rounding).
    let mut d = decision("other", "high", 0.99, 0.99);
    let p = &mut d.category.as_mut().unwrap().probabilities;
    p.insert("other".into(), 0.45);
    p.insert("correspondence".into(), 0.46);
    p.insert("transactions".into(), 0.06);
    p.insert("updates".into(), 0.0);
    p.insert("newsletters".into(), 0.0);
    p.insert("promotions".into(), 0.0);
    assert!(provider::check_decision(&d, &questions(a)).is_ok());
}

#[test]
fn missing_answers_become_reasons() {
    let c = config::default_config();
    let a = &c.accounts["work"];
    let empty = Decision {
        category: None,
        urgency: None,
        action_required: None,
        ..decision("other", "high", 0.99, 0.99)
    };
    let result = policy::apply(checked(empty, a), a, &c.policy, false);
    assert_eq!(
        result.reasons,
        [
            "category_missing",
            "urgency_missing",
            "action_required_missing",
            "review_mode"
        ]
    );
    assert_eq!(result.state, "uncertain");
    assert_eq!(result.model, "typesafe/jev-1.13-20260917");
    assert_eq!(result.raw, json!({"fixture": true}));
}

#[test]
fn uncertainty_and_review_mode_remain_attention_items() {
    let mut c = config::default_config();
    let a = &c.accounts["work"];
    let result = policy::apply(
        checked(decision("correspondence", "low", 0.5, 0.5), a),
        a,
        &c.policy,
        true,
    );
    assert_eq!(result.state, "uncertain");
    assert_eq!(
        result.reasons,
        [
            "category_low_confidence",
            "urgency_low_confidence",
            "action_required_uncertain",
            "input_incomplete",
            "review_mode"
        ]
    );
    assert_eq!(result.action_required, None);
    assert_eq!(result.urgency, None);
    assert!(policy::attention(&result, "open").contains(&"uncertain".into()));
    assert!(policy::attention(&result, "done").is_empty());
    c.policy.review_mode = false;
    let a = &c.accounts["work"];
    let strong = policy::apply(
        checked(decision("correspondence", "high", 0.99, 0.99), a),
        a,
        &c.policy,
        false,
    );
    assert_eq!(strong.state, "ready");
    assert!(strong.reasons.is_empty());
    assert!(policy::attention(&strong, "open").contains(&"high_urgency".into()));
    let weak = policy::apply(
        checked(decision("correspondence", "low", 0.01, 0.99), a),
        a,
        &c.policy,
        true,
    );
    assert_eq!(weak.state, "uncertain");
    assert_eq!(weak.reasons, ["input_incomplete"]);
    assert!(policy::attention(&weak, "open").contains(&"uncertain".into()));

    let medium = policy::apply(
        checked(decision("updates", "medium", 0.01, 0.99), a),
        a,
        &c.policy,
        false,
    );
    assert_eq!(medium.state, "ready");
    assert_eq!(medium.action_required, Some(false));
    assert!(policy::attention(&medium, "open").contains(&"medium_urgency".into()));

    let quiet = policy::apply(
        checked(decision("newsletters", "low", 0.01, 0.99), a),
        a,
        &c.policy,
        false,
    );
    assert_eq!(quiet.state, "ready");
    assert!(policy::attention(&quiet, "open").is_empty());
}
