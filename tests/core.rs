use mailtriage::{config, normalize, policy};
use serde_json::json;
use sha2::{Digest, Sha256};

fn response(category: &str, urgency: &str, action: f64, confidence: f64) -> serde_json::Value {
    let mut category_probabilities = json!({"correspondence":0.02,"transactions":0.02,"updates":0.02,"newsletters":0.02,"promotions":0.02,"other":0.9});
    if category_probabilities.get(category).is_some() {
        for value in category_probabilities.as_object_mut().unwrap().values_mut() {
            *value = json!(0.02);
        }
        category_probabilities[category] = json!(0.9);
    }
    let mut urgency_probabilities = json!({"low":0.05,"medium":0.05,"high":0.9});
    if urgency_probabilities.get(urgency).is_some() {
        for value in urgency_probabilities.as_object_mut().unwrap().values_mut() {
            *value = json!(0.05);
        }
        urgency_probabilities[urgency] = json!(0.9);
    }
    json!({
        "model": "typesafe/jev-1.13-20260917",
        "answers": {
            "category": {"type":"choice", "choice":category, "confidence":confidence,
                "probabilities":category_probabilities},
            "urgency": {"type":"choice", "choice":urgency, "confidence":confidence,
                "probabilities":urgency_probabilities},
            "action_required": {"type":"noul", "noul":action}
        }
    })
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

#[test]
fn response_rejects_unknown_choices_and_invalid_probabilities() {
    let c = config::default_config();
    let a = &c.accounts["work"];
    let mut r = response("unconfigured", "high", 0.99, 0.99);
    assert!(policy::decode_response(&r, a, &c.policy, false).is_err());
    r = response("correspondence", "high", 1.5, 0.99);
    assert!(policy::decode_response(&r, a, &c.policy, false).is_err());
    r = response("correspondence", "high", 0.99, 0.99);
    r["answers"]["category"]["probabilities"]["correspondence"] = json!(0.2);
    assert!(policy::decode_response(&r, a, &c.policy, false).is_err());
    r = response("correspondence", "high", 0.99, 0.99);
    r.as_object_mut().unwrap().remove("model");
    assert!(policy::decode_response(&r, a, &c.policy, false).is_err());
}

#[test]
fn uncertainty_and_review_mode_remain_attention_items() {
    let mut c = config::default_config();
    let a = &c.accounts["work"];
    let result = policy::decode_response(
        &response("correspondence", "low", 0.5, 0.5),
        a,
        &c.policy,
        true,
    )
    .unwrap();
    assert_eq!(result.state, "uncertain");
    assert_eq!(result.action_required, None);
    assert_eq!(result.urgency, None);
    assert!(policy::attention(&result, "open").contains(&"uncertain".into()));
    assert!(policy::attention(&result, "done").is_empty());
    c.policy.review_mode = false;
    let strong = policy::decode_response(
        &response("correspondence", "high", 0.99, 0.99),
        &c.accounts["work"],
        &c.policy,
        false,
    )
    .unwrap();
    assert_eq!(strong.state, "ready");
    assert!(policy::attention(&strong, "open").contains(&"high_urgency".into()));
    let weak = policy::decode_response(
        &response("correspondence", "low", 0.01, 0.99),
        &c.accounts["work"],
        &c.policy,
        true,
    )
    .unwrap();
    assert_eq!(weak.state, "uncertain");
    assert!(policy::attention(&weak, "open").contains(&"uncertain".into()));

    let medium = policy::decode_response(
        &response("updates", "medium", 0.01, 0.99),
        &c.accounts["work"],
        &c.policy,
        false,
    )
    .unwrap();
    assert_eq!(medium.state, "ready");
    assert_eq!(medium.action_required, Some(false));
    assert!(policy::attention(&medium, "open").contains(&"medium_urgency".into()));

    let quiet = policy::decode_response(
        &response("newsletters", "low", 0.01, 0.99),
        &c.accounts["work"],
        &c.policy,
        false,
    )
    .unwrap();
    assert_eq!(quiet.state, "ready");
    assert!(policy::attention(&quiet, "open").is_empty());
}
