# Background service reference

For a walkthrough, see [background service](../guide/service.md). Use this page to look up flags, output fields and detailed behavior.

Let mailtriage check your mail in the background, so you don't need a terminal open for it. The service runs `watch` for you and starts it again after an error.

```sh
mailtriage service install --account work
mailtriage service status --account work --json
mailtriage service uninstall --account work
```

`service install` writes a launchd agent (macOS) or a systemd user unit (Linux) and starts it. It runs `watch` for the account with absolute paths, for example:

```text
/usr/local/bin/mailtriage watch --config /Users/alice/.config/mailtriage/mailtriage.json --account work --interval-seconds 60 --limit 100 --json
```

`--interval-seconds` (1 to 86400, default 60) and `--limit` (1 to 500, default 100) are passed to `watch`.

The executable is the one that ran `service install`. For a Homebrew install it is its `opt` path, `$(brew --prefix)/opt/mailtriage/bin/mailtriage`, which `brew upgrade` keeps pointing at the current version. `mailtriage setup` runs the same install in its last step.

| | macOS (launchd) | Linux (systemd user unit) |
| --- | --- | --- |
| File | `~/Library/LaunchAgents/digital.wirdrei.mailtriage.<account>.plist` | `~/.config/systemd/user/mailtriage-<account>.service` |
| Logs | `<config dir>/logs/<account>.log` (stdout, one JSON line per pass) and `<account>.err` (stderr); the directory has mode 0700 | journald: `journalctl --user -u mailtriage-<account>.service` |
| `install` runs | `launchctl bootout gui/<uid>/<label>` if loaded, then `launchctl bootstrap gui/<uid> <plist>` | `systemctl --user daemon-reload`, `enable <unit>`, `restart <unit>` |
| `uninstall` runs | `launchctl bootout gui/<uid>/<label>`, then deletes the file | `systemctl --user disable --now <unit>`, deletes the file, `daemon-reload` |
| Restart | After an error exit, at most every 30 seconds (`KeepAlive` with `SuccessfulExit` false, `ThrottleInterval` 30) | `Restart=on-failure`, `RestartSec=30` |
| File mode of new files | `Umask` 077 | `UMask=0077` |

`<label>` is `digital.wirdrei.mailtriage.<account>`. With the home config, the log directory is `~/.config/mailtriage/logs`.

