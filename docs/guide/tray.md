# Use the tray app

The tray app shows whether mailtriage is working and lets you manage services and categories without opening a terminal. It appears in the macOS menu bar or the Linux system tray.

## Install and open it {#install-the-tray}

The [install script](./install.md) includes the tray by default on macOS and on Linux desktops. Start it with:

```sh
mailtriage-tray
```

If you installed mailtriage elsewhere, keep the tray and command-line app from the same release in the same directory. See the [tray installation reference](../reference/tray.md#install-the-tray) for manual downloads and source builds.

Quitting the tray leaves background services running. Use **Stop service** in an account's menu to stop that account's checks.

## Start at login

Choose **Start at login** from the tray menu, or run:

```sh
mailtriage-tray autostart enable
```

To turn this off, uncheck the menu item or run `mailtriage-tray autostart disable`. Disabling login startup leaves the current tray running until you quit it.

## Read the status {#what-the-menu-shows}

Open the menu to see each account and its last check. Status refreshes every 15 seconds.

| Status | What to do |
| --- | --- |
| Running | Nothing; checks are working. |
| Starting or Restarting | Wait for the next check. |
| Some mail skipped | Open the log and run `mailtriage doctor --account work`. |
| Configuration changed | Wait; the next check uses the new configuration. |
| Another mailtriage was busy | Stop the other `sync` or `watch` for that account. |
| Stopped or Not installed | Choose **Start service** or **Install service** if you want automatic checks. |
| Problem or No check since | Open the log, then follow [troubleshooting](./troubleshooting.md). |
| Runs another config | The service belongs to a different configuration. See the [status reference](../reference/tray.md#what-the-menu-shows). |

## Edit categories {#the-categories-window}

Choose **Edit categories…** from an account's menu. You can also open the window directly:

```sh
mailtriage-tray categories --account work
```

1. Select a category or add one.
2. Give it a clear name and explain what belongs there.
3. Keep exactly one default category for mail that fits nowhere else.
4. Review the change summary and choose **Apply**.

The window checks your edits as you type. If a change requires classifying open mail again, it tells you before applying it. This uses your OpenRouter key and may take several passes.

**Revert** discards unsaved edits. If another window or agent changed the categories, reload them before trying again.

The **Move filed mail…** panel previews mail whose folder no longer matches its category. It can queue moves only when filing is live. Your own moves, corrections, pins and Done choices are respected.

## Linux requirements

You need GTK 3, Ayatana AppIndicator, libxdo, and a desktop with a StatusNotifier host. KDE Plasma works; GNOME needs the AppIndicator extension, included with Ubuntu. The categories window also needs OpenGL.

On Ubuntu, with GTK already provided by the desktop:

```sh
sudo apt-get install libayatana-appindicator3-1 libxdo3
```

## If the tray cannot connect {#when-the-tray-shows-a-problem}

- **Not set up:** run `mailtriage setup` in a terminal.
- **mailtriage not found:** put both executables in the same directory, or pass `--mailtriage PATH`.
- **Different versions:** run `mailtriage update`, or `brew upgrade mailtriage` for Homebrew.
- **Another error:** choose **Show details** and follow the command's error message.

The [tray reference](../reference/tray.md) lists every status, command, keyboard shortcut and login-item detail.

## Detailed reference

- <span id="what-the-tray-runs"></span>[What the tray runs](../reference/tray.md#what-the-tray-runs)
