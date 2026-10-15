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
| Change the classifier, model or key source | [Classifier and models](./provider.md) |
| Preview or enable filing | [File mail into folders](./filing.md) |
| Change automatic updates | [Updates](./updates.md#modes) |

Keep an account's `identity` tied to the same mailbox. To connect a different mailbox, create a new account name. See [account binding](./reference.md#account-binding).

## Review mode and classification costs

### Category and urgency selection

New configurations use **best guess** for both category and urgency. It accepts the model's chosen label when its confidence is at least **0.25** and its probability leads every other label by at least **5 percentage points**. A tie, a smaller lead, a missing answer or confidence below the floor leaves that field empty. These are starting values to test on your mail, not measured accuracy guarantees.

Confidence and label probability are separate model outputs. For example, Updates at 60% probability with confidence 0.52 and a runner-up at 18% passes best guess. Promotions at 49% versus Other at 40%, with confidence 0.39, also passes. Other at 33% versus Updates at 31% is withheld.

**Strict** mode accepts a category or urgency only at `policy.choice_confidence_min`, which defaults to 0.75. Existing configurations without `choice_mode` keep strict behavior. To enable best guess in an existing file, set these fields inside its `policy` object and set the top-level `schema_version` to `5`:

```json
"choice_mode": "best_guess",
"best_guess_confidence_min": 0.25,
"best_guess_margin_min": 0.05
```

Use `"choice_mode": "strict"` to select strict mode. An optional `accounts.NAME.choice_mode` overrides the global mode for that account; the thresholds remain shared. The mode applies to both category and urgency. Action-required decisions still use `action_yes_min` and `action_no_max`.

JSON results identify accepted best guesses in `classification.best_guess_fields`, for example `["category", "urgency"]`. `read` keeps the original scores under `item.model_decision.raw.answers`. Manual corrections take precedence, and a corrected field is removed from the effective best-guess list. Changing the effective mode queues open mail for classification again; done mail stays done.

Keep filing off while evaluating the results. Best guess does not change your filing settings, and review mode does not prevent live filing. See the [field reference](../reference/configuration.md#policy-selection) for the settings and reasons.

### Review and freshness

`policy.review_mode` starts as `true`. This puts every open message in the attention list so you can check the results. Turn it off only after reviewing classification quality on your own mail. It does not prevent live filing.

Open messages are classified again after `policy.freshness_hours`, which defaults to 24. With OpenRouter, each classification is a request. Changes to your brief, time zone, category rules, provider or policy can also queue open mail for classification again. Changing only `api_key_command` or local Kev's `provider.managed.idle_seconds` does not.

## A complete configuration

Use the [complete example and field reference](../reference/configuration.md#a-complete-configuration) when writing a file manually. It includes every field, valid value and default. Do not put passwords or API keys in the file; use the credential tools described in [manual setup](./manual-setup.md) and [Classifier and models](./provider.md).

After editing, run:

```sh
mailtriage doctor --account work
```

Read the readiness fields as well as the exit code. `doctor` can exit successfully while reporting a problem.
