# mailtriage

mailtriage is a local command-line tool that classifies your email. For each message it decides a category from your own list, an urgency (`low`, `medium` or `high`), and whether you need to act, using OpenRouter's Jev decisions model. It reads mail through the [Himalaya](https://github.com/pimalaya/himalaya) CLI. With live filing, it moves mail into one IMAP folder per category, so every mail client shows the result. Every command can print JSON, so agents can use it as well as people.

## How it works

- `sync` and `watch` read new mail over IMAP through Himalaya. mailtriage never sends, deletes or expunges mail and never marks mail unread. Only the optional reply queue marks mail read: answered or done mail you approved.
- Each new message goes to OpenRouter's Decisions API, which returns the three decisions.
- Messages and decisions are stored in a local SQLite database. `list` and `read` work from it without network access.
- Filing is `off`, `dry_run` (plans moves and changes nothing) or `live` (creates the folders, moves mail and flags mail that needs action).
- With the [reply queue](https://wir-drei-digital.github.io/mailtriage/guide/filing#reply-queue), mail that needs action stays in the inbox until you answer it; then it is filed, and marked read once you approve it.

## Requirements

- [Himalaya](https://github.com/pimalaya/himalaya) with IMAP support, in a version mailtriage is tested with (2.1.0 or 2.2.1). `mailtriage setup` offers to install one for mailtriage when none is found ([Himalaya versions](https://wir-drei-digital.github.io/mailtriage/guide/himalaya)).
- An IMAP account. Filing needs a server with the MOVE extension.
- An [OpenRouter](https://openrouter.ai) API key.
- macOS or Linux. The background service uses launchd on macOS and systemd on Linux.

## Install

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/wir-drei-digital/mailtriage/main/install.sh | sh
```

or, with Homebrew:

```sh
brew install wir-drei-digital/tap/mailtriage
```

The script installs mailtriage, and on macOS the tray app, into `~/.local/bin` (macOS arm64, Linux amd64 or arm64), then offers `mailtriage setup`; script installs keep themselves up to date. Homebrew installs update with `brew upgrade`. The guide's [Install](https://wir-drei-digital.github.io/mailtriage/guide/install) page has the options, building from source and uninstalling.

## Get started

```sh
mailtriage setup
```

Setup asks one question at a time and shows a default that Enter accepts:

1. the Himalaya account to use, or a new one created with `himalaya configure`;
2. your address, time zone and the folders to watch;
3. where the OpenRouter key lives: the macOS Keychain, Secret Service, `pass`, a command that prints it, or an environment variable. The store's own tool asks for the key, and mailtriage never writes it to a file;
4. whether to file mail into folders (`dry_run` by default; setup never turns on `live`);
5. whether to run `watch` in the background with launchd or systemd.

Setup writes `~/.config/mailtriage/mailtriage.json`, checks the result and prints the command that fixes anything not ready. Then classify and look at your mail:

```sh
mailtriage sync --account work
mailtriage list --account work
```

`work` stands for the account name you chose in setup; it defaults to the Himalaya account name.

## For agents

```sh
mailtriage setup --yes --himalaya-install --himalaya-account work --key-store env
```

`--yes` turns prompts off. Each answer then comes from its flag or its default, and a missing required flag exits 2 and names the flag. `--himalaya-install` installs a tested Himalaya for mailtriage when none is found. With `--key-store env`, set `OPENROUTER_API_KEY` in the environment of the process that runs mailtriage. The [agent guide](https://wir-drei-digital.github.io/mailtriage/agents/) covers the other flags, exit codes and health checks.

## Everyday commands

| Command | What it does |
| --- | --- |
| `mailtriage sync --account work` | Runs one pass: finds new mail, classifies it, and files it when filing is on. |
| `mailtriage watch --account work` | Repeats `sync` every 60 seconds until stopped. |
| `mailtriage list --account work` | Lists messages that need attention, from the local database. |
| `mailtriage read --account work --id ID` | Shows one message with its text and decisions. |
| `mailtriage correct --account work --id ID --category transactions` | Overrides a decision. |
| `mailtriage done --account work --id ID` | Marks a message handled. Nothing changes on the server. |
| `mailtriage filing plan --account work` | Shows what the next pass would move and flag. |
| `mailtriage filing enable --account work --mode live` | Starts moving mail. Do the [provider check](https://wir-drei-digital.github.io/mailtriage/guide/provider-check) first. |
| `mailtriage service status --account work` | Shows whether the background service runs, and the last pass. |

Add `--json` to any command for one line of JSON.

A tray app, `mailtriage-tray`, shows each account's state in the menu bar and edits categories; see [Tray](https://wir-drei-digital.github.io/mailtriage/guide/tray).

## Try it offline

The `fake` provider classifies with fixed keyword rules and needs no key and no mailbox. Run this in an empty directory with `MAILTRIAGE_CONFIG` unset, and delete the directory afterwards:

```sh
mailtriage init --json
mailtriage classify --account work --input /path/to/mailtriage/examples/reply-request.eml --format rfc822 --json
mailtriage list --account work --json
```

## Documentation

The documentation is at [wir-drei-digital.github.io/mailtriage](https://wir-drei-digital.github.io/mailtriage/). It follows `main`, so it can describe a feature that is newer than the latest release.

- [Guide](https://wir-drei-digital.github.io/mailtriage/guide/introduction): setup in detail, manual setup, configuration, the provider and its key, the background service, categories, filing and exit codes.
- [Agents](https://wir-drei-digital.github.io/mailtriage/agents/): non-interactive setup and safe use from Hermes or another agent.
- [Development](https://wir-drei-digital.github.io/mailtriage/development/): the service API, adding a provider, releases, and what has been verified.

## License

MIT. See [LICENSE](LICENSE).
