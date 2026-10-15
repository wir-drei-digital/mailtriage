# Set up your mailbox

After [installing mailtriage](./install.md), run:

```sh
mailtriage setup
```

Setup asks one question at a time. Press Enter to accept the value shown in brackets. It saves your answers in `~/.config/mailtriage/mailtriage.json` and checks that the configuration is ready.

## What to choose

| When setup asks about | Start with |
| --- | --- |
| Himalaya | Accept the offer to install a tested copy if needed. Choose your existing IMAP account, or use the offered Himalaya wizard to add one. |
| Account name | A short name such as `work`. You will use this with `--account`. |
| Your details | Check your email address and time zone. Add a brief such as “I manage customer projects. Customer questions and invoices usually need my attention.” |
| Folders to watch | `INBOX`. You can add more later. |
| Classifier | OpenRouter and the default Jev model. On an Apple Silicon Mac, [local Kev](./local-kev.md) keeps your mail on your Mac and needs no key. Use `fake` only for offline testing. |
| API key | Use a key store for OpenRouter or a Kev server on another machine. Local Kev needs no key. See below. |
| Filing | Keep `dry_run`. It previews moves without changing your mailbox. |
| Background service | Accept if you want mailtriage to check for new mail automatically. |

A new account gets six categories: Correspondence, Transactions, Updates, Newsletters, Promotions and Other. Their [descriptions](./categories.md#default-category-boundaries) include examples and exclusions. New configurations use [best-guess selection](./configuration.md#category-and-urgency-selection) for category and urgency; existing configurations without a mode keep strict behavior. You can edit categories and selection settings later. Local Kev uses a [configurable 5-minute idle timeout](./local-kev.md#change-the-idle-timeout).

Setup never enables live filing. Automatic updates are on by default; [change the update mode](./updates.md#modes) if needed.

## Key stores

Use macOS Keychain on macOS, or Secret Service or `pass` on Linux. The store's own tool asks for your OpenRouter or remote Kev key. mailtriage saves only the command that reads it, never the key itself.

A key store also works for the background service. A variable exported in your terminal does **not** reach that service. See [Classifier and models](./provider.md) for other options.

## Check the result

Setup ends with a readiness check. If an item says “not ready”, follow the fix printed beside it. Setup can finish successfully even when an item still needs attention.

To check again, replacing `work` with your account name:

```sh
mailtriage doctor --account work
```

Look for `ready: true` and `transport.configured: true`. `doctor` checks the configuration and key availability. For Kev it checks the server's model list without starting local Kev; for OpenRouter the first classification tests the connection.

If you accepted the background service, check it and view the mail it has processed:

```sh
mailtriage service status --account work
mailtriage list --account work
```

If you skipped the service, run a small first pass yourself:

```sh
mailtriage sync --account work --limit 20
mailtriage list --account work
```

Do not run `sync` alongside the service for the same account. The first passes work through existing mail, so a large inbox takes time. Each classification uses your chosen provider: an API request for OpenRouter or your own Kev server, or local inference for local Kev.

## Updating an account

Run setup again to change watched folders, your brief, or other settings:

```sh
mailtriage setup --update --account work
```

Setup keeps the account's categories and uses its current settings as defaults. It also keeps an existing live filing mode unless you change it.

To add another mailbox, use a new account name:

```sh
mailtriage setup --update --account home --himalaya-account home
```

Once an account has been used, its identity is tied to that mailbox. Use a new name for a different mailbox instead of changing the existing account's address or IMAP server.

For unattended setup and every flag, see the [setup reference](../reference/setup.md). To write the configuration yourself, use [manual setup](./manual-setup.md).

**Next: [Daily use](./daily-use.md).**

## Detailed reference

- <span id="the-ten-steps"></span>[The ten steps](../reference/setup.md#the-ten-steps)
- <span id="prompts-yes-and-interactive"></span>[Prompts, --yes and --interactive](../reference/setup.md#prompts-yes-and-interactive)
- <span id="output"></span>[Output](../reference/setup.md#output)
- <span id="setup-exit-codes"></span>[Setup exit codes](../reference/setup.md#setup-exit-codes)
