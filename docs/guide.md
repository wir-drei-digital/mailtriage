# mailtriage guide

This guide is the full reference: setup, configuration, the OpenRouter key, the background service, daily use, categories, filing, the tray app and exit codes. The [README](../README.md) is the short introduction.

- [What mailtriage does and changes](#what-mailtriage-does-and-changes)
- [Try it offline](#try-it-offline)
- [Install](#install)
- [Guided setup](#guided-setup)
- [Manual setup](#manual-setup)
- [Configuration](#configuration)
- [The OpenRouter key](#the-openrouter-key)
- [Background service](#background-service)
- [Daily use](#daily-use)
- [Categories](#categories)
- [Filing into folders](#filing-into-folders)
- [Tray](#tray)
- [Reference](#reference)

## What mailtriage does and changes

`mailtriage` is a command-line tool that classifies email. For each message it records three decisions: a category from your own list, an urgency (`low`, `medium` or `high`), and whether you need to act. It reads mail over IMAP through the [Himalaya](https://github.com/pimalaya/himalaya) CLI, gets the decisions from OpenRouter's Decisions API with a Jev model, and stores messages and results in a local SQLite database. Listing and reading work from that database without network access.

mailtriage never sends, deletes or expunges mail and never changes the read state (`\Seen`). By default it does not write to the mailbox at all. An account that enables [filing into folders](#filing-into-folders) also creates category folders, moves mail into them and adds `\Flagged`. `done` and `reopen` change only the local review state. Every command works on one configured account, named with `--account`.

## Try it offline

The built-in `fake` provider classifies with fixed keyword rules and makes no network request. Use it in a scratch directory and delete the directory afterwards: the first command against an account records that account's identity in the state directory, so the example account cannot be reused for real mail. Unset `MAILTRIAGE_CONFIG` first; it comes before `./mailtriage.json` in the [config resolution order](#where-mailtriage-finds-the-config).

```sh
mkdir mailtriage-demo && cd mailtriage-demo
mailtriage init --json
mailtriage classify --account work --input /path/to/mailtriage/examples/reply-request.eml --format rfc822 --json
mailtriage list --account work --view attention --json
```

`init` writes `mailtriage.json` in the current directory, with the `fake` provider, one account named `work` and state in `.state/`. It refuses to overwrite an existing file (exit code 2). The later commands find that file because they run in the same directory. `classify` reads one message, classifies it and stores the result without a mailbox location. It accepts an RFC 822 file (`--format rfc822`), the JSON shape in `examples/message.json` (`--format json`), or stdin (`--input -`). Classifying the same message again returns `outcome: cached`.

## Install

You need:

- macOS arm64, Linux amd64 or Linux arm64,
- Himalaya v2.1.0 with IMAP support,
- an IMAP account whose server supports UID and UIDVALIDITY (the MOVE extension too, if you want filing),
- an OpenRouter API key,
- for the background service: launchd (macOS) or systemd (Linux).

From a GitHub release: each release carries `mailtriage-vVERSION-linux-amd64.tar.gz`, `-linux-arm64.tar.gz` and `-macos-arm64.tar.gz`, a `.sha256` file per archive and a combined `SHA256SUMS`. Each archive holds the `mailtriage` executable, the README and the license. While the repository is private, download with authenticated access, for example with the GitHub CLI:

```sh
VERSION=0.1.0
gh release download "v$VERSION" --repo wir-drei-digital/mailtriage --pattern "mailtriage-v$VERSION-macos-arm64.tar.gz*"
shasum -a 256 --check "mailtriage-v$VERSION-macos-arm64.tar.gz.sha256"   # Linux: sha256sum --check
tar -xzf "mailtriage-v$VERSION-macos-arm64.tar.gz"
sudo install -m 0755 mailtriage /usr/local/bin/mailtriage
```

The macOS executable is unsigned and not notarized. See the [release guide](releases.md) for how releases are made.

From source, with a stable Rust toolchain:

```sh
cargo build --release --locked
sudo install -m 0755 target/release/mailtriage /usr/local/bin/mailtriage
mailtriage --version
```

The examples below assume `mailtriage` is on your `PATH`. The background service records the absolute path of the executable that installs it, so install the binary in its final place first.

## Guided setup

```sh
mailtriage setup
```

`setup` asks one question per setting, writes the configuration, checks it and can install the background service. Every answer can also be given as a flag, so it runs without prompts too.

- It writes `~/.config/mailtriage/mailtriage.json`, or the path in `--config` or `MAILTRIAGE_CONFIG`. It never uses `./mailtriage.json`.
- A new config keeps its state in `state/` next to the config file (`~/.config/mailtriage/state`).
- It never writes IMAP settings; Himalaya's own wizard does that.
- It makes no change on the IMAP server.
- It never asks for, prints or stores the OpenRouter key. The key store's own tool asks for it; setup only checks that the read command prints a key.
- It never turns on `live` filing.

### The ten steps

| Step | What happens | Flags |
| --- | --- | --- |
| 1. Config | If the config exists: update an account, add an account, or abort. Other accounts are never changed. | `--config`, `--update`, `--account` |
| 2. Himalaya | Finds the Himalaya binary, its config file and the account, and runs `himalaya account check`. Can create an account with `himalaya configure`. | `--himalaya-binary`, `--himalaya-config`, `--himalaya-account` |
| 3. Account | The account name in mailtriage, your address, time zone and a one-line brief. | `--account`, `--identity`, `--timezone`, `--brief` |
| 4. Folders | Lists the server's folders; you choose which to watch. | `--mailbox` |
| 5. Classifier | OpenRouter or the offline `fake` provider, the model, and where the key lives. | `--provider`, `--model`, `--key-store`, `--key-command`, `--key-env`, `--key-stored` |
| 6. Categories | Six default categories for a new account. | none |
| 7. Filing | `dry_run` or `off`. | `--filing` |
| 8. Write | Validates and writes the config. | none |
| 9. Check | Runs `doctor` and prints each item with its fix. | none |
| 10. Service | Offers to run `watch` in the background. | `--service`, `--interval-seconds`, `--limit` |

**Step 1, config.**

- With prompts, an existing config shows a menu: update an account, add an account, or abort.
- With prompts, `--account NAME` answers that menu. An existing name is updated; a new name is added.
- Without prompts, an existing config needs `--update`; otherwise setup exits 5.
- `--update --account NAME` updates `NAME` if it exists and adds it otherwise. Without `--account`, the name defaults to the Himalaya account name (step 3).

**Step 2, Himalaya.**

- Binary: `--himalaya-binary`, else the stored binary of the account being updated, else the first `himalaya` on `PATH`. Setup stores it as an absolute path. Its `--version` must report `himalaya v2.1.0` with `+imap`; otherwise setup exits 3.
- Config file: `--himalaya-config`, else the stored file of the account being updated, else `HIMALAYA_CONFIG`, else Himalaya's default. `HIMALAYA_CONFIG` must name one file; several `:`-separated files exit 2.
- Himalaya's default is the first existing file of: `~/Library/Application Support/himalaya/config.toml` on macOS, or `$XDG_CONFIG_HOME/himalaya/config.toml` on Linux when `XDG_CONFIG_HOME` is an absolute path; then `~/.config/himalaya/config.toml`; then `~/.himalayarc`.
- Account: setup offers only accounts with an IMAP backend. The default is the account being updated, else Himalaya's default account.
- The menu's last entry runs `himalaya configure`, Himalaya's own wizard, attached to your terminal. Setup also offers it when no IMAP account exists. See [Set up Himalaya](#1-set-up-himalaya) to write the file yourself.
- Without prompts, `--himalaya-account` is required, and `himalaya configure` never runs. An account updated by `--account NAME` keeps its stored Himalaya account instead. Setup refuses to change the Himalaya account of a bound account (exit 5); see [Updating an account](#updating-an-account).
- Check: setup runs `himalaya account check` for the IMAP backend. If it fails, setup exits 3 and prints the command that shows why. It never prints Himalaya's output.

**Step 3, account details.**

- `--account`: default the Himalaya account name. 1 to 64 ASCII letters, digits, `-` or `_`; the name is used in service and log file names. An added account needs a new name.
- `--identity`: your address. Default the account's `email` in the Himalaya file.
- `--timezone`: an IANA name such as `Europe/Zurich`. Default `TZ`, then the zone `/etc/localtime` points to, then `UTC`.
- `--brief`: optional. One line about you that helps classification. It is sent to the provider with every message.
- When you update an account, its current values are the defaults. Its identity is bound after the first check, and setup refuses to change it (exit 5); see [Updating an account](#updating-an-account).

**Step 4, folders.**

- Setup lists the account's folders through Himalaya. Folders marked `\Noselect` are left out.
- The default is `INBOX`, or the account's current folders when you update it. With prompts, enter numbers separated by commas.
- `--mailbox NAME`, repeated for several folders, must name a folder on the server; otherwise setup exits 2.
- With filing on, watched folders must be printable ASCII without `\`, `"` or `&` and must not start with `-`.

**Step 5, classifier.**

- `--provider openrouter` (default) or `--provider fake` (offline keyword rules, no key).
- `--model`: default `typesafe/jev-1.13`. It must start with `typesafe/jev-` or `~typesafe/jev-`.
- The key: see [Key stores](#key-stores).
- On an existing config without classifier flags, setup asks "Keep the current classifier?" (default yes). Without prompts it keeps the classifier. The classifier flags are `--provider`, `--model`, `--key-store`, `--key-command`, `--key-env` and `--key-stored`.
- Only a key flag (`--key-store`, `--key-command`, `--key-env`, `--key-stored`) changes where an OpenRouter key comes from. `--model` or `--provider openrouter` alone keep `api_key_command` and `api_key_env` as they are and skip the key question. If you answer no to "Keep the current classifier?", the key store menu offers the current store as its default.
- Key flags with `--provider fake` exit 2: the offline classifier needs no key.
- All accounts in a config share one provider, so there is one key per config.

**Step 6, categories.** A new account gets six categories: Correspondence, Transactions, Updates, Newsletters, Promotions and Other (the catch-all). Each has `folder` set to its own name. An updated account keeps its categories. Once the config is written, setup prints the commands to change them; see [Categories](#categories).

**Step 7, filing.** `--filing dry-run` plans moves and flags and writes nothing; it is the default for a new account, and an updated account defaults to its current mode. `--filing off` only classifies. Setup never selects `live`. An account that is already `live` stays `live` unless you pass `--filing`. To go live, follow the [rollout](#rollout).

**Step 8, write.** Setup validates the whole config and writes it atomically with mode 0600. The state directory is created with mode 0700. If the result is invalid, setup exits 2 and writes nothing. If the state directory already holds a binding for the account and the new answers would change it, setup exits 5 and writes nothing; see [Updating an account](#updating-an-account).

**Step 9, check.** Setup runs `doctor` for the account. It prints each item (`provider`, `key`, `mail`, and `filing` when filing is on) as `ok`, or as `not ready` with the one command that fixes it. If `doctor` itself fails, for example because the state database cannot be opened, setup reports a single not-ready `state` item instead. Setup exits 0 even when an item is not ready.

**Step 10, service.** With prompts on macOS or Linux, setup asks whether to run `watch` in the background (default yes); on other platforms it skips this step. When the key comes from an environment variable, the default is no, because the service does not inherit the variable; setup says so and prints the command that moves the key into a key store. Without prompts, `--service install` installs it and `--service skip` (the default) does not. `--interval-seconds` (1 to 86400, default 60) and `--limit` (1 to 500, default 100) are passed to `watch`. When step 9 reported a not-ready `state` item, setup installs no service, since every pass would fail; it prints the `service install` command to run once that is fixed. See [Background service](#background-service).

**Next steps.** Setup ends with the commands to run next: `sync` and `watch`, or, with the service installed, `service status` and `list` (`sync` or `watch` would compete with the service for the account lock). Every `mailtriage` command setup prints passes `--config` when a command run in the same directory and environment without it would find another config or none. When a `./mailtriage.json` in the working directory would win over the written config, setup also prints a warning.

### Key stores

The key never goes into `mailtriage.json`. Setup records either a command that prints the key (`provider.api_key_command`) or the name of an environment variable (`provider.api_key_env`).

The menu lists the stores available on this machine, in this order. The first one is the default, also without prompts. When you change an existing OpenRouter classifier with prompts, the menu's default is the store the key comes from now (`command` for a command of your own, `env` for a variable).

| `--key-store` | Offered | Store command (the tool asks for the key) | Read command, saved as `api_key_command` |
| --- | --- | --- | --- |
| `keychain` | on macOS | `security add-generic-password -U -s mailtriage -a openrouter -w` | `security find-generic-password -s mailtriage -a openrouter -w` |
| `secret-service` | when `secret-tool` is on `PATH` | `secret-tool store --label='mailtriage OpenRouter key' service mailtriage provider openrouter` | `secret-tool lookup service mailtriage provider openrouter` |
| `pass` | when `pass` is on `PATH` | `pass insert mailtriage/openrouter` | `pass show mailtriage/openrouter` |
| `command` | always | none | `["/bin/sh", "-c", "COMMAND"]` from `--key-command 'COMMAND'` |
| `env` | always | none | none; sets `api_key_env` from `--key-env NAME` |

For `keychain`, `secret-service` and `pass`:

1. Setup runs the read command. If it prints a key, setup offers to use it (default yes). Without prompts it uses it.
2. Otherwise setup runs the store command. The tool asks for the key on your terminal. mailtriage never sees the key, and the tool's output is not shown.
3. Setup runs the read command again. If it prints no key, setup exits 3.
4. Setup saves the read command, with the tool's absolute path, in `api_key_command`.

Without prompts, a tool store needs one of these: a terminal for the tool's prompt (`--yes` run in a terminal), a key that is already stored, or `--key-stored`. Otherwise setup exits 2 and prints the store command to run first.

`--key-stored` says the key is already in the store. Setup then skips the question, saves and checks the read command, and exits 3 with the store command if no key is found. For example, on macOS:

```sh
security add-generic-password -U -s mailtriage -a openrouter -w
mailtriage setup --yes --himalaya-account work --key-store keychain --key-stored
```

For `command`, `--key-command 'COMMAND'` is any shell command that prints the key; `--key-command` alone implies `--key-store command`. Setup runs it once and requires a key. A command typed at the prompt is asked again after a failure; a failing `--key-command` exits 3.

For `env`, `--key-env NAME` names the variable; `--key-env` alone implies `--key-store env`. The default is the current `api_key_env`, else `OPENROUTER_API_KEY`. Setup removes any `api_key_command` and warns when the variable is not set in its own environment. The variable must be set wherever mailtriage runs; see [Environment variable](#environment-variable).

`--key-command` and `--key-env` exclude each other, and a `--key-store` that contradicts them exits 2.

To change where the key comes from later:

```sh
mailtriage setup --update --account work --key-store keychain
```

This keeps the model, endpoint, timeout and `api_key_env`, so no mail is classified again. A new provider, a new model or a new variable name in `--key-env` does queue open mail for classification again.

### Prompts, `--yes` and `--interactive`

- Prompts go to stderr. Each shows its default in brackets; Enter accepts it. Menus are numbered. Invalid input is asked again.
- Prompts are on when stdin is a terminal, or with `--interactive` (answers from a pipe, as the tests do). Otherwise setup runs as with `--yes`.
- `--yes` turns prompts off. Each value comes from its flag or its default. A value without a default, such as `--himalaya-account`, exits 2 and names the flag.
- A flag always answers its question; setup does not ask it.
- Choosing "Abort" in step 1 exits 2 with `setup aborted; nothing was changed`.
- End of input exits 2 with `setup aborted: input ended`. The config is written in step 8, so an abort at the service question keeps it. Run `mailtriage service install --account NAME` for the service.

### Updating an account

`mailtriage setup --update --account NAME` (or "Update an account" in the menu) keeps:

- the account's Himalaya binary, config file and account, unless you pass the `--himalaya-*` flags,
- its categories and `taxonomy_revision`,
- its filing `flag`, `max_actions_per_pass` and `live` mode,
- the classifier, unless you pass a classifier flag or answer no,
- where the key comes from, unless you pass a key flag or choose another store in the menu.

Its identity, time zone, brief and folders become the defaults of their questions.

Step 9 binds the account to its mailbox: `doctor` records the identity, the Himalaya account and the IMAP server settings (see [Account binding](#account-binding)). Every later command for an account whose binding changed would exit 5, so setup compares the bindings before it writes. An update that would change any of them exits 5 with `step 3 (account): account NAME is bound to its previous mailbox (identity, Himalaya account or IMAP server changed); keep them, or set this mailbox up under a new name with --account NEW` and writes nothing. The same check applies to a new config whose `state/` directory is left over from an earlier one. To use a different mailbox, set it up under a new account name:

```sh
mailtriage setup --update --account home --himalaya-account home
```

### Output

Progress and the check summary go to stderr. stdout carries one result object, on one line with `--json`:

```json
{"schema_version":1,"setup":{"account":"work","config":"/Users/alice/.config/mailtriage/mailtriage.json","doctor":{"items":[{"check":"provider","ready":true},{"check":"key","ready":true},{"check":"mail","ready":true},{"check":"filing","ready":true}],"ready":true},"filing":"dry_run","key_source":"command","key_store":"keychain","mailboxes":["INBOX"],"model":"typesafe/jev-1.13","provider":"openrouter","service":null}}
```

| Field | Content |
| --- | --- |
| `config` | The absolute path of the config written. |
| `account`, `mailboxes` | The account name and its watched folders. |
| `provider`, `model` | The classifier. |
| `key_source` | `command`, `env`, or `null` for `fake`. |
| `key_store` | The `--key-store` value chosen in this run; `null` when no store was chosen: the classifier or its key source was kept, or it is `fake`. |
| `filing` | `off`, `dry_run` or `live`. |
| `doctor` | `ready`, and `items`: each `{check, ready}`, plus `error` and `fix` when not ready. |
| `service` | `null` when skipped or not installed after a failed `state` check, else the [`service install` result](#service-commands). |

### Setup exit codes

| Code | Cases |
| --- | --- |
| 0 | Setup finished. `doctor` items that are not ready are listed in the result. |
| 2 | Invalid input; a required flag missing without prompts; an invalid account name; conflicting key flags, or key flags with `--provider fake`; a key tool not on `PATH`; a tool store without a terminal; setup aborted; the service on an unsupported platform. |
| 3 | Himalaya missing or not v2.1.0 with IMAP; `account check` failed; the folders could not be listed; a key tool or key command failed; the config could not be written; `launchctl` or `systemctl` failed. |
| 5 | The config exists and `--update` was not given (without prompts); the account is bound to another mailbox (its identity, Himalaya account or IMAP server would change); a service file exists that mailtriage did not write. |

Every error except the two abort messages (`setup aborted; nothing was changed`, `setup aborted: input ended`) starts with `step N (name): ` and names the flag or command that fixes it, for example `step 2 (Himalaya): --himalaya-account is required without prompts`. Step 10 errors (`step 10 (service): `) happen after the config is written; they name the `mailtriage service install` command, with `--config` and this run's `--interval-seconds` and `--limit`, to run once the cause is fixed. On a platform without launchd or systemd the fix is to drop `--service install` instead.

## Manual setup

Write the configuration yourself when you want full control, for example on a server. Install mailtriage first ([Install](#install)).

### 1. Set up Himalaya

mailtriage runs the `himalaya` executable for every mailbox operation. It accepts only Himalaya v2.1.0 with IMAP support: the first line of `himalaya --version` must start with `himalaya v2.1.0` and contain `+imap`. Install it from the [v2.1.0 release](https://github.com/pimalaya/himalaya/releases/tag/v2.1.0) or a package manager, then check:

```sh
himalaya --version
```

Create an account with `himalaya configure`, or write a Himalaya configuration file with one account. mailtriage uses only the account's `imap` settings; it never sends mail, so no `smtp` settings are needed. A minimal file, for example `~/.config/himalaya/config.toml`:

```toml
[accounts.work]
email = "alice@example.org"
imap.server = "imaps://imap.example.org:993"
imap.sasl.plain.username = "alice@example.org"
imap.sasl.plain.password.cmd = "security find-generic-password -s imap.example.org -a alice@example.org -w"
```

- `[accounts.work]`: the account name. `engine.account` in `mailtriage.json` refers to it.
- `email`: the address. `mailtriage setup` uses it as the default identity.
- `imap.server`: `imaps://HOST:993` for TLS. A bare `HOST` also means `imaps://`. For STARTTLS on port 143, use `imap://HOST:143` with `imap.starttls = true`.
- Credentials use Himalaya's own mechanism. `password.cmd` (also spelled `password.command`) runs a command that prints the password. The example reads it from the macOS keychain; store it there once with `security add-generic-password -s imap.example.org -a alice@example.org -w`, which prompts for the password. On Linux, a command such as `pass show mail/work` works the same way. `password.raw` stores the password in the file; avoid it. For OAuth servers, Himalaya offers `imap.sasl.oauthbearer` and `imap.sasl.xoauth2`; see [Himalaya's sample configuration](https://github.com/pimalaya/himalaya/blob/v2.1.0/config.sample.toml).
- mailtriage starts Himalaya with no terminal input and passes on its own environment. The password command must not prompt, and the commands it calls must be found on that environment's `PATH`.
- Do not add a `mailbox.alias` entry to this account that maps a watched folder or category folder name to a different mailbox. With filing on, mailtriage stops scanning such a folder and makes no filing writes until the alias is removed.

Check the login and the folder names. These commands contact the server:

```sh
himalaya --config ~/.config/himalaya/config.toml --account work account check
himalaya --config ~/.config/himalaya/config.toml --account work imap list --all
himalaya --config ~/.config/himalaya/config.toml --account work --json imap status INBOX
```

`account check` prints whether the IMAP login works. It exits 0 even when the check fails, so read its output. The second command lists every folder; use these exact names in `engine.mailboxes`. The third must print `uid_validity` and `uid_next`, which mailtriage depends on. mailtriage discards Himalaya's error output, so use these commands to see the cause of an IMAP error later.

### 2. Create the configuration

```sh
mailtriage init --config ~/.config/mailtriage/mailtriage.json --json
```

`init` writes a starting file with the `fake` provider, one account named `work` without an engine, and `state_dir` `.state`. Edit it as described in [Configuration](#configuration) before you run any other command against the account. The first command that opens an account stores its binding: `identity`, the Himalaya account name, `imap.server` and the other non-secret IMAP settings. A later change to any of them is refused with exit code 5 (see [Account binding](#account-binding)).

### 3. Provide the OpenRouter key

Set `provider.api_key_command` to a command that prints the key, or set `provider.api_key_env` and the variable it names. On macOS, store the key in the Keychain (the command prompts for it):

```sh
security add-generic-password -U -s mailtriage -a openrouter -w
```

Then set the read command in `mailtriage.json`:

```json
"api_key_command": ["/usr/bin/security", "find-generic-password", "-s", "mailtriage", "-a", "openrouter", "-w"]
```

The other stores and the rules for the command are in [The OpenRouter key](#the-openrouter-key).

### 4. Check the setup

```sh
mailtriage doctor --account work --json
```

Run it in the same environment as the command you are checking. `doctor` exits 0 whether or not the account is ready, so read the fields:

| Field | Value when ready | Meaning |
| --- | --- | --- |
| `ready` | `true` | The provider configuration is valid, the key is present, and the Himalaya check passed. |
| `provider.kind`, `provider.model` | your values | The configured provider. |
| `provider.configuration_valid` | `true` | The provider block passes validation. |
| `provider.key_source` | `command` or `env` | Where the key comes from: `api_key_command` when set, else `api_key_env`. `null` for `fake`. |
| `provider.key_present` | `true` | The key command printed a key, or the variable named in `api_key_env` is set and not blank in this process. Always `true` for `fake`. |
| `provider.key_error` | absent | Present only when the key is missing: one of the fixed messages in [Key command rules](#key-command-rules), or `OpenRouter API key environment variable is missing`, or `OpenRouter API key environment variable is empty`. |
| `transport.configured` | `true` | The account has an `engine`. Without one, `transport.ready` is `true` as well. |
| `transport.ready` | `true` | The Himalaya configuration file was read and `himalaya --version` reported v2.1.0 with `+imap`. Otherwise `transport.error` is set. |
| `transport.version` | `himalaya v2.1.0 ...` | The first line of `himalaya --version`. |
| `live_checks_performed` | `false` | Always `false`. |

`doctor` runs the key command to check it, so on macOS the first run may show a Keychain access dialog. It makes no provider request. It logs in to the IMAP server only when filing is on, to add a `filing` block with the server's capabilities, folders and problems. The first `sync` is therefore the first full test of the IMAP login and the API key.

### 5. First run

1. Run `doctor` (step 4) until `ready` and `transport.configured` are `true`.
2. Run one small pass:

   ```sh
   mailtriage sync --account work --limit 20 --json
   ```

   Exit code 0 with `scan_errors: 0` and `failed: 0` means the login, the fetch and the provider request worked. Exit code 4 means the pass was partial. A `classification` object with `skipped: true` means the key is unavailable; its `reason` says why, and `doctor` shows the same `key_error`. `scan_errors` counts folders that could not be scanned (see `coverage.scans[].error`), and `failed` counts messages whose fetch or classification failed (`list --view all` shows each message's `error`). Run the Himalaya check commands from step 1 to see an IMAP error.
3. Look at the result:

   ```sh
   mailtriage list --account work --view attention --json
   ```

4. Start `watch` in the background ([Background service](#background-service)), or in a terminal:

   ```sh
   mailtriage watch --account work --limit 100 --interval-seconds 60 --json
   ```

5. To move mail into category folders, follow the [filing rollout](#rollout).

The first passes work through the existing mail in each watched folder, oldest UID first. Each pass scans at most `--limit` UIDs per folder and classifies at most `--limit` messages. Every classification is one OpenRouter request.

## Configuration

### Where mailtriage finds the config

Every command except `init` and `setup` uses the first of:

1. `--config PATH`;
2. the `MAILTRIAGE_CONFIG` environment variable;
3. `./mailtriage.json`, if that file exists in the working directory;
4. `~/.config/mailtriage/mailtriage.json`.

`setup` uses the same order without step 3: it never picks `./mailtriage.json`. `init` writes `--config PATH` if given, else `./mailtriage.json`; it ignores `MAILTRIAGE_CONFIG`. Step 4 needs `HOME`; without it the error asks for `--config`. A missing config exits 2 with ``configuration not found; run `mailtriage setup` or pass --config``.

Commands therefore need no `--config` when you use the home config. Pass `--config` in scripts that must not depend on the working directory or the environment.

### A complete configuration

A configuration for one account:

```json
{
  "schema_version": 2,
  "state_dir": "state",
  "provider": {
    "kind": "openrouter",
    "model": "typesafe/jev-1.13",
    "endpoint": "https://openrouter.ai/api/alpha/decisions",
    "api_key_command": ["/usr/bin/security", "find-generic-password", "-s", "mailtriage", "-a", "openrouter", "-w"],
    "api_key_env": "OPENROUTER_API_KEY",
    "timeout_seconds": 30
  },
  "policy": {
    "choice_confidence_min": 0.75,
    "action_yes_min": 0.8,
    "action_no_max": 0.2,
    "review_mode": true,
    "max_body_chars": 20000,
    "max_attempts": 5,
    "freshness_hours": 24
  },
  "accounts": {
    "work": {
      "identity": "alice@example.org",
      "timezone": "Europe/Zurich",
      "brief": "Alice Example, project lead at Example AG. Requests from customers and from the leadership team usually need a reply within a day. Invoices need her approval. Vendor marketing and newsletters never need action.",
      "taxonomy_revision": 1,
      "categories": [
        {
          "id": "correspondence",
          "name": "Correspondence",
          "description": "Direct conversations with people, including customers and colleagues",
          "examples": [],
          "catch_all": false,
          "folder": "INBOX"
        },
        {
          "id": "transactions",
          "name": "Transactions",
          "description": "Invoices, receipts, orders and account activity",
          "examples": [],
          "catch_all": false
        },
        {
          "id": "updates",
          "name": "Updates",
          "description": "Automated status, ticket and service notifications",
          "examples": [],
          "catch_all": false
        },
        {
          "id": "newsletters",
          "name": "Newsletters",
          "description": "Editorial mail and subscriptions",
          "examples": [],
          "catch_all": false
        },
        {
          "id": "promotions",
          "name": "Promotions",
          "description": "Offers and marketing",
          "examples": [],
          "catch_all": false
        },
        {
          "id": "other",
          "name": "Other",
          "description": "Mail that fits none of the other categories",
          "examples": [],
          "catch_all": true
        }
      ],
      "engine": {
        "kind": "himalaya",
        "binary": "/opt/homebrew/bin/himalaya",
        "config": "/Users/alice/.config/himalaya/config.toml",
        "account": "work",
        "mailboxes": ["INBOX"],
        "expected_version": "2.1.0",
        "timeout_seconds": 60,
        "max_output_bytes": 50000000
      },
      "filing": {
        "mode": "off",
        "flag": true,
        "max_actions_per_pass": 200
      }
    }
  }
}
```

Each command that opens the configuration checks the whole file. A file that breaks a rule below is refused with exit code 2 and `invalid configuration; check required fields, categories and provider settings`; the message does not name the field. Relative paths in `state_dir`, `engine.config` and `engine.binary` resolve against the directory that holds `mailtriage.json`.

Top level:

| Field | What to set |
| --- | --- |
| `schema_version` | `2`, as written by `init` and `setup`. |
| `state_dir` | Directory for the SQLite database and normalized message text. mailtriage creates it with mode 0700. Keep it on a local filesystem. |
| `provider` | The classification provider, below. |
| `policy` | Thresholds and limits, below. The `init` values are a reasonable start. |
| `accounts` | One entry per mailbox. The key (`work`) is the name you pass to `--account`. |

Account (`accounts.NAME`):

| Field | What to set |
| --- | --- |
| `identity` | The mailbox owner's address, such as `alice@example.org`. Sent to the provider as the recipient. Part of the account binding. |
| `timezone` | An IANA time zone name, such as `Europe/Zurich`. Sent to the provider to judge timing. mailtriage checks only that it is not empty. |
| `brief` | A few sentences about the recipient: role, whose mail matters, what usually needs action. Sent to the provider with every message. Replace the `init` placeholder. |
| `taxonomy_revision` | Leave as is. `categories apply` advances it. |
| `categories` | The categories to choose from, below. |
| `engine` | The Himalaya settings, below. Without an engine the account can only `classify` files. |
| `filing` | Filing settings, below. Optional; filing is off by default. |

Categories (`accounts.NAME.categories[]`). You can also edit them later with [`categories apply`](#categories).

| Field | What to set |
| --- | --- |
| `id` | Stable identifier of letters, digits, `_` and `-`, unique in the account. Corrections and folders refer to it; keep it when you rename a category. |
| `name` | Display name. Sent to the provider. Also the default folder name for filing. |
| `description` | What belongs in the category. Sent to the provider with the name; this text decides the choice. |
| `examples` | Optional list of strings. Stored, but not sent to the provider. |
| `catch_all` | `true` for exactly one category. The provider sees only names and descriptions, so give that category a description that covers mail outside the others. |
| `folder` | Optional IMAP folder for filing; defaults to `name`. `"INBOX"` keeps the category's mail in its source folder. See the [folder rules](#folders). |

Engine (`accounts.NAME.engine`):

| Field | What to set |
| --- | --- |
| `kind` | `"himalaya"`. |
| `binary` | Absolute path to the Himalaya executable, such as `/opt/homebrew/bin/himalaya` (`command -v himalaya` prints it). A bare name is looked up on the `PATH` of the mailtriage process, which is short under launchd and systemd. |
| `config` | Path to the Himalaya configuration file: absolute, or relative to `mailtriage.json`. `~` is not expanded. |
| `account` | The account name in that file (`[accounts.work]` means `"work"`). |
| `mailboxes` | The source folders to watch, usually `["INBOX"]`, spelled as `himalaya imap list --all` prints them. With filing on, each must be printable ASCII without `\`, `"` or `&` and must not start with `-`. |
| `expected_version` | `"2.1.0"`. Other values are refused. |
| `timeout_seconds` | Time limit for each Himalaya call, 1 to 600. |
| `max_output_bytes` | Output limit for each Himalaya call, 1 to 268435456 (256 MiB). A message larger than this cannot be fetched and is recorded as a failed fetch. |

`doctor` reports an engine whose `expected_version`, `timeout_seconds` or `max_output_bytes` is out of range as `transport.ready: false`. Configurations written before schema 2 have a `himalaya` block instead of `engine`. mailtriage still reads it and writes it back as `engine` the next time it saves the file. An account cannot have both.

Provider (`provider`):

| Field | What to set |
| --- | --- |
| `kind` | `"openrouter"` for real classification, `"fake"` for offline tests. No other value is accepted. |
| `model` | For `openrouter`, a Jev Decisions model ID that starts with `typesafe/jev-` or `~typesafe/jev-`, such as `typesafe/jev-1.13`. For `fake`, any non-empty text. |
| `endpoint` | For `openrouter`, exactly `https://openrouter.ai/api/alpha/decisions`. An `http://127.0.0.1:PORT/api/alpha/decisions` address is also accepted, for local tests. Ignored for `fake`. |
| `api_key_command` | Optional. A command that prints the API key, as a list of program and arguments, such as `["/usr/bin/security", "find-generic-password", "-s", "mailtriage", "-a", "openrouter", "-w"]`. The first element must be a non-empty program. When set, it is the only key source. See [Key command rules](#key-command-rules). Ignored for `fake`. |
| `api_key_env` | The name of the environment variable that holds the API key, such as `OPENROUTER_API_KEY`: uppercase letters `A` to `Z`, digits and `_` only. This is the variable's name, never the key. For `openrouter`, required unless `api_key_command` is set; it may then be empty or missing. Ignored for `fake`. |
| `timeout_seconds` | Time limit for each provider request, 1 to 300. |

For each message the provider receives the account's `identity`, `timezone` and `brief`, each category's `name` and `description`, and the message's sender, To and Cc addresses, subject, date and body text up to `policy.max_body_chars`. mailtriage never falls back to another provider. A failed request marks the message failed, and a later pass retries it.

Policy (`policy`), with the `init` defaults:

| Field | Default | Effect |
| --- | --- | --- |
| `review_mode` | `true` | Every classification gets the reason `review_mode`, so all open mail is listed in the attention view. Turn it off only after you have checked the results on your own mail. |
| `choice_confidence_min` | `0.75` | A category or urgency below this confidence is left empty, and the message is listed for attention. 0 to 1. |
| `action_yes_min` | `0.8` | `action_required` becomes `true` at or above this probability. |
| `action_no_max` | `0.2` | `action_required` becomes `false` at or below this probability. Between the two values it stays empty. Must be lower than `action_yes_min`. |
| `max_body_chars` | `20000` | Body characters kept and sent to the provider, 1 to 1000000. A longer body is cut, and the message is marked incomplete (`input_incomplete`). Incomplete messages are not filed automatically. |
| `max_attempts` | `5` | Failed fetch or classification attempts before mailtriage stops retrying a message, 1 to 100. The wait before a retry starts at 1 minute and doubles per attempt, up to 64 minutes. |
| `freshness_hours` | `24` | Open messages whose classification is older than this are classified again. With OpenRouter, each open message costs one request per period. Must be positive. |

Changing `brief`, `timezone`, a category's `id`, `description`, `examples` or `catch_all`, the `provider` block (except `api_key_command`) or the `policy` block queues every open message for classification again. Setting, changing or removing `api_key_command` does not.

Filing (`accounts.NAME.filing`), described in [Filing into folders](#filing-into-folders):

| Field | Default | Effect |
| --- | --- | --- |
| `mode` | `"off"` | `"off"`, `"dry_run"` or `"live"`. Change it with `filing enable` and `filing disable`. |
| `flag` | `true` | Add `\Flagged` to mail that needs action or has `high` urgency. |
| `max_actions_per_pass` | `200` | Moves and flags per pass, 1 to 1000. |

## The OpenRouter key

mailtriage gets the key in one of two ways:

- If `provider.api_key_command` is set, it runs that command. This is the only source then; `api_key_env` is ignored even if the variable is set.
- Otherwise it reads the environment variable named in `provider.api_key_env` from its own process environment.

mailtriage does not read a `.env` file or any other key file. Never put the key in `mailtriage.json`. `mailtriage setup` sets up either source; see [Key stores](#key-stores).

The commands that classify need the key: `sync`, `watch`, `classify` and `reclassify`. They resolve it once, and only when a message is due for classification, before they take it; a pass with nothing to classify never runs the key command and needs no key. Without it they still run but classify nothing: no message is taken, so no retry attempt is used and the mail stays queued. The result gains `"classification": {"skipped": true, "reason": "..."}`, where `reason` is one of the fixed key errors below, and is partial (exit 4). `sync` still scans the folders and, with filing on, runs the filing steps. Once the key works, the next pass classifies the queued mail; changing `api_key_command` queues nothing again. `doctor` reports whether the key is present (`provider.key_present`) and why not (`provider.key_error`). `list`, `read`, `correct`, `done`, `reopen`, `export`, `categories` and `filing` commands do not use the key.

### Key command rules

- `api_key_command` is a list of program and arguments. It runs directly, without a shell. For pipes or variables, use `["/bin/sh", "-c", "COMMAND"]`, which is what `setup --key-command` stores.
- Its stdin is closed and its stderr is discarded.
- It has 10 seconds and at most 4 KB (4096 bytes) of output.
- It must exit 0. The key is the first line of its output, with surrounding whitespace removed.
- It runs at most once per command, and only when a message is due for classification, so an idle `watch` raises no key prompt. `watch` runs it again for each pass that has mail to classify, so a rotated key is used from the next pass on.
- `doctor` runs it to check it.
- The key never appears in output, logs or errors. Errors are fixed messages:

| Message | Cause |
| --- | --- |
| `API key command failed (exit N)` | The command exited with code N, or with `exit signal` when a signal ended it. |
| `API key command timed out` | It ran longer than 10 seconds. |
| `API key command printed no key` | Its first line was empty, or it printed more than 4 KB. |
| `API key command could not start` | The program was not found or could not be run. |

The key command runs with the same trust as Himalaya's `password.cmd`: it comes from your own config, which mailtriage writes with mode 0600. Anyone who can edit that file can run commands as you, so keep it writable only by you.

A store that is locked, such as a GPG agent without a cached passphrase or a keyring after logout, makes the command fail. `doctor` then shows `key_error`.

### Environment variable

`api_key_env` holds the name of the variable, for example `OPENROUTER_API_KEY`. mailtriage reads the key from its own process environment when it sends a request. In an interactive shell:

```sh
read -rs OPENROUTER_API_KEY    # paste the key and press Enter; nothing is echoed
export OPENROUTER_API_KEY
mailtriage doctor --account work --json
```

The variable lasts until the shell exits. To fill it from the macOS Keychain in every new shell, add this line to `~/.zshrc` (a key command does the same without a variable):

```sh
export OPENROUTER_API_KEY="$(security find-generic-password -s mailtriage -a openrouter -w)"
```

A supervised `watch`, including the [background service](#background-service), does not see variables from your login shell. Prefer a key command there. Agents: the process that runs mailtriage must have the variable in its environment, because mailtriage inherits it from its parent; see the [Hermes guide](hermes.md).

## Background service

```sh
mailtriage service install --account work
mailtriage service status --account work --json
mailtriage service uninstall --account work
```

`service install` writes a launchd agent (macOS) or a systemd user unit (Linux) and starts it. It runs `watch` for the account with absolute paths, for example:

```text
/usr/local/bin/mailtriage watch --config /Users/alice/.config/mailtriage/mailtriage.json --account work --interval-seconds 60 --limit 100 --json
```

`--interval-seconds` (1 to 86400, default 60) and `--limit` (1 to 500, default 100) are passed to `watch`. The executable is the one that ran `service install`. `mailtriage setup` runs the same install in its last step.

| | macOS (launchd) | Linux (systemd user unit) |
| --- | --- | --- |
| File | `~/Library/LaunchAgents/digital.wirdrei.mailtriage.<account>.plist` | `~/.config/systemd/user/mailtriage-<account>.service` |
| Logs | `<config dir>/logs/<account>.log` (stdout, one JSON line per pass) and `<account>.err` (stderr); the directory has mode 0700 | journald: `journalctl --user -u mailtriage-<account>.service` |
| `install` runs | `launchctl bootout gui/<uid>/<label>` if loaded, then `launchctl bootstrap gui/<uid> <plist>` | `systemctl --user daemon-reload`, `enable <unit>`, `restart <unit>` |
| `uninstall` runs | `launchctl bootout gui/<uid>/<label>`, then deletes the file | `systemctl --user disable --now <unit>`, deletes the file, `daemon-reload` |
| Restart | After an error exit, at most every 30 seconds (`KeepAlive` with `SuccessfulExit` false, `ThrottleInterval` 30) | `Restart=on-failure`, `RestartSec=30` |
| File mode of new files | `Umask` 077 | `UMask=0077` |

`<label>` is `digital.wirdrei.mailtriage.<account>`. With the home config, the log directory is `~/.config/mailtriage/logs`.

- **Repeating `install`** rewrites the file and reloads the job. Run it again after you move the executable or the config, or change your `PATH`.
- **Marked files.** mailtriage marks the files it writes. It replaces or removes only marked files. An unmarked file at the path exits 5 with `PATH exists and was not written by mailtriage; move it away first`.
- **Accounts.** `install` and `status` need the account to be in the config (exit 2, `unknown account`). `uninstall` also works for an account you have removed.
- **PATH.** launchd and systemd start jobs with a short `PATH`. The service file therefore records the `PATH` of the shell that runs `install`, so key tools such as `pass` and `gpg` find their helpers.
- **Key from an environment variable.** The service does not inherit your shell's variables. When the key comes from `api_key_env`, `install` adds a `note` to its result with the command that moves the key into this platform's store, for example `mailtriage setup --update --config /Users/alice/.config/mailtriage/mailtriage.json --account work --key-store keychain` (on Linux `secret-service` or `pass`; `command` everywhere). Do not put the key into the plist or unit yourself: those files are readable, and `service install` rewrites them. Without a key, each pass classifies nothing and is partial (see [The OpenRouter key](#the-openrouter-key)).
- **Linux and logout.** A user unit stops when you log out, unless lingering is on: `loginctl enable-linger $USER`. Setup prints this hint; it does not run the command.
- **Other platforms.** `install` and `uninstall` exit 2 (`unsupported platform`); `status` reports `manager: "none"`. On Linux without `systemctl`, `install` exits 3 and `status` reports `manager: "none"`.

### Service commands

Each command prints `{"schema_version":1,"service":{...}}`.

`install` reports `action` (`installed`), `manager` (`launchd` or `systemd`), `account`, `unit_path`, `log_paths` (empty for systemd), `command` (the `watch` command line as a list) and, when the key comes from an environment variable, `note`.

`uninstall` reports `action` (`uninstalled`, or `not_installed` when no file was there), `manager`, `account` and `unit_path`.

```sh
mailtriage service stop --account work --json
mailtriage service start --account work --json
```

`start` starts the installed service, which then also starts at login again. launchd: `launchctl enable gui/<uid>/<label>`, then `launchctl bootstrap gui/<uid> <plist>` when the job is not loaded, or `launchctl kickstart gui/<uid>/<label>` when it is loaded but idle. systemd: `systemctl --user enable --now <unit>`. It then checks for up to 5 s that the job runs (`launchctl print` shows a PID; `systemctl --user is-active` prints `active`), else it exits 3 with `service did not start; see LOG`. LOG is `<config dir>/logs/<account>.err` on macOS and `journalctl --user -u mailtriage-<account>.service` on Linux. `start` reports `action` (`started`, or `already_running` when the job was running), `manager`, `account` and `unit_path`. A service that is not installed exits 2 with `service for account A is not installed; run mailtriage service install --account A`.

`stop` stops the service and keeps it stopped across logins and reboots; the service file stays. launchd: `launchctl bootout gui/<uid>/<label>`, waiting until launchd drops the job as `install` does, then `launchctl disable gui/<uid>/<label>`. systemd: `systemctl --user disable --now <unit>`; a unit that is also enabled globally or masked is outside this guarantee, and `status` shows it in `enablement`. `stop` reports `action` (`stopped` when launchd had the job loaded or systemd had it running, else `already_stopped`), `manager`, `account` and `unit_path`. A service that is not installed exits 2 with the same message as for `start`.

`start` and `stop` act only on a service that runs the config they read, and call no manager command otherwise:

- When the service runs another config (`config_matches: false`), they exit 5 with reason `service_config_mismatch`: `service for account A runs config X; pass --config X`, the second X shell-quoted. Pass that `--config` to act on that service.
- When its config cannot be told (`config_matches: null`, for example when `launchctl` or `systemctl` fails or systemd needs a daemon reload), they exit 5 with reason `service_config_unknown`: `cannot tell which config the service for account A runs; try again, or reinstall it with mailtriage service install --account A`.

`install` keeps its meaning: it rewrites the service for the config it read. On macOS it now runs `launchctl enable` before `bootstrap`, so it works after a `stop`; systemd's `enable` already covers this. Like `install`, `start` and `stop` exit 2 for an unknown account or an unsupported platform, and 3 when `launchctl` or `systemctl` fails.

`install`, `uninstall`, `start` and `stop` hold a lock per account while they read the config and call the manager: `service-<label>.lock` in `~/Library/Caches/mailtriage` on macOS and `~/.cache/mailtriage` on Linux. The directory comes from `HOME` only, never from `XDG_CACHE_HOME`, so commands for one account from different configs or environments wait for each other. A second command waits up to 30 s, then exits 5 with reason `service_busy`: `another service command for account A is running; try again`. Step 10 of `mailtriage setup` takes the same lock around its install.

`status` reports:

```json
{"schema_version":1,"service":{"account":"work","installed":true,"last_exit_status":0,"last_pass":{"exit_code":0,"finished_at":"2026-10-05T08:00:00.000000+00:00","mode":"dry_run","partial":false},"loaded":true,"log_paths":["/Users/alice/.config/mailtriage/logs/work.log","/Users/alice/.config/mailtriage/logs/work.err"],"manager":"launchd","pid":4242,"running":true,"unit_path":"/Users/alice/Library/LaunchAgents/digital.wirdrei.mailtriage.work.plist"}}
```

| Field | Content |
| --- | --- |
| `manager` | `launchd`, `systemd`, or `none` on other platforms. |
| `installed` | A file written by mailtriage is at `unit_path`. |
| `loaded` | The manager knows the job. |
| `running` | The job is running now. |
| `pid` | Its process ID, or `null`. |
| `last_exit_status` | The last exit status the manager reports, or `null`. |
| `unit_path`, `log_paths` | The service file and the launchd log files (`log_paths` is empty for systemd). |
| `last_pass` | The account's latest `sync` or `watch` pass, from the state database: `finished_at`, `partial`, `exit_code` (0, 4 for partial, or the error's exit code) and `mode` (`off`, `dry_run` or `live`). `null` before the first pass. |

`last_pass` comes from the state database and works without any service. Every `sync` and every `watch` pass that holds the account lock records it. A healthy service shows `running: true` and a `last_pass.finished_at` no older than a few intervals.

### Status of every account

```sh
mailtriage service status --json
```

Without `--account`, `service status` reports every account of the config, one object per account, sorted by account name:

```json
{"schema_version":1,"config":"/Users/alice/.config/mailtriage/mailtriage.json","services":[{"account":"personal",…},{"account":"work",…}]}
```

`config` is the config that `service status` read, as a canonical absolute path. With `--account`, the result has the same `config` and a single `service` object. Every service object has the fields above and these:

| Field | Content |
| --- | --- |
| `service_config` | The `--config` the service runs with. launchd: from `launchctl print`. systemd: from the running process (`/proc/<pid>/cmdline`), else from the `ExecStart` systemd has loaded. A job the manager has not loaded: from the service file. `null` when not installed, or when it cannot be read. |
| `file_config` | The `--config` in the service file, or `null`. |
| `needs_daemon_reload` | systemd only: `true` when the unit file changed after systemd loaded it (`NeedDaemonReload`), else `false`; `null` when unknown or not installed. |
| `config_matches` | `true` when the service runs this config. `false` when it, or its file, names another config. `null` when that cannot be established, for example when `launchctl` or `systemctl` fails or systemd needs a reload. `true` when not installed. |
| `enabled` | Whether the manager starts the service again, for example at login: `true`, `false`, or `null` when unknown. |
| `enablement` | The state behind `enabled`. launchd: `enabled`, `disabled`, or `unknown` when `launchctl print-disabled` fails. systemd: what `systemctl --user is-enabled <unit>` prints; `enabled` and `enabled-runtime` count as enabled, `disabled`, `masked` and `masked-runtime` as not, anything else (such as `static`) as unknown. `not_installed` when not installed. |
| `interval_seconds` | The `--interval-seconds` in the service file, or `null`. |
| `filing_mode` | The account's configured filing mode: `off`, `dry_run` or `live`. |
| `identity` | The account's `identity` from the config. |

Two configs can name the same account. The service of an account belongs to one config at a time: `service install` from the other config rewrites it. `config_matches: false` shows that the service runs the other config; `service_config` names it.

### Your own supervisor

On a server you may prefer a system-wide unit that runs as a dedicated user. First give the config a key command: set `provider.api_key_command` to a command that prints the key for that user, for example from `pass` or another key store (see [The OpenRouter key](#the-openrouter-key)). The unit then holds no secret. A systemd example, in `/etc/systemd/system/mailtriage-work.service`:

```ini
[Unit]
Description=mailtriage watch for account work
Wants=network-online.target
After=network-online.target

[Service]
User=mailtriage
ExecStart=/usr/local/bin/mailtriage watch --config /etc/mailtriage/mailtriage.json --account work --json
Restart=on-failure
RestartSec=30

[Install]
WantedBy=multi-user.target
```

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now mailtriage-work.service
```

- The key command runs as the user named in `User=`, with no terminal. That user's key store must hold the key and release it without a prompt.
- If the config uses `api_key_env` instead, the variable must reach the mailtriage process; mailtriage does not read it from a file. Do not put the key into a unit or plist with `Environment=` or `EnvironmentVariables`: those files are readable, and `mailtriage service install` rewrites the ones it manages. Keep the key in a key store and give the config a key command instead.
- `mailtriage service` does not manage this unit. `service status` reports it as not installed, but its `last_pass` still shows the latest pass.
- The user named in `User=` needs write access to `state_dir` and to the directory that holds `mailtriage.json`. mailtriage creates `mailtriage.lock` there and rewrites the file for `filing enable`, `filing disable` and `categories apply`.
- Supervisors start jobs with a short default `PATH` (launchd: `/usr/bin:/bin:/usr/sbin:/sbin`), which is why `engine.binary` should be an absolute path.

## Daily use

The commands below omit `--config`; see [Where mailtriage finds the config](#where-mailtriage-finds-the-config). Pass it explicitly in scripts and supervised jobs.

### Sync and watch

```sh
mailtriage sync --account work --limit 100 --json
mailtriage watch --account work --limit 100 --interval-seconds 60 --json
```

`sync` runs one bounded pass: it scans the watched folders, fetches new messages, classifies queued ones and, with filing on, files them. Its result reports `discovered`, `fetched`, `classified`, `cached`, `failed`, `pending` and `scan_errors`, plus `coverage`, with filing on `filing`, and, when the OpenRouter key is unavailable, `classification` (`{"skipped": true, "reason": "..."}`; see [The OpenRouter key](#the-openrouter-key)). `--limit` is 1 to 500 (default 100).

`watch` repeats the pass every `--interval-seconds` (1 to 86400, default 60) until Ctrl-C or SIGTERM. With `--json` it prints one JSON line per pass and a final stop object with `passes`, `partial_passes` and `skipped_passes`. A partial pass does not stop `watch`. A pass that `mailtriage.json` or the Himalaya configuration changed under is skipped: `watch` prints its error object (code 5), counts it in `skipped_passes` and runs the next pass with the current configuration. Any other error, including a changed account binding or a second worker, ends `watch` with that error's exit code. After a graceful stop `watch` exits 0, or 4 if any pass was partial or skipped. Run it under a supervisor that restarts it, such as the [background service](#background-service).

Only one worker runs per account at a time. `sync`, `classify`, `reclassify` and each `watch` pass take a lock in `state_dir`; a second one exits 5 with `an account worker is already running`. Run either `watch` or scheduled `sync` for an account, never both.

A failed fetch or classification is retried on later passes (see `policy.max_attempts`). A pass without a key takes no message and uses no attempt. A failed message whose source is still present stays in the attention view. A message whose source occurrence has disappeared stays in the `all` view, and its local content is kept. A complete scan needs a stable mailbox UIDVALIDITY. Mail that enters and leaves a watched folder between two passes is never seen.

### Query and correct

```sh
mailtriage list --account work --json
mailtriage list --account work --view all --limit 50 --json
mailtriage list --account work --category correspondence --urgency high --action-required true --json
mailtriage read --account work --id MESSAGE_ID --json
mailtriage correct --account work --id MESSAGE_ID --action-required true --json
mailtriage correct --account work --id MESSAGE_ID --category transactions --json
mailtriage correct --account work --id MESSAGE_ID --clear urgency --json
mailtriage done --account work --id MESSAGE_ID --json
mailtriage reopen --account work --id MESSAGE_ID --json
mailtriage export --account work --json > mailtriage-export.json
```

`list` and `read` use only the local database; they make no IMAP or provider request. `list` shows the attention view by default: open messages with at least one entry in `attention_reasons`, such as `high_urgency`, `medium_urgency`, `action_required`, `uncertain`, `failed`, `incomplete_decision` or `review_mode`. Done messages never appear there. `--view all` lists every stored message. `--limit` is 1 to 500 (default 50). Pass `next_cursor` from a response as `--cursor` to get the next page. A cursor belongs to its query and to the database revision; after any change it is refused with exit code 5, and you start again from the first page.

`correct` sets an override for one decision: `--urgency`, `--category` (a category `id`) or `--action-required`. `--clear FIELD` removes the override, and the model's decision applies again. Overrides survive reclassification. With filing on, a category correction also moves the message (see [Corrections from a mail client](#corrections-from-a-mail-client)).

`done` marks a message handled. It leaves the attention view, stays done when the message is classified again, and nothing changes on the server. `reopen` reverses it and classifies the message again if the configuration changed since.

`export` prints all stored messages, classifications and attempts as JSON. Use it for inspection. It is not a restore format; back up the state directory instead (see [State and backups](#state-and-backups)).

## Categories

```sh
mailtriage categories export --account work --json > categories.json
mailtriage categories validate --file categories.json --account work --json
mailtriage categories apply --account work --file categories.json --json
mailtriage reclassify --account work --dry-run --json
mailtriage reclassify --account work --since 2026-09-01 --json
```

A category file holds a JSON array of categories or an object with a `categories` array; `categories export` writes the object form. The [category fields](#a-complete-configuration) are the same as in `mailtriage.json`: a unique `id`, a non-empty `name` and `description`, optional `examples` and `folder`, and exactly one category with `catch_all: true`.

`categories validate --file FILE` checks the file alone. With `--account NAME` it checks the file against that account's configuration, including the folder rules when the account's filing is on. Neither form writes anything.

`categories apply` writes the categories into `mailtriage.json`. A change to a category's `id`, `description`, `examples` or `catch_all`, or an added or removed category, advances `taxonomy_revision` and queues all open messages for classification. A rename (a change to `name` only) keeps the ID and does not reclassify. Done messages stay done; a reopened message is classified under the current categories. Removing a category that a manual correction uses fails with `category has manual corrections; remap or clear them before removal` (exit code 2); correct those messages to another category or clear the correction first. Keep IDs when you edit category files by hand.

`reclassify` queues the matching stored messages for classification and processes up to `--limit` of them (1 to 500, default 100); later passes process the rest. `--since YYYY-MM-DD` selects messages first observed on or after that local date. `--dry-run` reports `matched` and `will_process` and changes nothing.

### Checking a change before you apply it

```sh
mailtriage categories export --account work --json > categories.json
# edit categories.json
mailtriage categories validate --file categories.json --account work --json
mailtriage categories apply --account work --file categories.json --expect-digest "$(jq -r .digest categories.json)" --json
```

- `categories export` and `categories validate --account` report `digest`: `v1:` and a SHA-256 of the account's categories and whether its filing is on. It changes whenever something changes how `apply` would write the categories. The key order of a file and `"folder": null` versus no `folder` do not change it; the category order does.
- `categories apply --expect-digest DIGEST` writes nothing and exits 5 with `categories changed since export; export again` (reason `categories_changed`) when the account's categories changed since the export (another window or agent applied a change), or filing was turned on or off. Export again and redo the edit.
- `categories validate --file FILE --account NAME` also reports `changes`, computed after the same folder rules `apply` uses:
  - `added` lists category IDs;
  - `removed` lists `{"id","folder"}`;
  - `renamed` and `folders_changed` list `{"id","from","to"}`;
  - `edited` lists categories whose description, examples or default flag changed;
  - `reclassifies` is `true` exactly when `apply` would sort all open mail again (it advances `taxonomy_revision`, which uses your OpenRouter key).

  Folder names in `changes` are for display.

## Filing into folders

Filing makes the classification visible in every mail client. With filing on, each classified message in a source folder (the engine's `mailboxes`, usually `INBOX`) is moved into a top-level folder for its category, and mail that needs action or has `high` urgency gets `\Flagged`. Filing is off by default and is enabled per account; `mailtriage setup` turns on `dry_run` for a new account unless you pass `--filing off`. Filing needs an `engine`; `filing enable` refuses an account without one.

```sh
mailtriage filing enable --account work --mode dry-run --json
mailtriage filing plan --account work --json
mailtriage filing enable --account work --mode live --json
mailtriage filing backfill --account work --days 30 --json
mailtriage filing backfill --account work --days 30 --apply --json
mailtriage filing status --account work --json
mailtriage filing log --account work --limit 50 --json
```

`filing.mode` has three values:

| Mode | Effect |
| --- | --- |
| `off` | The default when the account has no `filing` block. The mailbox is left exactly as before. |
| `dry_run` | Each pass watches the category folders and plans moves and flags, and writes nothing: no folder is created, no message is moved or flagged. `sync` reports the planned counts under `filing.planned`, and `filing plan` lists the actions. |
| `live` | mailtriage creates and subscribes the category folders, moves mail and adds flags. |

`filing enable --mode dry-run` or `--mode live` and `filing disable` write `filing.mode` into `mailtriage.json` under the configuration lock (`dry-run` on the command line is `dry_run` in the file). The next pass uses the new mode.

### Rollout

1. Run `filing enable --mode dry-run`, or keep the `dry_run` that setup chose.
2. Keep `watch` or scheduled `sync` running for a few days. Check `filing plan` and the `filing.planned` counts of each pass. `filing plan`'s `total` counts the actions of the next pass only, at most `filing.max_actions_per_pass`; it is not the size of the backlog.
3. Check that your provider has a recorded go (see [Provider check](#provider-check)).
4. Run `filing enable --mode live`.
5. Optionally file older mail with `filing backfill`.

Only new mail is filed automatically: mail whose server internal date is at or after the time filing was enabled. Mail delivered during the dry-run days counts as new, so the first live passes file it. Mail that arrives while filing is off counts as older mail when filing is enabled again.

To file older mail, preview it with `filing backfill --days N` or `filing backfill --all`, then repeat the command with `--apply`. `--days` (1 to 3650) compares against the server's internal date. `--apply` requires `live` and makes each listed message eligible once. The moves happen over the following passes, at most `filing.max_actions_per_pass` (default 200) per pass.

### Folders

`filing enable` writes an explicit `folder` (the category's current `name`) into every category that has none, so a later rename does not move its folder. `categories apply` with filing on keeps each existing category's folder the same way. `mailtriage setup` writes an explicit `folder` for each default category.

A category folder must:

- be a single path segment of printable ASCII, at most 200 bytes, without leading or trailing spaces,
- contain none of `/ . * % " \ &` and not start with `-`,
- not be a case variant of `inbox`, other than the literal `INBOX`, which keeps the category's mail in its source folder,
- be unique among the account's categories, ignoring case.

With filing on, the source mailboxes in `engine.mailboxes` must be printable ASCII without `\`, `"` or `&` and must not start with `-`. They may contain `/` or `.`. `filing enable` and `categories apply` refuse invalid folders with `categories need a valid folder: <ids>`; set `folder` for those categories in the category file and run `categories apply`. They refuse unsafe source mailboxes with `source mailboxes are not safe to file from: ...`.

mailtriage adds the server's personal namespace prefix (for example `INBOX.`) to folder names. An existing folder with a category's name is adopted when the server reports that it has no special role. When the server cannot report roles, the folder waits for `filing adopt --folder NAME`. Folders with a special role (Sent, Trash, Junk, Archive, All Mail and similar, by role or by name) are never used, and a deleted category folder is not recreated. `filing status` lists the affected categories under `paused_categories`.

### What is moved and flagged

A message is moved automatically at most once, only out of a source folder, and only when its classification is current or you corrected its category. A current classification was made under the current categories, provider and policy, from complete input. Review mode does not hold filing back.

`\Flagged` is added once, when the effective decision is `action_required` or `high` urgency, and only to new mail, backfilled mail or mail that mailtriage filed. A message that already carries `\Flagged` counts as flagged, so removing the flag in a mail client is respected. Set `filing.flag` to `false` to add no flags.

### Corrections from a mail client

Moving a message into another category's folder is a category correction, the same as `correct --category`. Moving it back to a source folder pins it there; it is not filed again until `filing unpin --id ID`. Moving it into its own category's folder keeps it there and removes any pin. Mail delivered straight into a category folder, for example by a server rule, is recorded as filed by you in that category.

With filing on, `correct --category` and `correct --clear category` move the message to the resulting category's folder. `filing pin --id ID` keeps a message in its source folder and moves it back if it was filed.

Archiving or deleting a message in a mail client means done. Once the message is gone from every watched folder, a later pass that fully scanned every watched folder, with no paused folder and no unidentified arrival outstanding, marks it done locally. `list --view all` then shows it with `review_state` `done`. If the message comes back, that inferred done is reopened. A message you marked done yourself is never reopened by observation.

### Status and log

`filing status` makes no mailbox calls. It reports:

| Field | Content |
| --- | --- |
| `mode`, `state_mode`, `enabled_at` | The configured mode, the mode the last pass stored, and when filing was enabled. |
| `capabilities` | The server's MOVE, UIDPLUS and SPECIAL-USE support from the last pass. |
| `folders` | Every folder with its state, pause and subscription. |
| `paused_categories` | Categories whose folder cannot be used. |
| `intents` | Move and flag intents counted by state, finished ones included. |
| `blocked`, `blocked_ids` | Blocked messages; up to 50 entries of `{id, blocked_reason}`. |
| `quarantined`, `quarantined_ids` | Quarantined messages; up to 50 ids. |
| `ambiguous`, `ambiguous_ids` | Messages found in several watched folders; up to 50 ids. |
| `unresolved_arrivals`, `unresolved_arrival_items` | Mail mailtriage could not identify; up to 50 items. |
| `eligible_unfiled` | Messages in a source folder that the next passes may file. |
| `stale_requests` | `count` and `ids` of requests for a category that no longer exists. Correct or unpin them. |
| `alias_conflicts`, `problems`, `last_pass` | Folder alias conflicts, problems of the last pass, and its summary. |

`filing log` lists filing events, newest first; `--id ID` limits it to one message. `list` and `read` items carry a `placement` object: `folder`, `location_state`, `filed_by`, `pinned`, `flagged`, `blocked_reason` and `pending_action`. With filing on, `doctor` adds a `filing` block with the server's capabilities, folders and problems.

### Safety rules

mailtriage never deletes or expunges mail, never removes a flag, never changes `\Seen`, and never renames or deletes a folder. It writes only to servers with the IMAP MOVE extension; there is no copy-and-delete fallback. Every move and flag is journaled before it is sent and is resolved by observation after a failure or crash. If a write may have run against a recreated mailbox, mailtriage reverts it where the server reported where the mail went. Otherwise it pauses that folder and quarantines the mail that arrived. Nothing is written to a paused folder or a blocked message until you act.

A Himalaya mailbox alias that points a watched folder elsewhere stops that folder's scanning and fetches, and all filing writes, until the alias is fixed. A change to the Himalaya configuration during a pass, or to `mailtriage.json` before the pass writes, aborts the pass with exit code 5. `sync` exits with that code; `watch` prints the error object, continues, and uses the new configuration in its next pass.

### Lifting blocks and pauses

Check the mailbox before you lift a block or a pause.

| Case | Meaning | What to do |
| --- | --- | --- |
| Folder pause (`pause_reason` in `folders`) | A write may have run against a recreated folder. | After checking the folder, `filing retry --folder NAME` releases the pause and rescans the folder. |
| Folder in state `needs_confirmation` | The server could not report the folder's role. | `filing adopt --folder NAME` confirms it. |
| `move_failed` | The move kept failing. | `filing retry --id ID` lifts the block and makes the message eligible once, so a message still in a source folder is filed, and possibly flagged, on the next pass. |
| `quarantined` | The message arrived while a write may have run against a recreated mailbox. | `filing retry --id ID` lifts it, with the same effect as for `move_failed`. |
| `duplicate_copy` | The message is in two folders. | Delete one copy in your mail client, run `sync` so mailtriage observes it, then run `filing retry --id ID`. While both copies are recorded, the retry is refused with exit code 5 (`remove one copy first, then sync and retry`). Lifting `duplicate_copy` does not make the message eligible once. If the copy you kept is in a source folder and the message is still new mail, the next pass files it again; `filing pin --id ID` keeps it in the source folder instead. `filing pin` and `correct --category` do not lift this block. |
| `merge_conflict` | An unresolved arrival duplicates this message. `filing log --id ID` shows the arrival in `detail.arrival_id`. | `filing retry --id ID` does not lift this block. `filing dismiss --arrival N` marks the arrival reviewed and lifts it. `filing retry --arrival N` fetches the arrival again instead; a repeated conflict blocks again. |
| Unresolved arrival | Mail that mailtriage could not identify. | `filing retry --arrival N` fetches it again; `filing dismiss --arrival N` marks it reviewed. |

### Provider check

Before you use `live` on a real mailbox, the live provider check in the [filing design](superpowers/specs/2026-10-04-imap-category-filing-design.md#live-provider-check) must have recorded a go for your provider (Gmail / Google Workspace, Microsoft 365 / Outlook.com, iCloud, Fastmail / Dovecot) in [the verification receipt](verification.md#live-provider-check-required-before-live-on-a-real-mailbox). No provider has been checked yet, so use `dry_run` until yours is. A no-go will be listed here.

### Filing exit codes

| Code | Filing cases |
| --- | --- |
| 0 | Success. |
| 2 | Invalid input or configuration: no mail engine, invalid folders, `backfill --apply` outside `live`, an unknown folder or message, an arrival that is not unresolved. |
| 3 | Operational failure. |
| 4 | Partial sync. Any filing error in a pass makes it partial. |
| 5 | Conflict: the configuration changed during the command or pass, a placement changed concurrently, a message's identity is not yet established, a `duplicate_copy` retry while both copies are still recorded, or a changed account binding. |

## Tray

`mailtriage-tray` shows each account's state in the menu bar (macOS) or the system tray (Linux). It starts, stops and installs the background service, opens the service log, and edits categories in a window. It keeps no state of its own: every action is one `mailtriage` command, so you or an agent can do the same in a terminal.

```text
mailtriage-tray [--config PATH] [--mailtriage PATH]
mailtriage-tray categories [--account NAME] [--config PATH] [--mailtriage PATH]
mailtriage-tray autostart enable|disable|status [--config PATH] [--mailtriage PATH] [--json]
```

### What the tray runs

The tray uses the `mailtriage` next to its own executable, else the first on your `PATH`; `--mailtriage PATH` overrides both. It uses the config that `mailtriage service status` finds (see [Where mailtriage finds the config](#where-mailtriage-finds-the-config)); `--config PATH` overrides it. Both are resolved once at start and made absolute. A later change of `PATH`, the working directory or a symlink therefore never redirects a running tray; restart it to follow one.

Every command ends with `--config CONFIG`, the absolute path of the tray's config:

| Action | Command |
| --- | --- |
| Refresh: every 15 s, after each action, and "Refresh now" | `mailtriage service status --json --config CONFIG` |
| Start service | `mailtriage service start --account NAME --json --config CONFIG` |
| Stop service | `mailtriage service stop --account NAME --json --config CONFIG` |
| Install service | `mailtriage service install --account NAME --json --config CONFIG` |
| Edit categories… | `mailtriage-tray categories --account NAME --config CONFIG --mailtriage CLI` |
| Load the window (open, Reload, another account) | `mailtriage service status --json --config CONFIG`, then `mailtriage categories export --account NAME --json --config CONFIG` |
| Check the window's edits | `mailtriage categories validate --account NAME --file DRAFT --json --config CONFIG` |
| Apply | `mailtriage categories apply --account NAME --file DRAFT --expect-digest DIGEST --json --config CONFIG` |
| Move filed mail… | `mailtriage filing refile --account NAME --json --config CONFIG` |
| Move (a folder no longer used) | `mailtriage filing refile --account NAME --folder NATIVE --apply --json --config CONFIG` |
| Move all | `mailtriage filing refile --account NAME --apply --json --config CONFIG` |
| Start at login | `mailtriage-tray autostart enable` or `disable` with the tray's `--config` and `--mailtriage` (the tray runs the same code itself) |

- `CLI` is the absolute path of `mailtriage`. `DRAFT` is a file in the window's private temporary directory, `DIGEST` the `digest` of the last export, and `NATIVE` the folder's server name from the preview's `folders`.
- "Open log" opens one of the service's `log_paths` with `open` (macOS). "Copy log command" puts `journalctl --user -u mailtriage-NAME.service -e` on the clipboard (Linux).
- The tray also runs `mailtriage --version` to compare versions. A tray started without `--config` runs its first `service status --json` without it, and uses the `config` that command reports from then on.

### Install the tray

Each release also carries `mailtriage-tray-vVERSION-macos-arm64.tar.gz`, `-linux-amd64.tar.gz` and `-linux-arm64.tar.gz`, each with a `.sha256` file and listed in `SHA256SUMS`. The archive holds `mailtriage-tray` and the license. Use the same release as `mailtriage`, verify the archive like the CLI's, and install the tray in the same directory as `mailtriage`:

```sh
VERSION=0.1.0
gh release download "v$VERSION" --repo wir-drei-digital/mailtriage --pattern "mailtriage-tray-v$VERSION-macos-arm64.tar.gz*"
shasum -a 256 --check "mailtriage-tray-v$VERSION-macos-arm64.tar.gz.sha256"   # Linux: sha256sum --check
tar -xzf "mailtriage-tray-v$VERSION-macos-arm64.tar.gz"
sudo install -m 0755 mailtriage-tray /usr/local/bin/mailtriage-tray
```

The macOS executable is unsigned and not notarized, like the CLI. From source, with a stable Rust toolchain (on Linux, install the build packages from [Linux requirements](#linux-requirements) first):

```sh
cargo build --release --locked -p mailtriage-tray
sudo install -m 0755 target/release/mailtriage-tray /usr/local/bin/mailtriage-tray
mailtriage-tray --version
```

Start it:

```sh
mailtriage-tray
```

The icon appears in the menu bar (macOS, without a Dock icon) or the system tray (Linux), and stays until you choose Quit.

- One tray runs per user. A second start prints `mailtriage-tray is already running` and exits 0.
- When `mailtriage-tray` is replaced on disk by a version that runs, the running tray restarts itself onto it within about 15 s, with the same config and `mailtriage`. An open categories window keeps running.

### Start at login

Choose "Start at login" in the menu, or:

```sh
mailtriage-tray autostart enable
mailtriage-tray autostart status --json
mailtriage-tray autostart disable
```

- `enable` writes a login item: `~/Library/LaunchAgents/digital.wirdrei.mailtriage-tray.plist` on macOS, `$XDG_CONFIG_HOME/autostart/mailtriage-tray.desktop` on Linux (`~/.config/autostart/mailtriage-tray.desktop` when `XDG_CONFIG_HOME` is unset or not absolute).
- The login item records the tray's absolute path with `--config` and `--mailtriage`, both absolute and resolved as above. Without `--config`, `enable` runs `mailtriage service status --json` once to learn the config. On macOS it also records your current `PATH`, as `service install` does. Run `enable` again after you move `mailtriage`, the tray or the config.
- macOS: the tray starts at login and restarts after a crash, but Quit stays quit until the next login (`RunAtLoad`, `KeepAlive` with `SuccessfulExit` false). `enable` also runs `launchctl enable gui/<uid>/digital.wirdrei.mailtriage-tray`. It does not start a second tray.
- `disable` deletes the file and leaves the running tray alone.
- `status` reports `enabled: true` when the file exists and, on macOS, launchd has not disabled the label.
- mailtriage marks the files it writes and changes only marked files.

Each prints `{"schema_version":1,"autostart":{"enabled":true,"path":"…","config":"…","mailtriage":"…"}}`: one line with `--json`, pretty-printed without it.

| Code | Meaning |
| --- | --- |
| 0 | Success. |
| 2 | Invalid arguments. |
| 3 | The file cannot be written or removed, `launchctl enable` failed, or the config or `mailtriage` cannot be found (pass `--config` or `--mailtriage`). |
| 5 | A file at the path was not written by mailtriage: `PATH exists and was not written by mailtriage; move it away first`. It is left alone. |

### What the menu shows

```text
mailtriage: daniel needs attention
──────
daniel (daniel@example.com)  ▲ Some mail skipped  ▸  Running · filing live
                                                      Last check 12:03, some mail skipped · v0.3.0
                                                      ──────
                                                      Stop service
                                                      Open log
                                                      Edit categories…
info  ○ Not installed                             ▸  Not installed · filing off
                                                      ──────
                                                      Install service
                                                      Edit categories…
──────
Refresh now
Start at login
Quit
```

Each account shows its name, its identity when that differs, a mark (● running, ○ off, ▲ needs attention) and its state:

| State | What it means | What to do |
| --- | --- | --- |
| Running · checked HH:MM | The service runs and its last check was complete and recent. | Nothing. |
| Starting… | The service runs and has not finished its first check yet. | Wait one interval. |
| Restarting… | The service should run but has stopped, for less than 2 minutes so far. launchd and systemd restart it after an error. | Wait. After 2 minutes it shows "Problem since". |
| Some mail skipped | The last check was partial: some folders or messages failed, or the OpenRouter key was unavailable. | Open the log and run `mailtriage doctor --account NAME`. `mailtriage list --account NAME --view all --json` shows each message's `error`. |
| Configuration changed | The last check stopped because `mailtriage.json` changed while it ran. | Nothing: the next check uses the new file. |
| Another mailtriage was busy | Another worker for this account, such as a second `watch` or a `sync`, was running when the check started. | Run one worker per account: stop the other one. |
| No check since HH:MM | The service runs, but its last check finished more than 3 intervals plus 30 minutes ago. It may hang. | Open the log, then Stop service and Start service. |
| No check yet | The service has run that long without finishing a check. | As for "No check since". |
| Problem since HH:MM | The service should run but has not run for 2 minutes or more, or its last check failed. The submenu says which. | Open the log ("Open log" and "Open error log" on macOS; on Linux, "Copy log command" and run it in a terminal), and run `mailtriage doctor --account NAME`. |
| Stopped | The service is installed but stopped, and does not start at login, for example after Stop service. | Start service, or `mailtriage service start --account NAME --config CONFIG`. |
| Not installed | The account has no background service. | Install service, or `mailtriage service install --account NAME --config CONFIG`. |
| Runs another config | The account's service belongs to another `mailtriage.json`; the submenu names it. Start and Stop are not offered: they would act on that config's service. | Open the tray with that `--config`, or move the service to this config with `mailtriage service install --account NAME --config CONFIG`. |
| Status unknown | The tray cannot tell which config the service runs (`launchctl` or `systemctl` failed, or systemd needs a reload), or, for a stopped service, whether it starts at login. The submenu says which. | Wait for the next refresh. If it stays, run `mailtriage service status --account NAME --json --config CONFIG`; `mailtriage service install --account NAME --config CONFIG` rewrites the service for this config. |
| No background service on this system | Neither launchd nor systemd is available. | Run `watch` under [your own supervisor](#your-own-supervisor). "Edit categories…" still works. |

- **The submenu** shows whether the job runs and the filing mode, then the last check, for example "Last check 12:03, some mail skipped · v0.3.0" (the version of mailtriage that ran it). A job that runs although its manager would not start it again adds "Disabled: won't start again after logout" and keeps "Stop service". While a service command runs, its item reads "Starting…", "Stopping…" or "Installing…".
- **The summary line** reads "All accounts running", "NAME needs attention", "N of M accounts running", "Stopped", "No background service on this system", or a problem from [When the tray shows a problem](#when-the-tray-shows-a-problem). It is also the icon's tooltip.
- **The icon** shows the worst state across accounts: an envelope with a `!` badge for a warning, a problem or "Status unknown"; else a plain envelope when a service runs, starts or restarts; else an outline (every account stopped, not installed, running another config, or without a service manager). On macOS it follows the menu bar's light or dark look, so a warning and a problem look the same. Linux uses the same shapes in green, amber, red and grey.
- **Times** are local `HH:MM`, with the date when not today ("Oct 5, 23:59").
- **"(stale)"** after the summary: the last refresh failed. The menu keeps the last good state for up to 2 minutes, then shows the failure.
- **"mailtriage X and tray Y differ; run mailtriage update"** appears when the CLI's version differs from the tray's. Install both from the same release.
- **Notices.** A failed action, or a line from the categories window such as "the categories window is already open", shows under the summary for 60 s, with "Show details" when there is more.

### The categories window

Open it with "Edit categories…" in an account's submenu, or:

```sh
mailtriage-tray categories --account work
```

Without `--account` it shows the first account. One window runs per config. A second start for the same config prints `the categories window is already open` and exits 0; on macOS it first brings the open window to the front. The tray shows that line as a notice. `categories` exits 0 when the window is closed or already open, 2 for invalid arguments, and 3 when `mailtriage` cannot be found or the first load fails, after the window has shown why.

The window shows the account selector and Reload at the top, the categories on the left (with their mail folder; the default one is marked "default") and the selected category on the right: Name, "What belongs here" (the description), "Use for mail that fits nowhere else" (the default category; exactly one), Mail folder (only when the account's filing is not `off`), Advanced, and "Remove category". The footer shows the check result on the left and "Move filed mail…", Revert and Apply on the right.

- **Checks run as you type.** Half a second after your last change, the window writes the categories to a private temporary file and runs `categories validate`. The footer shows the changes in words, for example "1 added, 1 renamed, mail folder changed for Newsletters", or what is wrong, with "Show details".
- **Apply** is enabled when the latest check of your current edits passed and they differ from what was loaded; otherwise its tooltip says why. Apply (or ⌘/Ctrl+S) lists the changes and asks first. When the change sorts all open mail again, it adds: "All open mail in this account will be sorted again. This uses your OpenRouter key and takes a few passes." After a successful apply, the footer says "Saved." and what happens next, and the window loads the categories again.
- **"These categories were changed somewhere else."** Another program, such as another window, an agent or `categories apply`, changed this account's categories since the window loaded them, or filing was turned on or off. Apply passes `--expect-digest`, so nothing was written. Choose "Reload (discard my edits)" or "Keep editing".
- **"mailtriage is busy; trying again…"** Another command is editing `mailtriage.json`. The window retries three times, 2 s apart, then shows "Try again".
- **"The configuration changed while saving. Checking your changes again."** `mailtriage.json` changed while Apply ran; your edits are checked again.
- **Removing a category** asks first and says what happens: its mail stays in its folder until you move it, and new mail is sorted into the remaining categories. Nothing is written until Apply. The default category cannot be removed; choose another default first.
- **Mail folder.** Left empty, the folder is the category's name. The field always shows the folder mail will go to.
- **Advanced.** A new category's ID is suggested from its name and can be changed until you apply. Existing IDs cannot change: corrections and folders refer to them. Examples, one per line, are saved with the category and not sent to the classifier.
- **Revert** drops your unsaved edits without asking. Switching accounts, Reload and closing the window ask first ("Discard changes?").
- **Feedback.** Every command shows a spinner and a verb ("Loading…", "Checking…", "Applying…", "Moving…"). While a load, Apply or a move runs, the form is read-only. Closing during Apply or a move shows "Finishing…" and closes when it ends. The draft files are removed when the window closes.

**Moving filed mail.** "Move filed mail…" (shown when the account's filing is not `off`) opens a panel below the form. It also opens after an apply that added, removed or re-pointed a category, or sorts mail again. It mirrors `mailtriage filing refile`:

- each folder no longer used, with how many messages can move now and how many may move once they are sorted again, and "Move" (which also marks the waiting ones);
- "Move all N" for every message whose folder no longer matches its category, including those in folders no longer used;
- how many messages are still being sorted again, and "Not moved" with each reason and its count.

After a move, the panel says how many messages were marked, for example "Marked 30 messages; 12 more will move if their new category calls for it." The background service moves them at its next pass; when it does not run, the panel says to start it or run `mailtriage sync`. With filing in `dry_run`, the panel only previews and says "Moving filed mail needs filing set to live." "Close" in the panel's heading row closes it; it waits while a move runs.

**Keyboard.** ⌘N (Ctrl+N on Linux) adds a category, ⌘S (Ctrl+S) opens the apply confirmation, Esc closes a dialog, and ⌘W (Ctrl+W) closes the window, asking about unsaved edits. Tab follows the visual order. Every control has a label that screen readers announce.

### Linux requirements

- GTK 3, the Ayatana AppIndicator library and libxdo, as `tray-icon` documents. On Ubuntu: `sudo apt-get install libayatana-appindicator3-1 libxdo3`; GTK 3 comes with the desktop. Building from source also needs `libgtk-3-dev libayatana-appindicator3-dev libxdo-dev`.
- A desktop with a StatusNotifier host: KDE Plasma, or GNOME with the AppIndicator extension, which Ubuntu ships.
- OpenGL for the categories window.

### When the tray shows a problem

The summary line, and the categories window, show these:

| Message | What to do |
| --- | --- |
| mailtriage not found (looked in …) | Install `mailtriage` in the same directory as `mailtriage-tray` or on your `PATH`, or start the tray with `--mailtriage PATH`. |
| Not set up. Run `mailtriage setup` in a terminal. | No config was found, or `--config` names a missing file. Run `mailtriage setup`; the tray finds the config at its next refresh. The tray never runs setup itself. |
| mailtriage is older than the tray; run `mailtriage update`. | The CLI lacks fields this tray needs; "Show details" names the missing one. Install `mailtriage` from the tray's release. |
| Any other message | The failed command's error. In the menu, "Show details" copies the command, its exit code and its output; in the window, it shows them with "Copy". Run the command in a terminal to see more. |

## Reference

### Output and exit codes

With `--json`, every result is one line of JSON on stdout, and so is every error: `{"schema_version":1,"error":{"code":N,"message":"..."}}`. Without `--json`, results are pretty-printed JSON and errors go to stderr as `mailtriage: MESSAGE`. Every result has a `schema_version`. Error messages omit message bodies and credentials.

Some errors also carry a machine-readable `reason` in the error object, for scripts that react to a class of error rather than to its message: `config_changed` (`mailtriage.json` or the Himalaya configuration changed during the command), `config_busy` (another command is editing `mailtriage.json`), `account_busy` (another worker for the account is running), `binding_conflict` (the account binding changed, see [Account binding](#account-binding)), `categories_changed` (`categories apply --expect-digest` found other categories), `service_config_mismatch` (the account's service runs another config, see [Service commands](#service-commands)), `service_config_unknown` (the config of the account's service cannot be told) and `service_busy` (another service command for the account held the service lock for 30 s). An error without a reason has no `reason` key.

| Code | Meaning |
| --- | --- |
| 0 | Success. A query exits 0 even if messages need attention, and `doctor` exits 0 even if `ready` is `false`. |
| 2 | Invalid input or configuration: an unknown flag value, account, category or message, a missing or invalid `mailtriage.json`. |
| 3 | Operational failure, reported as `Operation failed; check configuration and dependency availability`, for example when Himalaya cannot run. |
| 4 | Partial result: a pass, `classify` or `reclassify` with failed messages or scan errors, or whose classification was skipped because the key is unavailable; a pass with filing errors; `watch` at stop after a partial or skipped pass. |
| 5 | Conflict: the configuration changed during the command, another worker is running, the account binding changed, a cursor expired, or a placement changed concurrently. |

`setup` and `service` have their own cases; see [Setup exit codes](#setup-exit-codes) and [Background service](#background-service).

### Account binding

The first command that opens an account stores a binding in the state directory: the account's `identity`, the Himalaya account name, `imap.server` and the other IMAP settings except secrets. Settings whose key contains `password`, `passwd`, `token` or `secret` are left out, so you can rotate a password or token. After a change to anything else in the binding, commands for that account exit 5 with `account binding changed or state unavailable; verify config and use a new namespace for a different mailbox`. To use a different mailbox, add a new account name in `mailtriage.json` (or with `mailtriage setup --update --account NEWNAME`). `mailtriage setup` checks the binding before it writes and refuses a change with exit 5. An opaque OAuth token helper can change its underlying account without changing the visible configuration, so keep `identity` accurate.

### State and backups

The SQLite database and normalized message text under `state_dir` are authoritative private data. Raw messages and attachments are not stored. Stop the worker before you copy the directory, including any SQLite WAL files; for the background service, run `service uninstall` first and `service install` afterwards. Keep a JSON export as well. Losing the database loses your corrections and Done state; operational state is restored from the database copy, not from the export. The state directory is for one local owner. Do not share it over a network filesystem. Independent installations do not synchronize state.

### Verification boundary

The offline fixture tests, the adapter contract tests and the Dovecot end-to-end test do not prove a real provider's `\Seen` behavior, the live OpenRouter gateway, or classification quality on representative mail. Those need a test mailbox, an API key and labeled examples. Check thresholds against labeled mail before you turn off `review_mode`. The fake provider only exercises the plumbing, and no accuracy claim is made until the live Jev evaluation is complete. The tests of setup and the background service use fake key tools, `launchctl` and `systemctl`; the [human check](verification.md#guided-setup-on-a-real-machine-human-check) covers the real ones. [The verification receipt](verification.md) lists what has been checked; [the design](design.md) holds the full behavior contract.

### Practical limits

The first scan advances in bounded UID ranges from the start of each watched folder, so a large or sparse UID history may take many passes. `coverage` reports the progress. Reconciliation of vanished messages also runs in bounded windows per pass. `reclassify --since` uses the local first-observed date, not the message date.
