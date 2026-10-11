# Provider reference

For a walkthrough, see [provider](../guide/provider.md). Use this page to look up flags, output fields and detailed behavior.

mailtriage asks a provider to make its decisions. You can use OpenRouter or Kev for real mail, or the offline stand-in to try it out.

The `provider` block names the service that answers the three questions for every message. There are three kinds:

| `kind` | What it is | Key |
| --- | --- | --- |
| `openrouter` | OpenRouter's Decisions API. `model` is any Decisions model ID; setup writes `typesafe/jev-latest`. | Needed; see [The OpenRouter key](#the-openrouter-key). |
| `kev` | Local Kev, which mailtriage installs and runs on an Apple Silicon Mac, or a server you run. See [Kev](#kev). | Only for a server off this machine |
| `fake` | Fixed keyword rules for offline tests ([Try it offline](../guide/introduction.md#try-it-offline)). It makes no network request. | None |

`typesafe/jev-latest` is an alias that OpenRouter moves to the newest Jev model. Your config does not change when it moves, so no mail is queued for classification again.

Mail classified after the move gets the newer model, including open mail whose classification is older than `freshness_hours`. Each classification records the model the response named, as `classification.model` in `list` and `read`.

To stay on one model, name it, such as `typesafe/jev-1.13`; changing `model` queues open mail for classification again.

::: warning Important
mailtriage does not check a model ID with OpenRouter, and `doctor` makes no provider request, so a mistyped model goes unnoticed until mail is classified. If OpenRouter refuses the model, every classification fails with `classification provider failed; check doctor and retry`, and each message is tried up to `policy.max_attempts` times. Once you correct `model`, all open mail is queued for classification again, failed messages included, and their attempt count starts over.
:::

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

## Kev

A `kev` provider sends the same System One request as `openrouter` and reads the same answer shape. Kev rounds probabilities to four decimals, which pass the same contract checks. `model` is usually `kev-latest`, the name Kev serves its checkpoint under.

For local Kev, setup writes:

```json
"provider": {
  "kind": "kev",
  "model": "kev-latest",
  "endpoint": "http://127.0.0.1:8009/v1/systemone",
  "managed": { "checkpoint": "jaredpalmer/kev-4b@v1.0" },
  "timeout_seconds": 60
}
```

A config with `managed` uses [local Kev](#local-kev). One without it uses a server you run:

```json
"provider": {
  "kind": "kev",
  "model": "kev-latest",
  "endpoint": "https://classify.example.com/v1/systemone",
  "api_key_command": ["/usr/bin/security", "find-generic-password", "-s", "mailtriage", "-a", "kev", "-w"],
  "timeout_seconds": 60
}
```

`managed.checkpoint` helps decide when to classify open mail again, just as `model` does. Changing it by hand to `jaredpalmer/kev-9b@v1.0` queues open mail again; `mailtriage setup --update` then installs that checkpoint. Setup offers only Kev-4B. Other provider kinds reject `managed`.

### The URL rule

- `endpoint` must end in `/v1/systemone`, with no query, fragment, user name or password.
- `http` is allowed only for `127.0.0.1`, `localhost` and `[::1]`. Another host needs `https`, or validation exits 2 with `provider.endpoint must use https for a server off this machine`.
- A server off this machine needs `api_key_command` or `api_key_env`, under the [key rules](#the-openrouter-key), stored by setup under account `kev`. Without a key source, validation exits 2 with `Kev server needs api_key_command or api_key_env`. A loopback server receives no key, even if the config names one.
- A managed endpoint must be exactly `http://127.0.0.1:PORT/v1/systemone`, with an explicit non-default port. Port 80 is unsupported because URL normalization removes it. `managed.checkpoint` cannot be empty.

### Local Kev

Setup installs local Kev into `<data dir>/mailtriage/kev`, usually `~/.local/share/mailtriage/kev`, or under `XDG_DATA_HOME`. The code is pinned to tag `kev-1.0` in `src`, weights are in `hf`, and uv uses `python` and `uv-cache`. Setup writes `installed.json` (`tag`, `checkpoint`, `installed_at`, `uv`) only after sample classification and confirmed shutdown.

When mail needs classifying, mailtriage checks `GET http://127.0.0.1:PORT/v1/models`. If nothing answers, it starts a supervisor and waits up to 120 seconds, checking every second. One supervisor owns the shared installation. It runs `uv run --frozen --extra serve python -m kev.serve --run CHECKPOINT --host 127.0.0.1 --port PORT` in `src` with `HF_HUB_OFFLINE=1` and `UV_OFFLINE=1`; output goes to `kev.log`. Ordinary starts make no download.

Every 30 seconds, the supervisor checks whether 15 minutes have passed since the last request. It then asks the whole owned Kev process group to stop with TERM, followed by KILL after 10 seconds if needed. The supervisor records its final exit status. It also exits when Kev exits on its own. On macOS it runs in an independent session, so it can outlive a `watch` pass. The background service needs no separate Kev job.

Before setup changes the shared checkout or checkpoint, it stops an owned supervisor and confirms shutdown. An installation guard prevents ordinary starts during mutation. Setup rejects an unrelated listener on the port. If ownership termination cannot be confirmed, it leaves durable evidence and refuses to replace files. A free lock alone does not establish whole-group termination; see [Repair an installation](../guide/local-kev.md#repair-an-installation).

While local Kev cannot answer, classification is skipped: no message is taken, no retry attempt is used, and the result gains `"classification": {"skipped": true, "reason": "..."}` (exit 4). A later managed outage after a job is taken returns that job to the queue without using its current attempt.

| `reason` | Cause |
| --- | --- |
| `local Kev is not installed; run mailtriage setup --update` | No matching installation receipt or required installation artifacts. |
| `port PORT is used by another program` | Another program answers on the port, including Kev serving a different checkpoint. |
| `local Kev did not start; see PATH/kev.log` | Nothing answered within 120 seconds. |
| `the Kev server did not answer at URL` | Your own server is unreachable. |

HTTP errors, failed requests after a successful remote reachability check, and invalid answers fail the classification and count an attempt, as with OpenRouter. Error text includes `Kev server returned HTTP 422` or `Kev server request failed` as applicable.

### `doctor` and Kev

For `kev`, `doctor` adds `provider.kev` and sets `live_checks_performed: true`. This means Kev status checks are enabled. A missing local installation can return before an HTTP request. Doctor never starts or stops local Kev.

| Field | Meaning |
| --- | --- |
| `mode`, `endpoint` | `managed` or `server`, and the configured endpoint. |
| `dir`, `installed` | Local Kev's directory and whether a matching receipt and required artifacts are present. |
| `tag`, `checkpoint` | Local Kev's installed release and checkpoint when verified. `installed_checkpoint` appears when the receipt names another checkpoint. |
| `running` | Local Kev: whether its owned server answers now. `false` is fine when `ready` is true; it starts on demand. |
| `reachable` | Your own server: whether `/v1/models` answered. It can be true even when the configured model is absent. |
| `ready` | Local Kev is installed and stopped cleanly or serving the expected checkpoint; your own server serves the configured model. This also contributes to top-level `ready`. |
| `error`, `fix` | When not ready, the cause and a suggested repair. |

Doctor first checks whether an installer or its mutation helper owns the installation. During that work, `ready` is false and the helper is not reported as a serving Kev runtime. It then checks ownership proof and required files without starting Kev. It checks the source tag, `src/.venv`, `hf` directory and recorded uv executable, but cannot establish that every weight file is intact. Uncertain ownership stays not ready and needs the [stopped-caller repair sequence](../guide/local-kev.md#repair-an-installation).
