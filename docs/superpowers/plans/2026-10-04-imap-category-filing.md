# IMAP Category Filing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** File classified mail into per-category IMAP folders and set `\Flagged` on actionable or urgent mail, treat client-side moves as corrections, and expose everything through the CLI, behind a swappable `MailEngine` boundary implemented with Himalaya v2.1.0.

**Architecture:** A `MailEngine` trait (`src/engine/`) hides Himalaya; the service only talks to the trait. A pure planner (`src/filing/planner.rs`) decides moves and flags from local state; filing orchestration (`src/filing/`) runs inside `Service::sync` in a fixed step order: observe folders, discover, reconcile, recover intents, fetch/classify, resolve arrivals, plan/apply, infer done. All state lives in SQLite schema v3; every write is journaled as an intent before the engine call.

**Tech Stack:** Rust 2021 (stable), rusqlite (bundled SQLite), serde/serde_json, toml, sha2, chrono, clap 4, Himalaya v2.1.0 CLI as a subprocess. No new crate dependencies.

**Spec:** `docs/superpowers/specs/2026-10-04-imap-category-filing-design.md`. Read it before every task; section names in this plan refer to it. Where this plan and the spec disagree, the spec wins, except for the one deliberate change below.

**Deliberate deviation from the spec:** the spec puts the live provider check first. It needs the user's own test accounts, so it cannot run inside autonomous execution. This plan builds and verifies against `FakeEngine` and a real Dovecot server (Task 10), and leaves the live provider check as a human gate before `live` mode is used on a real mailbox (final section).

## Global Constraints

- Work directly on `main` (user instruction). Commit at the end of every task; every commit message ends with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- After every task all three pass: `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`.
- No new dependencies in `Cargo.toml`.
- Himalaya compatibility target stays exactly `2.1.0`. Never emit `--seen`, `--seq`, `expunge`, `delete`, `rename`, `-FLAGS`, `--action remove`, `--action set` in any Himalaya argument or raw IMAP text.
- Subprocess rules stay: argument arrays (no shell), stdin closed, `--backend imap`, timeout and output limits, stderr discarded.
- JSON responses keep `schema_version: 1` and the error envelope `{"schema_version":1,"error":{"code","message"}}`. Exit codes: 0 ok, 2 invalid input/config, 3 operational, 4 partial, 5 conflict/binding.
- Error text never contains message bodies, subjects or credentials.
- The account binding identity must not change. For the fixture in Task 1 it is exactly `c4a294832a82e8e274fb8bcdf9c2c0daa895f811efe13776ab60da3c7fd50aaa`.
- The classification generation hash must not change. For `config::default_config()`'s `work` account it is exactly `6dfd7bf30e6bf1c0b27ee97af991ecf21dc5ac0400044c2f3adbda7f79d37514`.
- `filing.mode` defaults to `off`; with `off` the observable behaviour of every existing command is unchanged except for additive JSON fields.
- Existing tests keep passing. The only permitted edits to existing tests are: adding `..Default::default()` to `SourceEnvelope` literals, and updating Himalaya fake-binary match patterns for the new `imap fetch` argument order (Task 2).

## Review Focus

These are the inputs most likely to hurt a real user that no task would otherwise exercise. Each line names the expected behaviour; the owning task contains the pinning test.

1. **Enabling filing on an account with a large existing mailbox** (hundreds of already-synced messages): no message is moved or flagged on the first passes; placements are bootstrapped and hydrated at most 100 UIDs per folder per pass. Test: Task 6, `enabling_on_large_existing_mailbox_moves_nothing_and_hydrates_in_batches`.
2. **Renaming a category's display name while filing is on**: the folder stays the same. `filing enable` and `categories apply` write an explicit `folder` for every category, so a later rename does not move mail into a new folder. Test: Task 9, `rename_keeps_folder_after_enable`.
3. **Himalaya `--json imap list` output shape** (an object with `mailboxes`, or a bare array): both parse. Test: Task 2, `list_folders_accepts_object_or_array_json`.
4. **Folder names with spaces** (`Bills and Receipts`): quoted correctly in raw IMAP text and accepted by validation; `&` (the modified UTF-7 shift character) is rejected by validation and by `quote_mailbox` until the provider check confirms Himalaya's encoding. Test: Task 2, `move_quotes_names_with_spaces`.
5. **Mail that arrives during `dry_run` and is then switched to `live`**: filed on the first live pass, because `enabled_at` is kept. Test: Task 7, `dry_run_then_live_files_mail_that_arrived_during_dry_run`.

---

## File Structure

| Path | Status | Responsibility |
| --- | --- | --- |
| `src/domain.rs` | modify | `EngineConfig`, `FilingMode`, `FilingConfig`, `Category.folder`, extended `SourceEnvelope` |
| `src/config.rs` | modify | schema v2, legacy normalization, filing/folder validation |
| `src/engine/mod.rs` | create | `MailEngine` trait, engine types, `ConfigChanged`, factory |
| `src/engine/himalaya.rs` | create (moved from `src/himalaya.rs`) | Himalaya implementation |
| `src/engine/raw.rs` | create | raw IMAP response parser, quoting, UID sets |
| `src/engine/fake.rs` | create | in-memory engine for tests |
| `src/himalaya.rs` | delete | replaced by re-export in `lib.rs` |
| `src/lib.rs` | modify | `pub mod engine; pub mod filing;` and `himalaya` re-export |
| `src/store.rs` | modify | schema v3 migration, `stage_with`, rescan-set capture, filing-aware `attach` |
| `src/filing/mod.rs` | create | module root, `FilingSummary`, shared types |
| `src/filing/types.rs` | create | `Placement`, `FolderRecord`, `Arrival`, `Intent`, `Revert`, `FilingStateRow` |
| `src/filing/store.rs` | create | `impl Store` filing persistence |
| `src/filing/planner.rs` | create | pure planner |
| `src/filing/observe.rs` | create | mode sync, folder resolution, watch list, bootstrap, hydration |
| `src/filing/inputs.rs` | create | builds `PlanInput` from the store |
| `src/filing/apply.rs` | create | claims, move/flag batches, epoch races, reverts |
| `src/filing/recover.rs` | create | intent and revert recovery |
| `src/filing/arrivals.rs` | create | arrival resolution, placement re-evaluation, reopen |
| `src/filing/transitions.rs` | create | correct/pin/unpin/retry/dismiss/adopt transitions |
| `src/filing/done.rs` | create | done inference |
| `src/service.rs` | modify | engine injection, sync step order, filing commands, `placement` in items |
| `src/cli.rs` | modify | `filing` subcommands |
| `tests/common/mod.rs` | create | `Harness` for FakeEngine-backed service tests |
| `tests/config_v2.rs` | create | Task 1 |
| `tests/engine_contract.rs` | create | Task 2 |
| `tests/filing_store.rs` | create | Task 4 |
| `tests/filing_observe.rs` | create | Task 6 |
| `tests/filing_writes.rs` | create | Task 7 |
| `tests/filing_location.rs` | create | Task 8 |
| `tests/filing_cli.rs` | create | Task 9 |
| `tests/e2e_dovecot.rs`, `tests/e2e/` | create | Task 10 |
| `.github/workflows/e2e.yml` | create | Task 10 |
| `README.md`, `docs/hermes.md`, `docs/design.md`, `docs/verification.md` | modify | Tasks 9 and 10 |

---

### Task 1: Config schema v2 and the engine boundary

Moves Himalaya behind a `MailEngine` trait without changing behaviour, and adds the new config fields (`engine`, `filing`, `categories[].folder`).

**Files:**
- Modify: `src/domain.rs`, `src/config.rs`, `src/service.rs`, `src/lib.rs`
- Create: `src/engine/mod.rs`, `src/engine/himalaya.rs` (content of `src/himalaya.rs` plus trait impl)
- Delete: `src/himalaya.rs`
- Test: `tests/config_v2.rs`; unit tests in `src/service.rs`

**Interfaces:**
- Produces:
  - `domain::EngineConfig` (`#[serde(tag = "kind", rename_all = "lowercase")] enum { Himalaya(HimalayaConfig) }`) with `fn mailboxes(&self) -> &[String]`, `fn timeout_seconds(&self) -> u64`.
  - `domain::FilingMode { Off, DryRun, Live }` (serde `snake_case`: `"off"`, `"dry_run"`, `"live"`), `domain::FilingConfig { mode, flag, max_actions_per_pass }` with `Default`.
  - `AccountConfig { .., himalaya: Option<HimalayaConfig> /* legacy input only */, engine: Option<EngineConfig>, filing: FilingConfig }` and `fn engine_config(&self) -> Option<EngineConfig>`.
  - `Category { .., folder: Option<String> }` and `fn effective_folder(&self) -> &str`.
  - `SourceEnvelope` derives `Default` and gains `message_id: Option<String>`, `internal_date: Option<String>`, `size: Option<u64>`, `flags: Vec<String>` (all `#[serde(default)]`).
  - `config::normalize(&mut AppConfig) -> Result<()>`.
  - `engine::MailEngine` with `version`, `binding_identity`, `snapshot`, `discover`, `fetch_raw` (Task 2 adds the rest); `engine::open(&EngineConfig) -> Result<Rc<dyn MailEngine>>`.
  - `engine::himalaya::source_binding(&HimalayaConfig) -> Result<serde_json::Value>` (moved from `service.rs`, identical output and error codes).

- [ ] **Step 1: Write the golden tests (characterization)**

Append to `src/service.rs`:

```rust
#[cfg(test)]
mod golden {
    use crate::domain::HimalayaConfig;

    #[test]
    fn binding_identity_is_unchanged_for_legacy_himalaya_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let toml = dir.path().join("himalaya.toml");
        std::fs::write(&toml, "[accounts.work]\nimap.server='imaps://example.test'\nimap.sasl.plain.username='work@example.test'\nimap.sasl.plain.password.cmd='pass show work'\n").unwrap();
        let mut cfg = crate::config::default_config();
        let a = cfg.accounts.get_mut("work").unwrap();
        a.identity = "work@example.test".into();
        a.himalaya = Some(HimalayaConfig {
            binary: "himalaya".into(),
            config: toml,
            account: "work".into(),
            mailboxes: vec!["INBOX".into()],
            expected_version: "2.1.0".into(),
            timeout_seconds: 30,
            max_output_bytes: 1_000_000,
        });
        crate::config::normalize(&mut cfg).unwrap();
        let a = &cfg.accounts["work"];
        assert_eq!(
            super::binding_identity(a, None).unwrap(),
            "c4a294832a82e8e274fb8bcdf9c2c0daa895f811efe13776ab60da3c7fd50aaa"
        );
    }

    #[test]
    fn generation_hash_is_unchanged_by_filing_and_folder_fields() {
        let mut cfg = crate::config::default_config();
        let expected = "6dfd7bf30e6bf1c0b27ee97af991ecf21dc5ac0400044c2f3adbda7f79d37514";
        assert_eq!(super::generation(&cfg, &cfg.accounts["work"]).unwrap(), expected);
        let a = cfg.accounts.get_mut("work").unwrap();
        a.filing.mode = crate::domain::FilingMode::Live;
        a.categories[0].folder = Some("Mail".into());
        assert_eq!(super::generation(&cfg, &cfg.accounts["work"]).unwrap(), expected);
    }
}
```

`tempfile` is already a dev-dependency, so it is available to `#[cfg(test)]` code.

Create `tests/config_v2.rs`:

```rust
use mailtriage::{config, domain::{EngineConfig, FilingMode, HimalayaConfig}};
use serde_json::json;
use std::fs;

fn legacy_json(dir: &std::path::Path) -> serde_json::Value {
    fs::write(dir.join("h.toml"), "[accounts.work]\nimap.server='imaps://x.test'\n").unwrap();
    let mut c = serde_json::to_value(config::default_config()).unwrap();
    c["schema_version"] = json!(1);
    c["accounts"]["work"]["himalaya"] = json!({
        "binary": "himalaya", "config": "h.toml", "account": "work",
        "mailboxes": ["INBOX"], "expected_version": "2.1.0",
        "timeout_seconds": 30, "max_output_bytes": 1000000
    });
    c["accounts"]["work"].as_object_mut().unwrap().remove("engine");
    c["accounts"]["work"].as_object_mut().unwrap().remove("filing");
    c
}

#[test]
fn legacy_himalaya_config_loads_as_engine_and_saves_as_v2() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.json");
    fs::write(&path, legacy_json(dir.path()).to_string()).unwrap();
    let loaded = config::load(&path).unwrap();
    assert_eq!(loaded.schema_version, 2);
    let a = &loaded.accounts["work"];
    assert!(a.himalaya.is_none());
    assert!(matches!(a.engine, Some(EngineConfig::Himalaya(ref h)) if h.mailboxes == ["INBOX"]));
    assert_eq!(a.filing.mode, FilingMode::Off);
    config::save(&path, &loaded).unwrap();
    let on_disk: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(on_disk["schema_version"], 2);
    assert_eq!(on_disk["accounts"]["work"]["engine"]["kind"], "himalaya");
    assert!(on_disk["accounts"]["work"].get("himalaya").is_none());
    assert_eq!(on_disk["accounts"]["work"]["filing"]["mode"], "off");
}

#[test]
fn both_engine_and_legacy_himalaya_is_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.json");
    let mut c = legacy_json(dir.path());
    c["accounts"]["work"]["engine"] = json!({"kind": "himalaya", "binary": "himalaya", "config": "h.toml", "account": "work", "mailboxes": ["INBOX"], "expected_version": "2.1.0", "timeout_seconds": 30, "max_output_bytes": 1000000});
    fs::write(&path, c.to_string()).unwrap();
    assert!(config::load(&path).is_err());
}

#[test]
fn legacy_category_names_load_while_filing_is_off_and_fail_when_on() {
    let mut c = config::default_config();
    c.accounts.get_mut("work").unwrap().categories[1].name = "Receipts / Orders".into();
    config::validate(&c).unwrap();
    c.accounts.get_mut("work").unwrap().filing.mode = FilingMode::DryRun;
    let err = config::validate(&c).unwrap_err().to_string();
    assert!(err.contains("folder"), "{err}");
    c.accounts.get_mut("work").unwrap().categories[1].folder = Some("Receipts".into());
    config::validate(&c).unwrap();
}

#[test]
fn folder_rules_apply_when_filing_is_on() {
    let mut c = config::default_config();
    let a = c.accounts.get_mut("work").unwrap();
    a.filing.mode = FilingMode::Live;
    for bad in ["", " News", "News ", "a/b", "a.b", "x*", "x%", "q\"", "b\\s", "a&b", "tab\there", "Inbox", "inbox", "Grüße"] {
        c.accounts.get_mut("work").unwrap().categories[0].folder = Some(bad.into());
        assert!(config::validate(&c).is_err(), "accepted {bad:?}");
    }
    c.accounts.get_mut("work").unwrap().categories[0].folder = None;
    let a = c.accounts.get_mut("work").unwrap();
    a.categories[0].folder = Some("INBOX".into());
    a.categories[1].folder = Some("INBOX".into());
    config::validate(&c).unwrap();
    let a = c.accounts.get_mut("work").unwrap();
    a.categories[2].folder = Some("Bills and Receipts".into());
    a.categories[3].folder = Some("bills and receipts".into());
    assert!(config::validate(&c).is_err(), "case-insensitive duplicate accepted");
    c.accounts.get_mut("work").unwrap().categories[3].folder = Some("Newsletters".into());
    config::validate(&c).unwrap();
    c.accounts.get_mut("work").unwrap().filing.max_actions_per_pass = 0;
    assert!(config::validate(&c).is_err());
}

#[test]
fn himalaya_struct_field_still_works_for_existing_callers() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("h.toml"), "[accounts.work]\nimap.server='imaps://x.test'\n").unwrap();
    let mut c = config::default_config();
    c.accounts.get_mut("work").unwrap().himalaya = Some(HimalayaConfig {
        binary: "himalaya".into(), config: dir.path().join("h.toml"), account: "work".into(),
        mailboxes: vec!["INBOX".into()], expected_version: "2.1.0".into(),
        timeout_seconds: 3, max_output_bytes: 1000,
    });
    let path = dir.path().join("c.json");
    config::save(&path, &c).unwrap();
    assert!(config::load(&path).unwrap().accounts["work"].engine.is_some());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --locked --test config_v2 && cargo test --locked --lib golden`
Expected: compile errors (`FilingMode`, `normalize`, `engine` field, `binding_identity` arity are not defined).

- [ ] **Step 3: Implement domain and config changes**

