# Tray app

Date: 2026-10-06
Status: Design approved in conversation; written spec revised after Codex review round 1.
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
| Toolkit | Rust: `tray-icon` with a `tao` event loop for the tray, `egui`/`eframe` (glow renderer, AccessKit) for the window. A move to Tauri later replaces only the drawing code. |
| Program | A separate binary, `mailtriage-tray`, in a Cargo workspace with the existing crate. The `mailtriage` binary gets no GUI dependencies. |
| Features | Health per account, service start/stop/install, opening logs, a categories window with a refile panel, start at login. |
| Distribution | Separate release archives for the tray; `mailtriage update` installs them next to the CLI. |

## Architecture

### Program

A new crate `tray/` (package and binary `mailtriage-tray`) joins a Cargo workspace
whose root is the existing `mailtriage` package; both share `Cargo.lock`.

```
mailtriage-tray [--config PATH] [--mailtriage PATH]                       the tray
mailtriage-tray categories [--account NAME] [--config PATH] [--mailtriage PATH]
mailtriage-tray autostart enable|disable|status [--config PATH] [--mailtriage PATH] [--json]
mailtriage-tray --version                                                 prints "mailtriage-tray X.Y.Z"
```

- **Tray process** (no subcommand): an icon and a menu, no window. It runs the
  platform event loop on the main thread (`tao`, which also drives GTK on Linux) and
  sets the macOS activation policy to accessory, so there is no Dock icon.
  - The tray icon is created only after the event loop has started
    (`StartCause::Init`).
  - Worker threads and menu events reach the loop through an `EventLoopProxy`, so an
    idle loop wakes for them.
  - Every native UI call happens on the main thread.
- **Categories window** (`categories`): an `eframe` app that exits when its window
  closes. The tray starts it as a child process; a person or script can start it
  directly. Keeping the window in its own process avoids hidden-window tricks in the
  tray process.
