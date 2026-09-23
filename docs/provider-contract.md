# Adapter contract evidence

Verified against public upstream documentation and Himalaya v2.1.0 source on 2026-09-23. No private credentials or live mailbox were used.

## OpenRouter Decisions

OpenRouter's [Jev tutorial](https://openrouter.ai/blog/tutorials/jev-vs-llm-when-to-use-each/) identifies `POST https://openrouter.ai/api/alpha/decisions` and the Jev model ID `typesafe/jev-1.13`. Its example uses a state object and named `choice`/`noul` questions, and shows typed `answers`, `model`, `provider`, and `usage` in the response. The [Decisions API reference](https://openrouter.ai/docs/api/api-reference/alphadecisions/submit-a-decisions-request) specifies a direct JSON body `{model, state, questions}` with `criteria` and `instructions` per question. The SDK's `decisionsRequest` argument is a client wrapper, not an HTTP body field. The [TypeSafe model page](https://openrouter.ai/typesafe/jev-1.13/api) confirms the Jev model family.

The adapter uses only this endpoint, three fixed typed questions, and Bearer authorization from a named environment variable. A loopback HTTP fixture verifies the actual method, path, body, and response decoder. A synthetic authorized live request remains a release gate.

## Himalaya v2.1.0 IMAP

Pinned [release](https://github.com/pimalaya/himalaya/releases/tag/v2.1.0) and source:

- [`imap/mailbox/status.rs`](https://github.com/pimalaya/himalaya/blob/v2.1.0/src/imap/mailbox/status.rs): `imap status MAILBOX` emits JSON fields `uid_validity` and `uid_next` directly from IMAP STATUS; missing or zero values are errors.
- [`imap/fetch.rs`](https://github.com/pimalaya/himalaya/blob/v2.1.0/src/imap/fetch.rs): `imap fetch --mailbox MAILBOX --envelope START:END` uses UIDs by default and returns `messages[]` with `uid` and envelope data. Its UID range avoids date-sorted pagination; each call is bounded to 100 UID positions.
- [`imap/envelope/search.rs`](https://github.com/pimalaya/himalaya/blob/v2.1.0/src/imap/envelope/search.rs): `imap search` returns UIDs by default but does not expose a UID-set search criterion, so it is not the incremental range operation.
- [`shared/message/read.rs`](https://github.com/pimalaya/himalaya/blob/v2.1.0/src/shared/message/read.rs): `message read --raw` writes original bytes without global JSON; `--seen` is opt-in and is omitted.
- [`imap/mailbox/arg.rs`](https://github.com/pimalaya/himalaya/blob/v2.1.0/src/imap/mailbox/arg.rs): status accepts a positional mailbox; fetch accepts `--mailbox`.

The published `himalaya.aarch64-darwin.tgz` asset was downloaded and verified against the [release API's SHA-256 digest](https://api.github.com/repos/pimalaya/himalaya/releases/tags/v2.1.0): `a5a787b7c4dbf065408e7772908fc75c799626f4cebab8e9c78fafe3e2fa585c`. Its actual `--version` output begins `himalaya v2.1.0 +jmap ... +imap ...`, followed by build and Git lines. The adapter checks the `v2.1.0` token and `+imap` feature in the first line; a CLI `doctor` check using this downloaded binary returned `transport.ready: true` with no mailbox operation. The release binary's `imap status --help`, `imap fetch --help`, and `message read --help` confirm the argument forms above, including UID mode by default and opt-in `--seen`. Its bundled JSON Schemas confirm `uid_next` and `uid_validity` at the STATUS top level, plus `messages[].uid` and `messages[].envelope` in FETCH output.

The adapter checks UIDVALIDITY before and after discovery and raw fetch. The service must bind stored locators to the account identity, mailbox, and UIDVALIDITY, and must persist discovered jobs before advancing a checkpoint. A test IMAP server is still required to verify Seen preservation and epoch transitions against a real binary/server pair.
