# Local Kev Provider Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Classify mail without OpenRouter: a new provider kind, `kev`, talks to a Kev server that mailtriage installs and runs on demand on an Apple Silicon Mac, or to a Kev server you run anywhere else.

**Architecture:** Task 1 lets `DecisionProvider::key_account` see the config and adds `ProviderConfig.managed`. Task 2 moves OpenRouter's System One wire code into `provider/systemone.rs` (byte for byte) and adds `provider/kev.rs` for a Kev server at any URL. Task 3 adds `src/kev/` (layout, `installed.json`, the hidden `mailtriage kev run` supervisor, `ensure_running`) and a `ProviderUnavailable` error that skips classification like a missing key. Task 4 adds the setup paths (menu, flags, the resumable managed install, the URL check and key steps for your own server). Task 5 adds `doctor`'s `provider.kev` block and `self uninstall`'s note. Task 6 writes the docs. Task 7 is a manual live check on this Mac.

**Tech Stack:** Rust 2021 (anyhow, serde_json with sorted maps, reqwest blocking with rustls, clap derive, fs2 `flock`, ctrlc with `termination`); POSIX `sh` stand-ins for `git`, `uv` and `brew`; the test binary itself as the stand-in Kev server; VitePress 1.6.4 on Bun for the docs.

**Spec:** `design/specs/2026-10-11-local-kev-provider-design.md` (source of truth), building on `design/specs/2026-10-09-provider-adapter-design.md` and `design/specs/2026-10-05-guided-setup-design.md`. Docs voice: `design/specs/2026-10-09-docs-site-design.md`, section "Voice". Read the Kev spec before your task.

## Global Constraints

- Compatibility: "OpenRouter and `fake` configs, requests, generation hashes and outputs are unchanged." `PINNED_BODY`, `openrouter_failures_keep_todays_errors`, `the_openrouter_request_is_byte_identical`, and `generation_hash_is_unchanged_by_filing_and_folder_fields` / `generation_hash_ignores_the_key_command` (expected hash `6dfd7bf30e6bf1c0b27ee97af991ecf21dc5ac0400044c2f3adbda7f79d37514`) pass without edits. The only existing assertions this plan changes are the ones the spec itself changes: the registry's kind list and `key_account` call (Tasks 1 and 2) and the setup menu test (Task 4).
- `KINDS` becomes `["openrouter", "kev", "fake"]`.
- Release pins, verbatim: git tag `kev-1.0` of `https://github.com/jaredpalmer/kev.git`; checkpoint `jaredpalmer/kev-4b@v1.0`; `model: "kev-latest"`; managed endpoint `http://127.0.0.1:8009/v1/systemone`; `timeout_seconds: 60` for a new `kev` config.
- URL rule: "`http` only for `127.0.0.1`, `localhost` and `[::1]`; any other host needs `https` and a key. A managed endpoint must be on this machine." Error text: `provider.endpoint must use https for a server off this machine`.
- `ProviderConfig.managed: Option<ManagedKev>` (one field, `checkpoint`) is serialized only when set; `managed` on any kind other than `kev` is an error: `provider.managed is only for kind kev`.
- Lifecycle numbers: stop after 15 minutes without a request, checked every 30 seconds; TERM, then KILL after 10 seconds; `ensure_running` polls `/v1/models` every second for up to 120 seconds; setup waits up to 30 minutes for the first start.
- Managed install location: `<data dir>/mailtriage/kev/` where `<data dir>` is `distribution::himalaya::default_data_dir()`; weights in `kev/hf` (`HF_HOME`).
- Managed Kev only on macOS arm64. Elsewhere `--provider kev` without `--kev-url` exits 2 with `step 5 (classifier): local Kev needs an Apple Silicon Mac; run your own Kev server and pass --kev-url`.
- Keys: no API key is ever written to a file, log, JSON output or doc example. A remote Kev server's key uses the existing key-store machinery (`secrets::read_command` / `store_command`) under account `kev`. The key is sent only to a server off this machine.
- Every setup error starts with `step N (name): ` and names the flag or command that fixes it (a `--` flag or a backticked command).
- Tests need no Python, `uv`, network or real Kev: stand-in executables on `PATH` (or named by absolute path), an in-process loopback HTTP fixture, and the test binary itself as the stand-in Kev server (see Task 3). Platform, disk, memory, port and wait times for tests come from the debug-only `MAILTRIAGE_TEST_KEV` JSON (release builds never read it), like `MAILTRIAGE_TEST_HIMALAYA_VERSIONS`.
- Tests never touch the real HOME, keychain, `launchctl`, `systemctl` or `~/.local/share/mailtriage/kev`: each sets `HOME`/`XDG_DATA_HOME` to a temp dir or builds a `kev::Layout` in one. In-process tests that read the user's data dir use a checkpoint no install has (`test/never-installed@v0`), so they never find a real install and never write there.
- Every task ends green with exactly what CI runs (`.github/workflows/build.yml`): `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`. Task 6 also runs the docs build (`cd docs && bun install --frozen-lockfile && bun run build`) and the em-dash check.
- One commit per task on local `main`, message as given in the task's last step, ending with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never push.
- User-facing docs follow the voice rules (you for the reader; problem, then solution, then term; never open with "AI"; limits in a `::: warning Important` callout at the feature; short paragraphs; no em dashes; "wirdrei.digital" lower case; US spelling).

## Rulings on spec gaps

Decided while writing this plan. Implementers and reviewers take them as given.

1. **Where `kev.log` lives.** The spec writes `<logs>/kev.log`, but `<logs>` is per config (`<config dir>/logs`) and one Kev server serves every config of the user. The log is `<data dir>/mailtriage/kev/kev.log`, inside the directory that holds everything else.
2. **One directory holds everything, uv included.** Every `uv` call gets `HF_HOME=kev/hf`, `UV_CACHE_DIR=kev/uv-cache` and `UV_PYTHON_INSTALL_DIR=kev/python`, so the Python uv fetches and its package cache are also under `kev/` and `rm -rf kev` removes all of it.
3. **`installed.json` also records `uv`** (its absolute path at install time): `{"tag", "checkpoint", "installed_at", "uv"}`. The background service's `PATH` may lack the Homebrew directory uv came from, so the supervisor gets `--uv PATH` instead of searching.
4. **`mailtriage kev run`'s flags:** `--dir DIR --port PORT --checkpoint ID --uv PATH [--allow-download] [--idle-seconds 900] [--check-seconds 30] [--stop-grace-seconds 10]`. `ensure_running` and setup pass all of them; the timing flags exist so tests run in seconds.
5. **Stopping the server (setup's step 8).** The supervisor writes its diagnostic PID into `server.lock` after taking the lock and stops Kev on SIGTERM (ctrlc's `termination` handler) or a layout-scoped stop request. `kev::runtime::stop` requests the current lock holder to stop and requires both lock release and the termination proof in `server-ownership.json`. It never signals a persisted PID. Review correction C5 and its regression tests establish this safer protocol.
6. **How `ProviderUnavailable` skips.** `DecisionProvider` gains `prepare(&self, config, key) -> Result<(), ProviderUnavailable>` (default: ready). `Service` calls it once per command, after the key check and before any job is leased, and caches the answer like the key. `Kev::prepare` runs `ensure_running` for managed configs and `GET /v1/models` for your own server: a connection failure there is "your own server unreachable". A `ProviderUnavailable` that `decide` raises later (a managed server that dies and does not come back) gives the job back without an attempt (`Store::unlease`) and ends that command's classification with the same `classification: {skipped: true, reason}`. A failed request after `prepare` succeeded (`Kev server request failed`, an HTTP error, a contract failure) counts an attempt.
7. **A managed endpoint is exactly `http://127.0.0.1:PORT/v1/systemone`.** The supervisor binds Kev to `127.0.0.1`, so `localhost` or `[::1]` would not reach it.
8. **More URL rules:** no query, no fragment, no user name or password in `provider.endpoint` (a password there would put a key in the config file).
9. **Order for your own server.** The spec checks `/v1/models` "with the key when one is needed" and then runs "today's key steps". The key is needed for the check, so setup runs the key steps first, then checks with the key it can resolve. A 401 or 403 when no key can be resolved here (an environment variable set only where mailtriage runs) is accepted with a note.
10. **Flags.** `--kev-url` alone implies `--provider kev`; with another `--provider` it exits 2. `--model` works with both Kev paths and is not asked; the default is `kev-latest`.
11. **The managed install runs inside step 5**, as the spec lists it, with the account's questions for its check classification: setup computes the categories (step 6 shows them) before step 5. A kept managed classifier (`setup --update` without classifier flags) runs the install too; finished it is a no-op, so `setup --update` is the fix `doctor` names.
12. **Confirming the download without prompts** needs `--yes`; otherwise step 5 exits 2 naming `--yes`. `SetupArgs` gains `yes` and `kev_url`.
13. **`doctor`** gains `provider.kev`; its `ready` joins the top-level `ready`; `live_checks_performed` is `true` for `kev` (doctor sends `GET /v1/models`). `doctor` never starts Kev.
14. **Key texts per kind.** OpenRouter's stay character for character. Kev's: `Kev server needs api_key_command or api_key_env`, `Kev server API key environment variable is missing` / `... is empty`, and setup's prompts say "Kev server key" where OpenRouter's say "OpenRouter key" (`provider::key_owner`).
15. **The menu** has four entries in `KINDS` order (local Kev only on an Apple Silicon Mac); `Kev::label()` is "Kev server you run (enter its URL)" and local Kev's label is the setup constant `LOCAL_KEV_LABEL`. `Kev::new_config()` returns the managed block (port 8009).
16. **Platform detection** is `update::release::platform() == Some("macos-arm64")`; free space comes from `/bin/df -Pk` on the nearest existing ancestor of `kev/`, memory from `/usr/sbin/sysctl -n hw.memsize`. When either cannot be read, setup goes ahead without the check.
17. **The "Run your own Kev server" guide section** goes on `docs/guide/provider.md`, whose path stays (other pages link it) while its title becomes "Classifier and models". The new page is `docs/guide/local-kev.md`.

## Review Focus

### Implementation review corrections (2026-10-11)

The design and task order are sufficient. These corrections override the corresponding snippets and new test expectations below; the existing OpenRouter compatibility assertions remain unchanged.

- **C1 (Tasks 2, 3):** `Kev::decide` and `Kev::prepare` discard an explicitly supplied key for a loopback endpoint. Correct the new direct-loopback request test to expect no Authorization header for either input key. Verify optional Bearer serialization separately at the shared transport boundary; remote Kev still uses its key.
- **C2 (Task 2):** Managed endpoints require exactly `/v1/systemone`, without a prefix, query, fragment or credentials. Test prefixed managed paths and fragments. Handle explicit default port 80 consistently with URL normalization, or reject it clearly; never advertise it as supported while runtime cannot derive its port.
- **C3 (Task 3):** Remove inherited `KEV_API_KEY` from every managed Kev child, including the first start. Test inherited authentication without printing the sentinel. Normal managed starts must also enforce uv offline operation, not only `HF_HUB_OFFLINE=1`; setup may fetch dependencies and weights.
- **C4 (Tasks 3, 4):** The first-download supervisor must stay alive for the entire setup download deadline and probe, even beyond ordinary idle time. Keep normal runtime idle at 900 seconds. Pin this with shortened test timings, and retain bounded failure cleanup.
- **C5 (Tasks 3, 4):** Propagate `setsid` errors; pass the stop grace flag explicitly. `runtime::stop` must distinguish an already stopped server, confirmed shutdown and timeout/error, and setup must not write success after unconfirmed shutdown. Stop the whole Kev process group, including a TERM-resistant descendant after its leader exits; test this. Log the final exit status on controlled shutdown. The reviewed implementation uses a lock-holder stop request and `server-ownership.json` termination proof; persisted PIDs are diagnostic only. Abandoned ownership blocks readiness and startup until cleanup has been explicitly verified. An independent session is the tested macOS behavior; do not claim it escapes systemd cgroups.
- **C6 (Tasks 4, 5):** Before changing the shared checkout or checkpoint, deliberately stop and confirm shutdown of the owned supervisor. Scope cleanup to the install server setup owns or deliberately adopts, and reject unrelated listeners without stopping them. Exclude competing installs and normal managed starts while the shared checkout is being changed, using a shared installation guard respected by startup. A bounded internal mutation supervisor retains exclusion and durable owned-group proof across installer disappearance, including relevant writer descendants that close inherited descriptors; abandoned mutation ownership blocks replacement. Ordinary supervisors revalidate receipt tag/checkpoint/executable under their reacquired startup guard, rejecting delayed stale launches after replacement; explicit standalone downloads remain supported. Doctor uses a read-only installation-busy inspection to distinguish mutation ownership from a serving Kev runtime. Test active-checkpoint handover, concurrent startup during handover and interrupted-start resume. A matching `installed.json` alone is insufficient for the current-install shortcut: check the source directory and recorded executable, and reject unconfirmed ownership. Doctor reports missing artifacts or ownership problems without starting Kev. Missing/corrupt weights must have an actionable explicit repair route through setup; do not guess Hugging Face cache structure.
- **C7 (Task 4):** Model-only or key-only updates retain the current Kev mode and endpoint unless provider-selection flags explicitly replace it. Preserve existing OpenRouter and fake behavior. Test managed model-only and remote key-only updates. Enforce Apple Silicon support on every managed install route, including kept/copied configs.
- **C8 (Task 3):** Add a controlled managed failure after successful prepare. Verify that the job is queued again, only the current lease increment is undone, earlier successes stay counted, later jobs are untouched, and classify/sync/reclassify report skipped/partial correctly. Ordinary POST/HTTP/contract failures continue to count attempts.
- **C9 (Task 4):** The unresolved-key 401/403 exception applies only to a remote server with a configured key source. Local authentication failures are actionable errors. Kept own-server configs are checked and include `setup.kev` consistently. Unreachable remote fixtures prove key-source selection and non-disclosure; shared transport tests prove header transmission.
- **C10 (Task 6):** The new Kev fixtures require no Python or real Kev; existing setup fixtures still use Python. Keep offline and process-lifecycle claims aligned with C3/C5, and measured timing/accuracy claims pending the live check.
- **C11 (Task 7):** Force fresh classification for the offline and restart checks, using fresh inputs/state or explicit reclassification with filing off. Repeating a cached example does not establish inference or restart. This task remains human-run.

For commits in this Codex execution, replace the Claude trailer below with `Co-Authored-By: Codex <noreply@openai.com>`. Follow the specified local `main` workflow and never push. Task 1 also commits these review corrections so they remain part of the durable record.

1. **The background service starts Kev.** `watch` under launchd or systemd starts the supervisor; it must outlive the pass and `service stop` (its own session via `setsid`), and `watch` must not collect zombies. Pinned in Task 3 by `the_supervisor_runs_in_its_own_session`.
2. **Something else on port 8009**, including a Kev you run yourself that serves another checkpoint under the same name `kev-latest`: mailtriage must refuse it with the port reason, never classify with it. Pinned in Task 3 by `another_program_on_the_port_is_named` (a stub that serves `kev-latest` with another `run`).
3. **`doctor` and setup's check step start nothing.** A user runs `doctor` to see why classification stopped; it must not start a 12 GB process or download anything. Pinned in Task 5 by `doctor_reports_local_kev_without_starting_it`.
4. **A hand-edited checkpoint** (`jaredpalmer/kev-9b@v1.0`): classification skips with the setup fix, and `setup --update` then installs that checkpoint, never silently the default one. Pinned in Task 4 by `a_kept_local_kev_is_installed_with_its_own_checkpoint`.
5. **The remote key** is never printed, never written to the config, and never sent to a server on this machine. Pinned in Task 2 by `a_kev_server_on_this_machine_gets_no_key` and in Task 4 by `a_remote_kev_server_uses_a_key_store_under_account_kev`.

## File map

| File | Task | Responsibility |
| --- | --- | --- |
| `src/domain.rs` | 1 | `ProviderConfig.managed`, `ManagedKev` |
| `src/provider/mod.rs` | 1, 2, 3 | trait (`key_account(config)`, `prepare`), `KINDS`, `provider_for`, `key_owner`, `ProviderUnavailable`, `prepare` |
| `src/provider/systemone.rs` | 2 | `request_body`, `send`, `parse`, moved from `openrouter.rs` |
| `src/provider/kev.rs` | 2, 3 | `Kev` adapter, URL rule, `list_models`, `models_url`, `local_endpoint` |
| `src/kev/mod.rs` | 3 | `Layout`, `Installed`, constants, `MAILTRIAGE_TEST_KEV` |
| `src/kev/runtime.rs` | 3, 5 | `Runtime`, `ensure_running`, `probe`, `spawn_supervisor`, `stop`, `status` |
| `src/kev/supervisor.rs` | 3 | `mailtriage kev run` |
| `src/kev/install.rs` | 4 | setup's managed install |
| `src/service.rs` | 1, 3, 5 | skip on `ProviderUnavailable`; `doctor`'s `provider.kev` |
| `src/store.rs` | 3 | `Store::unlease` |
| `src/setup.rs` | 1, 4, 5 | menu, flags, Kev paths, doctor item |
| `src/cli.rs` | 3, 4 | hidden `kev run`; `--kev-url`, `yes` |
| `src/secrets.rs` | 2 | per-kind key texts |
| `src/process.rs` | 3 | `signal`, `SIGTERM`, `SIGKILL` |
| `src/distribution/self_uninstall.rs` | 5 | keep and name `kev/` |
| `tests/kev_support/mod.rs` | 3, 4 | stand-in Kev server, `Stub`, stand-in `uv`/`git`/`brew` scripts |
| `tests/kev_runtime.rs` | 3, 5 | runtime, skips, doctor |
| `tests/provider_contract.rs` | 1, 2 | contract suite with `kev`, Kev texts and URL rule |
| `tests/setup.rs` | 4, 5 | setup paths for Kev |
| `docs/...` | 6 | user, reference and development docs |

---

### Task 1: `key_account` sees the config; `ProviderConfig.managed`

**Files:**
- Modify: `src/domain.rs` (`ProviderConfig`, new `ManagedKev`)
- Modify: `src/config.rs:86-93` (`default_config`'s provider literal)
- Modify: `src/provider/mod.rs` (trait, `key_account`, `validate_configuration`, `classify_with_key`)
- Modify: `src/provider/openrouter.rs` (`key_account`, `new_config`)
- Modify: `src/provider/fake.rs` (`key_account`)
- Modify: `src/setup.rs:978` and `src/setup.rs:1017` (the two direct trait calls)
- Modify: `src/service.rs` (new unit test in `mod golden`, next to `generation_hash_ignores_the_key_command`)
- Test: `tests/provider_contract.rs`
- Unchanged and must stay green: every other caller of `provider::key_account(&ProviderConfig)` (`src/service.rs`, `src/system_service.rs`, `src/setup.rs` lines 1364, 1524, 1705, 1856) keeps its signature; the pinned generation hashes.

**Interfaces:**
- Consumes: nothing new.
- Produces (later tasks rely on these):
  - `domain::ManagedKev { pub checkpoint: String }` deriving `Debug, Clone, PartialEq, Eq, Serialize, Deserialize`
  - `domain::ProviderConfig { kind, model, endpoint, api_key_command: Option<Vec<String>>, api_key_env: String, timeout_seconds: u64, managed: Option<ManagedKev> }`; `managed` has `#[serde(default, skip_serializing_if = "Option::is_none")]`
  - `trait DecisionProvider { fn key_account(&self, config: &ProviderConfig) -> Option<&'static str>; ... }` (the rest unchanged)
  - `provider::key_account(config: &ProviderConfig) -> Option<&'static str>` (unchanged signature)
  - `provider::validate_configuration` refuses `managed` on any kind but `kev` with `provider.managed is only for kind kev`

- [ ] **Step 1: Write the failing tests**

In `tests/provider_contract.rs`, change the `use` block to import `ManagedKev`:

```rust
use mailtriage::{
    config,
    domain::{AccountConfig, Address, Category, ManagedKev, NormalizedMessage, ProviderConfig},
    provider::{self, Decision, DecisionRequest},
    service::Service,
};
```

In `the_registry_knows_exactly_its_kinds`, replace the last two assertions

```rust
    assert_eq!(
        provider::provider_for("openrouter").unwrap().key_account(),
        Some("openrouter")
    );
    assert_eq!(provider::provider_for("fake").unwrap().key_account(), None);
```

with

```rust
    let openrouter = provider::provider_for("openrouter").unwrap();
    assert_eq!(
        openrouter.key_account(&openrouter.new_config()),
        Some("openrouter")
    );
    let fake = provider::provider_for("fake").unwrap();
    assert_eq!(fake.key_account(&fake.new_config()), None);
```

Append this test to `tests/provider_contract.rs`:

```rust
/// Kev spec, config: `managed` is written only when set, so every existing
/// config and its generation hash stay the same, and only `kev` may have it.
#[test]
fn managed_is_only_for_kev_and_serialized_only_when_set() {
    let mut config = openrouter(provider::DECISIONS_ENDPOINT);
    let written = serde_json::to_value(&config).unwrap();
    assert!(written.get("managed").is_none(), "{written}");
    let managed = ManagedKev {
        checkpoint: "jaredpalmer/kev-4b@v1.0".into(),
    };
    config.managed = Some(managed.clone());
    assert_eq!(
        serde_json::to_value(&config).unwrap()["managed"],
        json!({"checkpoint": "jaredpalmer/kev-4b@v1.0"})
    );
    assert_eq!(
        provider::validate_configuration(&config)
            .unwrap_err()
            .to_string(),
        "provider.managed is only for kind kev"
    );
    let mut fake = provider::provider_for("fake").unwrap().new_config();
    fake.managed = Some(managed);
    assert_eq!(
        provider::validate_configuration(&fake)
            .unwrap_err()
            .to_string(),
        "provider.managed is only for kind kev"
    );
    let read: ProviderConfig = serde_json::from_value(json!({
        "kind": "fake", "model": "fake/offline", "endpoint": "", "timeout_seconds": 30
    }))
    .unwrap();
    assert_eq!(read.managed, None);
}
```

In `src/service.rs`, in `mod golden` (it holds `generation_hash_ignores_the_key_command`), add:

```rust
    /// Kev spec: `managed.checkpoint` is part of the generation hash, as
    /// `model` is; without `managed` the hash is today's.
    #[test]
    fn generation_hash_includes_the_managed_checkpoint() {
        let mut cfg = crate::config::default_config();
        let plain = super::generation(&cfg, &cfg.accounts["work"]).unwrap();
        assert_eq!(
            plain,
            "6dfd7bf30e6bf1c0b27ee97af991ecf21dc5ac0400044c2f3adbda7f79d37514"
        );
        let with = |checkpoint: &str| crate::domain::ManagedKev {
            checkpoint: checkpoint.into(),
        };
        cfg.provider.managed = Some(with("jaredpalmer/kev-4b@v1.0"));
        let four = super::generation(&cfg, &cfg.accounts["work"]).unwrap();
        cfg.provider.managed = Some(with("jaredpalmer/kev-9b@v1.0"));
        let nine = super::generation(&cfg, &cfg.accounts["work"]).unwrap();
        assert_ne!(plain, four);
        assert_ne!(four, nine);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --locked --test provider_contract 2>&1 | tail -20`
Expected: compile error, `cannot find type ManagedKev in module domain` (and `this method takes 0 arguments but 1 argument was supplied` for `key_account`).

- [ ] **Step 3: Add `managed` to the config**

In `src/domain.rs`, replace the `ProviderConfig` struct with:

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
    /// `kev` only: setup installed this Kev server and mailtriage runs it.
    /// Serialized only when set, so every other config, and its generation
    /// hash, is unchanged; the checkpoint is part of the hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed: Option<ManagedKev>,
}

/// A Kev server mailtriage installed and starts on demand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedKev {
    /// The checkpoint Kev serves, as `--run` takes it:
    /// `jaredpalmer/kev-4b@v1.0`.
    pub checkpoint: String,
}
```

In `src/config.rs`, add `managed: None,` after `timeout_seconds: 30,` in `default_config`'s `ProviderConfig` literal. In `src/provider/openrouter.rs`, add `managed: None,` after `timeout_seconds: 30,` in `new_config`.

- [ ] **Step 4: Pass the config to `key_account`**

In `src/provider/mod.rs`, replace the trait's `key_account` declaration and its doc comment with:

```rust
    /// The key store's account name (`openrouter`) when `config` needs a
    /// key; `None` when it needs none (the offline demo, a Kev server on
    /// this machine).
    fn key_account(&self, config: &ProviderConfig) -> Option<&'static str>;
