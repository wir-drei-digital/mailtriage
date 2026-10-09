# Guided Setup, Key Command and Background Service Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One command (`mailtriage setup`) takes a new user from nothing to a `doctor`-ready config without the OpenRouter key ever touching a file, agents get the same through flags, and `watch` can run as a launchd agent or systemd user unit.

**Architecture:** The key comes from `provider.api_key_command` (a bounded child process, `src/secrets.rs` on top of `src/process.rs`) or an environment variable. Commands find the config by a fixed resolution order (`src/config.rs`). `src/setup.rs` runs ten steps, each answered by a flag or a numbered prompt (`src/prompt.rs`, standard library only); it reuses Himalaya's own wizard and the existing `MailEngine` to list folders. `src/system_service.rs` writes marked plist/unit files from pure functions and drives `launchctl`/`systemctl`; each sync pass records a heartbeat row (schema v5) that `service status` reports.

**Tech Stack:** Rust 2021 (stable, 1.92 locally), clap 4, serde/serde_json, toml, rusqlite. Himalaya v2.1.0 CLI as a subprocess. No new crate dependencies.

**Spec:** `design/specs/2026-10-05-guided-setup-design.md`. Read it before every task. Where this plan and the spec disagree, the spec wins, except for the rulings listed under "Decisions this plan adds".

## Verified Himalaya v2.1.0 behaviour

Checked against `himalaya v2.1.0 +smtp +imap +jmap …` (Homebrew, macOS) on 2026-10-05. These replace the spec's "confirmed in the plan's first task" notes; Task 6 records them in the spec.

- `himalaya --config T --json account list` prints `{"accounts":[{"name":"home","default":false,"backends":["imap"]},{"name":"work","default":true,"backends":["imap"]}]}`. Accounts are sorted by name. `backends` lists the configured backends (`imap`, …). An account with no backend block has `"backends":[]`.
- `himalaya --config T --account A --backend imap --json account check` prints `{"account":"A","backends":[{"backend":"imap","ok":false,"error":"connect 127.0.0.1:1: Connection refused (os error 61)"}]}` and **exits 0 even when the check fails**. Setup must read `ok`. An unknown account exits 1 with `{"error":"Get account `nope` error",…}`.
- With no `--config` and no `HIMALAYA_CONFIG`, Himalaya uses the first existing file of: `<platform config dir>/himalaya/config.toml`, `~/.config/himalaya/config.toml`, `~/.himalayarc`. On macOS the platform config dir is `~/Library/Application Support` and `XDG_CONFIG_HOME` is ignored (verified). On Linux it is `$XDG_CONFIG_HOME` when that is an absolute path, else `~/.config` (the `dirs` crate rule). With none of them present it fails: `No configuration found. Run bare `himalaya` to launch the wizard and generate one.`
- `HIMALAYA_CONFIG` may hold several paths separated by `:`.
- `himalaya configure` (alias `wizard`) creates an account interactively. The account email is only in the TOML (`[accounts.<name>] email = "…"`), not in `account list`.
- The v2.1.0 TOML shape is `imap.server = "imaps://host"`, `imap.sasl.plain.username`, `imap.sasl.plain.password.raw|cmd` (see `tests/e2e/himalaya.toml.in`).

## Decisions this plan adds

Rulings on points the spec leaves open. Each records what it costs if wrong.

1. **Prompts and `himalaya configure`.** "With a terminal" means "prompts are on" (stdin is a TTY or `--interactive`). This lets tests drive `configure` through a fake. Cost if wrong: one condition in `himalaya_step`.
2. **`setup` and `MAILTRIAGE_CONFIG`.** Setup honours `--config`, then `MAILTRIAGE_CONFIG`, then the home path. It never picks `./mailtriage.json`. Cost: one function.
3. **`api_key_env` stays stable.** For OpenRouter, setup always writes `api_key_env` (the existing value, else `OPENROUTER_API_KEY`) and adds `api_key_command` for command-backed stores. Switching stores therefore never changes the generation hash. `api_key_env` gains `#[serde(default)]` so a hand-written config with only a command loads. Cost: none to existing configs.
4. **Key cache scope.** The resolved key, or its error, is cached per `Service` instance, not per process. `watch` opens a new `Service` each pass, so a rotated key is used on the next pass, as the spec intends. Cost: one field.
5. **A signal-killed key command** reports `API key command failed (exit signal)`. Output over 4096 bytes reports `API key command printed no key`. Cost: two strings.
6. **Updating an existing account** keeps its categories, taxonomy revision, `flag` and `max_actions_per_pass`. A `live` account stays `live` unless `--filing` is given. Defaults for identity, time zone, brief and folders come from the existing account. Cost: one field each.
7. **Keeping the classifier.** When the config exists and no classifier flag is given, setup asks "Keep the current classifier?" (default yes). Without prompts it keeps the classifier. The classifier flags are `--provider`, `--model`, `--key-store`, `--key-command`, `--key-env` and `--key-stored`. `--key-command` implies `--key-store command` and `--key-env` implies `--key-store env`; a conflicting combination exits 2.
8. **Watched folders without prompts** must exist in the server's folder list (exit 2 otherwise). Folders with `\Noselect` are never offered.
9. **Doctor failure inside setup** (for example, a changed account binding) is reported as a not-ready `state` item. Setup still exits 0, as the spec's exit-code table says.
10. **Service details.**
    - The plist and unit carry `PATH` from the installing shell, so `pass`/`gpg` work under launchd.
    - Linux install runs `daemon-reload`, `enable`, `restart` instead of `enable --now`, so a changed unit takes effect.
    - Install refuses an account whose name is not 1–64 ASCII letters, digits, `-` or `_`.
    - `uninstall` works for an account that is no longer in the config.
    - `status` and `install` require the account to be in the config.
11. **Heartbeat.**
    - It is written for every pass that acquired the account lock and names a configured account.
    - Exit code: 0 ok, 4 partial, or the error's exit code (ServiceError code, 5 for an engine configuration change, else 3). `partial` is false for failed passes.
    - Mode: the pass mode (`off` without an engine).
12. **A helper module.** `src/process.rs` holds the bounded runner and `find_on_path`. `terminate` and `read_bounded` move there from `src/engine/himalaya.rs` so both share one copy.
13. **Account names** for setup and service: 1–64 ASCII letters, digits, `-`, `_` (`config::valid_account_name`).
14. **Setup's Himalaya engine values** for new accounts: `timeout_seconds` 60 and `max_output_bytes` 50000000 (the README's example). Updates keep the existing values.

## Global Constraints

- Work directly on `main` (user instruction). Commit at the end of every task. Every commit message ends with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- After every task all three pass: `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`. CI runs them on Linux (amd64, arm64) and macOS, so tests must not depend on the host platform unless they are `#[cfg]`-gated.
- No new dependencies; `Cargo.toml` and `Cargo.lock` stay unchanged.
- The OpenRouter key never appears in a config or service file, log, JSON output, stderr or error message. Setup never reads, prints or writes the key; a store tool prompts for it.
- Key command runner: argument list run directly (no shell), stdin closed, stderr discarded, 10-second timeout, at most 4096 bytes of stdout, must exit 0, key = first stdout line trimmed. Its errors are exactly: `API key command failed (exit N)`, `API key command timed out`, `API key command printed no key`, `API key command could not start`.
- If `api_key_command` is set it is the only key source.
- Config resolution for every command except `init` and `setup`: `--config PATH`; else `MAILTRIAGE_CONFIG`; else `./mailtriage.json` if it exists; else `~/.config/mailtriage/mailtriage.json`. `init` without `--config` writes `./mailtriage.json`. `setup` without `--config` writes `MAILTRIAGE_CONFIG` or `~/.config/mailtriage/mailtriage.json`, with `state_dir` `state`.
- Setup exit codes:
  - 0: completed (doctor items may be not ready).
  - 2: invalid input, a missing flag without prompts, an invalid account name, or an unsupported platform.
  - 3: Himalaya missing or not v2.1.0 `+imap`, `account check` failed, a key tool failed, or `launchctl`/`systemctl` failed.
  - 5: the config exists without `--update`, or there is an unmarked service file.
- Every setup error names its step and the flag or command that fixes it.
- Setup makes no IMAP changes, never selects `live`, and runs `himalaya configure` only with prompts on.
- Every subprocess setup and the service code start runs through `process::run_bounded`, except two that run attached to the terminal: `himalaya configure` and a key store's store step.
- Service files carry a marker (`<key>XMailtriageManaged</key>` in the plist, `# managed by mailtriage` as the unit's first line), contain only absolute paths and no secrets, and use the label prefix `digital.wirdrei.mailtriage`.
- JSON responses keep `schema_version: 1` and the error envelope `{"schema_version":1,"error":{"code","message"}}`.
- The classification generation hash must not change: exactly `6dfd7bf30e6bf1c0b27ee97af991ecf21dc5ac0400044c2f3adbda7f79d37514` for `config::default_config()`'s `work` account, with or without an `api_key_command`. The account binding identity golden value `c4a294832a82e8e274fb8bcdf9c2c0daa895f811efe13776ab60da3c7fd50aaa` must not change.
- Existing tests keep passing unchanged, with two permitted edits:
  - `tests/filing_store.rs::newer_schema_is_rejected` sets `user_version` 6 instead of 5 (Task 5);
  - `tests/common/mod.rs` gains helpers (Task 5).
- Error text never contains mail content, credentials, or the output of Himalaya or a key tool.

## Review Focus

Inputs the spec implies but no task's own tests would otherwise exercise, most likely to bite first. Each has a pinned test in the task named.

1. **Paths with spaces or special characters** (macOS `~/Library/Application Support/…`, a home or config directory with a space, `&`, `%` or `$`). They must stay one argument everywhere: in the stored engine config, in the plist (XML-escaped), in the unit's `ExecStart` (quoted, `%%`/`$$`), and in the commands shown in messages. Tests:
   - Task 3: `a_himalaya_config_path_with_spaces_is_kept_whole`, and `setup_writes_the_home_config_unless_told_otherwise` (with `--config ".../Some Dir/flag.json"`);
   - Task 5: fixed-output plist and unit tests with `/opt/mail triage` and `/Users/a & b`, and `systemd_args_escape_specifiers`.
2. **Users upgrading with `./mailtriage.json`.** Every command still finds it, and `setup` never modifies it. Tests:
   - Task 2: `commands_find_the_config_in_the_documented_order`;
   - Task 3: `setup_writes_the_home_config_unless_told_otherwise`.
3. **Real key-tool output shapes**: CRLF endings, trailing lines (`pass` metadata), surrounding spaces, a blank first line, output written to stderr. Test: Task 1 `the_key_is_the_trimmed_first_line` and `failures_report_fixed_messages_without_output`.
4. **Re-running setup or install.**
   - Setup: `--update` keeps other accounts, the provider, the account's edited categories and its `live` mode.
   - Install: a second run rewrites the same file and reloads.
   Tests:
   - Task 3: `an_existing_config_needs_update_and_keeps_other_accounts`;
   - Task 5: `launchd_install_reload_status_and_uninstall` and `systemd_install_status_and_uninstall`.
5. **Stdin shared with a child or ending early.**
   - A store tool must read the user's next line, so the prompter never reads ahead.
   - Input that ends mid-flow aborts with exit 2 and writes nothing.
   Tests:
   - Task 2: `answers_are_read_without_reading_ahead`;
   - Task 3: `end_of_input_aborts_without_writing`;
   - Task 4: `the_store_tool_asks_for_the_key_on_the_shared_stdin`.

## File Structure

| Path | Task | Responsibility |
| --- | --- | --- |
| `src/process.rs` (new) | 1 | `run_bounded` (no shell, stdin closed, stderr discarded, deadline, stdout cap), `find_on_path`, and `terminate`/`read_bounded` moved from the Himalaya engine |
| `src/secrets.rs` (new) | 1, 3, 4 | Key command runner, key resolution, `KeyCache`, `key_source`; the `KeyStore` options and their read/store commands |
| `src/domain.rs` | 1 | `ProviderConfig.api_key_command` |
| `src/provider.rs` | 1 | Validation for command/env, `classify_with_key`, public `DECISIONS_ENDPOINT` and `valid_env_name` |
| `src/service.rs` | 1, 2, 5 | Key cache, doctor `key_source`/`key_error`, generation hash without the command, open message, heartbeat on each pass, `exit_code` |
| `src/engine/himalaya.rs` | 1, 3 | Uses `process::{terminate, read_bounded}`; `check_version_output` |
| `src/config.rs` | 2, 3 | `CONFIG_FILE`, `home_config_path`, `resolve_path`, `setup_path`, `valid_account_name`, `unsafe_source_mailbox` |
| `src/prompt.rs` (new) | 2 | `Prompter` (ask, confirm, numbered menus, re-ask, end of input = exit 2) and unbuffered stdin |
| `src/setup.rs` (new) | 3, 4, 5 | The ten setup steps and the flags-to-answers model |
| `src/store.rs` | 5 | Schema v5 `pass_heartbeats`, `record_heartbeat`, `heartbeat` |
| `src/system_service.rs` (new) | 5 | Plist/unit text (pure), install/uninstall/status via `launchctl`/`systemctl`, status parsing |
| `src/cli.rs` | 2, 3, 4, 5 | Optional `--config` with resolution; `setup` and `service` subcommands |
| `src/lib.rs` | 1, 2, 3, 5 | New modules |
| `tests/key_command.rs` (new) | 1 | Runner, precedence, validation, cache, doctor |
| `tests/config_path.rs` (new) | 2 | Resolution order through the binary |
| `tests/setup.rs` (new) | 3, 4, 5 | Setup through the binary with a fake Himalaya and fake key tools on `PATH` |
| `tests/system_service.rs` (new) | 5 | Pure text, both managers against fake tools, CLI `service` commands |
| `tests/heartbeat.rs` (new) | 5 | Heartbeat per pass |
| `README.md`, `docs/guide.md` (new), `docs/agents/index.md`, `docs/development/service-api.md`, `docs/development/verification.md`, the spec | 6 | Short README, full guide, agent setup, references |

---

### Task 1: Key command

**Files:**
- Create: `src/process.rs`, `src/secrets.rs`, `tests/key_command.rs`
- Modify:
  - `src/lib.rs`
  - `src/domain.rs:13-20` (`ProviderConfig`)
  - `src/config.rs:82-88` (`default_config` provider)
  - `src/provider.rs` (validation, `classify`, `request_decision`)
  - `src/service.rs` (`Service` struct and `open`, classify call near line 395, `doctor` near line 255, `generation` near line 1538, `mod golden`)
  - `src/engine/himalaya.rs` (remove `terminate`/`read_bounded`, import them)

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `process::run_bounded<S: AsRef<OsStr>>(program: &Path, args: &[S], timeout: Duration, max_stdout: usize) -> std::io::Result<Bounded>`.
  - `process::Bounded { pub stdout: Vec<u8>, pub ending: Ending }` with `fn success(self) -> Option<Vec<u8>>`.
  - `process::Ending { Exited(ExitStatus), TimedOut, Overflowed }`.
  - `process::find_on_path(name: &str) -> Option<PathBuf>`.
  - `pub(crate) process::terminate(&mut Child)` and `pub(crate) process::read_bounded(...)`.
  - `secrets::run_key_command(&[String]) -> anyhow::Result<String>`.
  - `secrets::resolve_key(&ProviderConfig) -> anyhow::Result<String>`.
  - `secrets::key_source(&ProviderConfig) -> &'static str` (`"command"` | `"env"`).
  - `secrets::KeyCache` (`Default`, `fn get(&self, &ProviderConfig) -> anyhow::Result<String>`).
  - `ProviderConfig.api_key_command: Option<Vec<String>>`.
  - `provider::DECISIONS_ENDPOINT` (pub).
  - `provider::valid_env_name(&str) -> bool`.
  - `provider::classify_with_key(.., key: &KeyCache)`.
  - Doctor JSON: `provider.key_source` (`"command"`, `"env"` or `null` for `fake`) and, when the key is missing, `provider.key_error`.

- [ ] **Step 1: Write the failing tests**

Create `tests/key_command.rs`:

```rust
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
```

Add to `mod golden` in `src/service.rs`:

```rust
    #[test]
    fn generation_hash_ignores_the_key_command() {
        let mut cfg = crate::config::default_config();
        cfg.provider.api_key_command = Some(vec![
            "/usr/bin/security".into(),
            "find-generic-password".into(),
        ]);
        assert_eq!(
            super::generation(&cfg, &cfg.accounts["work"]).unwrap(),
            "6dfd7bf30e6bf1c0b27ee97af991ecf21dc5ac0400044c2f3adbda7f79d37514"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test key_command`
Expected: compile errors (`secrets`, `api_key_command`, `DECISIONS_ENDPOINT` not found).

- [ ] **Step 3: Add `src/process.rs`**

Move `terminate` and `read_bounded` out of `src/engine/himalaya.rs` (lines ~751–790) into this file unchanged, made `pub(crate)`. In `himalaya.rs` replace them with `use crate::process::{read_bounded, terminate};` and drop imports that become unused.

```rust
//! Child processes with limits: an argument array (no shell), stdin closed,
//! stderr discarded, a deadline and a cap on stdout.
use std::{
    ffi::OsStr,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

/// How a bounded child ended.
#[derive(Debug)]
pub enum Ending {
    Exited(ExitStatus),
    /// Killed at the deadline.
    TimedOut,
    /// Killed after writing more than the stdout cap.
    Overflowed,
}

#[derive(Debug)]
pub struct Bounded {
    /// Everything read before the child ended or was killed.
    pub stdout: Vec<u8>,
    pub ending: Ending,
}

impl Bounded {
    /// Stdout of a child that exited 0.
    pub fn success(self) -> Option<Vec<u8>> {
        match self.ending {
            Ending::Exited(status) if status.success() => Some(self.stdout),
            _ => None,
        }
    }
}

/// Runs `program` with `args`; `Err` only when it cannot be started.
pub fn run_bounded<S: AsRef<OsStr>>(
    program: &Path,
    args: &[S],
    timeout: Duration,
    max_stdout: usize,
) -> std::io::Result<Bounded> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let overflow = Arc::new(AtomicBool::new(false));
    let collected = Arc::new(Mutex::new(Vec::new()));
    let reader = {
        let overflow = Arc::clone(&overflow);
        let collected = Arc::clone(&collected);
        thread::spawn(move || {
            let _ = read_bounded(stdout, max_stdout, &overflow, |chunk| {
                collected
                    .lock()
                    .map(|mut all| all.extend_from_slice(chunk))
                    .is_ok()
            });
        })
    };
    let deadline = Instant::now() + timeout;
    let mut status = None;
    let ending = loop {
        if overflow.load(Ordering::Relaxed) {
            terminate(&mut child);
            break Ending::Overflowed;
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(done) => status = done,
                Err(e) => {
                    terminate(&mut child);
                    return Err(e);
                }
            }
        }
        if let (Some(status), true) = (status, reader.is_finished()) {
            break if overflow.load(Ordering::Relaxed) {
                Ending::Overflowed
            } else {
                Ending::Exited(status)
            };
        }
        if Instant::now() >= deadline {
            terminate(&mut child);
            break Ending::TimedOut;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let stdout = std::mem::take(&mut *collected.lock().unwrap_or_else(|e| e.into_inner()));
    Ok(Bounded { stdout, ending })
}

/// The first executable file called `name` in `PATH`, made absolute.
/// Symlinks are kept, so a package upgrade does not break the stored path.
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
        .and_then(|found| std::path::absolute(found).ok())
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

// `terminate` and `read_bounded` follow, moved verbatim from
// src/engine/himalaya.rs and made pub(crate).
```

- [ ] **Step 4: Add `src/secrets.rs`**

```rust
//! The OpenRouter API key comes from a key command or an environment
//! variable, never from the config file. Only `resolve_key` and
//! `KeyCache::get` return the key, and only to the provider; errors are
//! fixed strings that never contain command output.
use crate::{
    domain::ProviderConfig,
    process::{self, Ending},
};
use anyhow::{anyhow, bail, Result};
use std::{cell::OnceCell, path::Path, time::Duration};

const KEY_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const KEY_COMMAND_MAX_STDOUT: usize = 4096;

/// `"command"` when `api_key_command` is set, else `"env"`.
pub fn key_source(config: &ProviderConfig) -> &'static str {
    if config.api_key_command.is_some() {
        "command"
    } else {
        "env"
    }
}

/// Runs a key command and returns its first stdout line, trimmed.
pub fn run_key_command(command: &[String]) -> Result<String> {
    let could_not_start = || anyhow!("API key command could not start");
    let (program, args) = command
        .split_first()
        .filter(|(program, _)| !program.is_empty())
        .ok_or_else(could_not_start)?;
    let run = process::run_bounded(
        Path::new(program),
        args,
        KEY_COMMAND_TIMEOUT,
        KEY_COMMAND_MAX_STDOUT,
    )
    .map_err(|_| could_not_start())?;
    match run.ending {
        Ending::TimedOut => bail!("API key command timed out"),
        Ending::Overflowed => bail!("API key command printed no key"),
        Ending::Exited(status) if !status.success() => bail!(
            "API key command failed (exit {})",
            status
                .code()
                .map_or_else(|| "signal".to_owned(), |code| code.to_string())
        ),
        Ending::Exited(_) => {}
    }
    let key = std::str::from_utf8(&run.stdout)
        .ok()
        .and_then(|text| text.lines().next())
        .map(str::trim)
        .unwrap_or_default();
    if key.is_empty() {
        bail!("API key command printed no key");
    }
    Ok(key.to_owned())
}

/// The key: from `api_key_command` when set (the only source then), else
/// from the `api_key_env` variable.
pub fn resolve_key(config: &ProviderConfig) -> Result<String> {
    if let Some(command) = &config.api_key_command {
        return run_key_command(command);
    }
    let key = std::env::var(&config.api_key_env)
        .map_err(|_| anyhow!("OpenRouter API key environment variable is missing"))?;
    if key.trim().is_empty() {
        bail!("OpenRouter API key environment variable is empty");
    }
    Ok(key)
}

/// The key, or the reason it is missing, resolved at most once. A `Service`
/// holds one, so each `watch` pass resolves the key again. No `Debug`: it
/// holds the key.
#[derive(Default)]
pub struct KeyCache(OnceCell<std::result::Result<String, String>>);

impl KeyCache {
    pub fn get(&self, config: &ProviderConfig) -> Result<String> {
        self.0
            .get_or_init(|| resolve_key(config).map_err(|e| e.to_string()))
            .clone()
            .map_err(|message| anyhow!(message))
    }
}
```

- [ ] **Step 5: Config, provider and service changes**

`src/domain.rs`, `ProviderConfig`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub kind: String,
    pub model: String,
    pub endpoint: String,
    /// Argument list that prints the API key; when set it is the only key
    /// source. Serialized only when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_command: Option<Vec<String>>,
    #[serde(default)]
    pub api_key_env: String,
    pub timeout_seconds: u64,
}
```

`src/config.rs` `default_config`: add `api_key_command: None,` to the provider literal.

`src/provider.rs`:
- Make `DECISIONS_ENDPOINT` `pub`.
- In `validate_configuration`'s `"openrouter"` arm, replace the `api_key_env` check with:

```rust
            match &config.api_key_command {
                Some(command) => {
                    if command.first().is_none_or(|program| program.is_empty()) {
                        bail!("provider.api_key_command must name a program");
                    }
                }
                None if config.api_key_env.is_empty() => {
                    bail!("OpenRouter provider needs api_key_command or api_key_env")
                }
                None => {}
            }
            if !config.api_key_env.is_empty() && !valid_env_name(&config.api_key_env) {
                bail!("provider API key environment variable name is invalid");
            }
            Ok(())
