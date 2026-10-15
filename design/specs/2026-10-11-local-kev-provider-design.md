# Local Kev provider

Date: 2026-10-11
Status: Draft design, not implemented.
Builds on: [Provider adapter](2026-10-09-provider-adapter-design.md),
[Guided setup](2026-10-05-guided-setup-design.md)

## Goal

Classify mail without OpenRouter. A new provider, `kev`, talks to a
[Kev](https://github.com/jaredpalmer/kev) server: an open-weight decision model
whose API matches TypeSafe's System One, the API behind OpenRouter's Decisions
endpoint. Kev answers the same typed questions with a choice, a confidence and
a probability per label, so the contract check and the policy apply unchanged.

The server runs in one of two ways:

- **Managed** (Apple Silicon Macs only): setup installs Kev and downloads
  Kev-4B; mailtriage starts the server when mail needs classifying and stops it
  after a quiet period.
- **Your own server** (any platform): setup asks for the server's URL. The docs
  explain how to run Kev yourself, for example on a Linux machine with a GPU.

The model weights never enter this repository.

## Decisions

| Topic | Decision |
| --- | --- |
| Kind | One new kind, `kev`. `KINDS` becomes `["openrouter", "kev", "fake"]`. |
| Managed or not | A config with a `managed` block is managed; one without is your own server. |
| Release | Managed installs pin the Kev code to the git tag `kev-1.0` and the checkpoint to `jaredpalmer/kev-4b@v1.0`. |
| Model | `model: "kev-latest"`, the name Kev serves its checkpoint under. |
| Key | Needed only for a server off this machine. `key_account()` takes the config to say so. |
| URL rule | `http` only for `127.0.0.1`, `localhost` and `[::1]`; any other host needs `https` and a key. A managed endpoint must be on this machine. |
| Lifecycle | On demand: a supervisor process starts Kev and stops it after the configured idle timeout (5 minutes by default). No second launchd agent. |
| Platforms | Managed: macOS on arm64. Your own server: everywhere mailtriage runs. |
| Install location | `<data dir>/mailtriage/kev/`, next to the private Himalaya. The weights go to `kev/hf` (`HF_HOME`), so one directory holds everything. |
| Compatibility | OpenRouter and `fake` configs, requests, generation hashes and outputs are unchanged. |

## Config

```jsonc
// managed
"provider": {
  "kind": "kev",
  "model": "kev-latest",
  "endpoint": "http://127.0.0.1:8009/v1/systemone",
  "managed": { "checkpoint": "jaredpalmer/kev-4b@v1.0" },
  "timeout_seconds": 60
}

// your own server, off this machine
"provider": {
  "kind": "kev",
  "model": "kev-latest",
  "endpoint": "https://classify.example.com/v1/systemone",
  "api_key_command": ["security", "find-generic-password", "..."],
  "timeout_seconds": 60
}
```

- `ProviderConfig` gains `managed: Option<ManagedKev>` with one field,
  `checkpoint`. It is serialized only when set, so the schema version and every
  existing generation hash stay the same.
- `managed.checkpoint` is part of the generation hash: switching to
  `jaredpalmer/kev-9b@v1.0` by hand reclassifies open mail, as changing `model`
  does. Setup offers only Kev-4B.
- Validation for `kev`:
  - `endpoint` parses as a URL whose path ends in `/v1/systemone`;
  - `http` only for a loopback host; `https` for any host;
  - with `managed`: a loopback `http` endpoint with an explicit port, and a
    non-empty `checkpoint`;
  - `managed` on any other kind is an error.
- `api_key_env`/`api_key_command` follow today's shared rules whenever a key is
  needed.

## Interface change

```rust
pub trait DecisionProvider: Sync {
    /// `None` when this config needs no key.
    fn key_account(&self, config: &ProviderConfig) -> Option<&'static str>;
    // the rest unchanged
}
```

- OpenRouter: `Some("openrouter")` always.
- Fake: `None`.
- Kev: `Some("kev")` for a non-loopback endpoint, else `None`.

Setup asks the kind and, for `kev`, the URL before it decides whether to run
the key steps. The callers in `doctor`, `classification_skipped`, the service
install note, setup and `secrets` pass the config.

## The adapter

The request body and the answer parsing move from `openrouter.rs` into a shared
module, `provider/systemone.rs`, used by both providers:

- `request_body(config, request)`: today's `{model, state, questions}` body;
- `send(config, url, body, key, name)`: today's client (timeout, no redirects,
  1 MiB limit), with the Bearer header only when a key is given and the
  provider's name in the error texts;
