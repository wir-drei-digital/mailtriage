# Tray app

Date: 2026-10-06
Status: Design approved in conversation; written spec awaiting review.
Builds on: [guided setup](2026-10-05-guided-setup-design.md) (background service),
[refiling](2026-10-06-filing-refile-design.md) and [automatic updates](2026-10-06-auto-update-design.md).
Implementation order: refile, automatic updates, then this.

## Goal

A small tray app on macOS and Linux shows at a glance whether mailtriage is running
for each account, lets a person start or stop the background service, and offers a
clean window to manage categories, including moving already-filed mail after a
change. Everything the tray does is one documented `mailtriage` command, so an agent
can do the same with the CLI.

## Decisions

| Topic | Decision |
| --- | --- |
| Agents | The tray is a thin client of the CLI. Every read and every change is one `mailtriage … --json` command. The tray keeps no state of its own about mail or config, and never reads or writes mailtriage's files. |
| Platforms | macOS and Linux. Windows is a later project (mailtriage itself, service, key store, updater, tray). |
| Toolkit | Rust: `tray-icon` with a `tao` event loop for the tray, `egui`/`eframe` for the window. A move to Tauri later replaces only the drawing code. |
| Program | A separate binary, `mailtriage-tray`, in a Cargo workspace with the existing crate. The `mailtriage` binary gets no GUI dependencies. |
| Features | Health per account, service start/stop/install, opening logs, a categories window with a refile panel, start at login. |
| Distribution | Separate release archives for the tray; `mailtriage update` installs them next to the CLI. |

## Architecture

### Program

A new crate `tray/` (package and binary `mailtriage-tray`) joins a Cargo workspace
whose root is the existing `mailtriage` package; both share `Cargo.lock`.

```
mailtriage-tray [--config PATH] [--mailtriage PATH]          the tray
mailtriage-tray categories --account NAME [--config PATH] [--mailtriage PATH]
mailtriage-tray autostart enable|disable|status [--config PATH] [--json]
mailtriage-tray --version                                    prints "mailtriage-tray X.Y.Z"
```

- **Tray process** (no subcommand): an icon and a menu, no window. It runs the
  platform event loop on the main thread (`tao`, which also drives GTK on Linux)
  and sets the macOS activation policy to accessory, so there is no Dock icon.
- **Categories window** (`categories`): an `eframe` app that exits when its window
  closes. The tray starts it as a child process; a person or script can start it
  directly. Keeping the window in its own process avoids hidden-window tricks in
  the tray process.