```

Replace `key_account`:

```rust
/// The key store account of `config`'s provider: `None` when it needs no
/// key, and for an unknown kind, which validation refuses.
pub fn key_account(config: &ProviderConfig) -> Option<&'static str> {
    provider_for(&config.kind)
        .ok()
        .and_then(|provider| provider.key_account(config))
}
```

In `validate_configuration`, replace

```rust
    let provider = provider_for(&config.kind)?;
    if provider.key_account().is_some() {
```

with

```rust
    let provider = provider_for(&config.kind)?;
    if config.managed.is_some() && config.kind != "kev" {
        bail!("provider.managed is only for kind kev");
    }
    if provider.key_account(config).is_some() {
```

In `classify_with_key`, replace `let key = match provider.key_account() {` with `let key = match provider.key_account(config) {`.

In `src/provider/openrouter.rs`:

```rust
    fn key_account(&self, _config: &ProviderConfig) -> Option<&'static str> {
        Some("openrouter")
    }
```

In `src/provider/fake.rs`:

```rust
    fn key_account(&self, _config: &ProviderConfig) -> Option<&'static str> {
        None
    }
```

In `src/setup.rs`, `classifier_step`: replace

```rust
        if provider::provider_for(kind).is_ok_and(|p| p.key_account().is_none()) {
```

with

```rust
        if provider::provider_for(kind).is_ok_and(|p| p.key_account(&p.new_config()).is_none()) {
```

and replace `let Some(account) = adapter.key_account() else {` with `let Some(account) = adapter.key_account(&adapter.new_config()) else {`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --locked --test provider_contract && cargo test --locked --lib generation_hash`
Expected: PASS, including `managed_is_only_for_kev_and_serialized_only_when_set`, `generation_hash_includes_the_managed_checkpoint` and the two unchanged `generation_hash_*` tests.

- [ ] **Step 6: Run the full verification**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`
Expected: no formatting diff, no warnings, all tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/domain.rs src/config.rs src/provider/mod.rs src/provider/openrouter.rs src/provider/fake.rs src/setup.rs src/service.rs tests/provider_contract.rs
git commit -m "Let key_account see the provider config" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Shared System One module and the `kev` provider for your own server

**Files:**
- Create: `src/provider/systemone.rs`
- Create: `src/provider/kev.rs`
- Modify: `src/provider/openrouter.rs` (use `systemone`; keep `validate`, `label`, `new_config`, constants)
- Modify: `src/provider/mod.rs` (`pub mod kev; pub mod systemone;`, `KINDS`, `provider_for`, `key_owner`, key-source text per kind, module doc list)
- Modify: `src/secrets.rs` (`resolve_key`'s environment texts per kind)
- Test: `tests/provider_contract.rs`, `tests/key_command.rs`

**Interfaces:**
- Consumes (Task 1): `ProviderConfig.managed`, `ManagedKev`, `DecisionProvider::key_account(&self, &ProviderConfig)`.
- Produces:
  - `provider::systemone::request_body(config: &ProviderConfig, request: &DecisionRequest) -> serde_json::Value`
  - `provider::systemone::send(config: &ProviderConfig, url: &str, body: &Value, key: Option<&str>, name: &str) -> anyhow::Result<Value>` (Bearer only when `key` is `Some`)
  - `provider::systemone::parse(response: Value, name: &str) -> anyhow::Result<Decision>`
  - `provider::kev::Kev` (registered as `"kev"`), with `label()` = `"Kev server you run (enter its URL)"`
  - `provider::kev::{NAME = "Kev server", DEFAULT_MODEL = "kev-latest", DEFAULT_CHECKPOINT = "jaredpalmer/kev-4b@v1.0", DEFAULT_PORT: u16 = 8009, DEFAULT_KEY_ENV = "KEV_API_KEY"}`
  - `provider::kev::local_endpoint(port: u16) -> String` (`http://127.0.0.1:{port}/v1/systemone`)
  - `provider::kev::is_local(endpoint: &str) -> bool`
  - `provider::kev::models_url(endpoint: &str) -> Option<String>`
  - `provider::kev::ModelCard { pub name: String, pub run: Option<String> }`
  - `provider::kev::ModelsError { Unreachable, Http(u16), Invalid }` with `Display`: `the Kev server did not answer`, `the Kev server returned HTTP {code}`, `the Kev server sent no model list`
  - `provider::kev::list_models(url: &str, key: Option<&str>, timeout: Duration) -> Result<Vec<ModelCard>, ModelsError>`
  - `provider::key_owner(config: &ProviderConfig) -> &'static str` (`"Kev server"` for `kev`, else `"OpenRouter"`)
  - `provider::KINDS == ["openrouter", "kev", "fake"]`

- [ ] **Step 1: Write the failing tests**

In `tests/provider_contract.rs`, add after `RESPONSE`:

```rust
/// A Kev server's answer for the six default categories: `model` is the
/// requested name, probabilities have four decimals, and there is no
/// `provider` field (kev/serve.py `_body`).
const KEV_RESPONSE: &str = r#"{"model":"kev-latest","answers":{
"category":{"type":"choice","choice":"transactions","confidence":0.9612,"probabilities":{"correspondence":0.0081,"transactions":0.9677,"updates":0.0102,"newsletters":0.0043,"promotions":0.0035,"other":0.0062}},
"urgency":{"type":"choice","choice":"high","confidence":0.8571,"probabilities":{"low":0.0204,"medium":0.0844,"high":0.8952}},
"action_required":{"type":"noul","noul":0.9431}},"usage":{"input_tokens":412,"output_tokens":31},"latency_ms":721.4}"#;
```

Add after `fn openrouter(...)`:

```rust
/// A Kev server you run at the loopback endpoint `endpoint` made for
/// OpenRouter: the same listener, the Kev path.
fn kev(decisions_url: &str) -> ProviderConfig {
    let mut config = provider::provider_for("kev").unwrap().new_config();
    config.managed = None;
    config.endpoint = decisions_url.replace("/api/alpha/decisions", "/v1/systemone");
    config
}

fn kev_error(response: Vec<u8>) -> String {
    let (url, server) = endpoint(response);
    let error = provider::provider_for("kev")
        .unwrap()
        .decide(&kev(&url), &request(), None)
        .unwrap_err()
        .to_string();
    server.join().unwrap();
    error
}
```

In `fixture`, add an arm before `other =>`:

```rust
        "kev" => {
            let (url, server) = endpoint(http("200 OK", "", KEV_RESPONSE.as_bytes()));
            (kev(&url), None, "kev-latest", Some(server))
        }
```

In `the_registry_knows_exactly_its_kinds`, change the first line to `assert_eq!(provider::KINDS, ["openrouter", "kev", "fake"]);` and append before the closing brace:

```rust
    let kev = provider::provider_for("kev").unwrap();
    let managed = kev.new_config();
    assert_eq!(managed.model, "kev-latest");
    assert_eq!(managed.endpoint, "http://127.0.0.1:8009/v1/systemone");
    assert_eq!(managed.timeout_seconds, 60);
    assert_eq!(
        managed.managed.as_ref().map(|m| m.checkpoint.as_str()),
        Some("jaredpalmer/kev-4b@v1.0")
    );
    assert_eq!(kev.key_account(&managed), None);
    assert_eq!(kev.label(), "Kev server you run (enter its URL)");
```

Append these tests:

```rust
/// Kev spec, adapter: Kev gets OpenRouter's body with its own model, at
/// its own path, with a Bearer key only when one is given.
#[test]
fn the_kev_request_is_the_openrouter_body_with_its_model() {
    let expected = PINNED_BODY.replacen(
        r#""model":"typesafe/jev-latest""#,
        r#""model":"kev-latest""#,
        1,
    );
    for key in [None, Some("fixture-key")] {
        let (url, server) = endpoint(http("200 OK", "", KEV_RESPONSE.as_bytes()));
        let decision = provider::provider_for("kev")
            .unwrap()
            .decide(&kev(&url), &request(), key)
            .unwrap();
        let received = server.join().unwrap();
        assert!(received.head.starts_with("POST /v1/systemone HTTP/1.1\r\n"));
        let head = received.head.to_ascii_lowercase();
        assert_eq!(
            head.contains("\r\nauthorization: bearer fixture-key\r\n"),
            key.is_some(),
            "{head}"
        );
        assert_eq!(key.is_none(), !head.contains("\r\nauthorization:"), "{head}");
        assert_eq!(String::from_utf8(received.body).unwrap(), expected);
        assert_eq!(decision.model, "kev-latest");
    }
}

/// Review focus 5: a Kev server on this machine gets no key, even when the
/// config names a key source.
#[test]
fn a_kev_server_on_this_machine_gets_no_key() {
    let (url, server) = endpoint(http("200 OK", "", KEV_RESPONSE.as_bytes()));
    let mut config = kev(&url);
    config.api_key_command = Some(vec!["/bin/echo".into(), "sk-kev-secret".into()]);
    assert_eq!(provider::key_account(&config), None);
    provider::classify(
        &config,
        &pinned_account(),
        &pinned_message(),
        &config::default_config().policy,
    )
    .unwrap();
    let received = server.join().unwrap();
    assert!(
        !received.head.to_ascii_lowercase().contains("authorization:"),
        "{}",
        received.head
    );
}

#[test]
fn kev_failures_name_the_kev_server() {
    let oversized = vec![b' '; 1024 * 1024 + 1];
    let cases = [
        (
            http("429 Too Many Requests", "", b"provider secret diagnostic"),
            "Kev server returned HTTP 429",
        ),
        (
            http("401 Unauthorized", "", br#"{"detail":"provider secret diagnostic"}"#),
            "Kev server returned HTTP 401",
        ),
        (
            http("422 Unprocessable Entity", "", b"provider secret diagnostic"),
            "Kev server returned HTTP 422",
        ),
        (http("200 OK", "", &oversized), "Kev server response is too large"),
        (http("200 OK", "", b"not-json"), "invalid Kev server JSON"),
        (
            http("200 OK", "", br#"{"model":"kev-latest"}"#),
            "Kev server response lacks typed answers",
        ),
    ];
    for (response, expected) in cases {
        let error = kev_error(response);
        assert_eq!(error, expected);
        assert!(!error.contains("provider secret diagnostic"));
    }
    // Nothing listens on a port just freed.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let config = kev(&format!("http://127.0.0.1:{port}/api/alpha/decisions"));
    assert_eq!(
        provider::provider_for("kev")
            .unwrap()
            .decide(&config, &request(), None)
            .unwrap_err()
            .to_string(),
        "Kev server request failed"
    );
}

/// Kev spec, URL rule: `http` only on this machine; `https` and a key
/// elsewhere; a managed endpoint is `http://127.0.0.1:PORT/v1/systemone`.
#[test]
fn kev_endpoints_follow_the_url_rule() {
    let adapter = provider::provider_for("kev").unwrap();
    let server = |endpoint: &str, key_env: &str| {
        let mut config = adapter.new_config();
        config.managed = None;
        config.endpoint = endpoint.into();
        config.api_key_env = key_env.into();
        config
    };
    let check = |config: &ProviderConfig| {
        provider::validate_configuration(config)
            .err()
            .map(|e| e.to_string())
    };
    for local in [
        "http://127.0.0.1:8009/v1/systemone",
        "http://localhost:8009/v1/systemone",
        "http://[::1]:8009/v1/systemone",
    ] {
        let config = server(local, "");
        assert_eq!(check(&config), None, "{local}");
        assert_eq!(adapter.key_account(&config), None, "{local}");
    }
    for remote in [
        "https://kev.example.test/v1/systemone",
        "https://example.test/kev/v1/systemone",
        "https://127.0.0.1:8443/v1/systemone",
    ] {
        let config = server(remote, "KEV_API_KEY");
        assert_eq!(check(&config), None, "{remote}");
    }
    assert_eq!(
        adapter.key_account(&server("https://kev.example.test/v1/systemone", "")),
        Some("kev")
    );
    let https = "provider.endpoint must use https for a server off this machine";
    let rule = "provider.endpoint must be a Kev server URL ending in /v1/systemone";
    let cases = [
        ("http://kev.example.test/v1/systemone", "KEV_API_KEY", https),
        ("http://192.168.1.5:8009/v1/systemone", "KEV_API_KEY", https),
        (
            "https://kev.example.test/v1/systemone",
            "",
            "Kev server needs api_key_command or api_key_env",
        ),
        ("https://kev.example.test/v1/chat", "KEV_API_KEY", rule),
        ("http://127.0.0.1:8009/v1/systemone/", "", rule),
        ("http://127.0.0.1:8009/v1/systemone?x=1", "", rule),
        ("ftp://127.0.0.1/v1/systemone", "", rule),
        ("not a url", "", rule),
        (
            "https://user:pass@kev.example.test/v1/systemone",
            "KEV_API_KEY",
            "provider.endpoint must not contain a user name or password; use api_key_command for the key",
        ),
    ];
    for (endpoint, key_env, expected) in cases {
        assert_eq!(
            check(&server(endpoint, key_env)).as_deref(),
            Some(expected),
            "{endpoint}"
        );
    }
    let managed_rule = "a managed Kev endpoint must be http://127.0.0.1:PORT/v1/systemone";
    let mut managed = adapter.new_config();
    assert_eq!(check(&managed), None);
    for endpoint in [
        "http://localhost:8009/v1/systemone",
        "http://[::1]:8009/v1/systemone",
        "http://127.0.0.1/v1/systemone",
    ] {
        managed.endpoint = endpoint.into();
        assert_eq!(check(&managed).as_deref(), Some(managed_rule), "{endpoint}");
    }
    managed.endpoint = "https://kev.example.test/v1/systemone".into();
    managed.api_key_env = "KEV_API_KEY".into();
    assert_eq!(check(&managed).as_deref(), Some(managed_rule));
    let mut blank = adapter.new_config();
    blank.managed = Some(ManagedKev {
        checkpoint: " ".into(),
    });
    assert_eq!(
        check(&blank).as_deref(),
        Some("provider.managed.checkpoint cannot be empty")
    );
    assert_eq!(
        provider::kev::models_url("https://example.test/kev/v1/systemone").as_deref(),
        Some("https://example.test/kev/v1/models")
    );
    assert_eq!(provider::kev::models_url("https://example.test/v1/chat"), None);
}

#[test]
fn kev_models_are_listed_with_the_key_when_given() {
    let models = br#"{"models":[{"name":"kev-latest","run":"jaredpalmer/kev-4b@v1.0","device":"mps"},{"name":"jev-latest","run":"jaredpalmer/kev-4b@v1.0"}]}"#;
    let (url, server) = endpoint(http("200 OK", "", models));
    let url = url.replace("/api/alpha/decisions", "/v1/models");
    let list =
        provider::kev::list_models(&url, Some("fixture-key"), Duration::from_secs(5)).unwrap();
    let received = server.join().unwrap();
    assert!(received.head.starts_with("GET /v1/models HTTP/1.1\r\n"));
    assert!(received
        .head
        .to_ascii_lowercase()
        .contains("\r\nauthorization: bearer fixture-key\r\n"));
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].name, "kev-latest");
    assert_eq!(list[0].run.as_deref(), Some("jaredpalmer/kev-4b@v1.0"));
    let fails = |response: Vec<u8>| {
        let (url, server) = endpoint(response);
        let error = provider::kev::list_models(&url, None, Duration::from_secs(5)).unwrap_err();
        server.join().unwrap();
        error
    };
    assert_eq!(
        fails(http("401 Unauthorized", "", b"{}")),
        provider::kev::ModelsError::Http(401)
    );
    assert_eq!(
        fails(http("200 OK", "", b"not-json")),
        provider::kev::ModelsError::Invalid
    );
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let closed = format!("http://127.0.0.1:{port}/v1/models");
    let error = provider::kev::list_models(&closed, None, Duration::from_secs(5)).unwrap_err();
    assert_eq!(error, provider::kev::ModelsError::Unreachable);
    assert_eq!(error.to_string(), "the Kev server did not answer");
}
```

In `tests/key_command.rs`, append:

```rust
/// Kev spec: a remote Kev server's key follows the shared rules, and its
/// errors name the Kev server; OpenRouter's texts are unchanged.
#[test]
fn kev_key_errors_name_the_kev_server() {
    let mut provider = config::default_config().provider;
    provider.kind = "kev".into();
    provider.model = "kev-latest".into();
    provider.endpoint = "https://kev.example.test/v1/systemone".into();
    provider.api_key_env = "MT_KEV_KEY_MISSING_TEST".into();
    assert_eq!(
        secrets::resolve_key(&provider).unwrap_err().to_string(),
        "Kev server API key environment variable is missing"
    );
    std::env::set_var("MT_KEV_KEY_EMPTY_TEST", " ");
    provider.api_key_env = "MT_KEV_KEY_EMPTY_TEST".into();
    assert_eq!(
        secrets::resolve_key(&provider).unwrap_err().to_string(),
        "Kev server API key environment variable is empty"
    );
    let mut openrouter = openrouter();
    openrouter.api_key_env = "MT_OPENROUTER_KEY_MISSING_TEST".into();
    assert_eq!(
        secrets::resolve_key(&openrouter).unwrap_err().to_string(),
        "OpenRouter API key environment variable is missing"
    );
    assert_eq!(provider::key_owner(&provider), "Kev server");
    assert_eq!(provider::key_owner(&openrouter), "OpenRouter");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --locked --test provider_contract 2>&1 | tail -20`
Expected: compile errors: `could not find kev in provider`, `cannot find function key_owner`.

- [ ] **Step 3: Move the wire code into `systemone.rs`**

Create `src/provider/systemone.rs`. The bodies of `request_body`, `question`, `parse`, `choice` and `number` are moved unchanged from `openrouter.rs`; only `send` and `parse` take the provider's name for their texts:

```rust
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
```

Replace the rest of `src/provider/openrouter.rs` below `new_config` so the file keeps `DECISIONS_ENDPOINT`, `DEFAULT_MODEL`, `DEFAULT_KEY_ENV`, `validate`, `key_account`, `label`, `new_config`, and `decide` becomes:

```rust
    fn decide(
        &self,
        config: &ProviderConfig,
        request: &DecisionRequest,
        key: Option<&str>,
    ) -> Result<Decision> {
        self.validate(config)?;
        let key = key.ok_or_else(|| anyhow!("OpenRouter API key is missing"))?;
        let body = systemone::request_body(config, request);
        systemone::parse(
            systemone::send(config, &config.endpoint, &body, Some(key), NAME)?,
            NAME,
        )
    }
}

/// The provider's name in every error text, as before the split.
const NAME: &str = "OpenRouter Decisions";
```

Delete `MAX_RESPONSE_BYTES`, `request_body`, `question`, `send`, `parse`, `choice` and `number` from `openrouter.rs`, and change its imports to:

```rust
use super::{systemone, Decision, DecisionProvider, DecisionRequest};
use crate::domain::ProviderConfig;
use anyhow::{anyhow, bail, Context, Result};
```

Add `pub mod systemone;` next to `pub mod fake;` in `src/provider/mod.rs`.

- [ ] **Step 4: Check that the split builds**

Run: `cargo build --locked`
Expected: builds. (`tests/provider_contract.rs` does not compile until the Kev adapter exists; the pinned OpenRouter tests run in Step 7 and must pass without any edit to their expectations.)

- [ ] **Step 5: Add the Kev adapter**

Create `src/provider/kev.rs`:

```rust
//! A Kev server (<https://github.com/jaredpalmer/kev>): open-weight decision
//! models whose API is TypeSafe's System One, so the request and the
//! answers are OpenRouter's (`systemone`). It runs in one of two ways:
//! managed (`managed` is set; setup installs it and `crate::kev` starts it
//! on demand) or your own server at any URL. A key is needed, and sent,
//! only for a server off this machine.
use super::{systemone, Decision, DecisionProvider, DecisionRequest};
use crate::domain::{ManagedKev, ProviderConfig};
use anyhow::{anyhow, bail, Result};
use serde::Deserialize;
use std::{fmt, io::Read, time::Duration};

/// The provider's name in every error text.
pub const NAME: &str = "Kev server";
/// The name Kev serves its checkpoint under.
pub const DEFAULT_MODEL: &str = "kev-latest";
/// What a managed install serves (Kev 1.0's Kev-4B).
pub const DEFAULT_CHECKPOINT: &str = "jaredpalmer/kev-4b@v1.0";
pub const DEFAULT_PORT: u16 = 8009;
/// The variable setup offers for a remote server's key.
pub const DEFAULT_KEY_ENV: &str = "KEV_API_KEY";
const URL_RULE: &str = "provider.endpoint must be a Kev server URL ending in /v1/systemone";
const MAX_MODELS_BYTES: u64 = 1024 * 1024;

pub struct Kev;

/// A managed server's endpoint: `http://127.0.0.1:PORT/v1/systemone`.
pub fn local_endpoint(port: u16) -> String {
    format!("http://127.0.0.1:{port}/v1/systemone")
}

/// Whether `endpoint` is a URL on this machine: `127.0.0.1`, `localhost`
/// or `[::1]`.
pub fn is_local(endpoint: &str) -> bool {
    reqwest::Url::parse(endpoint)
        .is_ok_and(|url| matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")))
}

/// The model list next to `endpoint`: `.../v1/systemone` becomes
/// `.../v1/models`.
pub fn models_url(endpoint: &str) -> Option<String> {
    endpoint
        .strip_suffix("/v1/systemone")
        .map(|base| format!("{base}/v1/models"))
}

impl DecisionProvider for Kev {
    fn validate(&self, config: &ProviderConfig) -> Result<()> {
        let url = reqwest::Url::parse(&config.endpoint).map_err(|_| anyhow!(URL_RULE))?;
        if !config.endpoint.ends_with("/v1/systemone")
            || !url.path().ends_with("/v1/systemone")
            || url.query().is_some()
            || url.fragment().is_some()
        {
            bail!(URL_RULE);
        }
        if !url.username().is_empty() || url.password().is_some() {
            bail!("provider.endpoint must not contain a user name or password; use api_key_command for the key");
        }
        match url.scheme() {
            "https" => {}
            "http" if is_local(&config.endpoint) => {}
            "http" => bail!("provider.endpoint must use https for a server off this machine"),
            _ => bail!(URL_RULE),
        }
        if let Some(managed) = &config.managed {
            // The supervisor binds Kev to 127.0.0.1 on this port.
            if url.scheme() != "http" || url.host_str() != Some("127.0.0.1") || url.port().is_none()
            {
                bail!("a managed Kev endpoint must be http://127.0.0.1:PORT/v1/systemone");
            }
            if managed.checkpoint.trim().is_empty() {
                bail!("provider.managed.checkpoint cannot be empty");
            }
        }
        Ok(())
    }

    /// `kev` for a server off this machine; `None` on this machine, and for
    /// an endpoint that is no URL, which `validate` refuses.
    fn key_account(&self, config: &ProviderConfig) -> Option<&'static str> {
        reqwest::Url::parse(&config.endpoint)
            .ok()
            .filter(|_| !is_local(&config.endpoint))
            .map(|_| "kev")
    }

    fn label(&self) -> &'static str {
        "Kev server you run (enter its URL)"
    }

    /// A managed Kev-4B on port 8009; setup clears `managed` and sets the
    /// URL for a server you run.
    fn new_config(&self) -> ProviderConfig {
        ProviderConfig {
            kind: "kev".to_owned(),
            model: DEFAULT_MODEL.to_owned(),
            endpoint: local_endpoint(DEFAULT_PORT),
            api_key_command: None,
            api_key_env: String::new(),
            timeout_seconds: 60,
            managed: Some(ManagedKev {
                checkpoint: DEFAULT_CHECKPOINT.to_owned(),
            }),
        }
    }

    fn decide(
        &self,
        config: &ProviderConfig,
        request: &DecisionRequest,
        key: Option<&str>,
    ) -> Result<Decision> {
        self.validate(config)?;
        if config.managed.is_some() {
            // Replaced in the managed runtime's task by `ensure_running`.
            bail!("local Kev is not installed; run mailtriage setup --update");
        }
        let body = systemone::request_body(config, request);
        systemone::parse(
            systemone::send(config, &config.endpoint, &body, key, NAME)?,
            NAME,
        )
    }
}

/// One entry of `GET /v1/models`; Kev adds fields mailtriage ignores.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ModelCard {
    pub name: String,
    /// The checkpoint as given to `--run`.
    #[serde(default)]
    pub run: Option<String>,
}

/// Why `list_models` has no list. Fixed texts: no response body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelsError {
    /// No connection, or no answer within the timeout.
    Unreachable,
    Http(u16),
    /// An answer that is not a model list.
    Invalid,
}

impl fmt::Display for ModelsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelsError::Unreachable => f.write_str("the Kev server did not answer"),
            ModelsError::Http(code) => write!(f, "the Kev server returned HTTP {code}"),
            ModelsError::Invalid => f.write_str("the Kev server sent no model list"),
        }
    }
}

impl std::error::Error for ModelsError {}

/// `GET url` (a Kev server's `/v1/models`): the models it serves. Bearer
/// `key` when given; no redirects; at most 1 MiB.
pub fn list_models(
    url: &str,
    key: Option<&str>,
    timeout: Duration,
) -> Result<Vec<ModelCard>, ModelsError> {
    #[derive(Deserialize)]
    struct List {
        models: Vec<ModelCard>,
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| ModelsError::Unreachable)?;
    let mut get = client.get(url);
    if let Some(key) = key {
        get = get.bearer_auth(key);
    }
    let response = get.send().map_err(|_| ModelsError::Unreachable)?;
    if !response.status().is_success() {
        return Err(ModelsError::Http(response.status().as_u16()));
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_MODELS_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ModelsError::Invalid)?;
    if bytes.len() as u64 > MAX_MODELS_BYTES {
        return Err(ModelsError::Invalid);
    }
    serde_json::from_slice::<List>(&bytes)
        .map(|list| list.models)
        .map_err(|_| ModelsError::Invalid)
}
```

- [ ] **Step 6: Register the kind and the per-kind key texts**

In `src/provider/mod.rs`: add `pub mod kev;` next to `pub mod fake;`; in the module doc's step 1, write "next to `openrouter`, `kev` and `fake`" and add a sentence after the list: "`systemone` holds the request, client and answer parsing every System One service shares."

```rust
/// Registered kinds, in the order setup offers them.
pub const KINDS: &[&str] = &["openrouter", "kev", "fake"];

