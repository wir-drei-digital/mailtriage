# mailtriage

`mailtriage` is a local CLI that classifies mail by urgency, category, and whether the recipient needs to act. It stores results in SQLite so listing and reading work offline. Himalaya supplies IMAP messages; the provider is either an explicit offline fake or OpenRouter Decisions.

The first release handles one configured account namespace at a time. It does not send, move, delete, or mark server messages as read. `done` changes only the local review state.

## Build

```sh
cargo build --release
./target/release/mailtriage --help
```

The release binary is `target/release/mailtriage`. Rust's current stable toolchain and a local SQLite-compatible filesystem are required. The CI and release workflows are configured to build native binaries for Linux amd64, Linux arm64, and macOS arm64, then upload archives as workflow artifacts. These artifacts become available only after the workflows run successfully on GitHub; local verification does not prove those remote builds.

This checkout also includes a locally verified macOS arm64 executable at
`dist/mailtriage`. See [the verification receipt](docs/verification.md) for build
evidence and tests reserved for live credentials.

## Offline start

Run these commands in a fresh directory:

```sh
mailtriage init --config mailtriage.json --json
mailtriage classify --config mailtriage.json --account work --input /path/to/examples/reply-request.eml --format rfc822 --json
mailtriage list --config mailtriage.json --account work --view attention --json
```

`init` writes `mailtriage.json` with a named `fake` provider, a `work` account, and a relative `.state` directory. It refuses to replace an existing config. The fake provider is for deterministic offline evaluation; it does not make a network request. Keep the config path explicit in jobs and scripts. Paths in the config are resolved relative to that file.

You can also classify a JSON input with `--format json`, or pass `--input -` to read stdin. An example is in `examples/`. `classify` persists the result; it does not create a server occurrence.

## Query and correct

```sh
mailtriage list --account work --view all --limit 50 --json
mailtriage list --account work --category correspondence --urgency low --json
mailtriage read --account work --id MESSAGE_ID --json
mailtriage correct --account work --id MESSAGE_ID --action-required true --json
mailtriage correct --account work --id MESSAGE_ID --clear urgency --json
mailtriage done --account work --id MESSAGE_ID --json
mailtriage reopen --account work --id MESSAGE_ID --json
mailtriage export --account work --json > mailtriage-backup.json
```

`list` and `read` use only stored state. A correction overlays one model field; clearing it exposes the current model decision. `done` survives later classification and can be reversed with `reopen`. `list` accepts `--cursor` from a prior page; a cursor is tied to its query and state revision, so a changed result set requires starting again.

Every successful JSON response has a `schema_version`. A JSON error has `schema_version` and `error.code`/`error.message`. Exit codes are 0 for success, 2 for invalid input or config, 3 for a failed dependency or operation, 4 for a partial sync, and 5 for a revision or identity conflict. A successful query exits 0 even if individual messages need review. Error messages omit message bodies and credentials.

## Categories

```sh
mailtriage categories export --account work --json > categories.json
mailtriage categories validate --file categories.json --json
mailtriage categories apply --account work --file categories.json --json
mailtriage reclassify --account work --dry-run --json
mailtriage reclassify --account work --since 2026-09-01 --json
```

Category files may contain a JSON array or an object with a `categories` array. Each category has a stable `id`, name, description, optional examples, and `catch_all`; exactly one catch-all category is required. Renaming a category preserves its ID. Changes to membership or definitions advance the taxonomy revision and schedule open messages for classification. Done messages stay done; reopening them applies current policy. Applying a taxonomy that would strand a manual category correction fails until that correction is explicitly cleared or remapped.

## Himalaya and OpenRouter

Set up Himalaya v2.1.0 separately, then add `himalaya` to the target account in `mailtriage.json`. Set `binary`, `config`, `account`, `mailboxes`, `expected_version`, `timeout_seconds`, and `max_output_bytes`. The adapter requires IMAP UID and UIDVALIDITY support and checks the binary version in `doctor`. It does not invoke Himalaya's setup wizard.

```sh
mailtriage doctor --account work --json
mailtriage sync --account work --limit 100 --json
mailtriage watch --account work --limit 100 --interval-seconds 60 --json
```

`sync` performs one bounded pass; `watch` repeats it until Ctrl-C or SIGTERM. Watch prints one JSON object per completed pass and one final stop object when `--json` is used. Configure a supervisor to restart it if necessary. Fetch failures with a present source remain visible in Attention and can be retried. Reconciliation marks messages whose source occurrence has disappeared; they remain in the local `all` view. The sync response reports discovered, fetched, classified, cached, failed and pending counts. Periodic bounded reconciliation retires vanished source references while retaining local content; unresolved fetch failures stay visible. A complete scan requires a stable mailbox UID epoch and adequate UID-range discovery. Mail that enters and leaves watched folders between polls cannot be recovered.

OpenRouter requires an explicit `provider.kind` of `openrouter`, a verified Decisions endpoint, a model ID, and the name of an environment variable containing the API key. Put the key in that environment variable, never in the config. Verify a synthetic call and evaluate thresholds with labeled mail before disabling review mode. The tool does not silently fall back to a different provider.

The SQLite database and normalized message text under `state_dir` are authoritative private data. Stop the worker before copying this directory (including any SQLite WAL files), and keep a JSON export; losing the database loses user corrections and Done state. Raw attachments are not retained by default. The state directory is intended for one local owner and should not be shared over a network filesystem. See [the Hermes guide](docs/hermes.md) for headless use and [the design](docs/design.md) for the full behavior contract.

## Current verification boundary

Offline fixture and adapter contract tests do not prove the real mailbox's Seen flag behavior, the live OpenRouter gateway, or classifier quality on representative mail. These require a test mailbox, credentials, and labeled examples before production quiet dismissal.

## Practical limits

The first scan advances in bounded UID ranges from the start of each watched mailbox; large or sparse UID histories may require many passes. `coverage` reports progress. `reclassify` queues all matching messages and processes up to its limit; subsequent `sync` calls drain the remaining queue. The `--since` filter uses the local first-observed date.

The account binding detects changes to configured IMAP endpoint/auth identity, while allowing credential value rotation. An opaque OAuth token broker can change its underlying principal without changing visible configuration; keep `account.identity` accurate and use a new namespace for a different mailbox. Direct edits to category files should preserve IDs and revisions; `categories apply` performs the validation and migration checks for you.

The fake provider uses deterministic keyword rules only to exercise plumbing. Review mode defaults on. No accuracy claim is made until the live Jev evaluation is complete. The public JSON export is an inspection/export format; restore operational state from the database backup.
