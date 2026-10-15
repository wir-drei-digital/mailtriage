# Change your categories

Categories tell mailtriage how to sort your mail. A clear description matters more than a clever name: explain what belongs there, such as “Invoices, receipts, payments, and refunds; exclude login and security notifications.”

The easiest way to edit them is the [tray categories window](./tray.md#the-categories-window): choose **Edit categories…**, make your changes, review the summary and apply.

## Default category boundaries

New setups use the six descriptions below. Each describes the message's main purpose and includes examples and exclusions that the classifier receives.

| Category | Description sent to the classifier |
| --- | --- |
| Correspondence | Direct personal or work conversations: questions, replies, collaboration, scheduling, and support exchanges. Includes informational conversations that need no reply. Excludes automated notifications and unsolicited sales outreach. |
| Transactions | Confirmations and records of purchases or financial activity: orders, receipts, invoices, payments, refunds, subscriptions billed or renewed, account funding, and delivery updates for an order. Excludes login, verification, and security notifications. |
| Updates | Automated notifications about an account, service, or system: verification codes, login links, activation, password resets, security alerts, usage limits, workflow results, outages, and status changes. Includes factual product changes affecting an existing service. Excludes purchase records and marketing offers. |
| Newsletters | Editorial publications and recurring informational digests: articles, analysis, community news, and curated reading. Their main purpose is informing or educating. Excludes operational notifications and messages mainly promoting a purchase, upgrade, or offer. |
| Promotions | Marketing and sales messages: offers, discounts, product pitches, upgrade invitations, promotional events, and unsolicited sales outreach. Includes personalized sales emails and subscribed marketing. Excludes ordinary conversations and factual notices about an existing account. |
| Other | Mail whose main purpose fits none of the other categories. Use only when no defined category fits; verification, login, activation, and service notifications belong in Updates. |

A subscription receipt belongs in Transactions, an editorial publication in Newsletters, and a subscription discount in Promotions. An automated support-ticket status belongs in Updates; a support conversation belongs in Correspondence. Category is independent of urgency and action required: an invoice can be urgent, and a conversation can need no action.

Changing the application defaults does not replace categories in an existing account. To adopt them, export your categories and update their descriptions, keeping IDs, names, and folders. Validate and apply the file as below. Applying description changes queues open mail for classification again using your configured provider. Keep filing off while checking the new results.

## Edit from the terminal

Export the current categories:

```sh
mailtriage categories export --account work --json > categories.json
```

Edit `categories.json` in your text editor. Keep each existing `id`, even when renaming a category. Every category needs a unique ID, a name and a description. Exactly one must have `catch_all: true` for mail that fits nowhere else.

Then check and apply:

```sh
mailtriage categories validate --file categories.json --account work
mailtriage categories apply --account work --file categories.json
```

Validation changes nothing. With `--account`, it also checks folder rules and reports what applying the file would change. The file can contain a JSON array or an object with a `categories` array.

## What happens to existing mail

| Change | Result |
| --- | --- |
| Rename only (`name`) | Keeps the ID and folder. Does not classify mail again. |
| Add or remove a category; change its ID, description, examples or default status | Queues open mail for classification again using your configured provider. OpenRouter makes API requests; local Kev runs locally. |
| Change a folder | New filing uses that folder. Already-filed mail stays until you [refile it](./filing.md#refiling-after-category-changes). |

Done messages stay done. Your manual corrections survive reclassification. You cannot remove a category used by a manual correction until you change or clear those corrections.

Category `examples` are stored but are not sent to the classifier. Put the guidance the model needs in `description`. The [field reference](../reference/configuration.md#a-complete-configuration) covers all fields and folder rules.

## Classify stored mail again

To preview a manual reclassification:

```sh
mailtriage reclassify --account work --dry-run
```

To reclassify messages first observed on or after a local date:

```sh
mailtriage reclassify --account work --since 2026-09-01
```

This queues matching messages and processes up to `--limit` now (default 100, range 1 to 500). Later passes process the rest. The date refers to when mailtriage first saw a message, not the date printed on the email.

## Avoid overwriting another edit {#checking-a-change-before-you-apply-it}

If another window or agent may edit categories while you work, apply with the `digest` from your export. With `jq` installed:

```sh
mailtriage categories apply --account work --file categories.json --expect-digest "$(jq -r .digest categories.json)"
```

If the categories or filing-enabled state changed since export, this writes nothing and exits 5 with `categories_changed`. Export again and redo the edit.

::: details Digest and validation output
The digest starts with `v1:` and contains a SHA-256 of the categories and whether filing is enabled. Category order affects it; JSON key order and omitted versus null `folder` do not.

Validation with `--account` reports `changes`: `added` IDs; `removed` entries with `id` and `folder`; `renamed` and `folders_changed` entries with `id`, `from` and `to`; and `edited` categories whose description, examples or default status changed. `reclassifies: true` means applying advances `taxonomy_revision` and queues open mail again. Folder names in this result are for display.
:::
