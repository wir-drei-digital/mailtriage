# mailtriage

`mailtriage` is a local CLI that classifies mail by urgency, category, and whether the recipient needs to act. It stores results in SQLite so listing and reading work offline. Himalaya supplies IMAP messages; the provider is either an explicit offline fake or OpenRouter Decisions.

The first release handles one configured account namespace at a time. It never sends, deletes, or marks server messages as read, and it moves mail and adds `\Flagged` only for an account that enables [filing into folders](#filing-into-folders). `done` and `reopen` always change only the local review state; filing never changes them on the server.

## Build

```sh
cargo build --release
./target/release/mailtriage --help
```

The release binary is `target/release/mailtriage`. Rust's current stable toolchain and a local SQLite-compatible filesystem are required. CI tests and packages native binaries for Linux amd64, Linux arm64, and macOS arm64. Pushing a matching `vX.Y.Z` tag publishes these archives and SHA-256 checksums to [GitHub Releases](https://github.com/wir-drei-digital/mailtriage/releases) after all checks pass. See the [release guide](docs/releases.md) for publishing, prereleases, and download verification.

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

Set up Himalaya v2.1.0 separately, then add an `engine` object with `"kind": "himalaya"` to the target account in `mailtriage.json` (an older `himalaya` block is still read, and is written back as `engine` when mailtriage saves the config). Set `binary`, `config`, `account`, `mailboxes`, `expected_version`, `timeout_seconds`, and `max_output_bytes`. The adapter requires IMAP UID and UIDVALIDITY support and checks the binary version in `doctor`. It does not invoke Himalaya's setup wizard.

```sh
mailtriage doctor --account work --json
mailtriage sync --account work --limit 100 --json
mailtriage watch --account work --limit 100 --interval-seconds 60 --json
```

`sync` performs one bounded pass; `watch` repeats it until Ctrl-C or SIGTERM. Watch prints one JSON object per completed pass and one final stop object when `--json` is used. A pass that `mailtriage.json` or the Himalaya configuration changed under is skipped: watch prints its error object (exit code 5 in `error.code`) and continues with the next pass, which reads the current configuration; the stop object counts such passes in `skipped_passes`. Any other error, including a changed account binding, ends watch with its exit code. Configure a supervisor to restart it if necessary. Fetch failures with a present source remain visible in Attention and can be retried. Reconciliation marks messages whose source occurrence has disappeared; they remain in the local `all` view. The sync response reports discovered, fetched, classified, cached, failed and pending counts. Periodic bounded reconciliation retires vanished source references while retaining local content; unresolved fetch failures stay visible. A complete scan requires a stable mailbox UID epoch and adequate UID-range discovery. Mail that enters and leaves watched folders between polls cannot be recovered.

OpenRouter requires an explicit `provider.kind` of `openrouter`, a verified Decisions endpoint, a model ID, and the name of an environment variable containing the API key. Put the key in that environment variable, never in the config. Verify a synthetic call and evaluate thresholds with labeled mail before disabling review mode. The tool does not silently fall back to a different provider.

The SQLite database and normalized message text under `state_dir` are authoritative private data. Stop the worker before copying this directory (including any SQLite WAL files), and keep a JSON export; losing the database loses user corrections and Done state. Raw attachments are not retained by default. The state directory is intended for one local owner and should not be shared over a network filesystem. See [the Hermes guide](docs/hermes.md) for headless use and [the design](docs/design.md) for the full behavior contract.

## Filing into folders

Filing makes the classification visible in every mail client: each classified message in a source folder (the engine's `mailboxes`, usually `INBOX`) is moved into a top-level folder named after its category (or the category's `folder`), and mail that needs action or is urgent gets `\Flagged`, so the inbox stays empty without a new mail app. It is off by default and is enabled per account.

```sh
mailtriage filing enable --account work --mode dry-run --json
mailtriage filing plan --account work --json
mailtriage filing enable --account work --mode live --json
mailtriage filing backfill --account work --days 30 --json
mailtriage filing backfill --account work --days 30 --apply --json
mailtriage filing status --account work --json
mailtriage filing log --account work --limit 50 --json
```

`filing.mode` has three values. `off` (the default) leaves the mailbox exactly as before. `dry_run` watches the category folders and plans every pass without writing anything: no folder is created and no message is moved or flagged; each `sync` reports how many moves and flags it would make under `filing.planned`, and `filing plan` lists them. `filing plan`'s `total` counts the actions of the next pass only, capped at `filing.max_actions_per_pass`, not the whole backlog. `live` creates and subscribes the category folders, moves mail and adds flags. `filing enable` and `filing disable` write `filing.mode` under the configuration lock; the change takes effect at the next pass. The account needs a mail engine; `enable` refuses without one.

Recommended rollout: `filing enable --mode dry-run`, keep `watch` (or scheduled `sync`) running, and check `filing plan` and the `sync` summaries for a few days. Then `filing enable --mode live`. Only mail that arrives after filing was enabled is filed automatically; mail delivered during the dry-run days counts, so the first live passes file it. To file older mail, preview it with `filing backfill (--days N | --all)` and repeat with `--apply`, which requires `live` and makes each listed message eligible once; `--days` uses the server's internal date. Moves happen over the following passes, at most `filing.max_actions_per_pass` (default 200) per pass. Mail that arrives while filing is off counts as older mail when filing is enabled again.

`filing enable` gives every category without one an explicit `folder` (its current name), so renaming a category later never moves its folder; `categories apply` with filing on keeps each category's folder the same way. A folder must be a single path segment of printable ASCII, at most 200 bytes, with no leading or trailing spaces, without `/ . * % " \ &`, not starting with `-`, not a case variant of `inbox` other than the literal `INBOX`, and unique among the categories (ignoring case). A category whose `folder` is the literal `INBOX` stays in its source folder. Invalid folders are refused as `categories need a valid folder: <ids>`; to fix that, set `folder` for those categories in the category file (`categories export`) and run `categories apply`. `categories validate --file FILE --account NAME` checks a file against that account's configuration, folder rules included when its filing is on; without `--account` it checks the file alone, without folder rules. A server's personal namespace prefix (for example `INBOX.`) is added automatically. An existing folder with the category's name is adopted when the server reports it has no special role; when the server cannot report roles it waits for `filing adopt --folder NAME`. Folders with a special role (Sent, Trash, Junk, Archive, All Mail and the like, or such names) are never used, and a deleted category folder is not recreated. `filing status` lists such categories under `paused_categories`.

A message is moved automatically at most once, only out of a source folder, and only on a current classification (made under the current categories, provider and policy, and not from incomplete input) or your category correction. Review mode does not hold filing back. `\Flagged` is added once when the effective decision is `action_required` or `high` urgency, and only to new mail, backfilled mail, or mail mailtriage already filed. If the message already carries `\Flagged`, that counts as the attempt, so unflagging it later in a client is respected. Set `filing.flag` to `false` to disable flags.

You correct filing from any mail client. Moving a message into another category's folder is a category correction, exactly like `correct --category`. Moving it back to a source folder pins it there: it is not filed again until `filing unpin --id ID`. Moving a message into its own category's folder keeps it there and drops any pin. With filing on, `correct --category` (or clearing the correction) moves the mail to the resulting category's folder. `filing pin --id ID` keeps a message in its source folder, moving it back if it was filed. Mail delivered straight into a category folder (for example by a server rule) is recorded as filed by you in that category.

Archiving or deleting a message in a client means done: once it is gone from every watched folder, a later pass that has fully scanned every watched folder, with no paused folder and no unidentified arrival outstanding, marks it done locally (`list --view all` shows it with `review_state` `done`). If it comes back, that inferred done is reopened. A message you marked done yourself is never reopened by observation.

`filing status` makes no mailbox calls. It reports the configured and stored mode, `enabled_at`, the server's MOVE, UIDPLUS and SPECIAL-USE support from the last pass, every folder with its state, pause and subscription, paused categories, intents by state, counts of `blocked` and `quarantined` messages and `ambiguous` locations (up to 50 ids each in `blocked_ids`, `quarantined_ids` and `ambiguous_ids`; `blocked_ids` also gives each `blocked_reason`), `unresolved_arrivals` (listed under `unresolved_arrival_items`), `eligible_unfiled` mail, `stale_requests` (requests for a category that no longer exists; correct or unpin them), alias conflicts, and the last pass summary. `filing log` lists filing events newest first, optionally for one message with `--id`. `list` and `read` items carry a `placement` object (folder, location state, who filed it, pin, flag, block and pending move). `doctor` adds a `filing` block with the server's capabilities, folders and problems when filing is on.

Safety rules: mailtriage never deletes or expunges mail, never removes a flag, never changes read state (`\Seen`), and never renames or deletes a folder. It writes only on servers with the IMAP MOVE extension; there is no copy-and-delete fallback. Every move and flag is journaled before it is sent and is resolved by observation after a failure or crash. If a write may have run against a recreated mailbox, mailtriage reverts it where the server reported where the mail went, otherwise it pauses that folder and quarantines the mail that arrived; nothing is written to a paused folder or a blocked message until you act. After checking the folder, `filing retry --folder NAME` releases a pause and rescans it; `filing retry --id ID` lifts a message's `move_failed` or `quarantined` block and also makes it eligible once, so a message still in a source folder is filed (and possibly flagged) on the next pass. A `duplicate_copy` block means the message is in two folders: remove one copy in your mail client, run `sync` so mailtriage observes it, then `filing retry --id ID` lifts the block. While both copies are recorded the retry is refused with exit code 5 (`remove one copy first, then sync and retry`), and lifting `duplicate_copy` does not make the message eligible once. It does not lift a `merge_conflict` block, which belongs to an unresolved arrival: an arrival that could not be identified is refetched with `filing retry --arrival N` or marked reviewed with `filing dismiss --arrival N`, and either lifts a merge-conflict block it caused. A Himalaya mailbox alias that points a watched folder elsewhere stops that folder's discovery and fetches and all filing writes until it is fixed. A change to the Himalaya configuration during a pass, or to `mailtriage.json` before the pass writes, aborts the pass with exit code 5. `sync` exits with it; `watch` reports the aborted pass as an error object and continues, and its next pass uses the new configuration.

Provider check: before you run `live` on a real mailbox, the live provider check in the [filing design](docs/superpowers/specs/2026-10-04-imap-category-filing-design.md#live-provider-check) must have recorded a go for your provider (Gmail / Google Workspace, Microsoft 365 / Outlook.com, iCloud, Fastmail / Dovecot) in [the verification receipt](docs/verification.md). No provider has been checked yet, so use `dry_run` until yours is; a no-go will be listed here.

Filing commands use the usual exit codes: 0 for success; 2 for invalid input or config (no mail engine, invalid folders, `backfill --apply` outside `live`, an unknown folder or message, an arrival that is not unresolved); 3 for an operational failure; 4 for a partial sync, which any filing error in a pass causes; and 5 for a conflict (the configuration changed during the command or pass, a placement changed concurrently, a message whose identity is not yet established, a `duplicate_copy` retry while both copies are still recorded, or a changed account binding).

## Current verification boundary

Offline fixture and adapter contract tests do not prove the real mailbox's Seen flag behavior, the live OpenRouter gateway, or classifier quality on representative mail. These require a test mailbox, credentials, and labeled examples before production quiet dismissal.

## Practical limits

The first scan advances in bounded UID ranges from the start of each watched mailbox; large or sparse UID histories may require many passes. `coverage` reports progress. `reclassify` queues all matching messages and processes up to its limit; subsequent `sync` calls drain the remaining queue. The `--since` filter uses the local first-observed date.

The account binding detects changes to configured IMAP endpoint/auth identity, while allowing credential value rotation. An opaque OAuth token broker can change its underlying principal without changing visible configuration; keep `account.identity` accurate and use a new namespace for a different mailbox. Direct edits to category files should preserve IDs and revisions; `categories apply` performs the validation and migration checks for you.

The fake provider uses deterministic keyword rules only to exercise plumbing. Review mode defaults on. No accuracy claim is made until the live Jev evaluation is complete. The public JSON export is an inspection/export format; restore operational state from the database backup.
