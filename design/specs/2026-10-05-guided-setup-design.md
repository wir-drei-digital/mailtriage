# Guided setup, key command and background service

Date: 2026-10-05
Status: Implemented.
Builds on: [IMAP category filing](2026-10-04-imap-category-filing-design.md).

## Goal

A new user goes from nothing to a working, `doctor`-ready mailtriage with one
command, without ever writing the OpenRouter key into a file, and can leave
`watch` running as a background service. Agents get the same setup through
flags, with no prompts.

## Decisions

| Topic | Decision |
| --- | --- |
| Mail account | Reuse Himalaya's own wizard (`himalaya configure`); mailtriage never writes IMAP settings itself. |
| Key storage | A key command (`provider.api_key_command`). Setup offers five options, the platform's own store first: macOS Keychain, Linux Secret Service, `pass`, a custom command, or an environment variable only. |
| Background service | Optional last step of setup: a launchd agent (macOS) or systemd user unit (Linux) running `watch`, with `service install`, `uninstall` and `status`. |
| Config location | `~/.config/mailtriage/mailtriage.json` by default, state next to it. |
| Prompt style | Plain numbered prompts on stderr using only the standard library, and a flag for every answer. No new dependencies. |

## Config location

All commands resolve the config path in this order:

1. `--config PATH`;
2. the `MAILTRIAGE_CONFIG` environment variable;
3. `./mailtriage.json`, if that file exists (existing setups keep working);
4. `~/.config/mailtriage/mailtriage.json`.

`init` without `--config` still writes `./mailtriage.json` and is unchanged
otherwise. `setup` without `--config` uses `~/.config/mailtriage/mailtriage.json`
and sets `state_dir` to `state` (resolved next to the config, as today). `HOME`
must be set for step 4; if it is not, the error names `--config`.

## `mailtriage setup`

### Steps

1. **Config.** If the target config exists, ask: update an account, add an
   account, or abort. Without prompts, `--update` is required to modify an
   existing config; otherwise exit 5. Other accounts and their categories are
   never changed.
