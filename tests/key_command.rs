#![cfg(unix)]
//! The key command runner, key resolution, the per-service cache and the
//! doctor's key report. Environment variable names are unique per test
//! because tests in this file run in parallel.
use mailtriage::{
    config,
    domain::ProviderConfig,
    provider::{self, DECISIONS_ENDPOINT},
    secrets::{self, KeyCache},
    service::Service,
};
use std::{
    fs,
    time::{Duration, Instant},
};

fn sh(script: &str) -> Vec<String> {
    vec!["/bin/sh".into(), "-c".into(), script.into()]
}

fn run_err(script: &str) -> String {
    secrets::run_key_command(&sh(script))
        .unwrap_err()
        .to_string()
}

fn openrouter() -> ProviderConfig {
    let mut provider = config::default_config().provider;
    provider.kind = "openrouter".into();
    provider.model = "typesafe/jev-1.13".into();
    provider.endpoint = DECISIONS_ENDPOINT.into();
    provider.api_key_env = "OPENROUTER_API_KEY".into();
    provider
}

#[test]
fn the_key_is_the_trimmed_first_line() {
    assert_eq!(
        secrets::run_key_command(&sh("printf '  sk-or-1 \\r\\nsecond line\\n'")).unwrap(),
        "sk-or-1"
    );
    assert_eq!(
        secrets::run_key_command(&sh("echo sk-or-2; echo noise >&2")).unwrap(),
        "sk-or-2"
    );
}

#[test]
fn failures_report_fixed_messages_without_output() {
    assert_eq!(
        run_err("echo sk-leak; echo err-leak >&2; exit 4"),
        "API key command failed (exit 4)"
    );
    assert_eq!(
        run_err("printf '\\n  \\nsk-second\\n'"),
        "API key command printed no key"
    );
    assert_eq!(run_err("true"), "API key command printed no key");
    assert_eq!(
        run_err("head -c 5000 /dev/zero | tr '\\0' a"),
        "API key command printed no key"
    );
    assert_eq!(
        secrets::run_key_command(&["/nonexistent/key-tool".to_owned()])
            .unwrap_err()
            .to_string(),
        "API key command could not start"
    );
    assert_eq!(
        secrets::run_key_command(&[]).unwrap_err().to_string(),
        "API key command could not start"
    );
}

#[test]
fn a_slow_command_times_out() {
    let start = Instant::now();
    assert_eq!(run_err("sleep 30"), "API key command timed out");
    assert!(start.elapsed() < Duration::from_secs(15));
}

#[test]
fn the_command_gets_no_stdin() {
    assert_eq!(
        secrets::run_key_command(&sh(
            "if read -r line; then echo had-input; else echo sk-closed; fi"
        ))
        .unwrap(),
        "sk-closed"
    );
}

#[test]
fn the_command_beats_the_environment() {
    std::env::set_var("MT_KEY_PRECEDENCE_TEST", "sk-from-env");
    let mut provider = openrouter();
    provider.api_key_env = "MT_KEY_PRECEDENCE_TEST".into();
    assert_eq!(secrets::resolve_key(&provider).unwrap(), "sk-from-env");
    assert_eq!(secrets::key_source(&provider), "env");
    provider.api_key_command = Some(sh("echo sk-from-command"));
    assert_eq!(secrets::resolve_key(&provider).unwrap(), "sk-from-command");
    assert_eq!(secrets::key_source(&provider), "command");
    provider.api_key_command = Some(sh("exit 2"));
    assert_eq!(
        secrets::resolve_key(&provider).unwrap_err().to_string(),
        "API key command failed (exit 2)"
    );
}

#[test]
fn validation_needs_a_key_source() {
    let mut provider = openrouter();
    assert!(provider::validate_configuration(&provider).is_ok());
    provider.api_key_env = String::new();
    assert!(provider::validate_configuration(&provider).is_err());
    provider.api_key_command = Some(vec![]);
    assert!(provider::validate_configuration(&provider).is_err());
    provider.api_key_command = Some(vec![String::new(), "x".into()]);
    assert!(provider::validate_configuration(&provider).is_err());
    provider.api_key_command = Some(sh("echo k"));
    assert!(provider::validate_configuration(&provider).is_ok());
    provider.api_key_env = "lower_case".into();
    assert!(provider::validate_configuration(&provider).is_err());
}

#[test]
fn the_cache_runs_the_command_once() {
    let dir = tempfile::tempdir().unwrap();
    let count = dir.path().join("count");
    let mut provider = openrouter();
    provider.api_key_command = Some(sh(&format!(
        "echo run >> '{}'; echo sk-cached",
        count.display()
    )));
    let cache = KeyCache::default();
    assert_eq!(cache.get(&provider).unwrap(), "sk-cached");
    assert_eq!(cache.get(&provider).unwrap(), "sk-cached");
    assert_eq!(fs::read_to_string(&count).unwrap().lines().count(), 1);
}

#[test]
fn doctor_reports_the_key_source_without_the_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mailtriage.json");
    let mut c = config::default_config();
    c.provider = openrouter();
    c.provider.api_key_command = Some(sh("echo sk-doctor-secret"));
    config::save(&path, &c).unwrap();
    let report = Service::open(&path).unwrap().doctor("work").unwrap();
    assert_eq!(report["provider"]["key_source"], "command");
    assert_eq!(report["provider"]["key_present"], true);
    assert!(report["provider"].get("key_error").is_none());
    assert!(!report.to_string().contains("sk-doctor-secret"));

    c.provider.api_key_command = Some(sh("echo sk-doctor-secret; exit 3"));
    config::save(&path, &c).unwrap();
    let report = Service::open(&path).unwrap().doctor("work").unwrap();
    assert_eq!(report["provider"]["key_present"], false);
    assert_eq!(
        report["provider"]["key_error"],
        "API key command failed (exit 3)"
    );
    assert_eq!(report["ready"], false);
    assert!(!report.to_string().contains("sk-doctor-secret"));

    c.provider = config::default_config().provider;
    config::save(&path, &c).unwrap();
    let report = Service::open(&path).unwrap().doctor("work").unwrap();
    assert_eq!(report["provider"]["key_source"], serde_json::Value::Null);
    assert_eq!(report["provider"]["key_present"], true);
}

#[test]
fn a_config_without_a_key_command_serializes_as_before() {
    let value = serde_json::to_value(config::default_config().provider).unwrap();
    assert!(value.get("api_key_command").is_none());
    let loaded: ProviderConfig = serde_json::from_value(serde_json::json!({
        "kind": "openrouter",
        "model": "typesafe/jev-1.13",
        "endpoint": DECISIONS_ENDPOINT,
        "api_key_command": ["/bin/echo", "k"],
        "timeout_seconds": 30
    }))
    .unwrap();
    assert_eq!(loaded.api_key_env, "");
    assert!(provider::validate_configuration(&loaded).is_ok());
}
