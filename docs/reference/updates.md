# Updates reference

For a walkthrough, see [updates](../guide/updates.md). Use this page to look up flags, output fields and detailed behavior.

You don't have to keep an eye on new releases: mailtriage installs them itself, from [GitHub Releases](https://github.com/wir-drei-digital/mailtriage/releases).

The background service installs a new stable release within about a day and switches to it between passes. `mailtriage update` installs one at once.

## Modes

`updates` in `mailtriage.json` decides what `watch` does:

| Value | `watch` |
| --- | --- |
| `auto` (default) | Checks for a new release about once a day and installs it. |
| `notify` | Checks about once a day and prints an `available` event; installs nothing. |
| `off` | Makes no network call. |

Set it with `mailtriage setup --update --updates notify`, or edit the file.

The mode governs only `watch`: `mailtriage update` works in every mode and needs no config.

## `mailtriage update`

`--check` reports and changes nothing but the cache; without it, `update` installs the newest stable release:

```sh
mailtriage update --check --json
mailtriage update --json
```

Both read the release list of `wir-drei-digital/mailtriage` from the GitHub API, every page, without a token.

The candidate is the highest release whose tag is exactly `vX.Y.Z`: drafts, prereleases such as `v0.4.0-rc.1` and other tags are ignored, and GitHub's "Latest" flag is not used. Versions compare by SemVer, so `0.3.0-rc.1 < 0.3.0 < 0.3.1`.

A release is installed only when it is newer than the installed binary. mailtriage never downgrades, and a release candidate you installed by hand stays until a higher stable release appears.

`update` replaces the binary that runs it (its path with symlinks resolved). In order, it:

