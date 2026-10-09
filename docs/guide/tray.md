# Tray

`mailtriage-tray` shows each account's state in the menu bar (macOS) or the system tray (Linux). It starts, stops and installs the background service, opens the service log, and edits categories in a window. Every action is one documented command (listed below), so you or an agent can do the same in a terminal.

```text
mailtriage-tray [--config PATH] [--mailtriage PATH]
mailtriage-tray categories [--account NAME] [--config PATH] [--mailtriage PATH]
mailtriage-tray autostart enable|disable|status [--config PATH] [--mailtriage PATH] [--json]
mailtriage-tray quit [--json]
```

## What the tray runs

The tray uses the `mailtriage` next to its own executable, else the first on your `PATH`; `--mailtriage PATH` overrides both. For a Homebrew install it uses the `opt` path, `$(brew --prefix)/opt/mailtriage/bin/mailtriage`, which outlives `brew upgrade`. It uses the config that `mailtriage service status` finds (see [Where mailtriage finds the config](./configuration.md#where-mailtriage-finds-the-config)); `--config PATH` overrides it. Both are resolved once at start and made absolute. A later change of `PATH`, the working directory or a symlink therefore never redirects a running tray; restart it to follow one.

Every command ends with `--config CONFIG`, the absolute path of the tray's config:

| Action | Command |
| --- | --- |
| Refresh: every 15 s, after each action, and "Refresh now" | `mailtriage service status --json --config CONFIG` |
| Start service | `mailtriage service start --account NAME --json --config CONFIG` |
| Stop service | `mailtriage service stop --account NAME --json --config CONFIG` |
| Install service | `mailtriage service install --account NAME --json --config CONFIG` |
| Edit categories… | `mailtriage-tray categories --account NAME --config CONFIG --mailtriage CLI` |
| Load the window (open, Reload, another account) | `mailtriage service status --json --config CONFIG`, then `mailtriage categories export --account NAME --json --config CONFIG` |
| Check the window's edits | `mailtriage categories validate --account NAME --file DRAFT --json --config CONFIG` |
| Apply | `mailtriage categories apply --account NAME --file DRAFT --expect-digest DIGEST --json --config CONFIG` |
| Move filed mail… | `mailtriage filing refile --account NAME --json --config CONFIG` |
| Move (a folder no longer used) | `mailtriage filing refile --account NAME --folder NATIVE --apply --json --config CONFIG` |
| Move all | `mailtriage filing refile --account NAME --apply --json --config CONFIG` |
| Start at login | `mailtriage-tray autostart enable` or `disable` with the tray's `--config` and `--mailtriage` (the tray runs the same code itself) |

- `CLI` is the absolute path of `mailtriage`. `DRAFT` is a file in the window's private temporary directory, `DIGEST` the `digest` of the last export, and `NATIVE` the folder's server name from the preview's `folders`.
- "Open log" opens one of the service's `log_paths` with `open` (macOS). "Copy log command" puts `journalctl --user -u mailtriage-NAME.service -e` on the clipboard (Linux).
- The tray also runs `mailtriage --version` to compare versions. A tray started without `--config` runs its first `service status --json` without it, and uses the `config` that command reports from then on.

## Install the tray

The [install script](./install.md#the-install-script) installs the tray next to `mailtriage`: by default on macOS, and on Linux with `--tray` or when `DISPLAY` or `WAYLAND_DISPLAY` is set.

Each release also carries `mailtriage-tray-vVERSION-macos-arm64.tar.gz`, `-linux-amd64.tar.gz` and `-linux-arm64.tar.gz`, each with a `.sha256` file and listed in `SHA256SUMS`. The archive holds `mailtriage-tray` and the license. Use the same release as `mailtriage`, verify the archive's checksum, and install the tray in the same directory as `mailtriage` (`~/.local/bin` in [Install](./install.md)). On Linux, set `PLATFORM` to `linux-amd64` or `linux-arm64` and use `sha256sum --check` in place of `shasum -a 256 --check`:

```sh
VERSION=0.1.0 PLATFORM=macos-arm64
gh release download "v$VERSION" --repo wir-drei-digital/mailtriage --pattern "mailtriage-tray-v$VERSION-$PLATFORM.tar.gz*" &&
  shasum -a 256 --check "mailtriage-tray-v$VERSION-$PLATFORM.tar.gz.sha256" &&
  tar -xzf "mailtriage-tray-v$VERSION-$PLATFORM.tar.gz" &&
  install -d ~/.local/bin &&
  install -m 0755 mailtriage-tray ~/.local/bin/mailtriage-tray
```

The macOS executable is unsigned and not notarized, like the CLI. From source, with a stable Rust toolchain (on Linux, install the build packages from [Linux requirements](#linux-requirements) first):

```sh
cargo build --release --locked -p mailtriage-tray &&
  install -d ~/.local/bin &&
  install -m 0755 target/release/mailtriage-tray ~/.local/bin/mailtriage-tray &&
  mailtriage-tray --version
```

From then on `mailtriage update`, and the background service with `updates: auto`, keep a `mailtriage-tray` next to the CLI current: they update it after the CLI, to the same release (see [The tray next to the CLI](./updates.md#the-tray-next-to-the-cli)). That needs both in a directory you own, as above. A root-owned copy, for example one installed with `sudo` into `/usr/local/bin`, is not replaced (see [Binaries mailtriage does not replace](./updates.md#binaries-mailtriage-does-not-replace)).

Start it:

```sh
mailtriage-tray
```

The icon appears in the menu bar (macOS, without a Dock icon) or the system tray (Linux), and stays until you choose Quit.

- One tray runs per user. A second start prints `mailtriage-tray is already running` and exits 0.
- `mailtriage-tray quit` quits the running tray of this installation, as its menu's Quit does, and prints `quit`, `not_running` (no tray runs) or `other_installation` (the running tray belongs to a `mailtriage-tray` elsewhere, which keeps running); with `--json`, `{"schema_version":1,"quit":"quit"}`. It talks to the tray over `tray.sock` next to `tray.lock` in the cache directory and signals no process. It exits 3 with `the tray does not respond; quit it from its menu` when a tray holds its lock but does not answer within 5 seconds. Open categories windows keep running.
- When `mailtriage-tray` is replaced on disk by a version that runs, the running tray restarts itself onto it within about 15 s, with the same config and `mailtriage`. An open categories window keeps running. After `brew upgrade`, a Homebrew tray restarts onto its `opt` path.

## Start at login

Choose "Start at login" in the menu, or:

```sh
mailtriage-tray autostart enable
mailtriage-tray autostart status --json
mailtriage-tray autostart disable
```

- `enable` writes a login item: `~/Library/LaunchAgents/digital.wirdrei.mailtriage-tray.plist` on macOS, `$XDG_CONFIG_HOME/autostart/mailtriage-tray.desktop` on Linux (`~/.config/autostart/mailtriage-tray.desktop` when `XDG_CONFIG_HOME` is unset or not absolute).
- The login item records the tray's absolute path with `--config` and `--mailtriage`, both absolute and resolved as above. Without `--config`, `enable` runs `mailtriage service status --json` once to learn the config. On macOS it also records your current `PATH`, as `service install` does. Run `enable` again after you move `mailtriage`, the tray or the config. For a Homebrew install it records the `opt` paths of both, which survive `brew upgrade`.
- macOS: the tray starts at login and restarts after a crash, but Quit stays quit until the next login (`RunAtLoad`, `KeepAlive` with `SuccessfulExit` false). `enable` also runs `launchctl enable gui/<uid>/digital.wirdrei.mailtriage-tray`. It does not start a second tray.
- `disable` deletes the file and leaves the running tray alone.
- `status` reports `enabled: true` when the file exists and, on macOS, launchd has not disabled the label.
- mailtriage marks the files it writes and changes only marked files.

Each prints `{"schema_version":1,"autostart":{"enabled":true,"path":"…","config":"…","mailtriage":"…"}}`: one line with `--json`, pretty-printed without it.

| Code | Meaning |
| --- | --- |
| 0 | Success. |
| 2 | Invalid arguments. |
| 3 | The file cannot be written or removed, `launchctl enable` failed, or the config or `mailtriage` cannot be found (pass `--config` or `--mailtriage`). |
| 5 | A file at the path was not written by mailtriage: `PATH exists and was not written by mailtriage; move it away first`. It is left alone. |

## What the menu shows

```text
mailtriage: daniel needs attention
──────
daniel (daniel@example.com)  ▲ Some mail skipped  ▸  Running · filing live
                                                      Last check 12:03, some mail skipped · v0.3.0
                                                      ──────
                                                      Stop service
                                                      Open log
                                                      Edit categories…
info  ○ Not installed                             ▸  Not installed · filing off
                                                      ──────
                                                      Install service
                                                      Edit categories…
──────
Refresh now
Start at login
Quit
```

Each account shows its name, its identity when that differs, a mark (● running, ○ off, ▲ needs attention) and its state:

| State | What it means | What to do |
| --- | --- | --- |
| Running · checked HH:MM | The service runs and its last check was complete and recent. | Nothing. |
| Starting… | The service runs and has not finished its first check yet. | Wait one interval. |
| Restarting… | The service should run but has stopped, for less than 2 minutes so far. launchd and systemd restart it after an error. | Wait. After 2 minutes it shows "Problem since". |
| Some mail skipped | The last check was partial: some folders or messages failed, or the OpenRouter key was unavailable. | Open the log and run `mailtriage doctor --account NAME`. `mailtriage list --account NAME --view all --json` shows each message's `error`. |
| Configuration changed | The last check stopped because `mailtriage.json` changed while it ran. | Nothing: the next check uses the new file. |
| Another mailtriage was busy | Another worker for this account, such as a second `watch` or a `sync`, was running when the check started. | Run one worker per account: stop the other one. |
| No check since HH:MM | The service runs, but its last check finished more than 3 intervals plus 30 minutes ago. It may hang. | Open the log, then Stop service and Start service. |
| No check yet | The service has run that long without finishing a check. | As for "No check since". |
| Problem since HH:MM | The service should run but has not run for 2 minutes or more, or its last check failed. The submenu says which. | Open the log ("Open log" and "Open error log" on macOS; on Linux, "Copy log command" and run it in a terminal), and run `mailtriage doctor --account NAME`. |
| Stopped | The service is installed but stopped, and does not start at login, for example after Stop service. | Start service, or `mailtriage service start --account NAME --config CONFIG`. |
| Not installed | The account has no background service. | Install service, or `mailtriage service install --account NAME --config CONFIG`. |
| Runs another config | The account's service belongs to another `mailtriage.json`; the submenu names it. Start and Stop are not offered: they would act on that config's service. | Open the tray with that `--config`, or move the service to this config with `mailtriage service install --account NAME --config CONFIG`. |
| Status unknown | The tray cannot tell which config the service runs (`launchctl` or `systemctl` failed, or systemd needs a reload), or, for a stopped service, whether it starts at login. The submenu says which. | Wait for the next refresh. If it stays, run `mailtriage service status --account NAME --json --config CONFIG`; `mailtriage service install --account NAME --config CONFIG` rewrites the service for this config. |
| No background service on this system | Neither launchd nor systemd is available. | Run `watch` under [your own supervisor](./service.md#your-own-supervisor). "Edit categories…" still works. |

- **The submenu** shows whether the job runs and the filing mode, then the last check, for example "Last check 12:03, some mail skipped · v0.3.0" (the version of mailtriage that ran it). A job that runs although its manager would not start it again adds "Disabled: won't start again after logout" and keeps "Stop service". While a service command runs, its item reads "Starting…", "Stopping…" or "Installing…".
- **The summary line** reads "All accounts running", "NAME needs attention", "N of M accounts running", "Stopped", "No background service on this system", or a problem from [When the tray shows a problem](#when-the-tray-shows-a-problem). When no account runs for this config but some run another one, it says so instead of "Stopped": "Runs another config" (the only account), "Every account runs another config", "NAME runs another config" or "N accounts run another config". It is also the icon's tooltip.
- **The icon** shows the worst state across accounts: an envelope with a `!` badge for a warning, a problem or "Status unknown"; else a plain envelope when a service runs, starts or restarts; else an outline (every account stopped, not installed, running another config, or without a service manager). On macOS it follows the menu bar's light or dark look, so a warning and a problem look the same. Linux uses the same shapes in green, amber, red and grey.
- **Times** are local `HH:MM`, with the date when not today ("Oct 5, 23:59").
- **"(stale)"** after the summary: the last refresh failed. The menu keeps the last good state for up to 2 minutes, then shows the failure.
- **"mailtriage X and tray Y differ; run mailtriage update"** appears when the CLI's version differs from the tray's. `mailtriage update` brings a CLI and the tray next to it to the same release; otherwise install both from the same release.
- **Notices.** A failed action, or a line from the categories window such as "the categories window is already open", shows under the summary for 60 s, with "Show details" when there is more.

## The categories window

Open it with "Edit categories…" in an account's submenu, or:

```sh
mailtriage-tray categories --account work
```

Without `--account` it shows the first account. One window runs per config. A second start for the same config prints `the categories window is already open` and exits 0; on macOS it first brings the open window to the front. The tray shows that line as a notice. `categories` exits 0 when the window is closed or already open, 2 for invalid arguments, and 3 when `mailtriage` cannot be found or the first load fails, after the window has shown why.

The window shows the account selector and Reload at the top, the categories on the left (with their mail folder; the default one is marked "default") and the selected category on the right: Name, "What belongs here" (the description), "Use for mail that fits nowhere else" (the default category; exactly one), Mail folder (only when the account's filing is not `off`), Advanced, and "Remove category". The footer shows the check result on the left and "Move filed mail…", Revert and Apply on the right.

- **Checks run as you type.** Half a second after your last change, the window writes the categories to a private temporary file and runs `categories validate`. The footer shows the changes in words, for example "1 added, 1 renamed, mail folder changed for Newsletters", or what is wrong, with "Show details".
- **Apply** is enabled when the latest check of your current edits passed and they differ from what was loaded; otherwise its tooltip says why. Apply (or ⌘/Ctrl+S) lists the changes and asks first. When the change sorts all open mail again, it adds: "All open mail in this account will be sorted again. This uses your OpenRouter key and takes a few passes." After a successful apply, the footer says "Saved." and what happens next, and the window loads the categories again.
- **"These categories were changed somewhere else."** Another program, such as another window, an agent or `categories apply`, changed this account's categories since the window loaded them, or filing was turned on or off. Apply passes `--expect-digest`, so nothing was written. Choose "Reload (discard my edits)" or "Keep editing".
- **"mailtriage is busy; trying again…"** Another command is editing `mailtriage.json`. The window retries three times, 2 s apart, then shows "Try again".
- **"The configuration changed while saving. Checking your changes again."** `mailtriage.json` changed while Apply ran; your edits are checked again.
- **Removing a category** asks first and says what happens: its mail stays in its folder until you move it, and new mail is sorted into the remaining categories. Nothing is written until Apply. The default category cannot be removed; choose another default first.
- **Mail folder.** Left empty, the folder is the category's name. The field always shows the folder mail will go to.
- **Advanced.** A new category's ID is suggested from its name and can be changed until you apply. Existing IDs cannot change: corrections and folders refer to them. Examples, one per line, are saved with the category and not sent to the classifier.
- **Revert** drops your unsaved edits without asking. Switching accounts, Reload and closing the window ask first ("Discard changes?").
- **Feedback.** Every command shows a spinner and a verb ("Loading…", "Checking…", "Applying…", "Moving…"). While a load, Apply or a move runs, the form is read-only. Closing during Apply or a move shows "Finishing…" and closes when it ends. The draft files are removed when the window closes.

**Moving filed mail.** "Move filed mail…" (shown when the account's filing is not `off`) opens a panel below the form. It also opens after an apply that added, removed or re-pointed a category, or sorts mail again. It mirrors `mailtriage filing refile`:

- each folder no longer used, with how many messages can move now and how many may move once they are sorted again, and "Move" (which also marks the waiting ones);
- "Move all N" for every message whose folder no longer matches its category, including those in folders no longer used;
- how many messages are still being sorted again, and "Not moved" with each reason and its count.

After a move, the panel says how many messages were marked, for example "Marked 30 messages; 12 more will move if their new category calls for it." The background service moves them at its next pass; when it does not run, the panel says to start it or run `mailtriage sync`. With filing in `dry_run`, the panel only previews and says "Moving filed mail needs filing set to live." "Close" in the panel's heading row hides the panel and gives the form its room back; while a move runs it is disabled, and its tooltip says why. "Move filed mail…" opens the panel again with a fresh preview.

**Keyboard.** ⌘N (Ctrl+N on Linux) adds a category, ⌘S (Ctrl+S) opens the apply confirmation, Esc closes a dialog, and ⌘W (Ctrl+W) closes the window, asking about unsaved edits. Tab follows the visual order. Every control has a label that screen readers announce.

## Linux requirements

- GTK 3, the Ayatana AppIndicator library and libxdo, as `tray-icon` documents. On Ubuntu: `sudo apt-get install libayatana-appindicator3-1 libxdo3`; GTK 3 comes with the desktop. Building from source also needs `libgtk-3-dev libayatana-appindicator3-dev libxdo-dev`.
- A desktop with a StatusNotifier host: KDE Plasma, or GNOME with the AppIndicator extension, which Ubuntu ships.
- OpenGL for the categories window.

## When the tray shows a problem

The summary line, and the categories window, show these:

| Message | What to do |
| --- | --- |
| mailtriage not found (looked in …) | Install `mailtriage` in the same directory as `mailtriage-tray` or on your `PATH`, or start the tray with `--mailtriage PATH`. |
| Not set up. Run mailtriage setup in a terminal. | No config was found. Run `mailtriage setup`; the tray finds the config at its next refresh. The tray never runs setup itself. |
| Not set up: no config at PATH. Run mailtriage setup --config PATH in a terminal. | `--config` names a file that does not exist. That command creates it there (plain `mailtriage setup` would write the default config instead); the tray finds it at its next refresh. |
| mailtriage is older than the tray; run mailtriage update. | The CLI lacks a field or a command this tray needs; "Show details" names the missing field, or shows the CLI's own complaint about the tray's arguments. Run `mailtriage update`, or install `mailtriage` from the tray's release. |
| Any other message | The failed command's error. In the menu, "Show details" copies the command, its exit code and its output; in the window, it shows them with "Copy". Run the command in a terminal to see more. |
