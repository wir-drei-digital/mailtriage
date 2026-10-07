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

## Tray app on a real desktop (human check)

The tray's automated tests use a fake `mailtriage`, render the window headless and print the menu as text. A person runs these checks once per desktop before relying on the tray. Record `pass`, `fail` or `differs` (with a note) and the date; attach screenshots to the review, not to the repository.

Setup: build both binaries as a release does (`cargo build --release --locked -p mailtriage`, then `-p mailtriage-tray`), install `mailtriage-tray` next to `mailtriage`, and use a throwaway account with the background service installed.

| Check | macOS light | macOS dark | Ubuntu GNOME + AppIndicator | KDE Plasma |
| --- | --- | --- | --- | --- |
| `mailtriage-tray` starts, no Dock icon (macOS), the icon is crisp and its shape matches the state (plain, `!` badge, outline) | | | | |
| The menu shows every account, its state line, "Last check …", the service item and "Edit categories…"; wording matches the spec | | | | |
| Stop service, then log out and in: the service stays stopped; Start service: it runs again | | | | |
| Install service for a not-installed account from the menu | | | | |
| Open log opens the `.log` file (macOS); Copy log command puts `journalctl --user -u mailtriage-NAME.service -e` on the clipboard (Linux) | | | | |
| After at least four hours idle, including sleep and wake, the menu still opens and its times advance | | | | |
| Start at login: enable from the menu, log out and in: one tray starts; Quit stays quit until the next login; `kill -SEGV` restarts it (macOS) | | | | |
| A second `mailtriage-tray` prints `mailtriage-tray is already running` and exits 0 | | | | |
| Restart onto an update: replace `mailtriage-tray` on disk with a newer build while the tray runs; within about 15 s it re-executes itself, keeps its menu, and an open categories window keeps running | | | | |
| Edit categories… opens the window; a second click shows "the categories window is already open" as a notice in the menu, and on macOS also brings the window to the front | | | | |
| The window follows the system's light or dark mode; edit, check, Apply, refile panel; closing with edits asks | | | | |
| The window's close button and ⌘/Ctrl+W ask about unsaved edits (Cancel keeps the window); after closing, the window's private drafts directory (`$TMPDIR/mailtriage-tray-*`) is gone | | | | |
| ⌘Q (macOS app menu) in the categories window with unsaved edits asks "Discard changes?" (Cancel keeps the window) | | | n/a | n/a |

| Screen reader check | VoiceOver (macOS) | Orca (Linux) |
| --- | --- | --- |
| Every control in the window is announced with its label; Tab follows the visual order | | |
| Editing a category name and description | | |
| A validation error is announced in the footer and Apply's disabled reason is reachable | | |
| The apply confirmation reads its title, each change and the OpenRouter warning | | |
| Cancelling a close with unsaved edits | | |

### Design review

The automated review renders every window screen in light and dark mode (`cargo test --locked -p mailtriage-tray --test screens -- --ignored`, files in `target/tray-screens/`) and prints the menus for every state. The table records the outcome per screen against the spec's UI principles.