- `parse(response, name)`: today's parsing into a `Decision`.

OpenRouter's request bytes and error texts stay pinned by the existing tests.
Kev's texts read "Kev server returned HTTP 4xx", "Kev server request failed"
and so on.

`Kev::decide`:

1. With `managed`: `kev::runtime::ensure_running(config)` (below). Its errors
   are `ProviderUnavailable`.
2. Touch the last-used marker (managed only).
3. Send and parse as above. Kev rounds probabilities to four decimals, so the
   existing contract check holds.

## The managed runtime (`src/kev/`)

### Layout

```
<data dir>/mailtriage/kev/
  src/            git checkout of kev at tag kev-1.0, with its .venv
  hf/             HF_HOME: the downloaded weights
  installed.json  {"tag": "kev-1.0", "checkpoint": "...", "installed_at": "..."}
  server.lock     held by the running supervisor
  last-used       mtime = the last request
```

`installed.json` is written last by setup, after the probe succeeds. It is the
proof that the code and the weights are in place.

### `mailtriage kev run` (hidden)

The supervisor. It:

1. takes `server.lock` (an exclusive `flock`); if another supervisor holds it,
   exits 0;
2. starts `uv run --frozen --extra serve python -m kev.serve --run <checkpoint>
   --host 127.0.0.1 --port <port>` in `kev/src`, with `HF_HOME=kev/hf` and
   `HF_HUB_OFFLINE=1`, output appended to `<logs>/kev.log`;
3. every 30 seconds, stops Kev when `last-used` is older than the configured idle timeout (5 minutes by default): TERM,
   then KILL after 10 seconds;
4. exits when Kev exits, for any reason, logging the exit status.

`--allow-download` drops `HF_HUB_OFFLINE`; only setup passes it.

### `ensure_running(config)`

1. `installed.json` missing or naming another checkpoint: `ProviderUnavailable`
   "local Kev is not installed; run mailtriage setup --update".
2. `GET http://127.0.0.1:<port>/v1/models` answers and lists `kev-latest` with
   `run` equal to the checkpoint: ready.
3. It answers with something else: `ProviderUnavailable` "port <port> is used by
   another program".
4. Nothing answers: spawn `mailtriage kev run` detached (`setsid`, stdin null),
   then poll `/v1/models` every second for up to 120 seconds. Loading takes 10
   to 30 seconds. Timeout: `ProviderUnavailable` "local Kev did not start; see
   <logs>/kev.log".

Concurrent callers (the service and a manual `sync`) may both spawn; the lock
leaves one supervisor, and both poll until it answers.

## Setup

The classifier menu:

1. OpenRouter (a Decisions model, Jev by default; needs an API key)
2. Local Kev on this Mac (downloads about 9 GB, no API key) — only on macOS
   arm64
3. Kev server you run (enter its URL)
4. Offline demo (`fake`)

Flags: `--provider kev` alone means managed; `--provider kev --kev-url URL`
means your own server. `--kev-url` with another kind is an error. Elsewhere
than macOS arm64, `--provider kev` without `--kev-url` exits 2 with "local Kev
needs an Apple Silicon Mac; run your own Kev server and pass --kev-url".

### Managed install

1. Checks: at least 15 GB free on the data volume, else stop with the number;
   less than 32 GB of memory gives a warning, not a stop.
2. Confirm: "Local Kev downloads about 9 GB to <dir> and uses about 12 GB of
   memory while it classifies. Continue?" `--yes` answers it.
