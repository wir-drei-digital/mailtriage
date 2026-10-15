# Providers

## Adding a provider

The classifier sits behind the `DecisionProvider` trait in `src/provider/mod.rs`. A provider fits when it answers the three typed questions (`category`, `urgency`, `action_required`) with a choice, a confidence and a probability per offered label.

General chat-model APIs do not fit: they give no calibrated probabilities, and the policy thresholds need them.

1. Implement `DecisionProvider` in a new module under `src/provider/`: `validate` (its own config rules; shared timeout and model checks have passed), `key_account(config)` (the key store account, or `None` when it needs no key, as for `fake` or a Kev server on this machine), `label`, `new_config`, optional `prepare` and `decide`. `prepare` checks whether the provider can answer before a job is taken; `Err(ProviderUnavailable)` skips classification like a missing key. `decide` returns a `Decision` for a `DecisionRequest`, with the whole response in `raw`. A System One service can reuse `provider/systemone.rs`: `request_body`, `send` (Bearer only with a key, with the provider name in errors) and `parse`.
2. Add its kind to `KINDS` (today `openrouter`, `kev`, `fake`) and `provider_for` in `src/provider/mod.rs`. This registers the kind for `--provider` and the provider contract. Setup also needs explicit integration: add the appropriate `Choice` and menu entry in `src/setup.rs`, then handle its literal kind in `classifier_step` and dispatch the choice to its setup step. Make the setup key and check steps match the provider's configuration.
3. Give it a fixture in `tests/provider_contract.rs`. The contract suite runs against every kind in `KINDS`, and setup tests should cover both flag and menu selection.

Every `Decision` passes one contract check before the policy thresholds apply, whichever provider made it: the model is named; each choice is an offered label; confidence and probabilities lie between 0 and 1; the probabilities cover exactly the offered labels and total 1 within 0.031; no label is more than 0.01 more probable than the chosen one; and `action_required` lies between 0 and 1.

A failed check fails the classification like a failed request: the message is marked failed, and a later pass retries it.

## Local Kev

