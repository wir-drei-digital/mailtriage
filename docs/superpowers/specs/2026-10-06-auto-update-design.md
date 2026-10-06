# Automatic updates

Date: 2026-10-06
Status: Design approved in conversation; written spec awaiting review.
Builds on: [guided setup](2026-10-05-guided-setup-design.md) (background service, `doctor`, heartbeats) and [releases](../../releases.md).
Extended by: [tray app](2026-10-06-tray-design.md) (tray archive, exit code 4).

## Goal

An installed mailtriage keeps itself current from GitHub Releases. The background
service installs a new stable release within about a day and switches to it between
passes. People, agents and the coming tray see whether an update is available, and
can install one with a single command, all in JSON.

Today updating means `git pull`, `cargo install --path . --locked --force` and
`mailtriage service install` again for every account.

## Decisions

| Topic | Decision |
| --- | --- |
| Source | GitHub Releases of `wir-drei-digital/mailtriage` (public), stable releases only. No token. |
| Policy | Config `updates`: `auto` (default), `notify` or `off`. `auto` installs in the background; `notify` only reports; `off` makes no network calls. |
| Trust | HTTPS to GitHub and the release's `SHA256SUMS`. No signatures. |
| Mechanism | Built into mailtriage: an `update` command and an update step in `watch`. New dependencies: `flate2` and `tar`. |
| Package managers | A binary owned by Homebrew or nix is never replaced; mailtriage only reports the update and names the manager's command. Homebrew distribution is a later, separate project. |
| Restart | The installer restarts nothing. Each running `watch` notices that its executable file was replaced and re-executes itself between passes. |
| Downgrades | Never. No rollback command; a bad release is fixed by a newer one. |

## Configuration

A new top-level key in `mailtriage.json`:

```json
{ "updates": "auto" }
```

- Values: `auto`, `notify`, `off`. A config without the key means `auto`. Any other
  value fails validation (exit 2, `updates must be auto, notify or off`).
- `init` and `setup` write the key explicitly.
- `setup` takes `--updates auto|notify|off`, with no prompt; the default is `auto`.
  `setup --update` keeps the existing value unless `--updates` is given. Setup's
  result gains `"updates": MODE`.
- The mode only governs `watch`. `mailtriage update` always works, in every mode,
  and needs no config.

## `mailtriage update`

```
mailtriage update [--check] [--json]
```

### `--check`