```

```rust
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

/// `classify`, resolving the OpenRouter key through `key`.
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
    let raw = match config.kind.as_str() {
        "fake" => fake_decision(config, account, message),
        "openrouter" => request_decision(config, account, message, &key.get(config)?)?,
        _ => unreachable!(),
    };
    crate::policy::decode_response(&raw, account, policy, message.incomplete)
}
```

`provider.rs` imports `crate::secrets::KeyCache`. `request_decision` gains a `key: &str` parameter. Its env lookup (old lines 73–77) is deleted, because `resolve_key` keeps those two messages. `.bearer_auth(key)` stays.

`src/service.rs`:
- Import `crate::secrets::{self, KeyCache}`.
- Add the field `key: KeyCache` to `Service`, initialised as `key: KeyCache::default()` in `open`.
- In the classify call (~line 395), call `provider::classify_with_key(&self.config.provider, account, &message, &self.config.policy, &self.key)`.
- `doctor` replaces its `key_present` computation and the `provider` object with:

```rust
        let provider_cfg = &self.config.provider;
        let (key_source, key_error) = if provider_cfg.kind == "fake" {
            (Value::Null, None)
        } else {
            (
                json!(secrets::key_source(provider_cfg)),
                self.key.get(provider_cfg).err().map(|e| e.to_string()),
            )
        };
        let key_present = key_error.is_none();
```

The `"provider"` value becomes `{"kind","model","configuration_valid","key_source":key_source,"key_present":key_present}`. After building `out`, add: `if let Some(e) = key_error { out["provider"]["key_error"] = json!(e); }`.

`generation`:

```rust
fn generation(config: &AppConfig, account: &AccountConfig) -> Result<String> {
    // The key command is how the key is fetched, not what classifies mail.
    let mut provider = serde_json::to_value(&config.provider)?;
    if let Some(fields) = provider.as_object_mut() {
        fields.remove("api_key_command");
    }
    Ok(hash(&serde_json::to_vec(
        &json!({"rubric_version":1,"normalizer_version":1,"taxonomy_revision":account.taxonomy_revision,"categories":category_semantics(&account.categories),"brief":account.brief,"timezone":account.timezone,"provider":provider,"policy":config.policy}),
    )?))
}
```

`src/lib.rs`: add `pub mod process;` and `pub mod secrets;`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --test key_command && cargo test --lib golden && cargo test --test adapters`
Expected: all PASS; the existing adapter tests (env keys) are unchanged.

- [ ] **Step 7: Full check and commit**

Run: `cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`

```bash
git add src/process.rs src/secrets.rs src/lib.rs src/domain.rs src/config.rs src/provider.rs src/service.rs src/engine/himalaya.rs tests/key_command.rs
git commit -m "Read the OpenRouter key from a key command

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Config path resolution and prompts

**Files:**
- Create: `src/prompt.rs`, `tests/config_path.rs`
- Modify: `src/config.rs` (new functions after `save`), `src/cli.rs` (`Cli.config`, every `&cli.config` use, `Init`, `watch`), `src/service.rs:131` (open message), `src/lib.rs`

**Interfaces:**
- Consumes: `service::err` (crate-visible `err(code, message) -> anyhow::Error`).
- Produces:
  - `config::CONFIG_FILE: &str = "mailtriage.json"`;
  - `config::home_config_path(home: Option<&Path>) -> Option<PathBuf>`;
  - `config::resolve_path(flag: Option<&Path>, env: Option<&OsStr>, cwd: &Path, home: Option<&Path>) -> anyhow::Result<PathBuf>`;
  - `config::setup_path(flag: Option<&Path>, env: Option<&OsStr>, home: Option<&Path>) -> anyhow::Result<PathBuf>`;
  - `prompt::Prompter<'a>`:
    - `new(input: impl Read + 'a, output: impl Write + 'a, enabled: bool)`;
    - `enabled()`, `say(&str)`;
    - `ask(question, default: Option<&str>, check: impl Fn(&str) -> Result<String, String>) -> anyhow::Result<String>`;
    - `confirm(question, default: bool) -> anyhow::Result<bool>`;
    - `choose(question, options: &[String], default: usize) -> anyhow::Result<usize>`;
    - `choose_many(question, options: &[String], defaults: &[usize]) -> anyhow::Result<Vec<usize>>`;
  - `prompt::stdin_unbuffered() -> Box<dyn Read>`.
- End of input during a prompt is `err(2, "setup aborted: input ended")`.

- [ ] **Step 1: Write the failing tests**

Create `tests/config_path.rs`:

```rust
//! Config resolution through the binary: `--config`, `MAILTRIAGE_CONFIG`,
//! `./mailtriage.json`, then `~/.config/mailtriage/mailtriage.json`.
use mailtriage::config;
use serde_json::Value;
use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn mailtriage(cwd: &Path, home: Option<&Path>, env_config: Option<&Path>, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mailtriage"));
    command
        .current_dir(cwd)
        .args(args)
        .env_remove("MAILTRIAGE_CONFIG")
        .env_remove("HOME");
    if let Some(home) = home {
        command.env("HOME", home);
    }
    if let Some(path) = env_config {
        command.env("MAILTRIAGE_CONFIG", path);
    }
    command.output().unwrap()
}

/// The state directory `doctor` reports, which tells configs apart.
fn doctor_state(cwd: &Path, home: Option<&Path>, env: Option<&Path>, extra: &[&str]) -> String {
    let mut args = vec!["doctor", "--account", "work", "--json"];
    args.extend_from_slice(extra);
    let out = mailtriage(cwd, home, env, &args);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stdout));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    v["state_dir"].as_str().unwrap().to_owned()
}

#[test]
fn commands_find_the_config_in_the_documented_order() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    let home = root.path().join("home");
    for dir in [&a, &b, &home] {
        fs::create_dir_all(dir).unwrap();
    }
    // `init` writes ./mailtriage.json and ignores MAILTRIAGE_CONFIG.
    let elsewhere = b.join("elsewhere.json");
    assert!(mailtriage(&a, Some(&home), Some(&elsewhere), &["init", "--json"]).status.success());
    assert!(a.join("mailtriage.json").exists());
    assert!(!elsewhere.exists());
    // Nothing in b and nothing under HOME: exit 2, pointing at setup.
    let out = mailtriage(&b, Some(&home), None, &["doctor", "--account", "work", "--json"]);
    assert_eq!(out.status.code(), Some(2));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["error"]["message"].as_str().unwrap().contains("mailtriage setup"));
    // 4. The home config.
    let home_config = home.join(".config/mailtriage/mailtriage.json");
    let made = mailtriage(&b, Some(&home), None, &["init", "--json", "--config", home_config.to_str().unwrap()]);
    assert!(made.status.success());
    assert!(doctor_state(&b, Some(&home), None, &[]).ends_with(".config/mailtriage/.state"));
    // 3. ./mailtriage.json beats the home config.
    assert!(doctor_state(&a, Some(&home), None, &[]).ends_with("a/.state"));
    // 2. MAILTRIAGE_CONFIG beats ./mailtriage.json.
    assert!(doctor_state(&a, Some(&home), Some(&home_config), &[]).ends_with(".config/mailtriage/.state"));
    // 1. --config beats MAILTRIAGE_CONFIG.
    let local = a.join("mailtriage.json");
    assert!(doctor_state(&b, Some(&home), Some(&home_config), &["--config", local.to_str().unwrap()]).ends_with("a/.state"));
}

#[test]
fn without_home_the_error_names_config() {
    let dir = tempfile::tempdir().unwrap();
    let out = mailtriage(dir.path(), None, None, &["doctor", "--account", "work", "--json"]);
    assert_eq!(out.status.code(), Some(2));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["error"]["message"].as_str().unwrap().contains("--config"), "{v}");
}

#[test]
fn an_empty_environment_value_counts_as_unset() {
    let cwd = Path::new("/nonexistent-cwd");
    let home = Path::new("/h");
    assert_eq!(
        config::resolve_path(None, Some(OsStr::new("")), cwd, Some(home)).unwrap(),
        PathBuf::from("/h/.config/mailtriage/mailtriage.json")
    );
    assert_eq!(
        config::setup_path(None, Some(OsStr::new("/x.json")), Some(home)).unwrap(),
        PathBuf::from("/x.json")
    );
    assert!(config::setup_path(None, None, None).is_err());
}
```

Add a test module at the end of `src/prompt.rs` (the file is created in Step 3; write the tests first):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::ServiceError;

    fn check(answer: &str) -> Result<String, String> {
        if answer.contains(' ') {
            Err("No spaces, please.".to_owned())
        } else {
            Ok(answer.to_owned())
        }
    }

    #[test]
    fn enter_takes_the_default_and_invalid_answers_are_asked_again() {
        let mut out = Vec::new();
        let mut p = Prompter::new(&b"\nbad name\nok\n"[..], &mut out, true);
        assert_eq!(p.ask("Name", Some("work"), check).unwrap(), "work");
        assert_eq!(p.ask("Name", None, check).unwrap(), "ok");
        drop(p);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Name [work]: "), "{text}");
        assert!(text.contains("  No spaces, please."), "{text}");
    }

    #[test]
    fn menus_are_numbered_and_ask_again_when_out_of_range() {
        let mut out = Vec::new();
        let options = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        let mut p = Prompter::new(&b"9\n2\n\n3 1\nx\n2,2\n"[..], &mut out, true);
        assert_eq!(p.choose("Pick", &options, 0).unwrap(), 1);
        assert_eq!(p.choose_many("Pick", &options, &[0]).unwrap(), vec![0]);
        assert_eq!(p.choose_many("Pick", &options, &[0]).unwrap(), vec![0, 2]);
        assert_eq!(p.choose_many("Pick", &options, &[0]).unwrap(), vec![1]);
        drop(p);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("  1) a (default)"), "{text}");
        assert!(text.contains("Please enter a number from 1 to 3."), "{text}");
        assert!(text.contains("Please enter numbers from 1 to 3"), "{text}");
    }

    #[test]
    fn confirm_accepts_yes_no_and_the_default() {
        let mut out = Vec::new();
        let mut p = Prompter::new(&b"\nn\nmaybe\nYES\n"[..], &mut out, true);
        assert!(p.confirm("Go?", true).unwrap());
        assert!(!p.confirm("Go?", true).unwrap());
        assert!(p.confirm("Go?", false).unwrap());
        drop(p);
        assert!(String::from_utf8(out).unwrap().contains("Go? [y/N]: "));
    }

    #[test]
    fn end_of_input_aborts_with_exit_2() {
        let mut out = Vec::new();
        let mut p = Prompter::new(&b""[..], &mut out, true);
        let error = p.ask("Name", Some("x"), check).unwrap_err();
        let error = error.downcast_ref::<ServiceError>().unwrap();
        assert_eq!((error.code, error.message.as_str()), (2, "setup aborted: input ended"));
    }

    #[test]
    fn answers_are_read_without_reading_ahead() {
        let mut input: &[u8] = b"first\nrest for a child\n";
        let mut out = Vec::new();
        let mut p = Prompter::new(&mut input, &mut out, true);
        assert_eq!(p.ask("Q", None, check).unwrap(), "first");
        drop(p);
        assert_eq!(input, b"rest for a child\n");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test config_path`
Expected: compile errors (`resolve_path`, `setup_path` not found).

- [ ] **Step 3: Implement `src/prompt.rs`**

```rust
//! Numbered prompts for `setup`, standard library only. Questions go to one
//! stream (stderr); answers are read from another (stdin) one byte at a
//! time, so a child process that inherits stdin sees exactly the bytes no
//! answer has consumed.
use crate::service::err;
use anyhow::Result;
use std::io::{Read, Write};

pub struct Prompter<'a> {
    input: Box<dyn Read + 'a>,
    output: Box<dyn Write + 'a>,
    enabled: bool,
}

impl<'a> Prompter<'a> {
    pub fn new(input: impl Read + 'a, output: impl Write + 'a, enabled: bool) -> Self {
        Self {
            input: Box::new(input),
            output: Box::new(output),
            enabled,
        }
    }

    /// Whether questions are asked; otherwise callers use flags and defaults.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// A line of progress or explanation; written with or without prompts.
    pub fn say(&mut self, text: &str) {
        let _ = writeln!(self.output, "{text}");
    }

    /// One answer, trimmed. End of input before any byte aborts setup.
    fn line(&mut self) -> Result<String> {
        let _ = self.output.flush();
        let mut bytes = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            match self.input.read(&mut byte) {
                Ok(0) if bytes.is_empty() => return Err(err(2, "setup aborted: input ended")),
                Ok(0) => break,
                Ok(_) if byte[0] == b'\n' => break,
                Ok(_) => bytes.push(byte[0]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return Err(err(2, "setup aborted: input ended")),
            }
        }
        Ok(String::from_utf8_lossy(&bytes).trim().to_owned())
    }

    /// Asks until `check` accepts the answer; Enter takes `default`.
    pub fn ask(
        &mut self,
        question: &str,
        default: Option<&str>,
        check: impl Fn(&str) -> Result<String, String>,
    ) -> Result<String> {
        loop {
            match default.filter(|d| !d.is_empty()) {
                Some(d) => {
                    let _ = write!(self.output, "{question} [{d}]: ");
                }
                None => {
                    let _ = write!(self.output, "{question}: ");
                }
            }
            let answer = self.line()?;
            let answer = if answer.is_empty() {
                default.unwrap_or("").to_owned()
            } else {
                answer
            };
            match check(&answer) {
                Ok(value) => return Ok(value),
                Err(reason) => self.say(&format!("  {reason}")),
            }
        }
    }

    pub fn confirm(&mut self, question: &str, default: bool) -> Result<bool> {
        let hint = if default { "Y/n" } else { "y/N" };
        loop {
            let _ = write!(self.output, "{question} [{hint}]: ");
            match self.line()?.to_ascii_lowercase().as_str() {
                "" => return Ok(default),
                "y" | "yes" => return Ok(true),
                "n" | "no" => return Ok(false),
                _ => self.say("  Please answer y or n."),
            }
        }
    }

    /// A numbered menu (1-based on screen); returns the 0-based index.
    pub fn choose(&mut self, question: &str, options: &[String], default: usize) -> Result<usize> {
        self.menu(question, options, &[default]);
        let n = options.len();
        loop {
            let _ = write!(self.output, "Choose 1-{n} [{}]: ", default + 1);
            let answer = self.line()?;
            if answer.is_empty() {
                return Ok(default);
            }
            match answer.parse::<usize>() {
                Ok(k) if (1..=n).contains(&k) => return Ok(k - 1),
                _ => self.say(&format!("  Please enter a number from 1 to {n}.")),
            }
        }
    }

    /// Several entries, numbers separated by commas or spaces; returns
    /// sorted, unique 0-based indices.
    pub fn choose_many(
        &mut self,
        question: &str,
        options: &[String],
        defaults: &[usize],
    ) -> Result<Vec<usize>> {
        self.menu(question, options, defaults);
        let n = options.len();
        let shown: Vec<String> = defaults.iter().map(|d| (d + 1).to_string()).collect();
        loop {
            let _ = write!(
                self.output,
                "Choose one or more of 1-{n}, separated by commas [{}]: ",
                shown.join(",")
            );
            let answer = self.line()?;
            if answer.is_empty() {
                return Ok(defaults.to_vec());
            }
            let picked: Option<std::collections::BTreeSet<usize>> = answer
                .split([',', ' '])
                .filter(|part| !part.is_empty())
                .map(|part| {
                    part.parse::<usize>()
                        .ok()
                        .filter(|k| (1..=n).contains(k))
                        .map(|k| k - 1)
                })
                .collect();
            match picked {
                Some(set) if !set.is_empty() => return Ok(set.into_iter().collect()),
                _ => self.say(&format!(
                    "  Please enter numbers from 1 to {n}, for example 1,3."
                )),
            }
        }
    }

    fn menu(&mut self, question: &str, options: &[String], marked: &[usize]) {
        self.say(question);
        for (i, option) in options.iter().enumerate() {
            let mark = if marked.contains(&i) { " (default)" } else { "" };
            self.say(&format!("  {}) {option}{mark}", i + 1));
        }
    }
}

/// Stdin without the standard library's buffer, so the bytes after an
/// answer stay for a child process that inherits stdin.
pub fn stdin_unbuffered() -> Box<dyn Read> {
    #[cfg(unix)]
    {
        use std::os::fd::AsFd;
        if let Ok(fd) = std::io::stdin().as_fd().try_clone_to_owned() {
            return Box::new(std::fs::File::from(fd));
        }
    }
    Box::new(std::io::stdin())
}
```

Add `pub mod prompt;` to `src/lib.rs`.

- [ ] **Step 4: Config resolution in `src/config.rs`**

Add `use anyhow::anyhow;` and `std::ffi::OsStr`/`PathBuf` imports, then:

```rust
/// The config file name, in the working directory and the home config dir.
pub const CONFIG_FILE: &str = "mailtriage.json";

/// `~/.config/mailtriage/mailtriage.json`; `None` without a home directory.
pub fn home_config_path(home: Option<&Path>) -> Option<PathBuf> {
    home.filter(|h| !h.as_os_str().is_empty())
        .map(|h| h.join(".config").join("mailtriage").join(CONFIG_FILE))
}

/// The config every command except `init` and `setup` uses: `--config`,
/// else `MAILTRIAGE_CONFIG`, else `./mailtriage.json` if it exists, else
/// `~/.config/mailtriage/mailtriage.json`.
pub fn resolve_path(
    flag: Option<&Path>,
    env: Option<&OsStr>,
    cwd: &Path,
    home: Option<&Path>,
) -> Result<PathBuf> {
    if flag.is_none() && env.is_none_or(OsStr::is_empty) {
        let local = cwd.join(CONFIG_FILE);
        if local.exists() {
            return Ok(local);
        }
    }
    setup_path(flag, env, home)
}

/// The config `setup` writes: `--config`, else `MAILTRIAGE_CONFIG`, else
/// the home config. It never picks `./mailtriage.json`.
pub fn setup_path(flag: Option<&Path>, env: Option<&OsStr>, home: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = flag {
        return Ok(path.to_path_buf());
    }
    if let Some(path) = env.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    home_config_path(home)
        .ok_or_else(|| anyhow!("cannot find the config: HOME is not set; pass --config PATH"))
}
```

`src/service.rs` `open`: the not-found message becomes `"configuration not found; run `mailtriage setup` or pass --config"`.

- [ ] **Step 5: CLI**

In `src/cli.rs`:
- `Cli.config` becomes `#[arg(long, global = true)] config: Option<PathBuf>` with the doc comment `/// Config file (default: MAILTRIAGE_CONFIG, ./mailtriage.json if present, else ~/.config/mailtriage/mailtriage.json).`
- Add:

```rust
impl Cli {
    /// The config path for every command except `init` and `setup`.
    fn config_path(&self) -> Result<PathBuf, CliError> {
        let cwd = std::env::current_dir()
            .map_err(|_| CliError::input("cannot read the working directory; pass --config"))?;
        let home = std::env::var_os("HOME").map(PathBuf::from);
        config::resolve_path(
            self.config.as_deref(),
            std::env::var_os("MAILTRIAGE_CONFIG").as_deref(),
            &cwd,
            home.as_deref(),
        )
        .map_err(|e| CliError::input(e.to_string()))
    }
}
```