pub fn provider_for(kind: &str) -> Result<&'static dyn DecisionProvider> {
    match kind {
        "openrouter" => Ok(&openrouter::OpenRouter),
        "kev" => Ok(&kev::Kev),
        "fake" => Ok(&fake::Fake),
        _ => bail!("unsupported provider kind"),
    }
}

/// Whose key a config needs, for messages: "OpenRouter" or "Kev server".
pub fn key_owner(config: &ProviderConfig) -> &'static str {
    if config.kind == "kev" {
        "Kev server"
    } else {
        "OpenRouter"
    }
}
```

In `validate_configuration`, replace

```rust
            None if config.api_key_env.is_empty() => {
                bail!("OpenRouter provider needs api_key_command or api_key_env")
            }
```

with

```rust
            None if config.api_key_env.is_empty() => {
                if config.kind == "kev" {
                    bail!("Kev server needs api_key_command or api_key_env")
                }
                bail!("OpenRouter provider needs api_key_command or api_key_env")
            }
```

In `src/secrets.rs`, `resolve_key`, replace the two environment texts:

```rust
    let owner = crate::provider::key_owner(config);
    let key = std::env::var(&config.api_key_env)
        .map_err(|_| anyhow!("{owner} API key environment variable is missing"))?;
    if key.trim().is_empty() {
        bail!("{owner} API key environment variable is empty");
    }
    Ok(key)
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test --locked --test provider_contract && cargo test --locked --test key_command && cargo test --locked --test setup setup_offers_exactly_the_registered_kinds`
Expected: PASS. The contract suite now runs `openrouter`, `kev` and `fake`; the setup menu test lists the three registered labels (Task 4 replaces that menu).

- [ ] **Step 8: Run the full verification**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`
Expected: all green.

- [ ] **Step 9: Commit**

