# Configuration

mailtriage keeps your settings in one JSON file, usually `~/.config/mailtriage/mailtriage.json`. [Setup](./setup.md) creates it for you. You only need to edit it by hand for settings the setup questions do not cover.

## Where mailtriage finds the config

Most commands use the first available option in this order:

1. `--config PATH`.
2. The `MAILTRIAGE_CONFIG` environment variable.
3. `mailtriage.json` in the current directory.
4. `~/.config/mailtriage/mailtriage.json`.

`setup` skips the current-directory file. `init` writes to `--config PATH` or the current directory and ignores the environment variable.

Use an explicit `--config` in scripts so a change of working directory cannot select another file.

## Understand the main settings

| Setting | Purpose |
| --- | --- |
| `accounts` | Your mailboxes. The name of each entry is the value used with `--account`. |
| `provider` | The AI service, model and how to retrieve its key. Shared by all accounts. |
| `policy` | When to ask you to review a decision, message size limits and retries. |
| `updates` | Automatic updates: `auto`, `notify` or `off`. |
| `state_dir` | The local database and stored message text. |

Each account has your email address (`identity`), `timezone`, a brief about you, categories, Himalaya settings (`engine`) and optional filing settings.

Relative paths are resolved from the directory containing `mailtriage.json`. Use absolute paths for Himalaya's executable and configuration when running in the background. `~` is not expanded in the engine's config path.

## Common changes

| You want to | Use |
| --- | --- |
| Change folders, your brief or time zone | `mailtriage setup --update --account work` |
| Add, rename or describe categories | The [tray categories window](./tray.md#the-categories-window) or [categories commands](./categories.md) |
| Change the model or key source | [API keys and models](./provider.md) |
| Preview or enable filing | [File mail into folders](./filing.md) |
| Change automatic updates | [Updates](./updates.md#modes) |

Keep an account's `identity` tied to the same mailbox. To connect a different mailbox, create a new account name. See [account binding](./reference.md#account-binding).

## Review mode and classification costs

`policy.review_mode` starts as `true`. This puts every open message in the attention list so you can check the results. Turn it off only after reviewing classification quality on your own mail. It does not prevent live filing.

Open messages are classified again after `policy.freshness_hours`, which defaults to 24. With OpenRouter, each classification is a request. Changes to your brief, time zone, category rules, provider or policy can also queue open mail for classification again. Changing only `api_key_command` does not.

## A complete configuration

Use the [complete example and field reference](../reference/configuration.md#a-complete-configuration) when writing a file manually. It includes every field, valid value and default. Do not put passwords or API keys in the file; use the credential tools described in [manual setup](./manual-setup.md) and [API keys](./provider.md).

After editing, run:

```sh
mailtriage doctor --account work
```

Read the readiness fields as well as the exit code. `doctor` can exit successfully while reporting a problem.