1. reads the installed version with `<binary> --version` (when that fails, it compares with the version that is running), and stops with `action: current` when the release is not newer;
2. checks that this binary may be replaced (see [Binaries mailtriage does not replace](#binaries-mailtriage-does-not-replace));
3. takes the installation lock, `.mailtriage-update.lock` next to the binary, waiting up to 60 seconds for another update, and reads the installed version again under it: when another update installed the release meanwhile, it stops with `action: current`;
4. downloads `mailtriage-vX.Y.Z-PLATFORM.tar.gz` and `SHA256SUMS` over HTTPS from `github.com` and GitHub's download hosts only, with at most 10 redirects and 200 MB; `PLATFORM` is `macos-arm64`, `linux-amd64` or `linux-arm64`;
5. checks the archive's SHA-256 against its line in `SHA256SUMS`;
6. unpacks the `mailtriage` executable next to the binary and runs `--version` on it, which must print the release's version. This catches a wrong architecture, a glibc older than the release needs, and macOS refusing to run the binary;
7. checks that the installed binary did not change meanwhile, keeps it as `<binary>.previous`, and moves the new binary into place with a rename. Running processes keep the old file open, so nothing running is disturbed.

When a step before the rename fails, the binary and `<binary>.previous` are unchanged and the temporary files are removed. After the rename the update counts as done; a later problem, such as keeping the backup or recording the update, is a warning.

::: warning Important
Releases are verified by HTTPS and `SHA256SUMS`, not by signatures.
:::

`update` starts and stops nothing: each running `watch` switches to the new binary by itself (see [How a running service switches](#how-a-running-service-switches)).

`--check` reports:

```json
{"schema_version":1,"update":{"current":"0.2.0","installed":"0.2.0","latest":"0.3.0","available":true,"release_url":"https://github.com/wir-drei-digital/mailtriage/releases/tag/v0.3.0","published_at":"2026-11-02T09:00:00Z","checked_at":"2026-11-03T08:12:40Z","install":{"path":"/Users/alice/.local/bin/mailtriage","replaceable":true,"reason":null,"fix":null},"warnings":[]}}
```

| Field | Content |
| --- | --- |
| `current` | The version of the process that ran the command. |
| `installed` | What the binary at `install.path` prints for `--version` now; `null` when it does not run. |
| `latest` | The highest stable release, or `null` when there is none. |
| `available` | `true` when `latest` is newer than `installed`. |
| `release_url`, `published_at` | The release's GitHub page and publication time. |
| `checked_at` | When this check ran. |
| `install` | `path`: the binary `update` would replace. `replaceable`, `reason` and `fix`: see [Binaries mailtriage does not replace](#binaries-mailtriage-does-not-replace). |
| `tray` | Only with a `mailtriage-tray` next to the binary: its `path`, `installed` version and `available` (see [The tray next to the CLI](#the-tray-next-to-the-cli)). `available` above is the CLI's alone. |
| `warnings` | Problems that did not stop the command, such as a cache that could not be written. |

Without `--check`:

```json
{"schema_version":1,"update":{"action":"updated","from":"0.2.0","to":"0.3.0","path":"/Users/alice/.local/bin/mailtriage","previous_path":"/Users/alice/.local/bin/mailtriage.previous","warnings":[],"services":[{"account":"work","manager":"launchd","unit_path":"/Users/alice/Library/LaunchAgents/digital.wirdrei.mailtriage.work.plist","executable":"/Users/alice/.local/bin/mailtriage","same_binary":true}]}}
```

| Field | Content |
| --- | --- |
| `action` | `updated`, or `current` when the release is not newer. With `current`, `from` and `to` are both the installed version, and `previous_path` and `services` are absent. |
| `from`, `to` | The installed version before and after. |
| `path` | The binary that was replaced. |
| `previous_path` | Where the previous binary is: `<binary>.previous`, or, with a warning, a `.mailtriage-update-*.prev` file next to it. |
| `warnings` | Problems after the rename, for example `installed; recording the update failed: …`. The update still counts as done. |
| `services` | Every service file mailtriage wrote in this user's LaunchAgents or systemd user directory: `account`, `manager`, `unit_path`, the `executable` it runs (`null` when the file cannot be decoded), and `same_binary`, true when that is the binary just replaced. Those services switch before their next pass; the others are left alone. `update` runs no `launchctl` or `systemctl` command. |

| Code | Cause |
| --- | --- |
| 0 | Updated (also with `warnings`), already current, or `--check` done. A skipped tray still exits 0. |
| 2 | Invalid flags. |
| 3 | GitHub could not be reached or answered with an error, including its rate limit; there is no stable release; the release has no archive for this platform or no `SHA256SUMS` line for it; a checksum mismatch; a bad archive; the new binary did not run; the installed binary changed during the update; no cache directory can be found (see [Files](#files)); or, without `--check`, the binary may not be replaced: the message names the reason and its fix. |
| 4 | The CLI was updated or already current, but updating the `mailtriage-tray` next to it failed (see [The tray next to the CLI](#the-tray-next-to-the-cli)). |
| 5 | Another update held the installation lock for 60 seconds (`another update is running`). |

## The tray next to the CLI

When a regular file `mailtriage-tray` (not a symlink) is in the same directory as the CLI, `mailtriage update` updates it in the same run, after the CLI. The CLI's directory is taken with symlinks resolved.

The tray update uses the same installation lock, download rules and `SHA256SUMS`, and the tray has its own entry in the cache:

1. `update` runs `mailtriage-tray --version`. When that fails, for example on a Linux host without the tray's GTK or AppIndicator libraries, the tray is skipped. Nothing is downloaded, and a skipped tray is not a failure.
2. When the release is newer than the tray's own version, `update` installs `mailtriage-tray-vX.Y.Z-PLATFORM.tar.gz` with the same steps as the CLI. The new tray must print `mailtriage-tray X.Y.Z`, and the old one is kept as `mailtriage-tray.previous`. A tray that is already current is left alone, even when the release has no tray archive.
3. A failure leaves the old tray in place and does not undo the CLI. `watch` retries the tray after 1 hour, doubling up to 24 hours, and `update` exits 4.

The CLI comes first. When its part fails, `update` exits as in the table above without trying the tray.

A tray that mailtriage may not replace (see [Binaries mailtriage does not replace](#binaries-mailtriage-does-not-replace)) fails the tray part.

`update`'s result adds `tray`:

```json
{"schema_version":1,"update":{"action":"updated","from":"0.2.0","to":"0.3.0","path":"/Users/alice/.local/bin/mailtriage","previous_path":"/Users/alice/.local/bin/mailtriage.previous","warnings":[],"services":[],"tray":{"action":"updated","from":"0.2.0","to":"0.3.0","error":null}}}
```

| `tray.action` | `from`, `to` and `error` |
| --- | --- |
| `updated` | The tray's version before and after; `error` is `null`. |
| `current` | Both the tray's version: the release is not newer. |
| `skipped` | `null`, `null`, and `mailtriage-tray does not run here: …` with the cause. |
| `failed` | The tray's version, the release's version, and why, for example `checksum mismatch for …` or `release v0.3.0 has no tray archive for linux-arm64`. The result then also has `"partial": true` at the top level, and `update` exits 4. |

`update --check`, `service status` and `doctor` add `"tray":{"path":"/Users/alice/.local/bin/mailtriage-tray","installed":"0.2.0","available":true}`:

- `path`: the tray file.
- `installed`: what it prints for `--version`, or `null` when it does not run.
- `available`: whether the release is newer.

`service status` and `doctor` look next to the binary the service runs and read only the cache. Their `available` is `false` while the cached release was checked by a mailtriage that did not know the tray; `watch` then checks again early.

The CLI's `available` still means the CLI alone. Without a tray file there is no `tray`.

## In the background

All of this happens in `watch`; no other command checks for updates by itself. Each pass runs, in order, the restart check (see [How a running service switches](#how-a-running-service-switches)), the update step, and then the pass.

The update step never fails, ends or changes a pass. Its problems are printed as `{"schema_version":1,"update":{"event":"error","message":"…"}}`, one JSON line with `--json`, else one text line.

The update step:

1. Reads `updates` from the config at every pass. When the file cannot be read or its `updates` is invalid, `watch` uses the mode it last stored for this config; with none, it skips the step.
2. With `off`, it stops there. With `notify` or `auto`, it refreshes the release information when the next check is due, about once a day: 24 hours after a successful check, plus up to an hour. Before the request it records the next check one hour ahead, so a `watch` that crashes or is killed during the request does not ask again sooner. A failed check is retried after 1 hour, doubling up to 24 hours (±10 %), or later when GitHub's `Retry-After` or rate-limit reset says so. When the cached release was checked by a mailtriage that did not know the tray, it checks once early, unless a failed check or a pending request holds the next one off.
3. With `auto`, when the cached release is newer than the installed binary, was checked at most 48 hours ago, and no earlier attempt waits for its retry, it installs the release as `mailtriage update` does. It tries the installation lock once; when another update holds it, it tries again at the next pass. It records the next attempt one hour ahead before it downloads, and a failed install waits 1 hour, doubling up to 24 hours. After an install, the restart check switches `watch` to the new binary before the pass. A `mailtriage-tray` next to the binary comes after it, under the same rules and with its own retry times, from the cached release only (see [The tray next to the CLI](#the-tray-next-to-the-cli)). After a switch to a new binary, the tray follows at a later pass. A tray that does not run here is skipped, and a tray failure is an `error` event.
4. With `notify`, or with `auto` when the binary may not be replaced, it prints once per release and config `{"schema_version":1,"update":{"event":"available","current":"0.2.0","latest":"0.3.0","release_url":"…"}}`. For `auto`, the event adds `install` with `reason` and `fix`.

The release information is shared by every `watch` of the same user: a `notify` watcher's check serves an `auto` watcher's install.

A cache that cannot be written stops the network work, and `watch` prints one event about it.

## Status

`mailtriage service status --account NAME` and `mailtriage doctor --account NAME` include an `update` block for the binary the account's service runs (see [Service commands](./service.md#service-commands)); `doctor` adds `ready` and `fix`. With a `mailtriage-tray` next to that binary, the block has `tray` too (see [The tray next to the CLI](#the-tray-next-to-the-cli)).

Both read the cache and make no network call. `mailtriage update --check --json` asks GitHub now.

## How a running service switches

A running `watch` notices when its binary is replaced and switches to the new one by itself.

`watch` records which file it runs when it starts. Before each pass, and every 5 seconds while it waits, it compares that file with the binary at the same path (device, inode, size, modification and change time, mode).

When the binary was replaced, by `mailtriage update`, by another account's service, or by `cargo install` over the same path, `watch`:

1. runs `<binary> --version`, which must print `mailtriage <version>`;
2. checks that the file did not change again meanwhile;
3. prints `{"schema_version":1,"update":{"event":"restarting","pid":1234,"from":"0.2.0","to":"0.3.0"}}`;
4. replaces itself with the new binary, with the same arguments and environment. The process ID stays the same, so launchd and systemd see no change, and the account lock is free while this happens.

It never does this during a pass, and after Ctrl-C or SIGTERM it stops instead.

It works in every `updates` mode and needs no `service install`; moving the binary to another path does need `service install`.

A Homebrew install keeps each version in its own directory, and `brew upgrade` deletes the old one.

`watch` therefore also follows `$(brew --prefix)/opt/mailtriage/bin/mailtriage`: when that leads to another file than the running one, it runs `--version` on it and re-executes the `opt` path, between passes, as above.

When the new binary does not run, or the switch fails, `watch` prints one `{"schema_version":1,"update":{"event":"error","message":"…"}}` per file and kind of failure and keeps running the old code. It tries again after 1 minute, doubling up to 1 hour, or at once when the file changes again (for example after `chmod +x`).

On Linux, a process whose binary file was replaced uses its absolute `argv[0]` to find the path; when that does not exist either, `watch` prints one error event and does not switch.

Without `--json`, events are one line of text, for example `update: restarting onto 0.3.0 (was 0.2.0, pid 1234)`.

## Binaries mailtriage does not replace

| `install.reason` | When | `install.fix` |
| --- | --- | --- |
| `unsupported_platform` | No release archive is built for this system: only macOS arm64 and Linux amd64 and arm64 with glibc have one. | Build from source, or set `updates` to `off`. |
| `managed_by_homebrew` | The path has a `Cellar` directory followed by `mailtriage`. | `brew upgrade mailtriage` |
| `managed_by_nix` | The path starts with `/nix/store/`. | Update it through nix. |
| `unsafe_permissions` | The binary or its directory is not owned by you, or is writable by group or others. | Install mailtriage into a directory only you own and can write, such as `~/.local/bin`, or set `updates` to `notify`. |
| `not_writable` | You cannot create files in the binary's directory. | Make the directory writable for you, or set `updates` to `notify`. |

::: warning Important
A binary installed with `sudo install … /usr/local/bin/mailtriage` belongs to root, so it is `unsafe_permissions`: mailtriage reports new releases for it but does not replace it.
:::

For automatic updates, install it as your own user into `~/.local/bin`, with [the install script](./install.md#the-install-script) or with the binary you have:

```sh
/usr/local/bin/mailtriage self install --dir ~/.local/bin
```

[`self install`](./install.md#mailtriage-self-install) says when `~/.local/bin` is not on your `PATH` and when another `mailtriage`, such as the root-owned one, comes first there.

Run `~/.local/bin/mailtriage service install --account NAME` again after you move the binary.

## Files

| File | Purpose |
| --- | --- |
| `update.json` in `~/Library/Caches/mailtriage` (macOS), or in `$XDG_CACHE_HOME/mailtriage` when `XDG_CACHE_HOME` is an absolute path, else `~/.cache/mailtriage` (Linux) | The last release check and when the next one is due, each config's mode and the last `available` event, and each installed binary's version and last install error. `update.lock` beside it guards it. Deleting it is safe; it is rebuilt. |
| `.mailtriage-update.lock` next to the binary | The installation lock. It is never deleted. |
| `.mailtriage-update-*` next to the binary | Temporary files of an update; the next update removes leftovers. |
| `<binary>.previous` | The binary before the last update; `mailtriage-tray.previous` for the tray. |

Without the cache directory (no `HOME`, and on Linux no absolute `XDG_CACHE_HOME`), `update` exits 3 and `watch` skips its update work.

## The first release with automatic updates

Copies older than the release that brought automatic updates cannot update themselves. Once:

1. Install the first release with automatic updates by hand into a directory you own: from its archive into `~/.local/bin` (see [Binaries mailtriage does not replace](#binaries-mailtriage-does-not-replace)), or with `cargo install` into `~/.cargo/bin`. Remove an older copy that you installed with `sudo` and no longer use, for example `sudo rm /usr/local/bin/mailtriage`, so it does not come first on your `PATH`: `service install` records the binary that runs it, and a root-owned binary is never updated. `command -v mailtriage` must show the new path.
2. Run `mailtriage service install --account NAME` once for every account, so each service runs the new binary. The old processes have no restart rule and would keep running the old code.
3. After the next pass, check that `mailtriage service status --account NAME --json` shows the new version in `last_pass.version` and `update.replaceable: true`. When `replaceable` is `false`, `update.reason` says why.

From then on, mailtriage updates itself.

## When a release breaks `watch`

A release that fails before `watch` reaches its update step cannot repair itself. Install a newer release by hand:

1. Run `mailtriage update`. It needs no config, so it may work when `watch` does not.
2. If it does not run either, download and check the archive yourself, then move the binary into place. Set `VERSION` to the release to install and `PLATFORM` to `macos-arm64`, `linux-amd64` or `linux-arm64`. On Linux, use `sha256sum` in place of `shasum -a 256`. Replace `~/.local/bin/mailtriage` with the path of your installed binary:

   ```sh
   VERSION=0.3.1 PLATFORM=macos-arm64
   base="https://github.com/wir-drei-digital/mailtriage/releases/download/v$VERSION"
   curl -fLO "$base/mailtriage-v$VERSION-$PLATFORM.tar.gz" &&
     curl -fLO "$base/SHA256SUMS" &&
     shasum -a 256 --check --ignore-missing SHA256SUMS &&
     tar -xzf "mailtriage-v$VERSION-$PLATFORM.tar.gz" mailtriage &&
     mv mailtriage ~/.local/bin/mailtriage
   ```

   Running services switch to it before their next pass.

## Rolling back by hand

There is no rollback command; a bad release is normally fixed by a newer one. To go back to the binary before the last update:

1. Set `updates` to `off` in every config (`mailtriage setup --update --updates off`, or edit the file), so the services do not install the newer release again.
2. Stop the services: `mailtriage service uninstall --account NAME` for each account.
3. `mv <binary>.previous <binary>`
4. Start them again: `mailtriage service install --account NAME`.

::: warning Important
Rolling back works only when the newer release did not migrate the state database. An older binary refuses a newer database (`database schema is newer than this binary`); then roll forward to a fixed release instead.
:::