Asks GitHub for the latest stable release, writes the cache (see [Cache and lock](#cache-and-lock)),
and changes nothing else. Output:

```json
{"schema_version":1,"update":{
  "current":"0.2.0","latest":"0.3.0","available":true,
  "release_url":"https://github.com/wir-drei-digital/mailtriage/releases/tag/v0.3.0",
  "published_at":"2026-11-02T09:00:00Z","checked_at":"2026-11-03T08:12:40Z",
  "install":{"path":"/Users/alice/.local/bin/mailtriage","replaceable":true,"reason":null,"fix":null}}}
```

- `available` is true only when `latest` is strictly newer than `current`.
- `install.reason` is `null` or one of `unsupported_platform`, `managed_by_homebrew`,
  `managed_by_nix`, `not_writable`. `install.fix` is one sentence: for example
  `run \`brew upgrade mailtriage\``, or
  `make /opt/mailtriage writable for this user, or set "updates" to "notify"`.
- `--check` exits 0 whether or not the binary is replaceable.

### Without `--check`

Checks as above, then installs the latest release when it is newer
([Installing a release](#installing-a-release)). Output:

```json
{"schema_version":1,"update":{
  "action":"updated","from":"0.2.0","to":"0.3.0",
  "path":"/Users/alice/.local/bin/mailtriage",
  "previous_path":"/Users/alice/.local/bin/mailtriage.previous",
  "services":[{"account":"work","manager":"launchd","unit_path":"…/digital.wirdrei.mailtriage.work.plist","executable":"/Users/alice/.local/bin/mailtriage","same_binary":true}]}}
```

- `action` is `updated` or `current`. With `current`, `from` and `to` are both the
  running version and `services`/`previous_path` are omitted.
- `services` lists every service file mailtriage wrote (its marker is present) in
  this user's LaunchAgents directory or systemd user unit directory, with the
  executable it records (`ProgramArguments[0]` or the first `ExecStart=` word).
  `same_binary` compares canonical paths. Services with `same_binary: true` switch
  to the new release before their next pass ([Background behaviour](#background-behaviour));
  the others are reported and left alone. `update` runs no `launchctl` or
  `systemctl` command. On a platform without a service manager `services` is `[]`.

### Exit codes

| Code | Cause |
| --- | --- |
| 0 | Updated, already current, or `--check` done. |
| 2 | Invalid flags. |
| 3 | Network or GitHub error, no matching asset, checksum mismatch, failed smoke test, unpacking error, or (without `--check`) the binary is not replaceable: the message names `install.reason` and its fix. |
| 5 | Another update holds the lock (`another update is running`). |

The [tray spec](2026-10-06-tray-design.md) adds exit code 4: the CLI part succeeded
or was current, but updating `mailtriage-tray` failed.

## Installing a release

The same steps serve `update` and `watch`. Each failure leaves the installed binary
untouched and removes the temporary file.

1. **Latest release.** `GET https://api.github.com/repos/wir-drei-digital/mailtriage/releases/latest`
   with `User-Agent: mailtriage/<version>` and `Accept: application/vnd.github+json`.
   That endpoint never returns drafts or prereleases. The `tag_name` must be exactly
   `vX.Y.Z` (decimal numbers), else the check fails with
   `latest release tag TAG is not vX.Y.Z`. Versions compare numerically by
   (X, Y, Z). Only a strictly newer version is installed.
2. **Asset.** The compile target picks the platform: `linux-amd64`
   (x86_64 Linux GNU), `linux-arm64` (aarch64 Linux GNU), `macos-arm64` (aarch64
   macOS). Any other target reports `unsupported_platform`. The release must list
   `mailtriage-vX.Y.Z-PLATFORM.tar.gz` and `SHA256SUMS`, else
   `release vX.Y.Z has no PLATFORM archive`.
3. **Replaceable?** The target is the canonical path of the running executable.
   - It contains a path component `Cellar` followed by `mailtriage`: `managed_by_homebrew`.
   - It starts with `/nix/store/`: `managed_by_nix`.
   - A new file cannot be created in its directory: `not_writable`.
4. **Download.** Over HTTPS with `rustls`, from each asset's `browser_download_url`.
   - Redirects are followed only to `github.com` and hosts ending in `.github.com`
     or `.githubusercontent.com`; any other redirect fails the update.
   - The API call times out after 30 s; each download after 300 s.
   - An asset whose listed `size` or actual body exceeds 200 MB is refused.
5. **Verify.** The archive's SHA-256 must equal the hash on its line in
   `SHA256SUMS` (`<64 hex>  <file name>`). A missing line or a mismatch fails the update.
6. **Unpack.** Read the gzip tar and take the top-level regular-file entry named
   `mailtriage`; ignore `README.md` and `LICENSE`. Refuse the archive if that entry
   is missing, is a link, or exceeds 200 MB; any entry with an absolute path or a
   `..` component also fails the archive. Write it to
   `<dir>/.mailtriage-update-<pid>.tmp` in the target's directory (same filesystem,
   so the rename below is atomic), mode 0755, then fsync. Before writing, remove
   leftover `.mailtriage-update-*.tmp` files in that directory.
7. **Smoke test.** Run `<tmp> --version` with a 10 s limit through
   `process::run_bounded`. It must exit 0 and print exactly `mailtriage X.Y.Z`
   (the release version). This catches a wrong architecture, a Linux host whose
   glibc is older than the Ubuntu 24.04 build needs, and macOS refusing to run the
   binary. Failure: `release vX.Y.Z does not run here: <first line of stderr>`.
8. **Swap.**
   - Remove `<target>.previous` if it exists, then hard-link the target to
     `<target>.previous` (copy it when the filesystem refuses hard links).
   - Rename the temporary file over the target.
   - There is never a moment without a binary at the path, and running processes
     keep their open file. Replacing by rename matters on macOS: writing into the
     file of a running signed binary gets that process killed.
9. **Record.** Write `installed: {path, version, at}` to the cache.

The macOS binary is unsigned beyond the linker's ad-hoc signature, which Apple
Silicon requires and the release build already has. A file downloaded by mailtriage
carries no quarantine attribute, so Gatekeeper does not block it; the smoke test
proves it runs.

### Cache and lock

A per-user cache directory: `~/Library/Caches/mailtriage` on macOS; on Linux
`$XDG_CACHE_HOME/mailtriage` when `XDG_CACHE_HOME` is an absolute path, else
`~/.cache/mailtriage`. Without `HOME`, `update` exits 3 naming `HOME`, and `watch`
skips its update step.

- `update.lock` (an `fs2` exclusive lock): held for the whole check-and-install.
  `update` exits 5 when it is held; `watch` skips its update step and tries again
  at its next due time.
- `update.json`, written atomically (temporary file and rename) under the lock:

  ```json
  {"schema_version":1,"mode":"auto","checked_at":"…","next_check_at":"…",
   "latest":{"version":"0.3.0","release_url":"…","published_at":"…"},
   "last_error":null,"notified_version":"0.3.0",
   "installed":{"path":"/Users/alice/.local/bin/mailtriage","version":"0.3.0","at":"…"}}
  ```

  `last_error` is `null` or `{"at":"…","message":"…"}`; a successful check clears
  it. A missing or unreadable file counts as empty and is rewritten on the next
  check.

## Background behaviour

All of this lives in `watch`; no other command checks for updates.

### Restarting onto a new binary

At start, `watch` records the canonical path of its executable and that file's
identity (device, inode, size, modification time). Before each pass, and every 5 s
while it waits between passes, it compares the identity of the file now at that
path. When it differs:

1. It runs `<path> --version` (10 s limit). If that fails, it prints one `update`
   event with the error and keeps running the old code; it tries again only when
   the file changes again.
2. Otherwise it prints `{"schema_version":1,"update":{"event":"restarting","from":"0.2.0","to":"0.3.0"}}`
   and re-executes `<path>` with its original arguments and environment (Unix
   `exec`). The process keeps its PID, so launchd and systemd see no change; the
   new process starts a fresh `watch`.

This covers an update installed by another account's service, a manual
`mailtriage update`, and a `cargo install` over the same path, so `service install`
is no longer needed after rebuilding. `watch` never re-executes while a pass runs,
and holds no account lock between passes. When a stop was requested (Ctrl-C,
`SIGTERM`), it exits instead.

### Checking and installing

Before each pass, after the restart check, `watch` reads `updates` from the config.
When the config cannot be read, it uses the `mode` stored in `update.json`; with
none, it skips the step. When the mode it read differs from the stored one, it
stores it there.

- `off`: nothing, and no network call.
- `notify` and `auto`: when `next_check_at` has passed or is missing, check. After
  a successful check, `next_check_at` is 24 h plus a random 0 to 60 min later;
  after a failed one, 1 h later. A failed install in `auto` counts as a failed
  check. A service that keeps crashing and restarting therefore asks GitHub no
  more often.
- `notify`: when a newer version is available and differs from `notified_version`,
  print `{"schema_version":1,"update":{"event":"available","current":"…","latest":"…","release_url":"…"}}`
  once, and store `notified_version`.
- `auto`: when a newer version is available and the binary is replaceable, install
  it. The file at the path has then changed, so the restart rule above re-executes
  `watch` before its pass. When the binary is not replaceable, behave like `notify`;
  the event adds `install.reason` and `install.fix`.

With two services on one binary, both may find the release. The one that takes the
lock second re-reads the file identity under the lock; when the file was already
replaced, it installs nothing and restarts through the rule above.

An update error never fails or ends a pass and does not count as a partial pass.
It is printed as `{"schema_version":1,"update":{"event":"error","message":"…"}}`
and stored as `last_error`. Events are printed the way `watch` prints passes: one
JSON line with `--json`, else one text line.

### Which version ran a pass

Each pass records the version that ran it: a new nullable column
`pass_heartbeats.version`, set on every heartbeat write. This is the next schema
version (v7 after refile's v6); the version guard moves with it.
`service status` reports it as `last_pass.version` (`null` for rows written before
the migration), so the tray and agents can confirm that a service runs the new
release.

## Visibility

No command except `update` and `watch` makes an update network call.

- **`service status`** gains `update`, read from `update.json` plus a local
  replaceability check:

  ```json
  "update":{"mode":"auto","current":"0.2.0","latest":"0.3.0","available":true,
            "checked_at":"…","last_error":null,"replaceable":true,"reason":null}
  ```

  `mode` comes from the config `service status` already reads. Before the first
  check, `latest` and `checked_at` are `null` and `available` is false.
- **`doctor`** gains the same `update` block plus `ready`. It is false only when
  the mode is `auto` and the binary is not replaceable; then it adds `fix`. The
  top-level `ready` keeps its meaning (mail can be classified) and ignores it.
- **`setup`** step 9 adds an `update` check item with that `fix` when the update
  block is not ready.

## Releases and rollout

- Every published stable release is a deployment: copies in `auto` install it
  within about a day. `docs/releases.md` says so, recommends a `vX.Y.Z-rc.N` tag
  (a prerelease, never installed automatically) to try a build on one machine
  first, and drops the note about the repository being private.
- The release workflow is unchanged: archive names and layout (`mailtriage`,
  `LICENSE`, `README.md` at the top level) and `SHA256SUMS` already match.
- The first release that contains the updater is installed by hand once (from the
  archive, or with `cargo install` as today). From then on it updates itself.

## Errors

| Situation | Behaviour |
| --- | --- |
| No network, GitHub 5xx, rate limit (403/429) | `update` exits 3; `watch` records `last_error`, retries in 1 h. |
| Latest tag not `vX.Y.Z`, asset or `SHA256SUMS` missing | Same; message names the release. |
| Checksum mismatch, bad archive, smoke test fails | Same; nothing replaced, temporary file removed. |
| Not replaceable | `update` exits 3 with reason and fix; `update --check` exits 0; `watch` reports like `notify`. |
| Lock held | `update` exits 5; `watch` skips until its next due time. |
| Replaced file fails `--version` | `watch` prints one error event and keeps running the old code. |
| Older binary opens a newer database | Unchanged: the schema guard refuses it. Only reachable by a manual rollback after a migration. |

## Docs

- `guide.md`: a new **Updates** section (modes, `update` and `--check`, what is
  checked, package-managed installs, the cache files, rolling back by hand with
  `<binary>.previous` and why that fails after a migration). The configuration
  reference gains `updates`; the service section says a running service switches
  to a replaced binary by itself, while moving the executable still needs
  `service install`.
- `hermes.md`: hosts where provisioning owns the binary set `--updates off` or
  `notify`; agents read `update --check --json` and `service status`.
- `releases.md`: as in [Releases and rollout](#releases-and-rollout).
- README: one line under install.

## Testing

Tests never touch the real `~/.cargo/bin`, cache or service directories: `HOME`
and `XDG_CACHE_HOME` point into temporary directories, and the binary under test is
copied into a temporary directory and run from there.

**Unit**

- Version parsing and comparison: `v0.10.0` > `v0.9.9`; `v1.2.3-rc.1`, `1.2.3` and
  `v1.2` are not stable tags; equal and older versions are never installed.
- Asset name for each supported target; unsupported targets.
- `SHA256SUMS` parsing: matching line, missing line, malformed line.
- Redirect allowlist: `objects.githubusercontent.com` allowed;
  `github.com.evil.example` and `evilgithubusercontent.com` refused.
- Unpacking: link entry, `..` entry, absolute path, missing `mailtriage`, oversized
  entry.
- Package-manager detection: `/opt/homebrew/Cellar/mailtriage/0.3.0/bin/mailtriage`,
  `/nix/store/…-mailtriage-0.3.0/bin/mailtriage`, and a plain path.
- `next_check_at`: 24 h plus jitter within bounds after success; 1 h after failure.

**Fake release server.** A loopback HTTP server inside the test serves the API
response, `SHA256SUMS` and the archive. A hidden environment variable,
`MAILTRIAGE_UPDATE_URL`, replaces the API base URL; it is honoured only for
loopback hosts, and only then is plain HTTP and that host allowed. The release's
"binary" is a shell script: `--version` prints `mailtriage 9.9.9`; any other call
appends its arguments to a marker file.

**`update`**

- Updated: binary swapped, `.previous` is the old file, `update.json` records
  `installed`, output lists a fake service file with `same_binary` true and another
  with false.
- Already current: `action: current`, nothing written but the cache.
- Checksum mismatch, a smoke test that prints the wrong version, an unwritable
  directory, a Homebrew-style path: exit 3, binary untouched, no temporary file left.
- Lock held by the test: exit 5.
- `--check` against each case reports `available` and `install` correctly and
  changes no binary.
- A redirect to a host outside the allowlist fails.

**`watch`**

- `auto` with a newer release: installs, prints `restarting`, and the script's
  marker shows `watch` re-executed with its original arguments.
- File replaced from outside: `watch` re-executes before the next pass.
- Replacement that fails `--version`: one error event, `watch` keeps passing.
- `notify`: one `available` event across several passes, binary untouched.
- `off`: the server sees zero requests.
- A failing check sets `next_check_at` one hour out; restarting `watch` within that
  hour makes no request.
- An update error leaves the pass result and the partial count unchanged.
- `service status` shows `update` and `last_pass.version`.

## Out of scope

- Release signatures.
- Homebrew tap, nix packaging, other package managers.
- Windows and Intel Mac builds.
- Rollback or version-pinning commands; a prerelease channel.
- Updating the tray application: the [tray spec](2026-10-06-tray-design.md) extends
  this updater with the tray archive and exit code 4.