In `src/domain.rs` add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum EngineConfig {
    Himalaya(HimalayaConfig),
}
impl EngineConfig {
    pub fn mailboxes(&self) -> &[String] {
        match self {
            EngineConfig::Himalaya(h) => &h.mailboxes,
        }
    }
    pub fn timeout_seconds(&self) -> u64 {
        match self {
            EngineConfig::Himalaya(h) => h.timeout_seconds,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FilingMode {
    #[default]
    Off,
    DryRun,
    Live,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FilingConfig {
    #[serde(default)]
    pub mode: FilingMode,
    #[serde(default = "default_true")]
    pub flag: bool,
    #[serde(default = "default_max_actions")]
    pub max_actions_per_pass: usize,
}
impl Default for FilingConfig {
    fn default() -> Self {
        Self { mode: FilingMode::Off, flag: true, max_actions_per_pass: 200 }
    }
}
fn default_true() -> bool {
    true
}
fn default_max_actions() -> usize {
    200
}
```

Change `AccountConfig`:

```rust
pub struct AccountConfig {
    pub identity: String,
    pub timezone: String,
    pub brief: String,
    pub taxonomy_revision: u64,
    pub categories: Vec<Category>,
    /// Schema 1 location of the Himalaya settings. Read on load, moved into
    /// `engine` by `config::normalize`, never serialized.
    #[serde(default, skip_serializing)]
    pub himalaya: Option<HimalayaConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<EngineConfig>,
    #[serde(default)]
    pub filing: FilingConfig,
}
impl AccountConfig {
    pub fn engine_config(&self) -> Option<EngineConfig> {
        self.engine
            .clone()
            .or_else(|| self.himalaya.clone().map(EngineConfig::Himalaya))
    }
}
```

Add `#[serde(default, skip_serializing_if = "Option::is_none")] pub folder: Option<String>` to `Category` and:

```rust
impl Category {
    pub fn effective_folder(&self) -> &str {
        self.folder.as_deref().unwrap_or(&self.name)
    }
}
```

Derive `Default` on `SourceEnvelope` and add `#[serde(default)] pub message_id: Option<String>`, `#[serde(default)] pub internal_date: Option<String>`, `#[serde(default)] pub size: Option<u64>`, `#[serde(default)] pub flags: Vec<String>`. Add `..Default::default()` to the two `SourceEnvelope` literals in `tests/storage.rs` and to the one in `src/himalaya.rs`'s `discover`.

In `src/config.rs`:
- `default_config()`: `schema_version: 2`, add `folder: None` to each category, `engine: None, filing: FilingConfig::default()` to the account. Keep `himalaya: None`.
- Add:

```rust
/// Moves the legacy `himalaya` block into `engine` and marks the config as schema 2.
pub fn normalize(config: &mut AppConfig) -> Result<()> {
    for (name, account) in config.accounts.iter_mut() {
        if let Some(h) = account.himalaya.take() {
            if account.engine.is_some() {
                bail!("account {name}: configure either engine or the legacy himalaya block, not both");
            }
            account.engine = Some(EngineConfig::Himalaya(h));
        }
    }
    config.schema_version = 2;
    Ok(())
}
```

- `load`: parse, then `normalize(&mut config)?`, then `validate(&config)?`.
- `save`: `let mut config = config.clone(); normalize(&mut config)?; validate(&config)?;` then write `config`.
- `validate`: accept `schema_version` 1 or 2. Validate the engine through `account.engine_config()`; reject an account with both `himalaya` and `engine` set. Keep all existing Himalaya field checks, applied to `EngineConfig::Himalaya(h)`. Add `filing.max_actions_per_pass` in `1..=1000`. When `filing.mode != FilingMode::Off`, apply the folder rules from the spec's Configuration section to every category's `effective_folder()`:

```rust
fn valid_folder_name(folder: &str) -> bool {
    folder == "INBOX"
        || (!folder.is_empty()
            && folder.len() <= 200
            && folder.trim() == folder
            && !folder.eq_ignore_ascii_case("inbox")
            && folder.chars().all(|c| (' '..='~').contains(&c))
            && !folder.contains(['/', '.', '*', '%', '"', '\\', '&']))
}
```

For the error message use `account {name}: category {id:?} needs a valid folder` so it contains `folder`. Check duplicate non-`INBOX` folders with a case-insensitive set.

- [ ] **Step 4: Create the engine module and move Himalaya**

`git mv src/himalaya.rs src/engine/himalaya.rs`. Create `src/engine/mod.rs`:

```rust
//! The boundary between mailtriage and a mailbox. The service only talks to
//! `MailEngine`; Himalaya is one implementation, a native IMAP engine can be
//! another. No method can delete, expunge, unflag, change \Seen, or delete or
//! rename folders.
pub mod himalaya;

use crate::domain::{EngineConfig, MailboxSnapshot, SourceEnvelope};
use anyhow::Result;
use std::rc::Rc;

pub trait MailEngine {
    fn version(&self) -> Result<String>;
    /// Stable JSON describing server and login identity, never secrets.
    fn binding_identity(&self) -> Result<serde_json::Value>;
    fn snapshot(&self, folder: &str) -> Result<MailboxSnapshot>;
    /// Envelopes for UIDs in (after, through].
    fn discover(&self, folder: &str, after: u64, through: u64) -> Result<Vec<SourceEnvelope>>;
    /// Raw RFC 5322 bytes; never sets \Seen.
    fn fetch_raw(&self, folder: &str, uid: u64) -> Result<Vec<u8>>;
}

pub fn open(config: &EngineConfig) -> Result<Rc<dyn MailEngine>> {
    match config {
        EngineConfig::Himalaya(h) => Ok(Rc::new(himalaya::Himalaya::new(h)?)),
    }
}
```

In `src/engine/himalaya.rs`, move `source_binding` and `scrub_secrets` from `service.rs` unchanged (they still build `ServiceError` with code 2 via `crate::service::ServiceError { code: 2, message }`). Implement the trait by delegating to the inherent methods:

```rust
impl MailEngine for Himalaya {
    fn version(&self) -> Result<String> {
        Himalaya::version(self)
    }
    fn binding_identity(&self) -> Result<serde_json::Value> {
        source_binding(&self.config)
    }
    fn snapshot(&self, folder: &str) -> Result<MailboxSnapshot> {
        Himalaya::snapshot(self, folder)
    }
    fn discover(&self, folder: &str, after: u64, through: u64) -> Result<Vec<SourceEnvelope>> {
        Himalaya::discover(self, folder, after, through)
    }
    fn fetch_raw(&self, folder: &str, uid: u64) -> Result<Vec<u8>> {
        self.fetch(folder, uid)
    }
}
```

In `src/lib.rs` replace `pub mod himalaya;` with:

```rust
pub mod engine;
/// Compatibility path for `mailtriage::himalaya::Himalaya`.
pub mod himalaya {
    pub use crate::engine::himalaya::*;
}
```

- [ ] **Step 5: Route the service through the trait**

In `src/service.rs`:
- Add the field `engine_override: Option<Rc<dyn MailEngine>>` to `Service` (`None` from `open`).
- Add:

```rust
fn engine(&self, account: &AccountConfig) -> Result<Option<Rc<dyn MailEngine>>> {
    match (&self.engine_override, account.engine_config()) {
        (_, None) => Ok(None),
        (Some(engine), Some(_)) => Ok(Some(Rc::clone(engine))),
        (None, Some(cfg)) => Ok(Some(crate::engine::open(&cfg)?)),
    }
}
```

- Change the free function to `fn binding_identity(account: &AccountConfig, engine: Option<&dyn MailEngine>) -> Result<String>`. `source` is `engine.binding_identity()` when an engine is given and the account has an engine config, otherwise `source_binding(h)` for `EngineConfig::Himalaya(h)`, otherwise `null`. The hashed JSON stays exactly `{"identity": .., "source": ..}`. Callers pass `self.engine_override.as_deref()`.
- Replace every `account.himalaya` use (`doctor`, `process_one` lease seconds, `sync`, `coverage`, `reclassify`, `resolve_paths`) with `account.engine_config()` or `self.engine(&account)?`. `process_one` takes `source: Option<&dyn MailEngine>` and calls `fetch_raw`. `resolve_paths` resolves paths inside `EngineConfig::Himalaya`.
- `sync` iterates `cfg.mailboxes()` of the engine config.

- [ ] **Step 6: Run the full suite**

Run: `cargo fmt && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`
Expected: all tests pass, including the two golden tests and `tests/config_v2.rs`. If `binding_identity_is_unchanged_for_legacy_himalaya_accounts` fails, the hashed JSON changed shape. Fix the code, never the expected value.

- [ ] **Step 7: Commit**

```bash
git add -A src tests
git commit -m "Move Himalaya behind a MailEngine trait and add config schema v2

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Himalaya engine operations and the raw IMAP parser

Completes the trait (capabilities, folders, envelopes, writes) and implements it for Himalaya, with a strict raw-response parser and exact-argument contract tests.

**Files:**
- Create: `src/engine/raw.rs`
- Modify: `src/engine/mod.rs`, `src/engine/himalaya.rs`, `tests/adapters.rs` (match patterns only)
- Test: unit tests in `src/engine/raw.rs`; `tests/engine_contract.rs`

**Interfaces:**
- Consumes: Task 1 trait and types.
- Produces (in `src/engine/mod.rs`):

```rust
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EngineCapabilities {
    pub move_supported: bool,
    pub uidplus: bool,
    pub special_use: bool,
    pub delimiter: Option<char>,
    pub personal_prefix: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderInfo {
    pub name: String,
    pub attributes: Vec<String>,
    pub roles: Option<Vec<String>>,
    pub subscribed: bool,
}
pub use raw::CopyUid;
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WriteOutcome {
    pub selected: bool,
    pub session_epoch: Option<u64>,
    pub completed: bool,
    pub copyuid: Option<CopyUid>,
}
/// The engine refused to run because its external configuration changed.
#[derive(Debug)]
pub struct ConfigChanged;
impl std::fmt::Display for ConfigChanged { /* "mail engine configuration changed during operation" */ }
impl std::error::Error for ConfigChanged {}

pub const SPECIAL_USE_ROLES: [&str; 7] = ["\\Sent", "\\Trash", "\\Drafts", "\\Junk", "\\Archive", "\\All", "\\Flagged"];

pub trait MailEngine {
    // Task 1 methods, plus:
    fn capabilities(&self) -> Result<EngineCapabilities>;
    fn list_folders(&self) -> Result<Vec<FolderInfo>>;
    fn create_folder(&self, native: &str) -> Result<()>;
    fn subscribe_folder(&self, native: &str) -> Result<()>;
    /// Envelopes for exactly these UIDs; absent UIDs are omitted.
    fn envelopes(&self, folder: &str, uids: &[u64]) -> Result<Vec<SourceEnvelope>>;
    /// One session: SELECT folder; UID MOVE uids target.
    fn move_messages(&self, folder: &str, uids: &[u64], target: &str) -> Result<WriteOutcome>;
    /// One session: SELECT folder; UID STORE uids +FLAGS.SILENT (\Flagged).
    fn add_flagged(&self, folder: &str, uids: &[u64]) -> Result<WriteOutcome>;
    /// Watched folder names that a client-side alias would resolve elsewhere.
    fn alias_conflicts(&self, _folders: &[String]) -> Result<Vec<String>> {
        Ok(vec![])
    }
}
```

- `engine::raw::{parse, RawResponse, Completion, CopyUid, ListLine, quote_mailbox, uid_set, parse_uid_set}`.

- [ ] **Step 1: Write the raw parser unit tests**

Create `src/engine/raw.rs` with this test module at the bottom (implementation in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_and_move_with_copyuid() {
        let out = b"* 3 EXISTS\r\n* OK [UIDVALIDITY 7] UIDs valid\r\na1 OK [READ-WRITE] done\r\n* OK [COPYUID 9 4,5 20:21] moved\r\n* 1 EXPUNGE\r\n* 1 EXPUNGE\r\na2 OK MOVE done\r\n";
        let r = parse(out);
        assert_eq!(r.completion("a1"), Some(Completion::Ok));
        assert_eq!(r.completion("a2"), Some(Completion::Ok));
        assert_eq!(r.uidvalidity, Some(7));
        assert_eq!(r.copyuid, Some(CopyUid { target_epoch: 9, pairs: vec![(4, 20), (5, 21)] }));
    }

    #[test]
    fn copyuid_in_tagged_response_and_partial_no() {
        let r = parse(b"* OK [UIDVALIDITY 7] x\r\na1 OK x\r\na2 NO [COPYUID 9 4 30] partial\r\n");
        assert_eq!(r.completion("a2"), Some(Completion::No));
        assert_eq!(r.copyuid.unwrap().pairs, vec![(4, 30)]);
    }

    #[test]
    fn multiple_copyuid_responses_accumulate() {
        let r = parse(b"* OK [COPYUID 9 1:2 10:11] a\r\n* OK [COPYUID 9 5 12] b\r\na2 OK\r\n");
        assert_eq!(r.copyuid.unwrap().pairs, vec![(1, 10), (2, 11), (5, 12)]);
    }

    #[test]
    fn mismatched_copyuid_sets_are_ignored() {
        assert!(parse(b"* OK [COPYUID 9 1:3 10:11] bad\r\n").copyuid.is_none());
    }

    #[test]
    fn bad_completion_and_missing_uidvalidity() {
        let r = parse(b"a1 BAD no mailbox\r\na2 BAD no mailbox selected\r\n");
        assert_eq!(r.completion("a1"), Some(Completion::Bad));
        assert_eq!(r.uidvalidity, None);
    }

    #[test]
    fn capability_and_namespace() {
        let r = parse(b"* CAPABILITY IMAP4rev1 MOVE UIDPLUS SPECIAL-USE\r\na1 OK\r\n* NAMESPACE ((\"INBOX.\" \".\")) NIL NIL\r\na2 OK\r\n");
        assert!(r.capabilities.contains(&"MOVE".to_string()));
        assert!(r.capabilities.contains(&"SPECIAL-USE".to_string()));
        assert_eq!(r.personal_namespace, Some(("INBOX.".to_string(), Some('.'))));
        let r = parse(b"* NAMESPACE NIL NIL NIL\r\n");
        assert_eq!(r.personal_namespace, None);
        let r = parse(b"* NAMESPACE ((\"\" \"/\")) NIL NIL\r\n");
        assert_eq!(r.personal_namespace, Some((String::new(), Some('/'))));
    }

    #[test]
    fn list_lines_quoted_atom_nil_and_literal() {
        let r = parse(b"* LIST (\\HasNoChildren \\Sent) \"/\" \"Sent Items\"\r\n* LIST () \".\" INBOX\r\n* LIST (\\Noselect) NIL \"\"\r\n* LIST () \"/\" {5}\r\nHello\r\na1 OK\r\n");
        assert_eq!(r.list[0], ListLine { attributes: vec!["\\HasNoChildren".into(), "\\Sent".into()], delimiter: Some('/'), name: "Sent Items".into() });
        assert_eq!(r.list[1].name, "INBOX");
        assert_eq!(r.list[2].delimiter, None);
        assert_eq!(r.list_errors, 1);
    }

    #[test]
    fn quoting_and_uid_sets() {
        assert_eq!(quote_mailbox("Bills and Receipts").unwrap(), "\"Bills and Receipts\"");
        assert!(quote_mailbox("A&B").is_err(), "& is the modified UTF-7 shift character");
        assert!(quote_mailbox("Grüße").is_err());
        assert!(quote_mailbox("a\r\nb").is_err());
        assert!(quote_mailbox("").is_err());
        assert_eq!(uid_set(&[4, 5, 9]), "4,5,9");
        assert_eq!(parse_uid_set("1:3,7").unwrap(), vec![1, 2, 3, 7]);
        assert_eq!(parse_uid_set("3:1").unwrap(), vec![1, 2, 3]);
        assert!(parse_uid_set("1:200000").is_err());
        assert!(parse_uid_set("x").is_err());
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --locked --lib engine::raw`
Expected: compile errors (functions not defined).

- [ ] **Step 3: Implement the parser**

Top of `src/engine/raw.rs`:

```rust
//! Strict, minimal parser for `himalaya imap raw` output. It reads tagged
//! completions, UIDVALIDITY, COPYUID, CAPABILITY, NAMESPACE and LIST lines and
//! ignores everything else.
use anyhow::{bail, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completion {
    Ok,
    No,
    Bad,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyUid {
    pub target_epoch: u64,
    pub pairs: Vec<(u64, u64)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListLine {
    pub attributes: Vec<String>,
    pub delimiter: Option<char>,
    pub name: String,
}

#[derive(Debug, Clone, Default)]
pub struct RawResponse {
    pub tagged: Vec<(String, Completion)>,
    pub uidvalidity: Option<u64>,
    pub copyuid: Option<CopyUid>,
    pub capabilities: Vec<String>,
    pub personal_namespace: Option<(String, Option<char>)>,
    pub list: Vec<ListLine>,
    pub list_errors: usize,
}

impl RawResponse {
    pub fn completion(&self, tag: &str) -> Option<Completion> {
        self.tagged.iter().find(|(t, _)| t == tag).map(|(_, c)| *c)
    }
}

const MAX_UID_SET: usize = 10_000;

pub fn parse(output: &[u8]) -> RawResponse {
    let text = String::from_utf8_lossy(output);
    let mut r = RawResponse::default();
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("* ") {
            untagged(rest, &mut r);
        } else if let Some((tag, rest)) = line.split_once(' ') {
            if tag.is_empty() || tag == "+" || !tag.chars().all(|c| c.is_ascii_alphanumeric()) {
                continue;
            }
            let (status, tail) = rest.split_once(' ').unwrap_or((rest, ""));
            let completion = match status.to_ascii_uppercase().as_str() {
                "OK" => Completion::Ok,
                "NO" => Completion::No,
                "BAD" => Completion::Bad,
                _ => continue,
            };
            r.tagged.push((tag.to_string(), completion));
            response_code(tail, &mut r);
        }
    }
    r
}

fn untagged(rest: &str, r: &mut RawResponse) {
    let upper = rest.to_ascii_uppercase();
    for prefix in ["OK ", "NO ", "BAD "] {
        if upper.starts_with(prefix) {
            response_code(&rest[prefix.len()..], r);
            return;
        }
    }
    if let Some(caps) = upper.strip_prefix("CAPABILITY ") {
        r.capabilities = caps.split_ascii_whitespace().map(str::to_string).collect();
    } else if upper.starts_with("NAMESPACE ") {
        r.personal_namespace = namespace(&rest["NAMESPACE ".len()..]);
    } else if upper.starts_with("LIST ") {
        match list_line(&rest["LIST ".len()..]) {
            Some(line) => r.list.push(line),
            None => r.list_errors += 1,
        }
    }
}

fn response_code(tail: &str, r: &mut RawResponse) {
    let Some(start) = tail.find('[') else { return };
    let Some(len) = tail[start..].find(']') else { return };
    let mut parts = tail[start + 1..start + len].split_ascii_whitespace();
    match parts.next().map(str::to_ascii_uppercase).as_deref() {
        Some("UIDVALIDITY") => r.uidvalidity = parts.next().and_then(|v| v.parse().ok()),
        Some("CAPABILITY") => {
            r.capabilities = parts.map(str::to_ascii_uppercase).collect();
        }
        Some("COPYUID") => {
            let (Some(epoch), Some(src), Some(dst)) = (parts.next(), parts.next(), parts.next()) else {
                return;
            };
            let (Ok(epoch), Ok(src), Ok(dst)) = (epoch.parse::<u64>(), parse_uid_set(src), parse_uid_set(dst)) else {
                return;
            };
            if src.len() != dst.len() {
                return;
            }
            let pairs = src.into_iter().zip(dst);
            match &mut r.copyuid {
                Some(existing) if existing.target_epoch == epoch => existing.pairs.extend(pairs),
                Some(_) => {}
                None => r.copyuid = Some(CopyUid { target_epoch: epoch, pairs: pairs.collect() }),
            }
        }
        _ => {}
    }
}

/// Parses a quoted string with `\"` and `\\` escapes; returns it and the rest.
fn quoted(s: &str) -> Option<(String, &str)> {
    let body = s.strip_prefix('"')?;
    let mut out = String::new();
    let mut escaped = false;
    for (i, c) in body.char_indices() {
        if escaped {
            out.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            return Some((out, &body[i + 1..]));
        } else {
            out.push(c);
        }
    }
    None
}

fn namespace(s: &str) -> Option<(String, Option<char>)> {
    let s = s.trim_start();
    let first = s.strip_prefix("((")?;
    let (prefix, rest) = quoted(first.trim_start())?;
    let rest = rest.trim_start();
    let delimiter = if rest.starts_with("NIL") { None } else { quoted(rest)?.0.chars().next() };
    Some((prefix, delimiter))
}

fn list_line(s: &str) -> Option<ListLine> {
    let s = s.trim();
    let inner = s.strip_prefix('(')?;
    let close = inner.find(')')?;
    let attributes = inner[..close].split_ascii_whitespace().map(str::to_string).collect();
    let rest = inner[close + 1..].trim_start();
    let (delimiter, rest) = match rest.strip_prefix("NIL") {
        Some(rest) => (None, rest),
        None => {
            let (d, rest) = quoted(rest)?;
            (d.chars().next(), rest)
        }
    };
    let rest = rest.trim();
    let name = if rest.starts_with('"') {
        quoted(rest)?.0
    } else if rest.starts_with('{') || rest.is_empty() {
        return None;
    } else {
        rest.split_ascii_whitespace().next()?.to_string()
    };
    Some(ListLine { attributes, delimiter, name })
}

pub fn quote_mailbox(name: &str) -> Result<String> {
    if name.is_empty() || !name.chars().all(|c| (' '..='~').contains(&c)) || name.contains('&') {
        bail!("mailbox names must be nonempty printable ASCII without '&'");
    }
    Ok(format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\"")))
}

pub fn uid_set(uids: &[u64]) -> String {
    uids.iter().map(u64::to_string).collect::<Vec<_>>().join(",")
}

pub fn parse_uid_set(s: &str) -> Result<Vec<u64>> {
    let mut out = Vec::new();
    for part in s.split(',') {
        let (a, b) = part.split_once(':').unwrap_or((part, part));
        let (a, b): (u64, u64) = (a.parse()?, b.parse()?);
        let (lo, hi) = (a.min(b), a.max(b));
        if (hi - lo) as usize + out.len() >= MAX_UID_SET {
            bail!("UID set too large");
        }
        out.extend(lo..=hi);
    }
    Ok(out)
}
```

Add `pub mod raw;` to `src/engine/mod.rs` and the new types and trait methods from the Interfaces block.

- [ ] **Step 4: Run the parser tests**

Run: `cargo test --locked --lib engine::raw`
Expected: PASS.

- [ ] **Step 5: Write the Himalaya contract tests**

Create `tests/engine_contract.rs`. The fake binary logs every argv as a JSON line and answers by command:

```rust
#![cfg(unix)]
use mailtriage::{
    domain::HimalayaConfig,
    engine::{himalaya::Himalaya, ConfigChanged, MailEngine},
};
use serde_json::Value;
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};
use tempfile::TempDir;

const SCRIPT: &str = r#"#!/usr/bin/env python3
import sys, json, os, time
a = sys.argv[1:]
open(os.environ["MT_LOG"], "a").write(json.dumps(a) + "\n")
i = a.index("--backend") + 2
cmd = a[i:]
if cmd and cmd[0] == "--json": cmd = cmd[1:]
mode = os.environ.get("MT_MODE", "")
def out(s): sys.stdout.write(s); sys.stdout.flush()
if a[-1] == "--version": out("himalaya v2.1.0 +imap\n")
elif cmd[:2] == ["imap", "raw"]:
    text = cmd[-1]
    if "CAPABILITY" in text:
        out('* CAPABILITY IMAP4rev1 MOVE UIDPLUS SPECIAL-USE\r\na1 OK done\r\n* NAMESPACE (("" "/")) NIL NIL\r\na2 OK done\r\n')
    elif "RETURN (SPECIAL-USE)" in text:
        out('* LIST (\\HasNoChildren) "/" "INBOX"\r\n* LIST (\\HasNoChildren \\Sent) "/" "Sent"\r\n* LIST () "/" "Newsletters"\r\na1 OK\r\n')
    elif "UID MOVE" in text:
        if mode == "timeout":
            out("* OK [UIDVALIDITY 7] ok\r\na1 OK done\r\n"); time.sleep(5)
        out("* OK [UIDVALIDITY 7] ok\r\na1 OK done\r\n* OK [COPYUID 9 4,5 20:21] moved\r\na2 OK done\r\n")
    elif "UID STORE" in text:
        out("* OK [UIDVALIDITY 7] ok\r\na1 OK done\r\na2 OK done\r\n")
    else: sys.exit(5)
elif cmd[:2] == ["imap", "list"]:
    rows = [{"name": "INBOX", "delimiter": "/", "attributes": []},
            {"name": "Sent", "delimiter": "/", "attributes": ["\\Sent"]},
            {"name": "Newsletters", "delimiter": "/", "attributes": []}]
    if "--all" not in cmd: rows = rows[:2]
    out(json.dumps(rows if mode == "array" else {"preset": "x", "mailboxes": rows}))
elif cmd[:2] in (["imap", "create"], ["imap", "subscribe"]): out("ok\n")
elif cmd[:2] == ["imap", "fetch"]:
    out(json.dumps({"messages": [{"uid": 4, "flags": ["\\Seen", "\\Flagged"], "internal_date": "2026-10-04T08:00:00+00:00", "size": 1234,
        "envelope": {"subject": "Hi", "from": ["A <a@x.test>"], "date": None, "message_id": "<m1@x.test>"}}]}))
elif cmd[:2] == ["imap", "status"]: out('{"uid_validity":7,"uid_next":10}')
else: sys.exit(7)
"#;

struct Fixture {
    _dir: TempDir,
    log: PathBuf,
    toml: PathBuf,
    engine: Himalaya,
}

fn fixture(mode: &str, timeout: u64) -> Fixture {
    let dir = TempDir::new().unwrap();
    let binary = dir.path().join("himalaya");
    fs::write(&binary, SCRIPT).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    let toml = dir.path().join("h.toml");
    fs::write(&toml, "[accounts.work]\nimap.server='imaps://x.test'\n[accounts.work.mailbox.alias]\nnewsletters = \"Lists/News\"\ninbox = \"INBOX\"\n").unwrap();
    let log = dir.path().join("log");
    std::env::set_var("MT_LOG", &log);
    std::env::set_var("MT_MODE", mode);
    let engine = Himalaya::new(&HimalayaConfig {
        binary, config: toml.clone(), account: "work".into(),
        mailboxes: vec!["INBOX".into()], expected_version: "2.1.0".into(),
        timeout_seconds: timeout, max_output_bytes: 100_000,
    }).unwrap();
    Fixture { _dir: dir, log, toml, engine }
}

fn calls(f: &Fixture) -> Vec<Vec<String>> {
    fs::read_to_string(&f.log).unwrap_or_default().lines()
        .map(|l| serde_json::from_str::<Vec<String>>(l).unwrap()).collect()
}

fn tail(call: &[String]) -> Vec<String> {
    let i = call.iter().position(|a| a == "--backend").unwrap() + 2;
    call[i..].to_vec()
}

fn assert_no_forbidden(f: &Fixture) {
    for call in calls(f) {
        let joined = call.join(" ");
        for bad in ["expunge", "EXPUNGE", "delete", "DELETE", "rename", "RENAME", "--seq", "--seen", "-FLAGS", "--action"] {
            assert!(!joined.contains(bad), "forbidden {bad:?} in {joined:?}");
        }
    }
}
```

The fixtures set process-wide environment variables, so run this file's tests serially: put them in one `#[test] fn contract()` that calls helper functions in sequence, or add `--test-threads=1` to the file's run command and document it in the file header. **Use the single-test approach**, so a plain `cargo test` works. Test bodies (each is a function called from `contract()`):

```rust
fn move_quotes_names_with_spaces() {
    let f = fixture("", 5);
    let out = f.engine.move_messages("INBOX", &[4, 5], "Bills and Receipts").unwrap();
    assert!(out.selected && out.completed);
    assert_eq!(out.session_epoch, Some(7));
    assert_eq!(out.copyuid.unwrap().pairs, vec![(4, 20), (5, 21)]);
    let last = calls(&f).pop().unwrap();
    assert_eq!(tail(&last), vec!["imap", "raw", "--", "a1 SELECT \"INBOX\"\r\na2 UID MOVE 4,5 \"Bills and Receipts\"\r\n"]);
    assert_no_forbidden(&f);
}

fn flag_store_text() {
    let f = fixture("", 5);
    let out = f.engine.add_flagged("INBOX", &[4]).unwrap();
    assert!(out.selected && out.completed && out.copyuid.is_none());
    let last = calls(&f).pop().unwrap();
    assert_eq!(tail(&last), vec!["imap", "raw", "--", "a1 SELECT \"INBOX\"\r\na2 UID STORE 4 +FLAGS.SILENT (\\Flagged)\r\n"]);
    assert_no_forbidden(&f);
}

fn timeout_keeps_partial_select_result() {
    let f = fixture("timeout", 1);
    let out = f.engine.move_messages("INBOX", &[4], "News").unwrap();
    assert!(out.selected);
    assert_eq!(out.session_epoch, Some(7));
    assert!(!out.completed);
}

fn capabilities_and_namespace() {
    let f = fixture("", 5);
    let caps = f.engine.capabilities().unwrap();
    assert!(caps.move_supported && caps.uidplus && caps.special_use);
    assert_eq!(caps.personal_prefix, "");
    assert_eq!(caps.delimiter, Some('/'));
    let last = calls(&f).pop().unwrap();
    assert_eq!(tail(&last), vec!["imap", "raw", "--", "a1 CAPABILITY\r\na2 NAMESPACE\r\n"]);
}

fn list_folders_accepts_object_or_array_json() {
    for mode in ["", "array"] {
        let f = fixture(mode, 5);
        let folders = f.engine.list_folders().unwrap();
        let sent = folders.iter().find(|x| x.name == "Sent").unwrap();
        assert_eq!(sent.roles.as_deref(), Some(&["\\Sent".to_string()][..]));
        assert!(sent.subscribed);
        let news = folders.iter().find(|x| x.name == "Newsletters").unwrap();
        assert_eq!(news.roles.as_deref(), Some(&[][..]));
        assert!(!news.subscribed);
    }
}

fn create_and_subscribe_args() {
    let f = fixture("", 5);
    f.engine.create_folder("Bills and Receipts").unwrap();
    f.engine.subscribe_folder("Bills and Receipts").unwrap();
    assert!(f.engine.create_folder("A&B").is_err());
    let c = calls(&f);
    assert_eq!(tail(&c[c.len() - 2]), vec!["imap", "create", "Bills and Receipts"]);
    assert_eq!(tail(&c[c.len() - 1]), vec!["imap", "subscribe", "Bills and Receipts"]);
}

fn envelopes_parse_new_fields_and_args() {
    let f = fixture("", 5);
    let env = f.engine.envelopes("INBOX", &[4, 9]).unwrap();
    assert_eq!(env[0].message_id.as_deref(), Some("<m1@x.test>"));
    assert_eq!(env[0].size, Some(1234));
    assert_eq!(env[0].internal_date.as_deref(), Some("2026-10-04T08:00:00+00:00"));
    assert!(env[0].flags.iter().any(|x| x == "\\Flagged"));
    let last = calls(&f).pop().unwrap();
    assert_eq!(tail(&last), vec!["--json", "imap", "fetch", "--mailbox", "INBOX", "--envelope", "--flags", "--internal-date", "--size", "4,9"]);
}

fn config_change_is_refused() {
    let f = fixture("", 5);
    f.engine.capabilities().unwrap();
    fs::write(&f.toml, "[accounts.work]\nimap.server='imaps://other.test'\n").unwrap();
    // capabilities() is cached; list_folders() must spawn and therefore re-check the TOML.
    let err = f.engine.list_folders().unwrap_err();
    assert!(err.downcast_ref::<ConfigChanged>().is_some());
}

fn alias_conflicts_detected() {
    let f = fixture("", 5);
    let conflicts = f.engine.alias_conflicts(&["Newsletters".into(), "INBOX".into(), "Receipts".into()]).unwrap();
    assert_eq!(conflicts, vec!["Newsletters".to_string()]);
}

#[test]
fn contract() {
    move_quotes_names_with_spaces();
    flag_store_text();
    timeout_keeps_partial_select_result();
    capabilities_and_namespace();
    list_folders_accepts_object_or_array_json();
    create_and_subscribe_args();
    envelopes_parse_new_fields_and_args();
    config_change_is_refused();
    alias_conflicts_detected();
}
```

`tail` keeps a leading `--json` for JSON commands, which is why the envelope assertion includes it. Non-JSON commands start directly with `imap`.

- [ ] **Step 6: Run to verify failure**

Run: `cargo test --locked --test engine_contract`
Expected: compile errors (methods missing).

- [ ] **Step 7: Implement the Himalaya operations**

In `src/engine/himalaya.rs`:

1. `Himalaya` gains `config_hash: String` (SHA-256 hex of the TOML bytes read in `new`) and `caps: std::cell::OnceCell<EngineCapabilities>`.
2. Split `run` into `fn spawn_capture(&self, args: &[&str], json: bool) -> Result<Captured>`, where `struct Captured { stdout: Vec<u8>, timed_out: bool, success: bool }`. Before spawning, re-read the TOML, hash it, and return `Err(ConfigChanged.into())` on mismatch. Collect stdout through an `mpsc::channel` fed by the reader thread. After a timeout or overflow, kill the process group as today, then `recv_timeout(Duration::from_secs(1))` for whatever was read. `run` keeps its current contract (error on timeout, overflow or non-zero status) built on `spawn_capture`.
3. `discover` and the new `envelopes` share `fn fetch_envelopes(&self, folder: &str, set: &str) -> Result<Vec<SourceEnvelope>>`. It calls `run(&["imap", "fetch", "--mailbox", folder, "--envelope", "--flags", "--internal-date", "--size", set], true)` and parses `uid`, `flags` (array of strings, default empty), `internal_date`, `size`, `envelope.message_id`, plus the existing subject/from/date handling. `envelopes` chunks UIDs by 100 and joins them with `raw::uid_set`. `discover` keeps its range form `lo:hi`.
4. Update the three shell patterns in `tests/adapters.rs` from `--envelope 42:43` / `--envelope 1:2` / `--envelope 1:1` to `--envelope --flags --internal-date --size 42:43` (and likewise). `tests/sync.rs` reads the range as `a[-1]` and needs no change.
5. `capabilities`: `get_or_try_init` on `caps` running `raw_text("a1 CAPABILITY\r\na2 NAMESPACE\r\n")` and mapping `MOVE`, `UIDPLUS` and `SPECIAL-USE` membership, plus `personal_namespace` (prefix and delimiter; prefix `""` when `None`). `OnceCell::get_or_try_init` is unstable, so use `if let Some(c) = self.caps.get() { return Ok(c.clone()) }`, compute, then `let _ = self.caps.set(c.clone())`.
6. `fn raw_text(&self, text: &str) -> Result<Captured>` runs `spawn_capture(&["imap", "raw", "--", text], false)`.
7. `list_folders`: parse `run(&["imap", "list", "--all"], true)` as either a JSON array or an object with `mailboxes`, rows `{name, delimiter, attributes}`. The subscribed set comes from `run(&["imap", "list"], true)`. If `capabilities()?.special_use`, run `raw_text("a1 LIST \"\" \"*\" RETURN (SPECIAL-USE)\r\n")` and set `roles = Some(attrs ∩ SPECIAL_USE_ROLES)` per name (`Some(vec![])` for a name listed there without roles); otherwise `roles = None`. Also merge any role found in the JSON attributes into `Some(..)` only when special_use is supported.
8. `create_folder` runs `run(&["imap", "create", native], false)`; `subscribe_folder` runs `run(&["imap", "subscribe", native], false)`. Both call `raw::quote_mailbox(native)?` first, only to enforce the ASCII rule.
9. `move_messages` / `add_flagged` build `format!("a1 SELECT {}\r\na2 UID MOVE {} {}\r\n", quote(folder)?, uid_set(uids), quote(target)?)` and `format!("a1 SELECT {}\r\na2 UID STORE {} +FLAGS.SILENT (\\Flagged)\r\n", quote(folder)?, uid_set(uids))`. They reject an empty UID list or more than 100 UIDs, and map the captured output:

```rust
fn write_outcome(captured: &Captured) -> Result<WriteOutcome> {
    let r = raw::parse(&captured.stdout);
    let selected = r.completion("a1") == Some(raw::Completion::Ok);
    if !selected && captured.timed_out {
        bail!("mail engine write produced no usable response");
    }
    if selected && r.uidvalidity.is_none() {
        bail!("SELECT response lacks UIDVALIDITY");
    }
    Ok(WriteOutcome {
        selected,
        session_epoch: r.uidvalidity,
        completed: selected && !captured.timed_out && r.completion("a2") == Some(raw::Completion::Ok),
        copyuid: r.copyuid,
    })
}
```

10. `alias_conflicts`: parse the TOML and read `accounts.<account>.mailbox.alias` (a table of string → string). A folder conflicts when an alias key equals it case-insensitively and the alias value differs from the folder name exactly.

- [ ] **Step 8: Run all tests**

Run: `cargo fmt && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`
Expected: PASS, including `tests/adapters.rs` with the updated patterns and `tests/sync.rs` unchanged.

- [ ] **Step 9: Commit**

```bash
git add -A src tests
git commit -m "Add Himalaya folder, envelope and session-bound write operations

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: FakeEngine and engine injection

An in-memory engine for service-level tests, plus `Service::open_with_engine` and a shared `Harness`.

**Files:**
- Create: `src/engine/fake.rs`, `tests/common/mod.rs`
- Modify: `src/engine/mod.rs` (`pub mod fake;`), `src/service.rs`
- Test: unit tests in `src/engine/fake.rs`; `tests/filing_observe.rs` (first test only)

**Interfaces:**
- Consumes: full `MailEngine` trait (Task 2).
- Produces:

```rust
// src/engine/fake.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FakeOp { Move, Flag, Create, Subscribe }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    ErrorBefore,     // return Err, no effect
    ErrorAfter,      // apply effect, then return Err (lost response)
    PartialCopy,     // MOVE copies but does not remove; a2 NO; COPYUID kept
    EpochRaceBefore, // reset the source folder's epoch, then execute
    NoSelect,        // a1 NO; nothing happens
}
#[derive(Clone)]
pub struct FakeEngine { /* Arc<Mutex<State>> */ }
impl FakeEngine {
    pub fn new() -> Self; // INBOX at epoch 1, MOVE+UIDPLUS+SPECIAL-USE on, prefix "", delimiter '/'
    pub fn add_folder(&self, name: &str, roles: &[&str]);
    pub fn remove_folder(&self, name: &str);
    pub fn deliver(&self, folder: &str, raw: &[u8]) -> u64;               // internal date = now
    pub fn deliver_at(&self, folder: &str, raw: &[u8], internal_date: &str) -> u64;
    pub fn client_move(&self, from: &str, uid: u64, to: &str) -> u64;     // keeps date and flags
    pub fn client_copy(&self, from: &str, uid: u64, to: &str) -> u64;
    pub fn client_delete(&self, folder: &str, uid: u64);
    pub fn client_set_flag(&self, folder: &str, uid: u64, flag: &str, on: bool);
    pub fn reset_epoch(&self, folder: &str);                              // new epoch, renumber UIDs from 1
    pub fn set_capabilities(&self, move_supported: bool, uidplus: bool, special_use: bool);
    pub fn set_prefix(&self, prefix: &str, delimiter: char);
    pub fn set_binding(&self, binding: &str);
    pub fn inject(&self, op: FakeOp, fault: Fault);                       // consumed by the next matching call
    pub fn write_calls(&self) -> usize;                                   // Move+Flag+Create+Subscribe calls
    pub fn calls(&self) -> Vec<String>;                                    // e.g. "move INBOX 1,2 -> Newsletters"
    pub fn uids(&self, folder: &str) -> Vec<u64>;
    pub fn flags(&self, folder: &str, uid: u64) -> Vec<String>;
    pub fn locate(&self, message_id: &str) -> Vec<(String, u64)>;        // all (folder, uid) holding it
    pub fn epoch(&self, folder: &str) -> u64;
    pub fn subscribed(&self, folder: &str) -> bool;
}
impl Default for FakeEngine { fn default() -> Self { Self::new() } }
impl MailEngine for FakeEngine { /* all methods */ }

// src/service.rs
impl Service {
    pub fn open_with_engine(path: &Path, engine: Rc<dyn MailEngine>) -> Result<Self>;
}

// tests/common/mod.rs
pub struct Harness { pub dir: tempfile::TempDir, pub path: std::path::PathBuf, pub fake: FakeEngine }
impl Harness {
    pub fn new(mode: FilingMode) -> Self;
    pub fn service(&self) -> Service;
    pub fn set_mode(&self, mode: FilingMode);
    pub fn edit(&self, f: impl FnOnce(&mut AppConfig));
    pub fn sync(&self) -> serde_json::Value;          // service().sync("work", 100).unwrap()
}
pub fn mail(message_id: &str, subject: &str, body: &str) -> Vec<u8>;
```

`Harness::new` writes `config.json` from `default_config()` with:
- `engine: EngineConfig::Himalaya { binary: "himalaya", config: <dir>/h.toml (written with "[accounts.work]\nimap.server='imaps://fake.test'\n"), account: "work", mailboxes: ["INBOX"], expected_version: "2.1.0", timeout_seconds: 5, max_output_bytes: 1_000_000 }`;
- `filing.mode = mode`;
- every category's `folder` set explicitly to its name, except `correspondence`, which is set to `"INBOX"` so tests cover both kinds of target;
- `review_mode` left at its default `true`, to prove review mode does not block filing.

`mail(id, subject, body)` returns `format!("Message-ID: <{id}@test>\r\nFrom: Alex <alex@example.com>\r\nTo: work@example.com\r\nSubject: {subject}\r\nContent-Type: text/plain\r\n\r\n{body}\r\n")`.

The fake provider categorizes by keywords (`src/provider.rs`): "newsletter" → newsletters, "invoice"/"receipt"/"payment" → transactions, "sale"/"discount" → promotions, "update" → updates, anything else → correspondence. "urgent"/"today"/"deadline" → high urgency. "please reply", "can you", "please review", "payment due" and "please pay" → action required.

- [ ] **Step 1: Write the fake's unit tests**

At the bottom of `src/engine/fake.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::MailEngine;