```bash
git add src/provider src/secrets.rs tests/provider_contract.rs tests/key_command.rs
git commit -m "Add the kev provider and the shared System One module" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 3: The managed runtime and `ProviderUnavailable`

**Files:**
- Create: `src/kev/mod.rs`, `src/kev/runtime.rs`, `src/kev/supervisor.rs`
- Modify: `src/lib.rs` (`pub mod kev;`)
- Modify: `src/process.rs` (`signal`, `SIGTERM`, `SIGKILL`)
- Modify: `src/provider/mod.rs` (`ProviderUnavailable`, `DecisionProvider::prepare`, `provider::prepare`)
- Modify: `src/provider/kev.rs` (`decide` for managed, `prepare`)
- Modify: `src/store.rs` (`Store::unlease`)
- Modify: `src/service.rs` (`Service` fields, `classification_skipped`, `process_one`, `fetch_and_classify`, `classify`, `reclassify`)
- Modify: `src/cli.rs` (hidden `kev run`)
- Create: `tests/kev_support/mod.rs`, `tests/kev_runtime.rs`
- Test: `tests/storage.rs`, `tests/cli.rs`

**Interfaces:**
- Consumes (Task 2): `provider::kev::{DEFAULT_MODEL, DEFAULT_CHECKPOINT, DEFAULT_PORT, local_endpoint, models_url, list_models, ModelsError, ModelCard}`; (Task 1) `ProviderConfig.managed`.
- Produces:
  - `provider::ProviderUnavailable(pub String)`: `Debug, Clone, PartialEq, Eq`, `Display` prints the string, implements `std::error::Error`
  - `DecisionProvider::prepare(&self, config: &ProviderConfig, key: Option<&str>) -> Result<(), ProviderUnavailable>` (default `Ok(())`)
  - `provider::prepare(config: &ProviderConfig, key: &KeyCache) -> Result<(), ProviderUnavailable>`
  - `kev::{TAG = "kev-1.0", REPO = "https://github.com/jaredpalmer/kev.git", IDLE, CHECK, STOP_GRACE, START_TIMEOUT, DOWNLOAD_TIMEOUT, TEST_ENV = "MAILTRIAGE_TEST_KEV"}`
  - `kev::Layout::{new(PathBuf), for_user() -> Option<Layout>, root(), src(), partial(), hf(), installed(), lock(), last_used(), log(), uv_env() -> [(&'static str, PathBuf); 3], touch_last_used(), idle_for() -> Duration}`
  - `kev::Installed { tag: String, checkpoint: String, installed_at: String, uv: PathBuf }` with `read(&Layout) -> Option<Installed>`, `write(&self, &Layout) -> io::Result<()>`, `matches(&self, checkpoint: &str) -> bool`
  - `kev::TestKev { apple_silicon, free_gb, memory_gb, port, start_seconds, download_seconds }` (all `Option`), `kev::test_kev() -> TestKev`, `kev::apple_silicon() -> bool`, `kev::port() -> u16`, `kev::download_timeout() -> Duration`
  - `kev::runtime::{NOT_INSTALLED, STOP_WAIT}`, `kev::runtime::Runtime { layout, port, checkpoint, exe, start_timeout, idle, check }` with `Runtime::for_config(&ProviderConfig) -> Result<Runtime, ProviderUnavailable>`
  - `kev::runtime::{ensure_running(&ProviderConfig), ensure_running_with(&Runtime)} -> Result<(), ProviderUnavailable>`
  - `kev::runtime::Probe { Ready, Other, Nothing }`, `kev::runtime::probe(&Runtime) -> Probe`
  - `kev::runtime::spawn_supervisor(rt: &Runtime, uv: &Path, allow_download: bool) -> io::Result<()>`
  - `kev::runtime::{stop(&Layout, Duration) -> bool, supervisor_running(&Layout) -> bool}`
  - `kev::supervisor::{RunArgs, run(&RunArgs) -> anyhow::Result<Value>}`
  - `Store::unlease(&mut self, id: &str, generation: &str) -> Result<()>`
  - `tests/kev_support`: `serve_from_env()`, `uv_script(control: &Path, exe: &Path) -> String`, `models_for(run: &str) -> Value`, `Stub::{start(models: Value, systemone_status: Option<u16>) -> Stub, endpoint(&self) -> String, requests(&self) -> Vec<String>}`, `free_port() -> u16`, `wait_until(Duration, impl FnMut() -> bool) -> bool`

**How the tests reach a "Kev server" without Python.** The stand-in `uv` is a shell script with two absolute paths baked in: a control directory and the running test binary (`std::env::current_exe()`). `uv sync` logs and makes `.venv`; `uv run ... --run C --port P` logs, then `exec`s the test binary with `kev_standin_server --exact --ignored --nocapture`, which runs that file's ignored test `kev_standin_server`, which calls `kev_support::serve_from_env()`: a loopback HTTP server on port P that answers `/v1/models` for checkpoint C and `/v1/systemone` with a valid answer for whatever questions it gets. Control files in the control directory make it misbehave (`exit-now`, `delay`). `cargo test` never runs the ignored test, and `serve_from_env` returns at once without its environment.

- [ ] **Step 1: Write the test support**

Create `tests/kev_support/mod.rs`:

```rust
#![allow(dead_code)]
//! Kev without Python, uv, a network or the model: a stand-in Kev server,
//! an in-process `Stub`, and stand-in tool scripts. The stand-in `uv run`
//! execs the test binary itself, whose ignored test `kev_standin_server`
//! calls `serve_from_env`; each test file that uses `uv_script` declares
//! that test.
use serde_json::{json, Map, Value};
use std::{
    fs,
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

/// A port nothing listens on right now.
pub fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Polls `done` every 100 ms until it is true or `timeout` passes.
pub fn wait_until(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    done()
}

/// What Kev's `GET /v1/models` lists for `--run run`.
pub fn models_for(run: &str) -> Value {
    json!({"models": [
        {"name": "kev-latest", "run": run, "device": "stand-in"},
        {"name": "jev-latest", "run": run, "device": "stand-in"}
    ]})
}

/// A valid System One answer for `request`'s questions: the first label
/// (in the sorted order the request has) at 0.9, the rest sharing 0.1,
/// four decimals, as Kev rounds.
pub fn answer(request: &Value) -> Value {
    let mut answers = Map::new();
    for (id, question) in request["questions"].as_object().into_iter().flatten() {
        if question["type"] == "noul" {
            answers.insert(id.clone(), json!({"type": "noul", "noul": 0.8123}));
            continue;
        }
        let labels: Vec<&String> = question["criteria"]
            .as_object()
            .map(|c| c.keys().collect())
            .unwrap_or_default();
        let Some(&chosen) = labels.first() else {
            continue;
        };
        let others = labels.len().saturating_sub(1).max(1) as f64;
        let rest = (0.1 / others * 10_000.0).round() / 10_000.0;
        let top = if labels.len() == 1 { 1.0 } else { 0.9 };
        let probabilities: Map<String, Value> = labels
            .iter()
            .map(|&l| (l.clone(), json!(if l == chosen { top } else { rest })))
            .collect();
        answers.insert(
            id.clone(),
            json!({"type": "choice", "choice": chosen, "confidence": 0.8667, "probabilities": probabilities}),
        );
    }
    json!({"model": request["model"], "answers": answers,
           "usage": {"input_tokens": 100, "output_tokens": 20}, "latency_ms": 5.0})
}

/// Reads one request: its head and body.
fn read_request(stream: &mut TcpStream) -> Option<(String, Vec<u8>)> {
    let mut bytes = Vec::new();
    let mut buf = [0u8; 8192];
    let head_end = loop {
        let n = stream.read(&mut buf).ok()?;
        if n == 0 {
            return None;
        }
        bytes.extend_from_slice(&buf[..n]);
        if let Some(p) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
    };
    let head = String::from_utf8_lossy(&bytes[..head_end]).into_owned();
    let length: usize = head
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .and_then(|v| v.trim().parse().ok())
        })
        .unwrap_or(0);
    while bytes.len() < head_end + length {
        let n = stream.read(&mut buf).ok()?;
        if n == 0 {
            return None;
        }
        bytes.extend_from_slice(&buf[..n]);
    }
    Some((head, bytes[head_end..head_end + length].to_vec()))
}

/// Answers one connection: `GET .../v1/models` with `models`,
/// `POST .../v1/systemone` with `answer` (or `systemone_status` and a body
/// that must never reach an error). Returns `"METHOD PATH auth=yes|no"`.
fn respond(
    mut stream: TcpStream,
    models: &Value,
    systemone_status: Option<u16>,
) -> Option<String> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let (head, body) = read_request(&mut stream)?;
    let mut first = head.lines().next()?.split(' ');
    let (method, path) = (first.next()?.to_owned(), first.next()?.to_owned());
    let auth = head
        .to_ascii_lowercase()
        .contains("\r\nauthorization: bearer ");
    let (status, reply) = if method == "GET" && path.ends_with("/v1/models") {
        (200, models.clone())
    } else if method == "POST" && path.ends_with("/v1/systemone") {
        match systemone_status {
            Some(code) => (code, json!({"detail": "stub secret diagnostic"})),
            None => (200, answer(&serde_json::from_slice(&body).ok()?)),
        }
    } else {
        (404, json!({"detail": "not found"}))
    };
    let text = reply.to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 {status} Stand-in\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    );
    Some(format!(
        "{method} {path} auth={}",
        if auth { "yes" } else { "no" }
    ))
}

/// The stand-in Kev server, run as the test binary's ignored test
/// `kev_standin_server` by the stand-in `uv run`. It reads
/// `KEV_STANDIN_DIR` (the control directory), `KEV_STANDIN_PORT` and
/// `KEV_STANDIN_RUN`; without them (a plain `cargo test -- --ignored`) it
/// returns at once. In the control directory, `exit-now` makes it exit 3
/// before listening and `delay` (seconds) makes it wait before listening.
/// Requests go to `requests.log`. Orphaned, it ends after 120 s without a
/// request.
pub fn serve_from_env() {
    let (Some(control), Ok(port), Ok(run)) = (
        std::env::var_os("KEV_STANDIN_DIR"),
        std::env::var("KEV_STANDIN_PORT"),
        std::env::var("KEV_STANDIN_RUN"),
    ) else {
        return;
    };
    let control = PathBuf::from(control);
    if control.join("exit-now").exists() {
        std::process::exit(3);
    }
    if let Ok(delay) = fs::read_to_string(control.join("delay")) {
        thread::sleep(Duration::from_secs(delay.trim().parse().unwrap_or(0)));
    }
    let port: u16 = port.parse().expect("KEV_STANDIN_PORT is a port");
    let listener = TcpListener::bind(("127.0.0.1", port)).expect("the stand-in Kev port is free");
    listener.set_nonblocking(true).unwrap();
    let models = models_for(&run);
    let log = control.join("requests.log");
    let mut last = Instant::now();
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                last = Instant::now();
                if let Some(line) = respond(stream, &models, None) {
                    let mut file = fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&log)
                        .unwrap();
                    let _ = writeln!(file, "{line}");
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                if last.elapsed() > Duration::from_secs(120) {
                    std::process::exit(0);
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => std::process::exit(1),
        }
    }
}

/// The stand-in `uv`. Each call appends
/// `ARGS|cwd=PATH|HF_HOME=..|HF_HUB_OFFLINE=..|UV_CACHE_DIR=..|UV_PYTHON_INSTALL_DIR=..`
/// to `control/uv.log` (`HF_HUB_OFFLINE=unset` when it is not set). `sync`
/// makes `.venv` (fails with `control/uv-sync-fails`); `run` execs the
/// stand-in Kev server in `exe`.
pub fn uv_script(control: &Path, exe: &Path) -> String {
    format!(
        r#"#!/bin/sh
control='{control}'
printf '%s|cwd=%s|HF_HOME=%s|HF_HUB_OFFLINE=%s|UV_CACHE_DIR=%s|UV_PYTHON_INSTALL_DIR=%s\n' "$*" "$(pwd -P)" "${{HF_HOME-}}" "${{HF_HUB_OFFLINE-unset}}" "${{UV_CACHE_DIR-}}" "${{UV_PYTHON_INSTALL_DIR-}}" >> "$control/uv.log"
case "$1" in
  sync)
    if [ -f "$control/uv-sync-fails" ]; then echo 'stand-in uv: sync failed' >&2; exit 1; fi
    mkdir -p .venv ;;
  run)
    port=; run=
    while [ $# -gt 0 ]; do
      case "$1" in
        --port) port="$2"; shift ;;
        --run) run="$2"; shift ;;
      esac
      shift
    done
    KEV_STANDIN_DIR="$control" KEV_STANDIN_PORT="$port" KEV_STANDIN_RUN="$run" exec '{exe}' kev_standin_server --exact --ignored --nocapture --test-threads=1 ;;
  *) exit 64 ;;
esac
"#,
        control = control.display(),
        exe = exe.display()
    )
}

/// An in-process Kev server on 127.0.0.1 for tests that need no managed
/// runtime. It runs until the test process ends.
pub struct Stub {
    pub port: u16,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Stub {
    /// Serves `models` at `GET .../v1/models`; `POST .../v1/systemone`
    /// answers each question, or replies `systemone_status`.
    pub fn start(models: Value, systemone_status: Option<u16>) -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&requests);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if let Some(line) = respond(stream, &models, systemone_status) {
                    seen.lock().unwrap().push(line);
                }
            }
        });
        Stub { port, requests }
    }

    pub fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}/v1/systemone", self.port)
    }

    /// `"METHOD PATH auth=yes|no"` per request so far.
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}
```

- [ ] **Step 2: Write the failing runtime and skip tests**

Create `tests/kev_runtime.rs`:

```rust
#![cfg(unix)]
//! Kev spec, "The managed runtime" and the skips: `ensure_running` against
//! the stand-in Kev server (started by the real `mailtriage kev run`), and
//! classification skipped without an attempt while Kev is unavailable.
mod common;
mod kev_support;
use common::{mail, write_tool, Harness};
use kev_support::{free_port, models_for, wait_until, Stub};
use mailtriage::{
    config,
    domain::{FilingMode, ManagedKev, ProviderConfig},
    kev::{
        runtime::{self, Probe, Runtime},
        Installed, Layout, TAG,
    },
    provider,
};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::Duration,
};

/// The stand-in Kev server, run by the stand-in `uv` (see `kev_support`).
#[test]
#[ignore = "the stand-in Kev server; the stand-in uv runs it"]
fn kev_standin_server() {
    kev_support::serve_from_env();
}

const CHECKPOINT: &str = "test/kev@v0";

/// A temp root with `data/mailtriage/kev` (the layout `mailtriage` finds
/// with `XDG_DATA_HOME=root/data`), a stand-in `uv` in `bin` and its
/// control directory, and a free port. Dropping it stops any supervisor.
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    layout: Layout,
    uv: PathBuf,
    control: PathBuf,
    port: u16,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let layout = Layout::new(root.join("data/mailtriage/kev"));
        let (bin, control) = (root.join("bin"), root.join("control"));
        for d in [&bin, &control, &root.join("home")] {
            fs::create_dir_all(d).unwrap();
        }
        let exe = std::env::current_exe().unwrap();
        write_tool(&bin, "uv", &kev_support::uv_script(&control, &exe));
        Self {
            uv: bin.join("uv"),
            _dir: dir,
            root,
            layout,
            control,
            port: free_port(),
        }
    }

    /// What a finished setup leaves: the checkout and `installed.json`.
    fn install(&self, checkpoint: &str) {
        fs::create_dir_all(self.layout.src()).unwrap();
        Installed {
            tag: TAG.into(),
            checkpoint: checkpoint.into(),
            installed_at: "2026-10-11T00:00:00+00:00".into(),
            uv: self.uv.clone(),
        }
        .write(&self.layout)
        .unwrap();
    }

    fn runtime(&self) -> Runtime {
        Runtime {
            layout: self.layout.clone(),
            port: self.port,
            checkpoint: CHECKPOINT.into(),
            exe: PathBuf::from(env!("CARGO_BIN_EXE_mailtriage")),
            start_timeout: Duration::from_secs(30),
            idle: Duration::from_secs(900),
            check: Duration::from_secs(30),
        }
    }

    fn control(&self, name: &str, text: &str) {
        fs::write(self.control.join(name), text).unwrap();
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.control.join(name)).unwrap_or_default()
    }

    /// The `uv run` lines of `uv.log`.
    fn runs(&self) -> Vec<String> {
        self.read("uv.log")
            .lines()
            .filter(|l| l.starts_with("run "))
            .map(str::to_owned)
            .collect()
    }

    fn kev_log(&self) -> String {
        fs::read_to_string(self.layout.log()).unwrap_or_default()
    }

    /// The provider block of a managed Kev on this fixture's port.
    fn managed_config(&self) -> ProviderConfig {
        let mut provider = provider::provider_for("kev").unwrap().new_config();
        provider.endpoint = provider::kev::local_endpoint(self.port);
        provider.managed = Some(ManagedKev {
            checkpoint: CHECKPOINT.into(),
        });
        provider
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        runtime::stop(&self.layout, runtime::STOP_WAIT);
    }
}

#[test]
fn the_first_caller_starts_kev_and_the_next_reuses_it() {
    let f = Fixture::new();
    f.install(CHECKPOINT);
    let rt = f.runtime();
    runtime::ensure_running_with(&rt).unwrap();
    runtime::ensure_running_with(&rt).unwrap();
    let runs = f.runs();
    assert_eq!(runs.len(), 1, "{}", f.read("uv.log"));
    let run = &runs[0];
    assert!(
        run.starts_with(&format!(
            "run --frozen --extra serve python -m kev.serve --run {CHECKPOINT} --host 127.0.0.1 --port {}|",
            f.port
        )),
        "{run}"
    );
    assert!(run.contains(&format!("|cwd={}|", f.layout.src().display())), "{run}");
    assert!(run.contains(&format!("|HF_HOME={}|", f.layout.hf().display())), "{run}");
    assert!(run.contains("|HF_HUB_OFFLINE=1|"), "{run}");
    assert!(
        run.contains(&format!("|UV_CACHE_DIR={}|", f.layout.root().join("uv-cache").display())),
        "{run}"
    );
    assert!(
        run.ends_with(&format!("|UV_PYTHON_INSTALL_DIR={}", f.layout.root().join("python").display())),
        "{run}"
    );
    assert!(f.layout.last_used().exists());
    assert_eq!(runtime::probe(&rt), Probe::Ready);
    assert!(runtime::supervisor_running(&f.layout));
    assert!(
        f.kev_log()
            .contains(&format!("starting Kev: --run {CHECKPOINT} on 127.0.0.1:{}", f.port)),
        "{}",
        f.kev_log()
    );
}

#[test]
fn without_a_matching_install_kev_is_not_installed() {
    let f = Fixture::new();
    let not_installed = "local Kev is not installed; run mailtriage setup --update";
    let e = runtime::ensure_running_with(&f.runtime()).unwrap_err();
    assert_eq!(e.to_string(), not_installed);
    f.install("jaredpalmer/kev-9b@v1.0");
    let e = runtime::ensure_running_with(&f.runtime()).unwrap_err();
    assert_eq!(e.to_string(), not_installed);
    assert!(f.runs().is_empty());
}

/// Review focus 2: another program on the port, even one that serves
/// `kev-latest` from another checkpoint, is never used.
#[test]
fn another_program_on_the_port_is_named() {
    let f = Fixture::new();
    f.install(CHECKPOINT);
    let other = Stub::start(models_for("jaredpalmer/kev-9b@v1.0"), None);
    let mut rt = f.runtime();
    rt.port = other.port;
    let e = runtime::ensure_running_with(&rt).unwrap_err();
    assert_eq!(
        e.to_string(),
        format!("port {} is used by another program", other.port)
    );
    assert!(f.runs().is_empty());
    assert!(other
        .requests()
        .iter()
        .all(|r| r.starts_with("GET /v1/models")));
}

#[test]
fn two_callers_at_once_leave_one_server() {
    let f = Fixture::new();
    f.install(CHECKPOINT);
    let rt = f.runtime();
    thread::scope(|s| {
        let a = s.spawn(|| runtime::ensure_running_with(&rt));
        let b = s.spawn(|| runtime::ensure_running_with(&rt));
        a.join().unwrap().unwrap();
        b.join().unwrap().unwrap();
    });
    assert_eq!(f.runs().len(), 1, "{}", f.read("uv.log"));
}

#[test]
fn kev_stops_after_the_idle_time_unless_it_is_used() {
    let f = Fixture::new();
    f.install(CHECKPOINT);
    let mut rt = f.runtime();
    rt.idle = Duration::from_secs(3);
    rt.check = Duration::from_secs(1);
    runtime::ensure_running_with(&rt).unwrap();
    // A request every second keeps it running past the idle time.
    for _ in 0..5 {
        thread::sleep(Duration::from_secs(1));
        runtime::ensure_running_with(&rt).unwrap();
    }
    assert!(runtime::supervisor_running(&f.layout));
    assert!(wait_until(Duration::from_secs(20), || !runtime::supervisor_running(&f.layout)));
    assert_eq!(runtime::probe(&rt), Probe::Nothing);
    assert!(
        f.kev_log()
            .contains("stopping Kev after 3 seconds without a request"),
        "{}",
        f.kev_log()
    );
    assert_eq!(f.runs().len(), 1);
}

#[test]
fn a_server_that_does_not_answer_in_time_is_reported() {
    let f = Fixture::new();
    f.install(CHECKPOINT);
    f.control("delay", "60");
    let mut rt = f.runtime();
    rt.start_timeout = Duration::from_secs(3);
    let e = runtime::ensure_running_with(&rt).unwrap_err();
    assert_eq!(
        e.to_string(),
        format!("local Kev did not start; see {}", f.layout.log().display())
    );
}

#[test]
fn kev_exiting_ends_the_supervisor_and_is_logged() {
    let f = Fixture::new();
    f.install(CHECKPOINT);
    f.control("exit-now", "");
    let mut rt = f.runtime();
    rt.start_timeout = Duration::from_secs(3);
    assert!(runtime::ensure_running_with(&rt).is_err());
    assert!(wait_until(Duration::from_secs(10), || !runtime::supervisor_running(&f.layout)));
    assert!(f.kev_log().contains("Kev exited (exit status: 3)"), "{}", f.kev_log());
}

/// Review focus 1: the supervisor leads its own session, so a service
/// manager that ends `watch`'s process group leaves Kev running.
#[test]
fn the_supervisor_runs_in_its_own_session() {
    let f = Fixture::new();
    f.install(CHECKPOINT);
    runtime::ensure_running_with(&f.runtime()).unwrap();
    let pid = fs::read_to_string(f.layout.lock()).unwrap().trim().to_owned();
    let out = Command::new("ps")
        .args(["-o", "pgid=", "-p", &pid])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), pid);
}

/// `(state, attempts)` of every job of the Harness config at `config`.
fn jobs(config: &Path) -> Vec<(String, u32)> {
    let db = rusqlite::Connection::open(
        config.with_file_name("mailtriage-state/mailtriage.sqlite"),
    )
    .unwrap();
    let mut stmt = db
        .prepare("SELECT state, attempts FROM jobs ORDER BY message_id")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// Kev spec, "Skips": an uninstalled local Kev skips classification like a
/// missing key: the mail stays queued and no attempt is counted. The
/// checkpoint is one no real install has, so the user's data dir is only
/// read.
#[test]
fn an_uninstalled_local_kev_skips_classification_without_an_attempt() {
    let h = Harness::new(FilingMode::Off);
    h.edit(|c| {
        c.provider = provider::provider_for("kev").unwrap().new_config();
        c.provider.managed = Some(ManagedKev {
            checkpoint: "test/never-installed@v0".into(),
        });
    });
    h.fake
        .deliver("INBOX", &mail("a", "Invoice", "Please pay by Friday."));
    let out = h.sync();
    assert_eq!(
        out["classification"],
        json!({"skipped": true, "reason": "local Kev is not installed; run mailtriage setup --update"}),
        "{out}"
    );
    assert_eq!(out["partial"], true);
    assert_eq!(jobs(&h.path), vec![("queued".to_owned(), 0)]);
}

fn kev_server(h: &Harness, endpoint: &str) {
    h.edit(|c| {
        c.provider = provider::provider_for("kev").unwrap().new_config();
        c.provider.managed = None;
        c.provider.endpoint = endpoint.into();
    });
}

#[test]
fn an_unreachable_kev_server_skips_classification() {
    let h = Harness::new(FilingMode::Off);
    let port = free_port();
    kev_server(&h, &provider::kev::local_endpoint(port));
    h.fake
        .deliver("INBOX", &mail("a", "Invoice", "Please pay by Friday."));
    let out = h.sync();
    assert_eq!(
        out["classification"]["reason"],
        format!("the Kev server did not answer at http://127.0.0.1:{port}/v1/models"),
        "{out}"
    );
    assert_eq!(jobs(&h.path), vec![("queued".to_owned(), 0)]);
}

#[test]
fn a_kev_server_classifies_and_its_http_errors_count_an_attempt() {
    let h = Harness::new(FilingMode::Off);
    let good = Stub::start(models_for("any"), None);
    kev_server(&h, &good.endpoint());
    h.fake
        .deliver("INBOX", &mail("a", "Lunch", "Are you free on Monday?"));
    let out = h.sync();
    assert_eq!(out["classified"], 1, "{out}");
    assert!(out.get("classification").is_none(), "{out}");
    assert!(good
        .requests()
        .contains(&"POST /v1/systemone auth=no".to_owned()));
    // Another account state: a provider change would queue "a" again.
    let h = Harness::new(FilingMode::Off);
    let failing = Stub::start(models_for("any"), Some(500));
    kev_server(&h, &failing.endpoint());
    h.fake
        .deliver("INBOX", &mail("b", "Invoice", "Please pay by Friday."));
    let out = h.sync();
    assert_eq!(out["failed"], 1, "{out}");
    assert_eq!(jobs(&h.path), vec![("retry".to_owned(), 1)]);
}

/// Through the binary, with an empty data dir: `classify` skips and
/// `export` lists no attempt.
#[test]
fn classify_skips_while_local_kev_is_not_installed() {
    let f = Fixture::new();
    let path = f.root.join("config.json");
    let mut c = config::default_config();
    c.provider = f.managed_config();
    c.state_dir = f.root.join("state");
    config::save(&path, &c).unwrap();
    let run = |args: &[&str]| -> Value {
        let out = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
            .arg("--config")
            .arg(&path)
            .args(args)
            .env("HOME", f.root.join("home"))
            .env("XDG_DATA_HOME", f.root.join("data"))
            .output()
            .unwrap();
        serde_json::from_slice(&out.stdout).unwrap()
    };
    let receipt = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/receipt.eml");
    let v = run(&["classify", "--account", "work", "--input", receipt, "--format", "rfc822", "--json"]);
    assert_eq!(v["outcome"], "skipped", "{v}");
    assert_eq!(
        v["classification"]["reason"],
        "local Kev is not installed; run mailtriage setup --update"
    );
    let export = run(&["export", "--account", "work", "--json"]);
    assert_eq!(export["attempts"], json!([]), "{export}");
    assert!(f.runs().is_empty());
}
```

In `tests/storage.rs`, append:

```rust
/// Kev spec: a job taken while the provider turned out unavailable goes
/// back without using an attempt.
#[test]
fn an_unleased_job_is_queued_again_without_an_attempt() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("db");
    let mut st = Store::open(&p).unwrap();
    st.ensure_account("work", "identity", "g").unwrap();
    let msg = normalize::rfc822(&mail("unavailable"), 1000).unwrap();
    let id = st.ingest("work", &msg, "g").unwrap();
    assert!(st.lease(&id, "g", 300).unwrap());
    assert!(st.queued("work", 10).unwrap().is_empty());
    st.unlease(&id, "g").unwrap();
    assert_eq!(st.queued("work", 10).unwrap(), vec![id.clone()]);
    let attempts: u32 = rusqlite::Connection::open(&p)
        .unwrap()
        .query_row("SELECT attempts FROM jobs WHERE message_id=?", [&id], |r| r.get(0))
        .unwrap();
    assert_eq!(attempts, 0);
}
```

In `tests/cli.rs`, append:

```rust
/// `kev run` is mailtriage's own supervisor, not a command for people.
#[test]
fn the_kev_supervisor_is_hidden() {
    let out = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
        .arg("--help")
        .output()
        .unwrap();
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(!help.contains("kev"), "{help}");
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --locked --test kev_runtime 2>&1 | tail -20`
Expected: compile error, `could not find kev in mailtriage`.

- [ ] **Step 4: Add the signal helper**

In `src/process.rs`, after `terminate`:

```rust
pub(crate) const SIGTERM: i32 = 15;
pub(crate) const SIGKILL: i32 = 9;

/// Sends `signal` to process `pid`, or to process group `-pid`; `false`
/// when it could not be sent.
#[cfg(unix)]
pub(crate) fn signal(pid: i32, signal: i32) -> bool {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    unsafe { kill(pid, signal) == 0 }
}

#[cfg(not(unix))]
pub(crate) fn signal(_pid: i32, _signal: i32) -> bool {
    false
}
```

- [ ] **Step 5: Add `ProviderUnavailable` and `prepare`**

In `src/provider/mod.rs`, add `use std::fmt;` to the imports and, after `Decision`:

```rust
/// The provider cannot answer now, for a reason outside the message: a
/// local server that is not installed or does not start, a server that
/// does not answer. Like a missing key it skips classification: no job is
/// taken, no attempt is used, and the reason is reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderUnavailable(pub String);

impl fmt::Display for ProviderUnavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProviderUnavailable {}
```

Add to the trait, after `new_config`:

```rust
    /// Whether the provider can answer now. Called once per command, after
    /// the key check and before any job is taken; `Err` skips
    /// classification like a missing key. Ready by default.
    fn prepare(
        &self,
        _config: &ProviderConfig,
        _key: Option<&str>,
    ) -> Result<(), ProviderUnavailable> {
        Ok(())
    }
```

Add after `key_owner`:

```rust
/// `prepare` of `config`'s provider, with the key when it needs one (the
/// caller has checked that the key resolves). An unknown kind is ready
/// here; validation refuses it.
pub fn prepare(config: &ProviderConfig, key: &KeyCache) -> Result<(), ProviderUnavailable> {
    let Ok(provider) = provider_for(&config.kind) else {
        return Ok(());
    };
    let key = provider
        .key_account(config)
        .and_then(|_| key.get(config).ok());
    provider.prepare(config, key.as_deref())
}
```

- [ ] **Step 6: Add the runtime module**

Add `pub mod kev;` to `src/lib.rs` (alphabetical, after `pub mod himalaya { ... }`).

Create `src/kev/mod.rs`:

```rust
//! Local Kev, run by mailtriage (Kev spec, "The managed runtime"): where
//! its files live, the record setup writes once it works
//! (`installed.json`), and the test stand-ins for this machine. `runtime`
//! starts the server on demand; `supervisor` is the hidden
//! `mailtriage kev run`.
pub mod runtime;
pub mod supervisor;

use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

/// The Kev release a managed install checks out.
pub const TAG: &str = "kev-1.0";
pub const REPO: &str = "https://github.com/jaredpalmer/kev.git";
/// How long Kev may go without a request before the supervisor stops it.
pub const IDLE: Duration = Duration::from_secs(15 * 60);
/// How often the supervisor looks.
pub const CHECK: Duration = Duration::from_secs(30);
/// How long Kev has after TERM before it gets KILL.
pub const STOP_GRACE: Duration = Duration::from_secs(10);
/// How long `ensure_running` waits for a server it started.
pub const START_TIMEOUT: Duration = Duration::from_secs(120);
/// How long setup waits for the first start, which downloads the weights.
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Debug builds only: a JSON object (`TestKev`) that describes this
/// machine to tests. Release builds never read it.
pub const TEST_ENV: &str = "MAILTRIAGE_TEST_KEV";

/// `<data dir>/mailtriage/kev` and the files in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    root: PathBuf,
}

impl Layout {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// This user's: under `XDG_DATA_HOME`, else `~/.local/share`, next to
    /// the private Himalaya; `None` without either.
    pub fn for_user() -> Option<Self> {
        crate::distribution::himalaya::default_data_dir()
            .map(|data| Self::new(data.join("mailtriage").join("kev")))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    /// The git checkout of Kev, with its `.venv`.
    pub fn src(&self) -> PathBuf {
        self.root.join("src")
    }
    /// Where a clone goes before it is renamed to `src`.
    pub fn partial(&self) -> PathBuf {
        self.root.join("src.partial")
    }
    /// `HF_HOME`: the downloaded weights.
    pub fn hf(&self) -> PathBuf {
        self.root.join("hf")
    }
    pub fn installed(&self) -> PathBuf {
        self.root.join("installed.json")
    }
    /// Held by the running supervisor; holds its PID.
    pub fn lock(&self) -> PathBuf {
        self.root.join("server.lock")
    }
    /// Its mtime is the last request.
    pub fn last_used(&self) -> PathBuf {
        self.root.join("last-used")
    }
    pub fn log(&self) -> PathBuf {
        self.root.join("kev.log")
    }

    /// What every `uv` call gets: the weights in `hf`, and uv's Python and
    /// cache inside this directory too, so deleting it removes everything.
    pub fn uv_env(&self) -> [(&'static str, PathBuf); 3] {
        [
            ("HF_HOME", self.hf()),
            ("UV_CACHE_DIR", self.root.join("uv-cache")),
            ("UV_PYTHON_INSTALL_DIR", self.root.join("python")),
        ]
    }

    /// Records a request now.
    pub fn touch_last_used(&self) {
        let _ = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.last_used())
            .and_then(|file| file.set_modified(SystemTime::now()));
    }

    /// The time since the last request; without a record, forever.
    pub fn idle_for(&self) -> Duration {
        match fs::metadata(self.last_used()).and_then(|m| m.modified()) {
            Ok(at) => at.elapsed().unwrap_or(Duration::ZERO),
            Err(_) => Duration::MAX,
        }
    }
}

/// `installed.json`, written last by setup: the proof that the code and the
/// weights are in place, and the `uv` that runs them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    pub tag: String,
    pub checkpoint: String,
    pub installed_at: String,
    pub uv: PathBuf,
}

impl Installed {
    /// `None` when missing or unreadable: not installed.
    pub fn read(layout: &Layout) -> Option<Self> {
        serde_json::from_slice(&fs::read(layout.installed()).ok()?).ok()
    }

    /// This release, and `checkpoint`.
    pub fn matches(&self, checkpoint: &str) -> bool {
        self.tag == TAG && self.checkpoint == checkpoint
    }

    /// Written to a temporary file and renamed, so a reader sees all or
    /// nothing.
    pub fn write(&self, layout: &Layout) -> std::io::Result<()> {
        fs::create_dir_all(layout.root())?;
        let partial = layout.root().join("installed.json.partial");
        fs::write(
            &partial,
            serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?,
        )?;
        fs::rename(partial, layout.installed())
    }
}

/// This machine as tests describe it (`MAILTRIAGE_TEST_KEV`); every field
/// left out is the real value.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestKev {
    pub apple_silicon: Option<bool>,
    pub free_gb: Option<u64>,
    pub memory_gb: Option<u64>,
    pub port: Option<u16>,
    pub start_seconds: Option<u64>,
    pub download_seconds: Option<u64>,
}

/// `MAILTRIAGE_TEST_KEV` in debug builds; empty otherwise.
pub fn test_kev() -> TestKev {
    #[cfg(debug_assertions)]
    if let Some(text) = std::env::var_os(TEST_ENV) {
        return serde_json::from_str(&text.to_string_lossy())
            .expect("MAILTRIAGE_TEST_KEV is a JSON object of TestKev fields");
    }
    TestKev::default()
}

/// A Mac with Apple Silicon: the only place mailtriage runs Kev itself.
pub fn apple_silicon() -> bool {
    test_kev()
        .apple_silicon
        .unwrap_or_else(|| crate::update::release::platform() == Some("macos-arm64"))
}

/// The port a new managed install uses: 8009.
pub fn port() -> u16 {
    test_kev()
        .port
        .unwrap_or(crate::provider::kev::DEFAULT_PORT)
}

/// How long setup waits for the first start.
pub fn download_timeout() -> Duration {
    test_kev()
        .download_seconds
        .map_or(DOWNLOAD_TIMEOUT, Duration::from_secs)
}
```

Create `src/kev/runtime.rs`:

```rust
//! Starting local Kev on demand (Kev spec, `ensure_running`) and stopping
//! it. Errors are `ProviderUnavailable`, so classification is skipped, not
//! failed.
use super::{Installed, Layout, CHECK, IDLE, START_TIMEOUT};
use crate::{
    domain::ProviderConfig,
    process,
    provider::{kev, ProviderUnavailable},
};
use fs2::FileExt;
use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub const NOT_INSTALLED: &str = "local Kev is not installed; run mailtriage setup --update";
/// How long `stop` waits for a supervisor to exit.
pub const STOP_WAIT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_secs(1);
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// One managed server: where its files are, its port and checkpoint, and
/// the mailtriage that runs its supervisor.
#[derive(Debug, Clone)]
pub struct Runtime {
    pub layout: Layout,
    pub port: u16,
    pub checkpoint: String,
    pub exe: PathBuf,
    pub start_timeout: Duration,
    pub idle: Duration,
    pub check: Duration,
}

impl Runtime {
    /// A managed `config`'s server: this user's layout, the endpoint's
    /// port, this program.
    pub fn for_config(config: &ProviderConfig) -> Result<Self, ProviderUnavailable> {
        let unavailable = |m: &str| ProviderUnavailable(m.to_owned());
        let checkpoint = config
            .managed
            .as_ref()
            .map(|m| m.checkpoint.clone())
            .ok_or_else(|| unavailable("provider.managed is missing"))?;
        let port = reqwest::Url::parse(&config.endpoint)
            .ok()
            .and_then(|url| url.port())
            .ok_or_else(|| unavailable("provider.endpoint names no port"))?;
        let layout =
            Layout::for_user().ok_or_else(|| unavailable("local Kev needs HOME to find its files"))?;
        let exe = std::env::current_exe()
            .map_err(|_| unavailable("cannot find this mailtriage program to start Kev"))?;
        Ok(Self {
            layout,
            port,
            checkpoint,
            exe,
            start_timeout: super::test_kev()
                .start_seconds
                .map_or(START_TIMEOUT, Duration::from_secs),
            idle: IDLE,
            check: CHECK,
        })
    }

    fn did_not_start(&self) -> ProviderUnavailable {
        ProviderUnavailable(format!(
            "local Kev did not start; see {}",
            self.layout.log().display()
        ))
    }

    fn port_taken(&self) -> ProviderUnavailable {
        ProviderUnavailable(format!("port {} is used by another program", self.port))
    }
}

/// What answers on a managed server's port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// Kev, serving `kev-latest` from this checkpoint.
    Ready,
    /// Something else.
    Other,
    /// Nothing.
    Nothing,
}

pub fn probe(rt: &Runtime) -> Probe {
    let url = format!("http://127.0.0.1:{}/v1/models", rt.port);
    match kev::list_models(&url, None, PROBE_TIMEOUT) {
        Ok(models)
            if models.iter().any(|m| {
                m.name == kev::DEFAULT_MODEL && m.run.as_deref() == Some(rt.checkpoint.as_str())
            }) =>
        {
            Probe::Ready
        }
        Err(kev::ModelsError::Unreachable) => Probe::Nothing,
        _ => Probe::Other,
    }
}

/// `ensure_running_with` for a managed config.
pub fn ensure_running(config: &ProviderConfig) -> Result<(), ProviderUnavailable> {
    ensure_running_with(&Runtime::for_config(config)?)
}

/// Kev answers on its port, started here if nothing does; records a
/// request. Concurrent callers may both start a supervisor: the lock keeps
/// one, and both wait until it answers.
pub fn ensure_running_with(rt: &Runtime) -> Result<(), ProviderUnavailable> {
    let installed = Installed::read(&rt.layout)
        .filter(|i| i.matches(&rt.checkpoint))
        .ok_or_else(|| ProviderUnavailable(NOT_INSTALLED.to_owned()))?;
    match probe(rt) {
        Probe::Ready => {
            rt.layout.touch_last_used();
            return Ok(());
        }
        Probe::Other => return Err(rt.port_taken()),
        Probe::Nothing => {}
    }
    spawn_supervisor(rt, &installed.uv, false).map_err(|_| rt.did_not_start())?;
    let deadline = Instant::now() + rt.start_timeout;
    loop {
        thread::sleep(POLL);
        match probe(rt) {
            Probe::Ready => {
                rt.layout.touch_last_used();
                return Ok(());
            }
            Probe::Other => return Err(rt.port_taken()),
            Probe::Nothing if Instant::now() >= deadline => return Err(rt.did_not_start()),
            Probe::Nothing => {}
        }
    }
}

#[cfg(unix)]
unsafe extern "C" {
    fn setsid() -> i32;
}

/// Starts `mailtriage kev run` detached: its own session (a service
/// manager ending this process's group leaves it running), stdin and
/// output closed. A thread reaps it, so a long `watch` keeps no zombies.
pub fn spawn_supervisor(rt: &Runtime, uv: &Path, allow_download: bool) -> std::io::Result<()> {
    let mut command = Command::new(&rt.exe);
    command
        .args(["kev", "run", "--dir"])
        .arg(rt.layout.root())
        .args(["--port", &rt.port.to_string(), "--checkpoint", &rt.checkpoint, "--uv"])
        .arg(uv)
        .args([
            "--idle-seconds",
            &rt.idle.as_secs().to_string(),
            "--check-seconds",
            &rt.check.as_secs().to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if allow_download {
        command.arg("--allow-download");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe and touches no memory.
        unsafe {
            command.pre_exec(|| {
                setsid();
                Ok(())
            });
        }
    }
    let mut child = command.spawn()?;
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn lock_file(layout: &Layout) -> Option<fs::File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(layout.lock())
        .ok()
}

/// Whether a supervisor holds `server.lock`.
pub fn supervisor_running(layout: &Layout) -> bool {
    let Some(file) = lock_file(layout) else {
        return false;
    };
    if file.try_lock_exclusive().is_ok() {
        let _ = FileExt::unlock(&file);
        return false;
    }
    true
}

/// Asks the running supervisor to stop Kev (TERM to the PID in
/// `server.lock`) and waits up to `wait` for it to exit. `false` when none
/// was running.
pub fn stop(layout: &Layout, wait: Duration) -> bool {
    if !layout.root().exists() {
        return false;
    }
    let Some(file) = lock_file(layout) else {
        return false;
    };
    if file.try_lock_exclusive().is_ok() {
        let _ = FileExt::unlock(&file);
        return false;
    }
    if let Some(pid) = fs::read_to_string(layout.lock())
        .ok()
        .and_then(|text| text.trim().parse::<i32>().ok())
        .filter(|&pid| pid > 0)
    {
        process::signal(pid, process::SIGTERM);
    }
    let deadline = Instant::now() + wait;
    while Instant::now() < deadline {
        if file.try_lock_exclusive().is_ok() {
            let _ = FileExt::unlock(&file);
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    true
}
```

Create `src/kev/supervisor.rs`:

```rust
//! `mailtriage kev run` (hidden): the supervisor `ensure_running` and setup
//! start. It holds `server.lock`, runs Kev with `uv`, stops it after the
//! idle time or on TERM, and exits when Kev exits, logging why.
use super::Layout;
use crate::process;
use anyhow::Result;
use fs2::FileExt;
use serde_json::{json, Value};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

/// `ETXTBSY`: another thread forked while a write handle to `uv` was open;
/// starting it is retried briefly, as `update::platform` does.
const TEXT_FILE_BUSY: i32 = 26;

/// `kev run`'s flags.
#[derive(Debug, Clone)]
pub struct RunArgs {
    pub dir: PathBuf,
    pub port: u16,
    pub checkpoint: String,
    pub uv: PathBuf,
    /// Setup's first start: the weights may be downloaded.
    pub allow_download: bool,
    pub idle: Duration,
    pub check: Duration,
    pub grace: Duration,
}

fn note(log: &mut File, text: &str) {
    let _ = writeln!(log, "{} {text}", chrono::Utc::now().to_rfc3339());
}

pub fn run(args: &RunArgs) -> Result<Value> {
    let layout = Layout::new(args.dir.clone());
    std::fs::create_dir_all(layout.root())?;
    let mut lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(layout.lock())?;
    if lock.try_lock_exclusive().is_err() {
        return Ok(json!({"schema_version": 1, "kev": {"action": "already_running"}}));
    }
    lock.set_len(0)?;
    write!(lock, "{}", std::process::id())?;
    lock.flush()?;
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(layout.log())?;
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        let _ = ctrlc::set_handler(move || stop.store(true, Ordering::SeqCst));
    }
    note(
        &mut log,
        &format!(
            "starting Kev: --run {} on 127.0.0.1:{}{}",
            args.checkpoint,
            args.port,
            if args.allow_download {
                " (downloads allowed)"
            } else {
                ""
            }
        ),
    );
    let port = args.port.to_string();
    let mut command = Command::new(&args.uv);
    command
        .args(["run", "--frozen", "--extra", "serve", "python", "-m", "kev.serve"])
        .args(["--run", &args.checkpoint, "--host", "127.0.0.1", "--port", &port])
        .current_dir(layout.src())
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log.try_clone()?);
    for (name, value) in layout.uv_env() {
        command.env(name, value);
    }
    if args.allow_download {
        command.env_remove("HF_HUB_OFFLINE");
    } else {
        command.env("HF_HUB_OFFLINE", "1");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = match spawn(&mut command) {
        Ok(child) => child,
        Err(e) => {
            note(&mut log, &format!("Kev could not start: {e}"));
            return Err(e.into());
        }
    };
    layout.touch_last_used();
    let mut checked = Instant::now();
    let action = loop {
        if let Some(status) = child.try_wait()? {
            note(&mut log, &format!("Kev exited ({status})"));
            break "exited";
        }
        if stop.load(Ordering::SeqCst) {
            note(&mut log, "stopping Kev on request");
            end(&mut child, args.grace);
            break "stopped";
        }
        if checked.elapsed() >= args.check {
            checked = Instant::now();
            if layout.idle_for() >= args.idle {
                note(
                    &mut log,
                    &format!(
                        "stopping Kev after {} seconds without a request",
                        args.idle.as_secs()
                    ),
                );
                end(&mut child, args.grace);
                break "idle";
            }
        }
        thread::sleep(Duration::from_millis(200));
    };
    Ok(json!({"schema_version": 1, "kev": {"action": action}}))
}

fn spawn(command: &mut Command) -> std::io::Result<Child> {
    let mut attempt = 0;
    loop {
        match command.spawn() {
            Err(e) if e.raw_os_error() == Some(TEXT_FILE_BUSY) && attempt < 20 => {
                attempt += 1;
                thread::sleep(Duration::from_millis(50));
            }
            result => return result,
        }
    }
}

/// TERM to Kev's process group, KILL after `grace`.
fn end(child: &mut Child, grace: Duration) {
    let group = -(child.id() as i32);
    process::signal(group, process::SIGTERM);
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
    process::signal(group, process::SIGKILL);
    let _ = child.wait();
}
```

- [ ] **Step 7: Start managed Kev from the adapter**

In `src/provider/kev.rs`, change the import line to `use super::{systemone, Decision, DecisionProvider, DecisionRequest, ProviderUnavailable};`, and in `decide` replace

```rust
        if config.managed.is_some() {
            // Replaced in the managed runtime's task by `ensure_running`.
            bail!("local Kev is not installed; run mailtriage setup --update");
        }
```

with

```rust
        if config.managed.is_some() {
            // Its errors are `ProviderUnavailable`: the job goes back
            // without an attempt.
            crate::kev::runtime::ensure_running(config)?;
        }
```

Add to `impl DecisionProvider for Kev`:

```rust
    /// Managed: Kev is installed and answers, started if needed. Your own
    /// server: `/v1/models` answers at all; anything it answers is left to
    /// the request, whose errors count an attempt.
    fn prepare(
        &self,
        config: &ProviderConfig,
        key: Option<&str>,
    ) -> Result<(), ProviderUnavailable> {
        if config.managed.is_some() {
            return crate::kev::runtime::ensure_running(config);
        }
        let url = models_url(&config.endpoint).unwrap_or_default();
        let timeout = Duration::from_secs(config.timeout_seconds.min(10));
        match list_models(&url, key, timeout) {
            Err(ModelsError::Unreachable) => Err(ProviderUnavailable(format!(
                "{} at {url}",
                ModelsError::Unreachable
            ))),
            _ => Ok(()),
        }
    }
```

- [ ] **Step 8: Skip without an attempt in the service**

In `src/store.rs`, after `lease`:

```rust
    /// Gives a leased job back without using an attempt: the provider was
    /// unavailable, so nothing was tried.
    pub fn unlease(&mut self, id: &str, generation: &str) -> Result<()> {
        self.db.execute(
            "UPDATE jobs SET state='queued',attempts=MAX(attempts-1,0),lease_until=NULL WHERE message_id=? AND generation=? AND state='leased'",
            params![id, generation],
        )?;
        Ok(())
    }
```

In `src/service.rs`, add `cell::OnceCell,` to the `std` imports. Add two fields to `Service` after `key: KeyCache,`:

```rust
    /// Whether the provider can answer (`provider::prepare`), asked at most
    /// once per `Service` like the key: `Some(reason)` when it cannot.
    ready: OnceCell<Option<String>>,
    /// Why a running pass stopped classifying: the provider became
    /// unavailable after `ready` (see `process_one`).
    unavailable: Option<String>,
```

and initialize them in `Service::open`: `ready: OnceCell::new(), unavailable: None,` after `key: KeyCache::default(),`.

Replace `classification_skipped` with:

```rust
    /// Why classification cannot run in this command: the provider needs a
    /// key and it is unavailable, or the provider itself cannot answer
    /// (`ProviderUnavailable`, such as local Kev not installed). Resolved
    /// once per `Service`, before any job is leased, so it consumes no
    /// attempts and the mail stays queued. Callers ask only when a job is
    /// eligible to lease, so an idle pass runs no key command and starts no
    /// server.
    fn classification_skipped(&self) -> Option<String> {
        let provider = &self.config.provider;
        if provider::key_account(provider).is_some() {
            if let Err(e) = self.key.get(provider) {
                return Some(e.to_string());
            }
        }
        self.ready
            .get_or_init(|| provider::prepare(provider, &self.key).err().map(|e| e.0))
            .clone()
    }
```

In `process_one`, replace the final `Err(_) => { self.store.fail(...) ... Ok("failed".into()) }` arm of the `classify_with_key` match with:

```rust
            Err(e) => {
                if let Some(unavailable) = e.downcast_ref::<provider::ProviderUnavailable>() {
                    // Nothing was tried: the job goes back without an
                    // attempt, and this command stops classifying.
                    self.store.unlease(id, generation)?;
                    self.unavailable = Some(unavailable.to_string());
                    return Ok("skipped".into());
                }
                self.store.fail(
                    id,
                    generation,
                    "classification provider failed; check doctor and retry",
                    self.config.policy.max_attempts,
                )?;
                Ok("failed".into())
            }
```

In `fetch_and_classify`, replace the loop body's `match self.process_one(...)?.as_str() { ... }` and the lines up to `if from.filing {` with:

```rust
            let outcome = self.process_one(name, account, generation, &id, h)?;
            match outcome.as_str() {
                "classified" => done.classified += 1,
                "failed" => done.failed += 1,
                "skipped" => {}
                _ => done.cached += 1,
            }
            // The canonical row: a merge redirects `id` to the existing message.
            let row = self.required(name, &id)?;
            if needed_fetch && row.normalized.is_some() {
                done.fetched += 1;
            }
            if outcome == "skipped" {
                return Ok((done, self.unavailable.take()));
            }
```

In `classify`, after `let outcome = match &skipped { ... };` add:

```rust
        let skipped = skipped.or_else(|| self.unavailable.take());
```

In `reclassify`, replace

```rust
        let mut failed = 0;
        for id in &selected {
            if self.process_one(name, &account, &generation, id, h.as_deref())? == "failed" {
                failed += 1;
            }
        }
        let mut out = json!({"schema_version":1,"matched":matched,"reclassified":selected.len()-failed,"pending":self.coverage(name)?["pending_jobs"],"failed":failed,"partial":failed>0});
```

with

```rust
        let mut failed = 0;
        let mut processed = 0;
        let mut skipped = skipped;
        for id in &selected {
            match self
                .process_one(name, &account, &generation, id, h.as_deref())?
                .as_str()
            {
                "failed" => failed += 1,
                "skipped" => {
                    skipped = self.unavailable.take();
                    break;
                }
                _ => {}
            }
            processed += 1;
        }
        let mut out = json!({"schema_version":1,"matched":matched,"reclassified":processed-failed,"pending":self.coverage(name)?["pending_jobs"],"failed":failed,"partial":failed>0});
```

- [ ] **Step 9: The hidden `kev run` command**

In `src/cli.rs`, add `kev,` to the `mailtriage::{...}` import list. Add to `enum Command`, after `SelfCmd`:

```rust
    /// Local Kev's supervisor; mailtriage starts it, not you.
    #[command(hide = true)]
    Kev {
        #[command(subcommand)]
        command: KevCommand,
    },
```

Add after `enum SelfCommand`:

```rust
#[derive(Subcommand)]
enum KevCommand {
    /// Run Kev until it has had no request for the idle time.
    Run(KevRunArg),
}

#[derive(Args)]
struct KevRunArg {
    #[arg(long)]
    dir: PathBuf,
    #[arg(long)]
    port: u16,
    #[arg(long)]
    checkpoint: String,
    #[arg(long)]
    uv: PathBuf,
    #[arg(long)]
    allow_download: bool,
    #[arg(long, default_value_t = 900)]
    idle_seconds: u64,
    #[arg(long, default_value_t = 30)]
    check_seconds: u64,
    #[arg(long, default_value_t = 10)]
    stop_grace_seconds: u64,
}
```

Add an arm to `execute`:

```rust
        Command::Kev {
            command: KevCommand::Run(arg),
        } => kev::supervisor::run(&kev::supervisor::RunArgs {
            dir: arg.dir.clone(),
            port: arg.port,
            checkpoint: arg.checkpoint.clone(),
            uv: arg.uv.clone(),
            allow_download: arg.allow_download,
            idle: Duration::from_secs(arg.idle_seconds),
            check: Duration::from_secs(arg.check_seconds),
            grace: Duration::from_secs(arg.stop_grace_seconds),
        })
        .map_err(service_error),
```

- [ ] **Step 10: Run the tests to verify they pass**

Run: `cargo test --locked --test kev_runtime && cargo test --locked --test storage && cargo test --locked --test cli && cargo test --locked --test key_unavailable && cargo test --locked --test provider_contract`
Expected: PASS. `kev_runtime` takes about 30 seconds (real processes, idle and start timeouts). Afterwards `pgrep -fl kev_standin_server` prints nothing.

- [ ] **Step 11: Run the full verification**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`
Expected: all green.

- [ ] **Step 12: Commit**

```bash
git add src/kev src/lib.rs src/process.rs src/provider src/store.rs src/service.rs src/cli.rs tests/kev_support tests/kev_runtime.rs tests/storage.rs tests/cli.rs
git commit -m "Run local Kev on demand and skip while it is unavailable" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 4: Setup: the classifier menu, `--kev-url`, the managed install

**Files:**
- Create: `src/kev/install.rs`
- Modify: `src/kev/mod.rs` (`pub mod install;`)
- Modify: `src/setup.rs` (`SetupArgs`, `run`, `classifier_step` and new helpers, `describe`, `chosen_store`, `key_step`, `doctor_step`'s key fix text)
- Modify: `src/cli.rs` (`SetupArg.kev_url`, `to_args`)
- Modify: `tests/kev_support/mod.rs` (`git_script`, `brew_script`)
- Test: `tests/setup.rs`

**Interfaces:**
- Consumes (Task 3): `kev::{Layout, Installed, TAG, REPO, test_kev, apple_silicon, port, download_timeout}`, `kev::runtime::{Runtime, Probe, probe, spawn_supervisor, stop, supervisor_running, STOP_WAIT}`; (Task 2) `provider::kev::{local_endpoint, models_url, list_models, ModelsError, DEFAULT_KEY_ENV}`, `provider::key_owner`; `tests/kev_support::{uv_script, Stub, models_for, free_port, serve_from_env}`.
- Produces:
  - `kev::install::install(p: &mut Prompter, provider: &ProviderConfig, account: &AccountConfig, yes: bool) -> anyhow::Result<Value>`; result `{"mode": "managed", "dir", "action": "installed"|"current", "tag", "checkpoint"}`; errors start with `kev::install::STEP` = `"step 5 (Kev)"`
  - `setup::SetupArgs { kev_url: Option<String>, yes: bool, .. }`
  - `setup::LOCAL_KEV_LABEL = "Local Kev on this Mac (downloads about 9 GB, no API key)"`
  - setup's result gains `setup.kev` for a Kev classifier: the install result, or `{"mode": "server", "endpoint", "checked": bool}`
  - `tests/kev_support::{git_script(control: &Path) -> String, brew_script(control: &Path) -> String}`

- [ ] **Step 1: Add the stand-in `git` and `brew`**

Append to `tests/kev_support/mod.rs`:

```rust
/// The stand-in `git`: calls go to `control/git.log`. `clone ... --branch
/// TAG URL DEST` makes DEST with the tag in `.standin-tag` (exit 128 with
/// `control/git-clone-fails`); `-C DIR describe --tags --exact-match`
/// prints it.
pub fn git_script(control: &Path) -> String {
    format!(
        r#"#!/bin/sh
control='{control}'
echo "$*" >> "$control/git.log"
case "$1" in
  clone)
    if [ -f "$control/git-clone-fails" ]; then exit 128; fi
    branch=; dest=; prev=
    for a in "$@"; do
      if [ "$prev" = --branch ]; then branch="$a"; fi
      prev="$a"; dest="$a"
    done
    mkdir -p "$dest" && printf '%s\n' "$branch" > "$dest/.standin-tag" ;;
  -C)
    if [ "$3" = describe ] && [ -f "$2/.standin-tag" ]; then cat "$2/.standin-tag"; else exit 128; fi ;;
  *) exit 64 ;;
esac
"#,
        control = control.display()
    )
}

/// The stand-in `brew`: `install uv` copies `uv.standin`, next to the
/// script, to `uv`. Calls go to `control/brew.log`.
pub fn brew_script(control: &Path) -> String {
    format!(
        r#"#!/bin/sh
here="$(dirname "$0")"
echo "$*" >> '{control}/brew.log'
[ "$1 $2" = "install uv" ] || exit 64
cp "$here/uv.standin" "$here/uv" && chmod 755 "$here/uv"
"#,
        control = control.display()
    )
}
```

- [ ] **Step 2: Write the failing setup tests**

In `tests/setup.rs`, add `mod kev_support;` after `mod common;`, add `use mailtriage::kev::{runtime, Layout};`, and add:

```rust
/// The stand-in Kev server, run by the stand-in `uv` (see `kev_support`).
#[test]
#[ignore = "the stand-in Kev server; the stand-in uv runs it"]
fn kev_standin_server() {
    kev_support::serve_from_env();
}

/// Kev's side of a setup fixture: stand-in `git` and `uv` in the fixture's
/// `bin`, their logs and control files in `control`, Kev's files under the
/// fixture's HOME, and this machine as `host` describes it
/// (`MAILTRIAGE_TEST_KEV`). Dropping it stops any supervisor.
struct KevHost {
    control: PathBuf,
    data: PathBuf,
    port: u16,
    host: Value,
}

impl KevHost {
    fn dir(&self) -> PathBuf {
        self.data.join("mailtriage/kev")
    }

    fn log(&self, name: &str) -> String {
        fs::read_to_string(self.control.join(name)).unwrap_or_default()
    }

    fn lines(&self, name: &str, prefix: &str) -> Vec<String> {
        self.log(name)
            .lines()
            .filter(|l| l.starts_with(prefix))
            .map(str::to_owned)
            .collect()
    }

    fn env(&self) -> Vec<(String, String)> {
        vec![
            ("MAILTRIAGE_TEST_KEV".into(), self.host.to_string()),
            ("XDG_DATA_HOME".into(), self.data.display().to_string()),
        ]
    }
}

impl Drop for KevHost {
    fn drop(&mut self) {
        runtime::stop(&Layout::new(self.dir()), runtime::STOP_WAIT);
    }
}

impl Fixture {
    /// An Apple Silicon Mac with room and memory, as setup sees it.
    fn add_kev(&self) -> KevHost {
        let control = self.bin.join("kev-control");
        fs::create_dir_all(&control).unwrap();
        write_tool(&self.bin, "git", &kev_support::git_script(&control));
        let exe = std::env::current_exe().unwrap();
        write_tool(&self.bin, "uv", &kev_support::uv_script(&control, &exe));
        let port = kev_support::free_port();
        KevHost {
            control,
            data: self.home.join(".local/share"),
            port,
            host: json!({"apple_silicon": true, "free_gb": 100, "memory_gb": 48,
                         "port": port, "start_seconds": 30, "download_seconds": 60}),
        }
    }

    fn run_kev(&self, kev: &KevHost, args: &[&str]) -> (Output, Value) {
        let env = kev.env();
        let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        self.run_with(args, "", &env)
    }
}

const LOCAL_KEV: [&str; 7] = [
    "setup",
    "--yes",
    "--json",
    "--himalaya-account",
    "work",
    "--provider",
    "kev",
];
```

Replace `setup_offers_exactly_the_registered_kinds` with:

```rust
/// Provider adapter and Kev specs: `--provider` offers exactly
/// `provider::KINDS`, and the menu lists them in that order, with local
/// Kev only on an Apple Silicon Mac.
#[test]
fn setup_offers_exactly_the_registered_kinds() {
    let f = Fixture::new();
    let (out, v) = f.run(&[&WORK_ENV[..], &["--provider", "anthropic"]].concat(), "");
    assert_eq!(out.status.code(), Some(2));
    assert!(
        message(&v).contains(&format!(
            "[possible values: {}]",
            mailtriage::provider::KINDS.join(", ")
        )),
        "{v}"
    );
    let label = |kind: &str| mailtriage::provider::provider_for(kind).unwrap().label();
    for apple_silicon in [true, false] {
        let f = Fixture::new();
        let host = json!({"apple_silicon": apple_silicon}).to_string();
        // Enter for the Himalaya account, name, identity, time zone, brief,
        // folders, classifier, model, key variable and filing.
        let (out, v) = f.run_with(
            &["setup", "--interactive", "--json", "--key-store", "env"],
            &"\n".repeat(10),
            &[("MAILTRIAGE_TEST_KEV", host.as_str())],
        );
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        assert_eq!(v["setup"]["provider"], mailtriage::provider::KINDS[0]);
        let err = stderr(&out);
        let menu = err
            .split("Which classifier?\n")
            .nth(1)
            .and_then(|rest| rest.split("Choose").next())
            .unwrap_or_else(|| panic!("{err}"));
        let mut labels = vec![label("openrouter")];
        if apple_silicon {
            labels.push(mailtriage::setup::LOCAL_KEV_LABEL);
        }
        labels.extend([label("kev"), label("fake")]);
        let expected: String = labels
            .iter()
            .enumerate()
            .map(|(i, l)| format!("  {}) {l}{}\n", i + 1, if i == 0 { " (default)" } else { "" }))
            .collect();
        assert_eq!(menu, expected);
    }
}
```

Append:

```rust
/// Kev spec, "Managed install": each step runs once, setup stops the server
/// it started and writes `installed.json` before the config; classifying
/// later starts Kev offline, and setup again reuses everything.
#[test]
fn local_kev_is_installed_checked_and_written() {
    let f = Fixture::new();
    let kev = f.add_kev();
    let (out, v) = f.run_kev(&kev, &LOCAL_KEV);
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    let s = &v["setup"];
    assert_eq!(s["provider"], "kev");
    assert_eq!(s["model"], "kev-latest");
    assert_eq!(s["key_source"], Value::Null);
    assert_eq!(s["key_store"], Value::Null);
    assert_eq!(s["kev"]["action"], "installed", "{s}");
    assert_eq!(s["kev"]["checkpoint"], "jaredpalmer/kev-4b@v1.0");
    assert_eq!(s["doctor"]["ready"], true, "{}", s["doctor"]);
    assert_eq!(
        f.config()["provider"],
        json!({
            "kind": "kev", "model": "kev-latest",
            "endpoint": format!("http://127.0.0.1:{}/v1/systemone", kev.port),
            "api_key_env": "", "timeout_seconds": 60,
            "managed": {"checkpoint": "jaredpalmer/kev-4b@v1.0"}
        })
    );
    let dir = kev.dir();
    let installed: Value =
        serde_json::from_slice(&fs::read(dir.join("installed.json")).unwrap()).unwrap();
    assert_eq!(installed["tag"], "kev-1.0");
    assert_eq!(installed["checkpoint"], "jaredpalmer/kev-4b@v1.0");
    assert_eq!(installed["uv"], f.bin.join("uv").to_str().unwrap());
    assert_eq!(
        kev.lines("git.log", "clone "),
        vec![format!(
            "clone --depth 1 --branch kev-1.0 https://github.com/jaredpalmer/kev.git {}",
            dir.join("src.partial").display()
        )]
    );
    let src = fs::canonicalize(dir.join("src")).unwrap();
    let sync = kev.lines("uv.log", "sync ");
    assert_eq!(sync.len(), 1, "{}", kev.log("uv.log"));
    assert!(
        sync[0].starts_with(&format!(
            "sync --frozen --extra serve|cwd={}|HF_HOME={}|",
            src.display(),
            dir.join("hf").display()
        )),
        "{}",
        sync[0]
    );
    assert!(
        sync[0].contains(&format!("|UV_CACHE_DIR={}|", dir.join("uv-cache").display())),
        "{}",
        sync[0]
    );
    let runs = kev.lines("uv.log", "run ");
    assert_eq!(runs.len(), 1);
    assert!(
        runs[0].starts_with(&format!(
            "run --frozen --extra serve python -m kev.serve --run jaredpalmer/kev-4b@v1.0 --host 127.0.0.1 --port {}|",
            kev.port
        )),
        "{}",
        runs[0]
    );
    // Setup's start may download the weights; later starts may not.
    assert!(runs[0].contains("|HF_HUB_OFFLINE=unset|"), "{}", runs[0]);
    assert!(kev.log("requests.log").contains("POST /v1/systemone auth=no"));
    assert!(
        !runtime::supervisor_running(&Layout::new(dir.clone())),
        "setup stops the server it started"
    );
    let receipt = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/receipt.eml");
    let (out, v) = f.run_kev(
        &kev,
        &["classify", "--account", "work", "--input", receipt, "--format", "rfc822", "--json"],
    );
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    assert_eq!(v["outcome"], "classified", "{v}");
    let runs = kev.lines("uv.log", "run ");
    assert_eq!(runs.len(), 2);
    assert!(runs[1].contains("|HF_HUB_OFFLINE=1|"), "{}", runs[1]);
    let (out, v) = f.run_kev(
        &kev,
        &["setup", "--update", "--yes", "--json", "--account", "work", "--provider", "kev"],
    );
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    assert_eq!(v["setup"]["kev"]["action"], "current");
    assert_eq!(kev.lines("git.log", "clone ").len(), 1);
}

#[test]
fn a_failed_install_step_writes_nothing_and_setup_again_resumes() {
    let f = Fixture::new();
    let kev = f.add_kev();
    fs::write(kev.control.join("uv-sync-fails"), "").unwrap();
    let (out, v) = f.run_kev(&kev, &LOCAL_KEV);
    assert_eq!(out.status.code(), Some(3), "{v}");
    assert!(message(&v).starts_with("step 5 (Kev): uv sync failed in "), "{v}");
    assert!(message(&v).contains("`mailtriage setup`"), "{v}");
    assert!(!f.config_path().exists());
    assert!(kev.dir().join("src").is_dir());
    assert!(!kev.dir().join("installed.json").exists());
    fs::remove_file(kev.control.join("uv-sync-fails")).unwrap();
    let (out, v) = f.run_kev(&kev, &LOCAL_KEV);
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    assert!(stderr(&out).contains("Reusing Kev kev-1.0 in "), "{}", stderr(&out));
    assert_eq!(kev.lines("git.log", "clone ").len(), 1);
    assert_eq!(kev.lines("uv.log", "sync ").len(), 2);
}

#[test]
fn a_missing_uv_comes_from_homebrew_or_its_installer() {
    let f = Fixture::new();
    let kev = f.add_kev();
    fs::rename(f.bin.join("uv"), f.bin.join("uv.standin")).unwrap();
    let (out, v) = f.run_kev(&kev, &LOCAL_KEV);
    assert_eq!(out.status.code(), Some(3), "{v}");
    assert!(
        message(&v).starts_with(
            "step 5 (Kev): uv is not on PATH; install it with `curl -LsSf https://astral.sh/uv/install.sh | sh`"
        ),
        "{v}"
    );
    assert!(kev.lines("git.log", "clone ").is_empty());
    write_tool(&f.bin, "brew", &kev_support::brew_script(&kev.control));
    let (out, v) = f.run_kev(&kev, &LOCAL_KEV);
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    assert_eq!(kev.log("brew.log"), "install uv\n");
    assert!(stderr(&out).contains("Installing uv with Homebrew."));
}

#[test]
fn local_kev_needs_an_apple_silicon_mac_room_and_a_yes() {
    let f = Fixture::new();
    let mut kev = f.add_kev();
    kev.host["apple_silicon"] = json!(false);
    let (out, v) = f.run_kev(&kev, &LOCAL_KEV);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        message(&v),
        "step 5 (classifier): local Kev needs an Apple Silicon Mac; run your own Kev server and pass --kev-url"
    );
    kev.host["apple_silicon"] = json!(true);
    kev.host["free_gb"] = json!(10);
    let (out, v) = f.run_kev(&kev, &LOCAL_KEV);
    assert_eq!(out.status.code(), Some(3));
    assert!(
        message(&v).contains(" has 10 GB free and local Kev needs 15 GB;"),
        "{v}"
    );
    kev.host["free_gb"] = json!(100);
    let (out, v) = f.run_kev(
        &kev,
        &["setup", "--json", "--himalaya-account", "work", "--provider", "kev"],
    );
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        message(&v),
        "step 5 (Kev): local Kev downloads about 9 GB; pass --yes to agree, or choose another --provider"
    );
    assert!(kev.log("git.log").is_empty());
    assert!(!f.config_path().exists());
    kev.host["memory_gb"] = json!(16);
    let (out, v) = f.run_kev(&kev, &LOCAL_KEV);
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    assert!(
        stderr(&out).contains("This Mac has 16 GB of memory."),
        "{}",
        stderr(&out)
    );
}

