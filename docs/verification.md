# Build and verification receipt

2026-09-23. Implemented with three GPT-6 Sol agents and coordinator integration,
following the [implementation plan](implementation-plan.md). Independent review
reports: [core](review-core.md) and [integration](review-cli-integration.md).

Completed: Rust library/CLI; all three decisions; editable taxonomy; RFC822/JSON
normalization; OpenRouter Decisions and explicit fake providers; Himalaya 2.1.0
adapter; SQLite jobs, retry and lease recovery; source binding and UID epochs;
bounded discovery/reconciliation; corrections, Done/reopen, query cursors and
export; Hermes guide and cross-platform CI/release workflows.

| Executed check | Result |
| --- | --- |
| `cargo fmt --check` | Passed |
| `cargo clippy --all-targets --locked --offline -- -D warnings` | Passed |
| `cargo test --locked --offline` | 30 tests passed |
| `cargo build --release --locked --offline` | macOS arm64 executable built |
| Release smoke | init, classify, list, correct, done, reopen, read, export |
| Supervisor shutdown | release watch exited cleanly on SIGTERM |
| Official Himalaya release | checksum, version, help and JSON schemas verified |

Tests comprise 7 adapter, 2 CLI, 7 core, 13 storage and 1 full sync test. HTTP
fixtures bind localhost and make no external model calls. The sync test runs an
executable Himalaya fixture through classification, idempotency, removal and
UID epoch reset. CI workflows are configured; no remote workflow was run here.

Local executable: `dist/mailtriage` (ignored build artifact). SHA-256:

```text
90197575357171c6333009646ce8ea2389512183c2c2f94231046fd89a5d9b9e
```

No private mailbox was read, no API key was used, and no remote repository or
release was published. No Valea application code was changed.

## Live gates

