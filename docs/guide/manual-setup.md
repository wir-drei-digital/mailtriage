# Manual setup

Write the configuration yourself when you want full control, for example on a server. You set up Himalaya, create the configuration, provide the key, check the result and run a first pass. Install mailtriage first ([Install](./install.md)).

## 1. Set up Himalaya

mailtriage runs the `himalaya` executable for every mailbox operation. It accepts only the [tested versions](./himalaya.md) with IMAP support: the first line of `himalaya --version` must start with `himalaya v` and a tested version, such as `himalaya v2.2.1`, and contain `+imap`.

Install one from [Himalaya's releases](https://github.com/pimalaya/himalaya/releases) or a package manager, then check:

```sh
himalaya --version
```

Create an account with `himalaya configure`, or write a Himalaya configuration file with one account. mailtriage uses only the account's `imap` settings; it never sends mail, so no `smtp` settings are needed. A minimal file, for example `~/.config/himalaya/config.toml`:

```toml
[accounts.work]
email = "alice@example.org"
imap.server = "imaps://imap.example.org:993"
imap.sasl.plain.username = "alice@example.org"
imap.sasl.plain.password.cmd = "security find-generic-password -s imap.example.org -a alice@example.org -w"
```

- `[accounts.work]`: the account name. `engine.account` in `mailtriage.json` refers to it.
- `email`: the address. `mailtriage setup` uses it as the default identity.
- `imap.server`: `imaps://HOST:993` for TLS. A bare `HOST` also means `imaps://`. For STARTTLS on port 143, use `imap://HOST:143` with `imap.starttls = true`.
- Credentials use Himalaya's own mechanism. `password.cmd` (also spelled `password.command`) runs a command that prints the password. The example reads it from the macOS keychain; store it there once with `security add-generic-password -s imap.example.org -a alice@example.org -w`, which prompts for the password. On Linux, a command such as `pass show mail/work` works the same way. `password.raw` stores the password in the file; avoid it. For OAuth servers, Himalaya offers `imap.sasl.oauthbearer` and `imap.sasl.xoauth2`; see [Himalaya's sample configuration](https://github.com/pimalaya/himalaya/blob/v2.1.0/config.sample.toml).
- mailtriage starts Himalaya with no terminal input and passes on its own environment. The password command must not prompt, and the commands it calls must be found on that environment's `PATH`.

::: warning Important
Do not add a `mailbox.alias` entry, globally or for this account, that maps a watched folder or category folder name to a different mailbox. mailtriage never reads mail from such a folder, whatever the filing mode; its mail waits until the alias is removed. With filing on, it also stops scanning the folder and makes no filing writes. See [Folder names Himalaya resolves](./himalaya.md#folder-names-himalaya-resolves).
:::

Check the login and the folder names. These commands contact the server:

```sh
himalaya --config ~/.config/himalaya/config.toml --account work account check
himalaya --config ~/.config/himalaya/config.toml --account work imap list --all
himalaya --config ~/.config/himalaya/config.toml --account work --json imap status INBOX
```

`account check` prints whether the IMAP login works. It exits 0 even when the check fails, so read its output.

The second command lists every folder; use these exact names in `engine.mailboxes`. The third must print `uid_validity` and `uid_next`, which mailtriage depends on.

mailtriage discards Himalaya's error output, so use these commands to see the cause of an IMAP error later.

## 2. Create the configuration

```sh
mailtriage init --config ~/.config/mailtriage/mailtriage.json --json
```

`init` writes a starting file with the `fake` provider, one account named `work` without an engine, and `state_dir` `.state`. Edit it as described in [Configuration](./configuration.md) before you run any other command against the account.

The first command that opens an account stores its binding: `identity`, the Himalaya account name, `imap.server` and the other non-secret IMAP settings. A later change to any of them is refused with exit code 5 (see [Account binding](./reference.md#account-binding)).

## 3. Provide the OpenRouter key

Set `provider.api_key_command` to a command that prints the key, or set `provider.api_key_env` and the variable it names. On macOS, store the key in the Keychain (the command prompts for it):

```sh
security add-generic-password -U -s mailtriage -a openrouter -w
```

Then set the read command in `mailtriage.json`:

```json
"api_key_command": ["/usr/bin/security", "find-generic-password", "-s", "mailtriage", "-a", "openrouter", "-w"]
```

The other stores and the rules for the command are in [The OpenRouter key](./provider.md#the-openrouter-key).

## 4. Check the setup

```sh
mailtriage doctor --account work --json
```

Run it in the same environment as the command you are checking. `doctor` exits 0 whether or not the account is ready, so read the fields:

Look for `ready: true`, `transport.configured: true` and `provider.key_present: true`. If a check fails, read `provider.key_error` or `transport.error`.

::: details All doctor fields
| Field | Value when ready | Meaning |
| --- | --- | --- |
| `ready` | `true` | The provider configuration is valid, the key is present, and the Himalaya check passed. |
| `provider.kind`, `provider.model` | your values | The configured provider. |
| `provider.configuration_valid` | `true` | The provider block passes validation. |
| `provider.key_source` | `command` or `env` | Where the key comes from: `api_key_command` when set, else `api_key_env`. `null` for `fake`. |
| `provider.key_present` | `true` | The key command printed a key, or the variable named in `api_key_env` is set and not blank in this process. Always `true` for `fake`. |
| `provider.key_error` | absent | Present only when the key is missing: one of the fixed messages in [Key command rules](./provider.md#key-command-rules), or `OpenRouter API key environment variable is missing`, or `OpenRouter API key environment variable is empty`. |
| `transport.configured` | `true` | The account has an `engine`. Without one, `transport.ready` is `true` as well. |
| `transport.ready` | `true` | The Himalaya configuration file was read and `himalaya --version` reported a [tested version](./himalaya.md) with `+imap`. Otherwise `transport.error` is set. |
| `transport.version` | `himalaya v2.2.1 ...` | The first line of `himalaya --version`. |
| `transport.tested` | `true` | The version is one mailtriage is tested with. `false` makes the transport not ready, with `transport.error` `Himalaya X is not a tested version (tested: 2.1.0, 2.2.1)`. |
| `transport.alias_conflicts` | `[]` | Source folders that Himalaya would resolve to another mailbox; mailtriage reads no mail from them. When the Himalaya configuration cannot be parsed, `transport.ready` is `false` and `transport.error` says why: no folder is read. |
| `live_checks_performed` | `false` | Always `false`. |
| `update.ready` | `true` | `update` is the block [`service status`](./service.md#service-commands) shows, plus `ready`: `false` only when `updates` is `auto` and the binary may not be replaced, and then `fix` says what to do. The top-level `ready` ignores it. |

:::

`doctor` runs the key command to check it, so on macOS the first run may show a Keychain access dialog. It makes no provider request.

It logs in to the IMAP server only when filing is on, to add a `filing` block with the server's capabilities, folders and problems. The first `sync` is therefore the first full test of the IMAP login and the API key.

## 5. First run

1. Run `doctor` (step 4) until `ready` and `transport.configured` are `true`.
2. Run one small pass:

   ```sh
   mailtriage sync --account work --limit 20 --json
   ```

   Exit code 0 with `scan_errors: 0` and `failed: 0` means the pass completed. Check that `classified` is greater than zero to confirm it exercised the provider. Exit code 4 means the pass was partial.

   A `classification` object with `skipped: true` means the key is unavailable; its `reason` says why, and `doctor` shows the same `key_error`.

   `scan_errors` counts folders that could not be scanned (see `coverage.scans[].error`), and `failed` counts messages whose fetch or classification failed (`list --view all` shows each message's `error`). Run the Himalaya check commands from step 1 to see an IMAP error.
3. Look at the result:

   ```sh
   mailtriage list --account work --view attention --json
   ```

4. Start `watch` in the background ([Background service](./service.md)), or in a terminal:

   ```sh
   mailtriage watch --account work --limit 100 --interval-seconds 60 --json
   ```

5. To move mail into category folders, follow the [filing rollout](./filing.md#rollout).

The first passes work through the existing mail in each watched folder, oldest UID first. Each pass scans at most `--limit` UIDs per folder and classifies at most `--limit` messages. Every classification is one OpenRouter request.
