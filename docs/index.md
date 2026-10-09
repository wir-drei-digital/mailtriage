---
layout: home

hero:
  name: mailtriage
  text: Your inbox is full. Which mail needs you?
  tagline: mailtriage reads your new mail and decides, for each message, a category from your own list, an urgency, and whether you need to act.
---

If you like, it also files your mail into one folder per category, so every mail client shows the result.

The decisions come from an AI model, by default Jev, through OpenRouter's Decisions API. mailtriage stores them in a local database, so listing and reading your mail work without network access. Every command can print JSON, so an agent can use it as well as you.

## Install

On macOS arm64 or Linux (amd64 or arm64):

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/wir-drei-digital/mailtriage/main/install.sh | sh
```

The script installs mailtriage into `~/.local/bin` and then offers `mailtriage setup`, which asks one question at a time.

**[Get started](/guide/introduction)**: the guide explains what mailtriage changes in your mailbox and lets you try it offline first.

## What stays and what leaves

Your mail stays on your mail server, and mailtriage runs on your machine. What leaves it is what the classifier needs to decide: each message's sender, recipients, subject, date and text, with your address, time zone, a one-line brief about you, and your category names and descriptions.

With the `openrouter` provider, that goes to OpenRouter's Decisions API. The `fake` provider, for trying mailtriage offline, sends nothing.

---

This site follows `main`, so it can describe a feature that is newer than the latest release.
