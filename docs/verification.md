# Build and verification receipt

2026-09-23. Implemented with three GPT-6 Sol agents and coordinator integration,
following the [implementation plan](implementation-plan.md). Independent review
reports: [core](review-core.md) and [integration](review-cli-integration.md).

Completed: Rust library/CLI; all three decisions; editable taxonomy; RFC822/JSON
normalization; OpenRouter Decisions and explicit fake providers; Himalaya 2.1.0
adapter; SQLite jobs, retry and lease recovery; source binding and UID epochs;
bounded discovery/reconciliation; corrections, Done/reopen, query cursors and
export; Hermes guide and cross-platform CI/release workflows.

| Executed check | Result |
| --- | --- |
| `cargo fmt --check` | Passed |
| `cargo clippy --all-targets --locked --offline -- -D warnings` | Passed |
| `cargo test --locked --offline` | 30 tests passed |
| `cargo build --release --locked --offline` | macOS arm64 executable built |
| Release smoke | init, classify, list, correct, done, reopen, read, export |
| Supervisor shutdown | release watch exited cleanly on SIGTERM |
| Official Himalaya release | checksum, version, help and JSON schemas verified |

Tests comprise 7 adapter, 2 CLI, 7 core, 13 storage and 1 full sync test. HTTP
fixtures bind localhost and make no external model calls. The sync test runs an
executable Himalaya fixture through classification, idempotency, removal and
UID epoch reset. CI workflows are configured; no remote workflow was run here.

Local executable: `dist/mailtriage` (ignored build artifact). SHA-256:

```text
90197575357171c6333009646ce8ea2389512183c2c2f94231046fd89a5d9b9e
```

No private mailbox was read, no API key was used, and no remote repository or
release was published. No Valea application code was changed.

## Live gates

1. Supply the OpenRouter key, change a test config from `fake` to `openrouter`,
   and classify synthetic mail through the documented Decisions endpoint.
2. Evaluate urgency/action recall and category quality on approved labeled mail
   before disabling review mode. Current thresholds are configurable starting
   values, not measured accuracy claims.
3. Verify Seen preservation and UID transitions with a test IMAP account and
   the actual Himalaya/server pair; validate the cloud Hermes runtime.

Review mode stays on by default. Fake-provider results prove plumbing, not Jev
accuracy. Source principals hidden behind opaque OAuth token helpers remain the
configured identity owner's responsibility. Independent installations do not
synchronize state. Operational restore uses the SQLite backup; JSON export is
an inspection/interchange artifact.
