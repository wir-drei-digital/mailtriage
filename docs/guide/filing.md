# Filing into folders

Filing makes the classification visible in every mail client. With filing on, each classified message in a source folder (the engine's `mailboxes`, usually `INBOX`) is moved into a top-level folder for its category, and mail that needs action or has `high` urgency gets `\Flagged`. Filing is off by default and is enabled per account; `mailtriage setup` turns on `dry_run` for a new account unless you pass `--filing off`. Filing needs an `engine`; `filing enable` refuses an account without one.

```sh
mailtriage filing enable --account work --mode dry-run --json
mailtriage filing plan --account work --json
mailtriage filing enable --account work --mode live --json
mailtriage filing backfill --account work --days 30 --json
mailtriage filing backfill --account work --days 30 --apply --json
mailtriage filing status --account work --json
mailtriage filing log --account work --limit 50 --json
```

`filing.mode` has three values:

| Mode | Effect |
| --- | --- |
| `off` | The default when the account has no `filing` block. The mailbox is left exactly as before. |
| `dry_run` | Each pass watches the category folders and plans moves and flags, and writes nothing: no folder is created, no message is moved or flagged. `sync` reports the planned counts under `filing.planned`, and `filing plan` lists the actions. |
| `live` | mailtriage creates and subscribes the category folders, moves mail and adds flags. |

`filing enable --mode dry-run` or `--mode live` and `filing disable` write `filing.mode` into `mailtriage.json` under the configuration lock (`dry-run` on the command line is `dry_run` in the file). The next pass uses the new mode.

## Rollout

1. Run `filing enable --mode dry-run`, or keep the `dry_run` that setup chose.
2. Keep `watch` or scheduled `sync` running for a few days. Check `filing plan` and the `filing.planned` counts of each pass. `filing plan`'s `total` counts the actions of the next pass only, at most `filing.max_actions_per_pass`; it is not the size of the backlog.
3. Check that your provider has a recorded go (see [Provider check](./provider-check.md)).
4. Run `filing enable --mode live`.
5. Optionally file older mail with `filing backfill`.

Only new mail is filed automatically: mail whose server internal date is at or after the time filing was enabled. Mail delivered during the dry-run days counts as new, so the first live passes file it. Mail that arrives while filing is off counts as older mail when filing is enabled again.

To file older mail, preview it with `filing backfill --days N` or `filing backfill --all`, then repeat the command with `--apply`. `--days` (1 to 3650) compares against the server's internal date. `--apply` requires `live` and makes each listed message eligible once. The moves happen over the following passes, at most `filing.max_actions_per_pass` (default 200) per pass.

## Folders

`filing enable` writes an explicit `folder` (the category's current `name`) into every category that has none, so a later rename does not move its folder. `categories apply` with filing on keeps each existing category's folder the same way. `mailtriage setup` writes an explicit `folder` for each default category.

A category folder must:

- be a single path segment of printable ASCII, at most 200 bytes, without leading or trailing spaces,
- contain none of `/ . * % " \ &` and not start with `-`,
- not be a case variant of `inbox`, other than the literal `INBOX`, which keeps the category's mail in its source folder,
- be unique among the account's categories, ignoring case.

With filing on, the source mailboxes in `engine.mailboxes` must be printable ASCII without `\`, `"` or `&` and must not start with `-`. They may contain `/` or `.`. `filing enable` and `categories apply` refuse invalid folders with `categories need a valid folder: <ids>`; set `folder` for those categories in the category file and run `categories apply`. They refuse unsafe source mailboxes with `source mailboxes are not safe to file from: ...`.

mailtriage adds the server's personal namespace prefix (for example `INBOX.`) to folder names. An existing folder with a category's name is adopted when the server reports that it has no special role. When the server cannot report roles, the folder waits for `filing adopt --folder NAME`. Folders with a special role (Sent, Trash, Junk, Archive, All Mail and similar, by role or by name) are never used, and a deleted category folder is not recreated. `filing status` lists the affected categories under `paused_categories`.

## What is moved and flagged

A message is moved automatically at most once, only out of a source folder, and only when its classification is current or you corrected its category. A current classification was made under the current categories, provider and policy, from complete input. Review mode does not hold filing back.

`\Flagged` is added once, when the effective decision is `action_required` or `high` urgency, and only to new mail, backfilled mail or mail that mailtriage filed. A message that already carries `\Flagged` counts as flagged, so removing the flag in a mail client is respected. Set `filing.flag` to `false` to add no flags. With the [reply queue](#reply-queue) on, a message it holds is flagged only for `high` urgency, and mail it filed after your reply is not flagged, even once the queue is off; other mail is flagged as without the queue.

## Reply queue

With the reply queue, the inbox holds exactly the mail that still needs you. New mail whose effective decision is `action_required` stays in its source folder instead of moving to its category folder. Once you have answered it, mailtriage moves it to its category folder, unread, and lists it for your approval. Only mail you approve is marked read.

```sh
mailtriage filing enable --account work --mode live --reply-queue on
mailtriage filing status --account work --json
mailtriage filing replies --account work
mailtriage filing replies --account work --approve
```

`--reply-queue off` turns it off; without the flag, `filing enable` keeps the configured value (`filing.reply_queue`, default `false`). With the queue off, held mail files like other mail, and mail you already approved in `filing replies` is still marked read. A config with the queue on is written as schema 4, which mailtriage releases without the reply queue refuse to read.

- **Answered.** A mail client sets `\Answered` when you reply from it. Each pass reads the flags of the held mail again, and the next pass after your reply moves it to its category folder without changing its read state.
- **Approval.** `filing replies` lists the answered or done mail that mailtriage filed, or is filing, and has not marked read: `id`, `subject`, `from`, `folder` (the source folder until the next pass confirms the move), `answered`, `requested_at` and `approved_at`. Mail whose move is given up (it failed, the mail vanished, or you reopened it first) leaves the list, even if you approved it. `--approve` approves all of it, `--approve --id ID` (repeatable) single messages; an id that is not waiting fails with exit code 2 and approves nothing. The next live pass adds `\Seen` to approved mail where it is now, after checking that its UID still names it; if it does not, the pass reports `read_mismatch` and the message waits.
- **Done.** `mailtriage done --account work --id ID` releases a held message the same way, into the approval list. Use it for a reply sent from another program or device that does not set `\Answered`, for a paid invoice, or when no answer is needed after all. `reopen` before the move has happened keeps the message held and takes it off the approval list.
- **Only new mail.** Mail that was in the folder before filing was enabled stays where it is, and backfilled mail files as before. A held message whose decision changes to "no action" files like other mail and is not listed.
- **Your own moves win.** Moving a held message into a category folder is a correction, archiving it means done, and `filing pin --id ID` keeps it in the inbox even after your reply.
- **Visibility.** `filing status` reports `reply_queue`, `awaiting_reply`, `awaiting_reply_ids` (up to 50), `read_waiting` and `read_approved_pending`. A pass reports `awaiting_reply`, `reply_exits`, `replies_checked` and `reads_applied` when they are not 0, and `filing plan` marks a reply exit's move with `"reason": "reply_exit"`.

Before you rely on it, check that the mail clients you answer from set `\Answered`: reply from each one to a test message and run `mailtriage filing plan --account work --json`.

## Corrections from a mail client

Moving a message into another category's folder is a category correction, the same as `correct --category`. Moving it back to a source folder pins it there; it is not filed again until `filing unpin --id ID`. Moving it into its own category's folder keeps it there and removes any pin. Mail delivered straight into a category folder, for example by a server rule, is recorded as filed by you in that category.

With filing on, `correct --category` and `correct --clear category` move the message to the resulting category's folder. `filing pin --id ID` keeps a message in its source folder and moves it back if it was filed.

Archiving or deleting a message in a mail client means done. Once the message is gone from every watched folder, a later pass that fully scanned every watched folder, with no paused folder and no unidentified arrival outstanding, marks it done locally. `list --view all` then shows it with `review_state` `done`. If the message comes back, that inferred done is reopened. A message you marked done yourself is never reopened by observation.

## Status and log

`filing status` makes no mailbox calls. It reports:

| Field | Content |
| --- | --- |
| `mode`, `state_mode`, `enabled_at` | The configured mode, the mode the last pass stored, and when filing was enabled. |
| `capabilities` | The server's MOVE, UIDPLUS and SPECIAL-USE support from the last pass. |
| `folders` | Every folder with its state, pause and subscription. |
| `paused_categories` | Categories whose folder cannot be used. |
| `intents` | Move and flag intents counted by state, finished ones included. |
| `blocked`, `blocked_ids` | Blocked messages; up to 50 entries of `{id, blocked_reason}`. |
| `quarantined`, `quarantined_ids` | Quarantined messages; up to 50 ids. |
| `ambiguous`, `ambiguous_ids` | Messages found in several watched folders; up to 50 ids. |
| `unresolved_arrivals`, `unresolved_arrival_items` | Mail mailtriage could not identify; up to 50 items. |
| `eligible_unfiled` | Messages in a source folder that the next passes may file. |
| `reply_queue`, `awaiting_reply`, `awaiting_reply_ids` | Whether the [reply queue](#reply-queue) is on, and the messages it holds; up to 50 ids. |
| `stale_requests` | `count` and `ids` of requests for a category that no longer exists. Correct or unpin them. |
| `alias_conflicts`, `problems`, `last_pass` | Folder alias conflicts, problems of the last pass, and its summary. |

`filing log` lists filing events, newest first; `--id ID` limits it to one message. `list` and `read` items carry a `placement` object: `folder`, `location_state`, `filed_by`, `pinned`, `flagged`, `blocked_reason` and `pending_action`. With filing on, `doctor` adds a `filing` block with the server's capabilities, folders and problems.

## Safety rules

mailtriage never deletes or expunges mail, never removes a flag or `\Seen`, and never renames or deletes a folder. It adds `\Seen` only with the [reply queue](#reply-queue), to answered or done mail you approved. It writes only to servers with the IMAP MOVE extension; there is no copy-and-delete fallback. Every move and flag is journaled before it is sent, and each `\Seen` write is recorded on its approval entry; all are resolved by observation after a failure or crash. If a write may have run against a recreated mailbox, mailtriage reverts a move where the server reported where the mail went. Otherwise it pauses that folder and quarantines the mail that arrived. A flag or `\Seen` that may have reached another message stays there and is reported as `epoch_race` in `filing log`. Nothing is written to a paused folder or a blocked message until you act.

A Himalaya mailbox alias that points a watched folder elsewhere stops that folder's scanning, fetches and reply queue checks, and all filing writes, until the alias is fixed. A change to the Himalaya configuration during a pass, or to `mailtriage.json` before the pass writes, aborts the pass with exit code 5. `sync` exits with that code; `watch` prints the error object, continues, and uses the new configuration in its next pass.

## Lifting blocks and pauses

Check the mailbox before you lift a block or a pause.

| Case | Meaning | What to do |
| --- | --- | --- |
| Folder pause (`pause_reason` in `folders`) | A write may have run against a recreated folder. | After checking the folder, `filing retry --folder NAME` releases the pause and rescans the folder. |
| Folder in state `needs_confirmation` | The server could not report the folder's role. | `filing adopt --folder NAME` confirms it. |
| `move_failed` | The move kept failing. | `filing retry --id ID` lifts the block and makes the message eligible once, so a message still in a source folder is filed, and possibly flagged, on the next pass. |
| `quarantined` | The message arrived while a write may have run against a recreated mailbox. | `filing retry --id ID` lifts it, with the same effect as for `move_failed`. |
| `duplicate_copy` | The message is in two folders. | Delete one copy in your mail client, run `sync` so mailtriage observes it, then run `filing retry --id ID`. While both copies are recorded, the retry is refused with exit code 5 (`remove one copy first, then sync and retry`). Lifting `duplicate_copy` does not make the message eligible once. If the copy you kept is in a source folder and the message is still new mail, the next pass files it again; `filing pin --id ID` keeps it in the source folder instead. `filing pin` and `correct --category` do not lift this block. |
| `merge_conflict` | An unresolved arrival duplicates this message. `filing log --id ID` shows the arrival in `detail.arrival_id`. | `filing retry --id ID` does not lift this block. `filing dismiss --arrival N` marks the arrival reviewed and lifts it. `filing retry --arrival N` fetches the arrival again instead; a repeated conflict blocks again. |
| Unresolved arrival | Mail that mailtriage could not identify. | `filing retry --arrival N` fetches it again; `filing dismiss --arrival N` marks it reviewed. |

## Refiling after category changes

Automatic filing moves mail only out of the source folders. When you add, remove or re-point categories, the mail that mailtriage already filed stays where it is until you refile it. `filing refile` shows which filed mail would follow its new category, and `--apply` lets the next passes move it:

```sh
mailtriage filing refile --account work --json
mailtriage filing refile --account work --category updates --json
mailtriage filing refile --account work --folder Promotions --apply --json
```

Refiling moves only mail that is still exactly where a mailtriage move put it, as the server confirmed when the move ran. Mail you moved (even back into the same folder), corrected, pinned or marked done stays where it is, and nothing is ever refiled into `INBOX` or another source folder. Mail filed before this version without a server-reported destination (COPYUID) cannot be refiled.

| Change | What to do |
| --- | --- |
| Add a category | `categories apply`, let a few passes classify open mail again, then preview with `filing refile --category NEW` and move with `--apply`. |
| Rename a category (`name` only) | Nothing moves; its folder stays. |
| Re-point its `folder` | `categories apply`. The next live pass creates the new folder and retires the old one. `filing refile --folder OLD --apply` moves the old folder's mail, including mail still being classified again. |
| Remove a category | `categories apply` retires its folder and classifies its open mail again. `filing refile --folder OLD --apply` moves that mail into the remaining categories' folders once it is classified. |

With filing on, `categories apply` returns a `hint` naming the command. The preview makes no mailbox calls and reports:

| Field | Content |
| --- | --- |
| `candidates` | Messages that would move, by folder and UID, at most `--limit` (1 to 500, default 50): `id`, `folder` and `target` (server folder names), `category` (the new category) and `reason` (`category_changed`, or `folder_retired` when no category uses the folder any more). |
| `total` | All candidates, without the limit. |
| `folders` | One entry per folder holding candidates or waiting mail: `folder` (the configured name, `null` when unknown), `native` (the server name; pass it to `--folder`), `retired`, `candidates`, `waiting`. |
| `waiting` | Messages still to be classified again; whether they move is decided then. Mail whose classification failed also counts, and a mark on it stays pending, until it is classified again (`reclassify` queues it again). |
| `skipped` | What stays, counted by reason (below). |

`--folder NAME` takes a folder's server name or its configured name; a configured name two folders share exits 2 and lists their server names. `--category ID` limits the candidates to one new category; waiting mail is reported but not marked, so run the command again once it is classified. With `--folder` and no `--category`, `--apply` also marks the waiting mail, which then moves once classified. `--apply` needs `live`, marks the whole matching set whatever `--limit` says, and is safe to repeat: `marked` and `waiting_marked` count only new marks. The moves happen over the following passes, at most `filing.max_actions_per_pass` per pass, with every safeguard of other moves; a refile adds no flag.

| `skipped` reason | Meaning |
| --- | --- |
| `not_filed_by_mailtriage` | The message is not where a mailtriage move put it: you moved it, a rescan or recovery placed it, or it was filed without COPYUID. |
| `corrected`, `pinned`, `done` | Your correction, pin or Done wins. |
| `blocked`, `open_intent` | A block or an unfinished move; see `filing status`. |
| `explicit_target` | An explicit move request is pending; if its category was removed, `correct --account NAME --id ID --category NEW` replaces it. |
| `multiple_copies` | It is in more than one folder. |
| `incomplete_input` | It was classified from incomplete content. |
| `retired_frozen` | It is in a retired folder mailtriage no longer watches. |
| `target_unusable` | Its new folder is paused, missing or awaiting `filing adopt`. A marked message waits for it. |
| `target_inbox_or_source` | Its new category keeps mail in `INBOX` or a source folder. |

A retired folder stays watched while it holds mail that mailtriage filed there and you have not moved, corrected, pinned or marked done, including mail you never refiled and mail that would not move; refile its mail to let it stop being watched. When none is left, it stays watched until mailtriage has scanned everything moved into it before then. After that it is no longer watched and the mail in it is not refiled. mailtriage never renames or deletes it; delete the emptied folder in your mail client when you like.

A mark is dropped, with a `refile_cleared` event naming the reason, when the message is corrected, pinned, marked done, moved, copied, or classified into its own folder, into `INBOX` or a source folder, or from incomplete content. `filing status` reports `refile_marked` and `refile_candidates`. `filing log` shows `refile_marked`, `refile_cleared`, `refile_cancelled` and `moved` events with `"reason": "refile"`.

Exit codes: 2 for `--apply` outside `live`, an unknown `--category`, a `--folder` that names no category or retired folder (or two of them), a `--limit` outside 1 to 500, or an unknown account; 3 when the state database is unavailable; 5 when `mailtriage.json` changed during `--apply` or the placements kept changing.

## Filing exit codes

| Code | Filing cases |
| --- | --- |
| 0 | Success. |
| 2 | Invalid input or configuration: no mail engine, invalid folders, `backfill --apply` outside `live`, an unknown folder or message, an arrival that is not unresolved. |
| 3 | Operational failure. |
| 4 | Partial sync. Any filing error in a pass makes it partial. |
| 5 | Conflict: the configuration changed during the command or pass, a placement changed concurrently, a message's identity is not yet established, a `duplicate_copy` retry while both copies are still recorded, or a changed account binding. |
