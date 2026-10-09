---
layout: home

hero:
  name: mailtriage
  text: An inbox that sorts itself.
  tagline: Keep the mail apps you already use. mailtriage tidies your existing mailbox in the background. With live filing enabled, it sorts and flags mail on your mail server, so the changes sync to every connected mail client.
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
  - title: One mailbox, all your mail apps
    details: Filing changes happen in your existing mailbox. The same folders, messages and flags appear on your phone, desktop and webmail as each client syncs.
    link: /guide/filing
    linkText: How filing works
  - title: Check mail in the background
    details: A service checks for new mail. The tray app shows account status and lets you edit categories.
    link: /guide/tray
    linkText: Use the tray app
  - title: Preview filing first
    details: See which messages would move into category folders before enabling changes in your mailbox.
    link: /guide/filing
    linkText: File mail into folders
---

## Keep using your mail app

mailtriage is not a new email app or a replacement for your inbox. It is a worker that runs on your computer, checks incoming mail and, when you enable live filing, moves it into category folders and flags what needs your attention.

Those changes happen on your mail server. Every mail client connected to that mailbox sees them when it syncs. Keep reading, replying and searching in the apps you already use; mailtriage handles the sorting in the background.

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

mailtriage reads mail through Himalaya and asks an AI model through OpenRouter to choose a category, urgency and whether you need to act. With live filing enabled, it uses those decisions to organize your mailbox. It also keeps a local record that you or an agent can inspect and correct from the terminal.

<span id="what-stays-and-what-leaves"></span>

**Message content is sent to OpenRouter for classification.** The worker runs locally, but the default classifier is an external service. See [where your data goes](./guide/introduction.md#where-your-data-goes).

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