- Replace each `open(&cli.config)?` with `open(&cli.config_path()?)?` and `filing(&cli.config, command)` with `filing(&cli.config_path()?, command)`. `categories validate` without `--account` must not resolve a path.
- `Init` uses `let path = cli.config.clone().unwrap_or_else(|| PathBuf::from(config::CONFIG_FILE));` in place of `cli.config`, including in its JSON.
- `watch` resolves `let path = cli.config_path()?;` once and passes `&path` to `Service::open` in the pass closure.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --test config_path && cargo test --lib prompt && cargo test --test cli && cargo test --test filing_cli`
Expected: all PASS.

- [ ] **Step 7: Full check and commit**

Run: `cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`

```bash
git add src/prompt.rs src/config.rs src/cli.rs src/service.rs src/lib.rs tests/config_path.rs
git commit -m "Find the config in a fixed order and add setup prompts

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: `mailtriage setup` (steps 1–9, command and env keys)

**Files:**
- Create: `src/setup.rs`, `tests/setup.rs`
- Modify:
  - `src/secrets.rs` (`KeyStore` with `Command`, `Env`; `key_store_options`)
  - `src/config.rs` (`valid_account_name`, `unsafe_source_mailbox`)
  - `src/engine/himalaya.rs:89-106` (`check_version_output`)
  - `src/cli.rs` (`Setup` subcommand)
  - `src/lib.rs`

**Interfaces:**
- Consumes:
  - Task 1: `process::{run_bounded, find_on_path, Bounded::success}`, `secrets::{run_key_command, key_source}`, `provider::{DECISIONS_ENDPOINT, valid_env_name}`.
  - Task 2: `prompt::{Prompter, stdin_unbuffered}`, `config::setup_path`.
  - Existing: `Service::{open, doctor}`, `engine::open`, `MailEngine::list_folders`, `config::{load, save, validate, default_config, SOURCE_RULE}`, `filing::mode_str`.
- Produces:
  - `setup::SetupArgs` (the fields below, `Default`);
  - `setup::run(args: &SetupArgs, path: &Path, prompt: &mut Prompter) -> anyhow::Result<Value>`;
  - `setup::{DEFAULT_MODEL, DEFAULT_KEY_ENV}`, `setup::default_timezone`, `setup::himalaya_config_candidates`;
  - `secrets::KeyStore` (`Copy`, `Eq`) with `flag()`, `from_flag(&str)`, `label()`, `KeyStore::ALL`;
  - `secrets::key_store_options(macos: bool, has_tool: impl Fn(&str) -> bool) -> Vec<KeyStore>`;
  - `config::valid_account_name(&str) -> bool`, `config::unsafe_source_mailbox(&str) -> bool`;
  - `himalaya::check_version_output(output: &[u8], expected: &str) -> anyhow::Result<String>`.
  - The setup result is `{"schema_version":1,"setup":{config, account, mailboxes, provider, model, key_source, key_store, filing, doctor, service}}`, where:
    - `service` is `null` until Task 5;
    - `doctor` is `{"ready": bool, "items": [{"check","ready","error"?,"fix"?}]}`, with the checks `state`, `provider`, `key`, `mail` and `filing`.

**Prompt order** (each reads one line; a flag skips its prompt):
1. Existing-config menu, only when the config exists: Update, Add, Abort.
2. Which account (only for Update).
3. Himalaya account menu: the IMAP accounts, then "Create a new account with `himalaya configure`". When none exist, a confirm takes its place.
4. Account name.
5. Identity.
6. Time zone.
7. Brief.
8. Folders (`choose_many`).
9. "Keep the current classifier?" (only when the config exists).
10. Classifier menu (openrouter, fake).
11. Model.
12. Key store menu.
13. Key command text or variable name.
14. Filing menu (dry run, off).

- [ ] **Step 1: Write the failing tests**

Create `tests/setup.rs`:

```rust
#![cfg(unix)]
//! `mailtriage setup` through the binary, with a fake Himalaya first on PATH
//! and HOME in a temp dir. Prompts are driven with `--interactive` and piped
//! stdin; every other run uses `--yes`.
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

/// Fake Himalaya v2.1.0. Accounts come from `accounts.json` next to the
/// script; accounts listed in `failing.json` fail `account check`;
/// `configure` adds the account `fresh`. Every call is logged to `calls.log`.
const HIMALAYA: &str = r#"#!/usr/bin/env python3
import json, os, sys
here = os.path.dirname(os.path.abspath(__file__))
argv = sys.argv[1:]
with open(os.path.join(here, "calls.log"), "a") as log:
    log.write(json.dumps(argv) + "\n")
def out(text):
    sys.stdout.write(text)
    sys.stdout.flush()
def load(name, default):
    try:
        with open(os.path.join(here, name)) as f:
            return json.load(f)
    except FileNotFoundError:
        return default
if argv and argv[-1] == "--version":
    out("himalaya v2.1.0 +smtp +imap +maildir\nbuild: test\n")
    sys.exit(0)
opts, rest, i = {}, [], 0
while i < len(argv):
    if argv[i] in ("--config", "--account", "--backend"):
        opts[argv[i]] = argv[i + 1]
        i += 2
    elif argv[i] == "--json":
        i += 1
    else:
        rest.append(argv[i])
        i += 1
accounts = load("accounts.json", [])
if rest == ["account", "list"]:
    out(json.dumps({"accounts": accounts}))
elif rest == ["account", "check"]:
    ok = opts.get("--account") not in load("failing.json", [])
    out(json.dumps({"account": opts.get("--account"), "backends": [
        {"backend": "imap", "ok": ok, "error": None if ok else "SECRET-DETAIL refused"}]}))
elif rest == ["configure"]:
    accounts.append({"name": "fresh", "default": False, "backends": ["imap"]})
    with open(os.path.join(here, "accounts.json"), "w") as f:
        json.dump(accounts, f)
    toml = opts.get("--config") or os.path.join(os.environ["HOME"], ".config", "himalaya", "config.toml")
    os.makedirs(os.path.dirname(toml), exist_ok=True)
    with open(toml, "a") as f:
        f.write('\n[accounts.fresh]\nemail = "fresh@example.test"\nimap.server = "imaps://fresh.example.test"\n')
elif rest[:2] == ["imap", "raw"]:
    text = rest[-1]
    if "CAPABILITY" in text:
        out('* CAPABILITY IMAP4rev1 MOVE UIDPLUS SPECIAL-USE\r\na1 OK done\r\n* NAMESPACE (("" "/")) NIL NIL\r\na2 OK done\r\n')
    elif "RETURN (SPECIAL-USE)" in text:
        out('* LIST (\\HasNoChildren) "/" "INBOX"\r\n* LIST (\\HasNoChildren \\Sent) "/" "Sent"\r\na1 OK done\r\n')
    else:
        sys.exit(5)
elif rest[:2] == ["imap", "list"]:
    rows = [{"name": "INBOX", "delimiter": "/", "attributes": []},
            {"name": "Sent", "delimiter": "/", "attributes": ["\\Sent"]},
            {"name": "Lists", "delimiter": "/", "attributes": ["\\Noselect"]},
            {"name": "Lists/News", "delimiter": "/", "attributes": []},
            {"name": "Caf&AOk-", "delimiter": "/", "attributes": []}]
    out(json.dumps({"mailboxes": rows}))
else:
    sys.exit(7)
"#;

const TOML: &str = "[accounts.home]\nemail = \"home@example.test\"\nimap.server = \"imaps://home.example.test\"\n\n[accounts.work]\ndefault = true\nemail = \"work@example.test\"\nimap.server = \"imaps://mail.example.test\"\nimap.sasl.plain.username = \"work@example.test\"\nimap.sasl.plain.password.raw = \"fixture-pass\"\n";

fn write_tool(dir: &Path, name: &str, script: &str) {
    let path = dir.join(name);
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn message(v: &Value) -> String {
    v["error"]["message"].as_str().unwrap_or_default().to_owned()
}

struct Fixture {
    _dir: tempfile::TempDir,
    home: PathBuf,
    bin: PathBuf,
    cwd: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let bin = dir.path().join("bin");
        let cwd = dir.path().join("cwd");
        for d in [&home, &bin, &cwd] {
            fs::create_dir_all(d).unwrap();
        }
        write_tool(&bin, "himalaya", HIMALAYA);
        let f = Self { _dir: dir, home, bin, cwd };
        f.set_accounts(json!([
            {"name": "home", "default": false, "backends": ["imap"]},
            {"name": "work", "default": true, "backends": ["imap"]}
        ]));
        fs::create_dir_all(f.himalaya_toml().parent().unwrap()).unwrap();
        fs::write(f.himalaya_toml(), TOML).unwrap();
        f
    }

    fn himalaya_toml(&self) -> PathBuf {
        self.home.join(".config/himalaya/config.toml")
    }

    fn set_accounts(&self, accounts: Value) {
        fs::write(self.bin.join("accounts.json"), accounts.to_string()).unwrap();
    }

    fn fail_check(&self, account: &str) {
        fs::write(self.bin.join("failing.json"), json!([account]).to_string()).unwrap();
    }

    fn calls(&self) -> String {
        fs::read_to_string(self.bin.join("calls.log")).unwrap_or_default()
    }

    fn config_path(&self) -> PathBuf {
        self.home.join(".config/mailtriage/mailtriage.json")
    }

    fn config(&self) -> Value {
        serde_json::from_slice(&fs::read(self.config_path()).unwrap()).unwrap()
    }

    /// Runs mailtriage with this fixture's HOME, PATH and time zone, extra
    /// environment `env`, and `stdin` piped in.
    fn run_with(&self, args: &[&str], stdin: &str, env: &[(&str, &str)]) -> (Output, Value) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mailtriage"));
        command
            .current_dir(&self.cwd)
            .args(args)
            .env("HOME", &self.home)
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("TZ", "Europe/Berlin")
            .env_remove("MAILTRIAGE_CONFIG")
            .env_remove("HIMALAYA_CONFIG")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("OPENROUTER_API_KEY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = command.spawn().unwrap();
        let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
        let out = child.wait_with_output().unwrap();
        let value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
        (out, value)
    }

    fn run(&self, args: &[&str], stdin: &str) -> (Output, Value) {
        self.run_with(args, stdin, &[])
    }
}

const WORK_ENV: [&str; 7] = [
    "setup", "--yes", "--json", "--himalaya-account", "work", "--key-store", "env",
];

#[test]
fn flags_alone_write_a_ready_config() {
    let f = Fixture::new();
    let (out, v) = f.run_with(&WORK_ENV, "", &[("OPENROUTER_API_KEY", "sk-or-fixture")]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let s = &v["setup"];
    assert_eq!(s["account"], "work");
    assert_eq!(s["mailboxes"], json!(["INBOX"]));
    assert_eq!(s["provider"], "openrouter");
    assert_eq!(s["model"], "typesafe/jev-1.13");
    assert_eq!(s["key_source"], "env");
    assert_eq!(s["key_store"], "env");
    assert_eq!(s["filing"], "dry_run");
    assert_eq!(s["doctor"]["ready"], true, "{}", s["doctor"]);
    assert_eq!(
        s["config"],
        fs::canonicalize(f.config_path()).unwrap().to_str().unwrap()
    );
    let c = f.config();
    assert_eq!(c["state_dir"], "state");
    assert_eq!(c["provider"]["api_key_env"], "OPENROUTER_API_KEY");
    assert!(c["provider"].get("api_key_command").is_none());
    let a = &c["accounts"]["work"];
    assert_eq!(a["identity"], "work@example.test");
    assert_eq!(a["timezone"], "Europe/Berlin");
    assert_eq!(a["filing"]["mode"], "dry_run");
    let e = &a["engine"];
    assert_eq!(e["kind"], "himalaya");
    assert_eq!(e["account"], "work");
    assert_eq!(e["binary"], f.bin.join("himalaya").to_str().unwrap());
    assert_eq!(e["config"], f.himalaya_toml().to_str().unwrap());
    assert_eq!(e["mailboxes"], json!(["INBOX"]));
    assert_eq!(e["expected_version"], "2.1.0");
    let categories = a["categories"].as_array().unwrap();
    assert_eq!(categories.len(), 6);
    assert!(categories.iter().all(|c| c["folder"] == c["name"]));
    let mode = |p: PathBuf| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(f.config_path()), 0o600);
    assert_eq!(mode(f.config_path().with_file_name("state")), 0o700);
    assert!(!stdout(&out).contains("sk-or-fixture"));
    assert!(!stderr(&out).contains("sk-or-fixture"));
    // No IMAP changes, no wizard.
    for forbidden in ["\"configure\"", "\"create\"", "\"subscribe\"", "MOVE", "STORE"] {
        assert!(!f.calls().contains(forbidden), "{forbidden}");
    }
    // Other commands now find the home config without --config.
    let (out, v) = f.run(&["doctor", "--account", "work", "--json"], "");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(v["provider"]["kind"], "openrouter");
}

#[test]
fn without_prompts_missing_answers_name_their_flag() {
    let f = Fixture::new();
    let cases: [(&[&str], &str); 4] = [
        (&["setup", "--yes", "--json", "--key-store", "env"], "--himalaya-account"),
        (
            &["setup", "--yes", "--json", "--himalaya-account", "nobody", "--key-store", "env"],
            "--himalaya-account",
        ),
        (
            &["setup", "--yes", "--json", "--himalaya-account", "work", "--key-store", "command"],
            "--key-command",
        ),
        (
            &["setup", "--yes", "--json", "--himalaya-account", "work", "--key-store", "env", "--account", "my work"],
            "--account",
        ),
    ];
    for (args, flag) in cases {
        let (out, v) = f.run(args, "");
        assert_eq!(out.status.code(), Some(2), "{args:?}: {v}");
        assert!(message(&v).contains(flag), "{args:?}: {v}");
    }
    assert!(!f.config_path().exists());
}

#[test]
fn an_existing_config_needs_update_and_keeps_other_accounts() {
    let f = Fixture::new();
    assert_eq!(f.run(&WORK_ENV, "").0.status.code(), Some(0));
    // The user's own changes to the work account.
    let mut c = f.config();
    c["accounts"]["work"]["categories"][0]["description"] = json!("People I write with");
    c["accounts"]["work"]["filing"]["mode"] = json!("live");
    fs::write(f.config_path(), serde_json::to_vec_pretty(&c).unwrap()).unwrap();
    let before = f.config();

    let (out, v) = f.run(&WORK_ENV, "");
    assert_eq!(out.status.code(), Some(5));
    assert!(message(&v).contains("--update"), "{v}");
    assert_eq!(f.config(), before);

    let (out, v) = f.run(
        &["setup", "--yes", "--update", "--json", "--himalaya-account", "home", "--account", "home"],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(v["setup"]["key_store"], Value::Null);
    let after = f.config();
    assert_eq!(after["accounts"]["work"], before["accounts"]["work"]);
    assert_eq!(after["provider"], before["provider"]);
    assert_eq!(after["accounts"]["home"]["identity"], "home@example.test");

    // Updating work keeps its categories and its live mode.
    let (out, _) = f.run(
        &["setup", "--yes", "--update", "--json", "--himalaya-account", "work", "--brief", "Runs a bakery"],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let work = &f.config()["accounts"]["work"];
    assert_eq!(work["brief"], "Runs a bakery");
    assert_eq!(work["filing"]["mode"], "live");
    assert_eq!(work["categories"][0]["description"], "People I write with");
}

#[test]
fn prompts_offer_defaults_and_ask_again_after_invalid_input() {
    let f = Fixture::new();
    // Himalaya account (9 is out of range, then Enter for work); name ("my
    // work" is invalid, then Enter); identity; time zone; brief; folders 1
    // and 3; provider; model; key variable; filing.
    let input = "9\n\nmy work\n\n\n\nRuns a bakery\n1,3\n\n\n\n\n";
    let (out, v) = f.run(&["setup", "--interactive", "--json", "--key-store", "env"], input);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("Please enter a number from 1 to 3."), "{err}");
    assert!(err.contains("Use 1 to 64 ASCII letters, digits, - or _."), "{err}");
    assert!(err.contains("Time zone [Europe/Berlin]"), "{err}");
    assert!(!err.contains(") Lists\n"), "a \\Noselect folder is offered: {err}");
    let a = &f.config()["accounts"]["work"];
    assert_eq!(a["brief"], "Runs a bakery");
    assert_eq!(a["engine"]["mailboxes"], json!(["INBOX", "Lists/News"]));
    assert_eq!(v["setup"]["filing"], "dry_run");
}

#[test]
fn end_of_input_aborts_without_writing() {
    let f = Fixture::new();
    let (out, v) = f.run(&["setup", "--interactive", "--json"], "");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(message(&v), "setup aborted: input ended");
    assert!(!f.config_path().exists());
    // Ending after the account menu and the name is no different.
    let (out, _) = f.run(&["setup", "--interactive", "--json"], "\n\n");
    assert_eq!(out.status.code(), Some(2));
    assert!(!f.config_path().exists());
}

#[test]
fn choosing_abort_changes_nothing() {
    let f = Fixture::new();
    assert_eq!(f.run(&WORK_ENV, "").0.status.code(), Some(0));
    let before = fs::read(f.config_path()).unwrap();
    let (out, v) = f.run(&["setup", "--interactive", "--json"], "3\n");
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(message(&v), "setup aborted; nothing was changed");
    assert_eq!(fs::read(f.config_path()).unwrap(), before);
}

#[test]
fn without_an_account_setup_runs_himalaya_configure() {
    let f = Fixture::new();
    fs::remove_file(f.himalaya_toml()).unwrap();
    f.set_accounts(json!([]));
    let (out, v) = f.run(&WORK_ENV, "");
    assert_eq!(out.status.code(), Some(3));
    assert!(message(&v).contains("himalaya configure"), "{v}");
    assert!(!f.calls().contains("\"configure\""));
    // Confirm creating one, take it (the default), then every default.
    let input = "\n".repeat(11);
    let (out, v) = f.run(&["setup", "--interactive", "--json", "--key-store", "env"], &input);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(f.calls().contains("\"configure\""));
    assert_eq!(v["setup"]["account"], "fresh");
    assert_eq!(f.config()["accounts"]["fresh"]["identity"], "fresh@example.test");
}

#[test]
fn a_failed_account_check_exits_3_without_himalaya_output() {
    let f = Fixture::new();
    f.fail_check("work");
    let (out, v) = f.run(&WORK_ENV, "");
    assert_eq!(out.status.code(), Some(3));
    assert!(message(&v).contains("account check"), "{v}");
    assert!(!stdout(&out).contains("SECRET-DETAIL"));
    assert!(!stderr(&out).contains("SECRET-DETAIL"));
    assert!(!f.config_path().exists());
}

#[test]
fn a_key_command_is_verified_and_stored_as_sh_c() {
    let f = Fixture::new();
    let args = ["setup", "--yes", "--json", "--himalaya-account", "work", "--key-command"];
    let (out, v) = f.run(&[&args[..], &["printf 'sk-or-cmd-123\\n'"]].concat(), "");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(v["setup"]["key_store"], "command");
    assert_eq!(v["setup"]["key_source"], "command");
    assert_eq!(v["setup"]["doctor"]["ready"], true, "{}", v["setup"]["doctor"]);
    assert_eq!(
        f.config()["provider"]["api_key_command"],
        json!(["/bin/sh", "-c", "printf 'sk-or-cmd-123\\n'"])
    );
    assert_eq!(f.config()["provider"]["api_key_env"], "OPENROUTER_API_KEY");
    assert!(!stdout(&out).contains("sk-or-cmd-123"));
    assert!(!stderr(&out).contains("sk-or-cmd-123"));

    fs::remove_file(f.config_path()).unwrap();
    let (out, v) = f.run(&[&args[..], &["exit 1"]].concat(), "");
    assert_eq!(out.status.code(), Some(3));
    assert!(message(&v).contains("API key command failed (exit 1)"), "{v}");
    assert!(!f.config_path().exists());
}

#[test]
fn watched_folders_must_exist_and_be_safe_to_file_from() {
    let f = Fixture::new();
    let with = |extra: &[&str]| f.run(&[&WORK_ENV[..], extra].concat(), "");
    let (out, v) = with(&["--mailbox", "Nope"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(message(&v).contains("Nope"), "{v}");
    let (out, v) = with(&["--mailbox", "Caf&AOk-"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(message(&v).contains("--filing off"), "{v}");
    let (out, v) = with(&["--mailbox", "INBOX", "--mailbox", "Caf&AOk-", "--filing", "off"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(v["setup"]["mailboxes"], json!(["INBOX", "Caf&AOk-"]));
    assert_eq!(v["setup"]["filing"], "off");
}

#[test]
fn the_offline_classifier_needs_no_key() {
    let f = Fixture::new();
    let (out, v) = f.run(
        &["setup", "--yes", "--json", "--himalaya-account", "work", "--provider", "fake"],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(v["setup"]["provider"], "fake");
    assert_eq!(v["setup"]["key_source"], Value::Null);
    assert_eq!(v["setup"]["doctor"]["ready"], true, "{}", v["setup"]["doctor"]);
    assert_eq!(f.config()["provider"]["kind"], "fake");
}

#[test]
fn doctor_items_that_are_not_ready_carry_a_fix() {
    let f = Fixture::new();
    let (out, v) = f.run(&WORK_ENV, "");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doctor = &v["setup"]["doctor"];
    assert_eq!(doctor["ready"], false);
    let key = doctor["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["check"] == "key")
        .unwrap();
    assert_eq!(key["ready"], false);
    assert!(key["fix"].as_str().unwrap().contains("export OPENROUTER_API_KEY="), "{key}");
    assert!(stderr(&out).contains("export OPENROUTER_API_KEY="));
}

#[test]
fn setup_writes_the_home_config_unless_told_otherwise() {
    let f = Fixture::new();
    // An older ./mailtriage.json is left alone.
    assert!(f.run(&["init", "--json"], "").0.status.success());
    let local = fs::read(f.cwd.join("mailtriage.json")).unwrap();
    assert_eq!(f.run(&WORK_ENV, "").0.status.code(), Some(0));
    assert!(f.config_path().exists());
    assert_eq!(fs::read(f.cwd.join("mailtriage.json")).unwrap(), local);
    // MAILTRIAGE_CONFIG, then --config (a directory with a space).
    let env_path = f.cwd.join("env.json");
    let (out, _) = f.run_with(&WORK_ENV, "", &[("MAILTRIAGE_CONFIG", env_path.to_str().unwrap())]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(env_path.exists());
    let flag_path = f.cwd.join("Some Dir/flag.json");
    let (out, v) = f.run(&[&WORK_ENV[..], &["--config", flag_path.to_str().unwrap()]].concat(), "");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(flag_path.exists());
    assert!(v["setup"]["config"].as_str().unwrap().ends_with("Some Dir/flag.json"));
}

#[test]
fn a_himalaya_config_path_with_spaces_is_kept_whole() {
    let f = Fixture::new();
    let toml = f.home.join("Library/Application Support/himalaya/config.toml");
    fs::create_dir_all(toml.parent().unwrap()).unwrap();
    fs::rename(f.himalaya_toml(), &toml).unwrap();
    let (out, _) = f.run(
        &[&WORK_ENV[..], &["--himalaya-config", toml.to_str().unwrap()]].concat(),
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(f.config()["accounts"]["work"]["engine"]["config"], toml.to_str().unwrap());
    let quoted = serde_json::to_string(toml.to_str().unwrap()).unwrap();
    assert!(f.calls().contains(&quoted), "{}", f.calls());
}
```