- **`autostart`**: manages starting the tray at login ([Start at login](#start-at-login)).

### Finding `mailtriage`

`--mailtriage PATH` when given; else a `mailtriage` file next to the tray's own
canonical executable path; else the first `mailtriage` on `PATH`. A `--config` given
to the tray is passed to every command it runs, and to the window it starts.

### Modules

| Module | Responsibility |
| --- | --- |
| `cli` | Runs one `mailtriage` command off the UI thread with a time limit (30 s for status, 120 s for others), parses its JSON into typed results, and keeps the exact argument list for "Show details". Unknown JSON fields are ignored; fields the tray needs but the CLI lacks become an "update mailtriage" error. |
| `model` | Pure logic, no UI: health per account, the menu's entries and actions, the window's draft state, change summaries and refile panel contents. |
| `tray` | Draws `model`'s menu with `tray-icon` and sends actions back. |
| `editor` | Draws the window with `egui` and sends actions back. |
| `autostart` | Writes and removes the login item. |
| `restart` | The re-exec rule from the update spec, for the tray process. |

`tray` and `editor` hold no logic that tests need; a later Tauri version replaces
only them.

### Commands the tray runs

This table is the contract with agents: each tray action is exactly this command.

| Action | Command |
| --- | --- |
| Refresh (every 15 s, after each action, "Refresh now"); the window's account list and filing mode | `mailtriage service status --json` |
| Start service | `mailtriage service start --account A --json` |
| Stop service | `mailtriage service stop --account A --json` |
| Install service | `mailtriage service install --account A --json` |
| Open the categories window | `mailtriage-tray categories --account A` |
| Load categories (open, Reload) | `mailtriage categories export --account A --json` |
| Check the draft | `mailtriage categories validate --account A --file DRAFT --json` |
| Apply | `mailtriage categories apply --account A --file DRAFT --expect-digest D --json` |
| Refile preview | `mailtriage filing refile --account A [--folder F] --json` |
| Refile | `mailtriage filing refile --account A [--folder F] --apply --json` |
| Start at login | `mailtriage-tray autostart enable` / `disable` |

`--config PATH` is added to each when the tray was given one. "Open log" opens a
path from `service status` with the platform's opener (`open`, `xdg-open`); on
systemd, "Copy log command" puts `journalctl --user -u mailtriage-A.service -e` on
the clipboard.

## CLI additions

All of these are ordinary CLI features that agents can use too.

### `service`

- **`service status` without `--account`** reports every account in the config:
  `{"schema_version":1,"services":[…]}`, one service object per account, sorted by
  account name. With `--account` the output is unchanged apart from the new fields.
- **New fields** in each service object:
  - `enabled`: `false` after `service stop`, else `true`; `false` when not installed.
    launchd: the label is listed as disabled by `launchctl print-disabled gui/<uid>`.
    systemd: `systemctl --user is-enabled <unit>` is not `enabled`.
  - `interval_seconds`: the `--interval-seconds` recorded in the unit file, `null`
    when not installed.
  - `filing_mode` (`off`, `dry_run`, `live`) and `identity`, from the config.
  - (From the update spec: `update` and `last_pass.version`.)
- **`service stop --account A`** stops the service and keeps it stopped across
  logins and reboots; the unit file stays. launchd: the same wait-for-bootout as
  `install`, then `launchctl disable gui/<uid>/<label>`. systemd:
  `systemctl --user disable --now <unit>`. Result: `{"action":"stopped"|"already_stopped",…}`.
- **`service start --account A`**: launchd: `launchctl enable`, then `bootstrap`
  when the job is not loaded. systemd: `systemctl --user enable --now <unit>`.
  Result: `{"action":"started"|"already_running",…}`. Not installed: exit 2,
  `service for account A is not installed; run mailtriage service install --account A`.
- **`service install`** runs `launchctl enable` before `bootstrap`, so it works after
  a `stop`. (systemd's `enable` already covers this.)
- Exit codes as for `install`: 2 for unknown accounts and unsupported platforms,
  3 when `launchctl` or `systemctl` fails.

### `categories`

- **`categories export`** adds `digest`: the lowercase hex SHA-256 of the account's
  categories serialized as compact JSON (`serde_json::to_vec` of the array, as
  stored).
- **`categories apply --expect-digest HEX`** (optional): under the config lock, when
  the current digest differs, nothing is written and the command exits 5 with
  `categories changed since export; export again`.
- **`categories validate --account A`** adds `changes`, comparing the file with the
  account's current categories after the same normalization `apply` performs
  (with filing on, existing categories keep their folder):

  ```json
  "changes":{"added":["travel"],
             "removed":[{"id":"promo","folder":"Promotions"}],
             "renamed":[{"id":"news","from":"News","to":"Newsletters"}],
             "folders_changed":[{"id":"news","from":"News","to":"Newsletters"}],
             "edited":["work"],
             "reclassifies":true}
  ```

  `folder`, `from` and `to` in folder entries are effective folders. `edited` lists
  categories whose `description`, `examples` or `catch_all` changed.
  `reclassifies` is true exactly when `apply` would advance `taxonomy_revision`.
  Without `--account`, `changes` is absent.

## Tray menu and health

### Health per account

From each service object:

| State | When |
| --- | --- |
| `error` | Enabled and installed but not running, or the last pass exited 2, 3 or 5. |
| `warning` | The last pass was partial (exit 4), or finished more than 3 × `interval_seconds` + 10 min ago. |
| `ok` | Running, and the last pass is clean and recent, or there is no pass yet ("Starting…"). |
| `stopped` | Installed, `enabled: false`. |
| `not_installed` | No unit file. |
| `unavailable` | `manager: "none"`: no launchd or systemd here. |

The icon shows the worst state across accounts: `error`, then `warning`, then `ok`.
It shows "off" when no account is `ok`, `warning` or `error`.

Icons are embedded PNGs. On macOS they are template images (monochrome, following
the menu bar's light or dark look), so states differ by shape: plain for ok, a `!`
badge for warning and error, an outline for off. Linux uses colored versions of the
same shapes. The tooltip repeats the summary line.

### Menu

```
mailtriage: daniel needs attention                 (summary, not clickable)
──────────────────
daniel  ● Running · checked 12:03                  ▸ Running · filing live
                                                     Last check 12:03, some mail skipped · v0.3.0
                                                     ──────
                                                     Stop service
                                                     Open log
                                                     Edit categories…
info    ○ Not installed                            ▸ Install service
                                                     Edit categories…
──────────────────
Refresh now
Start at login  ✓
Quit
```

- Account rows show `identity` when it differs from the account name.
- Per state, the service item is "Stop service" (`ok`, `warning`, `error`),
  "Start service" (`stopped`) or "Install service" (`not_installed`); `unavailable`
  shows "No background service on this system" instead.
- `error` adds "Open error log" (the `.err` file) on launchd.
- Times are local `HH:MM`, with the date when not today.
- The summary line reads "All accounts running", "daniel needs attention",
  "Stopped", or the failure below.
- A mismatch between `update.current` (the CLI's version) and the tray's own version
  adds the line "mailtriage X and tray Y differ; run mailtriage update".

### When `mailtriage` fails

| Situation | Summary line |
| --- | --- |
| No `mailtriage` found | "mailtriage not found" plus where the tray looked. |
| No config | "Not set up. Run `mailtriage setup` in a terminal." The tray never runs `setup`. |
| Any other failure | The command's error message; "Show details" (copy) gives the command and its output. |

A failed refresh keeps the last good menu for up to two minutes, marked "(stale)",
then shows the failure.

### One tray, one window per account

The tray holds an exclusive lock on `tray.lock` in the update cache directory
(`~/Library/Caches/mailtriage`, `$XDG_CACHE_HOME/mailtriage` or
`~/.cache/mailtriage`); a second tray prints `mailtriage-tray is already running`
and exits 0. The tray remembers the window process it started per account; while
that one runs, "Edit categories…" brings it to the front on macOS
(`NSRunningApplication` by PID) and does nothing on Linux.

## Categories window

`mailtriage-tray categories --account A`, about 720 × 520, resizable.

### Layout

- **Header:** the account (a selector when the config has several; switching with
  unsaved edits asks first) and "Reload".
- **Left:** the categories, each showing its name and mail folder; the default
  category is marked. "Add" below the list.
- **Right:** the selected category:
  - **Name**;
  - **What belongs here** (`description`, multi-line);
  - **Examples** (one per line; a hint says senders or subjects work well);
  - **Default category** (`catch_all`): "Use for mail that fits nowhere else",
    a choice where exactly one category is selected;
  - **Mail folder** (`folder`), placeholder "Same as name"; shown when the
    account's filing is not `off`;
  - **Advanced** (collapsed): **ID**. For a new category it is suggested from the
    name (lowercase letters, digits, `-`) and editable until applied; for existing
    categories it is read-only with the note "Keep IDs: corrections and folders
    refer to them".
  - "Remove category" at the bottom of the form.
- **Footer:** the check result and change summary on the left; "Revert" and
  "Apply" (primary) on the right.
- **Refile panel**: below the form when shown (see [Refile panel](#refile-panel)).

### Flow

1. **Load.** `categories export` fills the window and records `digest`. The
   account list and each account's `filing_mode` come from `service status --json`.
2. **Edit.** 500 ms after the last change, the draft is written as
   `{"categories":[…]}` to a file in a private temporary directory (mode 0700) and
   checked with `categories validate --account A --file DRAFT --json`.
   - Errors appear under the footer in plain words, with "Show details".
   - The footer summarizes `changes` in words: "1 added, 1 renamed, mail folder
     changed for Newsletters".
   - Apply is disabled while the draft is invalid, unchanged, or still being
     checked; its tooltip says why.
3. **Apply.** A confirmation lists the changes in words.
   - With `reclassifies`, it adds: "All open mail in this account will be sorted
     again. This uses your OpenRouter key and takes a few passes."
   - Then it runs `categories apply … --expect-digest D`.
   - Success: "Saved." plus what happens next. The window reloads from `export`,
     and the refile panel appears when filing is not `off` and the change added,
     removed or re-pointed a category, or reclassifies.
   - Exit 5 (changed elsewhere): "These categories were changed somewhere else."
     with "Reload (discard my edits)" and "Keep editing".
   - Any other error: its message, with "Show details".
4. **Remove.** Asks first, naming the category and saying what happens: "Mail in
   Promotions stays there until you move it; new mail is sorted into the remaining
   categories." The removal is part of the draft; nothing is written until Apply.
5. **Close.** With unsaved edits, asks "Discard changes?". The temporary directory
   is removed on exit.

### Refile panel

Shown when the account's filing is not `off`, after an apply as above, or through a
"Move filed mail…" button in the footer at any time.

- **Folders no longer used.** For each effective folder that the last apply in this
  window retired (`removed[].folder`, `folders_changed[].from`) and that no
  remaining category uses: `filing refile --account A --folder F --json`.
  - "Folder Promotions is no longer used. 34 messages there will move to their new
    categories, 12 of them once they are sorted again."
  - "Move" runs the same command with `--apply`, which also marks the waiting
    messages.
- **Mail in the wrong folder.** `filing refile --account A --json`:
  - "8 messages are in a folder that no longer matches their category."
  - "Move 8 messages" runs it with `--apply`.
  - With `waiting` > 0: "W messages are still being sorted again. Open this panel
    later to move those whose category changed."
- **Later.** When the window is opened again, retired folders from earlier applies
  are not known; mail in them that has been sorted again appears under "Mail in the
  wrong folder" (refile's `folder_retired` candidates).
- **Skipped mail.** The `skipped` counts appear under "Not moved", in words (for
  example "corrected by you: 3"), collapsed by default.
- With filing `dry_run`, both parts show the preview only, with "Moving filed mail
  needs filing set to live."
- After a move: "Marked. The background service moves them over the next passes."

## UI principles

The tray and the window must be clean and friendly. The design review in the plan
checks every screen against these.

- **Plain words.** No internal names on screen: no `catch_all`, `taxonomy`, `digest`,
  exit codes or JSON. Technical detail lives behind "Show details", which shows the
  exact command, its exit code and output, and has "Copy".
- **Calm layout.** List on the left, form on the right, one primary button (Apply)
  bottom right with Revert beside it. Destructive actions ask first and say what
  happens to mail.
- **Look.** Follows the system's light or dark mode. Spacing on an 8 px grid; text
  14 pt, headings 18 pt; consistent field widths; no decorative clutter.
- **Feedback.** Every command shows a spinner and a verb ("Checking…",
  "Applying…", "Moving…"); the window never freezes. Success says what happens
  next. Disabled controls say why in their tooltip.
- **Empty and first-run states** explain what to do, for example an account with
  only the default category: "Add categories for the kinds of mail you get."
- **Keyboard.** ⌘/Ctrl+N adds a category, ⌘/Ctrl+S opens the apply confirmation,
  Esc closes dialogs, ⌘/Ctrl+W closes the window (asking about unsaved edits), Tab
  follows the visual order.
- **Accessibility.** `eframe`'s AccessKit support is on, so screen readers work and
  every control has an accessible label; tests and agents use the same labels.
- **Menu wording.** Short, human: "Running · checked 12:03", "Stopped",
  "Problem since 11:40", "Open log".

## Start at login

`mailtriage-tray autostart enable|disable|status [--config PATH] [--json]`, and the
"Start at login" menu item, which runs the same code.

- **macOS:** `~/Library/LaunchAgents/digital.wirdrei.mailtriage-tray.plist` (the
  hyphen keeps it apart from the per-account labels
  `digital.wirdrei.mailtriage.<account>`). It runs the tray's canonical path, with
  `--config` when one was given to `enable`, the current `PATH` recorded as
  `service install` does, `RunAtLoad`, `KeepAlive` with `SuccessfulExit` false
  (Quit stays quit; a crash restarts), and `LimitLoadToSessionType` `Aqua`.
  `enable` writes the file for the next login; it does not start a second tray.
  `disable` deletes the file and leaves the running tray alone.
- **Linux:** `$XDG_CONFIG_HOME/autostart/mailtriage-tray.desktop` (when
  `XDG_CONFIG_HOME` is absolute, else `~/.config/autostart/…`) with `Type=Application`,
  `Name=mailtriage`, `NoDisplay=true`, and `Exec=` quoted per the Desktop Entry
  specification.
- Both files carry a marker, as service files do. A file without it exits 5 and is
  left alone.
- Result: `{"schema_version":1,"autostart":{"enabled":true,"path":"…"}}`. Exit codes:
  0; 2 for invalid arguments; 3 when the file cannot be written; 5 as above.

## Distribution and updates

- **Archives.** The release workflow adds
  `mailtriage-tray-vX.Y.Z-{macos-arm64,linux-amd64,linux-arm64}.tar.gz`, each with
  `mailtriage-tray` and `LICENSE` at the top level, plus a `.sha256` file per
  archive, all listed in the same `SHA256SUMS`. The `mailtriage` archives are
  unchanged and stay free of GUI libraries.
- **Runtime needs.** macOS: nothing beyond the OS; the binary is unsigned like the
  CLI. Linux: GTK 3, the Ayatana AppIndicator library and `libxdo` (as `tray-icon`
  documents), a desktop with a StatusNotifier host (KDE; GNOME with the AppIndicator
  extension, which Ubuntu ships), and OpenGL for the window.
- **Updating (extends the update spec).** After the CLI part of an update (installed
  or already current), when a regular file `mailtriage-tray` sits in the same
  directory as the CLI's canonical path:
  1. Run `mailtriage-tray --version` (10 s limit). If it does not run, skip the
     tray: `tray: {"action":"skipped","error":"mailtriage-tray does not run here: …"}`,
     nothing downloaded.
  2. If its version is older than the latest release, install the tray archive with
     steps 4 to 9 of the update spec: same allowlist, limits and `SHA256SUMS`;
     unpack the top-level `mailtriage-tray`; the smoke test expects
     `mailtriage-tray X.Y.Z`; swap with `mailtriage-tray.previous`.
  3. Report `tray: {"action":"updated"|"current"|"skipped"|"failed","from","to","error"}`
     in `update`'s result, and `tray: {"path","version","available"}` in
     `update --check`. Without a tray file, `tray` is absent.
  - A tray failure leaves the old tray in place. `update` then exits **4**: the CLI
    part succeeded or was current, the tray part failed. In `watch`, it is an
    `error` event; the next check tries again, because the tray is still older.
- **Restarting.** The tray process applies the update spec's restart rule to its own
  executable: when the file at its path is replaced and `--version` runs, it
  re-executes itself with its arguments. An open categories window keeps running.
- **CI.** Linux jobs install the GTK, AppIndicator and `xdo` development packages;
  formatting, clippy and tests run with `--workspace`; build and release jobs package,
  smoke-test (`--version`) and upload the tray archives next to the CLI archives.

## Errors and exit codes

The tray itself never exits on a command failure; it shows it. `mailtriage-tray
categories` exits 0 when closed, 2 for invalid arguments, 3 when `mailtriage` cannot
be found or the first `export` fails (after showing the message in the window).
`autostart` exit codes are listed above. CLI exit codes for the new commands are
listed with them.

## Testing

No test runs the real `launchctl`, `systemctl` or keychain, or writes into the real
`HOME`.

**`model` unit tests**

- Health from `service status` fixtures: every state; staleness exactly at and past
  3 × interval + 10 min; no pass yet; `manager: "none"`; worst state across
  accounts; "off" when nothing runs.
- Menu entries and actions for each state; `identity` shown only when it differs;
  local time and date formatting; the version-mismatch line.
- Window state: unsaved-change tracking; ID suggestion from names with accents,
  spaces and symbols; exactly one default category; change summaries in words for
  each `changes` kind; Apply's disabled reasons; retired folders computed from
  `changes` (a folder still used by another category is not retired).
- Refile panel texts for `total`, `waiting`, `skipped` and `dry_run`.

**Agent parity.** A fake `mailtriage` script answers with fixture JSON and records
its arguments. One table test runs every tray and window action and asserts the
exact command line from [Commands the tray runs](#commands-the-tray-runs), with and
without `--config`.

**Window tests** with `egui_kittest`, headless, finding controls by their
accessible labels:

- Edit a name, Apply, confirm: the apply command carries `--expect-digest`.
- Apply answered with exit 5: the reload offer appears; "Keep editing" keeps the
  draft.
- A validation error shows inline and disables Apply.
- A reclassifying change shows the OpenRouter warning in the confirmation.
- Remove asks first; nothing is written until Apply.
- Closing with unsaved edits asks.
- The refile panel: retired-folder move uses `--folder`; dry-run shows the preview
  only; skipped counts collapsed.

**CLI additions** (in the `mailtriage` crate)

- `service status` without `--account`: every account, sorted, new fields present.
- `service stop` and `start` with the existing fake `launchctl`/`systemctl`: call
  sequences, `already_*` results, start when not installed (exit 2), and `install`
  after `stop` runs `enable`.
- `categories export` digest stable across exports; `apply --expect-digest` with a
  matching and a stale digest (exit 5, nothing written).
- `categories validate --account` `changes`: each kind; a rename with filing on
  keeps the folder; `reclassifies` matches whether `apply` advances
  `taxonomy_revision`.

**Autostart.** `enable`, `status` and `disable` against a temporary `HOME`; the plist
and `.desktop` contents; a file without the marker (exit 5).

**Updates.** With the update spec's fake release server: a tray next to the CLI is
updated; a tray whose `--version` fails is skipped without a download; a bad tray
checksum gives exit 4 with the CLI updated; the next check retries the tray.

**By hand** (added to `docs/verification.md`): macOS menu bar in light and dark mode;
Ubuntu GNOME with the AppIndicator extension; KDE; start at login on each; the
design review of every screen against [UI principles](#ui-principles), with
screenshots.

## Docs

- `guide.md`: a **Tray** section (install, start at login, what the states mean,
  the categories window, Linux requirements) and the new `service start`/`stop`,
  `service status` without `--account`, `--expect-digest` and `changes`.
- `hermes.md`: the new CLI features for agents; agents never need the tray.
- `releases.md`: the tray archives.
- README: one line.

## Out of scope

- Windows.
- Showing or installing updates and the attention list in the tray UI.
- Notifications.
- Running `setup` from the tray.
- A Tauri version.
- Editing anything except categories (filing mode, accounts, provider).
