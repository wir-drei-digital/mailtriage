# Install mailtriage

The install script is the quickest way to get started. You need macOS on Apple silicon, or Linux on amd64 or arm64. Linux release binaries use glibc; see the [release requirements](../development/releases.md#publish-a-version).

For real mail, have an IMAP account and an [OpenRouter API key](https://openrouter.ai) ready. Setup can install Himalaya, the tool mailtriage uses to read your mailbox.

## The install script

Run this in a terminal:

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/wir-drei-digital/mailtriage/main/install.sh | sh
```

The script downloads the latest release, checks its checksum and installs it in `~/.local/bin`. It then offers to run setup. Accept to [connect your mailbox](./setup.md).

The tray app is included by default on macOS and on Linux desktops. To skip it, add `-s -- --no-tray` after `sh`.

Check that the command is available:

```sh
mailtriage --version
```

If your shell says “command not found”, follow the installer's instructions to add `~/.local/bin` to your `PATH`, then open a new terminal.

The installer does not need `sudo`. Installs owned by you can [update automatically](./updates.md) while the background service runs.

::: info macOS
The executables are unsigned and not notarized.
:::

## Homebrew

If you already use Homebrew:

```sh
brew install wir-drei-digital/tap/mailtriage
mailtriage setup
```

Use `brew upgrade mailtriage` for updates. Setup uses a tested Himalaya already on your machine or offers to install its own copy.

## From source

From a repository checkout with a stable Rust toolchain:

```sh
cargo build --release --locked
install -d ~/.local/bin
install -m 0755 target/release/mailtriage ~/.local/bin/mailtriage
```

Make sure `~/.local/bin` is on your `PATH`, then run `mailtriage setup`. Install the binary in its final location before starting the background service.

## Uninstall

For an install made with the script:

```sh
mailtriage self uninstall
```

It asks for confirmation, stops and removes this installation's services and tray login item, and removes its executables. Your configuration, local state, logs and private Himalaya stay on disk. Your mail is untouched.

For Homebrew, remove each account's service first:

```sh
mailtriage service uninstall --account work
```

On macOS, also run `mailtriage-tray autostart disable`. Then run `brew uninstall mailtriage`.

See the [installation reference](../reference/install.md) for custom directories, unattended installs, checksums and exit codes.

**Next: [Set up your mailbox](./setup.md).**

## Detailed reference

- <span id="mailtriage-self-install"></span>[mailtriage self install](../reference/install.md#mailtriage-self-install)