    fn raw(id: &str) -> Vec<u8> {
        format!("Message-ID: <{id}@t>\r\nSubject: s\r\nFrom: A <a@t>\r\n\r\nbody\r\n").into_bytes()
    }

    #[test]
    fn move_with_copyuid_and_epochs() {
        let f = FakeEngine::new();
        f.add_folder("News", &[]);
        let u = f.deliver("INBOX", &raw("a"));
        let out = f.move_messages("INBOX", &[u], "News").unwrap();
        assert!(out.selected && out.completed);
        assert_eq!(out.session_epoch, Some(f.epoch("INBOX")));
        let cu = out.copyuid.unwrap();
        assert_eq!(cu.target_epoch, f.epoch("News"));
        assert_eq!(f.locate("<a@t>"), vec![("News".to_string(), cu.pairs[0].1)]);
        assert_eq!(f.write_calls(), 1);
    }

    #[test]
    fn faults_and_capabilities() {
        let f = FakeEngine::new();
        f.add_folder("News", &[]);
        let u = f.deliver("INBOX", &raw("a"));
        f.inject(FakeOp::Move, Fault::ErrorAfter);
        assert!(f.move_messages("INBOX", &[u], "News").is_err());
        assert_eq!(f.locate("<a@t>")[0].0, "News");
        let v = f.deliver("INBOX", &raw("b"));
        f.inject(FakeOp::Move, Fault::PartialCopy);
        let out = f.move_messages("INBOX", &[v], "News").unwrap();
        assert!(!out.completed);
        assert_eq!(f.locate("<b@t>").len(), 2);
        let w = f.deliver("INBOX", &raw("c"));
        let before = f.epoch("INBOX");
        f.inject(FakeOp::Move, Fault::EpochRaceBefore);
        let out = f.move_messages("INBOX", &[w], "News").unwrap();
        assert_ne!(out.session_epoch, Some(before));
        f.set_capabilities(false, false, false);
        assert!(!f.capabilities().unwrap().move_supported);
        assert!(f.list_folders().unwrap().iter().all(|x| x.roles.is_none()));
    }