2. **Himalaya.**
   - Binary: `--himalaya-binary`, else the first `himalaya` on `PATH`; stored as
     an absolute path. Must report v2.1.0 with `+imap`, else exit 3.
   - Config: `--himalaya-config`, else `HIMALAYA_CONFIG`, else Himalaya's
     default path. Without `--config` and `HIMALAYA_CONFIG`, Himalaya v2.1.0
     uses the first existing file of `<platform config dir>/himalaya/config.toml`,
     `~/.config/himalaya/config.toml` and `~/.himalayarc`. The platform config
     dir is `~/Library/Application Support` on macOS (`XDG_CONFIG_HOME` is
     ignored there), and on Linux `$XDG_CONFIG_HOME` when that is an absolute
     path, else `~/.config`. `HIMALAYA_CONFIG` may hold several paths separated
     by `:`; setup passes exactly one `--config`, so it refuses several paths
     with exit 2 and asks for `--himalaya-config`.
   - Account: list with `himalaya account list`. With `--json` it prints
     `{"accounts":[{"name":"home","default":false,"backends":["imap"]},…]}`,
     sorted by name; an account without a backend block has `"backends":[]`.
     The account's email is only in the TOML (`[accounts.<name>] email`), not
     in this list. Choose one, or "create one", which runs `himalaya configure`
     attached to the terminal. Without a terminal, `--himalaya-account` is
     required (exit 2 naming the flag) and `configure` is never run.
   - Validate with `himalaya account check`; failure exits 3 and shows
     Himalaya's command to fix it, never its output. With `--json`, `account
     check` prints `{"account":"A","backends":[{"backend":"imap","ok":false,"error":"…"}]}`
     and exits 0 even when the check fails, so setup reads `ok`. An unknown
     account exits 1.
3. **Account details.**
   - mailtriage account name: `--account`, default the Himalaya account name.
     Must be ASCII letters, digits, `-` or `_` (it is used in service and file
     names).
   - Identity: `--identity`, default the Himalaya account's `email`.
   - Time zone: `--timezone`, default `TZ`, then the zone `/etc/localtime`
     points to, then `UTC`.
   - Brief: `--brief`, optional, one line about the recipient that helps
     classification.
4. **Folders to watch.** List the account's folders (through the Himalaya
   engine's `list_folders`) and choose by number; default `INBOX`.
   `--mailbox NAME` (repeatable) without prompts. Names must pass the existing
   source-folder rules when filing is on.
5. **Classifier.** `--provider openrouter|fake` (default `openrouter`);
   `--model` (default `typesafe/jev-1.13`); key storage per [Key
   command](#key-command).
6. **Categories.** Keep the six defaults with `folder` set explicitly to each
   name. Setup prints how to change them later (`categories export`, edit,
   `categories validate --account`, `categories apply`).
7. **Filing.** `--filing off|dry-run` (default `dry-run`). Setup never selects
   `live`; the summary points to `filing enable --mode live` and the provider
   checklist.
8. **Write.** Atomic write with 0600 permissions; the state directory is
   created with 0700.
9. **Check.** Run `doctor` for the account and print each readiness item with,
   for anything not ready, the one command that fixes it.
10. **Service.** Offer to install the background service (default yes when
    prompting; no when the key comes from an environment variable, which the
    service does not inherit). Without prompts: `--service install|skip`
    (default `skip`), `--interval-seconds`, `--limit`. Not installed when the
    check reported a not-ready `state` item.

### Prompts and output

- Prompts and progress go to stderr; with `--json`, stdout carries exactly one
  result object: config path, account, mailboxes, provider, key source,
  filing mode, doctor summary, service result.
- Each prompt shows its default in brackets; Enter accepts it. Menus are
  numbered. Invalid input re-asks; end of input on stdin aborts with exit 2.
- `--yes` disables all prompts: missing values take their defaults, and values
  without a safe default fail with exit 2 naming the flag.
- Prompts are used only when stdin is a terminal or `--interactive` is given
  (tests use `--interactive` with piped stdin). Otherwise setup behaves as with
  `--yes`.
- Setup makes no changes on the IMAP server.

### Exit codes

| Code | Cases |
| --- | --- |
| 0 | Setup completed (doctor may still report items not ready; they are listed). |
| 2 | Invalid input, a missing required flag without prompts, invalid account name, unsupported platform for the requested service. |
| 3 | Himalaya missing or not v2.1.0, `account check` failed, a key tool failed, `launchctl`/`systemctl` failed. |
| 5 | Config exists without `--update`, the account's stored binding would change (checked before writing; nothing is written), or a service file exists that mailtriage did not write. |

Every error names the step and the flag or command that fixes it.

## Key command

### Configuration

`provider` gains an optional `api_key_command`, an argument list:

```json
"provider": {
  "kind": "openrouter",
  "model": "typesafe/jev-1.13",
  "endpoint": "https://openrouter.ai/api/alpha/decisions",
  "api_key_command": ["/usr/bin/security", "find-generic-password", "-s", "mailtriage", "-a", "openrouter", "-w"],
  "api_key_env": "OPENROUTER_API_KEY",
  "timeout_seconds": 30
}
```

- `api_key_command` is serialized only when set, so existing configs and the
  classification generation hash are unchanged.
- The classification generation hash removes `api_key_command` from the
  provider value it hashes, so setting or changing a key command never
  reclassifies mail. `api_key_env` stays in the hashed value as today, so
  existing hashes do not change.
- Validation: for `openrouter`, at least one of `api_key_command` and
  `api_key_env` is set. When set, `api_key_command` is a nonempty list whose
  first element is nonempty. `api_key_env` may then be empty.

### Resolution

If `api_key_command` is set it is the only source; `api_key_env` is ignored
even if the variable exists. Otherwise the key comes from `api_key_env` as
today.

### Running the command

- Run directly (no shell; the custom option stores an explicit
  `["/bin/sh", "-c", "<text>"]`), stdin closed, stderr discarded, a 10-second
  timeout, at most 4 KB of stdout.
- It must exit 0. The key is the first line of stdout with surrounding
  whitespace trimmed; empty means failure.
- Resolved at most once per process (cached in memory); `watch` reopens the
  service each pass, so a rotated key is used on the next pass.
- The key never appears in logs, JSON output or errors. Errors are exactly:
  `API key command failed (exit N)`, `API key command timed out`,
  `API key command printed no key`, `API key command could not start`.
- `doctor` reports `provider.key_source` (`command` or `env`) and
  `provider.key_present`; for `command` it runs the command. On macOS the first
  run may show a Keychain access dialog.

### Options offered by setup

The provider is shared by all accounts, so there is one key per config. Tool
paths are resolved to absolute paths when stored.

| Option (`--key-store`) | Offered when | Store step (the tool prompts for the key) | Stored `api_key_command` |
| --- | --- | --- | --- |
| `keychain` (default on macOS) | macOS | `security add-generic-password -U -s mailtriage -a openrouter -w` | `security find-generic-password -s mailtriage -a openrouter -w` |
| `secret-service` (default on Linux when available) | `secret-tool` on `PATH` | `secret-tool store --label="mailtriage OpenRouter key" service mailtriage provider openrouter` | `secret-tool lookup service mailtriage provider openrouter` |
| `pass` | `pass` on `PATH` | `pass insert mailtriage/openrouter` | `pass show mailtriage/openrouter` |
| `command` | always | none; the user types a command (`--key-command`) | `["/bin/sh", "-c", "<text>"]` |
| `env` | always | none; the user names the variable (`--key-env`, default `OPENROUTER_API_KEY`) | none; sets `api_key_env` |

After the choice, setup runs the read command once and confirms a key was
printed, without showing it. If a key is already stored, setup offers to reuse
it. Without prompts, `keychain`, `secret-service` and `pass` need a terminal for
the tool's prompt unless `--key-stored` is given ("the key is already in the
store; record and verify the read command"). The store step's output is not
shown.

## Background service

### Commands

`mailtriage service install | uninstall | status --account NAME [--json]`
(`install` also takes `--interval-seconds` default 60 and `--limit` default
100). Setup's last step calls `install`.

### What `install` writes

The command line uses absolute paths: the current `mailtriage` executable,
`watch --config <absolute config path> --account <name> --interval-seconds N
--limit M --json`. No secrets are written; the key comes from the key command
(or, with `env`, the user adds the variable to the service themselves, and
`install` says so).

| | macOS (launchd) | Linux (systemd user) |
| --- | --- | --- |
| File | `~/Library/LaunchAgents/digital.wirdrei.mailtriage.<account>.plist` | `~/.config/systemd/user/mailtriage-<account>.service` |
| Restart | `KeepAlive` → `SuccessfulExit` false, `ThrottleInterval` 30 | `Restart=on-failure`, `RestartSec=30` |
| Permissions | `Umask` 63 (077) | `UMask=0077` |
| Logs | `<config dir>/logs/<account>.log` and `.err` | journald |
| Load | `launchctl bootstrap gui/<uid> <plist>` (after `launchctl bootout` when loaded) | `systemctl --user daemon-reload`, `systemctl --user enable <unit>`, `systemctl --user restart <unit>`, so a changed unit takes effect (plan decision 10) |
| Unload | `launchctl bootout gui/<uid>/<label>`, then delete the file | `systemctl --user disable --now <unit>`, then delete the file |

- Files mailtriage writes carry a marker (an `XMailtriageManaged` key in the
  plist; a `# managed by mailtriage` first line in the unit). `install` replaces
  and `uninstall` removes only marked files; an unmarked file at the path exits
  5.
