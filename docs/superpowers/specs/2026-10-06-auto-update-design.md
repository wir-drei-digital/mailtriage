# Automatic updates

Date: 2026-10-06
Status: Design approved in conversation; written spec revised after three Codex review rounds.
Builds on: [guided setup](2026-10-05-guided-setup-design.md) (background service, `doctor`, heartbeats), [refiling](2026-10-06-filing-refile-design.md) (schema v6) and [releases](../../releases.md).
Extended by: [tray app](2026-10-06-tray-design.md) (tray archive, exit code 4).
Implementation order: after refile; this spec's migration (v7) needs refile's v6.

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
| Source | The highest stable release of `wir-drei-digital/mailtriage` (public) on GitHub, read from the release list. No token. GitHub's "Latest" flag is not relied on. |
| Policy | Config `updates`: `auto` (default), `notify` or `off`. `auto` installs in the background; `notify` only reports; `off` makes no network calls. |
| Trust | HTTPS to GitHub and the release's `SHA256SUMS`. No signatures. |
| Mechanism | Built into mailtriage: an `update` command and an update step in `watch`. New dependencies: `flate2`, `tar` and `semver`. |
| Package managers | A binary owned by Homebrew or nix is never replaced; mailtriage only reports the update and names the manager's command. Homebrew distribution is a later, separate project. |
| Restart | The installer restarts nothing. Each running `watch` notices that its executable file was replaced and re-executes itself between passes. |
| Versions | SemVer precedence. Only a release newer than the installed binary is installed; never a downgrade. No rollback command; a bad release is fixed by a newer one. |
| Database | Every migration in an automatically installed release stays compatible with processes of the previous release that are still running (additive changes only). |

## Configuration

A new top-level key in `mailtriage.json`:

```json
{ "schema_version": 3, "updates": "auto" }
```

- Values: `auto`, `notify`, `off`. A config without the key means `auto`. Any other
  value fails validation (exit 2, `updates must be auto, notify or off`).
- **Config schema 3.** Updater-capable binaries read schemas 1 to 3 and write
  `schema_version: 3` with an explicit `updates` on every config write (`init`,
  `setup`, `filing enable`/`disable`, `categories apply`). Older binaries accept only
  1 and 2, so they refuse such a config instead of rewriting it without `updates`,
  which would silently turn `off` back into `auto`.
- Updater-capable `setup` saves under the config lock and refuses with exit 5
  (reason `config_changed`) when the file changed since setup read it, as the other
  config writers already do.
- **Accepted risk:** an older binary that read the config before it became schema 3
  (for example an old `setup` waiting at a prompt) and saves afterwards can still
  drop `updates`. Old code cannot be changed; the window exists only while old and
  new binaries run side by side during the bootstrap.
- `setup` takes `--updates auto|notify|off`, with no prompt; the default is `auto`.
  Every setup path that edits an existing config (`--update`, adding an account,
  interactive or not) keeps the existing value unless `--updates` is given. Setup's
  result gains `"updates": MODE`.
- The mode only governs `watch`. `mailtriage update` always works, in every mode,
  and needs no config.

## Versions

Three versions are distinguished:

- **running**: the version compiled into the process (`CARGO_PKG_VERSION`), which may
  be a prerelease such as `0.3.0-rc.1` when someone installed a release candidate.
- **installed**: what the executable file at the installation path prints for
  `--version` right now, read under the installation lock.
- **candidate**: the highest stable release on GitHub.

Versions compare by SemVer 2.0 precedence (the `semver` crate), so
`0.3.0-rc.1 < 0.3.0 < 0.3.1`. A candidate is installed only when it is greater than
the installed version. A running `0.4.0-rc.1` therefore never installs stable
`0.3.0`.

**Selecting the candidate.**
`GET https://api.github.com/repos/wir-drei-digital/mailtriage/releases?per_page=30`
with `User-Agent: mailtriage/<running>`, `Accept: application/vnd.github+json` and
`X-GitHub-Api-Version: 2022-11-28`, following the `Link: rel="next"` header through
every page. Each page URL passes the same URL rules as downloads (step 5); more than
20 pages, or any page that fails, makes the whole check fail rather than choose from a
partial list. Drafts, prereleases and tags that are not exactly `vX.Y.Z` (decimal, no
leading zeros) are ignored; the highest remaining version is the candidate. With
none, the check reports `latest: null`.

