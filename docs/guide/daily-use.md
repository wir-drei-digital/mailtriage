# Daily use

This page covers the everyday part: letting mailtriage check for new mail, then looking at what needs you and correcting what it got wrong.

The commands below omit `--config`; see [Where mailtriage finds the config](./configuration.md#where-mailtriage-finds-the-config). Pass it explicitly in scripts and supervised jobs.

## Sync and watch

```sh
mailtriage sync --account work --limit 100 --json
mailtriage watch --account work --limit 100 --interval-seconds 60 --json
```

`sync` runs one bounded pass: it scans the watched folders, fetches new messages, classifies queued ones and, with filing on, files them. `--limit` is 1 to 500 (default 100).

Its result reports `discovered`, `fetched`, `classified`, `cached`, `failed`, `pending` and `scan_errors`, plus `coverage`, with filing on `filing`, and, when the OpenRouter key is unavailable, `classification` (`{"skipped": true, "reason": "..."}`; see [The OpenRouter key](./provider.md#the-openrouter-key)).

`watch` repeats the pass every `--interval-seconds` (1 to 86400, default 60) until Ctrl-C or SIGTERM. Run it under a supervisor that restarts it, such as the [background service](./service.md).

A pass that is running when the signal arrives takes no further message and skips its filing steps, so it ends after the message it is on. That pass carries `"stopped": true` and is partial, and the next pass resumes from stored state. The systemd unit allows 120 seconds to stop (`TimeoutStopSec=120`), for a Himalaya or provider call that runs to its timeout.

With `--json` it prints one JSON line per pass and a final stop object with `passes`, `partial_passes` and `skipped_passes`. A partial pass does not stop `watch`.

A pass that `mailtriage.json` or the Himalaya configuration changed under is skipped: `watch` prints its error object (code 5), counts it in `skipped_passes` and runs the next pass with the current configuration. Any other error, including a changed account binding or a second worker, ends `watch` with that error's exit code.

After a graceful stop `watch` exits 0, or 4 if any pass was partial or skipped.

Only one worker runs per account at a time. `sync`, `classify`, `reclassify` and each `watch` pass take a lock in `state_dir`; a second one exits 5 with `an account worker is already running`. Run either `watch` or scheduled `sync` for an account, never both.

A failed fetch or classification is retried on later passes (see `policy.max_attempts`). A pass without a key takes no message and uses no attempt. A failed message whose source is still present stays in the attention view.

A message whose source occurrence has disappeared stays in the `all` view, and its local content is kept. A complete scan needs a stable mailbox UIDVALIDITY.

::: warning Important
Mail that enters and leaves a watched folder between two passes is never seen.
:::

## Query and correct

```sh
mailtriage list --account work --json
mailtriage list --account work --view all --limit 50 --json
mailtriage list --account work --category correspondence --urgency high --action-required true --json
mailtriage read --account work --id MESSAGE_ID --json
mailtriage correct --account work --id MESSAGE_ID --action-required true --json
mailtriage correct --account work --id MESSAGE_ID --category transactions --json
mailtriage correct --account work --id MESSAGE_ID --clear urgency --json
mailtriage done --account work --id MESSAGE_ID --json
mailtriage reopen --account work --id MESSAGE_ID --json
mailtriage export --account work --json > mailtriage-export.json
```

`list` and `read` use only the local database; they make no IMAP or provider request. `list` shows the attention view by default: open messages with at least one entry in `attention_reasons`, such as `high_urgency`, `medium_urgency`, `action_required`, `uncertain`, `failed`, `incomplete_decision` or `review_mode`. Done messages never appear there.

`--view all` lists every stored message. `--limit` is 1 to 500 (default 50). Pass `next_cursor` from a response as `--cursor` to get the next page. A cursor belongs to its query and to the database revision; after any change it is refused with exit code 5, and you start again from the first page.

`correct` sets an override for one decision: `--urgency`, `--category` (a category `id`) or `--action-required`. Say an invoice ended up in `correspondence`: `correct --category transactions` puts it in the right category, and it stays there.

`--clear FIELD` removes the override, and the model's decision applies again. Overrides survive reclassification. With filing on, a category correction also moves the message (see [Corrections from a mail client](./filing.md#corrections-from-a-mail-client)).

`done` marks a message handled. It leaves the attention view, stays done when the message is classified again, and nothing changes on the server. `reopen` reverses it and classifies the message again if the configuration changed since.

`export` prints all stored messages, classifications and attempts as JSON. Use it for inspection. It is not a restore format; back up the state directory instead (see [State and backups](./reference.md#state-and-backups)).