1. In a test config, set `provider` to `kind` `openrouter`, model
   `typesafe/jev-1.13` and endpoint `https://openrouter.ai/api/alpha/decisions`.
   Provide the key with an `api_key_command` or an exported `api_key_env`
   variable ([The OpenRouter key](guide.md#the-openrouter-key)); mailtriage
   does not read a `.env` file. Check that `doctor --json` reports
   `provider.key_present: true`, then `classify` a synthetic message such as
   `examples/reply-request.eml` and check that `outcome` is `classified`.
2. Evaluate urgency/action recall and category quality on approved labeled mail
   before disabling review mode. Current thresholds are configurable starting
   values, not measured accuracy claims.
3. Verify Seen preservation and UID transitions with a test IMAP account and
   the actual Himalaya/server pair; validate the cloud Hermes runtime.

Review mode stays on by default. Fake-provider results prove plumbing, not Jev
accuracy. Source principals hidden behind opaque OAuth token helpers remain the
configured identity owner's responsibility. Independent installations do not
synchronize state. Operational restore uses the SQLite backup; JSON export is
an inspection/interchange artifact.

## Dovecot end to end

`.github/workflows/e2e.yml` runs `tests/e2e_dovecot.rs` on every push to
`main`, on pull requests and on demand. It uses the real `mailtriage` binary,
the official Himalaya v2.1.0 Linux x86_64 release (`himalaya.x86_64-linux.tgz`,
SHA-256 `683a2ab8e1534f01e6bda3a69e204d564c31fbfbe20511fc7bc60b67f2e85884`) and
a `dovecot/dovecot:2.3.21` container in two namespace layouts: no prefix with
separator `/` (`tests/e2e/dovecot-flat.conf`) and prefix `INBOX.` with
separator `.` (`tests/e2e/dovecot-prefix.conf`). The test enables `live` filing
and checks: filing into category folders with read state preserved and
`\Flagged` added, a client move as a category correction, a client move back
to `INBOX` as a pin, a client delete as done, and an `INBOX` UIDVALIDITY reset
that keeps the pinned message known and open. To run it locally, you need
Docker with a running daemon, Python 3, Cargo and a Himalaya v2.1.0 binary.
Pass the binary in `MT_E2E_HIMALAYA`, the layout (`flat` or `prefix`) and a
free local port:

```sh
MT_E2E_HIMALAYA="$(command -v himalaya)" bash tests/e2e/run.sh flat 31143
MT_E2E_HIMALAYA="$(command -v himalaya)" bash tests/e2e/run.sh prefix 31144
```

`cargo test` ignores this test. When it runs with `--ignored` but without the
`MT_E2E_*` variables, it prints a notice and skips.

## Live provider check (required before `live` on a real mailbox)

The Dovecot end-to-end job covers the protocol contract, not provider
behaviour. Before setting `filing.mode` to `live` on a real mailbox, run this
check for that provider with a throwaway test account configured in Himalaya
([Set up Himalaya](guide.md#1-set-up-himalaya); procedure in the
[filing design](superpowers/specs/2026-10-04-imap-category-filing-design.md#live-provider-check)).
No credentials, message bodies or real addresses go into this file: record
redacted output, or where the evidence is kept. `Result` is `pass`, `fail` or
`differs` (with a note). Until a provider has a recorded go, use `dry_run`,
which never writes.

### Gmail / Google Workspace

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
| Gmail: label semantics of MOVE from INBOX, archive, and a message carrying two category labels | | | |
| Login rate: one `watch` with seven watched folders at a 60-second interval for 30 minutes without throttling; if throttled, whether pipelined STATUS via `imap raw` resolves it | | | |

### Microsoft 365 / Outlook.com

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
| Login rate: one `watch` with seven watched folders at a 60-second interval for 30 minutes without throttling; if throttled, whether pipelined STATUS via `imap raw` resolves it | | | |

### iCloud

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
| Login rate: one `watch` with seven watched folders at a 60-second interval for 30 minutes without throttling; if throttled, whether pipelined STATUS via `imap raw` resolves it | | | |

### Fastmail or a Dovecot host

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
| Login rate: one `watch` with seven watched folders at a 60-second interval for 30 minutes without throttling; if throttled, whether pipelined STATUS via `imap raw` resolves it | | | |

### Outcome

| Provider | Outcome (go / go with noted differences / no-go) | Notes | Date |
| --- | --- | --- | --- |
| Gmail / Google Workspace | not checked | | |
| Microsoft 365 / Outlook.com | not checked | | |
| iCloud | not checked | | |
| Fastmail or a Dovecot host | not checked | | |

A no-go blocks enabling `live` for that provider until it is resolved and is
listed in the guide's [Provider check](guide.md#provider-check) section.

## Guided setup on a real machine (human check)

The automated tests of `mailtriage setup` and `mailtriage service` use fake
`security`, `secret-tool`, `pass`, `launchctl` and `systemctl` scripts. Before
you rely on the background service and the tool-backed key stores, a person
runs these checks once per platform on a real machine. Use a throwaway
Himalaya account and a test OpenRouter key. Record results as in the provider
check: `pass`, `fail` or `differs` (with a note), and no keys, addresses or
message content.

### macOS (Keychain and launchd)

1. Move any existing `~/.config/mailtriage` aside, then run
   `mailtriage setup --account work` in Terminal. Choose the macOS Keychain and enter the key
   when `security` asks for it. Answer yes to the background service.
2. Run `mailtriage doctor --account work --json` and
   `mailtriage service status --account work --json`.
3. Send a message to the test account and wait for one interval.
4. Run `mailtriage service uninstall --account work`.

| Check | Result | Evidence | Date |
| --- | --- | --- | --- |
| `security` prompts for the key; setup prints `The key is in macOS Keychain.` and never shows the key | | | |
| `api_key_command` in `mailtriage.json` is `["/usr/bin/security", "find-generic-password", "-s", "mailtriage", "-a", "openrouter", "-w"]`; the file has mode 0600 | | | |
| Keychain Access shows the item `mailtriage` (account `openrouter`) in the login keychain | | | |
| Whether a Keychain access dialog appears on the first key read (setup, `doctor`, or the first pass under launchd), and that none appears after "Always Allow" | | | |
| Running `mailtriage setup --update --account work --key-store keychain` again offers to reuse the stored key | | | |
| `~/Library/LaunchAgents/digital.wirdrei.mailtriage.work.plist` exists; `launchctl print gui/$(id -u)/digital.wirdrei.mailtriage.work` shows `state = running` | | | |
| `service status`: `loaded`, `running` true, `pid` set; `log_paths` point to `~/.config/mailtriage/logs/work.log` and `.err` | | | |
| After the test message, `last_pass.exit_code` is 0, `last_pass.finished_at` is recent, and `list` shows the message classified (the key command works under launchd) | | | |
| `work.log` holds one JSON line per pass; `work.err` holds no key and no key error | | | |
| After logout and login (or a reboot), the agent runs again | | | |
| `service install --account work` a second time keeps the same file and leaves the job running | | | |
| `service uninstall`: `launchctl print` no longer finds the job, the plist is gone, `service status` shows `installed`, `loaded` and `running` false | | | |

### Linux (Secret Service, pass and systemd)

1. In a desktop session with a Secret Service provider (for example GNOME
   Keyring), move any existing `~/.config/mailtriage` aside and run
   `mailtriage setup --account work`. Choose Secret Service and enter the key when
   `secret-tool` asks for it. Answer yes to the background service.
2. Run `mailtriage doctor --account work --json` and
   `mailtriage service status --account work --json`.
3. Send a message to the test account and wait for one interval.
4. Run `loginctl enable-linger $USER`, log out, send another message, wait,
   log in again, and run `service status` again.
5. Repeat steps 1 to 3 with `mailtriage setup --update --account work
   --key-store pass` on a machine with `pass` set up.
6. Run `mailtriage service uninstall --account work` and
   `loginctl disable-linger $USER` if you enabled it only for this check.

| Check | Result | Evidence | Date |
| --- | --- | --- | --- |
| `secret-tool` prompts for the key; `api_key_command` is `secret-tool lookup service mailtriage provider openrouter` with the tool's absolute path | | | |
| The keyring shows the item labelled `mailtriage OpenRouter key` (for example in Seahorse) | | | |
| `~/.config/systemd/user/mailtriage-work.service` exists and starts with `# managed by mailtriage`; `systemctl --user status mailtriage-work.service` shows it active | | | |
| `service status`: `manager` `systemd`, `loaded` and `running` true, `pid` set, `log_paths` empty | | | |
| `journalctl --user -u mailtriage-work.service` shows one JSON line per pass | | | |
| After the test message, `last_pass.exit_code` is 0 and `list` shows the message classified (Secret Service is reachable from the user unit) | | | |
| With linger on and the user logged out, the unit keeps running; record whether the key command still works with the keyring locked (`last_pass.exit_code`, `doctor`'s `key_error` after login) | | | |
| `pass` store: `pass insert mailtriage/openrouter` prompts; under the user unit `pass show` reaches the GPG agent (test message classified) | | | |
| `service install --account work` a second time restarts the unit with the same file | | | |
| `service uninstall`: the unit file is gone, `systemctl --user status mailtriage-work.service` reports it not found, `service status` shows `installed`, `loaded` and `running` false | | | |