## `mailtriage update`

```
mailtriage update [--check] [--json]
```

Both forms refresh the release information (see [Checking](#checking-release-information))
and need no config. The installation path is the running executable's canonical path
([Installation path and image](#installation-path-and-image)).

### `--check`

Changes nothing except the cache. Output:

```json
{"schema_version":1,"update":{
  "current":"0.2.0","installed":"0.2.0","latest":"0.3.0","available":true,
  "release_url":"https://github.com/wir-drei-digital/mailtriage/releases/tag/v0.3.0",
  "published_at":"2026-11-02T09:00:00Z","checked_at":"2026-11-03T08:12:40Z",
  "install":{"path":"/Users/alice/.local/bin/mailtriage","replaceable":true,"reason":null,"fix":null}}}
```

- `current` is the running version, `installed` the installation path's `--version`
  (`null` when it cannot be read), `available` is true only when `latest` is greater
  than `installed`.
- `install.reason` is `null` or one of `unsupported_platform`, `managed_by_homebrew`,
  `managed_by_nix`, `not_writable`, `unsafe_permissions`. `install.fix` is one
  sentence, for example `run \`brew upgrade mailtriage\``, or
  `make /opt/mailtriage writable for this user, or set "updates" to "notify"`.
- `--check` exits 0 whether or not the binary is replaceable.

### Without `--check`

Checks, then installs the candidate when it is newer than the installed version
([Installing a release](#installing-a-release)). Output:

```json
{"schema_version":1,"update":{
  "action":"updated","from":"0.2.0","to":"0.3.0",
  "path":"/Users/alice/.local/bin/mailtriage",
  "previous_path":"/Users/alice/.local/bin/mailtriage.previous",
  "warnings":[],
  "services":[{"account":"work","manager":"launchd","unit_path":"…/digital.wirdrei.mailtriage.work.plist","executable":"/Users/alice/.local/bin/mailtriage","same_binary":true}]}}
```

- `action` is `updated` or `current`. With `current`, `from` and `to` are both the
  installed version and `services`/`previous_path` are omitted.
- `warnings` lists problems after the commit point (for example
  `installed; recording the update failed: …`). The update still counts as done.
- `services` lists every service file mailtriage wrote (its marker is present) in
  this user's LaunchAgents directory or systemd user unit directory.
  `executable` is decoded from the file ([Reading service files](#reading-service-files));
  `same_binary` compares canonical paths, and is `false` with `executable: null` when
  the file cannot be decoded. Services with `same_binary: true` switch to the new
  release before their next pass ([Restarting onto a new binary](#restarting-onto-a-new-binary));
  the others are reported and left alone. `update` runs no `launchctl` or `systemctl`
  command. On a platform without a service manager `services` is `[]`.

### Exit codes

| Code | Cause |
| --- | --- |
| 0 | Updated (also with `warnings`), already current, or `--check` done. |
| 2 | Invalid flags. |
| 3 | Network or GitHub error, no matching asset, checksum mismatch, bad archive, failed smoke test, the installed binary changed during the update, or (without `--check`) the binary is not replaceable: the message names `install.reason` and its fix. |
| 5 | Another update held the installation lock for 60 s (`another update is running`). |

The [tray spec](2026-10-06-tray-design.md) adds exit code 4: the CLI part succeeded
or was current, but updating `mailtriage-tray` failed.

## Installing a release

The same steps serve `update` and `watch`.

### Locks

- **Installation lock:** `<dir>/.mailtriage-update.lock` in the installation path's
  directory, an `fs2` exclusive lock on a file that is created when missing and never
  deleted. It does not depend on `HOME` or `XDG_CACHE_HOME`, so updaters with
  different cache directories, or different users sharing a writable installation,
  serialize on it. `update` waits up to 60 s for it, then exits 5; `watch` tries once
  and otherwise skips installing until its next pass.
- **Cache lock:** `update.lock` in the cache directory, held only while reading or
  writing `update.json`.
- Order: the installation lock first, then the cache lock; nothing waits for the
  installation lock while holding the cache lock.

Writers that do not use these locks (`cargo install`, a person copying a file) can
still race with an update. Step 9's revalidation narrows that window; it cannot close
it.

### Steps

1. **Candidate.** From a fresh check (`update`) or from the cached release
   information (`watch`, see [Installing in the background](#installing-in-the-background)).
2. **Asset.** The compile target picks the platform: `linux-amd64` (x86_64 Linux
   GNU), `linux-arm64` (aarch64 Linux GNU), `macos-arm64` (aarch64 macOS). Any other
   target reports `unsupported_platform`. The release must list exactly one asset
   named `mailtriage-vX.Y.Z-PLATFORM.tar.gz` and one named `SHA256SUMS`, else
   `release vX.Y.Z has no PLATFORM archive`.
3. **Replaceable?**
   - The path contains a component `Cellar` followed by `mailtriage`:
     `managed_by_homebrew`. It starts with `/nix/store/`: `managed_by_nix`.
   - The installation path must be a regular file owned by the current user, in a
     directory owned by the current user. Neither may be writable by group or others:
     else `unsafe_permissions`.
   - A file cannot be created in the directory: `not_writable`.
4. **Under the installation lock:**
   - Remove leftover `.mailtriage-update-*` files in the directory. No cooperating
     updater is active while the lock is held, so these are stale.
   - Read the installed version (`<path> --version`, see step 8 for the runner) and
     record the installation path's identity (device, inode, size, modification
     time, change time, mode).
   - When the candidate is not greater than the installed version: `action: current`,
     nothing downloaded.
5. **Download.** For the initial URL and every redirect:
   - scheme `https`, port 443, no user name or password;
   - host `github.com`, or ending in `.github.com` or `.githubusercontent.com`
     (`api.github.com`, `objects.githubusercontent.com` and
     `release-assets.githubusercontent.com` all match);
   - at most 10 redirects; a redirect to any other URL fails the update.
   - The API call times out after 30 s, each download after 300 s. An asset whose
     listed `size` or actual body exceeds 200 MB is refused.
6. **Verify.** `SHA256SUMS` must contain exactly one line for the archive's exact file
   name (`<64 lowercase hex>  <name>`); none, several or a malformed line fails the
   update. The archive's SHA-256 must match it.
7. **Unpack.** Read the gzip tar stream to its end before using anything from it:
   - at most 16 entries, at most 256 MB decompressed in total, entry paths at most
     255 bytes;
   - exactly one entry named `mailtriage` at the top level, and it is a regular file
     of at most 200 MB; `README.md` and `LICENSE` are skipped;
   - links, absolute paths, `..` components, a second `mailtriage` or a truncated
     stream fail the archive.
   - The executable is written to `<dir>/.mailtriage-update-<random>.tmp`, created
     exclusively (`create_new`, which never follows an existing link) with mode 0600.
     It is fsynced and closed, then set to mode 0755, and its identity is recorded.
8. **Smoke test.** Run `<staged> --version` with a 10 s limit through a new bounded
   runner that also captures stderr (at most 4 KB). The existing `run_bounded` keeps
   discarding stderr, because key commands use it. Stdout must be exactly
   `mailtriage X.Y.Z` (the candidate), optionally followed by one newline. This
   catches a wrong architecture, a Linux host whose glibc is older than the Ubuntu
   24.04 build needs, and macOS refusing to run the binary. Failure messages name the
   cause: `could not start` (spawn error), `killed by signal N`, `timed out`,
   `printed "…"` (wrong output), each with the first line of stderr when there is one.
9. **Commit.**
   1. **Revalidate.** The installation path still has the identity from step 4 and
      the staged file the one from step 7. Otherwise abort with
      `the installed binary changed during the update; try again`.
   2. **Backup copy.** Hard-link the installation path to
      `.mailtriage-update-<random>.prev` (when the filesystem refuses hard links,
      copy into that exclusively created file and fsync it). `.previous` is not
      touched yet.
   3. **Commit point.** Rename the staged file over the installation path.
   4. **Publish the backup.** Rename `.mailtriage-update-<random>.prev` over
      `<path>.previous`.
   5. fsync the directory.
   - Any failure before the commit point leaves the installation path and
     `.previous` untouched and removes this attempt's temporary files.
   - A failure in step 4 is a warning; the backup copy stays under its temporary
     name until the next update's cleanup.
   - From the commit point on, the update counts as installed. Later failures (the
     directory fsync, the cache write) become `warnings`. The swap is never
     reversed: another watcher may already run the new binary.
   - Replacing by rename keeps running processes on their open file, and matters on
     macOS: writing into the file of a running signed binary gets that process
     killed.
10. **Record.** Under the cache lock, write the installation's entry in `update.json`.

The macOS binary is unsigned beyond the linker's ad-hoc signature, which Apple
Silicon requires and the release build already has. A file downloaded by mailtriage
carries no quarantine attribute, so Gatekeeper does not block it; the smoke test
proves it runs.

### Cache

A per-user cache directory: `~/Library/Caches/mailtriage` on macOS; on Linux
`$XDG_CACHE_HOME/mailtriage` when `XDG_CACHE_HOME` is an absolute path, else
`~/.cache/mailtriage`. Without `HOME`, `update` exits 3 naming `HOME`, and `watch`
skips its update step.

`update.json`, written atomically (an exclusively created temporary file, fsync,
rename) under the cache lock:

```json
{"schema_version":1,
 "release":{"version":"0.3.0","release_url":"…","published_at":"…",
            "archives":{"mailtriage":{"name":"mailtriage-v0.3.0-macos-arm64.tar.gz","url":"…","size":4812345}},
            "sums":{"name":"SHA256SUMS","url":"…","size":512}},
 "checked_at":"…","next_check_at":"…","check_failures":0,"last_check_error":null,
 "configs":{"/Users/alice/.config/mailtriage/mailtriage.json":{"mode":"auto","notified_version":null}},
 "installs":{"/Users/alice/.local/bin/mailtriage":{"version":"0.3.0","at":"…","last_error":null,"failures":0,"next_attempt_at":null}}}
```

- `release` is `null` when the last successful check found no stable release.
- `release.archives` maps each component to its asset for this platform
  (`{name, url, size}`, or `null` when the release lacks it). This spec defines the
  `mailtriage` component; the tray spec adds `mailtriage-tray`. Installing from the
  cache selects the component's entry before step 4; a `null` entry fails that
  component with `release vX.Y.Z has no PLATFORM archive`.
- `configs` is keyed by canonical config path, and `installs` by canonical
  installation path. A process only uses its own entries.
- Errors are `null` or `{"at":"…","message":"…"}`.
- A missing or unreadable file counts as empty. A file that cannot be written stops
  `watch`'s network work (see below).

## Background behaviour

All of this lives in `watch`; no other command checks for updates in the background.
Each pass runs in this order: the restart check, the update step, then the pass
itself. A release whose passes fail can therefore still update itself, as long as it
reaches its update step.

### Installation path and image

At start, before anything else, `watch` records:

- **installation path:** `std::env::current_exe()`, canonicalized. On Linux, when that
  names a deleted file (`/proc/self/exe` ends in ` (deleted)`), `argv[0]` is used when
  it is absolute and exists. Otherwise the restart rule is off for this process, and
  one `update` error event says so.
- **image identity:** the identity of the file this process runs. On Linux, that is
  the metadata of `/proc/self/exe`, which follows the loaded file even after it was
  replaced. On macOS, it is the installation path's metadata at start; because the
  file could have been replaced between launch and that moment, `watch` also runs
  `<path> --version` once at start, and when it prints a version other than the
  running one, the restart rule fires before the first pass. **Accepted limit:** on
  macOS a rebuild with the same version that replaces the file in the instant
  between launch and this check is not noticed; the next replacement or restart
  picks it up. Releases always carry a new version, so updates are not affected.
- On Linux, when the installation path's identity already differs from the image
  identity at start, the file was replaced while the process started. The restart
  rule fires before the first pass.

Identity is (device, inode, size, modification time, change time, mode), so a
`chmod` that makes a file runnable again also counts as a change.

### Restarting onto a new binary

Before each pass, and every 5 s while waiting between passes, `watch` compares the
installation path's identity with the image identity. When they differ:

1. **Probe.** Run `<path> --version` with the bounded runner from step 8; stdout must
   be `mailtriage <semver>` with an optional newline.
2. **Revalidate.** Check that the path still has the identity that was probed.
3. **Clean up explicitly,** not through destructors, because a successful `exec` runs
   none:
   - drop the open `Service`, its database connection and the account lock;
   - release update locks;
   - flush stdout and stderr.
4. **Restart.** Print
   `{"schema_version":1,"update":{"event":"restarting","pid":1234,"from":"0.2.0","to":"0.3.0"}}`
   and `exec` `<path>` with the original `args_os` and environment. The process keeps
   its PID, so launchd and systemd see no change.

**Failures.** The probe fails, the file is missing, or `exec` returns an error.
`watch` then prints one `update` error event per identity and kind of failure, keeps
running the old code, and retries after 1 min, doubling up to 1 h. It retries at once
when the identity changes again.

**Stop requests.** When a stop was requested (Ctrl-C, `SIGTERM`), `watch` exits
instead of restarting.

`watch` never re-executes while a pass runs. This rule covers an update installed by
another account's service, a manual `mailtriage update`, and a `cargo install` over
the same path: a running service switches to a rebuilt binary without
`service install`.

### Checking release information

`watch` reads `updates` from its config at each pass, and stores the mode under the
canonical config path in `configs`. When the config cannot be read or its `updates`
value is invalid, `watch` uses that config's stored mode. With none stored, it skips
the update step. Another config's mode is never used.

- `off`: nothing, and no network call.
- `notify` and `auto`: refresh the release information when `next_check_at` has
  passed or is missing.
  - **Reservation.** Before the request, `watch` writes `next_check_at = now + 1 h`.
    If the cache cannot be written, it makes no network call. It prints one event
    and the pass continues. A process that crashes during the request therefore
    does not retry sooner after a restart.
  - **Success:** store `release`; `next_check_at = now + 24 h + random 0–60 min`;
    `check_failures = 0`.
  - **Failure:** `check_failures += 1`. `next_check_at` is the later of
    1 h × 2^(failures − 1) (at most 24 h, ±10 % jitter), the `Retry-After` header, and
    `X-RateLimit-Reset` when `X-RateLimit-Remaining` is 0.
- `update` and `update --check` refresh the same way, in every mode.

### Installing in the background

Installing is decided per installation from the cached `release`. It is separate
from the refresh deadline, so a `notify` watcher that refreshed first never holds
back an `auto` watcher.

- In `auto`, before each pass, when the cached `release` is at most 48 h old and the
  watcher's installation entry has no `next_attempt_at` in the future, `watch` reads
  the installed version at its installation path. Reading it means running
  `<path> --version` only when the file's identity changed since the last reading,
  otherwise the cached `installs` version is used. When the cached release is newer,
  `watch` installs it with steps 2 to 10, using the cached asset URLs.
- **Reservation.** Before downloading, write `installs[path].next_attempt_at =
  now + 1 h`; when the cache cannot be written, do not install. A watcher killed
  during a download therefore does not start another one right after its restart.
- **Failure.** Record `installs[path].last_error` and increase `failures`;
  `next_attempt_at` backs off from 1 h, doubling, up to 24 h.
- **Success.** Clear `next_attempt_at`, `failures` and `last_error`. The restart
  rule re-executes `watch` before its pass.
- **Not replaceable.** Behave like `notify`; the event adds `install.reason` and
  `install.fix`.
- **`notify`, or `auto` when not replaceable.** When the cached release is newer than
  the installed version and differs from `configs[config].notified_version`, print
  `{"schema_version":1,"update":{"event":"available","current":"…","latest":"…","release_url":"…"}}`
  once, and store `notified_version`.
- **Switching modes.** A change from `notify` to `auto` takes effect at the next pass,
  using the cached release.

**Errors stay contained.** An update error never fails or ends a pass, and does not
count as a partial pass. It is printed as
`{"schema_version":1,"update":{"event":"error","message":"…"}}`. Events are printed
the way `watch` prints passes: one JSON line with `--json`, else one text line.

### Rolling updates and the database

During an update, processes of the old and the new release can use one state
database at the same time. One account's service may already have restarted and
migrated it while another account's old process finishes a pass on an open
connection.

- **Migration rule.** Every migration in a release that `auto` installs must keep the
  previous release's running processes correct. Only new tables, and new columns that
  are nullable or have defaults, are allowed: no renames, no drops, no changed
  meanings. Refile's v6 and this spec's v7 follow the rule.
- **Old process after a migration.** When an old process opens the database again,
  the existing schema guard refuses it. That pass fails, and the process restarts
  onto the new binary, either through the restart rule or through its service
  manager.
- **Releases that break the rule.** A release whose migration cannot follow the rule
  must not be published as a stable release. Mechanisms for such releases are out of
  scope.

### Which version ran a pass

Each pass records the version that ran it: a new nullable column
`pass_heartbeats.version`, set on every heartbeat write, including error heartbeats.
This is schema **v7**.

- **Depends on v6.** v7 is implemented on top of refile's complete v6 migration.
  Jumping from 5 to 7, or reserving 6, is forbidden: migrations are chosen by
  `user_version`, so a skipped v6 would never run.
- **Guard.** The version guard moves to 7.
- **Shown in `service status`** as `last_pass.version`, which is `null` for rows
  written before the migration. The tray and agents use it to confirm that a service
  runs the new release.

## Visibility

No command except `update` and `watch` makes an update network call.

- **`service status`** gains `update`. It describes the service's executable (decoded
  from the unit file), not the binary that runs `service status`:

  ```json
  "update":{"mode":"auto","executable":"/Users/alice/.local/bin/mailtriage",
            "installed":"0.2.0","latest":"0.3.0","available":true,
            "checked_at":"…","last_error":null,"replaceable":true,"reason":null}
  ```

  - `mode` comes from the config that `service status` already reads.
  - `installed` is the executable's `--version`, run with the bounded runner; it is
    `null` when that fails.
  - `last_error` is the installation's `last_error`, else the check's
    `last_check_error`.
  - Without a service file, `executable` is `null`, and `installed` and `replaceable`
    describe the binary that runs `service status`.
  - Before the first check, `latest` and `checked_at` are `null` and `available` is
    false.
- **`doctor`** gains the same `update` block plus `ready`. It is false only when the
  mode is `auto` and the executable is not replaceable; then it adds `fix`. The
  top-level `ready` keeps its meaning (mail can be classified) and ignores it.
- **`setup`** step 9 adds an `update` check item with that `fix` when the update
  block is not ready.

### Reading service files

The executable is decoded from the file mailtriage wrote, with the exact inverse of
its writer:

- **plist:** the first `<string>` of `ProgramArguments`, with XML entities decoded.
- **systemd:** the first word of `ExecStart=`, undoing `systemd_arg`'s quoting and its
  `%`/`$` doubling.

A file that cannot be decoded gives `executable: null`; it is never an error after an
update has committed.

## Releases and rollout

- **Every published stable release is a deployment.** Copies in `auto` install it
  within about a day. `docs/releases.md` says so. It recommends a `vX.Y.Z-rc.N` tag (a
  prerelease, never installed automatically) to try a build on one machine first, and
  drops the note about the repository being private.
- **Release workflow changes.**
  - Publishing uses one concurrency group across all tags (queued, never cancelled),
    so two releases never publish at once.
  - A stable release is marked Latest only when it is higher than every published
    stable release. The updater does not depend on this, but people browsing GitHub
    do.
  - Archive names and layout (`mailtriage`, `LICENSE`, `README.md` at the top level)
    and `SHA256SUMS` stay as they are.
- **Bootstrap.**
  1. Install the first updater-capable release by hand (from the archive, or with
     `cargo install` as today).
  2. Run `mailtriage service install --account NAME` once for every account. The old
     processes have no restart rule, so they would keep running the old code.
  3. Check that `service status` shows the new `last_pass.version`.

  From then on, mailtriage updates itself.
- **Manual forward recovery,** documented in `guide.md`, for a release that breaks
  `watch` before its update step runs:
  - run `mailtriage update`, which needs no config;
  - if that does not run either, download the archive and `SHA256SUMS` with `curl`,
    verify them with `shasum -a 256 --check --ignore-missing`, unpack, and `mv` the
    binary into place.
- **Rollback by hand,** documented:
  1. set `updates` to `off` in every config;
  2. stop the services;
  3. `mv <binary>.previous <binary>`;
  4. start the services.

  This only works when the newer release did not migrate the database. An older binary
  refuses a newer database with a clear error; then roll forward instead.

## Errors

| Situation | Behaviour |
| --- | --- |
| No network, GitHub 5xx, rate limit (403/429) | `update` exits 3; `watch` records the error and backs off (honouring `Retry-After` and the rate-limit reset). |
| No stable release, asset or `SHA256SUMS` missing, ambiguous checksum line | `update` exits 3 naming the release; `watch` records the install error and backs off. |
| Checksum mismatch, bad archive, smoke test fails | Same; nothing replaced, temporary files removed. |
| Installed binary changed during the update | Same; nothing replaced. |
| Failure after the commit point | `update` exits 0 with `warnings`; `watch` prints an error event; the swap stays. |
| Not replaceable | `update` exits 3 with reason and fix; `update --check` exits 0; `watch` reports like `notify`. |
| Installation lock held | `update` waits 60 s, then exits 5; `watch` skips installing until its next pass. |
| Cache not writable | `update` reports a warning; `watch` makes no network call and prints one event. |
| Replaced file fails `--version`, or `exec` fails | `watch` prints one error event per identity and kind, keeps running the old code, retries with backoff. |
| Older process opens a newer database | Unchanged: the schema guard refuses it; it restarts onto the new binary. |

## Docs

- `guide.md`: a new **Updates** section covering:
  - the modes, `update` and `--check`, and what is checked;
  - package-managed installs and the cache files;
  - the bootstrap steps, manual forward recovery, and rollback by hand with
    `<binary>.previous` (and why that fails after a migration).

  The configuration reference gains `updates` and schema 3. The service section says
  a running service switches to a replaced binary by itself, while moving the
  executable still needs `service install`.
- `hermes.md`: hosts where provisioning owns the binary set `--updates off` or
  `notify`. Agents read `update --check --json` and `service status`.
- `releases.md`: as in [Releases and rollout](#releases-and-rollout), plus the
  workflow changes.
- README: one line under install.

## Testing

Tests never touch the real `~/.cargo/bin`, cache or service directories. `HOME` and
`XDG_CACHE_HOME` point into temporary directories, and the binary under test is copied
into a temporary directory and run from there.

**Unit**

- Version precedence: `0.10.0 > 0.9.9`, `0.3.0-rc.1 < 0.3.0`, and a running
  `0.4.0-rc.1` never installs `0.3.0`. Equal and older versions are never installed.
- Tag selection: `v1.2.3-rc.1`, `1.2.3`, `v1.2`, `v01.2.3`, drafts and prereleases are
  ignored, and the highest stable release wins even when it is not first in the list
  or sits on page 2 behind 30 release candidates; a failing second page fails the
  check.
- Asset names for each supported target; unsupported targets.
- `SHA256SUMS`: a matching line, a missing line, a duplicate line, uppercase hex, a
  malformed line.
- URL rules:
  - allowed: `objects.githubusercontent.com`, `release-assets.githubusercontent.com`;
  - refused: `github.com.evil.example`, `evilgithubusercontent.com`, `http://`, a
    port other than 443, a URL with a user name;
  - an 11th redirect fails.
- Unpacking: a link entry, a `..` entry, an absolute path, a missing `mailtriage`, two
  `mailtriage` entries, too many entries, a decompression bomb past 256 MB, a truncated
  stream.
- Package-manager detection (`/opt/homebrew/Cellar/mailtriage/0.3.0/bin/mailtriage`,
  `/nix/store/…`, a plain path); `unsafe_permissions` for a group-writable directory.
- Scheduling:
  - the reservation is written before the request;
  - success schedules 24 h plus jitter, within bounds;
  - the failure backoff doubles and is capped;
  - `Retry-After` and `X-RateLimit-Reset` win when later.
- Service file decoding round-trips paths with spaces, `%`, `$`, quotes, backslashes,
  `&` and `<`.
- Config: schema 3 written with `updates`; schemas 1 and 2 read as `auto`; every
  setup path keeps an existing `off`; setup refuses to save over a config changed
  since it read it (exit 5).

**Fake release server.** A loopback HTTP server inside the test serves the release
list, `SHA256SUMS` and the archive.

- A hidden environment variable, `MAILTRIAGE_UPDATE_URL`, replaces the API base URL.
  It is honoured only when its host is `127.0.0.1`, `::1` or `localhost`. Only then
  are plain HTTP and that origin allowed, for the initial URL and for redirects.
- The release's "binary" is a shell script: `--version` prints `mailtriage 9.9.9`, and
  any other call appends its arguments to a marker file.

**`update`**

- **Updated.** The binary is swapped and `.previous` is the old file. `update.json`
  records the installation. The output lists a fake service file with `same_binary`
  true and another with false.
- **Already current**, including when the installed file is newer than the running
  process: `action: current`.
- **Refused, binary untouched, no temporary file left:**
  - a checksum mismatch;
  - a smoke test that prints the wrong version;
  - an unwritable directory;
  - a Homebrew-style path.
  Each exits 3.
- **Installation lock** held by the test: exits 5 after the wait. Two different
  `XDG_CACHE_HOME` values against one binary serialize on the installation lock.
- **Revalidation.** The test replaces the installation path between download and
  commit (through a test hook): exit 3, and the test's file stays.
- **Fault injection** before and after each step of the commit:
  - before the commit point, including a failing commit rename: the installation
    path and the old `.previous` are untouched;
  - a failing backup publish after the commit: a warning, the new binary in place;
  - after it: the new binary is in place, the warning is reported, and nothing is
    reversed.
- **Redirects.** A redirect to a host outside the allowlist fails.
- **`--check`** in each case reports `available` and `install` correctly and changes
  no binary.

**`watch`**

- **Real re-exec,** not a script. Copy the real binary into a temp directory and start
  `watch` from it, using the fake provider and the config given through
  `MAILTRIAGE_CONFIG` to show the environment survives. Replace the file with a fresh
  copy (a new inode). Then:
  - the `restarting` event names the original PID;
  - that PID keeps producing passes for the same account;
  - between passes, a `sync` for that account takes the account lock, so locks were
    released;
  - `SIGTERM` after the re-exec stops it cleanly.
- **Script-based scenarios:**
  - `auto` installs a newer release and restarts;
  - a replacement that fails `--version` gives one error event, `watch` keeps
    passing, and a later `chmod +x` (a change-time change) triggers the restart;
  - `exec` failure (the file removed after the probe, through a test hook) keeps the
    old code running;
  - `notify` prints one `available` event across several passes, binary untouched;
  - `off`: the server sees zero requests.
- **Scheduling across restarts.** A failing check followed by restarting `watch` makes
  no request until `next_check_at`, including when the test kills `watch` right after
  the request. Killing `watch` during an archive download: the restarted `watch`
  does not download again before `next_attempt_at`. A notify watcher's refresh does not hold back an auto watcher's
  install.
- **Unreadable config.** It uses only that config's stored mode; with none, no
  request.
- **Isolation.** An update error leaves the pass result and the partial count
  unchanged.
- **Schema:**
  - empty → 7, 5 → 6 → 7, 6 → 7, and a database at 8 refused;
  - old heartbeat rows read as `version: null`;
  - success and error heartbeats carry the version;
  - a connection opened at v6 can still insert a heartbeat after another connection
    migrated to v7.
- **`service status`** shows `update` for the service's executable and
  `last_pass.version`.

## Out of scope

- Release signatures.
- Homebrew tap, nix packaging, other package managers.
- Windows and Intel Mac builds.
- Rollback or version-pinning commands; a prerelease channel.
- A recovery mechanism independent of the installed binary. **Accepted risk:** a
  release that prints its version but fails before `watch` reaches its update step
  cannot repair itself; the manual forward recovery above covers it.
- Releases whose migrations break running processes of the previous release.