Unit tests at the end of `src/setup.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_zone_defaults() {
        assert_eq!(default_timezone(Some("Europe/Berlin"), None), "Europe/Berlin");
        assert_eq!(default_timezone(Some(":America/New_York"), None), "America/New_York");
        assert_eq!(
            default_timezone(
                Some("/etc/localtime"),
                Some(Path::new("/var/db/timezone/zoneinfo/Europe/Vienna"))
            ),
            "Europe/Vienna"
        );
        assert_eq!(
            default_timezone(None, Some(Path::new("/usr/share/zoneinfo/Asia/Tokyo"))),
            "Asia/Tokyo"
        );
        assert_eq!(default_timezone(Some(""), Some(Path::new("/etc/other"))), "UTC");
        assert_eq!(default_timezone(None, None), "UTC");
    }

    #[test]
    fn himalaya_config_search_order() {
        let home = Path::new("/h");
        assert_eq!(
            himalaya_config_candidates(home, Some(Path::new("/x")), true),
            vec![
                PathBuf::from("/h/Library/Application Support/himalaya/config.toml"),
                PathBuf::from("/h/.config/himalaya/config.toml"),
                PathBuf::from("/h/.himalayarc"),
            ]
        );
        assert_eq!(
            himalaya_config_candidates(home, Some(Path::new("/x")), false),
            vec![
                PathBuf::from("/x/himalaya/config.toml"),
                PathBuf::from("/h/.config/himalaya/config.toml"),
                PathBuf::from("/h/.himalayarc"),
            ]
        );
        assert_eq!(
            himalaya_config_candidates(home, Some(Path::new("relative")), false),
            vec![
                PathBuf::from("/h/.config/himalaya/config.toml"),
                PathBuf::from("/h/.himalayarc"),
            ]
        );
    }

    #[test]
    fn account_names_fit_service_and_file_names() {
        assert!(config::valid_account_name("work-2_b"));
        assert!(config::valid_account_name(&"a".repeat(64)));
        assert!(!config::valid_account_name(&"a".repeat(65)));
        for bad in ["", "my work", "a.b", "ä", "a/b"] {
            assert!(!config::valid_account_name(bad), "{bad}");
        }
    }

    #[test]
    fn key_store_options_without_tool_stores() {
        assert_eq!(
            secrets::key_store_options(true, |_| true),
            vec![KeyStore::Command, KeyStore::Env]
        );
        assert_eq!(KeyStore::from_flag("env"), Some(KeyStore::Env));
        assert_eq!(KeyStore::from_flag("keychain"), None);
    }
}
```

(Task 4 replaces `key_store_options_without_tool_stores`.)

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test setup`
Expected: FAIL. `setup` is not a subcommand (clap exit 2), so every assertion on exit 0 fails.

- [ ] **Step 3: Small additions to existing modules**

`src/engine/himalaya.rs`: extract the version check so setup can run it before any TOML exists.

```rust
/// The version line of `himalaya --version` when it is `expected` with IMAP.
pub fn check_version_output(output: &[u8], expected: &str) -> Result<String> {
    let text = std::str::from_utf8(output).context("invalid Himalaya version output")?;
    let version = text.lines().next().unwrap_or_default().trim();
    let mut words = version.split_ascii_whitespace();
    let wanted = format!("v{expected}");
    if words.next() != Some("himalaya")
        || words.next() != Some(wanted.as_str())
        || !words.any(|feature| feature == "+imap")
    {
        bail!("unsupported Himalaya version; expected {expected}");
    }
    Ok(version.to_owned())
}
```

`Himalaya::version` becomes `check_version_output(&self.run(&["--version"], false)?, &self.config.expected_version)`.

`src/config.rs`:

```rust
/// Account names setup and the service accept: 1 to 64 ASCII letters,
/// digits, `-` or `_` (they become service labels and file names).
pub fn valid_account_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Whether filing cannot name `mailbox` safely in raw IMAP text
/// (`SOURCE_RULE`).
pub fn unsafe_source_mailbox(mailbox: &str) -> bool {
    !mailbox.chars().all(|c| (' '..='~').contains(&c))
        || mailbox.contains(['\\', '"', '&'])
        || mailbox.starts_with('-')
}
```

`unsafe_source_mailboxes` keeps its signature and filters with `|m| unsafe_source_mailbox(m)`.

`src/secrets.rs`:

```rust
/// Where setup keeps the key. Task 4 adds the tool-backed stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyStore {
    Command,
    Env,
}

impl KeyStore {
    pub const ALL: [KeyStore; 2] = [KeyStore::Command, KeyStore::Env];

    /// The `--key-store` value.
    pub fn flag(self) -> &'static str {
        match self {
            KeyStore::Command => "command",
            KeyStore::Env => "env",
        }
    }

    pub fn from_flag(flag: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|store| store.flag() == flag)
    }

    /// The menu text.
    pub fn label(self) -> &'static str {
        match self {
            KeyStore::Command => "A command that prints the key",
            KeyStore::Env => "An environment variable only",
        }
    }
}

/// The stores setup offers, the platform's own first.
pub fn key_store_options(_macos: bool, _has_tool: impl Fn(&str) -> bool) -> Vec<KeyStore> {
    vec![KeyStore::Command, KeyStore::Env]
}
```

- [ ] **Step 4: Implement `src/setup.rs`**

```rust
//! `mailtriage setup`: a guided first run. Every question is also a flag,
//! so agents run it without prompts. Setup writes only the mailtriage config
//! and its state directory: it makes no IMAP changes, never handles the API
//! key and never selects `live` filing.
use crate::{
    config,
    domain::{
        AccountConfig, AppConfig, Category, EngineConfig, FilingConfig, FilingMode,
        HimalayaConfig, ProviderConfig,
    },
    engine::{self, himalaya},
    filing, process,
    prompt::Prompter,
    provider,
    secrets::{self, KeyStore},
    service::{err, Service, ServiceError},
};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

pub const DEFAULT_MODEL: &str = "typesafe/jev-1.13";
pub const DEFAULT_KEY_ENV: &str = "OPENROUTER_API_KEY";
const HIMALAYA_VERSION: &str = "2.1.0";
const HIMALAYA_TIMEOUT: Duration = Duration::from_secs(60);
const HIMALAYA_MAX_OUTPUT: usize = 1024 * 1024;

/// Answers given as flags. `None` (or empty) means: ask, or without
/// prompts take the default, or fail naming the flag.
#[derive(Debug, Clone, Default)]
pub struct SetupArgs {
    pub update: bool,
    /// Stdin is a terminal, so a key tool can prompt even with `--yes`.
    pub terminal: bool,
    pub himalaya_binary: Option<PathBuf>,
    pub himalaya_config: Option<PathBuf>,
    pub himalaya_account: Option<String>,
    pub account: Option<String>,
    pub identity: Option<String>,
    pub timezone: Option<String>,
    pub brief: Option<String>,
    pub mailboxes: Vec<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub key_store: Option<KeyStore>,
    pub key_command: Option<String>,
    pub key_env: Option<String>,
    pub key_stored: bool,
    pub filing: Option<FilingMode>,
}

/// What step 1 decided.
enum Intent {
    /// No config yet.
    Create,
    /// Prompted: update this existing account.
    Update(String),
    /// Prompted: add an account; its name must be new.
    Add,
    /// `--update` without prompts: update the named account, or add it.
    UpdateOrAdd,
}

struct HimalayaChoice {
    binary: PathBuf,
    toml: PathBuf,
    account: String,
    email: Option<String>,
}

struct HimalayaAccount {
    name: String,
    default: bool,
}

pub fn run(args: &SetupArgs, path: &Path, p: &mut Prompter) -> Result<Value> {
    // 1. Config.
    let (mut cfg, intent) = load_target(args, path, p)?;
    // 2. Himalaya.
    let h = himalaya_step(args, p)?;
    // 3. Account details.
    let name = account_name(args, p, &h, &cfg, &intent)?;
    let previous = cfg.accounts.get(&name).cloned();
    let identity = answer(
        p,
        args.identity.as_deref(),
        "--identity",
        "Your email address for this account",
        previous
            .as_ref()
            .map(|a| a.identity.as_str())
            .or(h.email.as_deref()),
        nonempty,
    )?;
    let zone = previous.as_ref().map_or_else(
        || {
            default_timezone(
                std::env::var("TZ").ok().as_deref(),
                fs::read_link("/etc/localtime").ok().as_deref(),
            )
        },
        |a| a.timezone.clone(),
    );
    let timezone = answer(p, args.timezone.as_deref(), "--timezone", "Time zone", Some(&zone), |t| {
        if t.is_empty() || t.contains(char::is_whitespace) {
            Err("Use a time zone name such as Europe/Berlin.".to_owned())
        } else {
            Ok(t.to_owned())
        }
    })?;
    let brief = answer(
        p,
        args.brief.as_deref(),
        "--brief",
        "One line about you that helps classification (optional)",
        Some(previous.as_ref().map_or("", |a| a.brief.as_str())),
        |b| Ok(b.trim().to_owned()),
    )?;
    // 4. Folders.
    let old_engine = previous.as_ref().and_then(|a| match a.engine_config() {
        Some(EngineConfig::Himalaya(e)) => Some(e),
        None => None,
    });
    let mut engine = HimalayaConfig {
        binary: h.binary.clone(),
        config: h.toml.clone(),
        account: h.account.clone(),
        mailboxes: vec!["INBOX".to_owned()],
        expected_version: HIMALAYA_VERSION.to_owned(),
        timeout_seconds: old_engine.as_ref().map_or(60, |e| e.timeout_seconds),
        max_output_bytes: old_engine.as_ref().map_or(50_000_000, |e| e.max_output_bytes),
    };
    engine.mailboxes = folders_step(
        args,
        p,
        &engine,
        old_engine.as_ref().map(|e| e.mailboxes.as_slice()),
    )?;
    // 5. Classifier.
    let current = (!matches!(intent, Intent::Create)).then(|| cfg.provider.clone());
    let (provider, store) = classifier_step(args, p, current.as_ref())?;
    // 6. Categories.
    let categories = match &previous {
        Some(a) => a.categories.clone(),
        None => {
            let categories = default_categories();
            p.say(&categories_hint(&name, &categories));
            categories
        }
    };
    // 7. Filing.
    let mode = filing_step(
        args,
        p,
        previous.as_ref().map(|a| a.filing.mode),
        &engine.mailboxes,
    )?;
    // 8. Write.
    let filing_config = FilingConfig {
        mode,
        ..previous
            .as_ref()
            .map(|a| a.filing.clone())
            .unwrap_or_default()
    };
    cfg.accounts.insert(
        name.clone(),
        AccountConfig {
            identity,
            timezone,
            brief,
            taxonomy_revision: previous.as_ref().map_or(1, |a| a.taxonomy_revision),
            categories,
            himalaya: None,
            engine: Some(EngineConfig::Himalaya(engine.clone())),
            filing: filing_config,
        },
    );
    cfg.provider = provider;
    config::validate(&cfg).map_err(|e| err(2, format!("step 8 (write): {e}")))?;
    config::save(path, &cfg)
        .map_err(|_| err(3, format!("step 8 (write): could not write {}", path.display())))?;
    let path = fs::canonicalize(path)?;
    p.say(&format!("Wrote {}.", path.display()));
    // 9. Check.
    let doctor = doctor_step(p, &path, &name, &cfg.provider, &engine);
    next_steps(p, &name, mode);
    Ok(json!({"schema_version": 1, "setup": {
        "config": path,
        "account": name,
        "mailboxes": engine.mailboxes,
        "provider": cfg.provider.kind,
        "model": cfg.provider.model,
        "key_source": key_source_value(&cfg.provider),
        "key_store": store.map(KeyStore::flag),
        "filing": filing::mode_str(mode),
        "doctor": doctor,
        "service": Value::Null,
    }}))
}

/// Step 1: the config to change and what to do with it.
fn load_target(args: &SetupArgs, path: &Path, p: &mut Prompter) -> Result<(AppConfig, Intent)> {
    if !path.exists() {
        let mut cfg = config::default_config();
        cfg.accounts.clear();
        cfg.state_dir = PathBuf::from("state");
        return Ok((cfg, Intent::Create));
    }
    let cfg = config::load(path).map_err(|e| {
        err(
            2,
            format!(
                "step 1 (config): {} is not a valid config ({e:#}); fix it or pass --config",
                path.display()
            ),
        )
    })?;
    if !p.enabled() {
        if !args.update {
            return Err(err(
                5,
                format!(
                    "step 1 (config): {} already exists; pass --update to change it",
                    path.display()
                ),
            ));
        }
        return Ok((cfg, Intent::UpdateOrAdd));
    }
    p.say(&format!("A config already exists at {}.", path.display()));
    let actions = ["Update an account", "Add an account", "Abort"].map(String::from);
    match p.choose("What would you like to do?", &actions, 0)? {
        0 => {
            let names: Vec<String> = cfg.accounts.keys().cloned().collect();
            let pick = p.choose("Which account?", &names, 0)?;
            let name = names[pick].clone();
            Ok((cfg, Intent::Update(name)))
        }
        1 => Ok((cfg, Intent::Add)),
        _ => Err(err(2, "setup aborted; nothing was changed")),
    }
}

/// Step 2: the Himalaya binary, its config and the account, checked.
fn himalaya_step(args: &SetupArgs, p: &mut Prompter) -> Result<HimalayaChoice> {
    let binary = match &args.himalaya_binary {
        Some(binary) => absolute(binary)?,
        None => process::find_on_path("himalaya").ok_or_else(|| {
            err(
                3,
                "step 2 (Himalaya): himalaya is not on PATH; install Himalaya v2.1.0 or pass --himalaya-binary",
            )
        })?,
    };
    let version = run_himalaya(&binary, &[OsStr::new("--version")])
        .and_then(|out| himalaya::check_version_output(&out, HIMALAYA_VERSION))
        .map_err(|_| {
            err(
                3,
                format!(
                    "step 2 (Himalaya): {} is not Himalaya v{HIMALAYA_VERSION} with IMAP; install v{HIMALAYA_VERSION} or pass --himalaya-binary",
                    binary.display()
                ),
            )
        })?;
    p.say(&format!("Using {version} at {}.", binary.display()));
    let explicit = match &args.himalaya_config {
        Some(toml) => {
            let toml = absolute(toml)?;
            if !toml.is_file() {
                return Err(err(
                    2,
                    format!("--himalaya-config: {} does not exist", toml.display()),
                ));
            }
            Some(toml)
        }
        None => himalaya_config_env()?,
    };
    loop {
        let toml = explicit
            .clone()
            .or_else(default_himalaya_config)
            .filter(|t| t.is_file());
        let accounts = match &toml {
            Some(toml) => list_accounts(&binary, toml)?,
            None => Vec::new(),
        };
        let Some(toml) = toml.filter(|_| !accounts.is_empty()) else {
            if !p.enabled() {
                return Err(err(
                    3,
                    "step 2 (Himalaya): no Himalaya account with IMAP found; run `himalaya configure` or pass --himalaya-config",
                ));
            }
            if !p.confirm(
                "No Himalaya account with IMAP was found. Create one now with `himalaya configure`?",
                true,
            )? {
                return Err(err(
                    3,
                    "step 2 (Himalaya): no Himalaya account; run `himalaya configure`, then setup again",
                ));
            }
            configure(&binary, explicit.as_deref())?;
            continue;
        };
        let name = match &args.himalaya_account {
            Some(name) if accounts.iter().any(|a| &a.name == name) => name.clone(),
            Some(name) => {
                return Err(err(
                    2,
                    format!(
                        "--himalaya-account: {} has no IMAP account named {name}",
                        toml.display()
                    ),
                ))
            }
            None if !p.enabled() => {
                return Err(err(
                    2,
                    "step 2 (Himalaya): --himalaya-account is required without prompts",
                ))
            }
            None => {
                let mut options: Vec<String> = accounts.iter().map(|a| a.name.clone()).collect();
                options.push("Create a new account with `himalaya configure`".to_owned());
                let default = accounts.iter().position(|a| a.default).unwrap_or(0);
                let pick = p.choose(
                    "Which Himalaya account should mailtriage use?",
                    &options,
                    default,
                )?;
                if pick == accounts.len() {
                    configure(&binary, explicit.as_deref())?;
                    continue;
                }
                accounts[pick].name.clone()
            }
        };
        check_account(&binary, &toml, &name)?;
        let email = account_email(&toml, &name);
        return Ok(HimalayaChoice {
            binary,
            toml,
            account: name,
            email,
        });
    }
}

/// Himalaya v2.1.0's config search order without `--config` (verified on
/// macOS; Linux follows the `dirs` crate): the platform config dir, then
/// `~/.config`, then `~/.himalayarc`.
pub fn himalaya_config_candidates(home: &Path, xdg_config_home: Option<&Path>, macos: bool) -> Vec<PathBuf> {
    let platform = if macos {
        home.join("Library/Application Support")
    } else {
        xdg_config_home
            .filter(|p| p.is_absolute())
            .map_or_else(|| home.join(".config"), Path::to_path_buf)
    };
    let mut candidates = vec![platform.join("himalaya/config.toml")];
    let dot_config = home.join(".config/himalaya/config.toml");
    if !candidates.contains(&dot_config) {
        candidates.push(dot_config);
    }
    candidates.push(home.join(".himalayarc"));
    candidates
}

fn default_himalaya_config() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)?;
    let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
    himalaya_config_candidates(&home, xdg.as_deref(), cfg!(target_os = "macos"))
        .into_iter()
        .find(|candidate| candidate.is_file())
}

/// `HIMALAYA_CONFIG`, if set; several `:`-separated files are refused
/// because mailtriage passes exactly one `--config`.
fn himalaya_config_env() -> Result<Option<PathBuf>> {
    let Some(value) = std::env::var_os("HIMALAYA_CONFIG").filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let paths: Vec<PathBuf> = std::env::split_paths(&value).collect();
    if paths.len() != 1 {
        return Err(err(
            2,
            "step 2 (Himalaya): HIMALAYA_CONFIG names several files; pass --himalaya-config with the file that defines the account",
        ));
    }
    Ok(Some(absolute(&paths[0])?))
}

/// Runs Himalaya for setup's own checks; stdout when it exits 0.
fn run_himalaya(binary: &Path, args: &[&OsStr]) -> Result<Vec<u8>> {
    process::run_bounded(binary, args, HIMALAYA_TIMEOUT, HIMALAYA_MAX_OUTPUT)?
        .success()
        .ok_or_else(|| anyhow!("Himalaya command failed"))
}

fn list_accounts(binary: &Path, toml: &Path) -> Result<Vec<HimalayaAccount>> {
    let failed = || {
        err(
            3,
            format!(
                "step 2 (Himalaya): `{}` failed; check that file",
                shell_line(&[
                    binary.as_os_str(),
                    OsStr::new("--config"),
                    toml.as_os_str(),
                    OsStr::new("account"),
                    OsStr::new("list"),
                ])
            ),
        )
    };
    let out = run_himalaya(
        binary,
        &[
            OsStr::new("--config"),
            toml.as_os_str(),
            OsStr::new("--json"),
            OsStr::new("account"),
            OsStr::new("list"),
        ],
    )
    .map_err(|_| failed())?;
    let value: Value = serde_json::from_slice(&out).map_err(|_| failed())?;
    let rows = value["accounts"].as_array().ok_or_else(failed)?;
    Ok(rows
        .iter()
        .filter(|row| {
            row["backends"]
                .as_array()
                .is_some_and(|backends| backends.iter().any(|b| b == "imap"))
        })
        .filter_map(|row| {
            Some(HimalayaAccount {
                name: row["name"].as_str()?.to_owned(),
                default: row["default"] == true,
            })
        })
        .collect())
}