3. `git` must be on PATH (else: "install the Xcode command line tools:
   xcode-select --install"). `uv` must be on PATH; if it is missing and `brew`
   is present, setup offers `brew install uv`; otherwise it prints the install
   command and stops.
4. Clone `--depth 1 --branch kev-1.0` into `kev/src.partial`, rename to
   `kev/src`. An existing `kev/src` at the same tag is reused.
5. `uv sync --frozen --extra serve` (uv fetches the Python version Kev pins).
6. `mailtriage kev run --allow-download` in the background; setup shows its log
   lines while the weights download and waits up to 30 minutes for
   `/v1/models`.
7. Probe: one request with a fixed sample message and the account's questions
   must pass `check_decision`.
8. Stop the server, write `installed.json`, write the config.

A failure at any step leaves the config unchanged and says which step failed;
running setup again resumes, since finished steps are reused.

### Your own server

Setup asks for the URL, checks the URL rule, sends `GET /v1/models` (with the
key when one is needed) and requires `kev-latest` in the list. Remote servers
then go through today's key steps with the key store account `kev`.

## Behaviour elsewhere

| Place | After |
| --- | --- |
| `doctor` | For `kev`: endpoint, mode, and for managed `installed` (with tag and checkpoint) and `running`; for your own server, whether `/v1/models` answers. Each problem names its fix. |
| `sync`, `watch` | `ProviderUnavailable` skips classification like a missing key: mail stays queued, no attempt is counted, the result says `classification: {skipped: true, reason}`. HTTP errors from Kev count an attempt, as with OpenRouter. |
| `service install` | Unchanged; the service needs no extra agent. |
| `self uninstall` | Keeps `<data dir>/mailtriage/kev` and says how to delete it, as it does for the private Himalaya. |

## Errors

| Case | Result |
| --- | --- |
| `http` endpoint off this machine | exit 2, "provider.endpoint must use https for a server off this machine" |
| Remote endpoint without a key source | exit 2, today's key-source error |
| Not installed / other checkpoint | classification skipped, `doctor` says `mailtriage setup --update` |
| Port taken by another program | classification skipped, the reason names the port |
| Server did not start in 120 s | classification skipped, the reason points to `kev.log` |
| Your own server unreachable | classification skipped |
| HTTP error, contract failure | the classification fails and counts an attempt |

## Docs

User docs follow the voice guide.

- New guide page "Run the classifier on your Mac": what Kev is, the
  requirements, what is downloaded and where, speed and memory, how it starts
  and stops, removing it. An Important callout states the limits: Apple Silicon
  only, Kev-4B is less accurate than Jev, the first message after a pause waits
  for the model to load.
- "API keys and models" becomes "Classifier and models", covering OpenRouter,
  local Kev and your own server.
- New guide section "Run your own Kev server": Kev's own setup commands,
  `KEV_API_KEY`, HTTPS in front of it, and the URL to give setup.
- Reference: the `kev` kind, `managed`, the URL rule, the doctor fields, the
  setup flags.
- Development: `providers.md` lists `kev` and the shared System One module.

## Testing

- **OpenRouter unchanged.** The pinned request bytes, error texts and
  generation hashes pass unchanged.
- **Contract suite.** `kev` joins it through the loopback HTTP fixture, with and
  without a key.
- **Validation.** Each URL rule, `managed` on other kinds, missing key source.
- **Runtime.** A stand-in `uv` on PATH starts a test Kev server: first start,
  reuse, wrong program on the port, two concurrent callers, idle stop, start
  timeout, Kev exiting.
- **Setup.** Stand-in `git`, `uv` and `brew` on PATH: the full managed install,
  resume after a failed step, missing `uv` with and without `brew`, the
  non-Apple-Silicon refusal, your own server with a loopback and a remote URL.
- **Skips.** `sync` with an uninstalled managed config skips and counts no
  attempt.
- **Live check (manual).** On an Apple Silicon Mac: a real install, then `sync`
  on the example messages.

## Out of scope

- A managed install on Linux or Intel Macs.
- Choosing Kev-0.8B or Kev-9B in setup (the config can be edited by hand).
- Fine-tuning, images, Laya.
- Reclassifying when your own server changes its checkpoint: the URL and model
  name stay the same, so mailtriage cannot tell.
