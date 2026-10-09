# Keep mailtriage updated

An installation made with the install script can update itself while the background service runs. It checks for stable releases about once a day and switches versions between checks of your mail.

Homebrew installations use `brew upgrade mailtriage` instead. mailtriage reports available updates but does not replace a Homebrew-managed binary.

## Check or update now {#mailtriage-update}

```sh
mailtriage update --check
```

This checks GitHub without installing anything. To install the latest stable release:

```sh
mailtriage update
```

The command updates the executable you ran and the tray app beside it, if present. Running services pick up the new executable automatically. You do not need to reinstall the service unless you move the executable to a different path.

## Choose automatic updates {#modes}

The `updates` setting in `mailtriage.json` controls what `watch` does:

| Value | Behavior |
| --- | --- |
| `auto` | Checks and installs stable releases. This is the default. |
| `notify` | Checks and reports available releases without installing. |
| `off` | Does not check for releases. |

To change it through setup:

```sh
mailtriage setup --update --account work --updates notify
```

Or edit `updates` directly in the configuration. A manual `mailtriage update` works in every mode.

## If an update is unavailable {#binaries-mailtriage-does-not-replace}

`mailtriage update --check` reports `install.replaceable`, with a `reason` and `fix` when the executable cannot be replaced.

Common reasons are a package manager that owns the installation, a root-owned binary, or a directory you cannot write to. For automatic updates, install as your own user in `~/.local/bin`. See the [update reference](../reference/updates.md#binaries-mailtriage-does-not-replace) for every reason.

## If an update fails

A failed download or verification leaves the installed binary in place. Background checks retry later. If only the tray update failed, the command exits with code 4: the CLI may already be updated. Read `update.tray.error` and try again after fixing the cause.

If the service no longer runs, try `mailtriage update` directly. It needs no configuration. If that also fails, follow the [manual recovery steps](../reference/updates.md#when-a-release-breaks-watch).

## Rolling back by hand

Updates keep the previous executable as `mailtriage.previous`. There is no rollback command, and an older executable cannot open a database migrated to a newer schema.

Prefer installing a fixed release. If you must restore the previous executable, follow the [rollback procedure](../reference/updates.md#rolling-back-by-hand), including turning automatic updates off and stopping services first.

The [update reference](../reference/updates.md) covers checksums, release selection, cache files and JSON output.

## Detailed reference

- <span id="the-tray-next-to-the-cli"></span>[The tray next to the CLI](../reference/updates.md#the-tray-next-to-the-cli)
- <span id="in-the-background"></span>[In the background](../reference/updates.md#in-the-background)
- <span id="status"></span>[Status](../reference/updates.md#status)
- <span id="how-a-running-service-switches"></span>[How a running service switches](../reference/updates.md#how-a-running-service-switches)
- <span id="files"></span>[Files](../reference/updates.md#files)
- <span id="the-first-release-with-automatic-updates"></span>[The first release with automatic updates](../reference/updates.md#the-first-release-with-automatic-updates)
- <span id="when-a-release-breaks-watch"></span>[When a release breaks watch](../reference/updates.md#when-a-release-breaks-watch)
