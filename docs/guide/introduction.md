# Introduction

## What mailtriage does and changes

`mailtriage` is a command-line tool that classifies email. For each message it records three decisions: a category from your own list, an urgency (`low`, `medium` or `high`), and whether you need to act. It reads mail over IMAP through the [Himalaya](https://github.com/pimalaya/himalaya) CLI, gets the decisions from OpenRouter's Decisions API with a Decisions model (Jev by default), and stores messages and results in a local SQLite database. Listing and reading work from that database without network access.

mailtriage never sends, deletes or expunges mail and never removes the read state (`\Seen`). By default it does not write to the mailbox at all. An account that enables [filing into folders](./filing.md) also creates category folders, moves mail into them and adds `\Flagged`. Only the optional [reply queue](./filing.md#reply-queue) adds `\Seen`, to answered or done mail you approved. `done` and `reopen` change only the local review state. Every command works on one configured account, named with `--account`.

## Try it offline

The built-in `fake` provider classifies with fixed keyword rules and makes no network request. Use it in a scratch directory and delete the directory afterwards: the first command against an account records that account's identity in the state directory, so the example account cannot be reused for real mail. Unset `MAILTRIAGE_CONFIG` first; it comes before `./mailtriage.json` in the [config resolution order](./configuration.md#where-mailtriage-finds-the-config).

```sh
mkdir mailtriage-demo && cd mailtriage-demo
mailtriage init --json
mailtriage classify --account work --input /path/to/mailtriage/examples/reply-request.eml --format rfc822 --json
mailtriage list --account work --view attention --json
```

`init` writes `mailtriage.json` in the current directory, with the `fake` provider, one account named `work` and state in `.state/`. It refuses to overwrite an existing file (exit code 2). The later commands find that file because they run in the same directory. `classify` reads one message, classifies it and stores the result without a mailbox location. It accepts an RFC 822 file (`--format rfc822`), the JSON shape in `examples/message.json` (`--format json`), or stdin (`--input -`). Classifying the same message again returns `outcome: cached`.
