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

## Dovecot end to end

`.github/workflows/e2e.yml` runs `tests/e2e_dovecot.rs` on every push to
`main`, on pull requests and on demand. It uses the real `mailtriage` binary,
the official Himalaya v2.1.0 Linux x86_64 release (`himalaya.x86_64-linux.tgz`,
SHA-256 `683a2ab8e1534f01e6bda3a69e204d564c31fbfbe20511fc7bc60b67f2e85884`) and
a `dovecot/dovecot:2.3.21` container in two namespace layouts: no prefix with
separator `/` (`tests/e2e/dovecot-flat.conf`) and prefix `INBOX.` with
separator `.` (`tests/e2e/dovecot-prefix.conf`). The test enables `live` filing
and checks: filing into category folders with read state preserved and
`\Flagged` added, a client move as a category correction, a client move back
to `INBOX` as a pin, a client delete as done, and an `INBOX` UIDVALIDITY reset
that keeps the pinned message known and open. To run it locally (Docker,
Python 3 and Himalaya v2.1.0 required):

```sh
MT_E2E_HIMALAYA="$(command -v himalaya)" bash tests/e2e/run.sh flat 31143
MT_E2E_HIMALAYA="$(command -v himalaya)" bash tests/e2e/run.sh prefix 31144
```

`cargo test` ignores this test; run with `--ignored` but without the
`MT_E2E_*` variables, it prints a notice and skips.

## Live provider check (required before `live` on a real mailbox)

The Dovecot end-to-end job covers the protocol contract, not provider
behaviour. Before setting `filing.mode` to `live` on a real mailbox, run this
check for that provider with a throwaway test account configured in Himalaya
(see the [filing design](superpowers/specs/2026-10-04-imap-category-filing-design.md#live-provider-check)).
No credentials, message bodies or real addresses go into this file: record
redacted output, or where the evidence is kept. `Result` is `pass`, `fail` or
`differs` (with a note). Until a provider has a recorded go, use `dry_run`,
which never writes.

### Gmail / Google Workspace

| Check | Result | Evidence | Date |
| --- | --- | --- | --- |
| `imap raw` output for pipelined `CAPABILITY`/`NAMESPACE` | | | |
| `imap raw` output for `SELECT` + `UID MOVE` (UIDVALIDITY, COPYUID) | | | |
| `imap raw` output for `SELECT` + `UID STORE` | | | |
| `imap raw` output for `LIST "" "*" RETURN (SPECIAL-USE)` | | | |
| `imap list --all --json` and `imap list --json` shapes, delimiter, attributes returned, and the representation of a non-ASCII folder name | | | |
| The alias table location in the Himalaya TOML | | | |
| Create and subscribe for an ASCII and a non-ASCII name | | | |
| INTERNALDATE, size and Message-ID before and after a move | | | |
| `\Flagged` in the provider's web and mobile clients | | | |
| `\Seen` unchanged by fetch, move and store | | | |
| Gmail: label semantics of MOVE from INBOX, archive, and a message carrying two category labels | | | |
| Login rate: one `watch` with seven watched folders at a 60-second interval for 30 minutes without throttling; if throttled, whether pipelined STATUS via `imap raw` resolves it | | | |

### Microsoft 365 / Outlook.com

| Check | Result | Evidence | Date |
| --- | --- | --- | --- |
| `imap raw` output for pipelined `CAPABILITY`/`NAMESPACE` | | | |
| `imap raw` output for `SELECT` + `UID MOVE` (UIDVALIDITY, COPYUID) | | | |
| `imap raw` output for `SELECT` + `UID STORE` | | | |
| `imap raw` output for `LIST "" "*" RETURN (SPECIAL-USE)` | | | |
| `imap list --all --json` and `imap list --json` shapes, delimiter, attributes returned, and the representation of a non-ASCII folder name | | | |
| The alias table location in the Himalaya TOML | | | |
| Create and subscribe for an ASCII and a non-ASCII name | | | |
| INTERNALDATE, size and Message-ID before and after a move | | | |
| `\Flagged` in the provider's web and mobile clients | | | |
| `\Seen` unchanged by fetch, move and store | | | |
| Login rate: one `watch` with seven watched folders at a 60-second interval for 30 minutes without throttling; if throttled, whether pipelined STATUS via `imap raw` resolves it | | | |

### iCloud

| Check | Result | Evidence | Date |
| --- | --- | --- | --- |
| `imap raw` output for pipelined `CAPABILITY`/`NAMESPACE` | | | |
| `imap raw` output for `SELECT` + `UID MOVE` (UIDVALIDITY, COPYUID) | | | |
| `imap raw` output for `SELECT` + `UID STORE` | | | |
| `imap raw` output for `LIST "" "*" RETURN (SPECIAL-USE)` | | | |
| `imap list --all --json` and `imap list --json` shapes, delimiter, attributes returned, and the representation of a non-ASCII folder name | | | |
| The alias table location in the Himalaya TOML | | | |
| Create and subscribe for an ASCII and a non-ASCII name | | | |
| INTERNALDATE, size and Message-ID before and after a move | | | |
| `\Flagged` in the provider's web and mobile clients | | | |
| `\Seen` unchanged by fetch, move and store | | | |
| Login rate: one `watch` with seven watched folders at a 60-second interval for 30 minutes without throttling; if throttled, whether pipelined STATUS via `imap raw` resolves it | | | |

### Fastmail or a Dovecot host

| Check | Result | Evidence | Date |
| --- | --- | --- | --- |
| `imap raw` output for pipelined `CAPABILITY`/`NAMESPACE` | | | |
| `imap raw` output for `SELECT` + `UID MOVE` (UIDVALIDITY, COPYUID) | | | |
| `imap raw` output for `SELECT` + `UID STORE` | | | |
| `imap raw` output for `LIST "" "*" RETURN (SPECIAL-USE)` | | | |
| `imap list --all --json` and `imap list --json` shapes, delimiter, attributes returned, and the representation of a non-ASCII folder name | | | |
| The alias table location in the Himalaya TOML | | | |
| Create and subscribe for an ASCII and a non-ASCII name | | | |
| INTERNALDATE, size and Message-ID before and after a move | | | |
| `\Flagged` in the provider's web and mobile clients | | | |
| `\Seen` unchanged by fetch, move and store | | | |
| Login rate: one `watch` with seven watched folders at a 60-second interval for 30 minutes without throttling; if throttled, whether pipelined STATUS via `imap raw` resolves it | | | |

### Outcome

| Provider | Outcome (go / go with noted differences / no-go) | Notes | Date |
| --- | --- | --- | --- |
| Gmail / Google Workspace | not checked | | |
| Microsoft 365 / Outlook.com | not checked | | |
| iCloud | not checked | | |
| Fastmail or a Dovecot host | not checked | | |

A no-go blocks enabling `live` for that provider until it is resolved and is
listed in the README's "Filing into folders" section.
