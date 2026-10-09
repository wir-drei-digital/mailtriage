# What is mailtriage

mailtriage helps you find the email that needs your attention. It runs on your computer, checks your mailbox, and uses an AI model to give each message a category, an urgency, and an answer to “Do I need to act?”

For example, a customer's question might need a reply today. A receipt belongs in Transactions but needs no action. You can review those decisions in a terminal, correct them, and mark messages done.

## What mailtriage does and changes

mailtriage reads your mailbox through [Himalaya](./himalaya.md), a command-line email tool. It saves message text and decisions locally, so you can list and read messages even when you are offline.

You choose how much it changes in your mailbox:

| Mode | What happens |
| --- | --- |
| Classification only (`off`) | Records decisions locally. Makes no mailbox changes. |
| Preview filing (`dry_run`) | Also shows which folders, moves and flags it would use. Makes no mailbox changes. This is what setup chooses for a new account. |
| Live filing (`live`) | Creates category folders, moves eligible mail and flags messages that need attention. You must enable this yourself. |

mailtriage never sends or deletes mail. It preserves read state, except when you explicitly approve marking mail read through the optional [reply queue](./filing.md#reply-queue).

Live filing needs a verified mail provider. **None of the listed providers has a recorded go yet**; use dry-run mode for now. See [mail provider compatibility](./provider-check.md).

## Where your data goes

With the default OpenRouter provider, mailtriage sends message headers and body text to OpenRouter's Decisions API for classification. It also sends your address, time zone, brief and category definitions. The [configuration reference](../reference/configuration.md#a-complete-configuration) describes the fields and text limits.

Your mailbox remains on your mail server. The local database holds message text, decisions and your corrections. The offline `fake` provider sends nothing, but uses simple keyword rules rather than an AI model.

## Get started

1. [Install mailtriage](./install.md).
2. [Run setup](./setup.md) to connect your mailbox and OpenRouter key.
3. [Review your mail](./daily-use.md) and adjust your categories.

The examples use an account named `work`. Replace it with the name you choose during setup.

## Try it offline

If you have a copy of the repository, you can classify a sample message without connecting a mailbox:

```sh
mkdir mailtriage-demo
cd mailtriage-demo
mailtriage init --config ./mailtriage.json
mailtriage classify --config ./mailtriage.json --account work --input /path/to/mailtriage/examples/reply-request.eml --format rfc822
mailtriage list --config ./mailtriage.json --account work
```

Replace the sample path with the path to your checkout. `init` creates a demo account using the `fake` provider and stores its data in `.state/`.

Keep this demo directory separate from your real setup, and delete it when finished. Once used, the demo account is bound to its sample identity and cannot be reused for a different mailbox.