`src/kev/` runs Kev for a `kev` provider with `managed`: `Layout` and `Installed` (`installed.json`), `runtime` (`ensure_running`, `probe`, `stop`, `status` for `doctor`), `supervisor` (the hidden `mailtriage kev run`) and `install` (setup's managed install). `ensure_running` failures are `ProviderUnavailable`.

Startup and installation share an installation guard. The hidden `mailtriage kev mutate` helper owns it while git, uv or brew changes the shared directory. Normal startup rereads the installed receipt after it gets the guard. `doctor` checks installation activity and ownership proof without starting Kev. Unconfirmed ownership blocks replacement until the operator verifies no live process remains and preserves the private directory as a backup.

Managed Kev holds a shared activity lock through startup and each classification response. Idle shutdown takes the exclusive activity lock, confirms group termination and ownership, releases the supervisor lock, and then releases activity so a waiting caller can restart it. The optional `provider.managed.idle_seconds` setting defaults to 300, applies on the next start, and is excluded from the classification-generation hash.

The Kev fixtures need no Python, uv, external network or model. A stand-in `uv` (`tests/kev_support`) logs its calls and, for `uv run`, executes the test binary's ignored `kev_standin_server` test. Control files can make it exit or answer late. Stand-in `git` and `brew` cover setup. `MAILTRIAGE_TEST_KEV` (debug builds only) describes `apple_silicon`, `free_gb`, `memory_gb`, `port`, `start_seconds` and `download_seconds`. Existing non-Kev setup fixtures still invoke Python.

## Contract evidence

The contract review on 2026-09-23 used public upstream documentation and Himalaya v2.1.0 source, without private credentials or a live mailbox. Later live Kev checks are recorded separately in [Verification](./verification.md#local-kev-2026-10-15).

### OpenRouter Decisions

OpenRouter's [Jev tutorial](https://openrouter.ai/blog/tutorials/jev-vs-llm-when-to-use-each/) identifies `POST https://openrouter.ai/api/alpha/decisions` and the Jev model ID `typesafe/jev-1.13`. Its example uses a state object and named `choice`/`noul` questions, and shows typed `answers`, `model`, `provider`, and `usage` in the response.

The [Decisions API reference](https://openrouter.ai/docs/api/api-reference/alphadecisions/submit-a-decisions-request) specifies a direct JSON body `{model, state, questions}` with `criteria` and `instructions` per question. The SDK's `decisionsRequest` argument is a client wrapper, not an HTTP body field. The [TypeSafe model page](https://openrouter.ai/typesafe/jev-1.13/api) confirms the Jev model family.

The adapter uses only this endpoint, three fixed typed questions, and Bearer authorization with the key from `api_key_command` or a named environment variable. It accepts any Decisions model ID; a new setup writes `typesafe/jev-latest`, an alias OpenRouter moves to the newest Jev model.

A loopback HTTP fixture verifies the actual method, path, body, and response decoder. A synthetic authorized live request remains a release gate.

### Kev

Verified on 2026-10-11 against Kev's source at tag `kev-1.0` (`kev/serve.py`, `kev/api.py`) and its [README](https://github.com/jaredpalmer/kev/blob/kev-1.0/README.md). `POST /v1/systemone` takes `{state, model, questions}` with `choice` and `noul` questions. It answers with typed `answers` in the Decisions shape, echoes `model` and rounds probabilities to four decimals. `GET /v1/models` lists `{name, run, ...}` per served name (`kev-latest`, `jev-latest`), with `run` matching `--run`. `KEV_API_KEY` makes the server require `Authorization: Bearer <key>` on `/v1/*`. `python -m kev.serve --run --host --port` starts it; weights go to `HF_HOME`.

Loopback HTTP fixtures in `tests/provider_contract.rs` check the pinned OpenRouter request body with `"model":"kev-latest"` at `/v1/systemone`, the decoder, and Bearer transmission at the shared transport boundary. A Kev loopback endpoint receives no key even if one is supplied. A real install, example classifications, a 20-message mailbox pass, and offline inference were completed on Apple Silicon on 2026-10-15; see the [dated live-check record](./verification.md#local-kev-2026-10-15) for the measured timings, category retests, and limitations. Automatic shutdown with the configurable 5-minute default and a fresh restart also passed; the fresh classification took 11.15 seconds. Memory was not measured.

### Himalaya v2.1.0 IMAP

Pinned [release](https://github.com/pimalaya/himalaya/releases/tag/v2.1.0) and source:

- [`imap/mailbox/status.rs`](https://github.com/pimalaya/himalaya/blob/v2.1.0/src/imap/mailbox/status.rs): `imap status MAILBOX` emits JSON fields `uid_validity` and `uid_next` directly from IMAP STATUS; missing or zero values are errors.
- [`imap/fetch.rs`](https://github.com/pimalaya/himalaya/blob/v2.1.0/src/imap/fetch.rs): `imap fetch --mailbox MAILBOX --envelope START:END` uses UIDs by default and returns `messages[]` with `uid` and envelope data. Its UID range avoids date-sorted pagination; each call is bounded to 100 UID positions.
- [`imap/envelope/search.rs`](https://github.com/pimalaya/himalaya/blob/v2.1.0/src/imap/envelope/search.rs): `imap search` returns UIDs by default but does not expose a UID-set search criterion, so it is not the incremental range operation.
- [`shared/message/read.rs`](https://github.com/pimalaya/himalaya/blob/v2.1.0/src/shared/message/read.rs): `message read --raw` writes original bytes without global JSON; `--seen` is opt-in and is omitted.
- [`imap/mailbox/arg.rs`](https://github.com/pimalaya/himalaya/blob/v2.1.0/src/imap/mailbox/arg.rs): status accepts a positional mailbox; fetch accepts `--mailbox`.

The published `himalaya.aarch64-darwin.tgz` asset was downloaded and verified against the [release API's SHA-256 digest](https://api.github.com/repos/pimalaya/himalaya/releases/tags/v2.1.0): `a5a787b7c4dbf065408e7772908fc75c799626f4cebab8e9c78fafe3e2fa585c`. Its actual `--version` output begins `himalaya v2.1.0 +jmap ... +imap ...`, followed by build and Git lines.

The adapter checks the `v2.1.0` token and `+imap` feature in the first line; a CLI `doctor` check using this downloaded binary returned `transport.ready: true` with no mailbox operation.

The release binary's `imap status --help`, `imap fetch --help`, and `message read --help` confirm the argument forms above, including UID mode by default and opt-in `--seen`. Its bundled JSON Schemas confirm `uid_next` and `uid_validity` at the STATUS top level, plus `messages[].uid` and `messages[].envelope` in FETCH output.

The adapter checks UIDVALIDITY before and after discovery and raw fetch. The service must bind stored locators to the account identity, mailbox, and UIDVALIDITY, and must persist discovered jobs before advancing a checkpoint.

The [Dovecot end-to-end workflow](./verification.md#dovecot-end-to-end) exercises Seen preservation and UID transitions with a real Himalaya binary and test server. A provider-specific [live filing check](./mail-provider-check.md) is still required before relying on those behaviors on a real mailbox provider.
