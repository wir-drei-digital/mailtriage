# Mailtriage implementation plan

2026-09-23. Scope approved: standalone core and Himalaya adapter, with urgency,
category and action required. Build offline first; live OpenRouter evaluation
waits for the user's key. Application UI and mailbox mutations are excluded.

## Delivery and SDD workflow

Develop an independent Rust repository in a temporary writable checkout, then
deliver it to `/Users/daniel/Development/mailtriage`. Do not modify Valea code.
Use Sol implementation agents with exclusive module ownership, followed by
independent specification and correctness review. The coordinating agent owns
storage, orchestration, integration, final tests and delivery. Reviews must
produce verified findings and fixes, not just completion reports.

## Modules and sequence

1. **Shared contract (coordinator):** Cargo project, domain types, this plan and
   interfaces. No secret required. Dependencies fetched once before build.
2. **Domain/core (Sol):** validated JSON config with account namespaces,
   categories and revisions; RFC822/JSON normalization; recipient context;
   strict Jev response decoding; conservative attention policy. Meaningful
   fixtures for mixed MIME, incomplete input, unknown choices and uncertainty.
3. **External adapters (Sol):** OpenRouter Decisions HTTP provider and explicit
   offline fake; pinned Himalaya subprocess adapter with bounded output, closed
   stdin, deadlines, raw fetch, IMAP epoch/UID discovery. Research the actual
   public contracts. Local HTTP and executable fixtures; no user mailbox/key.
4. **Durable state and services (coordinator):** SQLite migrations, account
   binding, discovery checkpoints, jobs/leases, raw decision attempts, result
   revisions, overrides, Done/reopen, category edits, scan coverage, pagination,
   account lock, retries and stale result protection. No remote I/O in list/read.
5. **CLI and delivery (Sol):** init/doctor/classify/sync/watch/list/read/correct/
   done/reopen/categories/reclassify/export, explicit config, JSON envelope and
   exit codes. Offline examples, Hermes guide, release build workflow, README.
   Implement after shared API is published; coordinate service calls explicitly.
6. **Integrate (coordinator):** complete one fixture-backed end-to-end path for
   each command, review JSON examples and verify actual release binary.
7. **Review (fresh Sol passes):** specification review and independent security/
   correctness review; fix material findings and rerun affected checks.
8. **Deliver:** cargo fmt/check/clippy/tests/release, smoke-run the release binary
   with fake provider, document live gates, copy complete repo to final location,
   initialize/retain Git history and create a clean initial implementation commit.

## Stable implementation interfaces

Shared types are in `domain.rs`. Agents must preserve them or coordinate changes.
Errors use `anyhow::Result`; CLI maps error categories into stable exit codes.

- `config::load(path) -> AppConfig`, `config::save(path, &AppConfig)`,
  `config::validate(&AppConfig)`, `config::default_config() -> AppConfig`.
- `normalize::rfc822(bytes, max_body_chars) -> NormalizedMessage` and
  `normalize::json(bytes, max_body_chars) -> NormalizedMessage`.
- `policy::decode_response(raw, account, policy, incomplete) -> Classification`;
  `policy::attention(classification, review_state) -> Vec<String>`.
- `provider::classify(&ProviderConfig, &AccountConfig, &NormalizedMessage,
  &PolicyConfig) -> Classification`, `provider::validate_configuration`.
- `himalaya::Himalaya::new(&HimalayaConfig)`, `version() -> String`,
  `snapshot(mailbox) -> MailboxSnapshot`,
  `discover(mailbox, after_uid, through_uid) -> Vec<SourceEnvelope>`,
  `fetch(mailbox, uid) -> Vec<u8>`.
- `service::Service::open(config_path)` owns config + Store. CLI receives a
  versioned serde_json::Value response from each public service method. CLI
  arguments live in cli.rs; main.rs stays minimal.

Specific service signatures will be published in `docs/development/service-api.md` before
the CLI agent starts. A versioned JSON config avoids adding a second parser;
strict validation applies to meaningful known fields with explicit errors.

## Behavioral requirements

- One account's data must never appear in another. Binding a name to a changed
  mailbox identity is rejected. Parameterized SQL; no shell-built commands.
- Input is untrusted mail. Provider payloads use fixed questions and finite
  answers. No tools, attachment execution or remote content retrieval.
- Action required is independently uncertain; invalid/missing decisions and
  incomplete bodies never silently become low/no-action.
- Every arrival is durable before advancing its UID checkpoint. UIDVALIDITY
  changes invalidate locators; fetch checks epoch on both sides. Raw bytes are
  hashed without JSON's lossy conversion. Classification never changes Seen.
- Jobs are retryable with bounded attempts/backoff. Leases recover after crash.
  Manual overlays and Done survive retries; stale taxonomy results cannot become
  current. External requests are at-least-once, not falsely exactly-once.
- Default configuration uses an explicit fake provider only for demos; real
  OpenRouter use is opt-in and requires the named environment variable. No
  credentials are read, printed or requested during offline development.
- SQLite is authoritative. Back up/export results and user state. Normalized
  body retained for offline selective reads; raw attachments not retained.
- Filter and count before paging. Cursor tied to query and revision; reject
  expired/mismatched cursors. Attention includes pending/error/review rows.
- Taxonomy labels may change without invalidating semantics; membership or
  definitions advance revision and requeue active messages. Deletion cannot
  strand manual labels; explicit mapping or review required.
- Provider-dependent model behavior is separate from deterministic policy tests.
  Review mode defaults on until quality thresholds are validated on real mail.

## Acceptance and evidence

Offline end-to-end: init → classify three sample messages → list attention →
read selected message → correct → reclassify → verify override → done/reopen →
export. Assert account scoping, JSON/exit-code behavior, no secrets in errors.

Adapter tests: fake executable emits pinned-version outputs; delayed and large
output is bounded; raw MIME binary survives; no Seen flag sent; UID epoch reset
and partial scan errors retain truthful coverage. HTTP fixture validates the
actual Decisions request and malformed/error/retry handling.

Storage tests: duplicate content, crash lease recovery, correction races,
taxonomy revision races, cursor validity, checkpoint atomicity, partial fetches,
retry exhaustion, database reopen and account rebinding.

Delivery checks: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test`, `cargo build --release`, release CLI smoke. Linux artifacts are
provided by CI; do not claim local cross-platform execution without evidence.

Live gates explicitly deferred until credentials/test mailbox: synthetic Jev
call through OpenRouter, real IMAP Seen/epoch verification, quality evaluation,
and execution in the user's headless Hermes deployment.

## Completion

Implementation stages 1–8 completed on 2026-09-23. Sol agents implemented core,
adapters and CLI/delivery, then reviewed integrated behavior. Review findings
were reproduced, fixed and covered by regressions. See [verification.md](verification.md)
for the 30-test suite, release build, smoke checks and deferred live gates.