/// `account check` exits 0 even when the check fails, so its JSON decides.
fn check_account(binary: &Path, toml: &Path, name: &str) -> Result<()> {
    let ok = run_himalaya(
        binary,
        &[
            OsStr::new("--config"),
            toml.as_os_str(),
            OsStr::new("--account"),
            OsStr::new(name),
            OsStr::new("--backend"),
            OsStr::new("imap"),
            OsStr::new("--json"),
            OsStr::new("account"),
            OsStr::new("check"),
        ],
    )
    .ok()
    .and_then(|out| serde_json::from_slice::<Value>(&out).ok())
    .is_some_and(|report| {
        report["backends"].as_array().is_some_and(|backends| {
            backends
                .iter()
                .any(|b| b["backend"] == "imap" && b["ok"] == true)
        })
    });
    if ok {
        return Ok(());
    }
    Err(err(
        3,
        format!(
            "step 2 (Himalaya): account check failed for {name}; run `{}` to see why",
            shell_line(&[
                binary.as_os_str(),
                OsStr::new("--config"),
                toml.as_os_str(),
                OsStr::new("--account"),
                OsStr::new(name),
                OsStr::new("account"),
                OsStr::new("check"),
            ])
        ),
    ))
}

fn account_email(toml: &Path, name: &str) -> Option<String> {
    let text = fs::read_to_string(toml).ok()?;
    let value: toml::Value = toml::from_str(&text).ok()?;
    value
        .get("accounts")?
        .get(name)?
        .get("email")?
        .as_str()
        .map(str::to_owned)
}

/// Runs Himalaya's own wizard attached to the terminal.
fn configure(binary: &Path, toml: Option<&Path>) -> Result<()> {
    let mut command = Command::new(binary);
    if let Some(toml) = toml {
        command.arg("--config").arg(toml);
    }
    let status = command
        .arg("configure")
        .status()
        .map_err(|_| err(3, "step 2 (Himalaya): could not start `himalaya configure`"))?;
    if !status.success() {
        return Err(err(
            3,
            "step 2 (Himalaya): `himalaya configure` did not finish; run it yourself, then setup again",
        ));
    }
    Ok(())
}

/// Step 3's name. An account chosen for update keeps its name.
fn account_name(
    args: &SetupArgs,
    p: &mut Prompter,
    h: &HimalayaChoice,
    cfg: &AppConfig,
    intent: &Intent,
) -> Result<String> {
    if let Intent::Update(name) = intent {
        return Ok(name.clone());
    }
    let must_be_new = matches!(intent, Intent::Add);
    answer(
        p,
        args.account.as_deref(),
        "--account",
        "Name for this account in mailtriage",
        Some(&h.account),
        |name| {
            if !config::valid_account_name(name) {
                Err("Use 1 to 64 ASCII letters, digits, - or _.".to_owned())
            } else if must_be_new && cfg.accounts.contains_key(name) {
                Err(format!("An account named {name} already exists."))
            } else {
                Ok(name.to_owned())
            }
        },
    )
}

/// `TZ` (a zone name, not a path), else the zone `/etc/localtime` points
/// to, else `UTC`.
pub fn default_timezone(tz: Option<&str>, localtime: Option<&Path>) -> String {
    if let Some(zone) = tz
        .map(|t| t.trim().trim_start_matches(':'))
        .filter(|t| !t.is_empty() && !t.starts_with('/'))
    {
        return zone.to_owned();
    }
    localtime
        .and_then(Path::to_str)
        .and_then(|target| target.split_once("zoneinfo/"))
        .map(|(_, zone)| zone)
        .filter(|zone| !zone.is_empty())
        .unwrap_or("UTC")
        .to_owned()
}

/// Step 4: folders to watch, from the server's list.
fn folders_step(
    args: &SetupArgs,
    p: &mut Prompter,
    engine: &HimalayaConfig,
    previous: Option<&[String]>,
) -> Result<Vec<String>> {
    let failed = || {
        err(
            3,
            format!(
                "step 4 (folders): could not list the folders of {}; run `{}`",
                engine.account,
                shell_line(&[
                    engine.binary.as_os_str(),
                    OsStr::new("--config"),
                    engine.config.as_os_str(),
                    OsStr::new("--account"),
                    OsStr::new(&engine.account),
                    OsStr::new("imap"),
                    OsStr::new("list"),
                    OsStr::new("--all"),
                ])
            ),
        )
    };
    let folders: Vec<String> = engine::open(&EngineConfig::Himalaya(engine.clone()))
        .and_then(|e| e.list_folders())
        .map_err(|_| failed())?
        .into_iter()
        .filter(|f| !f.attributes.iter().any(|a| a.eq_ignore_ascii_case("\\Noselect")))
        .map(|f| f.name)
        .collect();
    if folders.is_empty() {
        return Err(failed());
    }
    if !args.mailboxes.is_empty() {
        let mut chosen: Vec<String> = Vec::new();
        for name in &args.mailboxes {
            if !folders.contains(name) {
                return Err(err(2, format!("--mailbox: no folder named {name} on the server")));
            }
            if !chosen.contains(name) {
                chosen.push(name.clone());
            }
        }
        return Ok(chosen);
    }
    let wanted = previous.map_or_else(|| vec!["INBOX".to_owned()], <[String]>::to_vec);
    let mut defaults: Vec<usize> = wanted
        .iter()
        .filter_map(|w| folders.iter().position(|f| f == w))
        .collect();
    if defaults.is_empty() {
        defaults.push(
            folders
                .iter()
                .position(|f| f.eq_ignore_ascii_case("INBOX"))
                .unwrap_or(0),
        );
    }
    if !p.enabled() {
        return Ok(defaults.iter().map(|&i| folders[i].clone()).collect());
    }
    let picked = p.choose_many("Which folders should mailtriage watch?", &folders, &defaults)?;
    Ok(picked.into_iter().map(|i| folders[i].clone()).collect())
}

/// Step 5: the classifier, and the key store when one was chosen now.
fn classifier_step(
    args: &SetupArgs,
    p: &mut Prompter,
    current: Option<&ProviderConfig>,
) -> Result<(ProviderConfig, Option<KeyStore>)> {
    let flags = args.provider.is_some()
        || args.model.is_some()
        || args.key_store.is_some()
        || args.key_command.is_some()
        || args.key_env.is_some()
        || args.key_stored;
    if let Some(current) = current.filter(|_| !flags) {
        let keep = !p.enabled()
            || p.confirm(
                &format!("Keep the current classifier ({})?", describe(current)),
                true,
            )?;
        if keep {
            return Ok((current.clone(), None));
        }
    }
    let kind = match args.provider.as_deref() {
        Some(kind) => kind.to_owned(),
        None if p.enabled() => {
            let options = [
                "OpenRouter (Jev decisions model; needs an API key)",
                "Offline demo (fake; keyword rules, no key)",
            ]
            .map(String::from);
            ["openrouter", "fake"][p.choose("Which classifier?", &options, 0)?].to_owned()
        }
        None => "openrouter".to_owned(),
    };
    if kind == "fake" {
        return Ok((config::default_config().provider, None));
    }
    let model = answer(p, args.model.as_deref(), "--model", "Model", Some(DEFAULT_MODEL), |m| {
        if m.starts_with("typesafe/jev-") || m.starts_with("~typesafe/jev-") {
            Ok(m.to_owned())
        } else {
            Err("Use a Jev decisions model such as typesafe/jev-1.13.".to_owned())
        }
    })?;
    let api_key_env = current
        .filter(|c| c.kind == "openrouter" && !c.api_key_env.is_empty())
        .map_or_else(|| DEFAULT_KEY_ENV.to_owned(), |c| c.api_key_env.clone());
    let mut provider = ProviderConfig {
        kind,
        model,
        endpoint: provider::DECISIONS_ENDPOINT.to_owned(),
        api_key_command: None,
        api_key_env,
        timeout_seconds: 30,
    };
    let store = key_step(args, p, &mut provider)?;
    Ok((provider, Some(store)))
}

fn describe(provider: &ProviderConfig) -> String {
    match (provider.kind.as_str(), &provider.api_key_command) {
        ("fake", _) => "offline demo".to_owned(),
        (_, Some(_)) => format!("{}, key from a key command", provider.model),
        (_, None) => format!("{}, key from ${}", provider.model, provider.api_key_env),
    }
}

/// The store from `--key-store`, implied by `--key-command`/`--key-env`,
/// or chosen from the menu (first option without prompts).
fn chosen_store(args: &SetupArgs, p: &mut Prompter) -> Result<KeyStore> {
    let implied = match (args.key_command.is_some(), args.key_env.is_some()) {
        (true, true) => return Err(err(2, "--key-command and --key-env exclude each other")),
        (true, false) => Some(KeyStore::Command),
        (false, true) => Some(KeyStore::Env),
        (false, false) => None,
    };
    match (args.key_store, implied) {
        (Some(store), Some(other)) if store != other => Err(err(
            2,
            format!(
                "--key-store {} conflicts with {}",
                store.flag(),
                if other == KeyStore::Command { "--key-command" } else { "--key-env" }
            ),
        )),
        (Some(store), _) | (None, Some(store)) => Ok(store),
        (None, None) => {
            let options = secrets::key_store_options(cfg!(target_os = "macos"), |tool| {
                process::find_on_path(tool).is_some()
            });
            if !p.enabled() {
                return Ok(options[0]);
            }
            let labels: Vec<String> = options.iter().map(|o| o.label().to_owned()).collect();
            Ok(options[p.choose("Where should mailtriage get the OpenRouter key?", &labels, 0)?])
        }
    }
}

/// How the provider gets the key. Setup never reads the key except to
/// confirm that a command prints one.
fn key_step(args: &SetupArgs, p: &mut Prompter, provider: &mut ProviderConfig) -> Result<KeyStore> {
    let store = chosen_store(args, p)?;
    match store {
        KeyStore::Command => loop {
            let text = answer(
                p,
                args.key_command.as_deref(),
                "--key-command",
                "Command that prints the key (run with /bin/sh -c)",
                None,
                nonempty,
            )?;
            let command = vec!["/bin/sh".to_owned(), "-c".to_owned(), text];
            match secrets::run_key_command(&command) {
                Ok(_) => {
                    p.say("The key command printed a key.");
                    provider.api_key_command = Some(command);
                    break;
                }
                Err(e) if args.key_command.is_none() && p.enabled() => {
                    p.say(&format!("  {e}. Try another command."))
                }
                Err(e) => return Err(err(3, format!("step 5 (key): {e}; check --key-command"))),
            }
        },
        KeyStore::Env => {
            let name = answer(
                p,
                args.key_env.as_deref(),
                "--key-env",
                "Environment variable that holds the key",
                Some(DEFAULT_KEY_ENV),
                |n| {
                    if provider::valid_env_name(n) {
                        Ok(n.to_owned())
                    } else {
                        Err("Use upper-case letters, digits and _.".to_owned())
                    }
                },
            )?;
            if !std::env::var(&name).is_ok_and(|v| !v.trim().is_empty()) {
                p.say(&format!("{name} is not set here; set it wherever mailtriage runs."));
            }
            provider.api_key_env = name;
        }
    }
    Ok(store)
}

fn default_categories() -> Vec<Category> {
    let mut categories = config::default_config()
        .accounts
        .remove("work")
        .map(|a| a.categories)
        .unwrap_or_default();
    for category in &mut categories {
        category.folder = Some(category.name.clone());
    }
    categories
}

fn categories_hint(name: &str, categories: &[Category]) -> String {
    let names: Vec<&str> = categories.iter().map(|c| c.name.as_str()).collect();
    format!(
        "Categories: {}. Each files into a folder of the same name. To change them: `mailtriage categories export --account {name} > categories.json`, edit the file, `mailtriage categories validate --account {name} --file categories.json`, then `mailtriage categories apply --account {name} --file categories.json`.",
        names.join(", ")
    )
}

/// Step 7: `off` or `dry_run`; a `live` account stays live. Filing needs
/// source folders it can name safely.
fn filing_step(
    args: &SetupArgs,
    p: &mut Prompter,
    previous: Option<FilingMode>,
    mailboxes: &[String],
) -> Result<FilingMode> {
    let blocked = mailboxes.iter().find(|m| config::unsafe_source_mailbox(m));
    let refuse = |mode: FilingMode| -> Result<()> {
        match blocked {
            Some(m) if mode != FilingMode::Off => Err(err(
                2,
                format!(
                    "step 7 (filing): folder {m:?} must be {} while filing is on; watch other folders or pass --filing off",
                    config::SOURCE_RULE
                ),
            )),
            _ => Ok(()),
        }
    };
    if let Some(mode) = args.filing {
        refuse(mode)?;
        return Ok(mode);
    }
    if previous == Some(FilingMode::Live) {
        refuse(FilingMode::Live)?;
        p.say("Filing stays live.");
        return Ok(FilingMode::Live);
    }
    let default = previous.unwrap_or(FilingMode::DryRun);
    if !p.enabled() {
        refuse(default)?;
        return Ok(default);
    }
    let options = [
        "Dry run: plan the moves and show them, change nothing (recommended)",
        "Off: classify only",
    ]
    .map(String::from);
    loop {
        let pick = p.choose(
            "File mail into one IMAP folder per category?",
            &options,
            usize::from(default == FilingMode::Off),
        )?;
        let mode = [FilingMode::DryRun, FilingMode::Off][pick];
        match refuse(mode) {
            Ok(()) => return Ok(mode),
            Err(e) => p.say(&format!("  {}", error_text(&e))),
        }
    }
}

/// Step 9: `doctor`, each item with the one command that fixes it.
fn doctor_step(
    p: &mut Prompter,
    path: &Path,
    name: &str,
    provider: &ProviderConfig,
    engine: &HimalayaConfig,
) -> Value {
    let mut items = Vec::new();
    match Service::open(path).and_then(|mut service| service.doctor(name)) {
        Err(e) => items.push(check_item(
            "state",
            false,
            Some(error_text(&e)),
            format!("check {} or choose another --account", path.display()),
        )),
        Ok(report) => {
            let key = &report["provider"];
            items.push(check_item(
                "provider",
                key["configuration_valid"] == true,
                None,
                format!("check the provider block in {}", path.display()),
            ));
            let key_fix = if key["key_source"] == "command" {
                "run `mailtriage setup --update` and store the key again".to_owned()
            } else {
                format!(
                    "export {}=<your OpenRouter key> where mailtriage runs",
                    provider.api_key_env
                )
            };
            items.push(check_item(
                "key",
                key["key_present"] == true,
                key["key_error"].as_str().map(str::to_owned),
                key_fix,
            ));
            items.push(check_item(
                "mail",
                report["transport"]["ready"] == true,
                None,
                format!(
                    "run `{}`",
                    shell_line(&[
                        engine.binary.as_os_str(),
                        OsStr::new("--config"),
                        engine.config.as_os_str(),
                        OsStr::new("--account"),
                        OsStr::new(&engine.account),
                        OsStr::new("account"),
                        OsStr::new("check"),
                    ])
                ),
            ));
            if let Some(filing) = report.get("filing") {
                let problems = filing["problems"].as_array().map_or(0, Vec::len);
                items.push(check_item(
                    "filing",
                    problems == 0,
                    None,
                    format!("run `mailtriage filing status --account {name}`"),
                ));
            }
        }
    }
    p.say("Checks:");
    for item in &items {
        let check = item["check"].as_str().unwrap_or_default();
        match item["fix"].as_str() {
            None => p.say(&format!("  ok         {check}")),
            Some(fix) => p.say(&format!("  not ready  {check}: {fix}")),
        }
    }
    json!({"ready": items.iter().all(|i| i["ready"] == true), "items": items})
}

/// One doctor item; `error` and `fix` only when it is not ready.
fn check_item(check: &str, ready: bool, error: Option<String>, fix: String) -> Value {
    let mut item = json!({"check": check, "ready": ready});
    if !ready {
        if let Some(error) = error {
            item["error"] = json!(error);
        }
        item["fix"] = json!(fix);
    }
    item
}

fn next_steps(p: &mut Prompter, name: &str, mode: FilingMode) {
    p.say("");
    p.say(&format!(
        "Next: `mailtriage sync --account {name}` classifies new mail; `mailtriage watch --account {name}` keeps doing it."
    ));
    if mode == FilingMode::DryRun {
        p.say(&format!(
            "Filing is a dry run: `mailtriage filing plan --account {name}` shows what would move."
        ));
        p.say(&format!(
            "Go live only after the provider checklist (docs/development/verification.md): `mailtriage filing enable --account {name} --mode live`."
        ));
    }
}

fn key_source_value(provider: &ProviderConfig) -> Value {
    if provider.kind == "fake" {
        Value::Null
    } else {
        json!(secrets::key_source(provider))
    }
}

/// A flag's value; else the prompt's answer; else the default; without
/// prompts and without a default, exit 2 naming the flag.
fn answer(
    p: &mut Prompter,
    flag: Option<&str>,
    flag_name: &str,
    question: &str,
    default: Option<&str>,
    check: impl Fn(&str) -> Result<String, String>,
) -> Result<String> {
    if let Some(value) = flag {
        return check(value).map_err(|reason| err(2, format!("{flag_name}: {reason}")));
    }
    if p.enabled() {
        return p.ask(question, default, check);
    }
    match default {
        Some(value) => check(value).map_err(|reason| err(2, format!("{flag_name}: {reason}"))),
        None => Err(err(2, format!("{flag_name} is required without prompts"))),
    }
}

fn nonempty(value: &str) -> Result<String, String> {
    if value.trim().is_empty() {
        Err("This cannot be empty.".to_owned())
    } else {
        Ok(value.trim().to_owned())
    }
}

fn absolute(path: &Path) -> Result<PathBuf> {
    std::path::absolute(path).map_err(|_| err(2, format!("cannot resolve {}", path.display())))
}

fn error_text(error: &anyhow::Error) -> String {
    error
        .downcast_ref::<ServiceError>()
        .map_or_else(|| "operation failed".to_owned(), |e| e.message.clone())
}

