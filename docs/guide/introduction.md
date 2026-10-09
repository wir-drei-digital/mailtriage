# Introduction

This guide gets mailtriage working on your own mail. You install it, set it up for your mailbox, and decide how far it goes: from only recording its decisions to filing your mail into folders.

## What mailtriage does and changes

In a busy inbox only some mail needs you, but you have to open it all to find out which. `mailtriage` is a command-line tool that classifies email, so you can look at the few messages that matter first.

For each message it records three decisions: a category from your own list, an urgency (`low`, `medium` or `high`), and whether you need to act. A colleague's question that waits for your answer might get your work category, `high` urgency and "action needed"; a newsletter might get its own category and nothing to do.

It reads mail over IMAP through the [Himalaya](https://github.com/pimalaya/himalaya) CLI and gets the decisions from OpenRouter's Decisions API with a Decisions model (Jev by default). It stores messages and results in a local SQLite database. Listing and reading work from that database without network access.

Your mail stays on your mail server, and mailtriage runs on your machine. What leaves it is what the classifier needs: each message's sender, recipients, subject, date and text, with your address, time zone and brief, and the IDs, names and descriptions of your categories.

With the `openrouter` provider, that goes to OpenRouter's Decisions API, along with the time of the request and notes on how complete the text is. [Configuration](./configuration.md#a-complete-configuration) has the details. The `fake` provider below sends nothing.

mailtriage never sends, deletes or expunges mail and never removes the read state (`\Seen`). Anything else it changes in your mailbox depends on what you turn on:

- **By default**: nothing. mailtriage does not write to the mailbox at all.
- **[Filing into folders](./filing.md)**: when an account enables it, mailtriage also creates category folders, moves mail into them and adds `\Flagged`.
- **The optional [reply queue](./filing.md#reply-queue)**: only this adds `\Seen`, to answered or done mail you approved.

`done` and `reopen` change only the local review state. Every command works on one configured account, named with `--account`.

## Try it offline

You can watch mailtriage classify a sample message before you point it at real mail. The built-in `fake` provider classifies with fixed keyword rules and makes no network request.

::: warning Important
Run the example in a scratch directory and delete the directory afterwards. The first command against an account records that account's identity in the state directory, so the example account cannot be reused for real mail.
:::

Unset `MAILTRIAGE_CONFIG` first; it comes before `./mailtriage.json` in the [config resolution order](./configuration.md#where-mailtriage-finds-the-config).

```sh
mkdir mailtriage-demo && cd mailtriage-demo
mailtriage init --json
mailtriage classify --account work --input /path/to/mailtriage/examples/reply-request.eml --format rfc822 --json
mailtriage list --account work --view attention --json
```

`init` writes `mailtriage.json` in the current directory, with the `fake` provider, one account named `work` and state in `.state/`. It refuses to overwrite an existing file (exit code 2). The later commands find that file because they run in the same directory.

`classify` reads one message, classifies it and stores the result without a mailbox location. It accepts an RFC 822 file (`--format rfc822`), the JSON shape in `examples/message.json` (`--format json`), or stdin (`--input -`). Classifying the same message again returns `outcome: cached`.
