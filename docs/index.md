---
layout: home

hero:
  name: mailtriage
  text: Find the mail that needs you.
  tagline: Classify incoming email, review what needs attention, and optionally file it into folders in your usual mail client.
  image:
    src: /mascot.webp
    alt: The mailtriage mascot, an orange envelope looking through a magnifying glass
  actions:
    - theme: brand
      text: Get started
      link: /guide/install
    - theme: alt
      text: How it works
      link: /guide/introduction

features:
  - title: Review what matters
    details: Each message gets a category, an urgency and a decision about whether you need to act. Correct a decision or mark a message done.
    link: /guide/daily-use
    linkText: Daily use
  - title: Check mail in the background
    details: A service checks for new mail. The tray app shows account status and lets you edit categories.
    link: /guide/tray
    linkText: Use the tray app
  - title: Preview filing first
    details: See which messages would move into category folders before enabling changes in your mailbox.
    link: /guide/filing
    linkText: File mail into folders
---

<span id="install"></span>
<span id="what-you-need"></span>

## Get started

You need macOS on Apple silicon or Linux on amd64 or arm64, an IMAP mailbox, and an OpenRouter API key.

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/wir-drei-digital/mailtriage/main/install.sh | sh
```

The installer offers to run setup, which connects your mailbox and key. Setup starts with a filing preview that makes no mailbox changes.

[Installation options](./guide/install.md) · [Setup walkthrough](./guide/setup.md) · [Troubleshooting](./guide/troubleshooting.md)

## How it works

mailtriage reads mail through Himalaya, asks an AI model through OpenRouter to classify it, and saves message text and decisions in a local database. You can then review messages in a terminal or let an agent work with them.

<span id="what-stays-and-what-leaves"></span>

**Message content is sent to OpenRouter for classification.** The app runs locally, but the default classifier is an external service. See [where your data goes](./guide/introduction.md#where-your-data-goes).

<span id="what-it-never-does"></span>

mailtriage never sends or deletes mail. Optional live filing moves mail and adds flags. Live filing still needs [provider verification](./guide/provider-check.md); no listed provider has a recorded go yet.

## Find the right documentation

- [User guide](./guide/introduction.md): understand mailtriage, set it up and use it day to day.
- [Technical reference](./reference/index.md): exact flags, configuration fields and command behavior.
- [Agent guide](./agents/index.md): integrate mailtriage into an agent's workflow.
- [Development](./development/index.md): work on the code, test it and publish releases.

<span id="who-makes-mailtriage"></span>

Created by [wirdrei.digital](https://wirdrei.digital). Report issues and contribute on [GitHub](https://github.com/wir-drei-digital/mailtriage).

This site follows `main`, so it may describe features newer than the latest release.
