# Errors and local data

Look things up here: how mailtriage reports results and errors, what ties an account to its mailbox, how to back up your state, and what has and has not been verified.

## Output and exit codes

With `--json`, every result is one line of JSON on stdout, and so is every error: `{"schema_version":1,"error":{"code":N,"message":"..."}}`. Without `--json`, results are pretty-printed JSON and errors go to stderr as `mailtriage: MESSAGE`.

Every result has a `schema_version`. Error messages omit message bodies and credentials.

Some errors also carry a machine-readable `reason` in the error object, for scripts that react to a class of error rather than to its message. An error without a reason has no `reason` key. The reasons:

- `config_changed`: `mailtriage.json` or the Himalaya configuration changed during the command.
- `config_busy`: another command is editing `mailtriage.json`.
- `account_busy`: another worker for the account is running.
- `binding_conflict`: the account binding changed, see [Account binding](#account-binding).
- `categories_changed`: `categories apply --expect-digest` found other categories.
- `service_config_mismatch`: the account's service runs another config, see [Service commands](./service.md#service-commands).
- `service_config_unknown`: the config of the account's service cannot be told.
- `service_busy`: another service command for the account held the service lock for 30 s.
- `unsafe_permissions`: a directory mailtriage would install a program into is not safe; see [A private Himalaya](./himalaya.md#a-private-himalaya).
- `setup_aborted`: you chose "Abort" in setup's menu; nothing was changed.

| Code | Meaning |
| --- | --- |
| 0 | Success. A query exits 0 even if messages need attention, and `doctor` exits 0 even if `ready` is `false`. |
| 2 | Invalid input or configuration: an unknown flag value, account, category or message, a missing or invalid `mailtriage.json`. |
| 3 | Operational failure, reported as `Operation failed; check configuration and dependency availability`, for example when Himalaya cannot run. |
| 4 | Partial result: a pass, `classify` or `reclassify` with failed messages or scan errors, or whose classification was skipped because the key is unavailable; a pass with filing errors; `watch` at stop after a partial or skipped pass. |
| 5 | Conflict: the configuration changed during the command, another worker is running, the account binding changed, a cursor expired, a placement changed concurrently, or the categories changed since their export (`categories_changed`). |

`setup`, `service`, `himalaya install`, `self install` and `self uninstall` have their own cases; see [Setup exit codes](./setup.md#setup-exit-codes), [Background service](./service.md), [A private Himalaya](./himalaya.md#a-private-himalaya), [`mailtriage self install`](./install.md#mailtriage-self-install) and [Uninstall](./install.md#uninstall).

## Account binding

The binding keeps mailtriage from mixing up two mailboxes under one account name.

The first command that opens an account stores a binding in the state directory: the account's `identity`, the Himalaya account name, `imap.server` and the other IMAP settings except secrets. Settings whose key contains `password`, `passwd`, `token` or `secret` are left out, so you can rotate a password or token.

After a change to anything else in the binding, commands for that account exit 5 with `account binding changed or state unavailable; verify config and use a new namespace for a different mailbox`.

To use a different mailbox, add a new account name in `mailtriage.json` (or with `mailtriage setup --update --account NEWNAME`). `mailtriage setup` checks the binding before it writes and refuses a change with exit 5.

An opaque OAuth token helper can change its underlying account without changing the visible configuration, so keep `identity` accurate.

## State and backups

The SQLite database and normalized message text under `state_dir` are authoritative private data. Raw messages and attachments are not stored.

To back it up, stop the worker before copying the directory, including any SQLite WAL files. For an installed background service, use `service stop --account NAME`, copy the state directory, then use `service start --account NAME`. Keep a JSON export for inspection as well.

Losing the database loses your corrections and Done state; operational state is restored from the database copy, not from the export.

The state directory is for one local owner. Do not share it over a network filesystem. Independent installations do not synchronize state.

## Verification boundary

The offline fixture tests, the adapter contract tests and the Dovecot end-to-end test do not prove a real provider's `\Seen` behavior, the live OpenRouter gateway, or classification quality on representative mail. Those need a test mailbox, an API key and labeled examples.

Check thresholds against labeled mail before you turn off `review_mode`. The fake provider only exercises the plumbing, and we make no accuracy claim until the live Jev evaluation is complete.

The tests of setup and the background service use fake key tools, `launchctl` and `systemctl`; the [human check](../development/verification.md#guided-setup-on-a-real-machine-human-check) covers the real ones.

[The verification receipt](../development/verification.md) lists what has been checked; [the design](https://github.com/wir-drei-digital/mailtriage/blob/main/design/history/design.md) holds the full behavior contract.

## Practical limits

The first scan advances in bounded UID ranges from the start of each watched folder, so a large or sparse UID history may take many passes. `coverage` reports the progress.

Reconciliation of vanished messages also runs in bounded windows per pass. `reclassify --since` uses the local first-observed date, not the message date.