/// Review focus 4: a local Kev whose checkpoint was edited by hand is
/// installed with that checkpoint when setup keeps the classifier.
#[test]
fn a_kept_local_kev_is_installed_with_its_own_checkpoint() {
    let f = Fixture::new();
    let kev = f.add_kev();
    assert_eq!(f.run(&WORK_ENV, "").0.status.code(), Some(0));
    let mut c = f.config();
    c["provider"] = json!({
        "kind": "kev", "model": "kev-latest",
        "endpoint": format!("http://127.0.0.1:{}/v1/systemone", kev.port),
        "timeout_seconds": 60, "managed": {"checkpoint": "test/kev-9b@v1.0"}
    });
    fs::write(f.config_path(), serde_json::to_vec_pretty(&c).unwrap()).unwrap();
    let (out, v) = f.run_kev(&kev, &["setup", "--update", "--yes", "--json", "--account", "work"]);
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    assert_eq!(v["setup"]["kev"]["action"], "installed", "{v}");
    assert_eq!(v["setup"]["kev"]["checkpoint"], "test/kev-9b@v1.0");
    let runs = kev.lines("uv.log", "run ");
    assert!(runs[0].contains("--run test/kev-9b@v1.0 "), "{}", runs[0]);
    assert_eq!(
        f.config()["provider"]["managed"]["checkpoint"],
        "test/kev-9b@v1.0"
    );
}

#[test]
fn a_kev_server_on_this_machine_needs_no_key() {
    let f = Fixture::new();
    let stub = kev_support::Stub::start(kev_support::models_for("jaredpalmer/kev-4b@v1.0"), None);
    let url = stub.endpoint();
    let base = ["setup", "--yes", "--json", "--himalaya-account", "work"];
    let refused = [
        (
            vec!["--provider", "openrouter", "--kev-url", url.as_str()],
            "step 5 (classifier): --kev-url needs --provider kev; drop it or pass --provider kev",
        ),
        (
            vec!["--kev-url", url.as_str(), "--key-store", "env"],
            "step 5 (key): --key-store has no effect with a Kev server on this machine; drop it",
        ),
        (
            vec!["--kev-url", "http://kev.example.test/v1/systemone"],
            "step 5 (classifier): --kev-url: provider.endpoint must use https for a server off this machine",
        ),
    ];
    for (extra, expected) in refused {
        let (out, v) = f.run(&[&base[..], &extra].concat(), "");
        assert_eq!(out.status.code(), Some(2), "{extra:?}: {v}");
        assert_eq!(message(&v), expected);
    }
    assert!(!f.config_path().exists());
    let (out, v) = f.run(&[&base[..], &["--kev-url", url.as_str()]].concat(), "");
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    let s = &v["setup"];
    assert_eq!(s["provider"], "kev");
    assert_eq!(s["key_source"], Value::Null);
    assert_eq!(s["kev"], json!({"mode": "server", "endpoint": url, "checked": true}));
    assert_eq!(
        f.config()["provider"],
        json!({"kind": "kev", "model": "kev-latest", "endpoint": url, "api_key_env": "", "timeout_seconds": 60})
    );
    assert!(stub.requests().contains(&"GET /v1/models auth=no".to_owned()));
    let other = kev_support::Stub::start(json!({"models": [{"name": "jev-latest"}]}), None);
    let (out, v) = f.run(
        &["setup", "--update", "--yes", "--json", "--account", "work", "--kev-url", &other.endpoint()],
        "",
    );
    assert_eq!(out.status.code(), Some(3), "{v}");
    assert_eq!(
        message(&v),
        format!(
            "step 5 (classifier): the Kev server at http://127.0.0.1:{}/v1/models does not serve kev-latest; check --kev-url and --model",
            other.port
        )
    );
}

