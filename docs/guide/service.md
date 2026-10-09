# Run in the background

The background service checks for new mail without an open terminal. It uses launchd on macOS or a systemd user service on Linux, and restarts after an error.

## Start the service

If you accepted the service during setup, it is already installed. Otherwise:

```sh
mailtriage service install --account work
```

By default it checks every 60 seconds and processes up to 100 messages per pass. To change those limits, install it again:

```sh
mailtriage service install --account work --interval-seconds 120 --limit 50
```

Use a [key store or key command](./provider.md#the-openrouter-key) for your OpenRouter key. The service does not inherit variables exported in your terminal.

## Check that it is working {#status-of-every-account}

```sh
mailtriage service status --account work
```

Look for `running: true` and a recent `last_pass.finished_at`. A successful pass has `last_pass.exit_code: 0`; `4` means some work was skipped or failed.

Leave out `--account` to check every configured account. The [tray app](./tray.md) shows the same status in the menu bar or system tray.

## Stop and restart {#service-commands}

```sh
mailtriage service stop --account work
mailtriage service start --account work
```

Stop keeps the service stopped across logins and reboots. Start turns it back on. To remove the service entirely:

```sh
mailtriage service uninstall --account work
```

These commands do not remove your mailtriage configuration or stored mail. Do not run `sync` or another `watch` alongside a running service for the same account.

## Read the logs

On macOS, with the default configuration, logs are in `~/.config/mailtriage/logs/`: `work.log` for results and `work.err` for errors. The tray can open them for you.

On Linux:

```sh
journalctl --user -u mailtriage-work.service -e
```

If a check fails, start with [troubleshooting](./troubleshooting.md).

## Linux after logout

A systemd user service normally stops when you log out. To keep it running:

```sh
loginctl enable-linger "$USER"
```

## Moving the installation

Run `service install` again after moving the executable or configuration, or changing the shell's `PATH` needed by your key tools. Ordinary updates at the same path are picked up automatically. Homebrew upgrades are handled automatically too.

For service files, JSON fields and a custom supervisor, see the [service reference](../reference/service.md).

## Detailed reference

- <span id="your-own-supervisor"></span>[Your own supervisor](../reference/service.md#your-own-supervisor)
