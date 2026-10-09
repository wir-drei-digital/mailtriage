# Provider

mailtriage asks a provider to make its decisions. For your real mail that is OpenRouter; for trying mailtriage out, there is an offline stand-in.

The `provider` block names the service that answers the three questions for every message. There are two kinds:

| `kind` | What it is | Key |
| --- | --- | --- |
| `openrouter` | OpenRouter's Decisions API. `model` is any Decisions model ID; setup writes `typesafe/jev-latest`. | Needed; see [The OpenRouter key](#the-openrouter-key). |
| `fake` | Fixed keyword rules for offline tests ([Try it offline](./introduction.md#try-it-offline)). It makes no network request. | None |

`typesafe/jev-latest` is an alias that OpenRouter moves to the newest Jev model. Your config does not change when it moves, so no mail is queued for classification again.

Mail classified after the move gets the newer model, including open mail whose classification is older than `freshness_hours`. Each classification records the model the response named, as `classification.model` in `list` and `read`.

To stay on one model, name it, such as `typesafe/jev-1.13`; changing `model` queues open mail for classification again.

Only Decisions-style services fit: they answer each question with a choice, a confidence and a probability per label. See [Adding a provider](../development/providers.md#adding-a-provider).

## The OpenRouter key

mailtriage needs your OpenRouter key only to classify. It gets the key in one of two ways:

- If `provider.api_key_command` is set, it runs that command. This is the only source then; `api_key_env` is ignored even if the variable is set.
- Otherwise it reads the environment variable named in `provider.api_key_env` from its own process environment.

mailtriage does not read a `.env` file or any other key file. Never put the key in `mailtriage.json`. `mailtriage setup` sets up either source; see [Key stores](./setup.md#key-stores).

The commands that classify need the key: `sync`, `watch`, `classify` and `reclassify`. They resolve it once, and only when a message is due for classification, before they take it. A pass with nothing to classify never runs the key command and needs no key.

Without the key they still run but classify nothing: no message is taken, so no retry attempt is used and the mail stays queued. The result gains `"classification": {"skipped": true, "reason": "..."}`, where `reason` is one of the fixed key errors below, and is partial (exit 4). `sync` still scans the folders and, with filing on, runs the filing steps.

Once the key works, the next pass classifies the queued mail; changing `api_key_command` queues nothing again. `doctor` reports whether the key is present (`provider.key_present`) and why not (`provider.key_error`).

`list`, `read`, `correct`, `done`, `reopen`, `export`, `categories` and `filing` commands do not use the key.

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

`api_key_env` holds the name of the variable, for example `OPENROUTER_API_KEY`. mailtriage reads the key from its own process environment when it sends a request.

::: warning Important
A supervised `watch`, including the [background service](./service.md), does not see variables from your login shell. Prefer a key command there.
:::

In an interactive shell, run the lines below; at `read`, paste the key and press Enter (nothing is echoed):

```sh
read -rs OPENROUTER_API_KEY
export OPENROUTER_API_KEY
mailtriage doctor --account work --json
```

The variable lasts until the shell exits. To fill it from the macOS Keychain in every new shell, add this line to `~/.zshrc` (a key command does the same without a variable):

```sh
export OPENROUTER_API_KEY="$(security find-generic-password -s mailtriage -a openrouter -w)"
```

Agents: the process that runs mailtriage must have the variable in its environment, because mailtriage inherits it from its parent; see the [Agent guide](../agents/index.md).