- **`autostart`**: manages starting the tray at login ([Start at login](#start-at-login)).

### Paths: `mailtriage` and the config

Resolved once at start, made absolute, and passed on explicitly. The tray, the window
and the login item therefore always use the same CLI and config, whatever the working
directory or environment of a later start.

- **CLI:** `--mailtriage PATH` when given, else a `mailtriage` file next to the
  tray's own canonical executable path, else the first `mailtriage` on `PATH`. The
  result is canonicalized.
- **Config:** `--config PATH` when given, made absolute against the working
  directory. Otherwise the `config` field of the first `service status --json`
  result, which reports the absolute config path the CLI resolved. From then on the
  tray passes `--config <absolute path>` to every command.
- The tray passes both `--mailtriage` and `--config` to every window it starts. The
  login item records both ([Start at login](#start-at-login)).

### Modules

| Module | Responsibility |
| --- | --- |
| `cli` | Runs one `mailtriage` command off the UI thread with a time limit (30 s for status, 120 s for others), parses its JSON (including `error.reason`) into typed results, and keeps the exact argument list for "Show details". Unknown JSON fields are ignored; fields the tray needs but the CLI lacks become an "update mailtriage" error. |
| `model` | Pure logic, no UI: health per account, the menu's entries and actions, the window's draft state and revisions, change summaries and refile panel contents. |
| `tray` | Draws `model`'s menu with `tray-icon` and sends actions back. |
| `editor` | Draws the window with `egui` and sends actions back. |
| `autostart` | Writes and removes the login item. |
| `restart` | The re-exec rule from the update spec, for the tray process. |

`tray` and `editor` hold no logic that tests need; a later Tauri version replaces
only them.

### Commands the tray runs

This table is the contract with agents: each tray action is exactly this command.
`--config <absolute path>` is added to every `mailtriage` command, and the tray's
`--mailtriage` and `--config` to every `mailtriage-tray` command.

| Action | Command |
| --- | --- |
| Refresh (every 15 s, after each action, "Refresh now"); the window's account list and filing modes | `mailtriage service status --json` |
| Start service | `mailtriage service start --account A --json` |
| Stop service | `mailtriage service stop --account A --json` |
| Install service | `mailtriage service install --account A --json` |
| Open the categories window | `mailtriage-tray categories --account A` |
| Load categories (open, Reload, account switch) | `mailtriage categories export --account A --json` |
| Check the draft | `mailtriage categories validate --account A --file DRAFT --json` |
| Apply | `mailtriage categories apply --account A --file DRAFT --expect-digest D --json` |
| Refile preview | `mailtriage filing refile --account A [--folder NATIVE] --json` |
| Refile | `mailtriage filing refile --account A [--folder NATIVE] --apply --json` |
| Start at login | `mailtriage-tray autostart enable` / `disable` |

"Open log" opens a path from `service status` with the platform's opener (`open`,
`xdg-open`) after checking that it is one of the service's `log_paths`. On systemd,
"Copy log command" puts `journalctl --user -u mailtriage-A.service -e` on the
clipboard.

## CLI additions

All of these are ordinary CLI features that agents can use too.

### Error reasons

The JSON error object gains an optional, machine-readable `reason` next to `code` and
`message`. Existing codes and messages are unchanged. This spec introduces these
reasons:

| Reason | Code | When |
| --- | --- | --- |
| `categories_changed` | 5 | `--expect-digest` does not match. |
| `config_busy` | 5 | Another command holds the config lock. |
| `config_changed` | 5 | `mailtriage.json` changed while the command ran (the existing unchanged-file guard). |
| `binding_conflict` | 5 | The account is bound to another mailbox. |
| `service_config_mismatch` | 5 | The service runs another config (see below). |

### `service`

- **`service status` without `--account`** reports every account in the config:
  `{"schema_version":1,"config":"/abs/mailtriage.json","services":[…]}`, one service
  object per account, sorted by account name. With `--account`, the output adds the
  same top-level `config` and is otherwise unchanged apart from the new fields.
- **New fields** in each service object:
  - **`service_config`**: the absolute `--config` path decoded from the unit file
    (with the same decoding the update spec uses for the executable), or `null` when
    not installed or undecodable.
  - **`config_matches`**: whether `service_config` names the same file as the
    resolved config (canonical paths). It is `true` when not installed.
  - **`enabled`** (`true`, `false` or `null` when unknown) and **`enablement`** (the
    raw state):
    - launchd: `launchctl print-disabled gui/<uid>` lists the label as `=> disabled`
      or `=> true`: `false`. As `=> enabled` or `=> false`, or not at all: `true`.
      When the query fails: `null`. `enablement` is `enabled`, `disabled` or
      `unknown`.
    - systemd: `enablement` is the output of `systemctl --user is-enabled <unit>`.
      `enabled` and `enabled-runtime` give `true`; `disabled`, `masked` and
      `masked-runtime` give `false`; anything else, or a failed query, gives `null`.
    - Not installed: `enabled: false`, `enablement: "not_installed"`.
  - `interval_seconds`: the `--interval-seconds` recorded in the unit file, `null`
    when not installed.
  - `filing_mode` (`off`, `dry_run`, `live`) and `identity`, from the config.
  - (From the update spec: `update` and `last_pass.version`.)
- **`service stop --account A`** stops the service and keeps it stopped across
  logins and reboots, for the user-scope unit mailtriage wrote. The unit file stays.
  - launchd: the same wait-for-bootout as `install`, then
    `launchctl disable gui/<uid>/<label>`.
  - systemd: `systemctl --user disable --now <unit>`. A unit that is also enabled
    globally or masked is outside this guarantee; `status` shows it in `enablement`.
  - Result: `{"action":"stopped"|"already_stopped",…}`.
- **`service start --account A`:**
  - launchd:
    - not loaded: `launchctl enable`, then `bootstrap`;
    - loaded and running: `already_running`;
    - loaded but not running: `launchctl enable`, then
      `launchctl kickstart gui/<uid>/<label>`.
  - systemd: `systemctl --user enable --now <unit>`.
  - Afterwards it checks for up to 5 s that the job is running (`launchctl print`
    shows a PID; `systemctl --user is-active` prints `active`). Otherwise it exits 3
    with `service did not start; see <log>`.
  - Result: `{"action":"started"|"already_running",…}`.
  - Not installed: exit 2,
    `service for account A is not installed; run mailtriage service install --account A`.
- **Binding to a config.** `start` and `stop` exit 5 with reason
  `service_config_mismatch` when `config_matches` is false:
  `service for account A runs config X; pass --config X`. `install` keeps its current
  meaning: it rewrites the unit for the resolved config.
- **`service install`** runs `launchctl enable` before `bootstrap`, so it works after
  a `stop`. (systemd's `enable` already covers this.)
- Exit codes as for `install`: 2 for unknown accounts and unsupported platforms,
  3 when `launchctl` or `systemctl` fails, 5 as above.

### `categories`

- **Digest.** `categories export` and `categories validate --account A` add `digest`.
  It identifies everything that decides how `apply` writes the account's categories:
  the categories and whether the account's filing is on (filing on keeps existing
  folders).
  - It is `v1:` followed by the lowercase hex SHA-256 of
    `serde_json::to_vec(&DigestInput { filing_on, categories })`.
  - Each `Category` serializes in declaration order: `id`, `name`, `description`,
    `examples` (always, possibly `[]`), `catch_all` (always), then `folder` (omitted
    when unset).
  - The digest is computed from the typed values, so the key order of an input file,
    or `"folder": null` versus no `folder`, does not change it. Category order does.
  - A future change to this encoding changes the prefix.
- **`categories apply --expect-digest D`** (optional). Under the exclusive config
  lock, before any other change, the current digest must equal `D`. Otherwise
  nothing is written and the command exits 5 with reason `categories_changed`:
  `categories changed since export; export again`. The existing unchanged-file guard
  stays.
- **`categories validate --account A`** adds `changes`. It compares the file with the
  account's current categories after `apply`'s own normalization; both use the same
  function. With filing on, an existing category without a `folder` in the file keeps
  its current folder.

  ```json
  "changes":{"added":["travel"],
             "removed":[{"id":"promo","folder":"Promotions"}],
             "renamed":[{"id":"news","from":"News","to":"Newsletters"}],
             "folders_changed":[{"id":"news","from":"News","to":"Newsletters"}],
             "edited":["work"],
             "reclassifies":true}
  ```

  - Folder entries hold configured (effective) folder names, for display only.
  - `edited` lists categories whose `description`, `examples` or `catch_all` changed.
  - `reclassifies` is true exactly when `apply` would advance `taxonomy_revision`.
  - Without `--account`, `changes` and `digest` are absent.

## Tray menu and health

### Health per account

The first matching row wins:

| State | When | Menu text |
| --- | --- | --- |
| `unavailable` | `manager: "none"` | "No background service on this system" |
| `not_installed` | No unit file. | "Not installed" |
| `other_config` | `config_matches` is false. | "Runs another config" (with the path) |
| `stopped` | `enabled` is false. | "Stopped" |
| `unknown` | `enabled` is null and the job is not running. | "Status unknown" (with the error) |
| `restarting` | Not running, and the tray has seen it not running for less than 2 min. | "Restarting…" |
| `error` | Not running for 2 min or more, or the last pass exited 2 or 3. | "Problem since HH:MM" |
| `warning` | The last pass was partial (exit 4) or hit a configuration change (exit 5); or it finished more than 3 × `interval_seconds` + 30 min ago; or there is no pass yet although the tray has seen this PID for longer than that. | "Some mail skipped", "Configuration changed", "No check since HH:MM", "No check yet" |
| `starting` | Running and no pass yet. | "Starting…" |
| `ok` | Running, and the last pass was clean and recent. | "Running · checked HH:MM" |

The tray keeps per-account observations in memory: when it first saw the job not
running, and when it first saw the current PID. A tray restart resets them.

The icon shows the worst state across accounts: `error` and `unknown`, then
`warning`, then `ok`, `starting` and `restarting`. It shows "off" when every account is
`stopped`, `not_installed`, `other_config` or `unavailable`.

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
- The service item depends on the state:
  - "Stop service" for `ok`, `starting`, `restarting`, `warning`, `error`;
  - "Start service" for `stopped`;
  - "Install service" for `not_installed`;
  - nothing for `other_config`, `unknown` and `unavailable`.
- `error` adds "Open error log" (the `.err` file) on launchd.
- Times are local `HH:MM`, with the date when not today.
- The summary line reads "All accounts running", "daniel needs attention",
  "Stopped", or the failure below.
- A mismatch between the CLI's version (`update.installed` for the binary the tray
  runs, else `mailtriage --version`) and the tray's own version adds the line
  "mailtriage X and tray Y differ; run mailtriage update".

### When `mailtriage` fails

| Situation | Summary line |
| --- | --- |
| No `mailtriage` found | "mailtriage not found" plus where the tray looked. |
| No config | "Not set up. Run `mailtriage setup` in a terminal." The tray never runs `setup`. |
| Any other failure | The command's error message; "Show details" (copy) gives the command and its output. |

A failed refresh keeps the last good menu for up to two minutes, marked "(stale)",
then shows the failure.

### Instances and child processes

- **One tray.** The tray holds an exclusive lock on `tray.lock` in the update spec's
  cache directory. A second tray prints `mailtriage-tray is already running` and
  exits 0.
  - The lock's file descriptor closes on `exec`. After a self-restart, the new image
    takes the lock again; if another tray took it in between, it exits 0.
- **One window per config.** The window has an account selector. It holds an
  exclusive lock on `editor-<first 16 hex of SHA-256 of the absolute config path>.lock`
  in the cache directory and writes its PID to the matching `.pid` file.
  - A second `categories` start for the same config exits 0. On macOS it first brings
    the running window to the front (`NSRunningApplication` for that PID). On Linux it
    prints `the categories window is already open`, and the tray shows that as a
    short notice.
  - The running window keeps its selected account.
  - The lock belongs to the window process, so it survives a tray restart.
- **Children.** The tray does not wait on the windows it starts. On every refresh it
  reaps exited children with `waitpid(-1, WNOHANG)`, which also covers windows started
  before a self-restart.

## Categories window

`mailtriage-tray categories [--account A]`, about 720 × 520, resizable. Without
`--account` it opens the first account.

### Layout

- **Header:** the account selector and "Reload".
- **Left:** the categories, each showing its name and mail folder; the default
  category is marked. "Add" below the list.
- **Right:** the selected category:
  - **Name**;
  - **What belongs here** (`description`, multi-line);
  - **Default category** (`catch_all`): "Use for mail that fits nowhere else",
    a choice where exactly one category is selected;
  - **Mail folder** (`folder`): shown when the account's filing is not `off`. It
    always displays the folder mail will go to:
    - the stored folder for existing categories;
    - for new ones, the name as a placeholder until typed over.
  - **Advanced** (collapsed):
    - **ID.** For a new category it is suggested from the name (lowercase letters,
      digits, `-`) and editable until applied. For existing categories it is
      read-only, with the note "Keep IDs: corrections and folders refer to them".
    - **Examples**, one per line, with the note "Saved with the category; not sent to
      the classifier".
  - "Remove category" at the bottom of the form.
- **Footer:** the check result and change summary on the left; "Move filed mail…",
  "Revert" and "Apply" (primary) on the right.
- **Refile panel:** below the form when shown ([Refile panel](#refile-panel)).

**Serializing the folder field.**

- **Filing on:** the draft always carries an explicit `folder`: the field's text, or
  the category's name when the field is left empty.
- **Filing off:** an empty field omits `folder`; a filled one writes it.
- So "Same as name" always means the current name, never an inherited old folder.

### Flow

1. **Load.** `categories export` fills the window and records `digest`. The account
   list and each account's `filing_mode` come from `service status --json`.
2. **Edit.** Every change increases the draft's revision.
   - **Check.** 500 ms after the last change, the draft is written as
     `{"categories":[…]}` to `draft-<revision>.json`, created exclusively in a
     private temporary directory (mode 0700). It is checked with
     `categories validate --account A --file DRAFT --json`.
   - **Out-of-order answers.** Each check carries its account and revision. An
     answer for an older revision or another account is ignored, and its file is
     deleted when its command ends.
   - **Showing the result.** Errors appear under the footer in plain words, with
     "Show details". The footer summarizes `changes` in words, for example
     "1 added, 1 renamed, mail folder changed for Newsletters".
   - **When Apply is enabled.** Only when the latest finished check is for the
     current revision and valid, and the draft differs from what was loaded. While
     it is disabled, its tooltip says why.
3. **Apply.** A confirmation lists the changes in words.
   - With `reclassifies`, it adds: "All open mail in this account will be sorted
     again. This uses your OpenRouter key and takes a few passes."
   - On confirmation, the window runs `categories apply` with that revision's file
     (never rewritten) and the digest from the load.
   - While apply or a refile command runs, the form is read-only, and the account
     selector and Reload are disabled.
   - Results:
     - **success:** "Saved." plus what happens next. The window reloads from
       `export` and opens the refile panel when filing is not `off` and `changes`
       added, removed or re-pointed a category, or reclassifies;
     - `categories_changed`: "These categories were changed somewhere else.", with
       "Reload (discard my edits)" and "Keep editing";
     - `config_busy`: "mailtriage is busy; trying again…", retried up to three times
       2 s apart, then shown with "Try again";
     - `config_changed`: "The configuration changed while saving. Checking your
       changes again.", after which the draft is checked again;
     - any other error: its message, with "Show details".
4. **Remove.** Asks first, naming the category and saying what happens: "Mail in
   Promotions stays there until you move it; new mail is sorted into the remaining
   categories." The removal is part of the draft; nothing is written until Apply.
5. **Close.** With unsaved edits, asks "Discard changes?". While a command runs, the
   window shows "Finishing…" and closes when it ends. The temporary directory is
   removed on exit.

### Refile panel

Shown when the account's filing is not `off`: after an apply as above, or through
"Move filed mail…" at any time. Its content comes from
`filing refile --account A --json`, whose `folders` list (from the refile spec)
supplies the server folder names.

- **Folders no longer used.** One row per `folders` entry with `retired: true`:
  - "Promotions is no longer used: 30 messages can move now; 12 more may move once
    they are sorted again."
  - "Move" runs `filing refile --account A --folder <native> --apply`, which also
    marks the waiting messages.
- **Mail in the wrong folder.** The remaining candidates:
  - "8 messages are in a folder that no longer matches their category."
  - "Move 8 messages" runs `filing refile --account A --apply`.
  - With `waiting` outside retired folders: "W messages are still being sorted
    again; some may need moving afterwards. Open this panel again later."
- **Not moved.** The `skipped` counts in words (for example "corrected by you: 3"),
  collapsed by default.
- **After a move,** the panel reports the result's actual `marked` and
  `waiting_marked`: "Marked 30 messages; 12 more will move if their new category
  calls for it."
  - When the account's state is not `ok`, `starting`, `restarting` or `warning`, it
    adds: "The background service is not running. Start it, or run
    `mailtriage sync`, to move them."
- **Filing in `dry_run`:** the panel shows the preview only, with "Moving filed mail
  needs filing set to live."

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

`mailtriage-tray autostart enable|disable|status [--config PATH] [--mailtriage PATH] [--json]`,
and the "Start at login" menu item, which runs the same code with the tray's
resolved paths.

- **What the login item records.** `enable` records the absolute CLI path and the
  absolute config path as `--mailtriage` and `--config` arguments.
  - Both are resolved as in [Paths](#paths-mailtriage-and-the-config).
  - Without `--config`, `enable` runs `mailtriage service status --json` once to learn
    the config path. When that fails, it exits 3 and names `--config`.
- **macOS:** `~/Library/LaunchAgents/digital.wirdrei.mailtriage-tray.plist`. The
  hyphen keeps it apart from the per-account labels
  `digital.wirdrei.mailtriage.<account>`.
  - Contents: it runs the tray's canonical path with those arguments and the current
    `PATH` (recorded as `service install` does), with `RunAtLoad`, `KeepAlive` with
    `SuccessfulExit` false (Quit stays quit; a crash restarts), and
    `LimitLoadToSessionType` `Aqua`.
  - `enable` writes the file and runs
    `launchctl enable gui/<uid>/digital.wirdrei.mailtriage-tray` to clear a disabled
    override. It does not load the job, so no second tray starts.
  - `disable` deletes the file and leaves the running tray alone.
  - `status` reports `enabled: true` only when the file with the marker exists and
    the label is not disabled.
- **Linux:** `$XDG_CONFIG_HOME/autostart/mailtriage-tray.desktop` (when
  `XDG_CONFIG_HOME` is absolute, else `~/.config/autostart/…`) with
  `Type=Application`, `Name=mailtriage`, `NoDisplay=true`, and `Exec=` quoted per the
  Desktop Entry specification.
- **Markers.** Both files carry a marker, as service files do. A file without it
  exits 5 and is left alone.
- **Result and exit codes.** The result is
  `{"schema_version":1,"autostart":{"enabled":true,"path":"…","config":"…","mailtriage":"…"}}`.
  Exit codes: 0; 2 for invalid arguments; 3 when the file cannot be written or the
  config cannot be resolved; 5 as above.

## Distribution and updates

- **Archives.** The release workflow adds
  `mailtriage-tray-vX.Y.Z-{macos-arm64,linux-amd64,linux-arm64}.tar.gz`, each with
  `mailtriage-tray` and `LICENSE` at the top level, plus a `.sha256` file per
  archive, all listed in the same `SHA256SUMS`. The `mailtriage` archives are
  unchanged and stay free of GUI libraries.
- **Runtime needs.**
  - macOS: nothing beyond the OS; the binary is unsigned like the CLI.
  - Linux:
    - GTK 3, the Ayatana AppIndicator library and `libxdo`, as `tray-icon` documents;
    - a desktop with a StatusNotifier host (KDE, or GNOME with the AppIndicator
      extension, which Ubuntu ships);
    - OpenGL for the window.
- **Updating (extends the update spec).** The tray is a second component next to the
  CLI. A regular file `mailtriage-tray` in the same directory as the CLI's canonical
  path is its installation path, with its own entry in the cache's `installs`, keyed
  by that path.
  1. **Does it run here?** Run `mailtriage-tray --version` with the bounded runner.
     If it does not run, skip the tray, with
     `tray: {"action":"skipped","error":"mailtriage-tray does not run here: …"}`, and
     download nothing. A skipped tray is not a failure.
  2. **Install** the tray archive when the release is newer than the tray's installed
     version. It goes through steps 4 to 10 of the update spec, under the same
     installation lock, URL rules, limits and `SHA256SUMS`.
     - It unpacks the top-level `mailtriage-tray`.
     - The smoke test expects `mailtriage-tray X.Y.Z`.
     - The backup is `mailtriage-tray.previous`.
     - Its own commit point is the rename of the tray file.
  3. **Report** `tray: {"action":"updated"|"current"|"skipped"|"failed","from","to","error"}`
     in `update`'s result, and `tray: {"path","installed","available"}` in
     `update --check` and in `service status`'s `update` block. Without a tray file,
     `tray` is absent. The CLI's `available` keeps meaning the CLI alone.
  - **Order.** `update` handles the CLI first, then the tray, in one run.
  - **Tray failures.** A failed tray leaves the old tray in place, records
    `installs[tray].last_error` with the update spec's backoff, and does not undo the
    CLI. `update` then exits **4**: the CLI part succeeded or was current, the tray
    part failed.
  - **In `watch`.** Background installation runs when either component is older than
    the cached release and its own `next_attempt_at` has passed. After a re-exec onto
    a current CLI, an older tray is therefore still installed at a later pass. A tray
    failure is an `error` event; the CLI's restart rule still applies.
- **Restarting.** The tray process applies the update spec's restart rule to its own
  executable, with the same installation-path and image-identity handling. When its
  file is replaced and `--version` runs, it re-executes itself with its arguments. An
  open categories window keeps running.
- **CI.**
  - Linux jobs install the GTK, AppIndicator and `xdo` development packages.
  - Formatting, clippy and tests run with `--workspace`.
  - Build and release jobs package, smoke-test (`--version`) and upload the tray
    archives next to the CLI archives.

## Errors and exit codes

- **The tray** never exits on a command failure; it shows it.
- **`mailtriage-tray categories`** exits:
  - 0 when closed, or when another window for the config is already open;
  - 2 for invalid arguments;
  - 3 when `mailtriage` cannot be found or the first `export` fails, after showing
    the message in the window.
- **`autostart`** exit codes are listed above, and the CLI exit codes for the new
  commands are listed with them.

## Testing

No test runs the real `launchctl`, `systemctl` or keychain, or writes into the real
`HOME`.

**`model` unit tests**

- **Health,** from `service status` fixtures:
  - every row of the precedence table, including `stopped` with an old error, and
    `unavailable` versus `not_installed`;
  - `restarting` turning into `error` after 2 min;
  - exit 5 as `warning`;
  - staleness exactly at and past the limit;
  - no pass yet, short and long after the PID appeared;
  - the worst state across accounts, and "off".
- **Menu:** entries and actions for each state; `identity` shown only when it
  differs; local time and date formatting; the version-mismatch line.
- **Window state:**
  - revisions, and out-of-order and other-account check answers ignored;
  - suggested IDs from names with accents, spaces and symbols;
  - exactly one default category;
  - folder serialization with filing on and off;
  - change summaries in words for each `changes` kind;
  - Apply's disabled reasons;
  - the read-only form and disabled selector while a command runs.
- **Refile panel:** texts for retired folders, `waiting`-only results, `skipped`,
  `dry_run`, and a stopped service after a move.

**Agent parity.** A fake `mailtriage` script answers with fixture JSON and records
its arguments. One table test runs every tray and window action and asserts the
exact command line from [Commands the tray runs](#commands-the-tray-runs). The tray
under test is started with a relative `--config`, and with the config only in
`MAILTRIAGE_CONFIG`; every recorded command must carry the absolute path.

**Window tests** with `egui_kittest`, headless, finding controls by their accessible
labels:

- Edit a name, Apply, confirm: the apply command carries `--expect-digest` and the
  confirmed revision's file. An edit made while apply runs is impossible (the form is
  read-only).
- `categories_changed`: the reload offer appears, and "Keep editing" keeps the draft.
- `config_busy`: retried, then "Try again".
- `config_changed`: checked again.
- A validation error shows inline and disables Apply. A delayed answer for an older
  revision does not re-enable it.
- A reclassifying change shows the OpenRouter warning in the confirmation.
- Remove asks first; nothing is written until Apply.
- Closing with unsaved edits asks; closing during a command waits.
- Refile panel: a retired folder's move uses `--folder <native>`; dry-run shows the
  preview only; skipped counts are collapsed; the result texts use `marked` and
  `waiting_marked`.

**CLI additions** (in the `mailtriage` crate)

- **`service status`:**
  - without `--account`: every account, sorted, with the top-level `config` and the
    new fields;
  - `service_config` and `config_matches` with two configs that share an account
    name;
  - `enabled` and `enablement` for each launchd and systemd state, and for a failed
    query.
- **`service start` and `stop`** with the existing fake `launchctl`/`systemctl`:
  - call sequences for unloaded, loaded-running and loaded-idle jobs;
  - start that does not reach running (exit 3);
  - `already_*` results;
  - start when not installed (exit 2);
  - `service_config_mismatch` (exit 5);
  - `install` after `stop` runs `enable`.
- **Digest:**
  - fixed digest vectors checked into the tests: reordered input keys and
    `folder: null` versus absent give the same digest; reordered categories and a
    changed filing mode give different ones; Unicode names;
  - `apply --expect-digest` with a matching digest, a stale one (exit 5,
    `categories_changed`, nothing written), and filing turned off between export and
    apply (exit 5).
- **Error reasons:** `config_busy` (lock held by the test), `config_changed`
  (file rewritten during the command, through a test hook) and `binding_conflict`
  are each reported with their `reason`.
- **`changes`:** each kind; a rename with filing on keeps the folder; `reclassifies`
  matches whether `apply` advances `taxonomy_revision`.

**Autostart.** `enable`, `status` and `disable` against a temporary `HOME` with a
fake `launchctl`:

- the plist and `.desktop` contents, including the absolute `--config` and
  `--mailtriage`;
- `enable` clearing a disabled label;
- `status` false for a disabled label;
- a file without the marker (exit 5);
- `enable` without `--config` when `service status` fails (exit 3).

**Instances.**

- A second tray exits 0.
- A second window for the same config exits 0, while one for another config opens.
- The window's lock survives a tray restart.
- Exited children are reaped.

**Updates,** with the update spec's fake release server:

- a tray next to the CLI is updated;
- a tray whose `--version` fails is skipped without a download, and `update` exits 0;
- a bad tray checksum gives exit 4 with the CLI updated, and the tray keeps its old
  file;
- a later `watch` pass, after the CLI re-executed onto the current release, installs
  the still-older tray once its backoff has passed;
- the cache holds separate `installs` entries for the CLI and the tray after
  CLI-only success, full success, tray failure and skip.

**By hand** (added to `docs/verification.md`):

- macOS menu bar in light and dark mode, Ubuntu GNOME with the AppIndicator extension,
  and KDE: native start, the menu still working after hours idle, and start at login
  on each;
- VoiceOver (macOS) and Orca (Linux) for editing a category, a validation error, the
  apply confirmation, and cancelling a close;
- the design review of every screen against [UI principles](#ui-principles), with
  screenshots.

## Docs

- `guide.md`:
  - a **Tray** section: install, start at login, what the states mean, the
    categories window, Linux requirements;
  - the new CLI features: `service start` and `stop`, `service status` without
    `--account` and its new fields, error `reason`, `--expect-digest`, `digest` and
    `changes`.
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
- Sending category examples to the classifier.