| Screen | Plain words | Calm layout | Look (light/dark, 8 px, 14/18 pt) | Feedback | Empty and first-run states | Keyboard and accessibility | Date |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 01-loading | pass: "Loading…" | pass: list left, form right, Apply bottom right with Revert beside it | fixed: disabled Apply was a dimmed blue (text 1.6:1 light, 3.7:1 dark) that still looked clickable; it is now grey like every disabled button | pass: spinner and "Loading…" in the form and the footer; selector, Reload, Add, Revert and Apply disabled with a tooltip | n/a | pass: every control labelled | 2026-10-07 |
| 02-loaded | pass: names and "default", no IDs; "Use for mail that fits nowhere else" | pass | pass: list x = 16, form x = 236 (16 px panel margins, 220 px list), gaps 8, fields 400 px, rows 24/72/24 px; 14 pt text, 18 pt "Categories". Fixed: the selected row's text used the plain text colour on the selection fill (2.2:1 dark); it now uses the selection's text colour (5.3:1 dark, 4.5:1 light). Fixed: secondary text (a category's folder) raised from 2.9:1/2.7:1 to 4.0:1/3.5:1 | pass: Apply "No changes to apply", Revert "No changes to revert" | n/a | pass: Tab and screen-reader order follow the screen (window tests) | 2026-10-07 |
| 03-change-summary | pass: "1 renamed" | pass: Apply is the only filled button | pass: white on blue 6.0:1 light, 4.9:1 dark | pass: the change summary in the footer | n/a | pass: ⌘/Ctrl+S opens the confirmation (window test) | 2026-10-07 |
| 04-confirm-apply | pass: "Apply these changes to daniel?", "Rename News to Newsletters" | fixed: "Apply changes" stood left of Cancel; all dialog buttons now sit at the right edge with the confirming one rightmost. One filled button per surface | pass: 18 pt title, 16 px margins | pass: lists each change | n/a | pass: Esc cancels; Cancel comes before "Apply changes" for Tab and screen readers (window test) | 2026-10-07 |
| 05-validation-error | pass: the CLI's `catch-all` message becomes "Every category needs a name, a description and its own ID, and exactly one must be the default category." | pass: the message wraps left of the buttons | fixed: error text was egui's pure red (3.8:1 light, 4.3:1 dark); now 6.3:1 in both | pass: Apply "Fix the problem shown below first"; "Show details" | n/a | pass: labelled; the announcement is a human check (VoiceOver, Orca) | 2026-10-07 |
| 06-details | pass: the only screen with the command, exit code and JSON, behind "Show details" | pass: Copy and Close at the right edge | pass: 13 pt monospace in a 240 px scroll box | pass: Copy | n/a | fixed: the text box had no label; it is now announced as "Details" (window test) | 2026-10-07 |
| 07-new-category-advanced | pass: "Travel", ID suggested from the name, "Saved with the category; not sent to the classifier" | pass: Advanced collapsed until opened | pass: ID and Examples 400 px, indented 16; the folder placeholder (4.2:1 light, 3.8:1 dark) stays lighter than typed text (11:1, 9.6:1). Fixed in the final review: the form's scroll bar showed only while the pointer was over it; every scroll area now has a thin bar that stays visible and takes its own room beside the fields (window test) | pass: "1 added" | pass: the folder shows the name as a placeholder | pass: ⌘/Ctrl+N adds (window test) | 2026-10-07 |
| 08-confirm-reclassify | pass: the OpenRouter warning, word for word from the spec | fixed with 04 | pass: the warning in strong text | pass | n/a | pass as 04 | 2026-10-07 |
| 09-confirm-remove | pass: says the mail stays in Promotions and where new mail goes | fixed with 04: Cancel, then Remove at the right | pass: the question as body text (two lines) | pass: nothing is written until Apply | n/a | pass: Esc cancels | 2026-10-07 |
| 10-categories-changed | pass: "These categories were changed somewhere else."; "Reload (discard my edits)" says what happens to edits | fixed with 04: "Keep editing", then the reload at the right | pass | pass | n/a | pass: Esc keeps editing | 2026-10-07 |
| 11-busy | pass: "mailtriage is busy." | pass | pass | pass: three retries ("trying again…") before "Try again" | n/a | pass: "Try again" labelled; the Tab-order test does not include it (known gap) | 2026-10-07 |
| 12-refile | pass: plain refile sentences; "Move" is announced as "Move mail from Promotions" | fixed: the preview overflowed the fixed 232 px panel ("Not moved" clipped); "Move all 38" now follows its sentence as "Move" does, and the whole preview fits (window test). Fixed in the final review: the panel, which also opens by itself after an apply, could not be closed and left the form about 190 px with its lower fields below the fold; its heading row now has "Close" (window and model tests) | pass: the form above keeps about 190 px and scrolls, with a scroll bar that stays visible | pass: the move buttons and "Close" say why while a move runs | n/a | pass: every button labelled; "Close" is announced as "Close Move filed mail" and comes first in the panel for Tab (window test) | 2026-10-07 |
| 13-refile-moved | pass: "Marked 30 messages; 12 more will move if their new category calls for it." | fixed: the result line makes the content taller than the fixed panel; it scrolls, now with a thin scroll bar that stays visible and takes its own room (before: a floating bar over the text, and "Not moved" below the fold with no sign of it) | pass | pass: success says what happens next | n/a | pass | 2026-10-07 |
| 14-refile-dry-run | pass: "Moving filed mail needs filing set to live." | pass: preview only, no move buttons; fits the panel | pass | pass | n/a | pass | 2026-10-07 |
| 15-empty-filing-off | pass: no folder field and no "Move filed mail…" with filing off | pass | pass | pass: Remove "Choose another default category first" | fixed: "Add categories for the kinds of mail you get." was weak text (2.9:1 light, 2.7:1 dark); it is an instruction, now in the text colour (7.6:1, 5.1:1) | pass | 2026-10-07 |
| 16-not-found | pass: "mailtriage not found (looked in …)" (the spec's text; the guide says what to do) | pass | pass: error colour as 05 | fixed: Add and Apply said "Wait until loading finishes" for a load that never comes; they now say "The categories could not be loaded; the message at the bottom says why" (window and model tests) | pass: the message names where the tray looked | pass: exits 3 (window test) | 2026-10-07 |
| Menus (`menus.txt`, 8 states) | pass: "Running · checked HH:MM", "Stopped", "Problem since HH:MM", "Open log", "Status unknown", "Runs another config" with "Runs PATH"; no exit codes or JSON. Commands in menu lines keep the spec's backticks, which a native menu shows as typed | pass: summary, notices, accounts with a submenu each, then Refresh now, Start at login, Quit. With every account off, including one whose service runs another config, the summary reads "Stopped", as the spec's icon rule says | native drawing: human check | pass: "Starting…", "Stopping…", "Installing…" replace the service item while it runs | pass: "No check yet", "Not installed" with "Install service" | native menu: human check | 2026-10-07 |
| Menu bar (real) | menu bar: human check | menu bar: human check | menu bar: human check | menu bar: human check | menu bar: human check | menu bar: human check | 2026-10-07 |