- **Repeating `install`**: it rewrites the file and reloads the job. Run it again after you move the executable or the config, or change your `PATH`.
- **A replaced binary**: a running service switches to a new binary at the same path by itself, between passes; see [How a running service switches](./updates.md#how-a-running-service-switches). Moving the binary to another path still needs `service install`.
- **Marked files**: mailtriage marks the files it writes. It replaces or removes only marked files. An unmarked file at the path exits 5 with `PATH exists and was not written by mailtriage; move it away first`.
- **Accounts**: `install` and `status` need the account to be in the config (exit 2, `unknown account`). `uninstall` also works for an account you have removed.
- **PATH**: launchd and systemd start jobs with a short `PATH`. The service file therefore records the `PATH` of the shell that runs `install`, so key tools such as `pass` and `gpg` find their helpers.
- **Key from an environment variable**: the service does not inherit your shell's variables. When the key comes from `api_key_env`, `install` adds a `note` to its result with the command that moves the key into this platform's store, for example `mailtriage setup --update --config /Users/alice/.config/mailtriage/mailtriage.json --account work --key-store keychain` (on Linux `secret-service` or `pass`; `command` everywhere). Do not put the key into the plist or unit yourself: those files are readable, and `service install` rewrites them. Without a key, each pass classifies nothing and is partial (see [The OpenRouter key](./provider.md#the-openrouter-key)).
- **Linux and logout**: a user unit stops when you log out, unless lingering is on: `loginctl enable-linger $USER`. Setup prints this hint; it does not run the command.
- **Other platforms**: `install` and `uninstall` exit 2 (`unsupported platform`); `status` reports `manager: "none"`. On Linux without `systemctl`, `install` exits 3 and `status` reports `manager: "none"`.

## Service commands

Each command prints `{"schema_version":1,"service":{...}}`.

`install` reports `action` (`installed`), `manager` (`launchd` or `systemd`), `account`, `unit_path`, `log_paths` (empty for systemd), `command` (the `watch` command line as a list) and, when the key comes from an environment variable, `note`.

`uninstall` reports `action` (`uninstalled`, or `not_installed` when no file was there), `manager`, `account` and `unit_path`.

```sh
mailtriage service stop --account work --json
mailtriage service start --account work --json
```

`start` starts the installed service, which then also starts at login again.

- launchd: `launchctl enable gui/<uid>/<label>`, then `launchctl bootstrap gui/<uid> <plist>` when the job is not loaded, or `launchctl kickstart gui/<uid>/<label>` when it is loaded but idle.
- systemd: `systemctl --user enable --now <unit>`.

It then checks for up to 5 s that the job runs (`launchctl print` shows a PID; `systemctl --user is-active` prints `active`), else it exits 3 with `service did not start; see LOG`. LOG is `<config dir>/logs/<account>.err` on macOS and `journalctl --user -u mailtriage-<account>.service` on Linux.

`start` reports `action` (`started`, or `already_running` when the job was running), `manager`, `account` and `unit_path`. A service that is not installed exits 2 with `service for account A is not installed; run mailtriage service install --account A`.

`stop` stops the service and keeps it stopped across logins and reboots; the service file stays.

- launchd: `launchctl bootout gui/<uid>/<label>`, waiting until launchd drops the job as `install` does, then `launchctl disable gui/<uid>/<label>`.
- systemd: `systemctl --user disable --now <unit>`. A unit that is also enabled globally or masked is outside this guarantee, and `status` shows it in `enablement`.

`stop` reports `action` (`stopped` when launchd had the job loaded or systemd had it running, else `already_stopped`), `manager`, `account` and `unit_path`. A service that is not installed exits 2 with the same message as for `start`.

`start` and `stop` act only on a service that runs the config they read, and call no manager command otherwise:

- When the service runs another config (`config_matches: false`), they exit 5 with reason `service_config_mismatch`: `service for account A runs config X; pass --config X`, the second X shell-quoted. Pass that `--config` to act on that service.
- When its config cannot be told (`config_matches: null`, for example when `launchctl` or `systemctl` fails or systemd needs a daemon reload), they exit 5 with reason `service_config_unknown`: `cannot tell which config the service for account A runs; try again, or reinstall it with mailtriage service install --account A`.

`install` keeps its meaning: it rewrites the service for the config it read. On macOS it now runs `launchctl enable` before `bootstrap`, so it works after a `stop`; systemd's `enable` already covers this.

Like `install`, `start` and `stop` exit 2 for an unknown account or an unsupported platform, and 3 when `launchctl` or `systemctl` fails.

`install`, `uninstall`, `start` and `stop` hold a lock per account while they read the config and call the manager: `service-<label>.lock` in `~/Library/Caches/mailtriage` on macOS and `~/.cache/mailtriage` on Linux.

The directory comes from `HOME` only, never from `XDG_CACHE_HOME`, so commands for one account from different configs or environments wait for each other. A second command waits up to 30 s, then exits 5 with reason `service_busy`: `another service command for account A is running; try again`. Step 10 of `mailtriage setup` takes the same lock around its install.

`status` reports:

```json
{"schema_version":1,"service":{"account":"work","installed":true,"last_exit_status":0,"last_pass":{"exit_code":0,"finished_at":"2026-10-05T08:00:00.000000+00:00","mode":"dry_run","partial":false,"reason":null,"version":"0.2.0"},"loaded":true,"log_paths":["/Users/alice/.config/mailtriage/logs/work.log","/Users/alice/.config/mailtriage/logs/work.err"],"manager":"launchd","pid":4242,"running":true,"unit_path":"/Users/alice/Library/LaunchAgents/digital.wirdrei.mailtriage.work.plist","update":{"mode":"auto","executable":"/Users/alice/.local/bin/mailtriage","installed":"0.2.0","latest":"0.3.0","available":true,"checked_at":"2026-11-03T08:12:40Z","last_error":null,"replaceable":true,"reason":null}}}
```

| Field | Content |
| --- | --- |
| `manager` | `launchd`, `systemd`, or `none` on other platforms. |
| `installed` | A file written by mailtriage is at `unit_path`. |
| `loaded` | The manager knows the job. |
| `running` | The job is running now. |
| `pid` | Its process ID, or `null`. |
| `last_exit_status` | The last exit status the manager reports, or `null`. |
| `unit_path`, `log_paths` | The service file and the launchd log files (`log_paths` is empty for systemd). |
| `last_pass` | The account's latest `sync` or `watch` pass, from the state database: `finished_at`, `partial`, `exit_code` (0, 4 for partial, or the error's exit code), `mode` (`off`, `dry_run` or `live`), `version`, the mailtriage version that ran it (`null` for passes recorded before schema 7), and `reason`, the error's machine-readable reason (see [Output and exit codes](../guide/reference.md#output-and-exit-codes)), `null` for a pass without one and for a pass recorded before schema 8 or by an older release, so it never names an earlier pass's error. `null` before the first pass. |
| `update` | The binary the service runs, and whether it is current: `mode` (the config's `updates`); `executable`, decoded from the service file (`null` without one, and then `installed` and `replaceable` describe the binary that runs `service status`); `installed`, its `--version` (`null` when it does not run); `latest`, `available` and `checked_at` from the last release check (`null`, `false` and `null` before the first one); `last_error`, the last failed install of that binary, else the last failed check (`{at, message}` or `null`); `replaceable` and `reason` (see [Binaries mailtriage does not replace](./updates.md#binaries-mailtriage-does-not-replace)); `tray` when a `mailtriage-tray` sits next to that binary (see [The tray next to the CLI](./updates.md#the-tray-next-to-the-cli)). It reads the update cache and makes no network call. |

`last_pass` comes from the state database and works without any service. Every `sync` and every `watch` pass records it, including one that finds another worker holding the account lock (exit 5, reason `account_busy`).

A healthy service shows `running: true` and a `last_pass.finished_at` no older than a few intervals.

## Status of every account

```sh
mailtriage service status --json
```

Without `--account`, `service status` reports every account of the config, one object per account, sorted by account name:

```json
{"schema_version":1,"config":"/Users/alice/.config/mailtriage/mailtriage.json","services":[{"account":"personal",…},{"account":"work",…}]}
```

`config` is the config that `service status` read, as a canonical absolute path. With `--account`, the result has the same `config` and a single `service` object.

Every service object has the fields above and these:

| Field | Content |
| --- | --- |
| `service_config` | The `--config` the service runs with. launchd: from `launchctl print`. systemd: from the running process (`/proc/<pid>/cmdline`), else from the `ExecStart` systemd has loaded. A job the manager has not loaded: from the service file. `null` when not installed, or when it cannot be read. |
| `file_config` | The `--config` in the service file, or `null`. |
| `needs_daemon_reload` | systemd only: `true` when the unit file changed after systemd loaded it (`NeedDaemonReload`), else `false`; `null` when unknown or not installed. |
| `config_matches` | `true` when the service runs this config. `false` when it, or its file, names another config. `null` when that cannot be established, for example when `launchctl` or `systemctl` fails or systemd needs a reload. `true` when not installed. |
| `enabled` | Whether the manager starts the service again, for example at login: `true`, `false`, or `null` when unknown. |
| `enablement` | The state behind `enabled`. launchd: `enabled`, `disabled`, or `unknown` when `launchctl print-disabled` fails. systemd: what `systemctl --user is-enabled <unit>` prints; `enabled` and `enabled-runtime` count as enabled, `disabled`, `masked` and `masked-runtime` as not, anything else (such as `static`) as unknown. `not_installed` when not installed. |
| `interval_seconds` | The `--interval-seconds` in the service file, or `null`. |
| `filing_mode` | The account's configured filing mode: `off`, `dry_run` or `live`. |
| `identity` | The account's `identity` from the config. |

Two configs can name the same account. The service of an account belongs to one config at a time: `service install` from the other config rewrites it.

`config_matches: false` shows that the service runs the other config; `service_config` names it.

## Your own supervisor

On a server you may prefer a system-wide unit that runs as a dedicated user.

First give the config a key command: set `provider.api_key_command` to a command that prints the key for that user, for example from `pass` or another key store (see [The OpenRouter key](./provider.md#the-openrouter-key)). The unit then holds no secret.

A systemd example, in `/etc/systemd/system/mailtriage-work.service`:

```ini
[Unit]
Description=mailtriage watch for account work
Wants=network-online.target
After=network-online.target

[Service]
User=mailtriage
ExecStart=/usr/local/bin/mailtriage watch --config /etc/mailtriage/mailtriage.json --account work --json
Restart=on-failure
RestartSec=30

[Install]
WantedBy=multi-user.target
```

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now mailtriage-work.service
```

- The key command runs as the user named in `User=`, with no terminal. That user's key store must hold the key and release it without a prompt.
- If the config uses `api_key_env` instead, the variable must reach the mailtriage process; mailtriage does not read it from a file. Do not put the key into a unit or plist with `Environment=` or `EnvironmentVariables`: those files are readable, and `mailtriage service install` rewrites the ones it manages. Keep the key in a key store and give the config a key command instead.
- `mailtriage service` does not manage this unit. `service status` reports it as not installed, but its `last_pass` still shows the latest pass.
- The user named in `User=` needs write access to `state_dir` and to the directory that holds `mailtriage.json`. mailtriage creates `mailtriage.lock` there and rewrites the file for `filing enable`, `filing disable` and `categories apply`.
- Supervisors start jobs with a short default `PATH` (launchd: `/usr/bin:/bin:/usr/sbin:/sbin`), which is why `engine.binary` should be an absolute path.