/// A command line for messages, quoting words the shell would split.
fn shell_line<S: AsRef<OsStr>>(words: &[S]) -> String {
    words
        .iter()
        .map(|word| {
            let word = word.as_ref().to_string_lossy();
            if !word.is_empty()
                && word
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./=:@+,".contains(c))
            {
                word.into_owned()
            } else {
                format!("'{}'", word.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
```

Add `pub mod setup;` to `src/lib.rs`.

- [ ] **Step 5: CLI `setup`**

In `src/cli.rs` add `use std::io::IsTerminal;`, `use mailtriage::{prompt::{self, Prompter}, secrets::KeyStore, setup}` and the subcommand `/// Guided first run: mail account, folders, classifier and key, filing. Setup(SetupArg),`:

```rust
#[derive(Args)]
struct SetupArg {
    /// Change an existing config (required without prompts).
    #[arg(long)]
    update: bool,
    /// No prompts: flags and defaults only.
    #[arg(long, conflicts_with = "interactive")]
    yes: bool,
    /// Prompt even when stdin is not a terminal.
    #[arg(long)]
    interactive: bool,
    #[arg(long)]
    himalaya_binary: Option<PathBuf>,
    #[arg(long)]
    himalaya_config: Option<PathBuf>,
    #[arg(long)]
    himalaya_account: Option<String>,
    /// Account name in mailtriage (letters, digits, - and _).
    #[arg(long)]
    account: Option<String>,
    #[arg(long)]
    identity: Option<String>,
    #[arg(long)]
    timezone: Option<String>,
    /// One line about the recipient that helps classification.
    #[arg(long)]
    brief: Option<String>,
    /// A folder to watch; repeat for several.
    #[arg(long = "mailbox")]
    mailboxes: Vec<String>,
    #[arg(long, value_parser = ["openrouter", "fake"])]
    provider: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long, value_parser = ["command", "env"])]
    key_store: Option<String>,
    /// Shell command that prints the key (stored as /bin/sh -c).
    #[arg(long)]
    key_command: Option<String>,
    /// Environment variable that holds the key.
    #[arg(long)]
    key_env: Option<String>,
    /// The key is already in the chosen store.
    #[arg(long)]
    key_stored: bool,
    #[arg(long, value_parser = ["off", "dry-run"])]
    filing: Option<String>,
}

impl SetupArg {
    fn to_args(&self, terminal: bool) -> setup::SetupArgs {
        setup::SetupArgs {
            update: self.update,
            terminal,
            himalaya_binary: self.himalaya_binary.clone(),
            himalaya_config: self.himalaya_config.clone(),
            himalaya_account: self.himalaya_account.clone(),
            account: self.account.clone(),
            identity: self.identity.clone(),
            timezone: self.timezone.clone(),
            brief: self.brief.clone(),
            mailboxes: self.mailboxes.clone(),
            provider: self.provider.clone(),
            model: self.model.clone(),
            key_store: self.key_store.as_deref().and_then(KeyStore::from_flag),
            key_command: self.key_command.clone(),
            key_env: self.key_env.clone(),
            key_stored: self.key_stored,
            filing: self.filing.as_deref().map(|f| {
                if f == "off" {
                    FilingMode::Off
                } else {
                    FilingMode::DryRun
                }
            }),
        }
    }
}

fn setup(cli: &Cli, arg: &SetupArg) -> Result<Value, CliError> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let path = config::setup_path(
        cli.config.as_deref(),
        std::env::var_os("MAILTRIAGE_CONFIG").as_deref(),
        home.as_deref(),
    )
    .map_err(|e| CliError::input(e.to_string()))?;
    let terminal = io::stdin().is_terminal();
    let mut prompt = Prompter::new(
        prompt::stdin_unbuffered(),
        io::stderr(),
        !arg.yes && (arg.interactive || terminal),
    );
    setup::run(&arg.to_args(terminal), &path, &mut prompt).map_err(service_error)
}
```

In `execute`: `Command::Setup(arg) => setup(cli, arg),`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --test setup && cargo test --lib setup`
Expected: all PASS.

- [ ] **Step 7: Full check and commit**

Run: `cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`

```bash
git add src/setup.rs src/secrets.rs src/config.rs src/engine/himalaya.rs src/cli.rs src/lib.rs tests/setup.rs
git commit -m "Add mailtriage setup for a guided first run

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Keychain, Secret Service and pass

**Files:**
- Modify:
  - `src/secrets.rs` (`KeyStore` variants, `tool`, `key_store_options`, `read_command`, `store_command`, unit tests)
  - `src/setup.rs` (`key_step` tool arm; replace the Task 3 unit test `key_store_options_without_tool_stores`)
  - `src/cli.rs` (`--key-store` value parser)
  - `tests/setup.rs` (fake key tools and tests)

**Interfaces:**
- Consumes:
  - Task 3: `KeyStore`, `chosen_store`, `key_step`, `shell_line`, `SetupArgs.{terminal, key_stored}`.
  - Task 1: `secrets::run_key_command`, `process::find_on_path`.
- Produces:
  - `KeyStore::{Keychain, SecretService, Pass, Command, Env}`, whose flags are `keychain`, `secret-service`, `pass`, `command` and `env`;
  - `KeyStore::tool() -> Option<&'static str>`;
  - `secrets::read_command(KeyStore, tool: &Path) -> Option<Vec<String>>`;
  - `secrets::store_command(KeyStore, tool: &Path) -> Option<Vec<String>>`.

- [ ] **Step 1: Write the failing tests**

Unit tests in `src/secrets.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_platform_store_comes_first() {
        assert_eq!(
            key_store_options(true, |_| false),
            vec![KeyStore::Keychain, KeyStore::Command, KeyStore::Env]
        );
        assert_eq!(
            key_store_options(false, |_| true),
            vec![KeyStore::SecretService, KeyStore::Pass, KeyStore::Command, KeyStore::Env]
        );
        assert_eq!(
            key_store_options(false, |tool| tool == "pass"),
            vec![KeyStore::Pass, KeyStore::Command, KeyStore::Env]
        );
        assert_eq!(key_store_options(false, |_| false), vec![KeyStore::Command, KeyStore::Env]);
        for store in KeyStore::ALL {
            assert_eq!(KeyStore::from_flag(store.flag()), Some(store));
        }
    }

    #[test]
    fn read_and_store_commands_use_the_tool_path() {
        let tool = Path::new("/usr/bin/security");
        assert_eq!(
            read_command(KeyStore::Keychain, tool).unwrap(),
            ["/usr/bin/security", "find-generic-password", "-s", "mailtriage", "-a", "openrouter", "-w"]
        );
        assert_eq!(
            store_command(KeyStore::Keychain, tool).unwrap(),
            ["/usr/bin/security", "add-generic-password", "-U", "-s", "mailtriage", "-a", "openrouter", "-w"]
        );
        let tool = Path::new("/usr/bin/secret-tool");
        assert_eq!(
            read_command(KeyStore::SecretService, tool).unwrap(),
            ["/usr/bin/secret-tool", "lookup", "service", "mailtriage", "provider", "openrouter"]
        );
        assert_eq!(
            store_command(KeyStore::SecretService, tool).unwrap(),
            ["/usr/bin/secret-tool", "store", "--label=mailtriage OpenRouter key", "service", "mailtriage", "provider", "openrouter"]
        );
        let tool = Path::new("/usr/bin/pass");
        assert_eq!(read_command(KeyStore::Pass, tool).unwrap(), ["/usr/bin/pass", "show", "mailtriage/openrouter"]);
        assert_eq!(store_command(KeyStore::Pass, tool).unwrap(), ["/usr/bin/pass", "insert", "mailtriage/openrouter"]);
        assert_eq!(read_command(KeyStore::Env, tool), None);
        assert_eq!(store_command(KeyStore::Command, tool), None);
    }
}
```

In `src/setup.rs` tests, delete `key_store_options_without_tool_stores` (now covered in `secrets`).

Add to `tests/setup.rs` (`use mailtriage::secrets;` at the top):

```rust
/// A fake key tool. The store verb reads one line from stdin, as the real
/// tools read the key from the terminal; the read verb prints it, or exits
/// 44 when nothing is stored. Calls are logged to `tools.log`.
fn key_tool(store_verb: &str, read_verb: &str, file: &str) -> String {
    format!(
        "#!/bin/sh\ndir=\"$(dirname \"$0\")\"\necho \"$*\" >> \"$dir/tools.log\"\ncase \"$1\" in\n  {store_verb}) IFS= read -r key || exit 1; printf '%s\\n' \"$key\" > \"$dir/{file}\" ;;\n  {read_verb}) [ -f \"$dir/{file}\" ] && cat \"$dir/{file}\" || exit 44 ;;\n  *) exit 64 ;;\nesac\n"
    )
}

impl Fixture {
    fn add_key_tools(&self) {
        write_tool(&self.bin, "security", &key_tool("add-generic-password", "find-generic-password", "keychain.key"));
        write_tool(&self.bin, "secret-tool", &key_tool("store", "lookup", "secret-service.key"));
        write_tool(&self.bin, "pass", &key_tool("insert", "show", "pass.key"));
    }

    fn stored(&self, file: &str) -> Option<String> {
        fs::read_to_string(self.bin.join(file)).ok().map(|s| s.trim().to_owned())
    }

    fn tool_calls(&self) -> String {
        fs::read_to_string(self.bin.join("tools.log")).unwrap_or_default()
    }
}

#[test]
fn the_store_tool_asks_for_the_key_on_the_shared_stdin() {
    let f = Fixture::new();
    f.add_key_tools();
    // Defaults for account, name, identity, time zone, brief, folders,
    // provider and model; the line secret-tool reads; then filing.
    let input = format!("{}sk-or-stored-1\n\n", "\n".repeat(8));
    let (out, v) = f.run(&["setup", "--interactive", "--json", "--key-store", "secret-service"], &input);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(f.stored("secret-service.key").as_deref(), Some("sk-or-stored-1"));
    assert_eq!(
        f.config()["provider"]["api_key_command"],
        json!([f.bin.join("secret-tool").to_str().unwrap(), "lookup", "service", "mailtriage", "provider", "openrouter"])
    );
    assert_eq!(v["setup"]["key_store"], "secret-service");
    assert_eq!(v["setup"]["doctor"]["ready"], true, "{}", v["setup"]["doctor"]);
    assert!(!stdout(&out).contains("sk-or-stored-1"));
    assert!(!stderr(&out).contains("sk-or-stored-1"));
}

#[test]
fn a_stored_key_is_reused_without_prompts() {
    let f = Fixture::new();
    f.add_key_tools();
    fs::write(f.bin.join("pass.key"), "sk-or-old\n").unwrap();
    let (out, v) = f.run(
        &["setup", "--yes", "--json", "--himalaya-account", "work", "--key-store", "pass"],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(v["setup"]["key_store"], "pass");
    assert!(!f.tool_calls().contains("insert"));
    assert_eq!(
        f.config()["provider"]["api_key_command"],
        json!([f.bin.join("pass").to_str().unwrap(), "show", "mailtriage/openrouter"])
    );
}

#[test]
fn without_a_terminal_the_store_step_needs_key_stored() {
    let f = Fixture::new();
    f.add_key_tools();
    let base = ["setup", "--yes", "--json", "--himalaya-account", "work", "--key-store", "pass"];
    let (out, v) = f.run(&base, "");
    assert_eq!(out.status.code(), Some(2));
    assert!(message(&v).contains("--key-stored"), "{v}");
    let (out, v) = f.run(&[&base[..], &["--key-stored"]].concat(), "");
    assert_eq!(out.status.code(), Some(3));
    assert!(message(&v).contains("no key found"), "{v}");
    assert!(!f.config_path().exists());
    assert!(!f.tool_calls().contains("insert"));
}

#[test]
fn a_stored_key_can_be_replaced_when_prompting() {
    let f = Fixture::new();
    f.add_key_tools();
    fs::write(f.bin.join("pass.key"), "sk-or-old\n").unwrap();
    // Eight defaults, decline reuse, the new key for pass, then filing.
    let input = format!("{}n\nsk-or-new\n\n", "\n".repeat(8));
    let (out, _) = f.run(&["setup", "--interactive", "--json", "--key-store", "pass"], &input);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(f.stored("pass.key").as_deref(), Some("sk-or-new"));
}

#[test]
fn the_key_store_menu_starts_with_the_platform_default() {
    let f = Fixture::new();
    f.add_key_tools();
    let options = secrets::key_store_options(cfg!(target_os = "macos"), |tool| f.bin.join(tool).exists());
    // Eight defaults, the last menu entry (env), its variable, then filing.
    let input = format!("{}{}\n\n\n", "\n".repeat(8), options.len());
    let (out, v) = f.run_with(
        &["setup", "--interactive", "--json"],
        &input,
        &[("OPENROUTER_API_KEY", "sk-or-env")],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let first = if cfg!(target_os = "macos") { "keychain" } else { "secret-service" };
    assert_eq!(options[0].flag(), first);
    assert!(stderr(&out).contains(&format!("  1) {} (default)", options[0].label())), "{}", stderr(&out));
    assert_eq!(v["setup"]["key_store"], "env");
}

#[test]
#[cfg(target_os = "macos")]
fn keychain_is_the_default_on_macos() {
    let f = Fixture::new();
    f.add_key_tools();
    fs::write(f.bin.join("keychain.key"), "sk-or-keychain\n").unwrap();
    let (out, v) = f.run(&["setup", "--yes", "--json", "--himalaya-account", "work"], "");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(v["setup"]["key_store"], "keychain");
    assert_eq!(
        f.config()["provider"]["api_key_command"],
        json!([f.bin.join("security").to_str().unwrap(), "find-generic-password", "-s", "mailtriage", "-a", "openrouter", "-w"])
    );
}

#[test]
#[cfg(target_os = "linux")]
fn secret_service_is_the_default_on_linux_when_available() {
    let f = Fixture::new();
    f.add_key_tools();
    fs::write(f.bin.join("secret-service.key"), "sk-or-linux\n").unwrap();
    let (out, v) = f.run(&["setup", "--yes", "--json", "--himalaya-account", "work"], "");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(v["setup"]["key_store"], "secret-service");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test setup && cargo test --lib secrets`
Expected: compile errors (`KeyStore::Keychain`, `read_command` missing).

- [ ] **Step 3: `src/secrets.rs`**

Replace the Task 3 `KeyStore` block with:

```rust
/// Where setup keeps the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyStore {
    Keychain,
    SecretService,
    Pass,
    Command,
    Env,
}

impl KeyStore {
    pub const ALL: [KeyStore; 5] = [
        KeyStore::Keychain,
        KeyStore::SecretService,
        KeyStore::Pass,
        KeyStore::Command,
        KeyStore::Env,
    ];

    /// The `--key-store` value.
    pub fn flag(self) -> &'static str {
        match self {
            KeyStore::Keychain => "keychain",
            KeyStore::SecretService => "secret-service",
            KeyStore::Pass => "pass",
            KeyStore::Command => "command",
            KeyStore::Env => "env",
        }
    }

    pub fn from_flag(flag: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|store| store.flag() == flag)
    }

    /// The menu text.
    pub fn label(self) -> &'static str {
        match self {
            KeyStore::Keychain => "macOS Keychain",
            KeyStore::SecretService => "Secret Service (secret-tool)",
            KeyStore::Pass => "pass (the standard Unix password manager)",
            KeyStore::Command => "A command that prints the key",
            KeyStore::Env => "An environment variable only",
        }
    }

    /// The program behind a tool-backed store.
    pub fn tool(self) -> Option<&'static str> {
        match self {
            KeyStore::Keychain => Some("security"),
            KeyStore::SecretService => Some("secret-tool"),
            KeyStore::Pass => Some("pass"),
            KeyStore::Command | KeyStore::Env => None,
        }
    }
}

/// The stores setup offers, the platform's own first: the Keychain on
/// macOS; Secret Service and `pass` when their tools are on `PATH`; a
/// command and an environment variable always.
pub fn key_store_options(macos: bool, has_tool: impl Fn(&str) -> bool) -> Vec<KeyStore> {
    let mut options = Vec::new();
    if macos {
        options.push(KeyStore::Keychain);
    }
    for store in [KeyStore::SecretService, KeyStore::Pass] {
        if store.tool().is_some_and(&has_tool) {
            options.push(store);
        }
    }
    options.extend([KeyStore::Command, KeyStore::Env]);
    options
}

/// The command that prints the stored key, with the tool's absolute path.
pub fn read_command(store: KeyStore, tool: &Path) -> Option<Vec<String>> {
    let args: &[&str] = match store {
        KeyStore::Keychain => &["find-generic-password", "-s", "mailtriage", "-a", "openrouter", "-w"],
        KeyStore::SecretService => &["lookup", "service", "mailtriage", "provider", "openrouter"],
        KeyStore::Pass => &["show", "mailtriage/openrouter"],
        KeyStore::Command | KeyStore::Env => return None,
    };
    Some(with_tool(tool, args))
}

/// The command that stores the key; the tool asks for it on the terminal.
pub fn store_command(store: KeyStore, tool: &Path) -> Option<Vec<String>> {
    let args: &[&str] = match store {
        KeyStore::Keychain => &["add-generic-password", "-U", "-s", "mailtriage", "-a", "openrouter", "-w"],
        KeyStore::SecretService => &[
            "store",
            "--label=mailtriage OpenRouter key",
            "service",
            "mailtriage",
            "provider",
            "openrouter",
        ],
        KeyStore::Pass => &["insert", "mailtriage/openrouter"],
        KeyStore::Command | KeyStore::Env => return None,
    };
    Some(with_tool(tool, args))
}

fn with_tool(tool: &Path, args: &[&str]) -> Vec<String> {
    std::iter::once(tool.display().to_string())
        .chain(args.iter().map(|arg| (*arg).to_owned()))
        .collect()
}
```

- [ ] **Step 4: `src/setup.rs` tool arm**

Add `use std::process::Stdio;` and this arm to `key_step`'s `match store` (before `KeyStore::Command`):

```rust
        KeyStore::Keychain | KeyStore::SecretService | KeyStore::Pass => {
            let tool_name = store.tool().expect("tool-backed store");
            let tool = process::find_on_path(tool_name).ok_or_else(|| {
                err(2, format!("--key-store {}: {tool_name} is not on PATH", store.flag()))
            })?;
            let read = secrets::read_command(store, &tool).expect("tool-backed store");
            let save = secrets::store_command(store, &tool).expect("tool-backed store");
            let stored = secrets::run_key_command(&read).is_ok();
            let reuse = stored
                && (args.key_stored
                    || !p.enabled()
                    || p.confirm(
                        &format!("A key is already stored in {}. Use it?", store.label()),
                        true,
                    )?);
            if !reuse {
                if args.key_stored {
                    return Err(err(
                        3,
                        format!(
                            "step 5 (key): no key found in {}; store it with `{}`",
                            store.label(),
                            shell_line(&save)
                        ),
                    ));
                }
                if !p.enabled() && !args.terminal {
                    return Err(err(
                        2,
                        format!(
                            "step 5 (key): storing the key needs a terminal; run `{}`, then pass --key-stored, or use --key-store env",
                            shell_line(&save)
                        ),
                    ));
                }
                p.say(&format!(
                    "{tool_name} will now ask for your OpenRouter key; mailtriage never sees it."
                ));
                // The tool prompts on the terminal; its stdout is not shown.
                let status = Command::new(&save[0])
                    .args(&save[1..])
                    .stdout(Stdio::null())
                    .status();
                if !status.is_ok_and(|s| s.success()) {
                    return Err(err(3, format!("step 5 (key): `{}` failed", shell_line(&save))));
                }
                secrets::run_key_command(&read).map_err(|e| {
                    err(
                        3,
                        format!(
                            "step 5 (key): {e} after storing it; check `{}`",
                            shell_line(&read)
                        ),
                    )
                })?;
            }
            p.say(&format!("The key is in {}.", store.label()));
            provider.api_key_command = Some(read);
        }
```

`src/cli.rs`: `--key-store` becomes `#[arg(long, value_parser = ["keychain", "secret-service", "pass", "command", "env"])]`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --test setup && cargo test --lib secrets && cargo test --lib setup`
Expected: all PASS (the macOS-only or Linux-only test runs on its platform).

- [ ] **Step 6: Full check and commit**

Run: `cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`

```bash
git add src/secrets.rs src/setup.rs src/cli.rs tests/setup.rs
git commit -m "Store the OpenRouter key in the Keychain, Secret Service or pass

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Heartbeat and background service

**Files:**
- Create: `src/system_service.rs`, `tests/system_service.rs`, `tests/heartbeat.rs`
- Modify:
  - `src/store.rs` (migration 5, version guard, two methods)
  - `src/service.rs` (`sync` wrapper, `exit_code`)
  - `src/setup.rs` (step 10, `SetupArgs` fields)
  - `src/cli.rs` (`service` commands, setup flags)
  - `src/lib.rs`
  - `tests/common/mod.rs` (fake tools)
  - `tests/setup.rs` (`--service skip` default for prompted runs, service tests)
  - `tests/filing_store.rs:365` (5 → 6)

**Interfaces:**
- Consumes:
  - `Service::{open, sync}`, `Service.config`, `Service.store`;
  - `config::valid_account_name`;
  - `process::{run_bounded, find_on_path}`;
  - `filing::mode_str`;
  - Task 3 `setup::run`, `Prompter`.
- Produces:
  - Store:
    - `Store::record_heartbeat(&self, account: &str, partial: bool, exit_code: i32, mode: &str) -> Result<()>`;
    - `Store::heartbeat(&self, account: &str) -> Result<Option<Value>>`, returning `{finished_at, partial, exit_code, mode}`.
  - `service::exit_code(&anyhow::Error) -> i32`.
  - `system_service`:
    - `Manager::{Launchd, Systemd}` with `name()`;
    - `Context { manager, home, tool, uid }` with `detect()` and `unit_path(account)`;
    - `Unit { account, exe, config, interval_seconds, limit, log_dir, path_env }` with `arguments()` and `log_paths()`;
    - `label`, `unit_name`, `plist`, `systemd_unit`;
    - `install(&Context, &Unit)`, `uninstall(&Context, account)`, `status(Option<&Context>, account, log_dir)`;
    - `unit_for(config_path, account, interval, limit)`;
    - `install_account(&Service, config_path, account, interval, limit, &Context)`;
    - `status_account(&Service, config_path, account, Option<&Context>)`.
  - Setup flags `--service install|skip`, `--interval-seconds`, `--limit`, and `setup.service` in the result.
  - CLI `mailtriage service install|uninstall|status`.

- [ ] **Step 1: Write the failing tests**

Append to `tests/common/mod.rs`:

```rust
/// Fake `launchctl`: `bootstrap` loads, `bootout` unloads (exit 3 when not
/// loaded), `print` describes a loaded job with tab-indented properties, as
/// the real tool does. Calls go to `launchctl.log` next to the script.
pub const LAUNCHCTL: &str = "#!/bin/sh\ndir=\"$(dirname \"$0\")\"\necho \"$*\" >> \"$dir/launchctl.log\"\ncase \"$1\" in\n  bootstrap) touch \"$dir/loaded\" ;;\n  bootout) [ -f \"$dir/loaded\" ] || exit 3; rm -f \"$dir/loaded\" ;;\n  print) [ -f \"$dir/loaded\" ] || exit 113; printf '%s = {\\n\\tstate = running\\n\\tpid = 4242\\n\\tlast exit code = 0\\n\\tendpoints = {\\n\\t\\tstate = active\\n\\t}\\n}\\n' \"$2\" ;;\n  *) exit 64 ;;\nesac\n";

/// Fake `systemctl --user`: `enable`, `restart`, `disable --now`, `show`.
pub const SYSTEMCTL: &str = "#!/bin/sh\ndir=\"$(dirname \"$0\")\"\necho \"$*\" >> \"$dir/systemctl.log\"\ncase \"$2\" in\n  daemon-reload) ;;\n  enable) touch \"$dir/enabled\" ;;\n  restart) touch \"$dir/active\" ;;\n  disable) rm -f \"$dir/enabled\" \"$dir/active\" ;;\n  show) if [ -f \"$dir/active\" ]; then printf 'LoadState=loaded\\nActiveState=active\\nSubState=running\\nMainPID=4343\\nExecMainStatus=0\\n'; else printf 'LoadState=not-found\\nActiveState=inactive\\nSubState=dead\\nMainPID=0\\nExecMainStatus=0\\n'; fi ;;\n  *) exit 64 ;;\nesac\n";

/// Writes an executable script.
pub fn write_tool(dir: &std::path::Path, name: &str, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}
```

(`tests/common/mod.rs` is compiled into non-unix test crates only if those exist. All current test crates run on unix CI; keep `write_tool` unix-only by adding `#[cfg(unix)]` to it.)

`tests/filing_store.rs` `newer_schema_is_rejected`: `.pragma_update(None, "user_version", 6)`.

Create `tests/heartbeat.rs`:

```rust
//! Every sync pass that holds the account lock records how it ended.
mod common;
use common::Harness;
use mailtriage::{domain::FilingMode, store::Store};

#[test]
fn each_pass_records_a_heartbeat() {
    let h = Harness::new(FilingMode::DryRun);
    assert!(h.service().store.heartbeat("work").unwrap().is_none());
    h.sync();
    let beat = h.service().store.heartbeat("work").unwrap().unwrap();
    assert_eq!(beat["exit_code"], 0);
    assert_eq!(beat["partial"], false);
    assert_eq!(beat["mode"], "dry_run");
    assert!(chrono::DateTime::parse_from_rfc3339(beat["finished_at"].as_str().unwrap()).is_ok());

    h.fake.fail_with_config_changed("version");
    assert!(h.service().sync("work", 100).is_err());
    let beat = h.service().store.heartbeat("work").unwrap().unwrap();
    assert_eq!(beat["exit_code"], 5);
    assert_eq!(beat["partial"], false);
}

#[test]
fn schema_5_adds_the_heartbeat_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db");
    let store = Store::open(&path).unwrap();
    store.record_heartbeat("work", true, 4, "off").unwrap();
    store.record_heartbeat("work", false, 0, "live").unwrap();
    let beat = store.heartbeat("work").unwrap().unwrap();
    assert_eq!((beat["partial"].clone(), beat["exit_code"].clone(), beat["mode"].clone()), (false.into(), 0.into(), "live".into()));
    drop(store);
    let version: u32 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 5);
}
```

(Uses `chrono` and `rusqlite`, both regular dependencies available to integration tests.)

Create `tests/system_service.rs`:

```rust
#![cfg(unix)]
//! Service files from pure functions, both managers against fake tools, and
//! the `service` commands through the binary.
mod common;
use common::{write_tool, LAUNCHCTL, SYSTEMCTL};
use mailtriage::{
    service::ServiceError,
    system_service::{self, Context, Manager, Unit},
};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn spaced_unit() -> Unit {
    Unit {
        account: "work".into(),
        exe: PathBuf::from("/opt/mail triage/bin/mailtriage"),
        config: PathBuf::from("/Users/a & b/.config/mailtriage/mailtriage.json"),
        interval_seconds: 60,
        limit: 100,
        log_dir: PathBuf::from("/Users/a & b/.config/mailtriage/logs"),
        path_env: Some("/opt/homebrew/bin:/usr/bin:/bin".into()),
    }
}

#[test]
fn the_plist_is_fixed_text_with_escaped_paths() {
    let expected = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>XMailtriageManaged</key>
  <true/>
  <key>Label</key>
  <string>digital.wirdrei.mailtriage.work</string>
  <key>ProgramArguments</key>
  <array>
    <string>/opt/mail triage/bin/mailtriage</string>
    <string>watch</string>
    <string>--config</string>
    <string>/Users/a &amp; b/.config/mailtriage/mailtriage.json</string>
    <string>--account</string>
    <string>work</string>
    <string>--interval-seconds</string>
    <string>60</string>
    <string>--limit</string>
    <string>100</string>
    <string>--json</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>/opt/homebrew/bin:/usr/bin:/bin</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ThrottleInterval</key>
  <integer>30</integer>
  <key>Umask</key>
  <integer>63</integer>
  <key>StandardOutPath</key>
  <string>/Users/a &amp; b/.config/mailtriage/logs/work.log</string>
  <key>StandardErrorPath</key>
  <string>/Users/a &amp; b/.config/mailtriage/logs/work.err</string>
</dict>
</plist>
"#;
    assert_eq!(system_service::plist(&spaced_unit()), expected);
}

#[test]
fn the_unit_is_fixed_text_with_quoted_paths() {
    let expected = "# managed by mailtriage
[Unit]
Description=mailtriage watch for account work

[Service]
Type=simple
ExecStart=\"/opt/mail triage/bin/mailtriage\" watch --config \"/Users/a & b/.config/mailtriage/mailtriage.json\" --account work --interval-seconds 60 --limit 100 --json
Environment=\"PATH=/opt/homebrew/bin:/usr/bin:/bin\"
Restart=on-failure
RestartSec=30
UMask=0077

[Install]
WantedBy=default.target
";
    assert_eq!(system_service::systemd_unit(&spaced_unit()), expected);
}

fn unit_in(dir: &Path) -> Unit {
    Unit {
        account: "work".into(),
        exe: dir.join("mailtriage"),
        config: dir.join("mailtriage.json"),
        interval_seconds: 60,
        limit: 100,
        log_dir: dir.join("logs"),
        path_env: None,
    }
}

fn context(manager: Manager, dir: &Path) -> Context {
    write_tool(dir, "launchctl", LAUNCHCTL);
    write_tool(dir, "systemctl", SYSTEMCTL);
    let tool = match manager {
        Manager::Launchd => dir.join("launchctl"),
        Manager::Systemd => dir.join("systemctl"),
    };
    Context { manager, home: dir.join("home"), tool, uid: 501 }
}

fn calls(dir: &Path, tool: &str) -> Vec<String> {
    fs::read_to_string(dir.join(format!("{tool}.log")))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn code(result: anyhow::Result<Value>) -> i32 {
    result.unwrap_err().downcast_ref::<ServiceError>().unwrap().code
}

#[test]
fn launchd_install_reload_status_and_uninstall() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = context(Manager::Launchd, dir.path());
    let unit = unit_in(dir.path());
    let target = "gui/501/digital.wirdrei.mailtriage.work";
    let out = system_service::install(&ctx, &unit).unwrap();
    let path = dir.path().join("home/Library/LaunchAgents/digital.wirdrei.mailtriage.work.plist");
    assert_eq!(out["action"], "installed");
    assert_eq!(out["unit_path"], path.to_str().unwrap());
    assert_eq!(fs::read_to_string(&path).unwrap(), system_service::plist(&unit));
    assert!(dir.path().join("logs").is_dir());
    assert_eq!(
        calls(dir.path(), "launchctl"),
        vec![format!("print {target}"), format!("bootstrap gui/501 {}", path.display())]
    );
    let s = system_service::status(Some(&ctx), "work", &unit.log_dir);
    assert_eq!(s["manager"], "launchd");
    assert_eq!(s["installed"], true);
    assert_eq!(s["loaded"], true);
    assert_eq!(s["running"], true);
    assert_eq!(s["pid"], 4242);
    assert_eq!(s["last_exit_status"], 0);
    assert_eq!(s["log_paths"], json!([dir.path().join("logs/work.log"), dir.path().join("logs/work.err")]));
    // Installing again reloads: bootout, then bootstrap, same file.
    system_service::install(&ctx, &unit).unwrap();
    let log = calls(dir.path(), "launchctl");
    assert_eq!(
        log[log.len() - 3..],
        [format!("print {target}"), format!("bootout {target}"), format!("bootstrap gui/501 {}", path.display())]
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), system_service::plist(&unit));
    let out = system_service::uninstall(&ctx, "work").unwrap();
    assert_eq!(out["action"], "uninstalled");
    assert!(!path.exists());
    let s = system_service::status(Some(&ctx), "work", &unit.log_dir);
    assert_eq!((s["installed"].clone(), s["loaded"].clone(), s["running"].clone()), (json!(false), json!(false), json!(false)));
    assert_eq!(system_service::uninstall(&ctx, "work").unwrap()["action"], "not_installed");
}

#[test]
fn systemd_install_status_and_uninstall() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = context(Manager::Systemd, dir.path());
    let unit = unit_in(dir.path());
    system_service::install(&ctx, &unit).unwrap();
    let path = dir.path().join("home/.config/systemd/user/mailtriage-work.service");
    assert_eq!(fs::read_to_string(&path).unwrap(), system_service::systemd_unit(&unit));
    assert_eq!(
        calls(dir.path(), "systemctl"),
        ["--user daemon-reload", "--user enable mailtriage-work.service", "--user restart mailtriage-work.service"]
    );
    let s = system_service::status(Some(&ctx), "work", &unit.log_dir);
    assert_eq!(s["manager"], "systemd");
    assert_eq!((s["loaded"].clone(), s["running"].clone(), s["pid"].clone()), (json!(true), json!(true), json!(4343)));
    assert_eq!(s["log_paths"], json!([]));
    system_service::uninstall(&ctx, "work").unwrap();
    assert!(!path.exists());
    // Install's three calls and status's `show` come first.
    let log = calls(dir.path(), "systemctl");
    assert_eq!(
        log[4..],
        ["--user disable --now mailtriage-work.service", "--user daemon-reload"]
    );
    let s = system_service::status(Some(&ctx), "work", &unit.log_dir);
    assert_eq!((s["loaded"].clone(), s["running"].clone(), s["pid"].clone()), (json!(false), json!(false), Value::Null));
}

#[test]
fn files_mailtriage_did_not_write_are_never_touched() {
    for manager in [Manager::Launchd, Manager::Systemd] {
        let dir = tempfile::tempdir().unwrap();
        let ctx = context(manager, dir.path());
        let path = ctx.unit_path("work");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "the user's own file\n").unwrap();
        assert_eq!(code(system_service::install(&ctx, &unit_in(dir.path()))), 5);
        assert_eq!(code(system_service::uninstall(&ctx, "work")), 5);
        assert_eq!(fs::read_to_string(&path).unwrap(), "the user's own file\n");
        assert!(calls(dir.path(), "launchctl").is_empty() && calls(dir.path(), "systemctl").is_empty());
        assert_eq!(system_service::status(Some(&ctx), "work", dir.path())["installed"], false);
    }
}

#[test]
fn a_failing_manager_is_exit_3() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = context(Manager::Systemd, dir.path());
    write_tool(dir.path(), "broken", "#!/bin/sh\nexit 1\n");
    ctx.tool = dir.path().join("broken");
    assert_eq!(code(system_service::install(&ctx, &unit_in(dir.path()))), 3);
}

#[test]
fn status_without_a_manager() {
    let s = system_service::status(None, "work", Path::new("/logs"));
    assert_eq!(
        s,
        json!({"manager":"none","installed":false,"loaded":false,"running":false,"pid":null,"last_exit_status":null,"unit_path":null,"log_paths":[]})
    );
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn service_commands_through_the_cli() {
    let dir = tempfile::tempdir().unwrap();
    let (home, bin, cwd) = (dir.path().join("home"), dir.path().join("bin"), dir.path().join("cwd"));
    for d in [&home, &bin, &cwd] {
        fs::create_dir_all(d).unwrap();
    }
    write_tool(&bin, "launchctl", LAUNCHCTL);
    write_tool(&bin, "systemctl", SYSTEMCTL);
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
            .current_dir(&cwd)
            .args(args)
            .env("HOME", &home)
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env_remove("MAILTRIAGE_CONFIG")
            .output()
            .unwrap();
        let value: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
        (out.status.code(), value)
    };
    assert_eq!(run(&["init", "--json"]).0, Some(0));
    let (code, v) = run(&["service", "status", "--account", "work", "--json"]);
    assert_eq!(code, Some(0), "{v}");
    let manager = if cfg!(target_os = "macos") { "launchd" } else { "systemd" };
    assert_eq!(v["service"]["manager"], manager);
    assert_eq!(v["service"]["installed"], false);
    assert_eq!(v["service"]["last_pass"], Value::Null);
    assert_eq!(run(&["sync", "--account", "work", "--json"]).0, Some(0));
    let (_, v) = run(&["service", "status", "--account", "work", "--json"]);
    assert_eq!(v["service"]["last_pass"]["exit_code"], 0);
    assert_eq!(v["service"]["last_pass"]["mode"], "off");

    let (code, v) = run(&["service", "install", "--account", "work", "--interval-seconds", "120", "--json"]);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["service"]["action"], "installed");
    let text = fs::read_to_string(v["service"]["unit_path"].as_str().unwrap()).unwrap();
    let config = fs::canonicalize(cwd.join("mailtriage.json")).unwrap();
    assert!(text.contains(config.to_str().unwrap()), "{text}");
    assert!(text.contains("120"), "{text}");
    let (_, v) = run(&["service", "status", "--account", "work", "--json"]);
    assert_eq!(v["service"]["installed"], true);
    assert_eq!(v["service"]["running"], true);
    let (code, v) = run(&["service", "uninstall", "--account", "work", "--json"]);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["service"]["action"], "uninstalled");
    assert_eq!(run(&["service", "install", "--account", "nobody", "--json"]).0, Some(2));
}
```

Unit tests in `src/system_service.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn systemd_args_escape_specifiers() {
        assert_eq!(systemd_arg("plain"), "plain");
        assert_eq!(systemd_arg("50%$x"), "50%%$$x");
        assert_eq!(systemd_arg("a b\"c"), "\"a b\\\"c\"");
        assert_eq!(systemd_arg(""), "\"\"");
    }

    #[test]
    fn launchctl_print_reads_only_top_level_properties() {
        let text = "gui/501/x = {\n\tstate = waiting\n\tlast exit code = (never exited)\n\tendpoints = {\n\t\tstate = running\n\t\tpid = 9\n\t}\n}\n";
        let state = parse_launchctl_print(text);
        assert!(state.loaded && !state.running);
        assert_eq!((state.pid, state.last_exit_status), (None, None));
    }

    #[test]
    fn systemctl_show_parsing() {
        let state = parse_systemctl_show("LoadState=loaded\nActiveState=activating\nSubState=auto-restart\nMainPID=0\nExecMainStatus=3\n");
        assert!(state.loaded && !state.running);
        assert_eq!((state.pid, state.last_exit_status), (None, Some(3)));
    }
}
```

Add to `tests/setup.rs`. Prompted runs default to `--service skip`, so the earlier tests do not see the new question. Change `run_with` to append `--service skip` when `args` contains `--interactive` and no `--service`. Add `run_exact` with the old behaviour.

```rust
    /// `run_with` without the automatic `--service skip`.
    fn run_exact(&self, args: &[&str], stdin: &str, env: &[(&str, &str)]) -> (Output, Value) {
        // the body `run_with` had in Task 3
    }

    fn run_with(&self, args: &[&str], stdin: &str, env: &[(&str, &str)]) -> (Output, Value) {
        let mut args = args.to_vec();
        if args.contains(&"--interactive") && !args.contains(&"--service") {
            args.extend(["--service", "skip"]);
        }
        self.run_exact(&args, stdin, env)
    }
```

Then replace the local `write_tool` in `tests/setup.rs` with `mod common; use common::{write_tool, LAUNCHCTL, SYSTEMCTL};` and add:

```rust
#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn setup_can_install_the_background_service() {
    let f = Fixture::new();
    write_tool(&f.bin, "launchctl", LAUNCHCTL);
    write_tool(&f.bin, "systemctl", SYSTEMCTL);
    let (out, v) = f.run(&[&WORK_ENV[..], &["--service", "install", "--interval-seconds", "300"]].concat(), "");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let service = &v["setup"]["service"];
    assert_eq!(service["action"], "installed");
    let text = fs::read_to_string(service["unit_path"].as_str().unwrap()).unwrap();
    assert!(text.contains("300"), "{text}");
    // The key comes from an environment variable the service does not have.
    assert!(service["note"].as_str().unwrap().contains("OPENROUTER_API_KEY"));
    if cfg!(target_os = "linux") {
        assert!(stderr(&out).contains("loginctl enable-linger"));
    }
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn prompting_offers_the_service_with_yes_as_default() {
    let f = Fixture::new();
    write_tool(&f.bin, "launchctl", LAUNCHCTL);
    write_tool(&f.bin, "systemctl", SYSTEMCTL);
    // Eight defaults, key variable, filing, then Enter for the service.
    let input = "\n".repeat(11);
    let (out, v) = f.run_exact(&["setup", "--interactive", "--json", "--key-store", "env"], &input, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(v["setup"]["service"]["action"], "installed");
}

#[test]
fn without_service_flags_nothing_is_installed() {
    let f = Fixture::new();
    let (out, v) = f.run(&WORK_ENV, "");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(v["setup"]["service"], Value::Null);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test system_service --test heartbeat --test setup`
Expected: compile errors (`system_service`, `heartbeat` missing).

- [ ] **Step 3: Store and sync**

`src/store.rs`:
- `MIGRATIONS` becomes `[(u32, &str); 5]` with `(5, "CREATE TABLE pass_heartbeats(account TEXT PRIMARY KEY, finished_at TEXT NOT NULL, partial INTEGER NOT NULL, exit_code INTEGER NOT NULL, mode TEXT NOT NULL);")`.
- The guard becomes `if version > 5`.
- Add:

```rust
    /// Records how the account's latest sync pass ended.
    pub fn record_heartbeat(&self, account: &str, partial: bool, exit_code: i32, mode: &str) -> Result<()> {
        self.db.execute(
            "INSERT INTO pass_heartbeats(account,finished_at,partial,exit_code,mode) VALUES(?,?,?,?,?)
             ON CONFLICT(account) DO UPDATE SET finished_at=excluded.finished_at,partial=excluded.partial,
             exit_code=excluded.exit_code,mode=excluded.mode",
            params![account, now(), partial, exit_code, mode],
        )?;
        Ok(())
    }

    /// The latest pass: `{finished_at, partial, exit_code, mode}`.
    pub fn heartbeat(&self, account: &str) -> Result<Option<Value>> {
        Ok(self
            .db
            .query_row(
                "SELECT finished_at,partial,exit_code,mode FROM pass_heartbeats WHERE account=?",
                [account],
                |r| {
                    Ok(json!({
                        "finished_at": r.get::<_, String>(0)?,
                        "partial": r.get::<_, bool>(1)?,
                        "exit_code": r.get::<_, i64>(2)?,
                        "mode": r.get::<_, String>(3)?,
                    }))
                },
            )
            .optional()?)
    }
```

`src/service.rs`: rename the body of `sync` after `let _lock = self.lock(name)?;` into `fn sync_locked(&mut self, name: &str, limit: usize) -> Result<Value>`. Then:

```rust
    pub fn sync(&mut self, name: &str, limit: usize) -> Result<Value> {
        if !(1..=1000).contains(&limit) {
            return Err(err(2, "sync limit must be 1..=1000"));
        }
        let _lock = self.lock(name)?;
        let result = self.sync_locked(name, limit);
        if let Some(account) = self.config.accounts.get(name) {
            let mode = if account.engine_config().is_some() {
                account.filing.mode
            } else {
                FilingMode::Off
            };
            let (partial, code) = match &result {
                Ok(value) => {
                    let partial = value.get("partial").and_then(Value::as_bool) == Some(true);
                    (partial, if partial { 4 } else { 0 })
                }
                Err(e) => (false, exit_code(e)),
            };
            // A heartbeat that cannot be written never hides the pass result.
            let _ = self
                .store
                .record_heartbeat(name, partial, code, filing::mode_str(mode));
        }
        result
    }
```

```rust
/// The process exit code the CLI reports for `error`.
pub fn exit_code(error: &anyhow::Error) -> i32 {
    if let Some(e) = error.downcast_ref::<ServiceError>() {
        return e.code;
    }
    if error.downcast_ref::<engine::ConfigChanged>().is_some() {
        return 5;
    }
    3
}
```

- [ ] **Step 4: `src/system_service.rs`**

```rust
//! The background service: a launchd agent (macOS) or a systemd user unit
//! (Linux) running `mailtriage watch` for one account. Files carry a marker;
//! mailtriage replaces or removes only files it wrote. Service files hold
//! absolute paths and never a secret.
use crate::{
    config, process,
    service::{err, Service},
};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

pub const LABEL_PREFIX: &str = "digital.wirdrei.mailtriage";
const PLIST_MARKER: &str = "<key>XMailtriageManaged</key>";
const UNIT_MARKER: &str = "# managed by mailtriage";
const TOOL_TIMEOUT: Duration = Duration::from_secs(30);
const TOOL_MAX_OUTPUT: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manager {
    Launchd,
    Systemd,
}

impl Manager {
    pub fn name(self) -> &'static str {
        match self {
            Manager::Launchd => "launchd",
            Manager::Systemd => "systemd",
        }
    }
}

/// How to reach this user's service manager.
#[derive(Debug, Clone)]
pub struct Context {
    pub manager: Manager,
    pub home: PathBuf,
    /// `launchctl` or `systemctl`.
    pub tool: PathBuf,
    /// launchd's `gui/<uid>` domain.
    pub uid: u32,
}

impl Context {
    /// This platform's manager; other platforms exit 2.
    pub fn detect() -> Result<Self> {
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| err(2, "HOME is not set"))?;
        let (manager, tool) = if cfg!(target_os = "macos") {
            (
                Manager::Launchd,
                process::find_on_path("launchctl").unwrap_or_else(|| PathBuf::from("/bin/launchctl")),
            )
        } else if cfg!(target_os = "linux") {
            (
                Manager::Systemd,
                process::find_on_path("systemctl")
                    .ok_or_else(|| err(3, "systemctl is not on PATH; the service needs systemd"))?,
            )
        } else {
            return Err(err(
                2,
                "unsupported platform: the background service needs macOS (launchd) or Linux (systemd)",
            ));
        };
        Ok(Self { manager, home, tool, uid: current_uid() })
    }

    pub fn unit_path(&self, account: &str) -> PathBuf {
        match self.manager {
            Manager::Launchd => self
                .home
                .join("Library/LaunchAgents")
                .join(format!("{}.plist", label(account))),
            Manager::Systemd => self.home.join(".config/systemd/user").join(unit_name(account)),
        }
    }
}

#[cfg(unix)]
fn current_uid() -> u32 {
    unsafe extern "C" {
        fn getuid() -> u32;
    }
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { getuid() }
}

#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

pub fn label(account: &str) -> String {
    format!("{LABEL_PREFIX}.{account}")
}

pub fn unit_name(account: &str) -> String {
    format!("mailtriage-{account}.service")
}

/// What the service runs.
#[derive(Debug, Clone)]
pub struct Unit {
    pub account: String,
    /// The mailtriage executable, absolute.
    pub exe: PathBuf,
    /// The config, absolute.
    pub config: PathBuf,
    pub interval_seconds: u64,
    pub limit: usize,
    /// Where launchd writes stdout and stderr: `<config dir>/logs`.
    pub log_dir: PathBuf,
    /// PATH for the service, so key tools such as `pass` find their helpers.
    pub path_env: Option<String>,
}

impl Unit {
    pub fn arguments(&self) -> Vec<String> {
        vec![
            self.exe.display().to_string(),
            "watch".into(),
            "--config".into(),
            self.config.display().to_string(),
            "--account".into(),
            self.account.clone(),
            "--interval-seconds".into(),
            self.interval_seconds.to_string(),
            "--limit".into(),
            self.limit.to_string(),
            "--json".into(),
        ]
    }

    pub fn log_paths(&self) -> [PathBuf; 2] {
        [
            self.log_dir.join(format!("{}.log", self.account)),
            self.log_dir.join(format!("{}.err", self.account)),
        ]
    }
}

pub fn plist(unit: &Unit) -> String {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n",
    );
    s += &format!("  {PLIST_MARKER}\n  <true/>\n");
    s += &format!("  <key>Label</key>\n  <string>{}</string>\n", xml(&label(&unit.account)));
    s += "  <key>ProgramArguments</key>\n  <array>\n";
    for arg in unit.arguments() {
        s += &format!("    <string>{}</string>\n", xml(&arg));
    }
    s += "  </array>\n";
    if let Some(path) = &unit.path_env {
        s += &format!(
            "  <key>EnvironmentVariables</key>\n  <dict>\n    <key>PATH</key>\n    <string>{}</string>\n  </dict>\n",
            xml(path)
        );
    }
    s += "  <key>RunAtLoad</key>\n  <true/>\n  <key>KeepAlive</key>\n  <dict>\n    <key>SuccessfulExit</key>\n    <false/>\n  </dict>\n  <key>ThrottleInterval</key>\n  <integer>30</integer>\n  <key>Umask</key>\n  <integer>63</integer>\n";
    let [out, errors] = unit.log_paths();
    s += &format!(
        "  <key>StandardOutPath</key>\n  <string>{}</string>\n  <key>StandardErrorPath</key>\n  <string>{}</string>\n",
        xml(&out.display().to_string()),
        xml(&errors.display().to_string())
    );
    s += "</dict>\n</plist>\n";
    s
}

pub fn systemd_unit(unit: &Unit) -> String {
    let exec: Vec<String> = unit.arguments().iter().map(|a| systemd_arg(a)).collect();
    let mut s = format!(
        "{UNIT_MARKER}\n[Unit]\nDescription=mailtriage watch for account {}\n\n[Service]\nType=simple\nExecStart={}\n",
        unit.account,
        exec.join(" ")
    );
    if let Some(path) = &unit.path_env {
        // Environment= expands % specifiers but not $ variables.
        s += &format!(
            "Environment=\"PATH={}\"\n",
            path.replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%")
        );
    }
    s += "Restart=on-failure\nRestartSec=30\nUMask=0077\n\n[Install]\nWantedBy=default.target\n";
    s
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// One `ExecStart=` word: `%` and `$` doubled, quoted when it holds
/// whitespace, quotes, backslashes or `;`.
fn systemd_arg(arg: &str) -> String {
    let escaped = arg.replace('%', "%%").replace('$', "$$");
    if escaped.is_empty()
        || escaped
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '\\' | ';'))
    {
        format!("\"{}\"", escaped.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        escaped
    }
}

fn is_marked(manager: Manager, text: &str) -> bool {
    match manager {
        Manager::Launchd => text.contains(PLIST_MARKER),
        Manager::Systemd => text.lines().next() == Some(UNIT_MARKER),
    }
}

/// Exit 5 when a file mailtriage did not write is at `path`.
fn refuse_unmarked(ctx: &Context, path: &Path) -> Result<()> {
    match fs::read_to_string(path) {
        Ok(text) if is_marked(ctx.manager, &text) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(err(
            5,
            format!(
                "{} exists and was not written by mailtriage; move it away first",
                path.display()
            ),
        )),
    }
}

/// Runs the manager tool; anything but exit 0 is exit 3.
fn tool(ctx: &Context, args: &[&str]) -> Result<()> {
    tool_output(ctx, args).map(|_| ()).ok_or_else(|| {
        err(
            3,
            format!(
                "`{} {}` failed",
                ctx.tool.file_name().unwrap_or_default().to_string_lossy(),
                args.join(" ")
            ),
        )
    })
}

fn tool_output(ctx: &Context, args: &[&str]) -> Option<String> {
    process::run_bounded(&ctx.tool, args, TOOL_TIMEOUT, TOOL_MAX_OUTPUT)
        .ok()?
        .success()
        .map(|out| String::from_utf8_lossy(&out).into_owned())
}

fn target(ctx: &Context, account: &str) -> String {
    format!("gui/{}/{}", ctx.uid, label(account))
}

pub fn install(ctx: &Context, unit: &Unit) -> Result<Value> {
    let path = ctx.unit_path(&unit.account);
    refuse_unmarked(ctx, &path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    match ctx.manager {
        Manager::Launchd => {
            fs::create_dir_all(&unit.log_dir)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&unit.log_dir, fs::Permissions::from_mode(0o700))?;
            }
            let target = target(ctx, &unit.account);
            if tool_output(ctx, &["print", &target]).is_some() {
                tool(ctx, &["bootout", &target])?;
            }
            fs::write(&path, plist(unit))?;
            let domain = format!("gui/{}", ctx.uid);
            tool(ctx, &["bootstrap", &domain, &path.display().to_string()])?;
        }
        Manager::Systemd => {
            fs::write(&path, systemd_unit(unit))?;
            let name = unit_name(&unit.account);
            tool(ctx, &["--user", "daemon-reload"])?;
            tool(ctx, &["--user", "enable", &name])?;
            tool(ctx, &["--user", "restart", &name])?;
        }
    }
    Ok(json!({
        "action": "installed",
        "manager": ctx.manager.name(),
        "account": unit.account,
        "unit_path": path,
        "log_paths": log_paths(ctx.manager, unit),
        "command": unit.arguments(),
    }))
}

pub fn uninstall(ctx: &Context, account: &str) -> Result<Value> {
    let path = ctx.unit_path(account);
    refuse_unmarked(ctx, &path)?;
    let existed = path.exists();
    match ctx.manager {
        Manager::Launchd => {
            let target = target(ctx, account);
            if tool_output(ctx, &["print", &target]).is_some() {
                tool(ctx, &["bootout", &target])?;
            }
            if existed {
                fs::remove_file(&path)?;
            }
        }
        Manager::Systemd => {
            if existed {
                tool(ctx, &["--user", "disable", "--now", &unit_name(account)])?;
                fs::remove_file(&path)?;
                tool(ctx, &["--user", "daemon-reload"])?;
            }
        }
    }
    Ok(json!({
        "action": if existed { "uninstalled" } else { "not_installed" },
        "manager": ctx.manager.name(),
        "account": account,
        "unit_path": path,
    }))
}

fn log_paths(manager: Manager, unit: &Unit) -> Vec<PathBuf> {
    match manager {
        Manager::Launchd => unit.log_paths().to_vec(),
        Manager::Systemd => vec![],
    }
}

/// What the manager reports for a job.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ManagerState {
    pub loaded: bool,
    pub running: bool,
    pub pid: Option<u32>,
    pub last_exit_status: Option<i64>,
}

/// `launchctl print` output of a loaded job; only top-level properties
/// (one tab deep) count, nested blocks repeat keys such as `state`.
pub fn parse_launchctl_print(text: &str) -> ManagerState {
    let mut state = ManagerState { loaded: true, ..ManagerState::default() };
    for line in text.lines() {
        let Some(property) = line.strip_prefix('\t').filter(|l| !l.starts_with('\t')) else {
            continue;
        };
        let Some((key, value)) = property.split_once(" = ") else {
            continue;
        };
        match key {
            "state" => state.running = value == "running",
            "pid" => state.pid = value.parse().ok(),
            "last exit code" => state.last_exit_status = value.parse().ok(),
            _ => {}
        }
    }
    state
}

pub fn parse_systemctl_show(text: &str) -> ManagerState {
    let get = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
    };
    ManagerState {
        loaded: get("LoadState") == Some("loaded"),
        running: get("ActiveState") == Some("active") && get("SubState") == Some("running"),
        pid: get("MainPID").and_then(|p| p.parse().ok()).filter(|&p| p > 0),
        last_exit_status: get("ExecMainStatus").and_then(|s| s.parse().ok()),
    }
}

/// `service status` manager fields; `None` means no supported manager.
pub fn status(ctx: Option<&Context>, account: &str, log_dir: &Path) -> Value {
    let Some(ctx) = ctx else {
        return json!({"manager":"none","installed":false,"loaded":false,"running":false,"pid":null,"last_exit_status":null,"unit_path":null,"log_paths":[]});
    };
    let path = ctx.unit_path(account);
    let installed = fs::read_to_string(&path).is_ok_and(|text| is_marked(ctx.manager, &text));
    let state = match ctx.manager {
        Manager::Launchd => tool_output(ctx, &["print", &target(ctx, account)])
            .map(|text| parse_launchctl_print(&text)),
        Manager::Systemd => tool_output(
            ctx,
            &[
                "--user",
                "show",
                &unit_name(account),
                "--property=LoadState,ActiveState,SubState,MainPID,ExecMainStatus",
            ],
        )
        .map(|text| parse_systemctl_show(&text)),
    }
    .unwrap_or_default();
    let log_paths: Vec<PathBuf> = match ctx.manager {
        Manager::Launchd => vec![
            log_dir.join(format!("{account}.log")),
            log_dir.join(format!("{account}.err")),
        ],
        Manager::Systemd => vec![],
    };
    json!({
        "manager": ctx.manager.name(),
        "installed": installed,
        "loaded": state.loaded,
        "running": state.running,
        "pid": state.pid,
        "last_exit_status": state.last_exit_status,
        "unit_path": path,
        "log_paths": log_paths,
    })
}

/// The unit for `account` of the config at `config_path`.
pub fn unit_for(config_path: &Path, account: &str, interval_seconds: u64, limit: usize) -> Result<Unit> {
    if !config::valid_account_name(account) {
        return Err(err(
            2,
            format!("account name {account:?} cannot name a service; use 1 to 64 letters, digits, - or _"),
        ));
    }
    let config = fs::canonicalize(config_path)?;
    let log_dir = config
        .parent()
        .map_or_else(|| PathBuf::from("logs"), |dir| dir.join("logs"));
    Ok(Unit {
        account: account.to_owned(),
        exe: std::env::current_exe()?,
        config,
        interval_seconds,
        limit,
        log_dir,
        path_env: std::env::var("PATH").ok().filter(|p| !p.is_empty()),
    })
}

/// `service install` for an account of `service`'s config, with a note
/// when the key comes from an environment variable the service lacks.
pub fn install_account(
    service: &Service,
    config_path: &Path,
    account: &str,
    interval_seconds: u64,
    limit: usize,
    ctx: &Context,
) -> Result<Value> {
    if !service.config.accounts.contains_key(account) {
        return Err(err(2, "unknown account"));
    }
    let unit = unit_for(config_path, account, interval_seconds, limit)?;
    let mut out = install(ctx, &unit)?;
    let provider = &service.config.provider;
    if provider.kind == "openrouter" && provider.api_key_command.is_none() {
        out["note"] = json!(format!(
            "The key comes from {}, which the service does not inherit. Store it with `mailtriage setup --update --key-store keychain` (or secret-service, pass, command), or add {} to the service's environment yourself.",
            provider.api_key_env, provider.api_key_env
        ));
    }
    Ok(out)
}

/// `service status`: manager fields plus the last sync pass from the state
/// database, which works without any service.
pub fn status_account(service: &Service, config_path: &Path, account: &str, ctx: Option<&Context>) -> Result<Value> {
    if !service.config.accounts.contains_key(account) {
        return Err(err(2, "unknown account"));
    }
    let config = fs::canonicalize(config_path)?;
    let log_dir = config
        .parent()
        .map_or_else(|| PathBuf::from("logs"), |dir| dir.join("logs"));
    let mut out = status(ctx, account, &log_dir);
    out["account"] = json!(account);
    out["last_pass"] = service.store.heartbeat(account)?.unwrap_or(Value::Null);
    Ok(out)
}

#[cfg(test)]
mod tests { /* the three unit tests from Step 1 */ }
```

Add `pub mod system_service;` to `src/lib.rs`. `Service.store` is already public.

- [ ] **Step 5: CLI `service` and setup step 10**

`src/cli.rs`:

```rust
    /// Run `watch` in the background (launchd on macOS, systemd on Linux).
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
```

```rust
#[derive(Subcommand)]
enum ServiceCommand {
    /// Install and start `watch` for an account.
    Install(ServiceInstallArg),
    /// Stop and remove the account's service.
    Uninstall(AccountArg),
    /// Whether the service is installed and running, and the last sync pass.
    Status(AccountArg),
}

#[derive(Args)]
struct ServiceInstallArg {
    #[arg(long)]
    account: String,
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..=86400))]
    interval_seconds: u64,
    #[arg(long, default_value_t = 100, value_parser = parse_limit)]
    limit: usize,
}

fn service_command(path: &Path, command: &ServiceCommand) -> Result<Value, CliError> {
    let result = match command {
        ServiceCommand::Install(arg) => Context::detect().and_then(|ctx| {
            let service = Service::open(path)?;
            system_service::install_account(&service, path, &arg.account, arg.interval_seconds, arg.limit, &ctx)
        }),
        ServiceCommand::Uninstall(arg) => {
            if !config::valid_account_name(&arg.account) {
                return Err(CliError::input("account name cannot name a service"));
            }
            Context::detect().and_then(|ctx| system_service::uninstall(&ctx, &arg.account))
        }
        ServiceCommand::Status(arg) => Service::open(path).and_then(|service| {
            system_service::status_account(&service, path, &arg.account, Context::detect().ok().as_ref())
        }),
    };
    result
        .map(|service| json!({"schema_version": 1, "service": service}))
        .map_err(service_error)
}
```

In `execute`: `Command::Service { command } => service_command(&cli.config_path()?, command),`.

Setup flags on `SetupArg`:

```rust
    /// Install the background service at the end (default: ask; skip without prompts).
    #[arg(long, value_parser = ["install", "skip"])]
    service: Option<String>,
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..=86400))]
    interval_seconds: u64,
    #[arg(long, default_value_t = 100, value_parser = parse_limit)]
    limit: usize,
```

They map to the `SetupArgs` fields `service: Option<bool>` (`install` → `Some(true)`), `interval_seconds: u64` and `limit: usize`.

`src/setup.rs`: add those three fields to `SetupArgs`, then step 10 after `next_steps`:

```rust
    // 10. Service.
    let service = service_step(args, p, &path, &name)?;
```

and `"service": service` in the result, with:

```rust
/// Step 10: the background service. Asked with prompts (default yes);
/// without prompts only `--service install` installs it.
fn service_step(args: &SetupArgs, p: &mut Prompter, path: &Path, name: &str) -> Result<Value> {
    let wanted = match args.service {
        Some(wanted) => wanted,
        None if p.enabled() => match Context::detect() {
            Ok(_) => p.confirm(
                &format!(
                    "Run mailtriage in the background now (`watch` every {} seconds)?",
                    args.interval_seconds
                ),
                true,
            )?,
            Err(_) => {
                p.say("The background service needs macOS or Linux; skipping it.");
                false
            }
        },
        None => false,
    };
    if !wanted {
        return Ok(Value::Null);
    }
    let ctx = Context::detect()?;
    let service = Service::open(path)?;
    let out = system_service::install_account(&service, path, name, args.interval_seconds, args.limit, &ctx)?;
    p.say(&format!("Installed the background service ({}).", ctx.manager.name()));
    if let Some(note) = out["note"].as_str() {
        p.say(note);
    }
    if ctx.manager == Manager::Systemd {
        p.say("To keep it running while you are logged out: loginctl enable-linger $USER");
    }
    Ok(out)
}
```

with `use crate::system_service::{self, Context, Manager};`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --test system_service --test heartbeat --test setup --test filing_store && cargo test --lib system_service`
Expected: all PASS.

- [ ] **Step 7: Full check and commit**

Run: `cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`

```bash
git add src/system_service.rs src/store.rs src/service.rs src/setup.rs src/cli.rs src/lib.rs tests/system_service.rs tests/heartbeat.rs tests/setup.rs tests/common/mod.rs tests/filing_store.rs
git commit -m "Run watch as a launchd agent or systemd user unit

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Documentation

**Files:**
- Modify: `README.md` (rewrite, short), `docs/agents/index.md`, `docs/development/service-api.md`, `docs/development/verification.md`, `design/specs/2026-10-05-guided-setup-design.md`
- Create: `docs/guide.md`

**Interfaces:**
- Consumes: the commands, flags, JSON and exit codes from Tasks 1–5. Verify each command you document with `cargo run -- <command> --help` or a dry run against `init`'s offline config. Never invent a flag.

The user asked for a short README: "helpful to see what this project is and how to get it up and running". The current README (625 lines) holds the full reference. That reference moves to `docs/guide.md`; the README keeps only what a newcomer needs.

- [ ] **Step 1: Write `docs/guide.md`**

Move every README section from "Setup" through "Reference" into `docs/guide.md` and update it, so nothing that exists today is lost:
- **Setup**: `mailtriage setup` first, covering:
  - the ten steps;
  - every flag;
  - the key stores, with the exact read and store commands;
  - `--yes`, `--interactive` and the exit codes.
  Then "Manual setup" (the current steps 1–6, edited for the new config location and `api_key_command`).
- **Configuration**: the resolution order; a full annotated `mailtriage.json` including `api_key_command`; and the key command rules:
  - no shell; 10 s; 4 KB; first line;
  - the four error strings;
  - the same trust as Himalaya's `password.cmd`, because it runs from the user's own 0600 config.
- **Background service**: `service install|uninstall|status`, where the files and logs live, the `PATH` note, the `env` note, `loginctl enable-linger`, and the status JSON fields.
- Daily use, Categories, Filing into folders, Reference: the existing sections, with commands that no longer need `--config`.

- [ ] **Step 2: Rewrite `README.md` (at most ~120 lines)**

Sections, in order:
1. One paragraph covering:
   - what mailtriage is: a local CLI that classifies mail (category, urgency, action needed) with OpenRouter's Jev decisions model;
   - how it reads mail: through Himalaya;
   - what filing does: files into one IMAP folder per category, so every mail client shows the result;
   - that JSON output makes it usable by agents.
2. How it works: 3–4 bullets, covering:
   - IMAP through Himalaya;
   - classification;
   - local SQLite state;
   - filing that is off, dry-run or live.
3. Requirements: Himalaya v2.1.0 with IMAP; an OpenRouter API key; macOS or Linux for the background service.
4. Install: the existing install instructions, shortened.
5. Get started: `mailtriage setup`, in 5–8 lines covering what it asks, that the key goes into the Keychain, Secret Service, `pass`, a command or an environment variable, and that it can install the background service. Then `mailtriage sync --account NAME` and `mailtriage list --account NAME`.
6. For agents: one `mailtriage setup --yes --himalaya-account work --key-store env` example, and a link to `docs/agents/index.md`.
7. Everyday commands: a table with one line each for:
   - `sync`, `watch`, `list`, `read`, `correct`, `done`;
   - `filing plan`, `filing enable`;
   - `service status`.
8. Try it offline: `init` with the fake provider, 3–4 lines.
9. Documentation: links to `docs/guide.md`, `docs/agents/index.md`, `docs/development/service-api.md` and `docs/development/verification.md`.
10. License.

Style: short, precise sentences; no marketing; every command copy-pasteable.

- [ ] **Step 3: Agent and reference docs**

- `docs/agents/index.md` "Host setup":
  - non-interactive setup (`--yes`, the required `--himalaya-account`, `--key-store` with `--key-stored`, `--key-command` or `--key-env`, `--service install`);
  - exit codes 2, 3 and 5 with the fix for each;
  - `service status --json` and `last_pass` for health checks;
  - that `--config` is optional now.
- `docs/development/service-api.md`:
  - config resolution;
  - the `setup` result object;
  - doctor's `provider.key_source`/`key_error`;
  - the `service` result objects;
  - `pass_heartbeats` (schema v5).
- `docs/development/verification.md`: a section "Guided setup on a real machine (human check)", covering:
  - macOS: Keychain store with the real `security`, the first Keychain access dialog, and `service install` with real `launchctl`, then `service status`;
  - Linux: `secret-tool`, the systemd user unit, and `loginctl enable-linger`.
- The spec: set Status to "Implemented" and replace its two "confirmed in the plan's first task" phrases with the verified facts from this plan's "Verified Himalaya v2.1.0 behaviour".

- [ ] **Step 4: Check and commit**

Run: `cargo test --locked` (docs only, still green), then check that every command in README and the guide parses:

```bash
cargo build && for c in setup "service install" "service status" "service uninstall" sync watch list filing; do ./target/debug/mailtriage $c --help >/dev/null || echo "BROKEN: $c"; done
```

```bash
git add README.md docs/guide.md docs/agents/index.md docs/development/service-api.md docs/development/verification.md design/specs/2026-10-05-guided-setup-design.md
git commit -m "Shorten the README and document setup, key command and service

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Human gate

Before relying on the background service and tool-backed key stores on a real machine, a person runs the "Guided setup on a real machine" checks from `docs/development/verification.md` (Task 6). Automated tests use fake `security`, `secret-tool`, `pass`, `launchctl` and `systemctl`.
