---
layout: home

hero:
  name: mailtriage
  text: Your inbox is full. Which mail needs you?
  tagline: mailtriage reads your new mail and tells you, for every message, what it's about, how urgent it is and whether you need to act. If you want, it sorts your mailbox to match.
  actions:
    - theme: brand
      text: Get started
      link: /guide/introduction
    - theme: alt
      text: GitHub
      link: https://github.com/wir-drei-digital/mailtriage

features:
  - title: Three decisions per message
    details: A category from your own list, an urgency (low, medium or high) and whether you need to act. You see the few messages that matter first.
    link: /guide/daily-use
    linkText: Daily use
  - title: Your mailbox, sorted
    details: Optionally, mail moves into one folder per category and mail that needs you gets a flag. It works in every mail client, your phone included.
    link: /guide/filing
    linkText: Filing into folders
  - title: A reply queue
    details: Mail that needs an answer can wait in your inbox until you reply. Then it moves to its folder and is marked read once you approve it.
    link: /guide/filing#reply-queue
    linkText: Reply queue
  - title: Runs on your machine
    details: Your mail stays on your mail server. Decisions live in a local database, so listing and reading your mail work offline.
    link: /guide/introduction#what-mailtriage-does-and-changes
    linkText: What it changes
  - title: Works in the background
    details: A launchd or systemd service checks for new mail, every minute by default. A tray app shows whether each account is running and lets you edit categories.
    link: /guide/service
    linkText: Background service
  - title: Made for agents too
    details: Every command can print JSON and has documented exit codes, so an agent can triage your mail as well as you can.
    link: /agents/
    linkText: Agent guide
---

## How it works

1. **It reads your new mail.** mailtriage fetches new messages over IMAP through the [Himalaya](https://github.com/pimalaya/himalaya) CLI, once with `mailtriage sync` or continuously in the background.
2. **A model decides.** For each message, an AI model (Jev by default, through OpenRouter's Decisions API) picks the category, the urgency and whether you need to act.
3. **You see what matters.** `mailtriage list` shows the mail that needs your attention. If a decision is wrong, you correct it, and a message you've handled you mark as done.
4. **Your mailbox can follow.** Turn on filing and mail moves into your category folders. Start with a dry run: it shows the planned moves and changes nothing.

For example, a colleague asks for your feedback by Friday. That gets your work category, high urgency and "you need to act". The weekly newsletter gets its own category, low urgency and nothing to do.

## What it never does

- It never sends, deletes or expunges mail.
- It never marks mail unread. Only the optional reply queue marks mail read, and only mail you approved.
- By default it doesn't write to your mailbox at all. Filing stays off until you turn it on.

## What you need

- macOS or Linux.
- An IMAP account. Filing into folders needs a server with the MOVE extension.
- An [OpenRouter](https://openrouter.ai) API key.
- [Himalaya](https://github.com/pimalaya/himalaya) 2.1.0 or 2.2.1. If you don't have it, `mailtriage setup` offers to install one just for mailtriage.

## Install

On macOS arm64 or Linux (amd64 or arm64):

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/wir-drei-digital/mailtriage/main/install.sh | sh
```

The script installs mailtriage into `~/.local/bin` and then offers `mailtriage setup`, which asks one question at a time.

**[Get started](/guide/introduction)**: the guide explains what mailtriage changes in your mailbox and walks you through setup.

## What stays and what leaves

Your mail stays on your mail server, and mailtriage runs on your machine. What leaves it is what the classifier needs to decide: each message's sender, recipients, subject, date and text, with your address, time zone, the brief about you, and your categories (IDs, names and descriptions).

With the `openrouter` provider, that goes to OpenRouter's Decisions API. The `fake` provider, for trying mailtriage offline, sends nothing.

## Who makes mailtriage

mailtriage is created and maintained by [wirdrei.digital](https://wirdrei.digital). We build it in the open on [GitHub](https://github.com/wir-drei-digital/mailtriage), where you can report a problem or suggest a change.

---

This site follows `main`, so it can describe a feature that is newer than the latest release.