/// Review focus 5: a remote server's key goes to a key store under account
/// `kev`, is used to check the server, and never reaches the output.
#[test]
fn a_remote_kev_server_uses_a_key_store_under_account_kev() {
    let f = Fixture::new();
    fs::write(f.bin.join("pass.key"), "sk-kev-secret\n").unwrap();
    let (out, v) = f.run(
        &[
            "setup", "--yes", "--json", "--himalaya-account", "work",
            "--kev-url", "https://kev.invalid/v1/systemone",
            "--key-store", "pass", "--key-stored",
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(3), "{v}");
    assert_eq!(
        message(&v),
        "step 5 (classifier): the Kev server did not answer at https://kev.invalid/v1/models; start it or check --kev-url"
    );
    assert!(f.tool_calls().contains("show mailtriage/kev"), "{}", f.tool_calls());
    for text in [stdout(&out), stderr(&out)] {
        assert!(!text.contains("sk-kev-secret"), "{text}");
    }
    assert!(!f.config_path().exists());
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --locked --test setup kev 2>&1 | tail -20`
Expected: compile errors: `cannot find function git_script`, `LOCAL_KEV_LABEL not found in setup`.

- [ ] **Step 4: Write the managed install**

Add `pub mod install;` to `src/kev/mod.rs` (before `pub mod runtime;`). Create `src/kev/install.rs`:

```rust
//! Setup's managed install (Kev spec, "Managed install"). A step that is
//! already done is reused, so running setup again after a failure resumes.
//! It writes only below `<data dir>/mailtriage/kev`; `installed.json`
//! comes last, and setup writes the config after it.
use super::{
    runtime::{self, Probe, Runtime, STOP_WAIT},
    test_kev, Installed, Layout, REPO, TAG,
};
use crate::{
    domain::{AccountConfig, Address, NormalizedMessage, ProviderConfig},
    process,
    prompt::Prompter,
    provider::{self, DecisionRequest},
    service::err,
};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    ffi::OsStr,
    fs,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// Every install error starts with this step.
pub const STEP: &str = "step 5 (Kev)";
const MIN_FREE_GB: u64 = 15;
const WARN_MEMORY_GB: u64 = 32;
/// After this, a supervisor that no longer holds its lock has exited.
const START_GRACE: Duration = Duration::from_secs(5);
const UV_INSTALLER: &str = "curl -LsSf https://astral.sh/uv/install.sh | sh";

/// Installs local Kev for `provider` (a managed `kev` block) and checks it
/// with one classification of a sample message with `account`'s
/// questions. `yes` agrees to the download without prompts.
pub fn install(
    p: &mut Prompter,
    provider: &ProviderConfig,
    account: &AccountConfig,
    yes: bool,
) -> Result<Value> {
    let rt = Runtime::for_config(provider).map_err(|e| {
        err(
            2,
            format!("{STEP}: {e}; check the provider block, then run `mailtriage setup` again"),
        )
    })?;
    let layout = &rt.layout;
    let dir = layout.root().display().to_string();
    let report = |action: &str| {
        json!({"mode": "managed", "dir": layout.root(), "action": action,
               "tag": TAG, "checkpoint": rt.checkpoint})
    };
    if Installed::read(layout).is_some_and(|i| i.matches(&rt.checkpoint)) {
        p.say(&format!("Local Kev is installed in {dir}."));
        return Ok(report("current"));
    }
    // 1. Room and memory.
    if let Some(free) = free_gb(layout.root()).filter(|&gb| gb < MIN_FREE_GB) {
        return Err(err(
            3,
            format!("{STEP}: {dir} has {free} GB free and local Kev needs {MIN_FREE_GB} GB; free some space, then run `mailtriage setup` again"),
        ));
    }
    if let Some(memory) = memory_gb().filter(|&gb| gb < WARN_MEMORY_GB) {
        p.say(&format!("This Mac has {memory} GB of memory. Local Kev uses about 12 GB while it classifies, so other programs may slow down."));
    }
    // 2. Agree to the download.
    let question = format!("Local Kev downloads about 9 GB to {dir} and uses about 12 GB of memory while it classifies. Continue?");
    let agreed = if p.enabled() {
        p.confirm(&question, true)?
    } else if yes {
        true
    } else {
        return Err(err(
            2,
            format!("{STEP}: local Kev downloads about 9 GB; pass --yes to agree, or choose another --provider"),
        ));
    };
    if !agreed {
        return Err(err(
            2,
            format!("{STEP}: local Kev was not installed and nothing was written; run `mailtriage setup` again to choose another classifier"),
        ));
    }
    // 3. Tools.
    let git = process::find_on_path("git").ok_or_else(|| {
        err(
            3,
            format!("{STEP}: git is not on PATH; install the Xcode command line tools with `xcode-select --install`, then run `mailtriage setup` again"),
        )
    })?;
    let uv = match process::find_on_path("uv") {
        Some(uv) => uv,
        None => install_uv(p, yes)?,
    };
    // 4. Code.
    fs::create_dir_all(layout.root()).map_err(|e| {
        err(
            3,
            format!("{STEP}: cannot create {dir} ({e}); check its permissions, then run `mailtriage setup` again"),
        )
    })?;
    checkout(p, &git, layout)?;
    // 5. Python environment (uv fetches the Python version Kev pins).
    p.say("Setting up Kev's Python environment with uv.");
    let mut sync = Command::new(&uv);
    sync.args(["sync", "--frozen", "--extra", "serve"])
        .current_dir(layout.src())
        .envs(layout.uv_env());
    if !visible(&mut sync) {
        return Err(err(
            3,
            format!(
                "{STEP}: uv sync failed in {}; see the output above, then run `mailtriage setup` again",
                layout.src().display()
            ),
        ));
    }
    // 6. First start, which downloads the weights; stopped however it ends.
    let started = Started(layout);
    start(p, &rt, &uv)?;
    // 7. One classification.
    let checked = check(provider, account);
    // 8. Stop, record.
    drop(started);
    checked.map_err(|e| {
        err(
            3,
            format!(
                "{STEP}: the test classification failed ({e}); see {}, then run `mailtriage setup` again",
                layout.log().display()
            ),
        )
    })?;
    Installed {
        tag: TAG.to_owned(),
        checkpoint: rt.checkpoint.clone(),
        installed_at: chrono::Utc::now().to_rfc3339(),
        uv,
    }
    .write(layout)
    .map_err(|e| {
        err(
            3,
            format!(
                "{STEP}: cannot write {} ({e}); check its permissions, then run `mailtriage setup` again",
                layout.installed().display()
            ),
        )
    })?;
    p.say(&format!("Local Kev is installed in {dir}."));
    Ok(report("installed"))
}

/// Stops the server setup started when it goes out of scope.
struct Started<'a>(&'a Layout);

impl Drop for Started<'_> {
    fn drop(&mut self) {
        runtime::stop(self.0, STOP_WAIT);
    }
}

/// Runs `command` with its output on stderr (stdout carries setup's
/// result); `true` when it exits 0.
fn visible(command: &mut Command) -> bool {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(io::stderr()))
        .status()
        .is_ok_and(|status| status.success())
}

/// Free space for `dir`, in GB: the test value, else `df -Pk` on its
/// nearest existing ancestor; `None` when it cannot be told.
fn free_gb(dir: &Path) -> Option<u64> {
    if let Some(gb) = test_kev().free_gb {
        return Some(gb);
    }
    let existing = dir.ancestors().find(|p| p.exists())?;
    let out = process::run_bounded(
        Path::new("/bin/df"),
        &[OsStr::new("-Pk"), existing.as_os_str()],
        Duration::from_secs(10),
        64 * 1024,
    )
    .ok()?
    .success()?;
    let text = String::from_utf8(out).ok()?;
    let kb: u64 = text.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok()?;
    Some(kb / (1024 * 1024))
}

/// This Mac's memory in GB: the test value, else `sysctl -n hw.memsize`.
fn memory_gb() -> Option<u64> {
    if let Some(gb) = test_kev().memory_gb {
        return Some(gb);
    }
    let out = process::run_bounded(
        Path::new("/usr/sbin/sysctl"),
        &["-n", "hw.memsize"],
        Duration::from_secs(10),
        4096,
    )
    .ok()?
    .success()?;
    let bytes: u64 = String::from_utf8(out).ok()?.trim().parse().ok()?;
    Some(bytes / (1024 * 1024 * 1024))
}

/// Step 3 for a missing uv: `brew install uv` when Homebrew is there and
/// you agree (`--yes` agrees), else the installer command.
fn install_uv(p: &mut Prompter, yes: bool) -> Result<PathBuf> {
    let Some(brew) = process::find_on_path("brew") else {
        return Err(err(
            3,
            format!("{STEP}: uv is not on PATH; install it with `{UV_INSTALLER}`, then run `mailtriage setup` again"),
        ));
    };
    let wanted = if p.enabled() {
        p.confirm("uv is not on PATH. Install it with Homebrew (brew install uv)?", true)?
    } else {
        yes
    };
    if !wanted {
        return Err(err(
            3,
            format!("{STEP}: uv is not on PATH; install it with `brew install uv`, then run `mailtriage setup` again"),
        ));
    }
    p.say("Installing uv with Homebrew.");
    if !visible(Command::new(&brew).args(["install", "uv"])) {
        return Err(err(
            3,
            format!("{STEP}: `brew install uv` failed; see the output above, then run `mailtriage setup` again"),
        ));
    }
    process::find_on_path("uv").ok_or_else(|| {
        err(
            3,
            format!("{STEP}: uv is still not on PATH after `brew install uv`; open a new terminal, then run `mailtriage setup` again"),
        )
    })
}

/// Step 4: `src` at `TAG`, reused when it is there; otherwise cloned into
/// `src.partial` and renamed, so a broken clone never passes for one.
fn checkout(p: &mut Prompter, git: &Path, layout: &Layout) -> Result<()> {
    let src = layout.src();
    if src.exists() {
        if tag_of(git, &src).as_deref() == Some(TAG) {
            p.say(&format!("Reusing Kev {TAG} in {}.", src.display()));
            return Ok(());
        }
        fs::remove_dir_all(&src).map_err(|e| {
            err(
                3,
                format!("{STEP}: cannot replace {} ({e}); remove it, then run `mailtriage setup` again", src.display()),
            )
        })?;
    }
    let partial = layout.partial();
    if partial.exists() {
        let _ = fs::remove_dir_all(&partial);
    }
    p.say(&format!("Downloading Kev {TAG}."));
    let mut clone = Command::new(git);
    clone
        .args(["clone", "--depth", "1", "--branch", TAG, REPO])
        .arg(&partial)
        .env("GIT_TERMINAL_PROMPT", "0");
    if !visible(&mut clone) {
        return Err(err(
            3,
            format!("{STEP}: `git clone` of {REPO} failed; check your network, then run `mailtriage setup` again"),
        ));
    }
    fs::rename(&partial, &src).map_err(|e| {
        err(
            3,
            format!("{STEP}: cannot move {} to {} ({e}); then run `mailtriage setup` again", partial.display(), src.display()),
        )
    })
}

/// The tag `src` is checked out at, if exactly one.
fn tag_of(git: &Path, src: &Path) -> Option<String> {
    let args = [
        OsStr::new("-C"),
        src.as_os_str(),
        OsStr::new("describe"),
        OsStr::new("--tags"),
        OsStr::new("--exact-match"),
    ];
    let out = process::run_bounded(git, &args, Duration::from_secs(10), 4096)
        .ok()?
        .success()?;
    Some(String::from_utf8_lossy(&out).trim().to_owned())
}

/// Step 6: the supervisor with downloads allowed. Its log is shown while
/// the weights download, for up to `download_timeout()`.
fn start(p: &mut Prompter, rt: &Runtime, uv: &Path) -> Result<()> {
    let log = rt.layout.log();
    let failed = |what: String| {
        err(
            3,
            format!("{STEP}: {what}; see {}, then run `mailtriage setup` again", log.display()),
        )
    };
    let mut shown = fs::metadata(&log).map_or(0, |m| m.len());
    runtime::spawn_supervisor(rt, uv, true).map_err(|e| failed(format!("cannot start Kev ({e})")))?;
    p.say("Starting Kev. The first start downloads the model (about 9 GB), which can take a while.");
    let began = Instant::now();
    let wait = super::download_timeout();
    loop {
        shown = show_log(p, &log, shown);
        match runtime::probe(rt) {
            Probe::Ready => return Ok(()),
            Probe::Other => {
                return Err(err(
                    3,
                    format!("{STEP}: port {} is used by another program; stop it, then run `mailtriage setup` again", rt.port),
                ))
            }
            Probe::Nothing => {}
        }
        if began.elapsed() > START_GRACE && !runtime::supervisor_running(&rt.layout) {
            show_log(p, &log, shown);
            return Err(failed("Kev stopped while it started".to_owned()));
        }
        if began.elapsed() >= wait {
            let wait = match wait.as_secs() {
                s if s >= 60 => format!("{} minutes", s / 60),
                s => format!("{s} seconds"),
            };
            return Err(failed(format!("Kev did not answer within {wait}")));
        }
        thread::sleep(Duration::from_secs(1));
    }
}

/// Shows the complete lines `log` gained after byte `shown`, indented and
/// cut to 160 characters; returns the new offset.
fn show_log(p: &mut Prompter, log: &Path, shown: u64) -> u64 {
    let Ok(mut file) = fs::File::open(log) else {
        return shown;
    };
    let mut bytes = Vec::new();
    if file
        .seek(SeekFrom::Start(shown))
        .and_then(|_| file.read_to_end(&mut bytes))
        .is_err()
    {
        return shown;
    }
    let Some(end) = bytes.iter().rposition(|&b| b == b'\n') else {
        return shown;
    };
    for line in String::from_utf8_lossy(&bytes[..end]).split(['\n', '\r']) {
        let line = line.trim();
        if !line.is_empty() {
            p.say(&format!("  {}", line.chars().take(160).collect::<String>()));
        }
    }
    shown + end as u64 + 1
}

/// Step 7: one classification of a fixed message, straight to the running
/// server: `installed.json` is not written yet, so the managed path would
/// refuse. It must pass the contract check.
fn check(provider: &ProviderConfig, account: &AccountConfig) -> Result<()> {
    let mut direct = provider.clone();
    direct.managed = None;
    let request = DecisionRequest::new(account, &sample(), chrono::Utc::now().to_rfc3339());
    let decision = provider::provider_for("kev")?.decide(&direct, &request, None)?;
    provider::check_decision(&decision, &request.questions)
}

fn sample() -> NormalizedMessage {
    NormalizedMessage {
        raw_sha256: "kev-setup-check".into(),
        from: vec![Address {
            email: "alex@example.invalid".into(),
            name: Some("Alex".into()),
        }],
        to: vec![],
        cc: vec![],
        subject: "Lunch on Friday?".into(),
        sent_at: None,
        body: "Hi, are you free for lunch on Friday at noon? Let me know by Thursday.".into(),
        incomplete: false,
        warnings: vec![],
    }
}
```

- [ ] **Step 5: Setup's flags and result**

In `src/setup.rs`, add to `SetupArgs` after `pub model: Option<String>,`:

```rust
    /// `--kev-url`: a Kev server you run; implies `--provider kev`.
    pub kev_url: Option<String>,
    /// `--yes`: agrees to local Kev's download without prompts.
    pub yes: bool,
```

In `src/cli.rs`, add to `SetupArg` after `model`:

```rust
    /// The URL of a Kev server you run, ending in /v1/systemone (implies --provider kev).
    #[arg(long, value_name = "URL")]
    kev_url: Option<String>,
```

and in `to_args` add `kev_url: self.kev_url.clone(),` and `yes: self.yes,`.

In `setup::run`, replace

```rust
    // 5. Classifier.
    let current = (!matches!(intent, Intent::Create)).then(|| cfg.provider.clone());
    let (provider, store) = classifier_step(args, p, current.as_ref())?;
    // 6. Categories: a new account gets the defaults, shown once the
    // config is written.
    let categories = match &previous {
        Some(a) => a.categories.clone(),
        None => default_categories(),
    };
```

with

```rust
    // 6. Categories: a new account gets the defaults, shown once the
    // config is written. Chosen before step 5, whose local Kev check
    // classifies a sample message with them.
    let categories = match &previous {
        Some(a) => a.categories.clone(),
        None => default_categories(),
    };
    // 5. Classifier.
    let current = (!matches!(intent, Intent::Create)).then(|| cfg.provider.clone());
    let probe = AccountConfig {
        identity: identity.clone(),
        timezone: timezone.clone(),
        brief: brief.clone(),
        taxonomy_revision: 1,
        categories: categories.clone(),
        himalaya: None,
        engine: None,
        filing: FilingConfig::default(),
    };
    let Classifier {
        provider,
        store,
        kev,
    } = classifier_step(args, p, current.as_ref(), &probe)?;
```

and replace the final `Ok(json!({"schema_version": 1, "setup": { ... }}))` with the same object bound to `let mut out = json!(...);`, followed by:

```rust
    if let Some(kev) = kev {
        out["setup"]["kev"] = kev;
    }
    Ok(out)
```

- [ ] **Step 6: The classifier step**

Replace `classifier_step` with the following, and add the helpers after it:

```rust
/// What step 5 chose: the provider block, the key store chosen in this
/// run, and for Kev what setup found (`setup.kev`).
struct Classifier {
    provider: ProviderConfig,
    store: Option<KeyStore>,
    kev: Option<Value>,
}

/// The classifier menu's entries. Kev appears twice: run by mailtriage on
/// this Mac, or a server you run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Choice {
    OpenRouter,
    LocalKev,
    KevServer,
    Fake,
}

/// Local Kev's menu entry, offered only on an Apple Silicon Mac.
pub const LOCAL_KEV_LABEL: &str = "Local Kev on this Mac (downloads about 9 GB, no API key)";

impl Choice {
    fn label(self) -> &'static str {
        match self {
            Choice::OpenRouter => registered("openrouter").label(),
            Choice::LocalKev => LOCAL_KEV_LABEL,
            Choice::KevServer => registered("kev").label(),
            Choice::Fake => registered("fake").label(),
        }
    }
}

/// The menu, in `provider::KINDS` order.
fn menu(apple_silicon: bool) -> Vec<Choice> {
    let mut menu = vec![Choice::OpenRouter];
    if apple_silicon {
        menu.push(Choice::LocalKev);
    }
    menu.extend([Choice::KevServer, Choice::Fake]);
    menu
}

/// Step 5: the classifier; the key store when one was chosen now; local
/// Kev installed or your own Kev server checked.
fn classifier_step(
    args: &SetupArgs,
    p: &mut Prompter,
    current: Option<&ProviderConfig>,
    probe: &AccountConfig,
) -> Result<Classifier> {
    let key_flag = [
        (args.key_store.is_some(), "--key-store"),
        (args.key_command.is_some(), "--key-command"),
        (args.key_env.is_some(), "--key-env"),
        (args.key_stored, "--key-stored"),
    ]
    .into_iter()
    .find_map(|(given, flag)| given.then_some(flag));
    // `--kev-url` names a Kev server you run, so it implies `--provider kev`.
    let kind = match (args.provider.as_deref(), &args.kev_url) {
        (Some(kind), Some(_)) if kind != "kev" => {
            return Err(err(
                2,
                format!("{STEP_CLASSIFIER}: --kev-url needs --provider kev; drop it or pass --provider kev"),
            ))
        }
        (Some(kind), _) => Some(kind.to_owned()),
        (None, Some(_)) => Some("kev".to_owned()),
        (None, None) => None,
    };
    if let Some(url) = &args.kev_url {
        kev_url_rule(url)
            .map_err(|reason| err(2, format!("{STEP_CLASSIFIER}: --kev-url: {reason}")))?;
    }
    if let (Some(flag), Some(kind)) = (key_flag, kind.as_deref()) {
        if let Ok(adapter) = provider::provider_for(kind) {
            let mut candidate = adapter.new_config();
            if let Some(url) = &args.kev_url {
                candidate.endpoint = url.trim().to_owned();
                candidate.managed = None;
            }
            if adapter.key_account(&candidate).is_none() {
                return Err(err(
                    2,
                    if kind == "kev" {
                        format!("{STEP_KEY}: {flag} has no effect with a Kev server on this machine; drop it")
                    } else {
                        format!("{STEP_KEY}: {flag} has no effect with --provider {kind}; drop it")
                    },
                ));
            }
        }
    }
    let flags = kind.is_some() || args.model.is_some() || key_flag.is_some();
    if let Some(current) = current.filter(|_| !flags) {
        let keep = !p.enabled()
            || p.confirm(
                &format!("Keep the current classifier ({})?", describe(current)),
                true,
            )?;
        if keep {
            // A kept local Kev is installed when it is missing (a no-op when
            // it is there), so `setup --update` is the fix `doctor` names.
            let kev = match current.managed {
                Some(_) => Some(crate::kev::install::install(p, current, probe, args.yes)?),
                None => None,
            };
            return Ok(Classifier {
                provider: current.clone(),
                store: None,
                kev,
            });
        }
    }
    let apple_silicon = crate::kev::apple_silicon();
    let choice = match kind.as_deref() {
        Some("kev") if args.kev_url.is_some() => Choice::KevServer,
        Some("kev") if apple_silicon => Choice::LocalKev,
        Some("kev") => {
            return Err(err(
                2,
                format!("{STEP_CLASSIFIER}: local Kev needs an Apple Silicon Mac; run your own Kev server and pass --kev-url"),
            ))
        }
        Some("openrouter") => Choice::OpenRouter,
        Some("fake") => Choice::Fake,
        Some(_) => {
            return Err(err(
                2,
                format!(
                    "{STEP_CLASSIFIER}: unsupported provider kind; use --provider {}",
                    provider::KINDS.join(" or ")
                ),
            ))
        }
        None if p.enabled() => {
            let menu = menu(apple_silicon);
            let labels: Vec<String> = menu.iter().map(|c| c.label().to_owned()).collect();
            menu[p.choose("Which classifier?", &labels, 0)?]
        }
        None => Choice::OpenRouter,
    };
    match choice {
        Choice::Fake => Ok(Classifier {
            provider: registered("fake").new_config(),
            store: None,
            kev: None,
        }),
        Choice::OpenRouter => openrouter_step(args, p, current, flags, key_flag.is_some()),
        Choice::LocalKev => local_kev_step(args, p, current, probe),
        Choice::KevServer => kev_server_step(args, p, current, flags, key_flag.is_some()),
    }
}

/// OpenRouter: the model, then the key unless a current key stays.
fn openrouter_step(
    args: &SetupArgs,
    p: &mut Prompter,
    current: Option<&ProviderConfig>,
    flags: bool,
    key_flag: bool,
) -> Result<Classifier> {
    let adapter = registered("openrouter");
    let account = adapter
        .key_account(&adapter.new_config())
        .expect("OpenRouter needs a key");
    // A current provider of this kind changes only where asked, so its
    // generation hash, and with it every classification, stays put.
    let current = current.filter(|c| c.kind == "openrouter");
    let mut provider = current.cloned().unwrap_or_else(|| adapter.new_config());
    provider.model = answer(
        p,
        STEP_CLASSIFIER,
        args.model.as_deref(),
        "--model",
        "Model",
        Some(&provider.model),
        nonempty,
    )?;
    // A current key stays where it is unless a key flag moves it: `--model`
    // or `--provider` alone never switch its source.
    if flags && !key_flag && current.is_some() {
        return Ok(Classifier {
            provider,
            store: None,
            kev: None,
        });
    }
    let default_env = adapter.new_config().api_key_env;
    let current = current.map(|c| current_store(c, account));
    let store = key_step(args, p, &mut provider, current, account, &default_env)?;
    Ok(Classifier {
        provider,
        store: Some(store),
        kev: None,
    })
}

/// Local Kev: a managed block on this Mac's port, installed (or found
/// installed) before anything is written. No key; the model is
/// `kev-latest` unless `--model` says otherwise.
fn local_kev_step(
    args: &SetupArgs,
    p: &mut Prompter,
    current: Option<&ProviderConfig>,
    probe: &AccountConfig,
) -> Result<Classifier> {
    let mut provider = current
        .filter(|c| c.kind == "kev" && c.managed.is_some())
        .cloned()
        .unwrap_or_else(|| {
            let mut new = registered("kev").new_config();
            new.endpoint = provider::kev::local_endpoint(crate::kev::port());
            new
        });
    if let Some(model) = args.model.as_deref() {
        provider.model = nonempty(model)
            .map_err(|reason| err(2, format!("{STEP_CLASSIFIER}: --model: {reason}")))?;
    }
    let kev = crate::kev::install::install(p, &provider, probe, args.yes)?;
    Ok(Classifier {
        provider,
        store: None,
        kev: Some(kev),
    })
}

/// A Kev server you run: its URL, the key steps for a server off this
/// machine (account `kev`), then a check that it serves the model.
fn kev_server_step(
    args: &SetupArgs,
    p: &mut Prompter,
    current: Option<&ProviderConfig>,
    flags: bool,
    key_flag: bool,
) -> Result<Classifier> {
    let adapter = registered("kev");
    let current = current.filter(|c| c.kind == "kev" && c.managed.is_none());
    let mut provider = current.cloned().unwrap_or_else(|| {
        let mut new = adapter.new_config();
        new.managed = None;
        new.endpoint = String::new();
        new
    });
    let shown = (!provider.endpoint.is_empty()).then(|| provider.endpoint.clone());
    provider.endpoint = answer(
        p,
        STEP_CLASSIFIER,
        args.kev_url.as_deref(),
        "--kev-url",
        "Kev server URL (ends in /v1/systemone)",
        shown.as_deref(),
        kev_url_rule,
    )?;
    if let Some(model) = args.model.as_deref() {
        provider.model = nonempty(model)
            .map_err(|reason| err(2, format!("{STEP_CLASSIFIER}: --model: {reason}")))?;
    }
    let store = match adapter.key_account(&provider) {
        None => {
            provider.api_key_command = None;
            provider.api_key_env = String::new();
            None
        }
        Some(account) => {
            // A current remote server's key stays unless a key flag moves it.
            let kept = current.filter(|c| adapter.key_account(c).is_some());
            if flags && !key_flag && kept.is_some() {
                None
            } else {
                let current = kept.map(|c| current_store(c, account));
                Some(key_step(
                    args,
                    p,
                    &mut provider,
                    current,
                    account,
                    provider::kev::DEFAULT_KEY_ENV,
                )?)
            }
        }
    };
    let kev = check_kev_server(p, &provider)?;
    Ok(Classifier {
        provider,
        store,
        kev: Some(kev),
    })
}