- `install` is idempotent: rewriting an identical file and reloading is a
  no-op for the user.
- On Linux, setup prints that running while logged out needs
  `loginctl enable-linger $USER`; it does not run it.
- Other platforms: `service install` and `uninstall` exit 2 "unsupported
  platform"; `service status` reports `manager: "none"`.

### Status and heartbeat

Each `sync`/`watch` pass writes a heartbeat row for the account: finish time,
`partial`, exit code, filing mode. Schema migration v5 adds
`pass_heartbeats(account PRIMARY KEY, finished_at, partial, exit_code, mode)`.

`service status` returns `{manager, installed, loaded, running, pid,
last_exit_status, unit_path, log_paths, last_pass: {finished_at, partial,
exit_code, mode} | null}`. Manager fields come from `launchctl print
gui/<uid>/<label>` or `systemctl --user show <unit>`; `last_pass` comes from
the state database and works without any service.

## Safety

- Setup never reads, prints or writes the key; the OS tool prompts for it.
- Nothing is overwritten without confirmation (`--update` without prompts).
- `himalaya configure` runs only with a terminal.
- Setup makes no IMAP changes and never sets filing to `live`.
- The key command runs with the same trust as Himalaya's `password.command`:
  it comes from the user's own 0600 config. Docs state this.
- Service files contain no secrets and only absolute paths.

## Module layout

| Path | Responsibility |
| --- | --- |
| `src/prompt.rs` | Numbered menus, defaults, re-ask, stdin/stderr abstraction for tests |
| `src/secrets.rs` | Key command runner, key resolution, store options |
| `src/setup.rs` | Setup steps and flags-to-answers model |
| `src/system_service.rs` | Plist/unit generation (pure), install/uninstall/status via `launchctl`/`systemctl` |
| `src/config.rs` | Config path resolution, `api_key_command` validation |
| `src/provider.rs` | Uses `secrets::resolve_key` |
| `src/store.rs` | Schema v5 heartbeat |
| `src/cli.rs` | `setup` and `service` subcommands; `--config` becomes optional with resolution |

## Testing

- **Prompt flow** with `--interactive` and piped stdin, a fake `himalaya`
  script (`--version`, `account list`, `account check`, `imap list`), and fake
  `security`/`secret-tool`/`pass` first on `PATH`: every key option, reusing a
  stored key, creating an account via a fake `configure`, `--update`, refusal to
  overwrite, re-ask on invalid input, abort on end of input.
- **Without prompts:** `--yes` combinations; exit 2 naming each missing flag.
- **Key runner:** first line trimmed, non-zero exit, timeout, empty output,
  oversized output; errors never contain stdout or stderr; precedence (command
  beats env; env without command); validation requires one of them; generation
  hash unchanged by adding a command (golden test).
- **Config resolution:** each of the four steps with `HOME` and
  `MAILTRIAGE_CONFIG` overridden.
- **Service:** plist and unit text from pure functions (fixed-output tests);
  install/uninstall/status against fake `launchctl`/`systemctl` on `PATH`;
  refusal to touch an unmarked file; status parsing; heartbeat written by each
  `sync` pass and read by `status`.
- Existing tests unchanged; `init` and `./mailtriage.json` keep working.

## Docs

- README setup leads with `mailtriage setup`; the manual steps remain as a
  reference (in `docs/guide.md`, which holds the full reference; the README is
  short).
- `docs/agents/index.md`: non-interactive setup for agents (`--yes`, flags,
  `--key-stored` or `--key-env`), `service status` for health.

## Out of scope

Windows service support; storing IMAP credentials (Himalaya's job); editing
categories inside setup; a tray app (next project, which will use `service
status` and the heartbeat); automatic `live` filing.
