# Configuration reference

For a walkthrough, see [configuration](../guide/configuration.md). Use this page to look up flags, output fields and detailed behavior.

mailtriage keeps the settings for all your accounts in one file, `mailtriage.json`. This page shows where mailtriage looks for it and what each field does, so you can read it or edit it by hand.

## Where mailtriage finds the config

Every command except `init` and `setup` uses the first of:

1. `--config PATH`;
2. the `MAILTRIAGE_CONFIG` environment variable;
3. `./mailtriage.json`, if that file exists in the working directory;
4. `~/.config/mailtriage/mailtriage.json`.

`setup` uses the same order without step 3: it never picks `./mailtriage.json`. `init` writes `--config PATH` if given, else `./mailtriage.json`; it ignores `MAILTRIAGE_CONFIG`.

Step 4 needs `HOME`; without it the error asks for `--config`. A missing config exits 2 with ``configuration not found; run `mailtriage setup` or pass --config``.

Commands therefore need no `--config` when you use the home config. Pass `--config` in scripts that must not depend on the working directory or the environment.

## A complete configuration

Here is a configuration for one account. The tables below explain each part.

```json
{
  "schema_version": 3,
  "updates": "auto",
  "state_dir": "state",
  "provider": {
    "kind": "openrouter",
    "model": "typesafe/jev-latest",
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

Each command that opens the configuration checks the whole file. A file that breaks a rule below is refused with exit code 2 and `invalid configuration; check required fields, categories and provider settings`.

The message does not name the field, except for an invalid `updates`, which exits 2 with `updates must be auto, notify or off`.

Relative paths in `state_dir`, `engine.config` and `engine.binary` resolve against the directory that holds `mailtriage.json`.

Top level:

| Field | What to set |
| --- | --- |
| `schema_version` | `3`, or `4` while an account has `filing.reply_queue` on, as written by `init`, `setup` and every command that edits the file. mailtriage reads schemas 1 to 4. Releases before automatic updates read only 1 and 2, and releases before the reply queue only 1 to 3, so they refuse a newer file instead of rewriting it without `updates` or `reply_queue`. |
| `updates` | What `watch` does about new releases: `"auto"` installs them, `"notify"` only reports them, `"off"` makes no network call. A config without the key means `"auto"`. See [Updates](./updates.md). |
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

Categories (`accounts.NAME.categories[]`). You can also edit them later with [`categories apply`](../guide/categories.md).

| Field | What to set |
| --- | --- |
| `id` | Stable identifier of letters, digits, `_` and `-`, unique in the account. Corrections and folders refer to it; keep it when you rename a category. |
| `name` | Display name. Sent to the provider. Also the default folder name for filing. |
| `description` | What belongs in the category. Sent to the provider with the name; this text decides the choice. |
| `examples` | Optional list of strings. Stored, but not sent to the provider. |
| `catch_all` | `true` for exactly one category. The provider sees only names and descriptions, so give that category a description that covers mail outside the others. |
| `folder` | Optional IMAP folder for filing; defaults to `name`. `"INBOX"` keeps the category's mail in its source folder. See the [folder rules](./filing.md#folders). |

Engine (`accounts.NAME.engine`):

| Field | What to set |
| --- | --- |
| `kind` | `"himalaya"`. |
| `binary` | Absolute path to the Himalaya executable, such as `/opt/homebrew/bin/himalaya` (`command -v himalaya` prints it). A bare name is looked up on the `PATH` of the mailtriage process, which is short under launchd and systemd. |
| `config` | Path to the Himalaya configuration file: absolute, or relative to `mailtriage.json`. `~` is not expanded. |
| `account` | The account name in that file (`[accounts.work]` means `"work"`). |
| `mailboxes` | The source folders to watch, usually `["INBOX"]`, spelled as `himalaya imap list --all` prints them. With filing on, each must be printable ASCII without `\`, `"` or `&` and must not start with `-`. |
| `expected_version` | The Himalaya version setup found, such as `"2.2.1"`. It must not be empty, but it is not compared: any [tested version](../guide/himalaya.md) is accepted whatever it says. The field stays so that older mailtriage versions can read the config. |
| `timeout_seconds` | Time limit for each Himalaya call, 1 to 600. |
| `max_output_bytes` | Output limit for each Himalaya call, 1 to 268435456 (256 MiB). A message larger than this cannot be fetched and is recorded as a failed fetch. |

`doctor` reports an engine whose `timeout_seconds` or `max_output_bytes` is out of range as `transport.ready: false`.

Configurations written before schema 2 have a `himalaya` block instead of `engine`. mailtriage still reads it and writes it back as `engine` the next time it saves the file. An account cannot have both.

Provider (`provider`):

| Field | What to set |
| --- | --- |
| `kind` | `"openrouter"` for real classification, `"fake"` for offline tests. No other value is accepted. See [Provider](./provider.md). |
| `model` | For `openrouter`, any non-empty Decisions model ID, such as `typesafe/jev-latest` (what setup writes) or `typesafe/jev-1.13`. For `fake`, any non-empty text. |
| `endpoint` | For `openrouter`, exactly `https://openrouter.ai/api/alpha/decisions`. An `http://127.0.0.1:PORT/api/alpha/decisions` address is also accepted, for local tests. Ignored for `fake`. |
| `api_key_command` | Optional. A command that prints the API key, as a list of program and arguments, such as `["/usr/bin/security", "find-generic-password", "-s", "mailtriage", "-a", "openrouter", "-w"]`. The first element must be a non-empty program. When set, it is the only key source. See [Key command rules](./provider.md#key-command-rules). Ignored for `fake`. |
| `api_key_env` | The name of the environment variable that holds the API key, such as `OPENROUTER_API_KEY`: uppercase letters `A` to `Z`, digits and `_` only. This is the variable's name, never the key. For `openrouter`, required unless `api_key_command` is set; it may then be empty or missing. Ignored for `fake`. |
| `timeout_seconds` | Time limit for each provider request, 1 to 300. |

This is what goes to the classifier. For each message the provider receives the account's `identity`, `timezone` and `brief`, each category's `name` and `description`, and the message's sender, To and Cc addresses, subject, date and body text up to `policy.max_body_chars`.

mailtriage never falls back to another provider. A failed request marks the message failed, and a later pass retries it.

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

Changing `brief`, `timezone`, a category's `id`, `description`, `examples` or `catch_all`, the `provider` block (except `api_key_command`) or the `policy` block queues every open message for classification again.

Setting, changing or removing `api_key_command` does not.

Filing (`accounts.NAME.filing`), described in [Filing into folders](./filing.md):

| Field | Default | Effect |
| --- | --- | --- |
| `mode` | `"off"` | `"off"`, `"dry_run"` or `"live"`. Change it with `filing enable` and `filing disable`. |
| `flag` | `true` | Add `\Flagged` to mail that needs action or has `high` urgency; mail the reply queue holds only for `high` urgency. |
| `max_actions_per_pass` | `200` | Moves and flags per pass, 1 to 1000. |
| `reply_queue` | `false` | Keep new mail that needs action in its source folder until you answer it or mark it done; see [Reply queue](./filing.md#reply-queue). Written only when `true`. Change it with `filing enable --reply-queue on` or `off`. |