/// A Kev server URL that passes the URL rule, trimmed.
fn kev_url_rule(url: &str) -> Result<String, String> {
    let adapter = registered("kev");
    let mut candidate = adapter.new_config();
    candidate.managed = None;
    candidate.endpoint = url.trim().to_owned();
    adapter.validate(&candidate).map_err(|e| e.to_string())?;
    Ok(candidate.endpoint)
}

/// `GET /v1/models` with the key setup can resolve here: the server must
/// serve the model. A server that wants a key not available here (an
/// environment variable set only where mailtriage runs) passes with a note.
fn check_kev_server(p: &mut Prompter, provider: &ProviderConfig) -> Result<Value> {
    let url = provider::kev::models_url(&provider.endpoint)
        .expect("a valid Kev endpoint ends in /v1/systemone");
    let key = provider::key_account(provider).and_then(|_| secrets::resolve_key(provider).ok());
    let checked = match provider::kev::list_models(&url, key.as_deref(), Duration::from_secs(10)) {
        Ok(models) if models.iter().any(|m| m.name == provider.model) => true,
        Ok(_) => {
            return Err(err(
                3,
                format!(
                    "{STEP_CLASSIFIER}: the Kev server at {url} does not serve {}; check --kev-url and --model",
                    provider.model
                ),
            ))
        }
        Err(provider::kev::ModelsError::Http(401 | 403)) if key.is_none() => false,
        Err(e) => {
            return Err(err(
                3,
                format!("{STEP_CLASSIFIER}: {e} at {url}; start it or check --kev-url"),
            ))
        }
    };
    if checked {
        p.say(&format!(
            "The Kev server at {} serves {}.",
            provider.endpoint, provider.model
        ));
    } else {
        p.say(&format!(
            "The Kev server at {} wants a key that is not available here, so its models were not checked.",
            provider.endpoint
        ));
    }
    Ok(json!({"mode": "server", "endpoint": provider.endpoint, "checked": checked}))
}
```

Replace `describe` with:

```rust
fn describe(provider: &ProviderConfig) -> String {
    match (provider.kind.as_str(), &provider.api_key_command) {
        ("fake", _) => "offline demo".to_owned(),
        ("kev", _) if provider.managed.is_some() => format!("local Kev, {}", provider.model),
        ("kev", _) if provider::key_account(provider).is_none() => {
            format!("Kev server at {}", provider.endpoint)
        }
        ("kev", Some(_)) => format!(
            "Kev server at {}, key from a key command",
            provider.endpoint
        ),
        ("kev", None) => format!(
            "Kev server at {}, key from ${}",
            provider.endpoint, provider.api_key_env
        ),
        (_, Some(_)) => format!("{}, key from a key command", provider.model),
        (_, None) => format!("{}, key from ${}", provider.model, provider.api_key_env),
    }
}
```

Key texts per kind (OpenRouter's unchanged): give `chosen_store` a fourth parameter `owner: &str` and change its prompt to `&format!("Where should mailtriage get the {owner} key?")`. In `key_step`, add `let owner = provider::key_owner(provider);` as its first line, call `chosen_store(args, p, current, owner)?`, and change the say to `"{tool_name} will now ask for your {owner} key; mailtriage never sees it."`. In `doctor_step`, change the environment fix to:

```rust
                format!(
                    "export {}=<your {} key> where mailtriage runs",
                    provider.api_key_env,
                    provider::key_owner(provider)
                )
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test --locked --test setup`
Expected: PASS, the new Kev tests included (about a minute: each managed install starts the stand-in server). Every existing test passes unchanged except `setup_offers_exactly_the_registered_kinds`, replaced above.

- [ ] **Step 8: Run the full verification**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`
Expected: all green; afterwards `pgrep -fl kev_standin_server` prints nothing.

- [ ] **Step 9: Commit**

