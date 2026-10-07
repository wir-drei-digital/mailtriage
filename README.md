# mailtriage

mailtriage is a local command-line tool that classifies your email. For each message it decides a category from your own list, an urgency (`low`, `medium` or `high`), and whether you need to act, using OpenRouter's Jev decisions model. It reads mail through the [Himalaya](https://github.com/pimalaya/himalaya) CLI. With live filing, it moves mail into one IMAP folder per category, so every mail client shows the result. Every command can print JSON, so agents can use it as well as people.

## How it works

- `sync` and `watch` read new mail over IMAP through Himalaya. mailtriage never sends, deletes or expunges mail and never changes the read state.
- Each new message goes to OpenRouter's Decisions API, which returns the three decisions.
- Messages and decisions are stored in a local SQLite database. `list` and `read` work from it without network access.
- Filing is `off`, `dry_run` (plans moves and changes nothing) or `live` (creates the folders, moves mail and flags mail that needs action).

## Requirements

- [Himalaya v2.1.0](https://github.com/pimalaya/himalaya/releases/tag/v2.1.0) with IMAP support (`himalaya --version` shows `+imap`).
- An IMAP account. Filing needs a server with the MOVE extension.
- An [OpenRouter](https://openrouter.ai) API key.
- macOS or Linux. The background service uses launchd on macOS and systemd on Linux.

## Install

From a release (macOS arm64, Linux amd64 or Linux arm64), with the GitHub CLI:

```sh
VERSION=0.1.0
gh release download "v$VERSION" --repo wir-drei-digital/mailtriage --pattern "mailtriage-v$VERSION-macos-arm64.tar.gz*"
shasum -a 256 --check "mailtriage-v$VERSION-macos-arm64.tar.gz.sha256"
tar -xzf "mailtriage-v$VERSION-macos-arm64.tar.gz"
install -d ~/.local/bin
install -m 0755 mailtriage ~/.local/bin/mailtriage
```

On Linux, use `linux-amd64` or `linux-arm64` in the file name and `sha256sum --check`. The macOS executable is unsigned and not notarized. `~/.local/bin` must be on your `PATH`.

From source, with a stable Rust toolchain:

```sh
cargo build --release --locked
install -d ~/.local/bin
install -m 0755 target/release/mailtriage ~/.local/bin/mailtriage
```

mailtriage keeps itself up to date: the background service installs new releases by itself, and `mailtriage update` installs one now. A root-owned or otherwise unsafe install, for example one made with `sudo` into `/usr/local/bin`, is not replaced: mailtriage only reports new releases for it ([Binaries mailtriage does not replace](docs/guide.md#binaries-mailtriage-does-not-replace)).

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
mailtriage setup --yes --himalaya-account work --key-store env
```

`--yes` turns prompts off. Each answer then comes from its flag or its default, and a missing required flag exits 2 and names the flag. With `--key-store env`, set `OPENROUTER_API_KEY` in the environment of the process that runs mailtriage. The [agent guide](docs/hermes.md) covers the other flags, exit codes and health checks.

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
| `mailtriage filing enable --account work --mode live` | Starts moving mail. Do the [provider check](docs/guide.md#provider-check) first. |
| `mailtriage service status --account work` | Shows whether the background service runs, and the last pass. |

Add `--json` to any command for one line of JSON.

## Try it offline

The `fake` provider classifies with fixed keyword rules and needs no key and no mailbox. Run this in an empty directory with `MAILTRIAGE_CONFIG` unset, and delete the directory afterwards:

```sh
mailtriage init --json
mailtriage classify --account work --input /path/to/mailtriage/examples/reply-request.eml --format rfc822 --json
mailtriage list --account work --json
```

## Documentation

- [Guide](docs/guide.md): setup in detail, manual setup, configuration, the OpenRouter key, the background service, categories, filing and exit codes.
- [Agent guide](docs/hermes.md): non-interactive setup and safe use from Hermes or another agent.
- [Service API](docs/service-api.md): the library API and the JSON results of `setup`, `doctor` and `service`.
- [Verification](docs/verification.md): what has been tested, and the checks to run before `live` filing and on a real machine.

## License

MIT. See [LICENSE](LICENSE).