    #[test]
    fn envelopes_and_client_actions() {
        let f = FakeEngine::new();
        f.add_folder("News", &[]);
        let u = f.deliver_at("INBOX", &raw("a"), "2026-01-01T00:00:00+00:00");
        let e = &f.envelopes("INBOX", &[u, 99]).unwrap()[0];
        assert_eq!(e.message_id.as_deref(), Some("<a@t>"));
        assert_eq!(e.internal_date.as_deref(), Some("2026-01-01T00:00:00+00:00"));
        f.client_set_flag("INBOX", u, "\\Flagged", true);
        let n = f.client_move("INBOX", u, "News");
        assert_eq!(f.flags("News", n), vec!["\\Flagged".to_string()]);
        f.reset_epoch("News");
        assert_eq!(f.uids("News"), vec![1]);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --locked --lib engine::fake`
Expected: compile error (module missing).

- [ ] **Step 3: Implement `FakeEngine`**

State layout:

```rust
use super::{CopyUid, EngineCapabilities, FolderInfo, MailEngine, WriteOutcome};
use crate::domain::{Address, MailboxSnapshot, SourceEnvelope};
use anyhow::{anyhow, bail, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Msg { uid: u64, raw: Vec<u8>, internal_date: String, flags: BTreeSet<String> }
struct Folder { epoch: u64, uid_next: u64, msgs: Vec<Msg>, roles: Vec<String>, subscribed: bool }
struct State {
    folders: BTreeMap<String, Folder>,
    next_epoch: u64,
    caps: EngineCapabilities,
    binding: String,
    faults: Vec<(FakeOp, Fault)>,
    calls: Vec<(FakeOp, String)>,
    all_calls: Vec<String>,
}
```

Rules:
- `new()` creates `INBOX` (epoch 1, `uid_next` 1, subscribed) and sets `next_epoch = 100`, so a reset epoch never equals an earlier one. `caps` = all three capabilities on, delimiter `'/'`, prefix `""`.
- Every trait call appends to `all_calls`; write calls also append to `calls`.
- Envelopes come from header lines, matched case-insensitively: `Message-ID:` (trimmed value kept with brackets), `Subject:`, `From:` (parsed as `Name <email>`, like `parse_address` in the Himalaya engine), `Date:` as `sent_at`. `size = raw.len()`. `flags` sorted.
- `snapshot` on a missing folder → `Err`. `create_folder` on an existing folder → `Err` (the service must LIST first). `create_folder` sets epoch `next_epoch++` and `uid_next` 1.
- `move_messages`: take the matching fault, if any. `ErrorBefore` → `Err`. `NoSelect` → `Ok(WriteOutcome::default())`. `EpochRaceBefore` → `reset_epoch(folder)` first. A missing folder → `Ok(default)`. Missing MOVE support → `Ok { selected: true, session_epoch, completed: false, copyuid: None }`. Otherwise move each UID present into the target (new UID = target `uid_next++`, internal date and flags preserved); `PartialCopy` copies instead of moving and sets `completed = false`. `copyuid` = pairs when UIDPLUS is on and pairs is nonempty. `ErrorAfter` applies the effect and returns `Err`.
- `add_flagged` uses the same fault handling and inserts `\Flagged`.
- `list_folders` returns every folder, with `roles: Some(roles)` when SPECIAL-USE is on, else `None`, and `attributes` empty.
- `reset_epoch(folder)`: `epoch = next_epoch++`, renumber messages 1..=n in their current order, `uid_next = n + 1`.

- [ ] **Step 4: Add engine injection and the harness**

`Service::open_with_engine` is `Service::open(path)` followed by `self.engine_override = Some(engine)`. Create `tests/common/mod.rs` exactly as specified in Interfaces (each integration test file declares `mod common;`; add `#![allow(dead_code)]` at the top of `common/mod.rs`).

Create `tests/filing_observe.rs` with the first test, proving injection works with filing off:

```rust
mod common;
use common::{mail, Harness};
use mailtriage::domain::FilingMode;

#[test]
fn sync_through_fake_engine_with_filing_off_is_unchanged() {
    let h = Harness::new(FilingMode::Off);
    h.fake.deliver("INBOX", &mail("a", "Weekly newsletter", "Our newsletter"));
    let out = h.sync();
    assert_eq!(out["discovered"], 1);
    assert_eq!(out["classified"], 1);
    assert_eq!(h.fake.write_calls(), 0);
    assert!(out.get("filing").is_none() || out["filing"]["mode"] == "off");
}
```

- [ ] **Step 5: Run all tests**

Run: `cargo fmt && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add -A src tests
git commit -m "Add in-memory FakeEngine and service engine injection

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 4: Schema v3 and filing persistence

All filing tables, typed rows, the store API every later task uses, and the filing-aware changes to `stage`, `checkpoint` and `attach`.

**Files:**
- Create: `src/filing/mod.rs`, `src/filing/types.rs`, `src/filing/store.rs`
- Modify: `src/lib.rs` (`pub mod filing;`), `src/store.rs`
- Test: `tests/filing_store.rs`

**Interfaces:**
- Consumes: `domain::{FilingMode, SourceEnvelope, MailboxSnapshot}`, existing `Store`.
- Produces (`src/filing/types.rs`; all `#[derive(Debug, Clone, PartialEq, serde::Serialize)]`):

```rust
#[derive(Copy, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LocationState { Known, Ambiguous, Absent }   // as_str()/parse()

pub struct Placement {
    pub account: String,
    pub message_id: String,
    pub source_folder: String,
    pub home_folder: Option<String>,
    pub home_epoch: Option<u64>,
    pub home_uid: Option<u64>,
    pub location_state: LocationState,
    pub absent_since: Option<String>,
    pub desired_target: Option<String>,      // category id or "@source"
    pub pinned: bool,
    pub eligible_once: bool,
    pub desired_rev: i64,
    pub filed_at: Option<String>,
    pub filed_by: Option<String>,            // "mailtriage" | "user"
    pub flag_attempted_at: Option<String>,
    pub flagged_at: Option<String>,
    pub done_inferred: bool,
    pub blocked_reason: Option<String>,      // move_failed | duplicate_copy | quarantined | merge_conflict
}

pub struct FolderRecord {
    pub account: String,
    pub native: String,
    pub configured: Option<String>,
    pub category_id: Option<String>,         // None for source folders
    pub origin: Option<String>,              // created | adopted | source
    pub state: String,                       // ok | missing | special_use | noselect | needs_confirmation | retired | error
    pub role_verified: bool,
    pub confirmed: bool,
    pub subscribed: bool,
    pub pause_reason: Option<String>,        // epoch_race | epoch_race_suspected
    pub epoch: Option<u64>,
    pub watch_from_uid: Option<u64>,
    pub rescan_epoch: Option<u64>,
    pub rescan_below_uid: Option<u64>,
    pub rescan_complete: bool,
    pub checked_at: Option<String>,
    pub error: Option<String>,
}

pub struct Arrival {
    pub id: i64, pub account: String, pub folder: String, pub epoch: u64, pub uid: u64,
    pub message_id: String, pub rfc_message_id: Option<String>,
    pub state: String,                       // pending | resolved | vanished | unresolved | dismissed
    pub kind: Option<String>,                // own_move | user_move | user_pin | extra | new | rescan | reverted | quarantined
    pub intent_id: Option<i64>, pub created_at: String, pub resolved_at: Option<String>,
}

pub struct Intent {
    pub id: i64, pub account: String, pub message_id: String,
    pub kind: String,                        // move | flag
    pub folder: String, pub epoch: u64, pub uid: u64,
    pub target: Option<String>, pub target_epoch: Option<u64>, pub target_uid_next: Option<u64>, pub target_uid: Option<u64>,
    pub desired_rev: i64, pub consumes_eligible: bool, pub batch: Option<String>,
    pub state: String,                       // in_flight | sent | uncertain | awaiting_rescan | applied | lost | failed | superseded
    pub attempts: u32, pub next_after: Option<String>, pub dispatched_at: Option<String>,
    pub created_at: String, pub updated_at: String, pub error: Option<String>,
}
pub const OPEN_INTENT_STATES: [&str; 4] = ["in_flight", "sent", "uncertain", "awaiting_rescan"];

pub struct Revert {
    pub id: i64, pub account: String, pub parent_intent: i64,
    pub folder: String, pub folder_epoch: u64, pub uid: u64,   // COPYUID destination locator
    pub target: String, pub target_epoch: u64,                  // original source, race session epoch
    pub state: String,                                          // pending | in_flight | applied | failed
    pub target_uid: Option<u64>, pub created_at: String, pub updated_at: String, pub error: Option<String>,
}

pub struct FilingStateRow {
    pub account: String, pub mode: crate::domain::FilingMode, pub enabled_at: Option<String>,
    pub bootstrap_done: bool, pub last_pass: Option<serde_json::Value>,
}

pub struct MessageMeta {                    // transport metadata of a message
    pub rfc_message_id: Option<String>, pub size: Option<u64>, pub internal_date: Option<String>,
    pub flags: Vec<String>, pub fingerprinted: bool, pub source_managed: bool,
}

pub struct RescanFilter { pub below_uid: u64, pub rfc_ids: std::collections::BTreeSet<Option<String>> }
pub struct StageOptions<'a> {
    pub record_arrivals: bool,
    /// target UID -> (message id, intent id), from epoch-bound COPYUID.
    pub known_targets: &'a std::collections::BTreeMap<u64, (String, i64)>,
    pub rescan_filter: Option<&'a RescanFilter>,
}
pub struct NewIntent<'a> {
    pub account: &'a str, pub message_id: &'a str, pub kind: &'a str,
    pub folder: &'a str, pub epoch: u64, pub uid: u64,
    pub target: Option<&'a str>, pub target_epoch: Option<u64>, pub target_uid_next: Option<u64>,
    pub desired_rev: i64, pub consumes_eligible: bool, pub batch: &'a str, pub state: &'a str, pub now: &'a str,
}
#[derive(Default)]
pub struct IntentPatch {
    pub target_uid: Option<u64>, pub attempts: Option<u32>, pub next_after: Option<String>,
    pub dispatched_at: Option<String>, pub error: Option<String>,
}
```

`src/filing/mod.rs` declares `pub mod types; pub mod store;` (later tasks add their modules) and re-exports `pub use types::*;`.

Store API (`src/filing/store.rs`, `impl crate::store::Store`, using the existing `pub(crate) db`). Every write function runs in its own transaction and calls the existing `bump` (make it `pub(crate)`) when it changes state visible to `list`:

| Function | Contract |
| --- | --- |
| `schema_version(&self) -> Result<u32>` | `PRAGMA user_version` |
| `filing_state(&self, account) -> Result<FilingStateRow>` | default row (`Off`, nulls) if absent |
| `sync_filing_mode(&mut self, account, mode, now) -> Result<FilingStateRow>` | transitions from spec "Filing mode": off→on sets `enabled_at = now` and `bootstrap_done = 0`; dry_run↔live keeps both; →off keeps them |
| `set_bootstrap_done(&mut self, account, done: bool)` / `set_last_pass(&mut self, account, &Value)` | plain updates |
| `placement(&self, account, id)` / `placements(&self, account)` | read rows |
| `ensure_placement(&mut self, account, id, sources: &[String]) -> Result<bool>` | creates a placement only if the message has a fingerprint, has at least one occurrence, and has none yet. Home = occurrence in the first source folder in `sources` order, lowest UID; else lowest (folder, UID). `source_folder` = that source folder, else `sources[0]`. Returns whether it created one. |
| `save_placement(&mut self, p, expected_rev: Option<i64>) -> Result<bool>` | writes all fields; with `Some(rev)` only when the stored `desired_rev == rev`; returns whether written |
| `folder_record` / `folder_records` / `save_folder` | read/upsert `folders` |
| `stage_with(&mut self, account, mailbox, epoch, through, envelopes, generation, complete, opts) -> Result<usize>` | the existing `stage` semantics plus: stores `rfc_message_id`, `size`, `internal_date` columns; for a UID in `opts.known_targets` adds the occurrence to that message (no new message, no job) and its arrival carries `intent_id`; skips (no message, no occurrence) an envelope with `uid < below_uid` whose Message-ID is not in `rescan_filter.rfc_ids`; inserts a `pending` arrival per new occurrence when `record_arrivals`. `stage` becomes `stage_with` with `record_arrivals: false`, no targets, no filter. |
| `checkpoint_start_at(&mut self, account, mailbox, snapshot, start_uid) -> Result<u64>` | if no checkpoint row exists, create it at `last_uid = start_uid` in `snapshot.uid_validity`; otherwise behave exactly like `checkpoint` |
| `arrivals(&self, account, state: Option<&str>)`, `resolve_arrival(&mut self, id, state, kind: Option<&str>, now)` | read/update |
| `insert_intent(&mut self, &NewIntent) -> Result<i64>`, `intents(&self, account, open_only)`, `update_intent(&mut self, id, state, IntentPatch, now)` | `update_intent` increments nothing implicitly; `IntentPatch` fields that are `Some` overwrite |
| `claim_move(&mut self, account, action, target_epoch, target_uid_next, batch, now) -> Result<Option<i64>>` | one transaction: re-read placement; refuse (`Ok(None)`) if `desired_rev` differs from the action's, `blocked_reason` set, or an open move intent exists; else insert intent `in_flight` with `dispatched_at = now` |
| `claim_flag(&mut self, account, action, batch, now) -> Result<Option<i64>>` | one transaction: refuse if `flag_attempted_at` set or an open flag intent exists; else set `flag_attempted_at = now` and insert intent `in_flight` |
| `insert_revert`, `reverts(&self, account, open_only)`, `update_revert(&mut self, id, state, target_uid, error, now)` | `filing_reverts` CRUD |
| `record_event(&mut self, account, message_id: Option<&str>, folder: Option<&str>, kind, detail: Value, now)`, `events(&self, account, message_id: Option<&str>, limit) -> Result<Vec<Value>>` | append-only; newest first |
| `rescan_members(&self, account, folder, epoch) -> Result<Vec<(String, Option<String>)>>` | (message id, rfc Message-ID) |
| `occurrences_of(&self, account, id) -> Result<Vec<(String, u64, u64)>>` | (folder, epoch, uid) |
| `remove_occurrence(&mut self, account, folder, epoch, uid)` | delete one row |
| `message_meta(&self, account, id) -> Result<Option<MessageMeta>>` | columns plus `flags` from envelope JSON |
| `hydrate(&mut self, account, id, env: &SourceEnvelope)` | sets the three columns and merges `message_id`, `internal_date`, `size`, `flags` into envelope JSON |
| `reconcile_range_ids(..same args as reconcile_range..) -> Result<Vec<String>>` | `reconcile_range` body, returning the message ids whose occurrence was removed; `reconcile_range` returns its `len()` |

Changes in `src/store.rs`:

1. `Store::open`: reject `version > 3`; add the v3 migration after v2:

```sql
BEGIN IMMEDIATE;
ALTER TABLE messages ADD COLUMN rfc_message_id TEXT;
ALTER TABLE messages ADD COLUMN size INTEGER;
ALTER TABLE messages ADD COLUMN internal_date TEXT;
CREATE INDEX message_rfc_id ON messages(account, rfc_message_id);
CREATE TABLE filing_state(account TEXT PRIMARY KEY, mode TEXT NOT NULL DEFAULT 'off', enabled_at TEXT,
 bootstrap_done INTEGER NOT NULL DEFAULT 0, last_pass TEXT);
CREATE TABLE placements(account TEXT NOT NULL, message_id TEXT PRIMARY KEY REFERENCES messages(id),
 source_folder TEXT NOT NULL, home_folder TEXT, home_epoch INTEGER, home_uid INTEGER,
 location_state TEXT NOT NULL DEFAULT 'known', absent_since TEXT, desired_target TEXT,
 pinned INTEGER NOT NULL DEFAULT 0, eligible_once INTEGER NOT NULL DEFAULT 0, desired_rev INTEGER NOT NULL DEFAULT 0,
 filed_at TEXT, filed_by TEXT, flag_attempted_at TEXT, flagged_at TEXT,
 done_inferred INTEGER NOT NULL DEFAULT 0, blocked_reason TEXT);
CREATE INDEX placement_account ON placements(account);
CREATE TABLE folders(account TEXT NOT NULL, native TEXT NOT NULL, configured TEXT, category_id TEXT, origin TEXT,
 state TEXT NOT NULL, role_verified INTEGER NOT NULL DEFAULT 0, confirmed INTEGER NOT NULL DEFAULT 0,
 subscribed INTEGER NOT NULL DEFAULT 0, pause_reason TEXT, epoch INTEGER, watch_from_uid INTEGER,
 rescan_epoch INTEGER, rescan_below_uid INTEGER, rescan_complete INTEGER NOT NULL DEFAULT 1,
 checked_at TEXT, error TEXT, PRIMARY KEY(account, native));
CREATE TABLE arrivals(id INTEGER PRIMARY KEY, account TEXT NOT NULL, folder TEXT NOT NULL, epoch INTEGER NOT NULL,
 uid INTEGER NOT NULL, message_id TEXT NOT NULL, rfc_message_id TEXT, state TEXT NOT NULL DEFAULT 'pending',
 kind TEXT, intent_id INTEGER, created_at TEXT NOT NULL, resolved_at TEXT, UNIQUE(account, folder, epoch, uid));
CREATE INDEX arrival_state ON arrivals(account, state);
CREATE TABLE rescan_sets(account TEXT NOT NULL, folder TEXT NOT NULL, epoch INTEGER NOT NULL, message_id TEXT NOT NULL,
 PRIMARY KEY(account, folder, epoch, message_id));
CREATE TABLE filing_intents(id INTEGER PRIMARY KEY, account TEXT NOT NULL, message_id TEXT NOT NULL, kind TEXT NOT NULL,
 folder TEXT NOT NULL, epoch INTEGER NOT NULL, uid INTEGER NOT NULL, target TEXT, target_epoch INTEGER,
 target_uid_next INTEGER, target_uid INTEGER, desired_rev INTEGER NOT NULL DEFAULT 0,
 consumes_eligible INTEGER NOT NULL DEFAULT 0, batch TEXT, state TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0,
 next_after TEXT, dispatched_at TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, error TEXT);
CREATE INDEX intent_state ON filing_intents(account, state);
CREATE TABLE filing_reverts(id INTEGER PRIMARY KEY, account TEXT NOT NULL, parent_intent INTEGER NOT NULL,
 folder TEXT NOT NULL, folder_epoch INTEGER NOT NULL, uid INTEGER NOT NULL, target TEXT NOT NULL,
 target_epoch INTEGER NOT NULL, state TEXT NOT NULL, target_uid INTEGER, created_at TEXT NOT NULL,
 updated_at TEXT NOT NULL, error TEXT);
CREATE TABLE filing_events(id INTEGER PRIMARY KEY, account TEXT NOT NULL, message_id TEXT, folder TEXT,
 at TEXT NOT NULL, kind TEXT NOT NULL, detail TEXT NOT NULL DEFAULT '{}');
CREATE INDEX event_account ON filing_events(account, id);
PRAGMA user_version=3; COMMIT;
```

2. `checkpoint`: in the epoch-change branch, before `DELETE FROM occurrences`, run (with `new` = `snapshot.uid_validity`):

```sql
INSERT OR IGNORE INTO rescan_sets SELECT account, mailbox, :new, message_id FROM occurrences WHERE account=:a AND mailbox=:m;
INSERT OR IGNORE INTO rescan_sets SELECT account, home_folder, :new, message_id FROM placements WHERE account=:a AND home_folder=:m;
INSERT OR IGNORE INTO rescan_sets SELECT account, target, :new, message_id FROM filing_intents
  WHERE account=:a AND target=:m AND state IN ('in_flight','sent','uncertain','awaiting_rescan');
INSERT OR IGNORE INTO rescan_sets SELECT account, folder, :new, message_id FROM arrivals WHERE account=:a AND folder=:m AND state='pending';
UPDATE arrivals SET state='vanished', resolved_at=:now WHERE account=:a AND folder=:m AND state='pending';
UPDATE folders SET rescan_epoch=:new, rescan_below_uid=:uid_next, rescan_complete=0, epoch=:new WHERE account=:a AND native=:m;
```

3. `attach`, merge branch, before deleting the provisional message: `UPDATE arrivals SET message_id=existing WHERE message_id=id`; `INSERT OR IGNORE INTO rescan_sets SELECT account, folder, epoch, :existing FROM rescan_sets WHERE message_id=:id` then `DELETE FROM rescan_sets WHERE message_id=:id`. Then merge transport metadata into the canonical row: the `rfc_message_id`, `size` and `internal_date` columns via `COALESCE(existing, provisional)`, and the envelope JSON keys `message_id`, `internal_date`, `size`, `flags` (provisional value wins for `flags`, otherwise keep existing). Non-merge branch: merge `subject`, `from`, `sent_at` into the existing envelope object instead of replacing it.

- [ ] **Step 1: Write the failing tests**

Create `tests/filing_store.rs`:

```rust
use mailtriage::{
    domain::{FilingMode, MailboxSnapshot, SourceEnvelope},
    filing::{LocationState, RescanFilter, StageOptions},
    normalize,
    store::Store,
};
use std::collections::{BTreeMap, BTreeSet};

fn store() -> (tempfile::TempDir, Store) {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("db")).unwrap();
    (d, s)
}
fn env(uid: u64, mid: &str) -> SourceEnvelope {
    SourceEnvelope {
        uid,
        subject: "s".into(),
        message_id: Some(format!("<{mid}@t>")),
        internal_date: Some("2026-10-04T10:00:00+00:00".into()),
        size: Some(100),
        flags: vec!["\\Seen".into()],
        ..Default::default()
    }
}
fn snap(epoch: u64, next: u64) -> MailboxSnapshot {
    MailboxSnapshot { uid_validity: epoch, uid_next: next }
}
fn opts<'a>(t: &'a BTreeMap<u64, (String, i64)>) -> StageOptions<'a> {
    StageOptions { record_arrivals: true, known_targets: t, rescan_filter: None }
}
const NOW: &str = "2026-10-04T12:00:00+00:00";

#[test]
fn migration_reaches_v3_and_is_idempotent() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    assert_eq!(Store::open(&p).unwrap().schema_version().unwrap(), 3);
    assert_eq!(Store::open(&p).unwrap().schema_version().unwrap(), 3);
}

#[test]
fn filing_mode_transitions_keep_or_reset_enabled_at() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    let a = s.sync_filing_mode("work", FilingMode::DryRun, "2026-10-01T00:00:00+00:00").unwrap();
    assert_eq!(a.enabled_at.as_deref(), Some("2026-10-01T00:00:00+00:00"));
    let b = s.sync_filing_mode("work", FilingMode::Live, "2026-10-02T00:00:00+00:00").unwrap();
    assert_eq!(b.enabled_at, a.enabled_at);
    let c = s.sync_filing_mode("work", FilingMode::Off, "2026-10-03T00:00:00+00:00").unwrap();
    assert_eq!(c.enabled_at, a.enabled_at);
    let e = s.sync_filing_mode("work", FilingMode::Live, "2026-10-04T00:00:00+00:00").unwrap();
    assert_eq!(e.enabled_at.as_deref(), Some("2026-10-04T00:00:00+00:00"));
}

#[test]
fn stage_with_records_arrivals_metadata_and_known_targets() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    s.checkpoint("work", "INBOX", &snap(5, 3)).unwrap();
    let none = BTreeMap::new();
    s.stage_with("work", "INBOX", 5, 2, &[env(1, "a"), env(2, "b")], "g1", true, &opts(&none)).unwrap();
    let arrivals = s.arrivals("work", Some("pending")).unwrap();
    assert_eq!(arrivals.len(), 2);
    assert_eq!(arrivals[0].rfc_message_id.as_deref(), Some("<a@t>"));
    let id = arrivals[0].message_id.clone();
    let meta = s.message_meta("work", &id).unwrap().unwrap();
    assert_eq!(meta.size, Some(100));
    assert_eq!(meta.flags, vec!["\\Seen".to_string()]);
    // A COPYUID-known target joins the existing message without a new message.
    s.checkpoint("work", "News", &snap(9, 21)).unwrap();
    let targets = BTreeMap::from([(20u64, (id.clone(), 77i64))]);
    s.stage_with("work", "News", 9, 20, &[env(20, "a")], "g1", true, &opts(&targets)).unwrap();
    let occ = s.occurrences_of("work", &id).unwrap();
    assert!(occ.contains(&("News".to_string(), 9, 20)));
    let arr = s.arrivals("work", Some("pending")).unwrap();
    assert!(arr.iter().any(|a| a.folder == "News" && a.intent_id == Some(77) && a.message_id == id));
}

#[test]
fn rescan_filter_skips_unrelated_old_content() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    s.checkpoint("work", "News", &snap(3, 10)).unwrap();
    let none = BTreeMap::new();
    let filter = RescanFilter { below_uid: 10, rfc_ids: BTreeSet::from([Some("<keep@t>".to_string())]) };
    let o = StageOptions { record_arrivals: true, known_targets: &none, rescan_filter: Some(&filter) };
    s.stage_with("work", "News", 3, 11, &[env(1, "keep"), env(2, "other"), env(11, "new")], "g1", true, &o).unwrap();
    let ids: Vec<_> = s.arrivals("work", None).unwrap().into_iter().map(|a| a.uid).collect();
    assert_eq!(ids, vec![1, 11]);
}

#[test]
fn placement_creation_selection_and_revision_cas() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    let raw = b"Message-ID: <a@t>\r\nSubject: s\r\n\r\nbody";
    let msg = normalize::rfc822(raw, 1000).unwrap();
    s.checkpoint("work", "News", &snap(9, 5)).unwrap();
    s.checkpoint("work", "INBOX", &snap(5, 5)).unwrap();
    let none = BTreeMap::new();
    s.stage_with("work", "News", 9, 4, &[env(4, "a")], "g1", true, &opts(&none)).unwrap();
    let id = s.arrivals("work", None).unwrap()[0].message_id.clone();
    let sources = vec!["INBOX".to_string(), "Work".to_string()];
    assert!(!s.ensure_placement("work", &id, &sources).unwrap(), "no fingerprint yet");
    s.attach("work", &id, &msg).unwrap();
    assert!(s.ensure_placement("work", &id, &sources).unwrap());
    let p = s.placement("work", &id).unwrap().unwrap();
    assert_eq!(p.home_folder.as_deref(), Some("News"));
    assert_eq!(p.source_folder, "INBOX");
    assert_eq!(p.location_state, LocationState::Known);
    let mut q = p.clone();
    q.pinned = true;
    q.desired_rev += 1;
    assert!(s.save_placement(&q, Some(p.desired_rev)).unwrap());
    assert!(!s.save_placement(&q, Some(p.desired_rev)).unwrap(), "stale revision must not write");
}

#[test]
fn filing_aware_merge_moves_arrivals_and_keeps_metadata() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    let raw = b"Message-ID: <a@t>\r\nSubject: s\r\n\r\nbody";
    let msg = normalize::rfc822(raw, 1000).unwrap();
    let canonical = s.ingest("work", &msg, "g1").unwrap();
    s.checkpoint("work", "INBOX", &snap(5, 5)).unwrap();
    let none = BTreeMap::new();
    s.stage_with("work", "INBOX", 5, 1, &[env(1, "a")], "g1", true, &opts(&none)).unwrap();
    let provisional = s.arrivals("work", None).unwrap()[0].message_id.clone();
    assert_eq!(s.attach("work", &provisional, &msg).unwrap(), canonical);
    let arrival = &s.arrivals("work", None).unwrap()[0];
    assert_eq!(arrival.message_id, canonical);
    let meta = s.message_meta("work", &canonical).unwrap().unwrap();
    assert_eq!(meta.rfc_message_id.as_deref(), Some("<a@t>"));
    assert_eq!(meta.size, Some(100));
}

#[test]
fn epoch_reset_captures_rescan_set_and_vanishes_pending_arrivals() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    s.checkpoint("work", "News", &snap(3, 5)).unwrap();
    let none = BTreeMap::new();
    s.stage_with("work", "News", 3, 4, &[env(4, "a")], "g1", true, &opts(&none)).unwrap();
    let id = s.arrivals("work", None).unwrap()[0].message_id.clone();
    s.checkpoint("work", "News", &snap(4, 2)).unwrap();
    let members = s.rescan_members("work", "News", 4).unwrap();
    assert_eq!(members, vec![(id, Some("<a@t>".to_string()))]);
    assert_eq!(s.arrivals("work", Some("vanished")).unwrap().len(), 1);
}

#[test]
fn claims_check_revision_and_consume_flag_attempt() {
    use mailtriage::filing::planner::{Action, Locator};
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    let raw = b"Message-ID: <a@t>\r\nSubject: s\r\n\r\nbody";
    let msg = normalize::rfc822(raw, 1000).unwrap();
    s.checkpoint("work", "INBOX", &snap(5, 5)).unwrap();
    let none = BTreeMap::new();
    s.stage_with("work", "INBOX", 5, 1, &[env(1, "a")], "g1", true, &opts(&none)).unwrap();
    let id = s.arrivals("work", None).unwrap()[0].message_id.clone();
    s.attach("work", &id, &msg).unwrap();
    s.ensure_placement("work", &id, &["INBOX".to_string()]).unwrap();
    let at = Locator { folder: "INBOX".into(), epoch: 5, uid: 1 };
    let stale = Action::Move { message_id: id.clone(), from: at.clone(), to: "News".into(), desired_rev: 9, consumes_eligible: false };
    assert!(s.claim_move("work", &stale, 1, 1, "b1", NOW).unwrap().is_none());
    let fresh = Action::Move { message_id: id.clone(), from: at.clone(), to: "News".into(), desired_rev: 0, consumes_eligible: false };
    assert!(s.claim_move("work", &fresh, 1, 1, "b1", NOW).unwrap().is_some());
    assert!(s.claim_move("work", &fresh, 1, 1, "b2", NOW).unwrap().is_none(), "one open move intent per message");
    let flag = Action::Flag { message_id: id.clone(), at };
    assert!(s.claim_flag("work", &flag, "b3", NOW).unwrap().is_some());
    assert!(s.claim_flag("work", &flag, "b4", NOW).unwrap().is_none(), "one flag attempt ever");
    assert!(s.placement("work", &id).unwrap().unwrap().flag_attempted_at.is_some());
}

#[test]
fn events_are_newest_first() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    s.record_event("work", None, Some("News"), "folder_created", serde_json::json!({}), "2026-10-04T10:00:00+00:00").unwrap();
    s.record_event("work", Some("m1"), None, "moved", serde_json::json!({"to": "News"}), "2026-10-04T11:00:00+00:00").unwrap();
    let e = s.events("work", None, 10).unwrap();
    assert_eq!(e[0]["kind"], "moved");
    assert_eq!(s.events("work", Some("m1"), 10).unwrap().len(), 1);
}
```

`claims_check_revision_and_consume_flag_attempt` needs `filing::planner::{Action, Locator}`. Create `src/filing/planner.rs` in this task with only the `Locator` and `Action` definitions from Task 5 (Task 5 adds the rest of the file).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --locked --test filing_store`
Expected: compile errors.

- [ ] **Step 3: Implement migration, types and store API**

Implement everything in this task's Interfaces and "Changes in `src/store.rs`" blocks. Row decoding follows `row_record` in `src/store.rs`: decode booleans from INTEGER, and `FilingMode` from `"off" | "dry_run" | "live"`. Code for the three functions with subtle rules:

```rust
pub fn sync_filing_mode(&mut self, account: &str, mode: FilingMode, now: &str) -> Result<FilingStateRow> {
    let tx = self.db.transaction()?;
    tx.execute("INSERT OR IGNORE INTO filing_state(account) VALUES(?)", [account])?;
    let (old, enabled_at): (String, Option<String>) = tx.query_row(
        "SELECT mode, enabled_at FROM filing_state WHERE account=?", [account], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let new = mode_str(mode);
    let turning_on = old == "off" && new != "off";
    let enabled_at = if turning_on { Some(now.to_string()) } else { enabled_at };
    tx.execute("UPDATE filing_state SET mode=?, enabled_at=? WHERE account=?", params![new, enabled_at, account])?;
    if turning_on {
        tx.execute("UPDATE filing_state SET bootstrap_done=0 WHERE account=?", [account])?;
    }
    tx.commit()?;
    self.filing_state(account)
}

pub fn ensure_placement(&mut self, account: &str, id: &str, sources: &[String]) -> Result<bool> {
    let tx = self.db.transaction()?;
    let eligible: bool = tx.query_row(
        "SELECT fingerprint IS NOT NULL AND NOT EXISTS(SELECT 1 FROM placements WHERE message_id=messages.id)
         FROM messages WHERE account=? AND id=?", params![account, id], |r| r.get(0)).optional()?.unwrap_or(false);
    if !eligible { return Ok(false); }
    let mut occ: Vec<(String, u64, u64)> = {
        let mut st = tx.prepare("SELECT mailbox, epoch, uid FROM occurrences WHERE account=? AND message_id=? ORDER BY mailbox, uid")?;
        let rows = st.query_map(params![account, id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    if occ.is_empty() { return Ok(false); }
    let rank = |f: &str| sources.iter().position(|s| s == f).unwrap_or(usize::MAX);
    occ.sort_by(|a, b| rank(&a.0).cmp(&rank(&b.0)).then(a.0.cmp(&b.0)).then(a.2.cmp(&b.2)));
    let home = &occ[0];
    let source = if rank(&home.0) != usize::MAX { home.0.clone() } else { sources.first().cloned().unwrap_or_else(|| "INBOX".into()) };
    tx.execute("INSERT INTO placements(account, message_id, source_folder, home_folder, home_epoch, home_uid) VALUES(?,?,?,?,?,?)",
        params![account, id, source, home.0, home.1, home.2])?;
    bump(&tx)?;
    tx.commit()?;
    Ok(true)
}

pub fn claim_move(&mut self, account: &str, action: &Action, target_epoch: u64, target_uid_next: u64, batch: &str, now: &str) -> Result<Option<i64>> {
    let Action::Move { message_id, from, to, desired_rev, consumes_eligible } = action else { bail!("claim_move needs a Move") };
    let tx = self.db.transaction()?;
    let ok: bool = tx.query_row(
        "SELECT desired_rev=? AND blocked_reason IS NULL AND NOT EXISTS(SELECT 1 FROM filing_intents
           WHERE message_id=placements.message_id AND kind='move' AND state IN ('in_flight','sent','uncertain','awaiting_rescan'))
         FROM placements WHERE account=? AND message_id=?",
        params![desired_rev, account, message_id], |r| r.get(0)).optional()?.unwrap_or(false);
    if !ok { return Ok(None); }
    tx.execute("INSERT INTO filing_intents(account,message_id,kind,folder,epoch,uid,target,target_epoch,target_uid_next,desired_rev,consumes_eligible,batch,state,dispatched_at,created_at,updated_at)
                VALUES(?,?,'move',?,?,?,?,?,?,?,?,?,'in_flight',?,?,?)",
        params![account, message_id, from.folder, from.epoch, from.uid, to, target_epoch, target_uid_next, desired_rev, consumes_eligible, batch, now, now, now])?;
    let id = tx.last_insert_rowid();
    tx.commit()?;
    Ok(Some(id))
}
```

`ensure_placement` uses `rusqlite::OptionalExtension`.

- [ ] **Step 4: Run all tests**

Run: `cargo fmt && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`
Expected: PASS (existing storage and sync tests unchanged).

- [ ] **Step 5: Commit**

```bash
git add -A src tests
git commit -m "Add schema v3 filing persistence with arrivals, intents and rescan sets

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Pure filing planner

The decision core: placement, classification and folder state in; moves, flags and cleared requests out. No I/O.

**Files:**
- Modify: `src/filing/planner.rs` (Task 4 created `Locator`/`Action`)
- Test: unit tests in the same file

**Interfaces:**
- Consumes: `domain::{FilingMode, Urgency}`.
- Produces: `planner::{plan, Plan, PlanInput, PlanMessage, Effective, FolderView, FolderUse, CategoryFolder, Action, Locator}` exactly as below.

- [ ] **Step 1: Write the planner tests**

Append to `src/filing/planner.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 4, h, 0, 0).unwrap()
    }
    fn folders() -> BTreeMap<String, FolderView> {
        let mut f = BTreeMap::new();
        f.insert("INBOX".into(), FolderView { usable: FolderUse::Ok, paused: false, is_source: true, is_category: false, retired_listed: false, epoch: Some(1) });
        f.insert("Newsletters".into(), FolderView { usable: FolderUse::Ok, paused: false, is_source: false, is_category: true, retired_listed: false, epoch: Some(2) });
        f.insert("Transactions".into(), FolderView { usable: FolderUse::Ok, paused: false, is_source: false, is_category: true, retired_listed: false, epoch: Some(3) });
        f
    }
    fn input(messages: Vec<PlanMessage>) -> PlanInput {
        PlanInput {
            mode: FilingMode::Live,
            preview: false,
            flag_enabled: true,
            max_actions: 200,
            enabled_at: Some(t(8)),
            categories: BTreeMap::from([
                ("newsletters".to_string(), CategoryFolder::Native("Newsletters".into())),
                ("transactions".to_string(), CategoryFolder::Native("Transactions".into())),
                ("correspondence".to_string(), CategoryFolder::Inbox),
            ]),
            folders: folders(),
            messages,
        }
    }
    fn msg(id: &str, category: &str) -> PlanMessage {
        PlanMessage {
            message_id: id.into(),
            source_folder: "INBOX".into(),
            home: Some(Locator { folder: "INBOX".into(), epoch: 1, uid: 10 }),
            location_known: true,
            hydrated: true,
            internal_date: Some(t(9)),
            flags: vec![],
            desired_target: None,
            pinned: false,
            eligible_once: false,
            desired_rev: 0,
            filed_at: None,
            flag_attempted: false,
            blocked: false,
            open_move_intent: false,
            open_flag_intent: false,
            effective: Effective { category_id: Some(category.into()), current: true, urgency: Some(Urgency::Low), action_required: Some(false), ..Default::default() },
        }
    }
    fn moves(p: &Plan) -> Vec<(String, String)> {
        p.actions.iter().filter_map(|a| match a { Action::Move { message_id, to, .. } => Some((message_id.clone(), to.clone())), _ => None }).collect()
    }
    fn flags(p: &Plan) -> Vec<String> {
        p.actions.iter().filter_map(|a| match a { Action::Flag { message_id, .. } => Some(message_id.clone()), _ => None }).collect()
    }

    #[test]
    fn new_mail_moves_to_category_folder() {
        let p = plan(&input(vec![msg("a", "newsletters")]));
        assert_eq!(moves(&p), vec![("a".into(), "Newsletters".into())]);
    }

    #[test]
    fn off_mode_plans_nothing() {
        let mut i = input(vec![msg("a", "newsletters")]);
        i.mode = FilingMode::Off;
        assert!(plan(&i).actions.is_empty());
    }

    #[test]
    fn inbox_category_stays_and_old_mail_is_not_new() {
        let mut old = msg("old", "newsletters");
        old.internal_date = Some(t(7));
        let mut undated = msg("undated", "newsletters");
        undated.internal_date = None;
        let p = plan(&input(vec![msg("c", "correspondence"), old, undated]));
        assert!(moves(&p).is_empty());
    }

    #[test]
    fn eligible_once_files_old_mail_and_is_consumed() {
        let mut old = msg("old", "newsletters");
        old.internal_date = Some(t(7));
        old.eligible_once = true;
        let p = plan(&input(vec![old]));
        assert!(matches!(&p.actions[0], Action::Move { consumes_eligible: true, .. }));
    }

    #[test]
    fn already_filed_pinned_blocked_intent_or_unhydrated_do_not_move() {
        let mut filed = msg("filed", "newsletters");
        filed.filed_at = Some("x".into());
        let mut pinned = msg("pinned", "newsletters");
        pinned.pinned = true;
        let mut blocked = msg("blocked", "newsletters");
        blocked.blocked = true;
        let mut busy = msg("busy", "newsletters");
        busy.open_move_intent = true;
        let mut raw = msg("raw", "newsletters");
        raw.hydrated = false;
        let mut lost = msg("lost", "newsletters");
        lost.location_known = false;
        let p = plan(&input(vec![filed, pinned, blocked, busy, raw, lost]));
        assert!(moves(&p).is_empty());
    }

    #[test]
    fn stale_generation_and_incomplete_input_block_automatic_moves_unless_overridden() {
        let mut stale = msg("stale", "newsletters");
        stale.effective.current = false;
        let mut partial = msg("partial", "newsletters");
        partial.effective.input_incomplete = true;
        let mut corrected = msg("corrected", "transactions");
        corrected.effective.current = false;
        corrected.effective.input_incomplete = true;
        corrected.effective.category_from_override = true;
        let p = plan(&input(vec![stale, partial, corrected]));
        assert_eq!(moves(&p), vec![("corrected".into(), "Transactions".into())]);
    }

    #[test]
    fn explicit_request_moves_from_category_folder_and_clears_when_satisfied() {
        let mut m = msg("m", "newsletters");
        m.home = Some(Locator { folder: "Newsletters".into(), epoch: 2, uid: 5 });
        m.filed_at = Some("x".into());
        m.desired_target = Some("transactions".into());
        m.desired_rev = 3;
        let mut back = msg("back", "newsletters");
        back.home = Some(Locator { folder: "Newsletters".into(), epoch: 2, uid: 6 });
        back.desired_target = Some("@source".into());
        let mut done = msg("done", "newsletters");
        done.home = Some(Locator { folder: "Transactions".into(), epoch: 3, uid: 7 });
        done.desired_target = Some("transactions".into());
        done.desired_rev = 4;
        let p = plan(&input(vec![m, back, done]));
        assert_eq!(moves(&p), vec![("back".into(), "INBOX".into()), ("m".into(), "Transactions".into())]);
        assert!(p.actions.iter().any(|a| matches!(a, Action::Move { message_id, desired_rev: 3, consumes_eligible: false, .. } if message_id == "m")));
        assert_eq!(p.cleared_requests, vec![("done".to_string(), 4)]);
    }

    #[test]
    fn paused_or_unusable_folders_block_actions() {
        let mut i = input(vec![msg("a", "newsletters"), msg("b", "transactions")]);
        i.folders.get_mut("Newsletters").unwrap().paused = true;
        i.folders.get_mut("Transactions").unwrap().usable = FolderUse::Unusable;
        assert!(moves(&plan(&i)).is_empty());
        let mut i = input(vec![msg("a", "newsletters")]);
        i.folders.get_mut("INBOX").unwrap().paused = true;
        assert!(plan(&i).actions.is_empty());
    }

    #[test]
    fn epoch_mismatch_blocks_actions() {
        let mut m = msg("a", "newsletters");
        m.home.as_mut().unwrap().epoch = 99;
        assert!(plan(&input(vec![m])).actions.is_empty());
    }

    #[test]
    fn preview_counts_would_create_folders() {
        let mut i = input(vec![msg("a", "newsletters")]);
        i.folders.get_mut("Newsletters").unwrap().usable = FolderUse::WouldCreate;
        assert!(moves(&plan(&i)).is_empty());
        i.preview = true;
        let p = plan(&i);
        assert_eq!(moves(&p).len(), 1);
        assert_eq!(p.folders_to_create, vec!["Newsletters".to_string()]);
    }

    #[test]
    fn flags_for_action_or_high_urgency_once() {
        let mut act = msg("act", "correspondence");
        act.effective.action_required = Some(true);
        let mut hot = msg("hot", "newsletters");
        hot.effective.urgency = Some(Urgency::High);
        let mut tried = msg("tried", "correspondence");
        tried.effective.action_required = Some(true);
        tried.flag_attempted = true;
        let mut has = msg("has", "correspondence");
        has.effective.action_required = Some(true);
        has.flags = vec!["\\Flagged".into()];
        let mut stale = msg("stale", "correspondence");
        stale.effective.action_required = Some(true);
        stale.effective.current = false;
        let mut forced = msg("forced", "correspondence");
        forced.effective.action_required = Some(true);
        forced.effective.current = false;
        forced.effective.action_from_override = true;
        let p = plan(&input(vec![act, hot, tried, has, stale, forced]));
        assert_eq!(flags(&p), vec!["act".to_string(), "forced".into(), "hot".into()]);
        let mut i = input(vec![msg("x", "correspondence")]);
        i.messages[0].effective.action_required = Some(true);
        i.flag_enabled = false;
        assert!(flags(&plan(&i)).is_empty());
    }

    #[test]
    fn backlog_is_not_flagged_until_in_scope() {
        let mut old = msg("old", "correspondence");
        old.internal_date = Some(t(7));
        old.effective.action_required = Some(true);
        let mut backfilled = old.clone();
        backfilled.message_id = "backfilled".into();
        backfilled.eligible_once = true;
        let mut filed = old.clone();
        filed.message_id = "filed".into();
        filed.filed_at = Some("x".into());
        let p = plan(&input(vec![old, backfilled, filed]));
        assert_eq!(flags(&p), vec!["backfilled".to_string(), "filed".into()]);
    }

    #[test]
    fn ordering_flags_first_oldest_first_and_cap() {
        let mut a = msg("a", "transactions");
        a.internal_date = Some(t(11));
        a.effective.action_required = Some(true);
        let mut b = msg("b", "newsletters");
        b.internal_date = Some(t(10));
        let mut i = input(vec![a, b]);
        let p = plan(&i);
        let order: Vec<_> = p.actions.iter().map(|x| match x { Action::Move { message_id, .. } => format!("m:{message_id}"), Action::Flag { message_id, .. } => format!("f:{message_id}") }).collect();
        assert_eq!(order, vec!["m:b", "f:a", "m:a"]);
        i.max_actions = 2;
        assert_eq!(plan(&i).actions.len(), 2);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --locked --lib filing::planner`
Expected: compile errors.

- [ ] **Step 3: Implement the planner**

Full `src/filing/planner.rs` (keep the test module at the bottom):

```rust
//! Pure filing planner (spec "Planner"): local state in, actions out.
use crate::domain::{FilingMode, Urgency};
use chrono::{DateTime, Utc};
use std::{cmp::Ordering, collections::BTreeMap};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Locator {
    pub folder: String,
    pub epoch: u64,
    pub uid: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    Move { message_id: String, from: Locator, to: String, desired_rev: i64, consumes_eligible: bool },
    Flag { message_id: String, at: Locator },
}
impl Action {
    pub fn message_id(&self) -> &str {
        match self {
            Action::Move { message_id, .. } | Action::Flag { message_id, .. } => message_id,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Plan {
    pub folders_to_create: Vec<String>,
    pub actions: Vec<Action>,
    /// Satisfied explicit requests: (message id, desired_rev) to clear.
    pub cleared_requests: Vec<(String, i64)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderUse {
    Ok,
    WouldCreate,
    Unusable,
}

#[derive(Debug, Clone)]
pub struct FolderView {
    pub usable: FolderUse,
    pub paused: bool,
    pub is_source: bool,
    pub is_category: bool,
    pub retired_listed: bool,
    pub epoch: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CategoryFolder {
    Inbox,
    Native(String),
}

#[derive(Debug, Clone, Default)]
pub struct Effective {
    pub category_id: Option<String>,
    pub category_from_override: bool,
    pub urgency: Option<Urgency>,
    pub urgency_from_override: bool,
    pub action_required: Option<bool>,
    pub action_from_override: bool,
    /// Generation is current and status is ready or uncertain.
    pub current: bool,
    pub input_incomplete: bool,
}

#[derive(Debug, Clone)]
pub struct PlanMessage {
    pub message_id: String,
    pub source_folder: String,
    pub home: Option<Locator>,
    pub location_known: bool,
    pub hydrated: bool,
    pub internal_date: Option<DateTime<Utc>>,
    pub flags: Vec<String>,
    pub desired_target: Option<String>,
    pub pinned: bool,
    pub eligible_once: bool,
    pub desired_rev: i64,
    pub filed_at: Option<String>,
    pub flag_attempted: bool,
    pub blocked: bool,
    pub open_move_intent: bool,
    pub open_flag_intent: bool,
    pub effective: Effective,
}

#[derive(Debug, Clone)]
pub struct PlanInput {
    pub mode: FilingMode,
    /// `filing plan` and dry_run passes: folders that would be created count as usable.
    pub preview: bool,
    pub flag_enabled: bool,
    pub max_actions: usize,
    pub enabled_at: Option<DateTime<Utc>>,
    pub categories: BTreeMap<String, CategoryFolder>,
    pub folders: BTreeMap<String, FolderView>,
    pub messages: Vec<PlanMessage>,
}

enum MoveDecision {
    Move(Action),
    Clear,
    Nothing,
}

pub fn plan(input: &PlanInput) -> Plan {
    let mut out = Plan::default();
    if input.mode == FilingMode::Off {
        return out;
    }
    out.folders_to_create = input
        .folders
        .iter()
        .filter(|(_, f)| f.is_category && f.usable == FolderUse::WouldCreate)
        .map(|(name, _)| name.clone())
        .collect();
    let mut per_message = Vec::new();
    for m in &input.messages {
        let Some(home) = eligible_home(input, m) else { continue };
        let mut actions = Vec::new();
        if let Some(flag) = flag_action(input, m, home) {
            actions.push(flag);
        }
        match move_action(input, m, home) {
            MoveDecision::Move(a) => actions.push(a),
            MoveDecision::Clear => out.cleared_requests.push((m.message_id.clone(), m.desired_rev)),
            MoveDecision::Nothing => {}
        }
        if !actions.is_empty() {
            per_message.push((m.internal_date, m.message_id.as_str(), actions));
        }
    }
    per_message.sort_by(|a, b| {
        let by_date = match (a.0, b.0) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        };
        by_date.then_with(|| a.1.cmp(b.1))
    });
    out.actions = per_message.into_iter().flat_map(|(_, _, a)| a).take(input.max_actions).collect();
    out
}

fn eligible_home<'a>(input: &PlanInput, m: &'a PlanMessage) -> Option<&'a Locator> {
    if !m.location_known || !m.hydrated || m.blocked {
        return None;
    }
    let home = m.home.as_ref()?;
    let view = input.folders.get(&home.folder)?;
    if view.paused || view.epoch != Some(home.epoch) {
        return None;
    }
    Some(home)
}

fn is_new(input: &PlanInput, m: &PlanMessage) -> bool {
    m.filed_at.is_none() && matches!((m.internal_date, input.enabled_at), (Some(d), Some(on)) if d >= on)
}

fn flag_action(input: &PlanInput, m: &PlanMessage, home: &Locator) -> Option<Action> {
    if !input.flag_enabled || m.flag_attempted || m.open_flag_intent {
        return None;
    }
    if !(is_new(input, m) || m.eligible_once || m.filed_at.is_some()) {
        return None;
    }
    if m.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Flagged")) {
        return None;
    }
    let e = &m.effective;
    let action = e.action_required == Some(true) && (e.action_from_override || e.current);
    let urgent = e.urgency == Some(Urgency::High) && (e.urgency_from_override || e.current);
    (action || urgent).then(|| Action::Flag { message_id: m.message_id.clone(), at: home.clone() })
}

fn resolve_target(input: &PlanInput, m: &PlanMessage, target: &str) -> Option<String> {
    if target == "@source" {
        return Some(m.source_folder.clone());
    }
    match input.categories.get(target)? {
        CategoryFolder::Inbox => Some(m.source_folder.clone()),
        CategoryFolder::Native(name) => Some(name.clone()),
    }
}

fn target_usable(input: &PlanInput, folder: &str) -> bool {
    match input.folders.get(folder) {
        Some(v) if !v.paused => {
            v.is_source
                || match v.usable {
                    FolderUse::Ok => true,
                    FolderUse::WouldCreate => input.preview,
                    FolderUse::Unusable => false,
                }
        }
        _ => false,
    }
}

fn move_action(input: &PlanInput, m: &PlanMessage, home: &Locator) -> MoveDecision {
    if m.open_move_intent {
        return MoveDecision::Nothing;
    }
    let home_view = &input.folders[&home.folder];
    if let Some(target) = &m.desired_target {
        let Some(to) = resolve_target(input, m, target) else { return MoveDecision::Nothing };
        if to == home.folder {
            return MoveDecision::Clear;
        }
        let home_ok = home_view.is_source
            || (home_view.is_category && home_view.usable == FolderUse::Ok)
            || home_view.retired_listed;
        if !home_ok || !target_usable(input, &to) {
            return MoveDecision::Nothing;
        }
        return MoveDecision::Move(Action::Move {
            message_id: m.message_id.clone(),
            from: home.clone(),
            to,
            desired_rev: m.desired_rev,
            consumes_eligible: false,
        });
    }
    if !home_view.is_source || m.pinned {
        return MoveDecision::Nothing;
    }
    let e = &m.effective;
    let Some(category) = &e.category_id else { return MoveDecision::Nothing };
    if !e.category_from_override && (!e.current || e.input_incomplete) {
        return MoveDecision::Nothing;
    }
    let Some(CategoryFolder::Native(to)) = input.categories.get(category) else { return MoveDecision::Nothing };
    if !(is_new(input, m) || m.eligible_once) || *to == home.folder || !target_usable(input, to) {
        return MoveDecision::Nothing;
    }
    MoveDecision::Move(Action::Move {
        message_id: m.message_id.clone(),
        from: home.clone(),
        to: to.clone(),
        desired_rev: m.desired_rev,
        consumes_eligible: m.eligible_once,
    })
}
```

- [ ] **Step 4: Run tests**

Run: `cargo fmt && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A src
git commit -m "Add pure filing planner

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 6: Observation pass (mode, folders, watching, bootstrap, hydration)

Restructures `Service::sync` into the spec's step order and implements the observation steps: mode sync, folder resolution (spec "Folders" rules 1–7), discovery of category and retired folders, rescan filtering and completion, placement creation, bootstrap and hydration.

**Files:**
- Create: `src/filing/observe.rs`
- Modify: `src/filing/mod.rs`, `src/service.rs`, `src/engine/fake.rs` (one toggle)
- Test: `tests/filing_observe.rs`

**Interfaces:**
- Consumes: Tasks 1–5.
- Produces:

```rust
// src/filing/mod.rs
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct FilingSummary {
    pub mode: String, pub planned: usize, pub moved: usize, pub flagged: usize, pub reverted: usize,
    pub client_corrections: usize, pub pinned: usize, pub archived_done: usize, pub quarantined: usize,
    pub unresolved: usize, pub folders_created: usize, pub hydrated: usize, pub errors: usize,
    pub problems: Vec<String>,
}
pub struct PassContext<'a> {
    pub account: &'a str,
    pub cfg: &'a crate::domain::AccountConfig,
    pub engine: &'a dyn crate::engine::MailEngine,
    pub mode: crate::domain::FilingMode,
    pub generation: &'a str,
    pub now: String,
    pub max_attempts: u32,
    /// Re-verifies the account binding; called before write batches and recovery observations.
    /// Build it from owned clones (account config, the stored identity string, the engine `Rc`)
    /// so it never borrows `Service` while the store is mutably borrowed.
    pub verify_binding: &'a dyn Fn() -> anyhow::Result<()>,
}
pub fn mode_str(mode: crate::domain::FilingMode) -> &'static str; // "off" | "dry_run" | "live"

// src/filing/observe.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchRole { Source, Category, Retired }
#[derive(Debug, Clone)]
pub struct WatchSpec { pub folder: String, pub role: WatchRole }
#[derive(Debug, Clone, Default)]
pub struct FolderMap {
    pub caps: Option<crate::engine::EngineCapabilities>,
    pub sources: Vec<String>,
    pub categories: std::collections::BTreeMap<String, crate::filing::planner::CategoryFolder>,
    pub watch: Vec<WatchSpec>,
    pub listed: std::collections::BTreeSet<String>,   // names LIST reported this pass
    pub would_create: Vec<String>,                    // dry_run: folders a live pass would create
    pub writes_allowed: bool,                         // live, MOVE supported, no alias conflict
}
impl FolderMap {
    pub fn sources_only(cfg: &crate::domain::AccountConfig) -> Self;  // mode off: watch = sources
    pub fn native_for(&self, category_id: &str) -> Option<&str>;
}
pub fn resolve_folders(store: &mut Store, ctx: &PassContext, summary: &mut FilingSummary) -> anyhow::Result<FolderMap>;
pub fn bootstrap_and_hydrate(store: &mut Store, ctx: &PassContext, map: &FolderMap, summary: &mut FilingSummary) -> anyhow::Result<()>;
pub fn update_rescan_completion(store: &mut Store, ctx: &PassContext, map: &FolderMap) -> anyhow::Result<()>;

// src/engine/fake.rs
impl FakeEngine { pub fn set_rich_discovery(&self, on: bool); } // false: discover() omits message_id/internal_date/size/flags; envelopes() always full
```

Behaviour:

1. **`Service::sync` step order** (replace the body; keep lock, limits, `ensure`, the existing discovery/reconciliation core and the JSON fields):
   1. `engine = self.engine(&account)?`; `mode = if engine.is_some() { account.filing.mode } else { Off }`; `now = store::now()`; `self.store.sync_filing_mode(name, mode, &now)?`; `engine.version()?`.
   2. `map = if mode != Off { observe::resolve_folders(..)? } else { FolderMap::sources_only(&account) }`.
   3. Discovery and reconciliation over `map.watch` (step 3–4). Source folders behave exactly as today. For a `Category` or `Retired` folder, use `checkpoint_start_at(.., start_uid = snapshot.uid_next - 1)` instead of `checkpoint`. Use `stage_with` with `record_arrivals: mode != Off` and `known_targets` built from intents whose `target == folder`, `target_epoch == snapshot.uid_validity` and `target_uid.is_some()` (map `target_uid → (message_id, intent id)`). When the folder's record has `rescan_complete == false` and `rescan_epoch == Some(snapshot.uid_validity)` and it is a category folder, pass `RescanFilter { below_uid: rescan_below_uid, rfc_ids: rescan_members(..).rfc ids }`. Collect `reconcile_range_ids` results into `removed: Vec<String>` (used by Task 8).
   4. *(Task 7 inserts intent recovery here.)*
   5. Fetch and classify as today. After `process_one` returns, when `mode != Off` and the message (`canonical_id`, the alias target if the merge redirected it) has an occurrence in a source folder, call `store.ensure_placement(name, &canonical_id, &map.sources)`. Mail first seen only in a category folder gets its placement from arrival resolution (Task 8), which needs to know it had none.
   6. *(Task 8 inserts arrival resolution and re-evaluation here.)*
   7. With `mode != Off`: `update_rescan_completion` then `bootstrap_and_hydrate`.
   8. *(Task 7 inserts plan/apply here; Task 8 inserts done inference after it.)*
   9. With `mode != Off`, add `"filing": summary` to the response, `store.set_last_pass(name, &summary_json)`, and set `partial` when `summary.errors > 0`.
   - An error that downcasts to `engine::ConfigChanged` aborts the pass with `err(5, "mail engine configuration changed during operation")`. Any other error inside steps 2, 4, 6, 7 and 8 increments `summary.errors`, pushes a code-only problem string (no message text), and continues.

2. **`resolve_folders`**, with `mode != Off`:
   - `caps = engine.capabilities()?`. `writes_allowed = mode == Live && caps.move_supported`. Missing MOVE in `Live` → problem `"move_unsupported"`.
   - `conflicts = engine.alias_conflicts(sources ∪ category folder names)?`. Any conflict → `writes_allowed = false`, problem `"alias_conflict:<folder>"`. Conflicting folders are excluded from `watch`.
   - `folders = engine.list_folders()?`; `listed` = their names.
   - Each source folder gets a record (`origin = "source"`, `category_id = None`, `state = "ok"` if listed, else `"missing"`) and is watched as `Source`.
   - For each category whose `effective_folder() != "INBOX"`: `native = caps.personal_prefix + folder`, unless the prefix is empty or the folder already starts with it. Insert `categories[id] = CategoryFolder::Native(native)`. If `native` equals a source name (`INBOX` compared case-insensitively), push problem `"folder_collides_with_source:<id>"` and create no record. Otherwise apply spec "Folders" rule 3 when listed (`\Noselect`/`\NonExistent` → `noselect`; any role from `SPECIAL_USE_ROLES` or a denylisted name → `special_use`; `roles == Some(empty)` → `ok`, `role_verified = true`; `roles == None` → `needs_confirmation` unless `confirmed`). Record `adopted` origin and event `folder_adopted` the first time; event `folder_special_use` / `folder_needs_confirmation` on state change.
   - Rule 4 (not listed, never recorded): in `Live` with `writes_allowed`, `create_folder(native)`. Success → record `ok`, `created`, `role_verified`, event `folder_created`, `summary.folders_created += 1`. Error → record `error`, `summary.errors += 1`. In `DryRun` → push to `would_create`.
   - Rule 5 (not listed, previously recorded and not `retired`) → `missing`, event `folder_missing` on the transition.
   - Rule 6: `ok` category folder not subscribed (per `FolderInfo.subscribed`), `Live` with `writes_allowed` → `subscribe_folder`, then `subscribed = true`, event `folder_subscribed`; error → `error` state.
   - Rule 7: records with a `category_id` that no longer matches its category's native name → `retired`. A retired folder is watched (`Retired`) while any open intent targets it, any open revert references it, any pending arrival is in it, or any rescan-set row names it.
   - `Category` watch entries: records in state `ok` or `needs_confirmation`, minus alias conflicts and paused folders (paused folders keep being watched; `pause_reason` only blocks writes).
   - Categories whose effective folder is `INBOX` map to `CategoryFolder::Inbox`.
   - `checked_at = now` on every record touched.
3. **`update_rescan_completion`**: for each folder record with `rescan_complete == false`, set it to true when the folder's checkpoint is in `rescan_epoch` with `last_uid >= rescan_below_uid - 1`, and no message occurring in that folder still has `normalized IS NULL` with a non-terminal job.
4. **`bootstrap_and_hydrate`**:
   - When `filing_state.bootstrap_done` is false: `ensure_placement(.., &map.sources)` for every source-managed message with a fingerprint and an occurrence; then `set_bootstrap_done(true)`.
   - Hydration: placements whose message `size IS NULL`, grouped by home folder where `home_epoch` equals the folder's checkpoint epoch, at most 100 UIDs per folder per pass. `engine.envelopes(folder, uids)`, then `store.hydrate` for each returned UID; `summary.hydrated += n`.

- [ ] **Step 1: Write the failing tests**

Append to `tests/filing_observe.rs`:

```rust
use mailtriage::domain::FilingMode::{DryRun, Live, Off};

fn folder_state(h: &Harness, native: &str) -> Option<String> {
    h.service().store.folder_record("work", native).unwrap().map(|f| f.state)
}

#[test]
fn dry_run_creates_nothing_and_writes_nothing() {
    let h = Harness::new(DryRun);
    h.fake.deliver("INBOX", &mail("a", "Weekly newsletter", "Our newsletter"));
    let out = h.sync();
    assert_eq!(out["filing"]["mode"], "dry_run");
    assert_eq!(h.fake.write_calls(), 0);
    assert!(folder_state(&h, "Newsletters").is_none());
}

#[test]
fn live_creates_and_subscribes_category_folders_once() {
    let h = Harness::new(Live);
    h.sync();
    for name in ["Transactions", "Updates", "Newsletters", "Promotions", "Other"] {
        assert_eq!(folder_state(&h, name).as_deref(), Some("ok"), "{name}");
        assert!(h.fake.subscribed(name), "{name}");
    }
    assert!(folder_state(&h, "Correspondence").is_none(), "INBOX-target category has no folder");
    let creates = h.fake.calls().iter().filter(|c| c.starts_with("create ")).count();
    h.sync();
    assert_eq!(h.fake.calls().iter().filter(|c| c.starts_with("create ")).count(), creates);
}

#[test]
fn existing_folder_is_adopted_and_its_content_not_ingested() {
    let h = Harness::new(Live);
    h.fake.add_folder("Newsletters", &[]);
    for i in 0..3 {
        h.fake.deliver("Newsletters", &mail(&format!("old{i}"), "Old newsletter", "newsletter"));
    }
    h.sync();
    let f = h.service().store.folder_record("work", "Newsletters").unwrap().unwrap();
    assert_eq!(f.origin.as_deref(), Some("adopted"));
    let all = h.service().list("work", mailtriage::service::ListOptions { view: "all".into(), ..Default::default() }).unwrap();
    assert_eq!(all["total"], 0);
}

#[test]
fn special_use_and_unverified_folders_are_not_used() {
    let h = Harness::new(Live);
    h.fake.add_folder("Promotions", &["\\Junk"]);
    h.sync();
    assert_eq!(folder_state(&h, "Promotions").as_deref(), Some("special_use"));
    let h = Harness::new(Live);
    h.fake.set_capabilities(true, true, false);
    h.fake.add_folder("Updates", &[]);
    h.sync();
    assert_eq!(folder_state(&h, "Updates").as_deref(), Some("needs_confirmation"));
    assert_eq!(folder_state(&h, "Newsletters").as_deref(), Some("ok"), "created folders are verified");
}

#[test]
fn personal_namespace_prefix_is_applied() {
    let h = Harness::new(Live);
    h.fake.set_prefix("INBOX.", '.');
    h.sync();
    assert_eq!(folder_state(&h, "INBOX.Newsletters").as_deref(), Some("ok"));
}

#[test]
fn category_folder_colliding_with_a_source_is_reported() {
    let h = Harness::new(Live);
    h.fake.add_folder("Newsletters", &[]);
    h.edit(|c| {
        if let Some(mailtriage::domain::EngineConfig::Himalaya(e)) = &mut c.accounts.get_mut("work").unwrap().engine {
            e.mailboxes.push("Newsletters".into());
        }
    });
    let out = h.sync();
    assert!(out["filing"]["problems"].as_array().unwrap().iter().any(|p| p == "folder_collides_with_source:newsletters"));
}

#[test]
fn missing_move_capability_blocks_all_writes() {
    let h = Harness::new(Live);
    h.fake.set_capabilities(false, true, true);
    let out = h.sync();
    assert_eq!(h.fake.write_calls(), 0);
    assert!(out["filing"]["problems"].as_array().unwrap().iter().any(|p| p == "move_unsupported"));
}

#[test]
fn enabling_on_large_existing_mailbox_moves_nothing_and_hydrates_in_batches() {
    let h = Harness::new(Off);
    h.fake.set_rich_discovery(false);
    for i in 0..250 {
        h.fake.deliver_at("INBOX", &mail(&format!("m{i}"), "Can you review this newsletter?", "please review"), "2026-01-01T00:00:00+00:00");
    }
    let mut s = h.service();
    for _ in 0..3 {
        s.sync("work", 100).unwrap();
    }
    h.set_mode(Live);
    let first = h.sync();
    assert_eq!(first["filing"]["hydrated"], 100);
    assert_eq!(h.sync()["filing"]["hydrated"], 100);
    assert_eq!(h.sync()["filing"]["hydrated"], 50);
    assert!(h.fake.calls().iter().all(|c| !c.starts_with("move ") && !c.starts_with("flag ")));
    let placements = h.service().store.placements("work").unwrap();
    assert_eq!(placements.len(), 250);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --locked --test filing_observe`
Expected: FAIL (no `filing` key, no folder records).

- [ ] **Step 3: Implement observation and the sync restructure**

Implement the Behaviour section above. `FakeEngine` call-log strings must start with `create `, `subscribe `, `move ` and `flag ` for write operations, which the tests rely on.

- [ ] **Step 4: Run all tests**

Run: `cargo fmt && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`
Expected: PASS, including the unchanged `tests/sync.rs`.

- [ ] **Step 5: Commit**

```bash
git add -A src tests
git commit -m "Observe category folders, bootstrap placements and hydrate metadata during sync

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Writes and intent recovery

Builds planner input from the store, claims and applies flags and moves in session-bound batches, handles epoch races with reverts or quarantine, and recovers every intent state (spec "Writes and recovery").

**Files:**
- Create: `src/filing/inputs.rs`, `src/filing/apply.rs`, `src/filing/recover.rs`
- Modify: `src/filing/mod.rs`, `src/service.rs`, `src/filing/store.rs` (helpers below)
- Test: `tests/filing_writes.rs`

**Interfaces:**
- Consumes: Tasks 4–6.
- Produces:

```rust
// src/filing/inputs.rs
pub fn effective(record: &crate::store::Record, generation: &str) -> planner::Effective;
pub fn plan_input(store: &Store, ctx: &PassContext, map: &FolderMap, preview: bool) -> anyhow::Result<planner::PlanInput>;

// src/filing/apply.rs
pub fn apply(store: &mut Store, ctx: &PassContext, map: &FolderMap, plan: &planner::Plan, summary: &mut FilingSummary) -> anyhow::Result<()>;
pub(crate) fn dispatch_moves(store: &mut Store, ctx: &PassContext, folder: &str, epoch: u64, target: &str, claimed: &[(i64, u64)], summary: &mut FilingSummary) -> anyhow::Result<()>;
pub(crate) fn dispatch_reverts(store: &mut Store, ctx: &PassContext, summary: &mut FilingSummary) -> anyhow::Result<()>;

// src/filing/recover.rs
pub fn recover(store: &mut Store, ctx: &PassContext, map: &FolderMap, summary: &mut FilingSummary) -> anyhow::Result<()>;
pub(crate) fn mark_move_applied(store: &mut Store, ctx: &PassContext, intent: &Intent, home: (String, u64, u64), summary: &mut FilingSummary) -> anyhow::Result<()>;

// src/filing/store.rs additions
pub fn checkpoint_state(&self, account: &str, folder: &str) -> Result<Option<(u64 /*epoch*/, u64 /*last_uid*/, bool /*complete*/, Option<String> /*scanned_at*/)>>;
pub fn intent(&self, id: i64) -> Result<Option<Intent>>;
pub fn records_for_planning(&self, account: &str) -> Result<Vec<(crate::store::Record, Placement, MessageMeta)>>;
```

Behaviour:

1. **`effective`** mirrors `Service::item`: start from the classification JSON (`category_id`, `urgency`, `action_required`, `reasons`); apply overrides and set the matching `*_from_override` flags; `current = record.generation == generation && matches!(record.status, "ready" | "uncertain")`; `input_incomplete = reasons contains "input_incomplete"`. A message without a classification has `current = false`.
2. **`plan_input`**:
   - `categories` and `max_actions` come from config.
   - `folders`: sources → `FolderView { usable: Ok, is_source: true, epoch: checkpoint epoch }`; category records `ok` → `Ok`, `is_category`; other states → `Unusable`; each `map.would_create` entry → `WouldCreate`, `is_category`, `epoch: None`; `retired` records → `is_category: false`, `retired_listed = map.listed.contains(native)`. `paused = pause_reason.is_some()`.
   - `messages`: one `PlanMessage` per placement. `home = Some(Locator)` only when `location_state == Known` and all three home fields are set. `hydrated = meta.size.is_some()`. Open intents come from `intents(account, true)`.
   - `enabled_at` from `filing_state`. `preview` as given (`true` for `DryRun` passes and the `filing plan` command).
3. **`apply`** (mode `Live` and `map.writes_allowed`, else return):
   - For each `cleared_requests` entry, clear `desired_target` and bump `desired_rev` with `save_placement(expected_rev)`.
   - Flags, grouped by `(folder, epoch)`, in batches of at most 100: `(ctx.verify_binding)()`; `snapshot(F)`, `envelopes(F, uids)`, `snapshot(F)`; abort the batch unless both epochs equal `epoch`; drop UIDs absent, whose Message-ID/size differ from `message_meta`, or that already carry `\Flagged`; `claim_flag` each; `add_flagged(F, claimed uids)`:
     - `selected && session_epoch == Some(epoch) && completed` → intents `applied`, `flagged_at = now`, event `flagged`, `summary.flagged += 1`;
     - `selected && session_epoch != Some(epoch)` → intents `failed` (error `epoch_race`), folder `pause_reason = "epoch_race"`, event `epoch_race`;
     - otherwise (including `Err`) → intents `uncertain`.
   - Moves, grouped by `(from.folder, from.epoch, to)`: the same verification, then `snapshot(T)` → `(target_epoch, target_uid_next)`, `claim_move` each, then `dispatch_moves`.
   - `dispatch_moves` calls `move_messages(F, uids, T)` and maps the outcome per spec "Moves" step 4. On success with matching epoch: intents `sent`, `remove_occurrence` for each source locator, COPYUID target UIDs stored via `IntentPatch.target_uid`, `summary.moved += n`. Epoch race with COPYUID: insert one `filing_reverts` row per pair (`folder = T`, `folder_epoch = copyuid.target_epoch`, `uid = dst`, `target = F`, `target_epoch = session_epoch`), intents `awaiting_rescan` with error `epoch_race`, then call `dispatch_reverts`. Epoch race without COPYUID: F `pause_reason = "epoch_race"`, intents `awaiting_rescan` with error `epoch_race`, event `epoch_race`.
   - `dispatch_reverts` handles each `pending` revert: `snapshot(folder).uid_validity` must equal `folder_epoch` and the UID must still be present (`envelopes`), else the revert fails. Then `move_messages(folder, [uid], target)`; success needs `session_epoch == Some(folder_epoch)`. Applied → store the revert's own COPYUID target UID, `summary.reverted += 1`, event `epoch_race_reverted`. Failed or raced → `target` folder `pause_reason = "epoch_race"`, event `epoch_race`. A revert never creates another revert.
4. **`recover`** (step 5, mode not `Off`; writes only when `map.writes_allowed`) walks open intents oldest first, applying the spec's intent-recovery table exactly:
   - "In T" = the intent's message has an occurrence in T in T's current epoch with `uid >= target_uid_next` (or `== target_uid`), or an arrival with `intent_id == intent.id`.
   - "In F" = `envelopes(F, [uid])` returns the UID with a matching Message-ID/size, observed only while F's epoch is unchanged.
   - "T scanned as for lost" = `checkpoint_state(T)` is complete, in `target_epoch`, with `scanned_at > dispatched_at`, and no arrival in T at `uid >= target_uid_next` is `pending` or `unresolved`.
   - A retry re-claims by setting the intent back to `in_flight` (attempts + 1, `next_after` from the job backoff `30 * 2^min(attempts,7)` seconds), but only after `next_after` has passed and only when `intent.desired_rev` equals the placement's current `desired_rev`; otherwise the intent becomes `superseded`. Retries are dispatched through `dispatch_moves`. When `attempts >= ctx.max_attempts` the intent becomes `failed` and the placement gets `blocked_reason = "move_failed"` (event `move_failed`).
   - A suspected epoch race sets F's `pause_reason = "epoch_race_suspected"`, the intent `awaiting_rescan` with error `epoch_race_suspected`, and event `epoch_race`.
   - `awaiting_rescan` waits for the rescan of each folder whose epoch changed (`folder_record.rescan_complete` in the new epoch); then it decides from occurrences only.
   - `mark_move_applied` updates home to `(T, epoch, uid)`, sets `location_state = Known`, `absent_since = None`, `filed_at = now`, `filed_by = "mailtriage"`; clears `desired_target` (bumping `desired_rev`) only when `intent.desired_rev == placement.desired_rev`, and clears `eligible_once` only when `intent.consumes_eligible` and the revision matches. Then intent `applied`, event `moved`, and arrivals with that `intent_id` or at the new home locator → `resolved`/`own_move`.
   - Uncertain flag intents follow spec "Flags" step 3.
   - Pending reverts are re-dispatched through `dispatch_reverts`.
5. **Sync wiring:** step 4 (after reconciliation) calls `recover`. Step 8: `let plan = planner::plan(&inputs::plan_input(.., preview = mode == DryRun)?)`; `summary.planned = plan.actions.len()`; in `Live`, `apply`.

- [ ] **Step 1: Write the failing tests**

Create `tests/filing_writes.rs`:

```rust
mod common;
use common::{mail, Harness};
use mailtriage::{domain::FilingMode::{DryRun, Live}, engine::fake::{FakeOp, Fault}};

fn place(h: &Harness, mid: &str) -> Vec<(String, u64)> {
    h.fake.locate(&format!("<{mid}@test>"))
}

#[test]
fn live_files_new_mail_and_flags_actionable_mail() {
    let h = Harness::new(Live);
    h.sync(); // creates folders
    h.fake.deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.deliver("INBOX", &mail("i", "Invoice", "Payment due today"));
    h.fake.deliver("INBOX", &mail("c", "Lunch", "Can you join us?"));
    let out = h.sync();
    assert_eq!(out["filing"]["moved"], 2);
    assert_eq!(out["filing"]["flagged"], 2);
    assert_eq!(place(&h, "n")[0].0, "Newsletters");
    let (folder, uid) = place(&h, "i")[0].clone();
    assert_eq!(folder, "Transactions");
    assert!(h.fake.flags(&folder, uid).contains(&"\\Flagged".to_string()));
    let (folder, uid) = place(&h, "c")[0].clone();
    assert_eq!(folder, "INBOX", "correspondence targets INBOX");
    assert!(h.fake.flags(&folder, uid).contains(&"\\Flagged".to_string()));
    let writes = h.fake.write_calls();
    h.sync();
    assert_eq!(h.fake.write_calls(), writes, "second pass is quiet");
    let s = h.service();
    for p in s.store.placements("work").unwrap() {
        if p.home_folder.as_deref() != Some("INBOX") {
            assert_eq!(p.filed_by.as_deref(), Some("mailtriage"));
        }
    }
    assert!(s.store.intents("work", true).unwrap().is_empty());
}

#[test]
fn dry_run_then_live_files_mail_that_arrived_during_dry_run() {
    let h = Harness::new(DryRun);
    h.fake.deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.sync();
    h.sync();
    assert_eq!(h.fake.write_calls(), 0);
    h.set_mode(Live);
    h.sync();
    assert_eq!(place(&h, "n")[0].0, "Newsletters");
}

#[test]
fn backlog_is_untouched() {
    let h = Harness::new(Live);
    h.fake.deliver_at("INBOX", &mail("old", "Old newsletter", "Can you review this newsletter today?"), "2026-01-01T00:00:00+00:00");
    h.sync();
    h.sync();
    assert_eq!(place(&h, "old")[0].0, "INBOX");
    assert!(h.fake.calls().iter().all(|c| !c.starts_with("move ") && !c.starts_with("flag ")));
}

#[test]
fn lost_response_converges_without_a_second_move() {
    let h = Harness::new(Live);
    h.sync();
    h.fake.deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::ErrorAfter);
    h.sync();
    h.sync();
    h.sync();
    assert_eq!(h.fake.calls().iter().filter(|c| c.starts_with("move ")).count(), 1);
    assert_eq!(place(&h, "n")[0].0, "Newsletters");
    assert!(h.service().store.intents("work", true).unwrap().is_empty());
}

#[test]
fn failed_dispatch_is_retried_once_backoff_allows() {
    let h = Harness::new(Live);
    h.sync();
    h.fake.deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::ErrorBefore);
    h.sync();
    assert_eq!(place(&h, "n")[0].0, "INBOX");
    // Make the backoff elapse: retries are allowed when next_after <= now.
    h.service().store.expire_intent_backoff_for_tests("work").unwrap();
    h.sync();
    h.sync();
    assert_eq!(place(&h, "n")[0].0, "Newsletters");
}

#[test]
fn partial_copy_blocks_with_duplicate_copy() {
    let h = Harness::new(Live);
    h.sync();
    h.fake.deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::PartialCopy);
    h.sync();
    h.sync();
    h.sync();
    let s = h.service();
    let blocked: Vec<_> = s.store.placements("work").unwrap().into_iter().filter_map(|p| p.blocked_reason).collect();
    assert_eq!(blocked, vec!["duplicate_copy".to_string()]);
    assert_eq!(h.fake.calls().iter().filter(|c| c.starts_with("move ")).count(), 1);
}

#[test]
fn epoch_race_with_copyuid_is_reverted() {
    let h = Harness::new(Live);
    h.sync();
    h.fake.deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::EpochRaceBefore);
    let out = h.sync();
    assert_eq!(out["filing"]["reverted"], 1);
    assert_eq!(place(&h, "n")[0].0, "INBOX");
    let s = h.service();
    assert!(s.store.events("work", None, 50).unwrap().iter().any(|e| e["kind"] == "epoch_race_reverted"));
    assert!(s.store.events("work", None, 50).unwrap().iter().all(|e| e["kind"] != "client_correction"));
}

#[test]
fn epoch_race_without_copyuid_pauses_the_source() {
    let h = Harness::new(Live);
    h.fake.set_capabilities(true, false, true);
    h.sync();
    h.fake.deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::EpochRaceBefore);
    h.sync();
    h.sync();
    let f = h.service().store.folder_record("work", "INBOX").unwrap().unwrap();
    assert_eq!(f.pause_reason.as_deref(), Some("epoch_race"));
    let moves = h.fake.calls().iter().filter(|c| c.starts_with("move ")).count();
    h.fake.deliver("INBOX", &mail("m", "Another newsletter", "newsletter"));
    h.sync();
    assert_eq!(h.fake.calls().iter().filter(|c| c.starts_with("move ")).count(), moves, "paused source is not written");
}

#[test]
fn without_uidplus_identity_comes_from_the_fingerprint() {
    let h = Harness::new(Live);
    h.fake.set_capabilities(true, false, true);
    h.sync();
    h.fake.deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.sync();
    h.sync();
    h.sync();
    let s = h.service();
    let p = &s.store.placements("work").unwrap()[0];
    assert_eq!(p.home_folder.as_deref(), Some("Newsletters"));
    assert_eq!(p.filed_by.as_deref(), Some("mailtriage"));
    assert!(s.store.intents("work", true).unwrap().is_empty());
    let all = s.list("work", mailtriage::service::ListOptions { view: "all".into(), ..Default::default() }).unwrap();
    assert_eq!(all["total"], 1, "the moved copy merged into the original");
}

#[test]
fn flag_is_attempted_once_and_never_retried() {
    let h = Harness::new(Live);
    h.sync();
    h.fake.deliver("INBOX", &mail("c", "Lunch", "Can you join us?"));
    h.fake.inject(FakeOp::Flag, Fault::ErrorBefore);
    h.sync();
    h.sync();
    h.sync();
    assert_eq!(h.fake.calls().iter().filter(|c| c.starts_with("flag ")).count(), 1);
    let s = h.service();
    assert!(s.store.events("work", None, 50).unwrap().iter().any(|e| e["kind"] == "flag_failed"));
}
```

`expire_intent_backoff_for_tests` is a `#[doc(hidden)] pub fn` on `Store` that sets `next_after` of open intents to the past. Add it in this task.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --locked --test filing_writes`
Expected: FAIL.

- [ ] **Step 3: Implement inputs, apply, recover and the sync wiring**

Implement the Behaviour section. Keep each function under about 80 lines by extracting helpers (`verify_batch`, `outcome_for_moves`, `observe_in_target`, `observe_in_source`, `target_scanned`).

- [ ] **Step 4: Run all tests**

Run: `cargo fmt && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A src tests
git commit -m "Apply filing moves and flags with journaled intents, epoch-race handling and recovery

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Arrivals, location, transitions and done inference

Turns observations into corrections, pins, relocations and done state, and adds the explicit placement transitions used by the CLI.

**Files:**
- Create: `src/filing/arrivals.rs`, `src/filing/transitions.rs`, `src/filing/done.rs`
- Modify: `src/filing/mod.rs`, `src/filing/store.rs`, `src/service.rs`
- Test: `tests/filing_location.rs`

**Interfaces:**
- Consumes: Tasks 4–7.
- Produces:

```rust
// src/filing/arrivals.rs
pub fn resolve_arrivals(store: &mut Store, ctx: &PassContext, map: &FolderMap, summary: &mut FilingSummary) -> anyhow::Result<()>;
pub fn reevaluate(store: &mut Store, ctx: &PassContext, map: &FolderMap, message_ids: &[String], summary: &mut FilingSummary) -> anyhow::Result<()>;

// src/filing/transitions.rs
pub enum Transition { CategoryChanged { effective_category: Option<String> }, Pin, Unpin, Retry }
/// Applies an explicit transition (spec "Placement transitions"), resolving an ambiguous location first.
pub fn apply_transition(store: &mut Store, account: &str, message_id: &str, t: Transition, sources: &[String], now: &str) -> anyhow::Result<Placement>;
pub fn release_folder(store: &mut Store, account: &str, native: &str, now: &str) -> anyhow::Result<()>;
pub fn retry_arrival(store: &mut Store, account: &str, arrival_id: i64, generation: &str, now: &str) -> anyhow::Result<()>;
pub fn dismiss_arrival(store: &mut Store, account: &str, arrival_id: i64, now: &str) -> anyhow::Result<()>;
pub fn adopt_folder(store: &mut Store, account: &str, native: &str, now: &str) -> anyhow::Result<()>;

// src/filing/done.rs
pub fn infer_done(store: &mut Store, ctx: &PassContext, map: &FolderMap, summary: &mut FilingSummary) -> anyhow::Result<()>;

// src/filing/store.rs additions
/// One transaction: compare-and-swap overrides, then write the placement (desired_rev bumped by the caller).
pub fn correct_with_placement(&mut self, account: &str, id: &str, expected: &serde_json::Value, overrides: &serde_json::Value, placement: Option<&Placement>) -> Result<bool>;
/// UPDATE messages SET review_state='done' WHERE review_state='open' ...; returns whether it changed.
pub fn infer_done_row(&mut self, account: &str, id: &str) -> Result<bool>;
pub fn reopen_inferred(&mut self, account: &str, id: &str) -> Result<bool>;
pub fn clear_done_inferred(&mut self, account: &str, id: &str) -> Result<()>;
pub fn canonical_for_fingerprint(&self, account: &str, fingerprint: &str) -> Result<Option<String>>;
```

Behaviour:

1. **`resolve_arrivals`** (step 6 of the sync order, after fetch/classify; mode not `Off`), for each `pending` arrival oldest first:
   - message not fingerprinted: occurrence gone → `vanished`; job `terminal` → `unresolved` (event `arrival_unresolved`, `summary.unresolved += 1`); else leave pending;
   - message has an open move intent → leave pending;
   - `(folder, epoch, uid)` equals an open revert's locator → leave pending; equals an applied revert's `(target, target_uid)` → `resolved`/`reverted` and location-only re-evaluation;
   - quarantine window (an intent with error `epoch_race*`, `target == folder`, `target_epoch == epoch`, `uid >= target_uid_next`, not explained by any intent's `target_uid`) → `resolved`/`quarantined`, `ensure_placement`, `blocked_reason = "quarantined"`, `summary.quarantined += 1`;
   - `intent_id` set, or the arrival's `(folder, epoch, uid)` equals the placement's home locator → `resolved`/`own_move` (no change);
   - the folder is in rescan (`rescan_epoch == epoch && uid < rescan_below_uid`) → `resolved`/`rescan`, location-only re-evaluation;
   - message without placement: `ensure_placement`; in a category folder C, write override `category_id = C` (CAS), `home` = this occurrence, `filed_by = "user"`, event `client_correction` → `resolved`/`new`; in a source folder → `resolved`/`new`;
   - message with placement: batch-check home presence with `envelopes(home_folder, [home_uid])` (skip until rescanned if the home folder's epoch changed). Home present → `resolved`/`extra`. Absent and arrival in category C → client move transition (override `C` unless already, home here, `filed_by = "user"`, `pinned = false`, `desired_target = None`, rev bump, event `client_correction`, `summary.client_corrections += 1`) → `user_move`. Absent and arrival in a source folder → home here, `pinned = true`, `desired_target = None`, event `pinned`, `summary.pinned += 1` → `user_pin`.
   - Any transition that makes a placement `Known` again with `done_inferred` reopens it (`reopen_inferred`, event `reopened`).
2. **`reevaluate`** for the ids collected from reconciliation (and from write-batch verification drops), skipping messages with an open intent, applies spec "Placement re-evaluation". One surviving watched occurrence: a category folder of another category → client move transition; the same category → event `relocated`; a source folder → pin transition. During a rescan of the surviving folder, update location only. Several survivors → `Ambiguous` (event `location_ambiguous`); none → `Absent`, `absent_since = now`.
3. **`apply_transition`**: if the placement is `Ambiguous`, first select home deterministically (the occurrence in the newly desired folder; else a source-folder occurrence by configuration order, lowest UID; else the lowest `(folder, uid)`) and set `Known`. Then apply the row from spec "Placement transitions", always bumping `desired_rev`. Errors: a message without fingerprint → `ServiceError { code: 5, message: "identity not yet established; retry after sync" }`; a fingerprinted message without placement → code 2 `"message has no mailbox placement"`.
4. **`Service::correct`**: when filing is not `Off` and the patch sets or clears `category_id`, compute the new effective category, build the placement via `CategoryChanged`, and persist with `correct_with_placement` in one transaction. Otherwise keep today's path. `Service::review` calls `clear_done_inferred`.
5. **Merge conflict:** in `process_one`, when `attach` refuses a merge because the provisional message has local edits, mark that provisional's pending arrival `unresolved` (event `merge_conflict`) and set the canonical placement's (`canonical_for_fingerprint`) `blocked_reason = "merge_conflict"`, in addition to today's job failure.
6. **`infer_done`** (step 9, last): for each `Absent` placement whose `absent_since` is strictly earlier than this pass's `ctx.now` (so the absence was recorded in an earlier pass), apply the spec's conditions using `infer_done_row` (which only changes `review_state = 'open'` rows). Then set `done_inferred = 1`, event `archived_done`, `summary.archived_done += 1`.
7. **Sync wiring:** after fetch/classify, call `resolve_arrivals` then `reevaluate(&removed)`; after plan/apply, call `infer_done`.

- [ ] **Step 1: Write the failing tests**

Create `tests/filing_location.rs`:

```rust
mod common;
use common::{mail, Harness};
use mailtriage::{
    domain::FilingMode::Live,
    engine::fake::{FakeOp, Fault},
    service::ListOptions,
};
use serde_json::json;

fn filed(h: &Harness, mid: &str, subject: &str, body: &str) -> (String, String) {
    h.fake.deliver("INBOX", &mail(mid, subject, body));
    h.sync();
    h.sync();
    let s = h.service();
    let all = s.list("work", ListOptions { view: "all".into(), ..Default::default() }).unwrap();
    let id = all["items"].as_array().unwrap().iter()
        .find(|i| i["subject"] == subject).unwrap()["id"].as_str().unwrap().to_string();
    let p = s.store.placement("work", &id).unwrap().unwrap();
    (id, p.home_folder.unwrap())
}
fn at(h: &Harness, mid: &str) -> Vec<(String, u64)> {
    h.fake.locate(&format!("<{mid}@test>"))
}
fn item(h: &Harness, id: &str) -> serde_json::Value {
    h.service().read("work", id).unwrap()["item"].clone()
}

#[test]
fn client_move_to_another_category_is_a_correction() {
    let h = Harness::new(Live);
    h.sync();
    let (id, home) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    assert_eq!(home, "Newsletters");
    let (f, uid) = at(&h, "n")[0].clone();
    h.fake.client_move(&f, uid, "Updates");
    h.sync();
    h.sync();
    let it = item(&h, &id);
    assert_eq!(it["classification"]["category_id"], "updates");
    assert_eq!(it["placement"]["filed_by"], "user");
    assert_eq!(at(&h, "n")[0].0, "Updates", "mailtriage does not move it back");
}

#[test]
fn client_move_back_to_inbox_pins() {
    let h = Harness::new(Live);
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    let (f, uid) = at(&h, "n")[0].clone();
    h.fake.client_move(&f, uid, "INBOX");
    h.sync();
    h.sync();
    h.sync();
    assert_eq!(at(&h, "n")[0].0, "INBOX");
    assert_eq!(item(&h, &id)["placement"]["pinned"], true);
}

#[test]
fn archive_marks_done_and_return_reopens() {
    let h = Harness::new(Live);
    h.fake.add_folder("Archive", &["\\Archive"]);
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    let (f, uid) = at(&h, "n")[0].clone();
    let archived = h.fake.client_move(&f, uid, "Archive");
    for _ in 0..3 {
        h.sync();
    }
    assert_eq!(item(&h, &id)["review_state"], "done");
    h.fake.client_move("Archive", archived, "Newsletters");
    h.sync();
    h.sync();
    assert_eq!(item(&h, &id)["review_state"], "open");
}

#[test]
fn explicit_done_is_never_reopened_by_observation() {
    let h = Harness::new(Live);
    h.fake.add_folder("Archive", &["\\Archive"]);
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    h.service().review("work", &id, true).unwrap();
    let (f, uid) = at(&h, "n")[0].clone();
    let archived = h.fake.client_move(&f, uid, "Archive");
    for _ in 0..3 {
        h.sync();
    }
    h.fake.client_move("Archive", archived, "Newsletters");
    h.sync();
    h.sync();
    assert_eq!(item(&h, &id)["review_state"], "done");
}

#[test]
fn second_label_is_an_extra_occurrence() {
    let h = Harness::new(Live);
    h.sync();
    let (id, home) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    let (f, uid) = at(&h, "n")[0].clone();
    h.fake.client_copy(&f, uid, "Updates");
    h.sync();
    h.sync();
    let it = item(&h, &id);
    assert_eq!(it["classification"]["category_id"], "newsletters");
    assert_eq!(it["placement"]["folder"], home);
}

#[test]
fn copy_then_delete_relocates_with_correction() {
    let h = Harness::new(Live);
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    let (f, uid) = at(&h, "n")[0].clone();
    h.fake.client_copy(&f, uid, "Updates");
    h.sync();
    h.fake.client_delete(&f, uid);
    for _ in 0..3 {
        h.sync();
    }
    let it = item(&h, &id);
    assert_eq!(it["classification"]["category_id"], "updates");
    assert_eq!(it["placement"]["folder"], "Updates");
}

#[test]
fn several_survivors_are_ambiguous_until_pinned() {
    let h = Harness::new(Live);
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    let (f, uid) = at(&h, "n")[0].clone();
    h.fake.client_copy(&f, uid, "Updates");
    h.fake.client_copy(&f, uid, "Promotions");
    h.sync();
    h.fake.client_delete(&f, uid);
    for _ in 0..3 {
        h.sync();
    }
    assert_eq!(item(&h, &id)["placement"]["location_state"], "ambiguous");
    h.service().filing_pin("work", &id).unwrap();
    h.sync();
    h.sync();
    assert!(at(&h, "n").iter().any(|(f, _)| f == "INBOX"));
}

#[test]
fn cli_correction_moves_and_a_newer_request_survives_an_older_intent() {
    let h = Harness::new(Live);
    h.sync();
    h.fake.deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::ErrorAfter);
    h.sync(); // moved to Newsletters, response lost
    let s = h.service();
    let all = s.list("work", ListOptions { view: "all".into(), ..Default::default() }).unwrap();
    let id = all["items"][0]["id"].as_str().unwrap().to_string();
    h.service().correct("work", &id, json!({"category_id": "transactions"}), None).unwrap();
    for _ in 0..4 {
        h.sync();
    }
    assert_eq!(at(&h, "n")[0].0, "Transactions");
    assert_eq!(item(&h, &id)["classification"]["category_id"], "transactions");
}

#[test]
fn pin_supersedes_an_unsent_move() {
    let h = Harness::new(Live);
    h.sync();
    h.fake.deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::ErrorBefore);
    h.sync();
    let s = h.service();
    let id = s.list("work", ListOptions { view: "all".into(), ..Default::default() }).unwrap()["items"][0]["id"].as_str().unwrap().to_string();
    h.service().filing_pin("work", &id).unwrap();
    h.service().store.expire_intent_backoff_for_tests("work").unwrap();
    h.sync();
    h.sync();
    assert_eq!(at(&h, "n")[0].0, "INBOX");
    let s = h.service();
    assert!(s.store.intents("work", false).unwrap().iter().any(|i| i.state == "superseded"));
    assert!(s.store.placement("work", &id).unwrap().unwrap().blocked_reason.is_none());
}

#[test]
fn mail_delivered_into_a_category_folder_is_user_filed() {
    let h = Harness::new(Live);
    h.sync();
    h.fake.deliver("Updates", &mail("u", "Hello", "Plain mail"));
    h.sync();
    h.sync();
    let s = h.service();
    let all = s.list("work", ListOptions { view: "all".into(), ..Default::default() }).unwrap();
    let it = &all["items"][0];
    assert_eq!(it["classification"]["category_id"], "updates");
    assert_eq!(it["placement"]["filed_by"], "user");
    assert_eq!(at(&h, "u")[0].0, "Updates");
}

#[test]
fn category_folder_epoch_reset_reattaches_without_ingesting_or_done() {
    let h = Harness::new(Live);
    h.fake.add_folder("Newsletters", &[]);
    h.fake.deliver("Newsletters", &mail("pre", "Pre-existing", "newsletter"));
    h.sync();
    let (id, _) = filed(&h, "n", "Weekly newsletter", "Our newsletter");
    h.fake.reset_epoch("Newsletters");
    for _ in 0..4 {
        h.sync();
    }
    let s = h.service();
    let all = s.list("work", ListOptions { view: "all".into(), ..Default::default() }).unwrap();
    assert_eq!(all["total"], 1, "pre-existing content stays out");
    let it = item(&h, &id);
    assert_eq!(it["review_state"], "open");
    assert_eq!(it["placement"]["location_state"], "known");
    assert_eq!(it["placement"]["folder"], "Newsletters");
}

#[test]
fn merge_conflict_blocks_instead_of_losing_edits() {
    let h = Harness::new(Live);
    h.sync();
    let uid = h.fake.deliver("INBOX", &mail("d", "Hello", "Plain mail"));
    h.fake.client_copy("INBOX", uid, "Updates");
    let mut s = h.service();
    s.sync("work", 1).unwrap(); // processes one job; the Updates copy stays provisional
    let all = s.list("work", ListOptions { view: "all".into(), ..Default::default() }).unwrap();
    let provisional = all["items"].as_array().unwrap().iter()
        .find(|i| i["classification"]["state"] == "pending").unwrap()["id"].as_str().unwrap().to_string();
    s.review("work", &provisional, true).unwrap();
    for _ in 0..3 {
        h.sync();
    }
    let s = h.service();
    assert!(s.store.events("work", None, 100).unwrap().iter().any(|e| e["kind"] == "merge_conflict"));
    assert!(s.store.placements("work").unwrap().iter().any(|p| p.blocked_reason.as_deref() == Some("merge_conflict")));
}
```

`filing_pin` is the service method Task 9 exposes on the CLI; define it in this task as `pub fn filing_pin(&mut self, account: &str, id: &str) -> Result<Value>`. It calls `apply_transition(.., Transition::Pin, ..)` under the account's config lock (shared, as `correct` does) and returns `self.read(account, id)`. `Service::item` gains the `placement` object (spec "Commands"), with `null` when the message has no placement.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --locked --test filing_location`
Expected: FAIL.

- [ ] **Step 3: Implement arrivals, transitions, done inference and wiring**

Implement the Behaviour section and `Service::filing_pin`. Add `placement` to `Service::item`: `{folder, location_state, filed_by, pinned, flagged (flagged_at set or envelope has \Flagged), blocked_reason, pending_action}`. `pending_action` is `"move:<native>"` when the latest plan would move the message, computed cheaply as: `desired_target` resolved to a folder different from home, else `null`.

- [ ] **Step 4: Run all tests**

Run: `cargo fmt && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A src tests
git commit -m "Resolve arrivals into corrections, pins and done state with explicit transitions

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 9: Filing commands, reporting and documentation

Exposes everything to humans and agents through the CLI with JSON output, extends `doctor`, `categories apply` and `sync` reporting, and documents the feature.

**Files:**
- Modify: `src/service.rs`, `src/cli.rs`, `src/filing/observe.rs` (offline map), `src/filing/mod.rs` (`FilingSummary.capabilities`), `README.md`, `docs/hermes.md`, `docs/design.md`
- Test: `tests/filing_cli.rs`

**Interfaces:**
- Consumes: Tasks 1–8.
- Produces (all return `Result<serde_json::Value>` with `schema_version: 1`):

```rust
pub enum Backfill { Days(u32), All }
pub enum RetryTarget { Message(String), Folder(String), Arrival(i64) }
impl Service {
    pub fn filing_status(&mut self, account: &str) -> Result<Value>;
    pub fn filing_enable(&mut self, account: &str, mode: FilingMode) -> Result<Value>;
    pub fn filing_disable(&mut self, account: &str) -> Result<Value>;
    pub fn filing_plan(&mut self, account: &str, limit: usize) -> Result<Value>;
    pub fn filing_backfill(&mut self, account: &str, scope: Backfill, apply: bool) -> Result<Value>;
    pub fn filing_pin(&mut self, account: &str, id: &str) -> Result<Value>;     // from Task 8
    pub fn filing_unpin(&mut self, account: &str, id: &str) -> Result<Value>;
    pub fn filing_retry(&mut self, account: &str, target: RetryTarget) -> Result<Value>;
    pub fn filing_dismiss(&mut self, account: &str, arrival: i64) -> Result<Value>;
    pub fn filing_adopt(&mut self, account: &str, folder: &str) -> Result<Value>;
    pub fn filing_log(&mut self, account: &str, id: Option<&str>, limit: usize) -> Result<Value>;
}
// src/filing/observe.rs
/// FolderMap built from stored records and config only (no engine calls), for `filing plan`/`status`.
pub fn offline_map(store: &Store, account: &str, cfg: &AccountConfig) -> anyhow::Result<FolderMap>;
```

Behaviour:

1. **`filing_enable`**:
   - Fails with code 2 `"account has no mail engine configured"` when `engine_config()` is `None`.
   - Under the exclusive config lock (the same pattern as `apply_categories`), set `folder = Some(name)` for every category without one, set `filing.mode`, run `config::validate` (on failure, code 2 `"categories need a valid folder: <ids>"`), and save.
   - Returns `{account, mode, folders: {id: folder}}`. Idempotent.
2. **`filing_disable`** sets `mode = off` the same way.
3. **`apply_categories`** with filing not `off`: each incoming category without `folder` inherits the previous config's effective folder for the same id, or its own name when new. Then validate.
4. **`filing_status`** makes no engine calls. It returns:
   - `{account, mode (config), state_mode, enabled_at, bootstrap_done, capabilities (from last_pass), folders: [FolderRecord], paused_categories: [ids], intents: {state: count}`;
   - counts: `blocked`, `quarantined`, `ambiguous`, `unresolved_arrivals`, and `eligible_unfiled` (placements homed in a source folder, `Known`, not pinned, `filed_at` null, `eligible_once` set or new);
   - `problems` and `last_pass`.
5. **`filing_plan`** is read-only: `offline_map` builds would-create entries for categories without a record, using `last_pass.capabilities.personal_prefix` or `""`. It then calls `plan_input(.., preview = true)` and `planner::plan`, and returns `{mode, folders_to_create, actions (first limit), total}`. With mode `off` it returns empty lists.
6. **`filing_backfill`**: candidates are placements homed in a source folder, `Known`, not pinned, `filed_at` null, not blocked. `Days(n)` also requires a hydrated `internal_date >= now - n days`. Without `apply` → `{matched, items: [first 50 ids]}`. With `apply` → requires mode `live` (code 2 `"backfill --apply requires filing mode live"`) → sets `eligible_once = 1` and bumps `desired_rev` for each → `{applied}`.
7. **Commands:**
   - `filing_unpin` uses `Transition::Unpin`.
   - `filing_retry`: `Message` → `Transition::Retry`; `Folder` → `release_folder` (clears `pause_reason`, event `released`, and sets `rescan_complete = false` with `rescan_epoch = epoch` so the folder is treated as rescanning until fully scanned again); `Arrival` → `retry_arrival` (requeues the provisional message's job if its occurrence still exists).
   - `filing_dismiss` → `dismiss_arrival`. `filing_adopt` → `adopt_folder` (sets `confirmed = 1`; the next pass makes it `ok`).
   - Message commands return `self.read(..)`; the others return `{ok: true}` plus the affected object.
8. **`filing_log`** returns `{items: events}`, newest first, limit 1..=500 (default 50).
9. **`doctor`**, when mode is not `off` and an engine is configured, adds `filing: {mode, move_supported, uidplus, special_use, personal_prefix, alias_conflicts, folders: [{name, roles, subscribed}], problems}` using only `capabilities`, `alias_conflicts` and `list_folders`.
10. **`sync`** reporting comes from Tasks 6–8. Add `capabilities` to `FilingSummary` (serialized `EngineCapabilities`) so `status` can report them.
11. **CLI** (`src/cli.rs`): add `Command::Filing { #[command(subcommand)] command: FilingCommand }` with:
    - `status --account`;
    - `enable --account --mode dry-run|live`;
    - `disable --account`;
    - `plan --account [--limit 1..=500, default 50]`;
    - `backfill --account (--days N | --all) [--apply]`: clap `ArgGroup` requiring exactly one of `days`/`all`; `days` in 1..=3650;
    - `pin --account --id`, `unpin --account --id`;
    - `retry --account (--id ID | --folder NAME | --arrival N)`: `ArgGroup`, exactly one;
    - `dismiss --account --arrival N`;
    - `adopt --account --folder NAME`;
    - `log --account [--id ID] [--limit]`.
    - Map `dry-run` to `FilingMode::DryRun`. Errors go through `service_error` like every other command.
12. **Docs:**
    - `README.md` gains a "Filing into folders" section: what filing does; the three modes and the recommended rollout (`enable --mode dry-run` → `filing plan` for a few days → `--mode live` → `backfill`); flags; correcting by moving mail in any client; pins; archive means done; `filing status`/`log`; the safety rules (never deletes, never expunges, never removes flags, never changes read state); the provider check gate; and exit codes.
    - `docs/hermes.md` gains agent usage: read `filing status` before acting, use `filing plan` to preview, `correct --category` moves mail, `filing pin` keeps a message in the inbox, never loop on `filing retry`.
    - `docs/design.md` gains a dated note under "Purpose and first-release boundary" pointing to the filing spec for accounts that enable it.

- [ ] **Step 1: Write the failing tests**

Create `tests/filing_cli.rs`:

```rust
mod common;
use common::{mail, Harness};
use mailtriage::{
    domain::{Category, FilingMode},
    service::{Backfill, RetryTarget},
};
use serde_json::Value;
use std::{path::Path, process::Command};

fn run(cwd: &Path, args: &[&str]) -> (i32, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_mailtriage")).current_dir(cwd).args(args).output().unwrap();
    let v = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    (out.status.code().unwrap(), v)
}

#[test]
fn cli_enable_status_plan_disable_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    assert_eq!(run(d, &["init", "--json"]).0, 0);
    let (code, v) = run(d, &["filing", "enable", "--account", "work", "--mode", "dry-run", "--json"]);
    assert_eq!(code, 2);
    assert!(v["error"]["message"].as_str().unwrap().contains("engine"));
    std::fs::write(d.join("h.toml"), "[accounts.work]\nimap.server='imaps://x.test'\n").unwrap();
    let mut c: Value = serde_json::from_str(&std::fs::read_to_string(d.join("mailtriage.json")).unwrap()).unwrap();
    c["accounts"]["work"]["engine"] = serde_json::json!({"kind":"himalaya","binary":"/nonexistent/himalaya","config":"h.toml","account":"work","mailboxes":["INBOX"],"expected_version":"2.1.0","timeout_seconds":5,"max_output_bytes":100000});
    std::fs::write(d.join("mailtriage.json"), c.to_string()).unwrap();
    let (code, v) = run(d, &["filing", "enable", "--account", "work", "--mode", "dry-run", "--json"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["mode"], "dry_run");
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(d.join("mailtriage.json")).unwrap()).unwrap();
    assert!(saved["accounts"]["work"]["categories"].as_array().unwrap().iter().all(|c| c["folder"].is_string()));
    let (code, v) = run(d, &["filing", "status", "--account", "work", "--json"]);
    assert_eq!((code, v["mode"].clone()), (0, Value::from("dry_run")));
    let (code, v) = run(d, &["filing", "plan", "--account", "work", "--json"]);
    assert_eq!(code, 0);
    assert!(v["folders_to_create"].as_array().unwrap().iter().any(|f| f == "Newsletters"));
    let (code, _) = run(d, &["filing", "backfill", "--account", "work", "--days", "30", "--apply", "--json"]);
    assert_eq!(code, 2, "apply requires live");
    let (code, _) = run(d, &["filing", "backfill", "--account", "work", "--json"]);
    assert_eq!(code, 2, "days or all is required");
    let (code, v) = run(d, &["filing", "log", "--account", "work", "--json"]);
    assert_eq!((code, v["items"].as_array().unwrap().len()), (0, 0));
    let (code, v) = run(d, &["filing", "disable", "--account", "work", "--json"]);
    assert_eq!((code, v["mode"].clone()), (0, Value::from("off")));
}

#[test]
fn rename_keeps_folder_after_enable() {
    let h = Harness::new(FilingMode::Off);
    h.edit(|c| c.accounts.get_mut("work").unwrap().categories.iter_mut().for_each(|c| c.folder = None));
    h.service().filing_enable("work", FilingMode::Live).unwrap();
    let mut cats: Vec<Category> = h.service().config.accounts["work"].categories.clone();
    for c in cats.iter_mut() {
        if c.id == "newsletters" {
            c.name = "News Digest".into();
        }
        c.folder = None;
    }
    h.service().apply_categories("work", cats).unwrap();
    let news = h.service().config.accounts["work"].categories.iter().find(|c| c.id == "newsletters").unwrap().clone();
    assert_eq!(news.effective_folder(), "Newsletters");
    h.sync();
    assert!(h.fake.calls().iter().all(|c| !c.contains("News Digest")));
}

#[test]
fn status_plan_backfill_and_log_report_state() {
    let h = Harness::new(FilingMode::Live);
    h.sync();
    h.fake.deliver_at("INBOX", &mail("old", "Old newsletter", "newsletter"), "2026-01-01T00:00:00+00:00");
    h.sync();
    let mut s = h.service();
    let plan = s.filing_plan("work", 50).unwrap();
    assert_eq!(plan["total"], 0, "backlog is not planned");
    let b = s.filing_backfill("work", Backfill::All, false).unwrap();
    assert_eq!(b["matched"], 1);
    assert_eq!(s.filing_backfill("work", Backfill::All, true).unwrap()["applied"], 1);
    assert_eq!(s.filing_plan("work", 50).unwrap()["total"], 1);
    h.sync();
    assert_eq!(h.fake.locate("<old@test>")[0].0, "Newsletters");
    let status = h.service().filing_status("work").unwrap();
    assert_eq!(status["mode"], "live");
    assert!(status["folders"].as_array().unwrap().iter().any(|f| f["native"] == "Newsletters" && f["state"] == "ok"));
    let log = h.service().filing_log("work", None, 50).unwrap();
    assert!(log["items"].as_array().unwrap().iter().any(|e| e["kind"] == "moved"));
}

#[test]
fn unpin_retry_folder_and_adopt() {
    let h = Harness::new(FilingMode::Live);
    h.fake.set_capabilities(true, true, false);
    h.fake.add_folder("Updates", &[]);
    h.sync();
    assert_eq!(h.service().filing_status("work").unwrap()["folders"].as_array().unwrap().iter()
        .find(|f| f["native"] == "Updates").unwrap()["state"], "needs_confirmation");
    h.service().filing_adopt("work", "Updates").unwrap();
    h.sync();
    let f = h.service().store.folder_record("work", "Updates").unwrap().unwrap();
    assert_eq!(f.state, "ok");
    let mut rec = f.clone();
    rec.pause_reason = Some("epoch_race".into());
    h.service().store.save_folder(&rec).unwrap();
    h.service().filing_retry("work", RetryTarget::Folder("Updates".into())).unwrap();
    assert!(h.service().store.folder_record("work", "Updates").unwrap().unwrap().pause_reason.is_none());
}

#[test]
fn placement_is_null_with_filing_off_and_doctor_reports_filing() {
    let h = Harness::new(FilingMode::Off);
    h.fake.deliver("INBOX", &mail("a", "Hello", "Plain"));
    h.sync();
    let mut s = h.service();
    let all = s.list("work", mailtriage::service::ListOptions { view: "all".into(), ..Default::default() }).unwrap();
    assert!(all["items"][0]["placement"].is_null());
    h.set_mode(FilingMode::Live);
    let d = h.service().doctor("work").unwrap();
    assert_eq!(d["filing"]["move_supported"], true);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --locked --test filing_cli`
Expected: FAIL.

- [ ] **Step 3: Implement service methods, CLI and docs**

Implement the Behaviour section. `doctor` currently builds the engine with `Himalaya::new(h).and_then(|h| h.version())`. Keep that for the `transport` field, and add the filing block through `self.engine(&account)`, which uses the injected FakeEngine in tests.

- [ ] **Step 4: Run all tests**

Run: `cargo fmt && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`
Expected: PASS, including the unchanged `tests/cli.rs`.

- [ ] **Step 5: Commit**

```bash
git add -A src tests README.md docs
git commit -m "Add filing CLI commands, status reporting and documentation

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Dovecot end-to-end test and provider checklist

Proves the whole flow against a real IMAP server with the real Himalaya v2.1.0 binary, in two namespace layouts, and records the live provider checklist the user must complete before using `live` on a real mailbox.

**Files:**
- Create: `tests/e2e/dovecot-flat.conf`, `tests/e2e/dovecot-prefix.conf`, `tests/e2e/run.sh`, `tests/e2e/imap_client.py`, `tests/e2e/himalaya.toml.in`, `tests/e2e_dovecot.rs`, `.github/workflows/e2e.yml`
- Modify: `docs/verification.md`

**Interfaces:**
- Consumes: the built `mailtriage` binary (`CARGO_BIN_EXE_mailtriage`), a Himalaya v2.1.0 binary, Docker.
- Produces: `cargo test --locked --test e2e_dovecot -- --ignored --test-threads=1`, which runs when `MT_E2E_HIMALAYA`, `MT_E2E_PORT` and `MT_E2E_LAYOUT` (`flat` | `prefix`) are set, and otherwise skips with a printed notice.

- [ ] **Step 1: Write the Dovecot configs and the runner**

`tests/e2e/dovecot-flat.conf` (Dovecot 2.3):

```
protocols = imap
listen = *
ssl = no
disable_plaintext_auth = no
auth_mechanisms = plain login
mail_location = maildir:/srv/mail/%u
first_valid_uid = 1000
passdb {
  driver = static
  args = password=e2e-pass
}
userdb {
  driver = static
  args = uid=1000 gid=1000 home=/srv/mail/%u
}
namespace inbox {
  inbox = yes
  separator = /
  prefix =
  mailbox Sent {
    special_use = \Sent
    auto = create
  }
  mailbox Trash {
    special_use = \Trash
    auto = create
  }
  mailbox Archive {
    special_use = \Archive
    auto = create
  }
}
log_path = /dev/stderr
```

`tests/e2e/dovecot-prefix.conf`: identical except `separator = .` and `prefix = INBOX.`.

`tests/e2e/run.sh LAYOUT PORT` does the following:
- starts `docker run -d --rm --name mt-e2e-$LAYOUT -p $PORT:143 -v $PWD/tests/e2e/dovecot-$LAYOUT.conf:/etc/dovecot/dovecot.conf:ro dovecot/dovecot:2.3.21`;
- waits until `python3 tests/e2e/imap_client.py --port $PORT ping` succeeds (30 s timeout);
- runs the e2e test with `MT_E2E_LAYOUT=$LAYOUT MT_E2E_PORT=$PORT`;
- always stops the container (`trap`).

It requires `MT_E2E_HIMALAYA` to be set by the caller.

`tests/e2e/imap_client.py` uses only Python's standard `imaplib`, user `e2e`, password `e2e-pass`. It has these subcommands, each printing JSON:
- `ping`;
- `append FOLDER FILE`;
- `locate MESSAGE_ID` (prints `[[folder, uid, flags], ...]` across all selectable mailboxes, selected read-only with `EXAMINE`);
- `move FOLDER MESSAGE_ID TARGET` (via `UID SEARCH HEADER Message-ID` and `UID MOVE`);
- `delete FOLDER MESSAGE_ID` (`\Deleted` + `EXPUNGE`; this is the simulated *user*, not mailtriage);
- `reset-epoch FOLDER CONTAINER` (`docker exec CONTAINER doveadm mailbox update -u e2e --uid-validity <now> FOLDER`).

`tests/e2e/himalaya.toml.in` holds the Himalaya v2.1.0 account definition for `imap://127.0.0.1:@PORT@` with plain login `e2e` / `e2e-pass` and no TLS. Before writing it, read Himalaya v2.1.0's configuration reference (`himalaya --help`, plus the `config.sample.toml` in the v2.1.0 source via `gh api repos/pimalaya/himalaya/contents/config.sample.toml?ref=v2.1.0`), and confirm it works with `himalaya --config <file> --account e2e --backend imap imap list --all` against the container. The repo's existing test fixtures use `imap.server` and `imap.sasl.plain.username`; follow the sample for the password and for disabling TLS.

- [ ] **Step 2: Write the e2e test**

`tests/e2e_dovecot.rs`:

```rust
//! Real IMAP end-to-end test. Run through tests/e2e/run.sh; skipped unless MT_E2E_* is set.
use serde_json::Value;
use std::{path::{Path, PathBuf}, process::Command};

struct Env { himalaya: String, port: String, layout: String, dir: tempfile::TempDir }

fn env() -> Option<Env> {
    Some(Env {
        himalaya: std::env::var("MT_E2E_HIMALAYA").ok()?,
        port: std::env::var("MT_E2E_PORT").ok()?,
        layout: std::env::var("MT_E2E_LAYOUT").ok()?,
        dir: tempfile::tempdir().unwrap(),
    })
}
fn imap(e: &Env, args: &[&str]) -> Value {
    let out = Command::new("python3").arg("tests/e2e/imap_client.py").arg("--port").arg(&e.port).args(args).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}
fn mt(e: &Env, args: &[&str]) -> Value {
    let mut full = vec!["--config", "mailtriage.json"];
    full.extend_from_slice(args);
    full.push("--json");
    let out = Command::new(env!("CARGO_BIN_EXE_mailtriage")).current_dir(e.dir.path()).args(&full).output().unwrap();
    serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
}
fn folder(e: &Env, name: &str) -> String {
    if e.layout == "prefix" && name != "INBOX" { format!("INBOX.{name}") } else { name.to_string() }
}
fn location(e: &Env, mid: &str) -> Vec<(String, Vec<String>)> {
    imap(e, &["locate", mid]).as_array().unwrap().iter()
        .map(|r| (r[0].as_str().unwrap().to_string(), r[2].as_array().unwrap().iter().map(|f| f.as_str().unwrap().to_string()).collect()))
        .collect()
}
fn write_mail(dir: &Path, name: &str, mid: &str, subject: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, format!("Message-ID: {mid}\r\nFrom: Alex <alex@example.com>\r\nTo: e2e@example.test\r\nSubject: {subject}\r\n\r\n{body}\r\n")).unwrap();
    p
}

#[test]
#[ignore]
fn filing_against_real_dovecot() {
    let Some(e) = env() else {
        eprintln!("MT_E2E_* not set; skipping");
        return;
    };
    // 1. Config: init, then engine + live filing + explicit folders (correspondence -> INBOX).
    assert_eq!(mt(&e, &["init"])["schema_version"], 1);
    let template = std::fs::read_to_string("tests/e2e/himalaya.toml.in").unwrap();
    std::fs::write(e.dir.path().join("himalaya.toml"), template.replace("@PORT@", &e.port)).unwrap();
    let cfg_path = e.dir.path().join("mailtriage.json");
    let mut cfg: Value = serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    let account = &mut cfg["accounts"]["work"];
    account["engine"] = serde_json::json!({
        "kind": "himalaya", "binary": e.himalaya, "config": "himalaya.toml", "account": "e2e",
        "mailboxes": ["INBOX"], "expected_version": "2.1.0", "timeout_seconds": 30, "max_output_bytes": 20000000
    });
    account["filing"] = serde_json::json!({"mode": "live", "flag": true, "max_actions_per_pass": 200});
    for c in account["categories"].as_array_mut().unwrap() {
        let folder = if c["id"] == "correspondence" { "INBOX".to_string() } else { c["name"].as_str().unwrap().to_string() };
        c["folder"] = Value::from(folder);
    }
    std::fs::write(&cfg_path, cfg.to_string()).unwrap();
    assert_eq!(mt(&e, &["doctor", "--account", "work"])["filing"]["move_supported"], true);
    mt(&e, &["sync", "--account", "work"]);
    let n = write_mail(e.dir.path(), "n.eml", "<n@e2e>", "Weekly newsletter", "Our newsletter");
    let i = write_mail(e.dir.path(), "i.eml", "<i@e2e>", "Invoice", "Payment due today");
    imap(&e, &["append", "INBOX", n.to_str().unwrap()]);
    imap(&e, &["append", "INBOX", i.to_str().unwrap()]);
    for _ in 0..2 { mt(&e, &["sync", "--account", "work"]); }
    let ln = location(&e, "<n@e2e>");
    assert_eq!(ln[0].0, folder(&e, "Newsletters"));
    assert!(!ln[0].1.contains(&"\\Seen".to_string()), "read state preserved");
    let li = location(&e, "<i@e2e>");
    assert_eq!(li[0].0, folder(&e, "Transactions"));
    assert!(li[0].1.contains(&"\\Flagged".to_string()));
    // 2. Client move = correction.
    imap(&e, &["move", &folder(&e, "Newsletters"), "<n@e2e>", &folder(&e, "Updates")]);
    for _ in 0..2 { mt(&e, &["sync", "--account", "work"]); }
    let list = mt(&e, &["list", "--account", "work", "--view", "all"]);
    let item = list["items"].as_array().unwrap().iter().find(|x| x["subject"] == "Weekly newsletter").unwrap().clone();
    assert_eq!(item["classification"]["category_id"], "updates");
    // 3. Client move back to INBOX = pin.
    imap(&e, &["move", &folder(&e, "Transactions"), "<i@e2e>", "INBOX"]);
    for _ in 0..3 { mt(&e, &["sync", "--account", "work"]); }
    assert_eq!(location(&e, "<i@e2e>")[0].0, "INBOX");
    // 4. Client delete = done.
    imap(&e, &["delete", &folder(&e, "Updates"), "<n@e2e>"]);
    for _ in 0..3 { mt(&e, &["sync", "--account", "work"]); }
    let read = mt(&e, &["read", "--account", "work", "--id", item["id"].as_str().unwrap()]);
    assert_eq!(read["item"]["review_state"], "done");
    // 5. Epoch reset of INBOX keeps the pinned message known and open.
    imap(&e, &["reset-epoch", "INBOX", &format!("mt-e2e-{}", e.layout)]);
    for _ in 0..4 { mt(&e, &["sync", "--account", "work"]); }
    let status = mt(&e, &["filing", "status", "--account", "work"]);
    assert_eq!(status["ambiguous"], 0);
    assert_eq!(location(&e, "<i@e2e>")[0].0, "INBOX");
}
```

- [ ] **Step 3: Pin Himalaya and add the CI workflow**

- Find the Linux x86_64 asset of Himalaya v2.1.0: `gh release view v2.1.0 -R pimalaya/himalaya --json assets`.
- Download it into the scratch directory: `gh release download v2.1.0 -R pimalaya/himalaya -p '<asset>' -D <scratch>`.
- Compute `shasum -a 256` and pin both the URL and the checksum in `.github/workflows/e2e.yml`.

The workflow runs on `push` to `main`, `pull_request` and `workflow_dispatch`, with a single `ubuntu-24.04` job:

1. checkout (`persist-credentials: false`), `dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2`;
2. download the pinned Himalaya asset with `curl -fsSL`, verify with `sha256sum -c`, extract, and `export MT_E2E_HIMALAYA=$PWD/himalaya`;
3. `cargo build --locked`;
4. `bash tests/e2e/run.sh flat 31143` and `bash tests/e2e/run.sh prefix 31144`.

Use the same action versions as `.github/workflows/build.yml`.

- [ ] **Step 4: Run locally when Docker is available**

Run: `docker info >/dev/null 2>&1 && MT_E2E_HIMALAYA=$(which himalaya) bash tests/e2e/run.sh flat 31143 && MT_E2E_HIMALAYA=$(which himalaya) bash tests/e2e/run.sh prefix 31144`

On macOS the local `himalaya` is v2.1.0 from Homebrew. Expected: PASS for both layouts. If the Docker daemon is not running, record "not run locally: Docker daemon unavailable" in the task report; CI covers it on the next push. Also run `cargo test --locked` to confirm the ignored test does not run by default.

- [ ] **Step 5: Add the provider checklist to `docs/verification.md`**

Append a section "Live provider check (required before `live` on a real mailbox)". It has one table per provider (Gmail, Microsoft 365, iCloud, Fastmail or Dovecot host), with rows copied from the spec's "Live provider check" bullets and columns `Result`, `Evidence`, `Date`. Below the tables, add the go / go with differences / no-go outcome line. State that the Dovecot e2e job covers the protocol contract but not provider behaviour.

- [ ] **Step 6: Commit**

```bash
git add -A tests .github docs
git commit -m "Add Dovecot end-to-end filing test, CI job and provider checklist

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Human gate: live provider check

Not executed by agents. Before setting `filing.mode` to `live` on a real mailbox, the user:

1. Creates one throwaway account per provider (Gmail, Microsoft 365, iCloud, Fastmail or a Dovecot host) and configures each in Himalaya.
2. Works through the checklist added to `docs/verification.md` in Task 10, filling in results and evidence.
3. Records go / go with differences / no-go per provider, and adds any no-go to the README.

Until then, `dry_run` is safe on any mailbox: it never writes.
