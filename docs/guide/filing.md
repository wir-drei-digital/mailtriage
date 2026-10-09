# File mail into folders

Filing makes mailtriage's categories visible in your usual mail client. In live mode, it creates category folders, moves eligible messages into them, and flags mail that needs action or has high urgency.

::: warning Start with a preview
No listed mail provider has a recorded live-filing go yet. Keep filing in `dry_run` until your provider has passed the [compatibility check](./provider-check.md).
:::

## Choose a mode

| Mode | What it does |
| --- | --- |
| `off` | Classifies mail locally. Makes no mailbox changes. |
| `dry_run` | Also plans folders, moves and flags. Makes no mailbox changes. Setup chooses this for new accounts. |
| `live` | Creates folders, moves eligible mail and adds flags. |

An account without a filing setting uses `off`. Filing requires a configured mail engine and an IMAP server with MOVE support.

## Preview before going live {#rollout}

Enable dry-run mode and inspect the plan:

```sh
mailtriage filing enable --account work --mode dry-run
mailtriage filing plan --account work
```

Let the service run for a few days and check that messages have sensible categories. The plan shows the next pass's actions, not the whole backlog.

Only after your provider has a recorded go, enable live filing:

```sh
mailtriage filing enable --account work --mode live
```

To stop filing:

```sh
mailtriage filing disable --account work
```

Disabling filing stops future mailbox changes; it does not undo earlier moves. Classification continues.

## Which messages move {#what-is-moved-and-flagged}

Automatic filing applies to new mail in your watched source folders, usually `INBOX`. “New” means its server arrival date is at or after the time filing was enabled. Mail received during dry-run mode is included when you go live.

A message needs a current classification based on complete input, or your category correction. Review mode does not prevent live filing.

To include older mail, preview a backfill first:

```sh
mailtriage filing backfill --account work --days 30
```

Once you have checked the preview, repeat with `--apply` while filing is live. The following passes make the moves in batches.

## Choose folders {#folders}

Each category has a folder. Setup uses its category name, such as `Transactions`. Renaming the category later leaves the folder unchanged. Set a category's folder to the literal `INBOX` to keep its mail in the source folder.

Use simple, unique ASCII names without leading or trailing spaces. Folder names cannot contain `/ . * % " \ &`, start with `-`, or exceed 200 bytes. Names are compared without regard to case, and variants of `inbox` other than `INBOX` are not allowed.

mailtriage never uses special folders such as Sent, Trash or Junk for a category. If it cannot establish an existing folder's role, it waits for you to check and adopt it. See the [folder reference](../reference/filing.md#folders) for server namespaces and source-folder rules.

## Correct a filing decision {#corrections-from-a-mail-client}

With filing enabled, you can correct decisions in your mail client:

| Your action | How mailtriage treats it |
| --- | --- |
| Move mail into another category folder | Records a category correction. |
| Move mail back to a source folder | Pins it there so automatic filing does not move it again. |
| Archive or delete mail | Marks it done locally after a complete scan confirms it has left all watched folders. |

You can also [correct a category from the terminal](./daily-use.md#correct-a-decision). To release a pin:

```sh
mailtriage filing unpin --account work --id MESSAGE_ID
```

## Keep unanswered mail in the inbox {#reply-queue}

The optional reply queue holds new mail that needs action in its source folder until you reply or mark it done. In live mode, enable it with:

```sh
mailtriage filing enable --account work --mode live --reply-queue on
```

After your reply, mailtriage moves the message to its category folder **without changing its read state**. To review and approve marking these messages read:

```sh
mailtriage filing replies --account work
mailtriage filing replies --account work --approve --id MESSAGE_ID
```

`--approve` without an ID approves every waiting message. The next live pass marks approved mail read.

Your mail client must set the IMAP `\Answered` flag when you reply; test each client you use. If it does not, use `mailtriage done --account work --id MESSAGE_ID` to release the message. See the [reply queue reference](../reference/filing.md#reply-queue) for pins, backfilled mail and disabling the queue.

## Move mail after changing categories {#refiling-after-category-changes}

Changing categories does not immediately move mail already filed. Let open mail be classified again, then preview:

```sh
mailtriage filing refile --account work
```

Add `--apply` to queue the matching moves when filing is live. Mail you moved, corrected, pinned or marked done stays where it is. The tray's **Move filed mail…** panel provides the same preview and action.

## Check progress {#status-and-log}

```sh
mailtriage filing status --account work
mailtriage filing log --account work --limit 50
```

Status shows blocked messages, paused folders and the last pass. Check the mailbox before retrying a blocked action. The [recovery table](../reference/filing.md#lifting-blocks-and-pauses) explains each case.

## What filing never does {#safety-rules}

mailtriage never deletes or expunges mail, removes flags, or renames or deletes folders. It marks mail read only through reply-queue approval. It pauses writes when it cannot establish that a folder or message is safe to use.

The [filing reference](../reference/filing.md) covers all commands, safeguards, output fields and exit codes.

## Detailed reference

- <span id="lifting-blocks-and-pauses"></span>[Lifting blocks and pauses](../reference/filing.md#lifting-blocks-and-pauses)
- <span id="filing-exit-codes"></span>[Filing exit codes](../reference/filing.md#filing-exit-codes)