```bash
git add src/kev src/setup.rs src/cli.rs tests/kev_support tests/setup.rs
git commit -m "Install local Kev and connect your own Kev server from setup" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: `doctor` and `self uninstall`

**Files:**
- Modify: `src/kev/runtime.rs` (`status`)
- Modify: `src/service.rs` (`doctor`)
- Modify: `src/setup.rs` (`doctor_step`: the `kev` item)
- Modify: `src/distribution/self_uninstall.rs` (`kept`)
- Test: `tests/kev_runtime.rs`, `tests/setup.rs`, `tests/self_uninstall.rs`

**Interfaces:**
- Consumes (Task 3): `kev::runtime::{Runtime, Probe, probe, NOT_INSTALLED}`, `kev::{Installed, Layout}`; (Task 2) `provider::kev::{models_url, list_models, ModelsError}`; the `tests/kev_runtime.rs` `Fixture`.
- Produces:
  - `kev::runtime::status(config: &ProviderConfig, key: Option<&str>) -> Value`: managed `{"mode": "managed", "endpoint", "dir", "installed", "tag"?, "checkpoint"?, "installed_checkpoint"?, "running", "ready", "error"?, "fix"?}`; your own server `{"mode": "server", "endpoint", "reachable", "ready", "error"?, "fix"?}`
  - `doctor`: `provider.kev` for `kind: "kev"`; top-level `ready` includes `provider.kev.ready`; `live_checks_performed` is `true` for `kev`
  - setup's check list gains the item `kev`
  - `self uninstall` keeps `<data dir>/mailtriage/kev` and lists it in `kept`

- [ ] **Step 1: Write the failing tests**

Append to `tests/kev_runtime.rs`:

```rust
impl Fixture {
    /// `mailtriage doctor` for a config with `provider`, with this
    /// fixture's data dir.
    fn doctor(&self, provider: &ProviderConfig) -> Value {
        let path = self.root.join("doctor.json");
        let mut c = config::default_config();
        c.provider = provider.clone();
        c.state_dir = self.root.join("doctor-state");
        config::save(&path, &c).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
            .arg("--config")
            .arg(&path)
            .args(["doctor", "--account", "work", "--json"])
            .env("HOME", self.root.join("home"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap()
    }
}

/// Review focus 3: `doctor` reports local Kev and never starts it.
#[test]
fn doctor_reports_local_kev_without_starting_it() {
    let f = Fixture::new();
    let provider = f.managed_config();
    let v = f.doctor(&provider);
    let kev = &v["provider"]["kev"];
    assert_eq!(kev["mode"], "managed", "{v}");
    assert_eq!(kev["installed"], false);
    assert_eq!(kev["ready"], false);
    assert_eq!(kev["error"], "local Kev is not installed; run mailtriage setup --update");
    assert_eq!(kev["fix"], "run `mailtriage setup --update`");
    assert_eq!(v["ready"], false);
    assert_eq!(v["live_checks_performed"], true);
    f.install(CHECKPOINT);
    let v = f.doctor(&provider);
    let kev = &v["provider"]["kev"];
    assert_eq!(kev["installed"], true, "{v}");
    assert_eq!(kev["tag"], "kev-1.0");
    assert_eq!(kev["checkpoint"], CHECKPOINT);
    assert_eq!(kev["running"], false);
    assert_eq!(kev["ready"], true);
    assert_eq!(v["ready"], true);
    assert!(f.runs().is_empty(), "doctor started Kev: {}", f.read("uv.log"));
    runtime::ensure_running_with(&f.runtime()).unwrap();
    assert_eq!(f.doctor(&provider)["provider"]["kev"]["running"], true);
}

#[test]
fn doctor_reports_whether_your_kev_server_answers() {
    let f = Fixture::new();
    let server = |endpoint: String| {
        let mut p = provider::provider_for("kev").unwrap().new_config();
        p.managed = None;
        p.endpoint = endpoint;
        p
    };
    let good = Stub::start(models_for("any"), None);
    let v = f.doctor(&server(good.endpoint()));
    assert_eq!(
        v["provider"]["kev"],
        json!({"mode": "server", "endpoint": good.endpoint(), "reachable": true, "ready": true})
    );
    let other = Stub::start(json!({"models": [{"name": "jev-latest"}]}), None);
    let v = f.doctor(&server(other.endpoint()));
    assert_eq!(v["provider"]["kev"]["reachable"], true);
    assert_eq!(v["provider"]["kev"]["ready"], false);
    assert_eq!(
        v["provider"]["kev"]["error"],
        "the Kev server does not serve kev-latest"
    );
    let port = free_port();
    let v = f.doctor(&server(provider::kev::local_endpoint(port)));
    let kev = &v["provider"]["kev"];
    assert_eq!(kev["reachable"], false);
    assert_eq!(kev["ready"], false);
    assert_eq!(
        kev["error"],
        format!("the Kev server did not answer at http://127.0.0.1:{port}/v1/models")
    );
    assert_eq!(kev["fix"], "start your Kev server, or check provider.endpoint and its key");
    assert_eq!(v["ready"], false);
}
```

In `tests/setup.rs`, in `local_kev_is_installed_checked_and_written`, after `assert_eq!(s["doctor"]["ready"], true, "{}", s["doctor"]);` add:

```rust
    assert!(
        s["doctor"]["items"]
            .as_array()
            .unwrap()
            .contains(&json!({"check": "kev", "ready": true})),
        "{}",
        s["doctor"]
    );
```

Append to `tests/self_uninstall.rs`:

```rust
/// Kev spec: local Kev stays, like the private Himalaya, and the message
/// says how to delete it.
#[test]
fn local_kev_is_kept_and_named() {
    let f = Fixture::new();
    let kev = f.root.join("data/mailtriage/kev");
    fs::create_dir_all(&kev).unwrap();
    fs::write(kev.join("installed.json"), "{}").unwrap();
    let (out, v) = f.uninstall(&["--yes"], "", &[]);
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    assert!(v["self_uninstall"]["kept"]
        .as_array()
        .unwrap()
        .contains(&json!(kev)));
    assert!(kev.join("installed.json").exists());
    let err = stderr(&out);
    assert!(
        err.contains(&format!(
            "Kept local Kev and its model in {}: other configs may use it. Delete it with rm -rf {} once none does.",
            kev.display(),
            kev.display()
        )),
        "{err}"
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --locked --test kev_runtime doctor && cargo test --locked --test self_uninstall local_kev`
Expected: FAIL: `provider.kev` is `null`, and `kept` lacks the Kev directory.

- [ ] **Step 3: `status` and `doctor`**

Append to `src/kev/runtime.rs` (add `use serde_json::{json, Value};` to its imports):

```rust
fn not_ready(out: &mut Value, error: impl Into<String>, fix: impl Into<String>) {
    out["ready"] = json!(false);
    out["error"] = json!(error.into());
    out["fix"] = json!(fix.into());
}

/// `doctor`'s `provider.kev`, read-only: it never starts Kev. Managed:
/// `installed` (with `tag` and `checkpoint`) and `running`; a stopped
/// server is ready, since it starts on demand. Your own server:
/// `reachable`, from `GET /v1/models` with `key`. `ready`, and `error` and
/// `fix` when it is not.
pub fn status(config: &ProviderConfig, key: Option<&str>) -> Value {
    let managed = config.managed.is_some();
    let mut out = json!({"mode": if managed { "managed" } else { "server" }, "endpoint": config.endpoint});
    if managed {
        let rt = match Runtime::for_config(config) {
            Ok(rt) => rt,
            Err(e) => {
                not_ready(&mut out, e.0, "run `mailtriage setup --update`");
                return out;
            }
        };
        out["dir"] = json!(rt.layout.root());
        match Installed::read(&rt.layout) {
            Some(installed) if installed.matches(&rt.checkpoint) => {
                out["installed"] = json!(true);
                out["tag"] = json!(installed.tag);
                out["checkpoint"] = json!(installed.checkpoint);
            }
            other => {
                out["installed"] = json!(false);
                if let Some(installed) = other {
                    out["installed_checkpoint"] = json!(installed.checkpoint);
                }
                out["running"] = json!(false);
                not_ready(&mut out, NOT_INSTALLED, "run `mailtriage setup --update`");
                return out;
            }
        }
        let state = probe(&rt);
        match state {
            Probe::Ready | Probe::Nothing => {
                out["running"] = json!(state == Probe::Ready);
                out["ready"] = json!(true);
            }
            Probe::Other => {
                out["running"] = json!(false);
                not_ready(
                    &mut out,
                    rt.port_taken().0,
                    format!("stop the program on port {}", rt.port),
                );
            }
        }
        return out;
    }
    let url = kev::models_url(&config.endpoint).unwrap_or_default();
    match kev::list_models(&url, key, Duration::from_secs(10)) {
        Ok(models) if models.iter().any(|m| m.name == config.model) => {
            out["reachable"] = json!(true);
            out["ready"] = json!(true);
        }
        Ok(_) => {
            out["reachable"] = json!(true);
            not_ready(
                &mut out,
                format!("the Kev server does not serve {}", config.model),
                "check provider.model, or the model your Kev server runs",
            );
        }
        Err(e) => {
            out["reachable"] = json!(e != kev::ModelsError::Unreachable);
            not_ready(
                &mut out,
                format!("{e} at {url}"),
                "start your Kev server, or check provider.endpoint and its key",
            );
        }
    }
    out
}
```

In `src/service.rs`, `doctor`: after `let key_present = key_error.is_none();` add

```rust
        // Kev: installed and running (local), or answering (your own
        // server). Read-only: doctor never starts Kev.
        let kev = (provider_cfg.kind == "kev" && provider_valid).then(|| {
            let key = provider::key_account(provider_cfg)
                .and_then(|_| self.key.get(provider_cfg).ok());
            crate::kev::runtime::status(provider_cfg, key.as_deref())
        });
        let kev_ready = kev.as_ref().is_none_or(|k| k["ready"] == true);
```

In the `json!` that builds `out`, change `"ready":provider_valid&&key_present&&transport["ready"]==true` to `"ready":provider_valid&&key_present&&kev_ready&&transport["ready"]==true` and `"live_checks_performed":false` to `"live_checks_performed":kev.is_some()`; after `if let Some(e) = key_error { ... }` add

```rust
        if let Some(kev) = kev {
            out["provider"]["kev"] = kev;
        }
```

In `src/setup.rs`, `doctor_step`, after the `items.push(check_item("key", ...))` call add:

```rust
            if let Some(kev) = report["provider"].get("kev") {
                let fix = if kev["installed"] == false {
                    format!(
                        "run `{}`",
                        mailtriage_line(shown, &["setup", "--update"], &["--account", name])
                    )
                } else {
                    kev["fix"].as_str().unwrap_or_default().to_owned()
                };
                items.push(check_item(
                    "kev",
                    kev["ready"] == true,
                    kev["error"].as_str().map(str::to_owned),
                    fix,
                ));
            }
```

- [ ] **Step 4: `self uninstall` keeps local Kev**

In `src/distribution/self_uninstall.rs`, `kept`, after the private Himalaya block:

```rust
    if let Some(kev) = crate::kev::Layout::for_user()
        .map(|layout| layout.root().to_path_buf())
        .filter(|path| path.exists())
    {
        p.say(&format!(
            "Kept local Kev and its model in {}: other configs may use it. Delete it with {} once none does.",
            kev.display(),
            shell_line(&[Path::new("rm"), Path::new("-rf"), &kev])
        ));
        kept.push(kev);
    }
```

and update the doc comment: "What stays, said once: the lock file, the private Himalaya and local Kev (other configs may use them) and the config directory."

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --locked --test kev_runtime && cargo test --locked --test self_uninstall && cargo test --locked --test setup kev && cargo test --locked --test key_command`
Expected: PASS.

- [ ] **Step 6: Run the full verification**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`
Expected: all green.

- [ ] **Step 7: Commit**

```bash
git add src/kev/runtime.rs src/service.rs src/setup.rs src/distribution/self_uninstall.rs tests/kev_runtime.rs tests/setup.rs tests/self_uninstall.rs
git commit -m "Report Kev in doctor and keep local Kev on uninstall" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 6: Docs

**Files:**
- Create: `docs/guide/local-kev.md`
- Modify: `docs/guide/provider.md` (title, opening, new section "Run your own Kev server")
- Modify: `docs/.vitepress/config.mts` (sidebar)
- Modify: `docs/guide/setup.md`, `docs/guide/configuration.md`, `docs/guide/troubleshooting.md`, `docs/guide/manual-setup.md`, `docs/guide/install.md`
- Modify: `docs/reference/provider.md`, `docs/reference/setup.md`, `docs/reference/configuration.md`, `docs/reference/install.md`, `docs/reference/index.md`
- Modify: `docs/development/providers.md`

**Interfaces:**
- Consumes: the behavior of Tasks 1 to 5, exactly as built: flag names, messages, JSON fields, paths, numbers. Before writing, run `cargo run -q -- setup --help` and read `src/kev/install.rs`, `src/kev/runtime.rs` and `src/provider/kev.rs` for the exact texts; where this task's text and the code differ, the code wins and the doc follows it.
- Produces: anchors other pages link to: `guide/local-kev.md`, `guide/provider.md#run-your-own-kev-server`, `reference/provider.md#kev`, `reference/provider.md#local-kev`, `reference/provider.md#the-url-rule`.

The guide pages follow the voice rules in Global Constraints in full; `docs/development/` gets the light rules (no em dashes, "you", short paragraphs). No doc example writes a key to a file or shows one.

- [ ] **Step 1: Write the new guide page**

Create `docs/guide/local-kev.md`:

````markdown
# Run the classifier on your Mac

You want your mail sorted without sending it to an outside service and without an API key. On a Mac with Apple Silicon, mailtriage can run the classifier itself. Setup downloads it once; after that, your mail stays on your Mac while it is classified.

The classifier is Kev, an open-weight decision model. It answers the same three questions as OpenRouter's Jev, each with a choice, a confidence and a probability per label, so your categories, urgency and attention list work as before.

::: warning Important
- Local Kev needs a Mac with Apple Silicon (M1 or later). On any other machine, [run your own Kev server](./provider.md#run-your-own-kev-server) and point mailtriage at it.
- mailtriage installs Kev-4B, which is less accurate than Jev. Expect more mail marked uncertain, and check its choices for a few days before you turn on live filing.
- Kev starts when mail needs classifying and stops after 15 minutes without a request. The first message after a pause waits while the model loads, usually 10 to 30 seconds.
:::

## What you need

- **A Mac with Apple Silicon.** 32 GB of memory is comfortable. With less, setup warns you, and other programs may slow down while Kev classifies.
- **About 15 GB of free disk space.** The download is about 9 GB.
- **git.** If it is missing, install Apple's command line tools: `xcode-select --install`.
- **uv**, the Python tool Kev runs with. If it is missing and you have Homebrew, setup offers `brew install uv`. Otherwise install it with `curl -LsSf https://astral.sh/uv/install.sh | sh`.

## Set it up

Run setup and choose **Local Kev on this Mac**:

```sh
mailtriage setup
```

To switch an account you already set up:

```sh
mailtriage setup --update --account work --provider kev
```

Setup checks the free space and asks before it downloads. Then it:

1. downloads Kev 1.0 with `git`;
2. sets up Kev's Python environment with `uv`;
3. starts Kev once, which downloads the model `jaredpalmer/kev-4b@v1.0`, and shows its progress;
4. classifies a sample message to check that everything works;
5. stops Kev and saves your config.

The download can take a while. If it stops, for example because your network dropped, run the same command again. Finished steps are reused, so setup continues where it stopped.

In a script, `--yes` agrees to the download:

```sh
mailtriage setup --update --yes --account work --provider kev
```

## How it starts and stops

You never start Kev yourself. When `sync`, `watch` or the background service finds mail to classify, mailtriage starts Kev and waits until the model has loaded. After 15 minutes without a request, Kev stops and frees its memory.

Kev listens only on this Mac, on `127.0.0.1` port 8009. It needs no API key, and once setup is done it downloads nothing: it runs offline.

While it runs, Kev uses about 12 GB of memory. Kev's authors measured about 0.7 seconds per message on an Apple M5.

If Kev cannot start, classification waits. Your mail stays queued, no retry attempt is used, and the next pass tries again. The result of `sync` says why:

| Reason | What to do |
| --- | --- |
| `local Kev is not installed; run mailtriage setup --update` | Run `mailtriage setup --update --account work`. |
| `port 8009 is used by another program` | Stop that program. |
| `local Kev did not start; see .../kev.log` | Read the end of `kev.log`. |

`mailtriage doctor --account work` shows whether Kev is installed and whether it is running; see [Local Kev](../reference/provider.md#local-kev).

## Where it lives

Everything is in one folder, next to mailtriage's [private Himalaya](./himalaya.md#a-private-himalaya):

| Path | What it holds |
| --- | --- |
| `~/.local/share/mailtriage/kev/src` | Kev's code and its Python environment |
| `~/.local/share/mailtriage/kev/hf` | The model, about 9 GB |
| `~/.local/share/mailtriage/kev/python`, `uv-cache` | The Python and the packages uv downloaded for Kev |
| `~/.local/share/mailtriage/kev/kev.log` | What Kev printed, and when it started and stopped |
| `~/.local/share/mailtriage/kev/installed.json` | Which Kev and model setup installed |

If `XDG_DATA_HOME` is set, the folder is `$XDG_DATA_HOME/mailtriage/kev`.

## Remove it

`mailtriage self uninstall` keeps local Kev, because another config may use it. To remove it, switch your accounts to another classifier with `mailtriage setup --update --account work`, then delete the folder:

```sh
rm -rf ~/.local/share/mailtriage/kev
```
````

- [ ] **Step 2: Rework "API keys and models"**

In `docs/guide/provider.md`, replace the title and first paragraph

```markdown
# API keys and models

mailtriage uses OpenRouter's Decisions API to classify real mail. The default model is `typesafe/jev-latest`. The `fake` provider is for offline testing and needs no key.
```

with

```markdown
# Classifier and models

To sort your mail, mailtriage asks a classifier three questions about each message. You choose which one:

| Classifier | Good for | API key |
| --- | --- | --- |
| OpenRouter with Jev | The most accurate results. Message text goes to OpenRouter. | Needed |
| [Local Kev](./local-kev.md) | Keeping mail on an Apple Silicon Mac. | None |
| [Your own Kev server](#run-your-own-kev-server) | Keeping mail on your own hardware when your Mac can't run Kev, for example on a Linux machine with a GPU. | Only for a server on another machine |

Setup asks which one you want, and `mailtriage setup --update --account work` changes it later. The OpenRouter default model is `typesafe/jev-latest`. The `fake` provider is for offline testing and needs no key.
```

Keep every other section as it is. Append before `## Detailed reference`:

````markdown
## Run your own Kev server

You can run Kev on a machine you control, such as a Linux server with a GPU, and have mailtriage send your mail there instead of to OpenRouter. Kev's API is the same one OpenRouter uses for Jev, so nothing else changes.

On that machine, with Python 3.12 or 3.13 and [uv](https://docs.astral.sh/uv/):

```sh
git clone --branch kev-1.0 https://github.com/jaredpalmer/kev.git && cd kev
uv sync --extra serve
uv run --extra serve python -m kev.serve --run jaredpalmer/kev-4b@v1.0 --host 127.0.0.1 --port 8009
```

Kev uses a GPU (CUDA or ROCm) when there is one. `--run jaredpalmer/kev-9b@v1.0` serves the larger Kev-9B; [Kev's README](https://github.com/jaredpalmer/kev#models) says which model suits which GPU.

::: warning Important
A server on another machine needs HTTPS and a key, so your mail and the key are encrypted on the way. mailtriage refuses `http://` for every host except `127.0.0.1`, `localhost` and `[::1]`.
:::

For a server on another machine:

1. Start Kev with `KEV_API_KEY` set in its environment, for example from your service manager's secret settings. Kev then answers only requests that carry that key. `openssl rand -hex 24` makes a good key; keep it in your password manager.
2. Put an HTTPS proxy, such as Caddy or nginx, in front of it, forwarding to `127.0.0.1:8009`.
3. On your Mac, run setup with the server's URL. Setup asks where to keep the key, as it does for OpenRouter, and stores it under the name `kev`:

```sh
mailtriage setup --update --account work --provider kev --kev-url https://kev.example.com/v1/systemone
```

Setup checks that the server answers and serves `kev-latest`. For a Kev server on the same machine as mailtriage, use `http://127.0.0.1:8009/v1/systemone`; it needs no key.

If the server does not answer, classification waits and your mail stays queued, as without a key.

::: warning Important
When you switch your server to another model, mailtriage cannot tell: the URL and the name `kev-latest` stay the same. Run `mailtriage reclassify --account work` to classify open mail again with the new model.
:::
````

- [ ] **Step 3: The other guide pages**

- `docs/.vitepress/config.mts`: in the guide's "Help and settings" group, change `{ text: 'API keys and models', link: '/guide/provider' }` to `{ text: 'Classifier and models', link: '/guide/provider' }` and add `{ text: 'Run the classifier on your Mac', link: '/guide/local-kev' }` right after it.
- `docs/guide/setup.md`, "What to choose": the Classifier row becomes `| Classifier | OpenRouter and the default Jev model. On an Apple Silicon Mac, [local Kev](./local-kev.md) keeps your mail on your Mac and needs no key. Use \`fake\` only for offline testing. |`. In "Key stores", change the link text `[API keys and models](./provider.md)` to `[Classifier and models](./provider.md)`.
- `docs/guide/configuration.md`: the row `| Change the model or key source | [API keys and models](./provider.md) |` becomes `| Change the classifier, model or key source | [Classifier and models](./provider.md) |`; in the last paragraph, `[API keys](./provider.md)` becomes `[Classifier and models](./provider.md)`.
- `docs/guide/troubleshooting.md`, "Classification was skipped": after the first paragraph add: `With Kev, the reason says what is wrong: local Kev is not installed, its port is taken, it did not start, or your own Kev server did not answer. See [How it starts and stops](./local-kev.md#how-it-starts-and-stops) for each fix. Your mail stays queued meanwhile, and no retry attempt is used.`
- `docs/guide/manual-setup.md`, "All doctor fields": change the `live_checks_performed` row to `| \`live_checks_performed\` | \`false\` | \`true\` only for \`kev\`: \`doctor\` then asks the Kev server for its models. |`, add a row after `provider.key_error`: `| \`provider.kev\` | \`ready: true\` | Only for \`kev\`. Local Kev: \`installed\`, \`tag\`, \`checkpoint\` and \`running\` (it starts on demand, so \`false\` is fine). Your own server: \`reachable\`. When not ready, \`error\` and \`fix\`. See [Provider reference](../reference/provider.md#doctor-and-kev). |`, extend the `ready` row's meaning with `, and for \`kev\` the server is installed or answering`, and add to the `provider.key_error` row's list: `or the same two messages naming the Kev server`. Change the last sentence `It makes no provider request.` to `It makes no provider request, except \`GET /v1/models\` to a Kev server; it never starts local Kev.`
- `docs/guide/install.md`, "Uninstall": `Your configuration, local state, logs and private Himalaya stay on disk.` becomes `Your configuration, local state, logs, private Himalaya and [local Kev](./local-kev.md#remove-it) stay on disk.`

- [ ] **Step 4: Reference pages**

`docs/reference/provider.md`:

- Replace `The \`provider\` block names the service that answers the three questions for every message. There are two kinds:` and its table with:

```markdown
The `provider` block names the service that answers the three questions for every message. There are three kinds:

| `kind` | What it is | Key |
| --- | --- | --- |
| `openrouter` | OpenRouter's Decisions API. `model` is any Decisions model ID; setup writes `typesafe/jev-latest`. | Needed; see [The OpenRouter key](#the-openrouter-key). |
| `kev` | A Kev server: local Kev, which mailtriage installs and runs on an Apple Silicon Mac, or a server you run. See [Kev](#kev). | Only for a server off this machine |
| `fake` | Fixed keyword rules for offline tests ([Try it offline](../guide/introduction.md#try-it-offline)). It makes no network request. | None |
```

- Append at the end of the page:

````markdown
## Kev

A `kev` provider sends the same request as `openrouter` to a Kev server and reads the same answers. Kev rounds probabilities to four decimals, so the same checks apply. `model` is `kev-latest`, the name Kev serves its checkpoint under.

```json
"provider": {
  "kind": "kev",
  "model": "kev-latest",
  "endpoint": "http://127.0.0.1:8009/v1/systemone",
  "managed": { "checkpoint": "jaredpalmer/kev-4b@v1.0" },
  "timeout_seconds": 60
}
```

A config with `managed` is [local Kev](#local-kev); one without is a server you run:

```json
"provider": {
  "kind": "kev",
  "model": "kev-latest",
  "endpoint": "https://classify.example.com/v1/systemone",
  "api_key_command": ["/usr/bin/security", "find-generic-password", "-s", "mailtriage", "-a", "kev", "-w"],
  "timeout_seconds": 60
}
```

`managed.checkpoint` is part of what decides whether mail is classified again, like `model`: changing it by hand to `jaredpalmer/kev-9b@v1.0` queues open mail again, and `mailtriage setup --update` then installs that checkpoint. Setup itself offers only Kev-4B. `managed` on any other kind is refused.

### The URL rule

- `endpoint` is a URL whose path ends in `/v1/systemone`, without a query, a fragment, a user name or a password.
- `http` only for `127.0.0.1`, `localhost` and `[::1]`; any other host needs `https`. Otherwise: `provider.endpoint must use https for a server off this machine` (exit 2).
- A server off this machine needs a key source, `api_key_command` or `api_key_env`, under the same [rules as OpenRouter's key](#the-openrouter-key); setup stores it under the account `kev`. Without one: `Kev server needs api_key_command or api_key_env` (exit 2). A server on this machine gets no key, even when the config names one.
- A managed endpoint is exactly `http://127.0.0.1:PORT/v1/systemone`, and `managed.checkpoint` is not empty.

### Local Kev

Setup installs local Kev into `<data dir>/mailtriage/kev` (`~/.local/share/mailtriage/kev`, or under `XDG_DATA_HOME`): Kev's code at tag `kev-1.0` in `src`, the weights in `hf`, and uv's Python and cache in `python` and `uv-cache`. It writes `installed.json` (`tag`, `checkpoint`, `installed_at`, `uv`) last, once a test classification passed.

When a command has mail to classify, mailtriage checks `GET http://127.0.0.1:PORT/v1/models`. If nothing answers, it starts a supervisor in the background and waits up to 120 seconds, checking every second. The supervisor:

- holds `server.lock`, so one Kev runs however many commands start one;
- runs `uv run --frozen --extra serve python -m kev.serve --run CHECKPOINT --host 127.0.0.1 --port PORT` in `src`, offline (`HF_HUB_OFFLINE=1`), with its output in `kev.log`;
- every 30 seconds, stops Kev when the last request is more than 15 minutes old: TERM, then KILL after 10 seconds;
- exits when Kev exits, for any reason, and logs why.

The background service needs no second job: `watch` starts the supervisor, which keeps running between passes.

While local Kev cannot answer, classification is skipped like it is without a key: no message is taken, no retry attempt is used, and the result gains `"classification": {"skipped": true, "reason": "..."}` (exit 4):

| `reason` | Cause |
| --- | --- |
| `local Kev is not installed; run mailtriage setup --update` | No `installed.json`, or it names another checkpoint or release. |
| `port PORT is used by another program` | Something else answers on the port, including a Kev that serves another checkpoint. |
| `local Kev did not start; see PATH/kev.log` | Nothing answered within 120 seconds. |
| `the Kev server did not answer at URL` | Your own server is not reachable. |

An HTTP error from Kev, or an answer that fails the contract check, fails the classification and counts an attempt, as with OpenRouter. Kev's errors read `Kev server returned HTTP 422`, `Kev server request failed` and so on.

### `doctor` and Kev

For `kev`, `doctor` adds `provider.kev`, asks the server for its models (`live_checks_performed: true`) and never starts local Kev:

| Field | Meaning |
| --- | --- |
| `mode` | `managed` or `server`. |
| `endpoint` | The configured endpoint. |
| `dir`, `installed`, `tag`, `checkpoint` | Local Kev: where it lives, and what `installed.json` names. `installed_checkpoint` when it names another checkpoint. |
| `running` | Local Kev: whether it answers now. `false` is fine; it starts on demand. |
| `reachable` | Your own server: whether `/v1/models` answered. |
| `ready` | Installed (local Kev) or serving the model (your own server). Part of the top-level `ready`. |
| `error`, `fix` | When not ready: what is wrong and what fixes it. |
````

`docs/reference/setup.md`:

- Step table, row 5: `| 5. Classifier | OpenRouter, local Kev, a Kev server you run, or the offline \`fake\` provider; the model; where the key lives. Installs local Kev. | \`--provider\`, \`--kev-url\`, \`--model\`, \`--key-store\`, \`--key-command\`, \`--key-env\`, \`--key-stored\`, \`--yes\` |`
- In **Step 5, classifier**, replace the first bullet with:

```markdown
- `--provider openrouter` (default), `--provider kev`, or `--provider fake` (offline keyword rules, no key). The menu offers OpenRouter, "Local Kev on this Mac" (only on an Apple Silicon Mac), "Kev server you run" and the offline demo.
- `--provider kev` alone is local Kev. On other machines it exits 2 with `step 5 (classifier): local Kev needs an Apple Silicon Mac; run your own Kev server and pass --kev-url`.
- `--kev-url URL` is a Kev server you run, and implies `--provider kev`; with another `--provider` it exits 2. The URL must follow [the URL rule](./provider.md#the-url-rule). Setup asks `GET /v1/models` and needs `kev-latest` (or `--model`) in the list; for a server off this machine it first runs the key steps, with the key store account `kev`. A server that refuses the request because the key is not available here (an environment variable set only where mailtriage runs) passes with a note.
- Local Kev installs before anything is written: at least 15 GB free (less exits 3; less than 32 GB of memory only warns), the question `Local Kev downloads about 9 GB to DIR and uses about 12 GB of memory while it classifies. Continue?` (`--yes` answers it; without prompts and without `--yes`, exit 2), `git` and `uv` on `PATH` (a missing `uv` is installed with `brew install uv` when Homebrew is there and you agree), `git clone --depth 1 --branch kev-1.0`, `uv sync --frozen --extra serve`, a first start that downloads the weights (up to 30 minutes, with Kev's output shown), a test classification, then the server stops and `installed.json` is written. Each failure exits 3 with `step 5 (Kev): ` and leaves the config unchanged; running setup again reuses every finished step. See [Run the classifier on your Mac](../guide/local-kev.md).
- Keeping a local Kev classifier ("Keep the current classifier?", or `--update` without classifier flags) installs it when it is missing, so `mailtriage setup --update` is the fix `doctor` names.
```

  Change the bullet `- Key flags with \`--provider fake\` exit 2: the offline classifier needs no key.` to `- Key flags with \`--provider fake\`, local Kev or a Kev server on this machine exit 2: they need no key.` and in the bullet listing the classifier flags add `--kev-url`.
- **Output** table: add the row `| \`kev\` | Only for \`kev\`. Local Kev: \`{mode: "managed", dir, action, tag, checkpoint}\`, \`action\` \`installed\` or \`current\` (it was already installed). Your own server: \`{mode: "server", endpoint, checked}\`, \`checked\` \`false\` when the server wanted a key not available here. |`.
- **Setup exit codes**: add to code 2: `\`--provider kev\` without \`--kev-url\` elsewhere than on an Apple Silicon Mac; \`--kev-url\` with another provider or against the URL rule; key flags with a Kev server on this machine; local Kev's download not agreed to;` and to code 3: `a local Kev install step failed (free space, \`git\`, \`uv\`, the clone, \`uv sync\`, the first start, the test classification); your Kev server did not answer or does not serve the model;`.

`docs/reference/configuration.md`, provider table:

- `kind`: `\`"openrouter"\` or \`"kev"\` for real classification, \`"fake"\` for offline tests. No other value is accepted. See [Provider](./provider.md).`
- `model`: append `For \`kev\`, \`kev-latest\`.`
- `endpoint`: append `For \`kev\`, a Kev server URL ending in \`/v1/systemone\`; see [the URL rule](./provider.md#the-url-rule).`
- `api_key_command` and `api_key_env`: change `Ignored for \`fake\`.` to `Ignored for \`fake\` and for a Kev server on this machine.`, and in `api_key_env` change `For \`openrouter\`, required` to `For \`openrouter\` and a Kev server off this machine, required`.
- New row after `timeout_seconds`: `| \`managed\` | Only for \`kev\`, written by setup for [local Kev](./provider.md#local-kev): \`{"checkpoint": "jaredpalmer/kev-4b@v1.0"}\`. Leave it out for a Kev server you run. |`

`docs/reference/install.md`, Uninstall: change `It keeps the installation lock file and the private Himalaya under \`~/.local/share/mailtriage/himalaya\` (other configs may use it; it prints how to delete it).` to `It keeps the installation lock file, the private Himalaya under \`~/.local/share/mailtriage/himalaya\` and local Kev under \`~/.local/share/mailtriage/kev\` (other configs may use them; it prints how to delete each).`

`docs/reference/index.md`: the Provider row's description becomes `Classifiers (OpenRouter, Kev), model selection, key lookup and key-command errors`.

- [ ] **Step 5: Development page**

In `docs/development/providers.md`, "Adding a provider":

- Step 1: list the trait as `validate`, `key_account(config)` (the key store account for this config, or `None` when it needs no key: the offline demo, a Kev server on this machine), `label`, `new_config`, `prepare` (optional: whether the provider can answer now; an `Err(ProviderUnavailable)` skips classification like a missing key, before any job is taken) and `decide`. Add: "A System One service (TypeSafe's API, which OpenRouter's Decisions and Kev share) reuses `provider/systemone.rs`: `request_body`, `send` (Bearer only with a key; the provider's name goes into the error texts) and `parse`."
- Step 2: "Add its kind to `KINDS` (today `openrouter`, `kev`, `fake`) and `provider_for`."
- Add a section before "## Contract evidence":

```markdown
## Local Kev

`src/kev/` runs Kev for the `kev` provider when the config has `managed`: `Layout` and `Installed` (`installed.json`), `runtime` (`ensure_running`, `probe`, `stop`, `status` for `doctor`), `supervisor` (the hidden `mailtriage kev run`) and `install` (setup's managed install). `ensure_running` errors are `ProviderUnavailable`.

The tests run without Python, uv, a network or the model. A stand-in `uv` (`tests/kev_support`) logs its calls and, for `uv run`, execs the test binary itself with `kev_standin_server --exact --ignored`, an ignored test that serves `/v1/models` and `/v1/systemone` on the given port. Control files make it exit at once or answer late. Stand-in `git` and `brew` cover setup. `MAILTRIAGE_TEST_KEV` (debug builds only) describes the machine to the tests: `apple_silicon`, `free_gb`, `memory_gb`, `port`, `start_seconds`, `download_seconds`.
```

- Add to "## Contract evidence":

```markdown
### Kev

Verified on 2026-10-11 against Kev's source at tag `kev-1.0` (`kev/serve.py`, `kev/api.py`) and its README. `POST /v1/systemone` takes `{state, model, questions}` with `choice` and `noul` questions and answers with typed `answers` in the Decisions shape, `model` echoing the request, probabilities rounded to four decimals. `GET /v1/models` lists `{name, run, ...}` per served name (`kev-latest`, `jev-latest`), `run` being `--run` as given. `KEV_API_KEY` makes the server require `Authorization: Bearer <key>` on `/v1/*`. `python -m kev.serve --run --host --port` starts it; the weights go to `HF_HOME`.

A loopback HTTP fixture in `tests/provider_contract.rs` checks the request (OpenRouter's pinned body with `"model":"kev-latest"`, at `/v1/systemone`), the Bearer header only with a key, and the decoder. A real install and a classification of the example messages on an Apple Silicon Mac remain a release gate.
```

- [ ] **Step 6: Build and check the docs**

Run: `cd docs && bun install --frozen-lockfile && bun run build`
Expected: the build succeeds, including the `buildEnd` anchor check (every `#anchor` above exists: `run-your-own-kev-server`, `how-it-starts-and-stops`, `remove-it`, `kev`, `the-url-rule`, `local-kev`, `doctor-and-kev`).

Run: `grep -rn '—' docs --include='*.md' --exclude-dir=node_modules; test $? -eq 1 && echo no-em-dashes`
Expected: `no-em-dashes`.

Read `docs/guide/local-kev.md` and the new section of `docs/guide/provider.md` once against the voice rules: "you", no opening with "AI", limits in the Important callouts, short paragraphs, US spelling.

- [ ] **Step 7: Run the full verification**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`
Expected: all green (no code changed in this task).

- [ ] **Step 8: Commit**

```bash
git add docs/guide docs/reference docs/development docs/.vitepress/config.mts
git commit -m "Document local Kev and your own Kev server" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Live check on this Mac (manual, not for subagents)

The human partner runs this on the Apple Silicon Mac (M4 Pro, 48 GB) after Tasks 1 to 6 are merged on `main`. It uses the real network, the real model and a scratch config, so the everyday config and its classifications stay untouched; the Kev install in `~/.local/share/mailtriage/kev` is the real one and stays for later use.

- [ ] **Step 1: Build and prepare a scratch config**

```sh
cd ~/Development/mailtriage
cargo build --release
export MT=$PWD/target/release/mailtriage
mkdir -p /tmp/kev-live
```

Expected: the build succeeds; `uv --version` and `git --version` print versions (if `uv` is missing, let setup offer `brew install uv` in Step 2).

- [ ] **Step 2: Install local Kev through setup**

```sh
$MT setup --config /tmp/kev-live/mailtriage.json --account kevlive --provider kev --filing off --service skip
```

Answer the Himalaya and account questions with your real IMAP account (read only: filing is off), and yes to the download question. Expected: the free-space check passes, Kev 1.0 is cloned, `uv sync` runs, Kev's log lines stream while about 9 GB download, the test classification passes, and setup ends with `Local Kev is installed in /Users/daniel/.local/share/mailtriage/kev.` and a ready check list including `kev`. Note the download time.

Then:

```sh
cat ~/.local/share/mailtriage/kev/installed.json
du -sh ~/.local/share/mailtriage/kev/*
lsof -nP -iTCP:8009 -sTCP:LISTEN
```

Expected: `installed.json` names `kev-1.0`, `jaredpalmer/kev-4b@v1.0` and the `uv` path; `hf` is about 9 GB and `python`, `uv-cache` are there; nothing listens on 8009 (setup stopped Kev).

- [ ] **Step 3: Classify the example messages**

```sh
for f in examples/*.eml; do
  /usr/bin/time -p $MT --config /tmp/kev-live/mailtriage.json classify --account kevlive --input "$f" --format rfc822 --json | jq '{outcome, state: .item.classification.state, category: .item.classification.category_id, urgency: .item.classification.urgency, action: .item.classification.action_required, model: .item.classification.model}'
done
$MT --config /tmp/kev-live/mailtriage.json classify --account kevlive --input examples/message.json --format json --json | jq .outcome
```

Expected: every outcome is `classified` and the model is `kev-latest`. Record the measured first-load and per-message times; the planned 10 to 30 second load and roughly one-second classification are estimates pending this check. Check that the categories are plausible: `receipt.eml` should be `transactions`, `reply-request.eml` should need action, `urgent-fyi.eml` should not.

- [ ] **Step 4: Sync a few real messages**

```sh
$MT --config /tmp/kev-live/mailtriage.json sync --account kevlive --limit 20 --json | jq '{classified, failed, partial, classification}'
$MT --config /tmp/kev-live/mailtriage.json list --account kevlive --view all --limit 20
```

Expected: `classified` > 0, `failed` 0, no `classification.skipped`. Spot-check five classifications against your own judgment.

- [ ] **Step 5: Offline, isolation, memory**

```sh
$MT --config /tmp/kev-live/mailtriage.json doctor --account kevlive --json | jq .provider.kev
lsof -nP -iTCP:8009 -sTCP:LISTEN
ps -o pid,pgid,rss,command -ax | grep -E 'mailtriage kev run|python -m kev[.]serve' | grep -v grep
tail -n 20 ~/.local/share/mailtriage/kev/kev.log
```

Expected: `doctor` shows `installed: true`, `running: true`, `ready: true`; Kev listens on `127.0.0.1:8009` only; the supervisor's PGID equals its PID; record Kev's actual RSS. The log shows `starting Kev: --run jaredpalmer/kev-4b@v1.0 on 127.0.0.1:8009` without a download. Keep the scratch account's filing off. Turn Wi-Fi off, then classify a fresh input whose bytes differ from the earlier example:

```sh
cp examples/receipt.eml /tmp/kev-live/offline.eml
printf '\nOffline probe %s\n' "$(date +%s)" >> /tmp/kev-live/offline.eml
$MT --config /tmp/kev-live/mailtriage.json classify --account kevlive --input /tmp/kev-live/offline.eml --format rfc822 --json | jq .outcome
```

Expect `classified`, not `cached`, to establish that Kev answered while offline. Turn Wi-Fi back on. The new bytes give this input a fresh identity in the scratch state.

- [ ] **Step 6: Idle stop and restart**

Wait 16 minutes without classifying, then:

```sh
lsof -nP -iTCP:8009 -sTCP:LISTEN
tail -n 10 ~/.local/share/mailtriage/kev/kev.log
cp examples/urgent-fyi.eml /tmp/kev-live/restart.eml
printf '\nRestart probe %s\n' "$(date +%s)" >> /tmp/kev-live/restart.eml
/usr/bin/time -p $MT --config /tmp/kev-live/mailtriage.json classify --account kevlive --input /tmp/kev-live/restart.eml --format rfc822 --json | jq .outcome
```

Expected: before classification nothing listens, and recent log lines include `stopping Kev after 900 seconds without a request` and `Kev exited (...)`. The fresh input returns `classified`, not `cached`; a new `starting Kev: --run ...` line and elapsed time show that this command restarted Kev and ran inference. Keep filing off throughout.

- [ ] **Step 7: Resume and clean up**

Run the Step 2 command again with `--update --yes`: it prints `Local Kev is installed in ...` and reuses everything (no clone, no download). A current-install result does not stop a running supervisor. Remove the scratch config with `rm -rf /tmp/kev-live`. Keep `~/.local/share/mailtriage/kev` for later use. If you choose to remove it, first stop all mailtriage callers that share it, identify any live `kev run` or `kev mutate` helper and Kev/git/uv/brew descendants from current process details, and require confirmed shutdown with no writer remaining. Do not remove the folder while ownership is uncertain; follow [the verified removal and repair guidance](../../docs/guide/local-kev.md#repair-an-installation) and preserve evidence as needed.

Record the results (download time, first-load time, per-message time, RSS, categories of the examples) in `docs/development/verification.md` under a new "Local Kev" heading, and commit that file with the message `Record the local Kev live check`.

---

## Self-review

Checked against `design/specs/2026-10-11-local-kev-provider-design.md`:

- **Decisions table.** Kind and `KINDS`: Task 2. Managed by `managed`: Tasks 1, 2. Release pins: Tasks 2 (checkpoint, model), 3 (tag, repo). Model `kev-latest`: Task 2. Key only off this machine, `key_account(config)`: Tasks 1, 2. URL rule: Task 2. On-demand lifecycle, no second agent: Task 3. Platforms: Task 4. Install location, `HF_HOME`: Task 3. Compatibility: Global Constraints, pinned tests unchanged in Tasks 1 and 2.
- **Config.** `managed` serialized only when set, part of the hash: Task 1. Validation list: Task 2 (`kev_endpoints_follow_the_url_rule`), `managed` on other kinds: Task 1.
- **Interface change.** Signature and the three answers: Tasks 1, 2. Callers (doctor, `classification_skipped`, service install note, setup, secrets): unchanged signature of `provider::key_account(config)` covers doctor, service and the note; setup's direct calls in Tasks 1 and 4; `secrets` takes the account as before and gets per-kind texts in Task 2.
- **Adapter.** `systemone.rs` with `request_body`, `send`, `parse`: Task 2. Kev texts: Task 2. `Kev::decide` steps 1 to 3: Tasks 2, 3 (the last-used touch happens in `ensure_running`, which every managed `decide` calls).
- **Managed runtime.** Layout: Task 3 (plus rulings 1 to 3). `kev run` steps 1 to 4 and `--allow-download`: Task 3. `ensure_running` steps 1 to 4 and concurrency: Task 3.
- **Setup.** Menu, flags, refusal text: Task 4. Managed install steps 1 to 8 and resume: Task 4. Your own server: Task 4 (ruling 9 on ordering).
- **Behaviour elsewhere.** `doctor`: Task 5. `sync`/`watch` skips and HTTP errors counting: Task 3. `service install` unchanged: no task touches it; its env-key note still asks `provider::key_account(config)`. `self uninstall`: Task 5.
- **Errors table.** Every row has a test: https (Task 2, Task 4), key source (Task 2), not installed and other checkpoint (Task 3, Task 5), port (Task 3), start timeout (Task 3), unreachable server (Task 3), HTTP error and contract failure count an attempt (Task 3; contract failures share OpenRouter's existing path).
- **Docs.** New page, "Classifier and models", "Run your own Kev server", reference, development: Task 6.
- **Testing.** OpenRouter unchanged; contract suite with and without a key; validation; runtime (first start, reuse, wrong program, two callers, idle stop, start timeout, Kev exiting); setup (full install, resume, missing uv with and without brew, refusal, loopback and remote server); skips; live check: Tasks 1 to 5 and 7.
- **Out of scope** stays out: no Linux or Intel managed install, setup offers only Kev-4B, nothing about fine-tuning or detecting a server's checkpoint change (the docs say how to reclassify).

Placeholders: none; every code step has its code, every run step its command and expected result. Names checked across tasks: `ManagedKev`, `key_account(&self, config)`, `ProviderUnavailable`, `prepare`, `Layout`, `Installed`, `Runtime`, `ensure_running_with`, `spawn_supervisor`, `supervisor_running`, `stop`, `STOP_WAIT`, `NOT_INSTALLED`, `status`, `install::install`, `LOCAL_KEV_LABEL`, `kev_support::{uv_script, git_script, brew_script, Stub, models_for, free_port, wait_until, serve_from_env}`.
