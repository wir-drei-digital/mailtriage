# Provider check

Before you use `live` on a real mailbox, the live provider check in the [filing design](https://github.com/wir-drei-digital/mailtriage/blob/main/design/specs/2026-10-04-imap-category-filing-design.md#live-provider-check) must have recorded a go for your provider (Gmail / Google Workspace, Microsoft 365 / Outlook.com, iCloud, Fastmail / Dovecot) in the [Outcome](#outcome) table below. No provider has been checked yet, so use `dry_run` until yours is. A no-go will be listed here.

The Dovecot end-to-end job covers the protocol contract, not provider
behaviour. Before setting `filing.mode` to `live` on a real mailbox, run this
check for that provider with a throwaway test account configured in Himalaya
([Set up Himalaya](./manual-setup.md#_1-set-up-himalaya); procedure in the
[filing design](https://github.com/wir-drei-digital/mailtriage/blob/main/design/specs/2026-10-04-imap-category-filing-design.md#live-provider-check)).
No credentials, message bodies or real addresses go into this file: record
redacted output, or where the evidence is kept. `Result` is `pass`, `fail` or
`differs` (with a note). Until a provider has a recorded go, use `dry_run`,
which never writes.

## Gmail / Google Workspace

| Check | Result | Evidence | Date |
| --- | --- | --- | --- |
| `imap raw` output for pipelined `CAPABILITY`/`NAMESPACE` | | | |
| `imap raw` output for `SELECT` + `UID MOVE` (UIDVALIDITY, COPYUID) | | | |
| `imap raw` output for `SELECT` + `UID STORE` | | | |
| `imap raw` output for `LIST "" "*" RETURN (SPECIAL-USE)` | | | |
| `imap list --all --json` and `imap list --json` shapes, delimiter, attributes returned, and the representation of a non-ASCII folder name | | | |
| The alias table location in the Himalaya TOML | | | |
| Create and subscribe for an ASCII and a non-ASCII name | | | |
| INTERNALDATE, size and Message-ID before and after a move | | | |
| `\Flagged` in the provider's web and mobile clients | | | |
| `\Seen` unchanged by fetch, move and store | | | |
| Reply queue: `\Answered` set by the web and mobile clients on reply; answered mail moved unread; `\Seen` added by `a2 UID STORE` only after `filing replies --approve` | | | |
| Gmail: label semantics of MOVE from INBOX, archive, and a message carrying two category labels | | | |
| Login rate: one `watch` with seven watched folders at a 60-second interval for 30 minutes without throttling; if throttled, whether pipelined STATUS via `imap raw` resolves it | | | |

## Microsoft 365 / Outlook.com

| Check | Result | Evidence | Date |
| --- | --- | --- | --- |
| `imap raw` output for pipelined `CAPABILITY`/`NAMESPACE` | | | |
| `imap raw` output for `SELECT` + `UID MOVE` (UIDVALIDITY, COPYUID) | | | |
| `imap raw` output for `SELECT` + `UID STORE` | | | |
| `imap raw` output for `LIST "" "*" RETURN (SPECIAL-USE)` | | | |
| `imap list --all --json` and `imap list --json` shapes, delimiter, attributes returned, and the representation of a non-ASCII folder name | | | |
| The alias table location in the Himalaya TOML | | | |
| Create and subscribe for an ASCII and a non-ASCII name | | | |
| INTERNALDATE, size and Message-ID before and after a move | | | |
| `\Flagged` in the provider's web and mobile clients | | | |
| `\Seen` unchanged by fetch, move and store | | | |
| Reply queue: `\Answered` set by the web and mobile clients on reply; answered mail moved unread; `\Seen` added by `a2 UID STORE` only after `filing replies --approve` | | | |
| Login rate: one `watch` with seven watched folders at a 60-second interval for 30 minutes without throttling; if throttled, whether pipelined STATUS via `imap raw` resolves it | | | |

## iCloud

| Check | Result | Evidence | Date |
| --- | --- | --- | --- |
| `imap raw` output for pipelined `CAPABILITY`/`NAMESPACE` | | | |
| `imap raw` output for `SELECT` + `UID MOVE` (UIDVALIDITY, COPYUID) | | | |
| `imap raw` output for `SELECT` + `UID STORE` | | | |
| `imap raw` output for `LIST "" "*" RETURN (SPECIAL-USE)` | | | |
| `imap list --all --json` and `imap list --json` shapes, delimiter, attributes returned, and the representation of a non-ASCII folder name | | | |
| The alias table location in the Himalaya TOML | | | |
| Create and subscribe for an ASCII and a non-ASCII name | | | |
| INTERNALDATE, size and Message-ID before and after a move | | | |
| `\Flagged` in the provider's web and mobile clients | | | |
| `\Seen` unchanged by fetch, move and store | | | |
| Reply queue: `\Answered` set by the web and mobile clients on reply; answered mail moved unread; `\Seen` added by `a2 UID STORE` only after `filing replies --approve` | | | |
| Login rate: one `watch` with seven watched folders at a 60-second interval for 30 minutes without throttling; if throttled, whether pipelined STATUS via `imap raw` resolves it | | | |

## Fastmail or a Dovecot host

| Check | Result | Evidence | Date |
| --- | --- | --- | --- |
| `imap raw` output for pipelined `CAPABILITY`/`NAMESPACE` | | | |
| `imap raw` output for `SELECT` + `UID MOVE` (UIDVALIDITY, COPYUID) | | | |
| `imap raw` output for `SELECT` + `UID STORE` | | | |
| `imap raw` output for `LIST "" "*" RETURN (SPECIAL-USE)` | | | |
| `imap list --all --json` and `imap list --json` shapes, delimiter, attributes returned, and the representation of a non-ASCII folder name | | | |
| The alias table location in the Himalaya TOML | | | |
| Create and subscribe for an ASCII and a non-ASCII name | | | |
| INTERNALDATE, size and Message-ID before and after a move | | | |
| `\Flagged` in the provider's web and mobile clients | | | |
| `\Seen` unchanged by fetch, move and store | | | |
| Reply queue: `\Answered` set by the web and mobile clients on reply; answered mail moved unread; `\Seen` added by `a2 UID STORE` only after `filing replies --approve` | | | |
| Login rate: one `watch` with seven watched folders at a 60-second interval for 30 minutes without throttling; if throttled, whether pipelined STATUS via `imap raw` resolves it | | | |

## Outcome

| Provider | Outcome (go / go with noted differences / no-go) | Notes | Date |
| --- | --- | --- | --- |
| Gmail / Google Workspace | not checked | | |
| Microsoft 365 / Outlook.com | not checked | | |
| iCloud | not checked | | |
| Fastmail or a Dovecot host | not checked | | |

A no-go blocks enabling `live` for that provider until it is resolved and is
listed at the top of this page.
