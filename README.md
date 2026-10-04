# mailtriage

`mailtriage` is a command-line tool that classifies email. For each message it records three decisions: a category from your own list, an urgency (`low`, `medium` or `high`), and whether you need to act. It reads mail over IMAP through the [Himalaya](https://github.com/pimalaya/himalaya) CLI, gets the decisions from OpenRouter's Decisions API with a Jev model, and stores messages and results in a local SQLite database. Listing and reading work from that database without network access.

mailtriage never sends, deletes or expunges mail and never changes the read state (`\Seen`). By default it does not write to the mailbox at all. An account that enables [filing into folders](#filing-into-folders) also creates category folders, moves mail into them and adds `\Flagged`. `done` and `reopen` change only the local review state. Every command works on one configured account, named with `--account`.

## Try it offline

The built-in `fake` provider classifies with fixed keyword rules and makes no network request. Use it in a scratch directory and delete the directory afterwards: the first command against an account records that account's identity in the state directory, so the example account cannot be reused for real mail.

```sh
mkdir mailtriage-demo && cd mailtriage-demo
mailtriage init --json
mailtriage classify --account work --input /path/to/mailtriage/examples/reply-request.eml --format rfc822 --json
mailtriage list --account work --view attention --json
```

`init` writes `mailtriage.json` with the `fake` provider, one account named `work` and state in `.state/`. It refuses to overwrite an existing file (exit code 2). `classify` reads one message, classifies it and stores the result without a mailbox location. It accepts an RFC 822 file (`--format rfc822`), the JSON shape in `examples/message.json` (`--format json`), or stdin (`--input -`). Classifying the same message again returns `outcome: cached`.

## Setup

You need:

- macOS arm64, Linux amd64 or Linux arm64,
- Himalaya v2.1.0 with IMAP support,
- an IMAP account whose server supports UID and UIDVALIDITY (the MOVE extension too, if you want filing),
- an OpenRouter API key.

### 1. Install mailtriage

From a GitHub release: each release carries `mailtriage-vVERSION-linux-amd64.tar.gz`, `-linux-arm64.tar.gz` and `-macos-arm64.tar.gz`, a `.sha256` file per archive and a combined `SHA256SUMS`. Each archive holds the `mailtriage` executable, this README and the license. While the repository is private, download with authenticated access, for example with the GitHub CLI:

```sh
VERSION=0.1.0
gh release download "v$VERSION" --repo wir-drei-digital/mailtriage --pattern "mailtriage-v$VERSION-macos-arm64.tar.gz*"
shasum -a 256 --check "mailtriage-v$VERSION-macos-arm64.tar.gz.sha256"   # Linux: sha256sum --check
tar -xzf "mailtriage-v$VERSION-macos-arm64.tar.gz"
sudo install -m 0755 mailtriage /usr/local/bin/mailtriage
```

The macOS executable is unsigned and not notarized. See the [release guide](docs/releases.md) for how releases are made.

From source, with a stable Rust toolchain:

```sh
cargo build --release --locked
sudo install -m 0755 target/release/mailtriage /usr/local/bin/mailtriage
mailtriage --version
```

The examples below assume `mailtriage` is on your `PATH`. The launchd and systemd examples use the absolute path `/usr/local/bin/mailtriage`.

### 2. Set up Himalaya

mailtriage runs the `himalaya` executable for every mailbox operation. It accepts only Himalaya v2.1.0 with IMAP support: the first line of `himalaya --version` must start with `himalaya v2.1.0` and contain `+imap`. Install it from the [v2.1.0 release](https://github.com/pimalaya/himalaya/releases/tag/v2.1.0) or a package manager, then check:

```sh
himalaya --version
```

Write a Himalaya configuration file with one account. mailtriage uses only the account's `imap` settings; it never sends mail, so no `smtp` settings are needed. A minimal file, for example `~/.config/himalaya/config.toml`:

```toml
[accounts.work]
imap.server = "imaps://imap.example.org:993"
imap.sasl.plain.username = "alice@example.org"
imap.sasl.plain.password.command = "security find-generic-password -s imap.example.org -a alice@example.org -w"
```

- `[accounts.work]`: the account name. `engine.account` in `mailtriage.json` refers to it.
- `imap.server`: `imaps://HOST:993` for TLS. A bare `HOST` also means `imaps://`. For STARTTLS on port 143, use `imap://HOST:143` with `imap.starttls = true`.
- Credentials use Himalaya's own mechanism. `password.command` runs a command that prints the password. The example reads it from the macOS keychain; store it there once with `security add-generic-password -s imap.example.org -a alice@example.org -w`, which prompts for the password. On Linux, a command such as `pass show mail/work` works the same way. `password.raw` stores the password in the file; avoid it. For OAuth servers, Himalaya offers `imap.sasl.oauthbearer` and `imap.sasl.xoauth2`; see [Himalaya's sample configuration](https://github.com/pimalaya/himalaya/blob/v2.1.0/config.sample.toml).
- mailtriage starts Himalaya with no terminal input and passes on its own environment. The password command must not prompt, and the commands it calls must be found on that environment's `PATH`.
- Do not add a `mailbox.alias` entry to this account that maps a watched folder or category folder name to a different mailbox. With filing on, mailtriage stops scanning such a folder and makes no filing writes until the alias is removed.

Check the login and the folder names. These commands contact the server:

```sh
himalaya --config ~/.config/himalaya/config.toml --account work imap list --all
himalaya --config ~/.config/himalaya/config.toml --account work --json imap status INBOX
```

The first command lists every folder; use these exact names in `engine.mailboxes`. The second must print `uid_validity` and `uid_next`, which mailtriage depends on. mailtriage discards Himalaya's error output, so use these commands to see the cause of an IMAP error later.

### 3. Create and edit the configuration

```sh
mkdir -p ~/mailtriage && cd ~/mailtriage
mailtriage init --config mailtriage.json --json
```

Edit `mailtriage.json` before you run any other command against the account. The first command that opens an account stores its binding: `identity`, the Himalaya account name, `imap.server` and the other non-secret IMAP settings. A later change to any of them is refused with exit code 5 (see [Account binding](#account-binding)).

A complete configuration for one account:

```json
{
  "schema_version": 2,
  "state_dir": ".state",
  "provider": {
    "kind": "openrouter",
    "model": "typesafe/jev-1.13",
    "endpoint": "https://openrouter.ai/api/alpha/decisions",
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
| `schema_version` | `2`, as written by `init`. |
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
| `api_key_env` | For `openrouter`, the name of the environment variable that holds the API key, such as `OPENROUTER_API_KEY`: uppercase letters `A` to `Z`, digits and `_` only. This is the variable's name, never the key. Ignored for `fake`. |
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

Changing `brief`, `timezone`, a category's `id`, `description`, `examples` or `catch_all`, the `provider` block or the `policy` block queues every open message for classification again.

Filing (`accounts.NAME.filing`), described in [Filing into folders](#filing-into-folders):

| Field | Default | Effect |
| --- | --- | --- |
| `mode` | `"off"` | `"off"`, `"dry_run"` or `"live"`. Change it with `filing enable` and `filing disable`. |
| `flag` | `true` | Add `\Flagged` to mail that needs action or has `high` urgency. |
| `max_actions_per_pass` | `200` | Moves and flags per pass, 1 to 1000. |

### 4. Provide the OpenRouter API key

`mailtriage.json` holds only the name of the variable (`provider.api_key_env`, for example `OPENROUTER_API_KEY`). mailtriage reads the key from its own process environment when it sends a request. mailtriage does not read a `.env` file or any other key file. Never put the key in `mailtriage.json`.

The commands that classify need the key: `sync`, `watch`, `classify` and `reclassify`. Without it they still run, but each message fails with `classification provider failed; check doctor and retry` and the command exits 4. `doctor` reports whether the variable is set (`provider.key_present`). `list`, `read`, `correct`, `done`, `reopen`, `export`, `categories` and `filing` commands do not use the key.

In an interactive shell:

```sh
read -rs OPENROUTER_API_KEY    # paste the key and press Enter; nothing is echoed
export OPENROUTER_API_KEY
mailtriage doctor --config ~/mailtriage/mailtriage.json --account work --json
```

The variable lasts until the shell exits. To load an existing `.env` file of `NAME=value` lines into the shell, run `set -a; . ./.env; set +a`. On macOS you can keep the key in the keychain (`security add-generic-password -s openrouter -a mailtriage -w` prompts for it) and add this line to `~/.zshrc`:

```sh
export OPENROUTER_API_KEY="$(security find-generic-password -s openrouter -a mailtriage -w)"
```

A supervised `watch` does not see variables from your login shell. Set the key in the supervisor's configuration.

launchd on macOS, in `~/Library/LaunchAgents/local.mailtriage.work.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>local.mailtriage.work</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/local/bin/mailtriage</string>
    <string>watch</string>
    <string>--config</string>
    <string>/Users/alice/mailtriage/mailtriage.json</string>
    <string>--account</string>
    <string>work</string>
    <string>--json</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>OPENROUTER_API_KEY</key>
    <string>sk-or-REPLACE-WITH-YOUR-KEY</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>StandardOutPath</key>
  <string>/Users/alice/mailtriage/watch.log</string>
  <key>StandardErrorPath</key>
  <string>/Users/alice/mailtriage/watch.err</string>
</dict>
</plist>
```

```sh
chmod 600 ~/Library/LaunchAgents/local.mailtriage.work.plist
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/local.mailtriage.work.plist
launchctl bootout gui/$(id -u)/local.mailtriage.work      # stops it
```

The plist holds the key in plain text, so keep it readable only by you. `KeepAlive` with `SuccessfulExit` set to `false` restarts `watch` after it exits with an error. launchd starts jobs with the `PATH` `/usr/bin:/bin:/usr/sbin:/sbin`, which is why `engine.binary` must be an absolute path.

systemd on Linux, in `/etc/systemd/system/mailtriage-work.service`:

```ini
[Unit]
Description=mailtriage watch for account work
Wants=network-online.target
After=network-online.target

[Service]
User=mailtriage
EnvironmentFile=/etc/mailtriage/openrouter.env
ExecStart=/usr/local/bin/mailtriage watch --config /etc/mailtriage/mailtriage.json --account work --json
Restart=on-failure
RestartSec=30

[Install]
WantedBy=multi-user.target
```

```sh
sudo touch /etc/mailtriage/openrouter.env
sudo chmod 600 /etc/mailtriage/openrouter.env
sudoedit /etc/mailtriage/openrouter.env      # one line: OPENROUTER_API_KEY=sk-or-...
sudo systemctl daemon-reload
sudo systemctl enable --now mailtriage-work.service
```

The environment file holds `NAME=value` lines without `export`. systemd reads it as root before it switches to the user named in `User=`, so the file can stay owned by root with mode 0600. Avoid `Environment=OPENROUTER_API_KEY=...` in the unit file: unit files are world-readable. The user named in `User=` needs write access to `state_dir` and to the directory that holds `mailtriage.json`, where mailtriage creates `mailtriage.lock` and rewrites the file for `filing enable`, `filing disable` and `categories apply`.

Hermes and other agents: the process that runs mailtriage must have the variable in its environment, because mailtriage inherits it from its parent. If an agent starts mailtriage commands, set the variable in the agent's own service environment with one of the methods above. Run `doctor` from the agent to check it. See the [Hermes guide](docs/hermes.md).

### 5. Check the setup

```sh
mailtriage doctor --config ~/mailtriage/mailtriage.json --account work --json
```

Run it in the same environment as the command you are checking. `doctor` exits 0 whether or not the account is ready, so read the fields:

| Field | Value when ready | Meaning |
| --- | --- | --- |
| `ready` | `true` | The provider configuration is valid, the key is present, and the Himalaya check passed. |
| `provider.kind`, `provider.model` | your values | The configured provider. |
| `provider.key_present` | `true` | The variable named in `api_key_env` is set and not blank in this process. Always `true` for `fake`. |
| `transport.configured` | `true` | The account has an `engine`. Without one, `transport.ready` is `true` as well. |
| `transport.ready` | `true` | The Himalaya configuration file was read and `himalaya --version` reported v2.1.0 with `+imap`. Otherwise `transport.error` is set. |
| `transport.version` | `himalaya v2.1.0 ...` | The first line of `himalaya --version`. |
| `live_checks_performed` | `false` | Always `false`. |

`doctor` makes no provider request. It logs in to the IMAP server only when filing is on, to add a `filing` block with the server's capabilities, folders and problems. The first `sync` is therefore the first full test of the IMAP login and the API key.

### 6. First run

1. Run `doctor` (step 5) until `ready` and `transport.configured` are `true`.
2. Run one small pass:

   ```sh
   mailtriage sync --config ~/mailtriage/mailtriage.json --account work --limit 20 --json
   ```

   Exit code 0 with `scan_errors: 0` and `failed: 0` means the login, the fetch and the provider request worked. Exit code 4 means the pass was partial. `scan_errors` counts folders that could not be scanned (see `coverage.scans[].error`), and `failed` counts messages whose fetch or classification failed (`list --view all` shows each message's `error`). Run the Himalaya check commands from step 2 to see an IMAP error.
3. Look at the result:

   ```sh
   mailtriage list --config ~/mailtriage/mailtriage.json --account work --view attention --json
   ```

4. Start `watch` under launchd or systemd (step 4), or in a terminal:

   ```sh
   mailtriage watch --config ~/mailtriage/mailtriage.json --account work --limit 100 --interval-seconds 60 --json
   ```

5. To move mail into category folders, follow the [filing rollout](#rollout).

The first passes work through the existing mail in each watched folder, oldest UID first. Each pass scans at most `--limit` UIDs per folder and classifies at most `--limit` messages. Every classification is one OpenRouter request.

## Daily use

`--config` defaults to `mailtriage.json` in the current directory. Pass it explicitly in scripts and supervised jobs; the examples below omit it.

### Sync and watch

```sh
mailtriage sync --account work --limit 100 --json
mailtriage watch --account work --limit 100 --interval-seconds 60 --json
```

`sync` runs one bounded pass: it scans the watched folders, fetches new messages, classifies queued ones and, with filing on, files them. Its result reports `discovered`, `fetched`, `classified`, `cached`, `failed`, `pending` and `scan_errors`, plus `coverage` and, with filing on, `filing`. `--limit` is 1 to 500 (default 100).

`watch` repeats the pass every `--interval-seconds` (1 to 86400, default 60) until Ctrl-C or SIGTERM. With `--json` it prints one JSON line per pass and a final stop object with `passes`, `partial_passes` and `skipped_passes`. A partial pass does not stop `watch`. A pass that `mailtriage.json` or the Himalaya configuration changed under is skipped: `watch` prints its error object (code 5), counts it in `skipped_passes` and runs the next pass with the current configuration. Any other error, including a changed account binding or a second worker, ends `watch` with that error's exit code. After a graceful stop `watch` exits 0, or 4 if any pass was partial or skipped. Run it under a supervisor that restarts it.

Only one worker runs per account at a time. `sync`, `classify`, `reclassify` and each `watch` pass take a lock in `state_dir`; a second one exits 5 with `an account worker is already running`. Run either `watch` or scheduled `sync` for an account, never both.

A failed fetch or classification is retried on later passes (see `policy.max_attempts`). A failed message whose source is still present stays in the attention view. A message whose source occurrence has disappeared stays in the `all` view, and its local content is kept. A complete scan needs a stable mailbox UIDVALIDITY. Mail that enters and leaves a watched folder between two passes is never seen.

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

A category file holds a JSON array of categories or an object with a `categories` array; `categories export` writes the object form. The [category fields](#3-create-and-edit-the-configuration) are the same as in `mailtriage.json`: a unique `id`, a non-empty `name` and `description`, optional `examples` and `folder`, and exactly one category with `catch_all: true`.

`categories validate --file FILE` checks the file alone. With `--account NAME` it checks the file against that account's configuration, including the folder rules when the account's filing is on. Neither form writes anything.

`categories apply` writes the categories into `mailtriage.json`. A change to a category's `id`, `description`, `examples` or `catch_all`, or an added or removed category, advances `taxonomy_revision` and queues all open messages for classification. A rename (a change to `name` only) keeps the ID and does not reclassify. Done messages stay done; a reopened message is classified under the current categories. Removing a category that a manual correction uses fails with `category has manual corrections; remap or clear them before removal` (exit code 2); correct those messages to another category or clear the correction first. Keep IDs when you edit category files by hand.

`reclassify` queues the matching stored messages for classification and processes up to `--limit` of them (1 to 500, default 100); later passes process the rest. `--since YYYY-MM-DD` selects messages first observed on or after that local date. `--dry-run` reports `matched` and `will_process` and changes nothing.

## Filing into folders

Filing makes the classification visible in every mail client. With filing on, each classified message in a source folder (the engine's `mailboxes`, usually `INBOX`) is moved into a top-level folder for its category, and mail that needs action or has `high` urgency gets `\Flagged`. Filing is off by default and is enabled per account. It needs an `engine`; `filing enable` refuses an account without one.

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
| `off` | The default. The mailbox is left exactly as before. |
| `dry_run` | Each pass watches the category folders and plans moves and flags, and writes nothing: no folder is created, no message is moved or flagged. `sync` reports the planned counts under `filing.planned`, and `filing plan` lists the actions. |
| `live` | mailtriage creates and subscribes the category folders, moves mail and adds flags. |

`filing enable --mode dry-run` or `--mode live` and `filing disable` write `filing.mode` into `mailtriage.json` under the configuration lock (`dry-run` on the command line is `dry_run` in the file). The next pass uses the new mode.

### Rollout

1. Run `filing enable --mode dry-run`.
2. Keep `watch` or scheduled `sync` running for a few days. Check `filing plan` and the `filing.planned` counts of each pass. `filing plan`'s `total` counts the actions of the next pass only, at most `filing.max_actions_per_pass`; it is not the size of the backlog.
3. Check that your provider has a recorded go (see [Provider check](#provider-check)).
4. Run `filing enable --mode live`.
5. Optionally file older mail with `filing backfill`.

Only new mail is filed automatically: mail whose server internal date is at or after the time filing was enabled. Mail delivered during the dry-run days counts as new, so the first live passes file it. Mail that arrives while filing is off counts as older mail when filing is enabled again.

To file older mail, preview it with `filing backfill --days N` or `filing backfill --all`, then repeat the command with `--apply`. `--days` (1 to 3650) compares against the server's internal date. `--apply` requires `live` and makes each listed message eligible once. The moves happen over the following passes, at most `filing.max_actions_per_pass` (default 200) per pass.

### Folders

`filing enable` writes an explicit `folder` (the category's current `name`) into every category that has none, so a later rename does not move its folder. `categories apply` with filing on keeps each existing category's folder the same way.

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

Before you use `live` on a real mailbox, the live provider check in the [filing design](docs/superpowers/specs/2026-10-04-imap-category-filing-design.md#live-provider-check) must have recorded a go for your provider (Gmail / Google Workspace, Microsoft 365 / Outlook.com, iCloud, Fastmail / Dovecot) in [the verification receipt](docs/verification.md#live-provider-check-required-before-live-on-a-real-mailbox). No provider has been checked yet, so use `dry_run` until yours is. A no-go will be listed here.

### Filing exit codes

| Code | Filing cases |
| --- | --- |
| 0 | Success. |
| 2 | Invalid input or configuration: no mail engine, invalid folders, `backfill --apply` outside `live`, an unknown folder or message, an arrival that is not unresolved. |
| 3 | Operational failure. |
| 4 | Partial sync. Any filing error in a pass makes it partial. |
| 5 | Conflict: the configuration changed during the command or pass, a placement changed concurrently, a message's identity is not yet established, a `duplicate_copy` retry while both copies are still recorded, or a changed account binding. |

## Reference

### Output and exit codes

With `--json`, every result is one line of JSON on stdout, and so is every error: `{"schema_version":1,"error":{"code":N,"message":"..."}}`. Without `--json`, results are pretty-printed JSON and errors go to stderr as `mailtriage: MESSAGE`. Every result has a `schema_version`. Error messages omit message bodies and credentials.

| Code | Meaning |
| --- | --- |
| 0 | Success. A query exits 0 even if messages need attention, and `doctor` exits 0 even if `ready` is `false`. |
| 2 | Invalid input or configuration: an unknown flag value, account, category or message, or an invalid `mailtriage.json`. |
| 3 | Operational failure, reported as `Operation failed; check configuration and dependency availability`, for example when Himalaya cannot run. |
| 4 | Partial result: a pass, `classify` or `reclassify` with failed messages or scan errors; a pass with filing errors; `watch` at stop after a partial or skipped pass. |
| 5 | Conflict: the configuration changed during the command, another worker is running, the account binding changed, a cursor expired, or a placement changed concurrently. |

### Account binding

The first command that opens an account stores a binding in the state directory: the account's `identity`, the Himalaya account name, `imap.server` and the other IMAP settings except secrets. Settings whose key contains `password`, `passwd`, `token` or `secret` are left out, so you can rotate a password or token. After a change to anything else in the binding, commands for that account exit 5 with `account binding changed or state unavailable; verify config and use a new namespace for a different mailbox`. To use a different mailbox, add a new account name in `mailtriage.json`. An opaque OAuth token helper can change its underlying account without changing the visible configuration, so keep `identity` accurate.

### State and backups

The SQLite database and normalized message text under `state_dir` are authoritative private data. Raw messages and attachments are not stored. Stop the worker before you copy the directory, including any SQLite WAL files. Keep a JSON export as well. Losing the database loses your corrections and Done state; operational state is restored from the database copy, not from the export. The state directory is for one local owner. Do not share it over a network filesystem. Independent installations do not synchronize state.

### Verification boundary

The offline fixture tests, the adapter contract tests and the Dovecot end-to-end test do not prove a real provider's `\Seen` behavior, the live OpenRouter gateway, or classification quality on representative mail. Those need a test mailbox, an API key and labeled examples. Check thresholds against labeled mail before you turn off `review_mode`. The fake provider only exercises the plumbing, and no accuracy claim is made until the live Jev evaluation is complete. [The verification receipt](docs/verification.md) lists what has been checked; [the design](docs/design.md) holds the full behavior contract.

### Practical limits

The first scan advances in bounded UID ranges from the start of each watched folder, so a large or sparse UID history may take many passes. `coverage` reports the progress. Reconciliation of vanished messages also runs in bounded windows per pass. `reclassify --since` uses the local first-observed date, not the message date.
