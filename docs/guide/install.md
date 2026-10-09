# Install

This page gets mailtriage onto your machine. The quickest way is the install script: one line in a terminal, and from then on mailtriage keeps itself up to date.

You need:

- macOS arm64, Linux amd64 or Linux arm64,
- an IMAP account whose server supports UID and UIDVALIDITY (the MOVE extension too, if you want filing),
- an OpenRouter API key,
- for the background service: launchd (macOS) or systemd (Linux).

mailtriage reads mail through Himalaya, in a [tested version](./himalaya.md) with IMAP support. Setup installs one for mailtriage when none is found.

There are three ways to install, and each stays current differently:

| Way | Installs into | Updated by |
| --- | --- | --- |
| [The install script](#the-install-script) | `~/.local/bin` | mailtriage itself ([Updates](./updates.md)) |
| [Homebrew](#homebrew) | Homebrew's prefix | `brew upgrade mailtriage` |
| [From source](#from-source) with Cargo | where you put it | you |

::: warning Important
The macOS executables are unsigned and not notarized. See the [release guide](../development/releases.md) for how releases are made.

A root-owned or otherwise unsafe install, for example one made with `sudo` into `/usr/local/bin`, is not replaced: mailtriage only reports new releases for it (see [Binaries mailtriage does not replace](./updates.md#binaries-mailtriage-does-not-replace)).
:::

## The install script

One command downloads mailtriage, checks it and installs it, then offers to set it up:

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/wir-drei-digital/mailtriage/main/install.sh | sh
```

The script downloads the newest release's archive and `SHA256SUMS` over HTTPS from GitHub. It checks the archive's checksum and that it holds exactly `mailtriage`, `LICENSE` and `README.md`.

Then it hands over to the downloaded binary's [`mailtriage self install`](#mailtriage-self-install), which installs it into `~/.local/bin` and offers setup.

It needs `curl`, `tar`, `mktemp`, `uname`, and `sha256sum` or `shasum`. It never uses `sudo`, never edits your shell's startup files, and never touches a `himalaya`. Each release also carries `install.sh` with that release as its default version.

Options follow `sh -s --`, for example:

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/wir-drei-digital/mailtriage/main/install.sh | sh -s -- --version 0.3.0 --no-tray
```

| Option | Environment | Meaning |
| --- | --- | --- |
| `--version X.Y.Z` | `MAILTRIAGE_VERSION` | Install this release instead of the newest. Never a downgrade. |
| `--dir DIR` | `MAILTRIAGE_INSTALL_DIR` | The install directory, default `~/.local/bin`. A leading `~/` or a bare `~` that reaches the script unexpanded means your `HOME`. |
| `--tray` / `--no-tray` | `MAILTRIAGE_TRAY=1` / `0` | Install the tray app, or not. Default: yes on macOS; on Linux only when `DISPLAY` or `WAYLAND_DISPLAY` is set. |
| `--no-setup` | `MAILTRIAGE_NO_SETUP=1` | Do not offer setup or the login item. |
| `--yes` | `MAILTRIAGE_YES=1` | Never ask. |
| `--uninstall` | | Uninstall the installation in `DIR`; see [Uninstall](#uninstall). |

It asks only when it can open your terminal:

| Question | Terminal | `--yes` | No terminal |
| --- | --- | --- | --- |
| Downgrade below the installed version | refused (exit 2) | refused (exit 2) | refused (exit 2) |
| Run setup now | asks, default yes | no; prints the command | no; prints the command |
| Install a private Himalaya (inside setup) | asks, default yes | not reached | not reached |
| Start the tray at login (after setup succeeded) | asks, default yes | no; prints the command | no; prints the command |
| Uninstall | asks, default no | yes | refused (exit 2; use `--yes`) |

The end of input at a question counts as its default.

Exit codes:

- **0**: done.
- **1**: a failed step, which the message names: a missing tool, a network error, a checksum mismatch, an unexpected archive or latest-release URL.
- **2**: invalid options, an unsupported platform, or a refused downgrade.
- **126**: the downloaded program could not be run, usually because the temporary directory is mounted `noexec`. Run the script again with `TMPDIR=<a directory that allows running programs>` before `sh`.
- Any other code is `mailtriage self install`'s.

A download that breaks off runs nothing: the whole script is one `{ … }` group, which `sh` reads completely before running it.

The script never downgrades. To go back to an older release, follow [Rolling back by hand](./updates.md#rolling-back-by-hand).

## Homebrew

If you manage your tools with Homebrew, install mailtriage from the wir-drei-digital tap:

```sh
brew install wir-drei-digital/tap/mailtriage
```

The formula in [wir-drei-digital/homebrew-tap](https://github.com/wir-drei-digital/homebrew-tap) installs the release archive for macOS arm64, with `mailtriage-tray`, or for Linux amd64 or arm64.

It has no Himalaya dependency: `mailtriage setup` uses a tested `himalaya` on your `PATH` or installs a private one. Then:

- **Updates**: `brew upgrade mailtriage` installs new releases. For a Homebrew install mailtriage only reports them (`managed_by_homebrew`).
- **Services and the tray**: the background service and the tray's login item record `$(brew --prefix)/opt/mailtriage/bin/…`, which `brew upgrade` keeps pointing at the current version. Running services and the tray switch to it by themselves.
- **Uninstall**: run `mailtriage service uninstall --account NAME` for each account, then on macOS `mailtriage-tray autostart disable` (the login item names the `opt` path that `brew uninstall` removes), then `brew uninstall mailtriage`. `mailtriage self uninstall` refuses a Homebrew install.

## From source

If you'd rather build mailtriage yourself, you need a stable Rust toolchain:

```sh
cargo build --release --locked &&
  install -d ~/.local/bin &&
  install -m 0755 target/release/mailtriage ~/.local/bin/mailtriage &&
  mailtriage --version
```

`~/.local/bin` must be on your `PATH`; the examples below assume `mailtriage` is. If `command -v mailtriage` prints nothing, add `export PATH="$HOME/.local/bin:$PATH"` to your shell profile (`~/.zprofile` on macOS, `~/.bashrc` on Linux) and open a new terminal.

The background service records the absolute path of the executable that installs it, so install the binary in its final place first.

## `mailtriage self install`

The install script runs this command. You or an agent can run it on a binary you placed yourself, and it installs that binary the way the script would:

```sh
mailtriage self install --dir DIR [--tray-file PATH] [--no-setup] [--yes] [--json]
```

1. It creates `DIR` and its missing parents with mode 0755. `DIR`, with symlinks resolved, must be yours and not writable by group or others, and each of its parents must pass the rule of [a private Himalaya's](./himalaya.md#a-private-himalaya) directory. Otherwise it exits 3 with `unsafe_permissions` and the fix: `chmod go-w DIR`, or another `--dir`.
2. It never goes back: when `DIR/mailtriage` prints a newer version, it exits 2 and points to [Rolling back by hand](./updates.md#rolling-back-by-hand).
3. It installs this binary as `DIR/mailtriage` the way `mailtriage update` installs a release: under the installation lock, from a new file that must run with `--version`, keeping the old binary as `DIR/mailtriage.previous`, with a rename. Running services switch to it by themselves. When `DIR/mailtriage` already prints this version (a reinstall), it is still replaced, but an existing `DIR/mailtriage.previous` stays as it is, so the release before it remains there for [rolling back by hand](./updates.md#rolling-back-by-hand); the tray's `.previous` is kept the same way.
4. With `--tray-file`, it installs that `mailtriage-tray` next to it the same way; it must be the same version. A tray that does not run here (on Linux without GTK, for example) is skipped, any existing tray is kept, and on Linux it names the packages the tray needs.
5. It records both in the update cache, clearing any earlier install error.
6. It says when `DIR` is not on your `PATH`, with the line to add for your shell (zsh, bash, fish, or a POSIX `export`), and when another `mailtriage` comes first on your `PATH`: that one stays in use and is not updated by this install.
7. With a terminal, unless `--no-setup` or `--yes`, it asks `Run mailtriage setup now? [Y/n]` and runs `DIR/mailtriage setup`, so the service it installs runs `DIR/mailtriage`. After a successful setup with the tray installed, it asks `Start the tray at login? [Y/n]` and runs `DIR/mailtriage-tray autostart enable --config CONFIG --mailtriage DIR/mailtriage` with the config setup wrote. Otherwise it prints those commands.

```json
{"schema_version":1,"self_install":{"dir":"/Users/alice/.local/bin","cli":{"action":"installed","version":"0.3.0","path":"/Users/alice/.local/bin/mailtriage"},"tray":{"action":"installed","error":null},"on_path":true,"shadowed_by":null,"setup":"ran","autostart":"enabled"}}
```

- `tray`: `null` without `--tray-file`, else `action` `installed`, `skipped` or `failed`, with the reason in `error`.
- `setup`: `ran`, `skipped` or `failed`. Choosing "Abort" in setup's menu (a re-run with an existing config) changes nothing and counts as `skipped`, not as a failed setup. `autostart`: `enabled`, `skipped` or `failed`.

Exit codes:

- **0**: done.
- **2**: invalid flags or a refused downgrade.
- **3**: an unsafe directory, a failed install, a failed tray install, or a failed setup (the binaries stay installed).
- **5**: another update or install held the installation lock for 60 seconds.

## Uninstall

A Homebrew install has its own steps; see [Homebrew](#homebrew). Either command removes any other installation again:

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/wir-drei-digital/mailtriage/main/install.sh | sh -s -- --uninstall
mailtriage self uninstall [--dir DIR] [--yes] [--json]
```

The script's `--uninstall` runs `DIR/mailtriage self uninstall --dir DIR`, with `--yes` when given, and downloads nothing. It asks through your terminal like the install does.

`self uninstall` removes the installation in `DIR`, by default the directory of the `mailtriage` that runs it, and nothing else:

1. A Homebrew install is refused: `installed by Homebrew; run brew uninstall mailtriage` (exit 2). Without `--yes` it asks first (default no); without a terminal it needs `--yes` (exit 2).
2. It takes the installation lock, waiting up to 60 seconds (else exit 5), so no update recreates the binaries meanwhile.
3. It uninstalls every background service whose executable is `DIR/mailtriage`, as `service uninstall` does, under each account's service lock. Services of other installations, Homebrew's included, stay.
4. It removes the tray's login item when that starts `DIR/mailtriage-tray` (on macOS it also stops the login job), then runs `DIR/mailtriage-tray quit`. Close any open categories window yourself.
5. Only when all of that worked, it deletes `DIR/mailtriage-tray`, the `.previous` copies and then `DIR/mailtriage`, and the update cache's entry of each program it deleted. When a step failed, it deletes no program file (services and a login item it already removed stay removed), lists the failures and exits 3. When a file cannot be deleted, it keeps `DIR/mailtriage`, so you can run `self uninstall` again, lists the failure and exits 3.

It keeps the installation lock file and the private Himalaya under `~/.local/share/mailtriage/himalaya` (other configs may use it; it prints how to delete it). It also keeps your config, state and logs (`~/.config/mailtriage/` by default). No mail is touched.

```json
{"schema_version":1,"self_uninstall":{"dir":"/Users/alice/.local/bin","services":[{"account":"work","unit_path":"/Users/alice/Library/LaunchAgents/digital.wirdrei.mailtriage.work.plist","action":"uninstalled","error":null}],"tray":"quit","removed":["/Users/alice/Library/LaunchAgents/digital.wirdrei.mailtriage-tray.plist","/Users/alice/.local/bin/mailtriage-tray","/Users/alice/.local/bin/mailtriage.previous","/Users/alice/.local/bin/mailtriage"],"kept":["/Users/alice/.local/bin/.mailtriage-update.lock","/Users/alice/.config/mailtriage"],"failures":[]}}
```

- `services[].action`: `uninstalled`, `skipped` (by the time its lock was held, the file named another executable) or `failed`, with `error`.
- `tray`: `quit`, `not_running`, `other_installation` (another installation's tray, which keeps running), `not_installed`, or `failed`.

Exit codes:

- **0**: done.
- **2**: a Homebrew install, a refused confirmation, no terminal without `--yes`, or `HOME` not set (nothing is changed).
- **3**: a failed service or tray step (no program file was deleted) or a file that could not be deleted (`DIR/mailtriage` stays).
- **5**: the installation lock was held for 60 seconds.
