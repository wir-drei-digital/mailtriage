# Daily use

The usual routine is simple: let mailtriage check for new mail, review what needs attention, and mark messages done when you have handled them. Replace `work` in these examples with your account name.

Commands print formatted JSON by default. Add `--json` when a script or agent needs one JSON object per line.

## Keep your mail up to date {#sync-and-watch}

If you enabled the [background service](./service.md) during setup, it checks for new mail every minute by default. You can leave it running and go straight to reviewing your mail.

Without a service, run one check:

```sh
mailtriage sync --account work --limit 100
```

Or keep checking in a terminal until you press Ctrl-C:

```sh
mailtriage watch --account work --interval-seconds 60
```

Use one worker per account: the service, `watch`, or individual `sync` runs. Running two at once causes a lock conflict.

The first checks work through existing mail. A large inbox may take many passes. Mail that enters and leaves a watched folder between checks will not be seen.

## Find and read a message {#query-and-correct}

```sh
mailtriage list --account work
```

This shows messages that need attention, including urgent mail, action items and uncertain decisions. While `policy.review_mode` is on, all open mail appears here so you can check the classifier's work.

To see everything stored locally, including done messages:

```sh
mailtriage list --account work --view all --limit 50
```

Copy a message's `id` from the result, then read it:

```sh
mailtriage read --account work --id MESSAGE_ID
```

`list` and `read` work from local storage. They do not contact your mailbox or mark messages read.

## Mark a message done

After you have handled it:

```sh
mailtriage done --account work --id MESSAGE_ID
```

The message leaves the attention list but remains in `--view all`. Done does not delete, archive or mark it read on the mail server. If you enabled the [reply queue](./filing.md#reply-queue), Done also lets a held message be filed on a later pass.

To bring it back into review:

```sh
mailtriage reopen --account work --id MESSAGE_ID
```

## Correct a decision

For example, put an invoice in Transactions:

```sh
mailtriage correct --account work --id MESSAGE_ID --category transactions
```

You can also correct urgency or whether action is needed:

```sh
mailtriage correct --account work --id MESSAGE_ID --urgency high
mailtriage correct --account work --id MESSAGE_ID --action-required true
```

A correction stays in place when the message is classified again. It changes this message only; it does not train the model. With live filing, a category correction also requests a move to that category's folder.

To remove a correction and use the model's decision again:

```sh
mailtriage correct --account work --id MESSAGE_ID --clear urgency
```

See [categories](./categories.md) to improve how future mail is sorted, or [troubleshooting](./troubleshooting.md) when a check fails. The [sync and query reference](../reference/daily-use.md) covers filters, pagination, retries and export.
