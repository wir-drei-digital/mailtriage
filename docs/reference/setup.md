# Guided setup reference

For a walkthrough, see [guided setup](../guide/setup.md). Use this page to look up flags, output fields and detailed behavior.

Guided setup gets you from a fresh install to a checked configuration by asking you questions, one setting at a time:

```sh
mailtriage setup
```

`setup` asks one question per setting, writes the configuration, checks it and can install the background service. Every answer can also be given as a flag, so it runs without prompts too.

What setup does and never does:

- **Where it writes**: `~/.config/mailtriage/mailtriage.json`, or the path in `--config` or `MAILTRIAGE_CONFIG`. It never uses `./mailtriage.json`.
- **State**: a new config keeps its state in `state/` next to the config file (`~/.config/mailtriage/state`).
- **IMAP settings**: setup never writes them; Himalaya's own wizard does that.
- **Your mailbox**: setup makes no change on the IMAP server.
- **Your OpenRouter key**: setup never asks for, prints or stores it. The key store's own tool asks for it; setup only checks that the read command prints a key.
- **Filing**: setup never turns on `live` filing.

## The ten steps

| Step | What happens | Flags |
| --- | --- | --- |
| 1. Config | If the config exists: update an account, add an account, or abort. Other accounts are never changed. | `--config`, `--update`, `--account` |
| 2. Himalaya | Finds the Himalaya binary, its config file and the account, and runs `himalaya account check`. Can install a tested Himalaya for mailtriage, and create an account with `himalaya configure`. | `--himalaya-binary`, `--himalaya-install`, `--himalaya-config`, `--himalaya-account` |
| 3. Account | The account name in mailtriage, your address, time zone and a one-line brief. | `--account`, `--identity`, `--timezone`, `--brief` |
| 4. Folders | Lists the server's folders; you choose which to watch. | `--mailbox` |
| 5. Classifier | OpenRouter or the offline `fake` provider, the model, and where the key lives. | `--provider`, `--model`, `--key-store`, `--key-command`, `--key-env`, `--key-stored` |
| 6. Categories | Six default categories for a new account. | none |
| 7. Filing | `dry_run` or `off`. | `--filing` |
| 8. Write | Validates and writes the config, including `updates`. | `--updates` |
| 9. Check | Runs `doctor` and prints each item with its fix. | none |
| 10. Service | Offers to run `watch` in the background. | `--service`, `--interval-seconds`, `--limit` |

**Step 1, config.**

- With prompts, an existing config shows a menu: update an account, add an account, or abort.
- With prompts, `--account NAME` answers that menu. An existing name is updated; a new name is added.
- Without prompts, an existing config needs `--update`; otherwise setup exits 5.
- `--update --account NAME` updates `NAME` if it exists and adds it otherwise. Without `--account`, the name defaults to the Himalaya account name (step 3).

**Step 2, Himalaya.**

- Binary: `--himalaya-binary`, else the stored binary of the account being updated, else the first `himalaya` on `PATH`. Setup stores it as an absolute path. Its `--version` must report a [tested version](../guide/himalaya.md) with `+imap`. Setup writes the version it found into `expected_version`.
- When that Himalaya is missing or untested, setup says why and asks `Install Himalaya 2.2.1 for mailtriage? [Y/n]` (default yes). It then installs a [private Himalaya](../guide/himalaya.md#a-private-himalaya) and uses it. Without prompts, `--himalaya-install` answers yes; otherwise setup exits 3 with the fix `run mailtriage himalaya install, then mailtriage setup --update --account NAME --himalaya-account NAME --himalaya-binary PATH`, naming the accounts known at that point (`--account` when given or being updated, `--himalaya-account` when given) and `--config` when needed. With a tested Himalaya found, `--himalaya-install` changes nothing.
- When the chosen Himalaya is in a Homebrew keg (its path with symlinks resolved contains `/Cellar/himalaya/`), setup prints once: `Homebrew may upgrade Himalaya to a version mailtriage has not tested; "brew pin himalaya" holds it, or run mailtriage himalaya install for a private copy.`
- Config file: `--himalaya-config`, else the stored file of the account being updated, else `HIMALAYA_CONFIG`, else Himalaya's default. `HIMALAYA_CONFIG` must name one file; several `:`-separated files exit 2.
- Himalaya's default is the first existing file of: `~/Library/Application Support/himalaya/config.toml` on macOS, or `$XDG_CONFIG_HOME/himalaya/config.toml` on Linux when `XDG_CONFIG_HOME` is an absolute path; then `~/.config/himalaya/config.toml`; then `~/.himalayarc`.
- Account: setup offers only accounts with an IMAP backend. The default is the account being updated, else Himalaya's default account.
- The menu's last entry runs `himalaya configure`, Himalaya's own wizard, attached to your terminal. Setup also offers it when no IMAP account exists. See [Set up Himalaya](../guide/manual-setup.md#_1-set-up-himalaya) to write the file yourself.
- Without prompts, `--himalaya-account` is required, and `himalaya configure` never runs. An account updated by `--account NAME` keeps its stored Himalaya account instead. Setup refuses to change the Himalaya account of a bound account (exit 5); see [Updating an account](#updating-an-account).
- Check: setup runs `himalaya account check` for the IMAP backend. If it fails, setup exits 3 and prints the command that shows why. It never prints Himalaya's output.

**Step 3, account details.**

- `--account`: default the Himalaya account name. 1 to 64 ASCII letters, digits, `-` or `_`; the name is used in service and log file names. An added account needs a new name.
- `--identity`: your address. Default the account's `email` in the Himalaya file.
- `--timezone`: an IANA name such as `Europe/Zurich`. Default `TZ`, then the zone `/etc/localtime` points to, then `UTC`.
- `--brief`: optional. One line about you that helps classification. It is sent to the provider with every message.
- When you update an account, its current values are the defaults. Its identity is bound after the first check, and setup refuses to change it (exit 5); see [Updating an account](#updating-an-account).

**Step 4, folders.**

- Setup lists the account's folders through Himalaya. Folders marked `\Noselect` are left out.
- The default is `INBOX`, or the account's current folders when you update it. With prompts, enter numbers separated by commas.
- `--mailbox NAME`, repeated for several folders, must name a folder on the server; otherwise setup exits 2.
- With filing on, watched folders must be printable ASCII without `\`, `"` or `&` and must not start with `-`.

**Step 5, classifier.**

- `--provider openrouter` (default) or `--provider fake` (offline keyword rules, no key).
- `--model`: any non-empty Decisions model ID. The default is `typesafe/jev-latest` for a new classifier and the current model when you update one; see [Provider](./provider.md).
- The key: see [Key stores](#key-stores).
- On an existing config without classifier flags, setup asks "Keep the current classifier?" (default yes). Without prompts it keeps the classifier. The classifier flags are `--provider`, `--model`, `--key-store`, `--key-command`, `--key-env` and `--key-stored`.
- Only a key flag (`--key-store`, `--key-command`, `--key-env`, `--key-stored`) changes where an OpenRouter key comes from. `--model` or `--provider openrouter` alone keep `api_key_command` and `api_key_env` as they are and skip the key question. If you answer no to "Keep the current classifier?", the key store menu offers the current store as its default.
- Key flags with `--provider fake` exit 2: the offline classifier needs no key.
- All accounts in a config share one provider, so there is one key per config.

**Step 6, categories.** A new account gets six categories: Correspondence, Transactions, Updates, Newsletters, Promotions and Other (the catch-all). Each has `folder` set to its own name.

An updated account keeps its categories. Once the config is written, setup prints the commands to change them; see [Categories](../guide/categories.md).

**Step 7, filing.** `--filing dry-run` plans moves and flags and writes nothing; it is the default for a new account, and an updated account defaults to its current mode. `--filing off` only classifies.

Setup never selects `live`. An account that is already `live` stays `live` unless you pass `--filing`. To go live, follow the [rollout](./filing.md#rollout).

**Step 8, write.** `--updates auto|notify|off` sets [`updates`](./updates.md), with no prompt: a new config gets `auto`, and an existing one keeps its value unless `--updates` is given.

Setup validates the whole config and writes it atomically with mode 0600, under the configuration lock (`mailtriage.lock`). The state directory is created with mode 0700. Setup writes nothing when:

- another command holds that lock: exit 5 (reason `config_busy`);
- the file changed since step 1 read it: exit 5 (reason `config_changed`);
- the result is invalid: exit 2;
- the state directory already holds a binding for the account and the new answers would change it: exit 5; see [Updating an account](#updating-an-account).

After `config_busy` or `config_changed`, run it again.

**Step 9, check.** Setup runs `doctor` for the account. It prints each item (`provider`, `key`, `mail`, `filing` when filing is on, and `update` when `updates` is `auto` but the binary may not be replaced) as `ok`, or as `not ready` with the one command that fixes it.

The `update` item does not make `doctor.ready` false, as in `doctor` itself. If `doctor` itself fails, for example because the state database cannot be opened, setup reports a single not-ready `state` item instead.

For an untested Himalaya, the `mail` item's fix is `run mailtriage himalaya install, then mailtriage setup --update --account NAME --himalaya-binary PATH`, with the path that command installs to and `--config` when needed. Setup exits 0 even when an item is not ready.

**Step 10, service.** With prompts on macOS or Linux, setup asks whether to run `watch` in the background (default yes); on other platforms it skips this step.

When the key comes from an environment variable, the default is no, because the service does not inherit the variable; setup says so and prints the command that moves the key into a key store.

Without prompts, `--service install` installs it and `--service skip` (the default) does not. `--interval-seconds` (1 to 86400, default 60) and `--limit` (1 to 500, default 100) are passed to `watch`.

When step 9 reported a not-ready `state` item, setup installs no service, since every pass would fail; it prints the `service install` command to run once that is fixed. See [Background service](./service.md).

**Next steps.** Setup ends with the commands to run next: `sync` and `watch`, or, with the service installed, `service status` and `list` (`sync` or `watch` would compete with the service for the account lock).

Every `mailtriage` command setup prints passes `--config` when a command run in the same directory and environment without it would find another config or none. When a `./mailtriage.json` in the working directory would win over the written config, setup also prints a warning.

## Key stores

The key never goes into `mailtriage.json`. Setup records either a command that prints the key (`provider.api_key_command`) or the name of an environment variable (`provider.api_key_env`). On macOS, for example, the key stays in your Keychain, and the config holds only the command that reads it from there.

The menu lists the stores available on this machine, in this order. The first one is the default, also without prompts. When you change an existing OpenRouter classifier with prompts, the menu's default is the store the key comes from now (`command` for a command of your own, `env` for a variable).

| `--key-store` | Offered | Store command (the tool asks for the key) | Read command, saved as `api_key_command` |
| --- | --- | --- | --- |
| `keychain` | on macOS | `security add-generic-password -U -s mailtriage -a openrouter -w` | `security find-generic-password -s mailtriage -a openrouter -w` |
| `secret-service` | when `secret-tool` is on `PATH` | `secret-tool store --label='mailtriage OpenRouter key' service mailtriage provider openrouter` | `secret-tool lookup service mailtriage provider openrouter` |
| `pass` | when `pass` is on `PATH` | `pass insert mailtriage/openrouter` | `pass show mailtriage/openrouter` |
| `command` | always | none | `["/bin/sh", "-c", "COMMAND"]` from `--key-command 'COMMAND'` |
| `env` | always | none | none; sets `api_key_env` from `--key-env NAME` |

For `keychain`, `secret-service` and `pass`:

1. Setup runs the read command. If it prints a key, setup offers to use it (default yes). Without prompts it uses it.
2. Otherwise setup runs the store command. The tool asks for the key on your terminal. mailtriage never sees the key, and the tool's output is not shown.
3. Setup runs the read command again. If it prints no key, setup exits 3.
4. Setup saves the read command, with the tool's absolute path, in `api_key_command`.

Without prompts, a tool store needs one of these: a terminal for the tool's prompt (`--yes` run in a terminal), a key that is already stored, or `--key-stored`. Otherwise setup exits 2 and prints the store command to run first.

`--key-stored` says the key is already in the store. Setup then skips the question, saves and checks the read command, and exits 3 with the store command if no key is found. For example, on macOS:

```sh
security add-generic-password -U -s mailtriage -a openrouter -w
mailtriage setup --yes --himalaya-account work --key-store keychain --key-stored
```

For `command`, `--key-command 'COMMAND'` is any shell command that prints the key; `--key-command` alone implies `--key-store command`. Setup runs it once and requires a key. A command typed at the prompt is asked again after a failure; a failing `--key-command` exits 3.

For `env`, `--key-env NAME` names the variable; `--key-env` alone implies `--key-store env`. The default is the current `api_key_env`, else `OPENROUTER_API_KEY`.

Setup removes any `api_key_command` and warns when the variable is not set in its own environment. The variable must be set wherever mailtriage runs; see [Environment variable](./provider.md#environment-variable).

`--key-command` and `--key-env` exclude each other, and a `--key-store` that contradicts them exits 2.

To change where the key comes from later:

```sh
mailtriage setup --update --account work --key-store keychain
```

This keeps the model, endpoint, timeout and `api_key_env`, so no mail is classified again. A new provider, a new model or a new variable name in `--key-env` does queue open mail for classification again.

## Prompts, `--yes` and `--interactive`

You can answer setup at a terminal, or a script or an agent can pass every answer as a flag. These rules decide when it asks:

- Prompts go to stderr. Each shows its default in brackets; Enter accepts it. Menus are numbered. Invalid input is asked again.
- Prompts are on when stdin is a terminal, or with `--interactive` (answers from a pipe, as the tests do). Otherwise setup runs as with `--yes`.
- `--yes` turns prompts off. Each value comes from its flag or its default. A value without a default, such as `--himalaya-account`, exits 2 and names the flag.
- A flag always answers its question; setup does not ask it.
- Choosing "Abort" in step 1 exits 2 with `setup aborted; nothing was changed` and the reason `setup_aborted`.
- End of input exits 2 with `setup aborted: input ended`. The config is written in step 8, so an abort at the service question keeps it. Run `mailtriage service install --account NAME` for the service.

## Updating an account

Run setup again when something about an account changes, such as your brief or the folders you watch. `mailtriage setup --update --account NAME` (or "Update an account" in the menu) keeps:

- the account's Himalaya binary, config file and account, unless you pass the `--himalaya-*` flags,
- its categories and `taxonomy_revision`,
- its filing `flag`, `max_actions_per_pass` and `live` mode,
- the classifier, unless you pass a classifier flag or answer no,
- where the key comes from, unless you pass a key flag or choose another store in the menu.

Its identity, time zone, brief and folders become the defaults of their questions.

Step 9 binds the account to its mailbox: `doctor` records the identity, the Himalaya account and the IMAP server settings (see [Account binding](../guide/reference.md#account-binding)). Every later command for an account whose binding changed would exit 5, so setup compares the bindings before it writes.

An update that would change any of them exits 5 with `step 3 (account): account NAME is bound to its previous mailbox (identity, Himalaya account or IMAP server changed); keep them, or set this mailbox up under a new name with --account NEW` and writes nothing. The same check applies to a new config whose `state/` directory is left over from an earlier one.

To use a different mailbox, set it up under a new account name:

```sh
mailtriage setup --update --account home --himalaya-account home
```

## Output

Progress and the check summary go to stderr. stdout carries one result object, on one line with `--json`:

```json
{"schema_version":1,"setup":{"account":"work","config":"/Users/alice/.config/mailtriage/mailtriage.json","doctor":{"items":[{"check":"provider","ready":true},{"check":"key","ready":true},{"check":"mail","ready":true},{"check":"filing","ready":true}],"ready":true},"filing":"dry_run","key_source":"command","key_store":"keychain","mailboxes":["INBOX"],"model":"typesafe/jev-latest","provider":"openrouter","service":null,"updates":"auto"}}
```

| Field | Content |
| --- | --- |
| `config` | The absolute path of the config written. |
| `account`, `mailboxes` | The account name and its watched folders. |
| `provider`, `model` | The classifier. |
| `key_source` | `command`, `env`, or `null` for `fake`. |
| `key_store` | The `--key-store` value chosen in this run; `null` when no store was chosen: the classifier or its key source was kept, or it is `fake`. |
| `filing` | `off`, `dry_run` or `live`. |
| `updates` | The config's `updates` after this run: `auto`, `notify` or `off`. |
| `doctor` | `ready`, and `items`: each `{check, ready}`, plus `error` and `fix` when not ready. |
| `service` | `null` when skipped or not installed after a failed `state` check, else the [`service install` result](./service.md#service-commands). |

## Setup exit codes

| Code | Cases |
| --- | --- |
| 0 | Setup finished. `doctor` items that are not ready are listed in the result. |
| 2 | Invalid input; a required flag missing without prompts; an invalid account name; conflicting key flags, or key flags with `--provider fake`; a key tool not on `PATH`; a tool store without a terminal; setup aborted; the service on an unsupported platform; the private Himalaya on a platform for which pimalaya has no build mailtriage can use. |
| 3 | Himalaya missing or not a tested version with IMAP, and the private Himalaya declined or, without prompts, `--himalaya-install` not given; the private Himalaya could not be installed (except on a platform pimalaya has no build for, exit 2); `account check` failed; the folders could not be listed; a key tool or key command failed; the config could not be written; `launchctl` or `systemctl` failed. |
| 5 | The config exists and `--update` was not given (without prompts); the account is bound to another mailbox (its identity, Himalaya account or IMAP server would change); a service file exists that mailtriage did not write; another command is editing the config (`config_busy`), or it changed since setup read it (`config_changed`). |

Every error except the two abort messages (`setup aborted; nothing was changed`, `setup aborted: input ended`) starts with `step N (name): ` and names the flag or command that fixes it, for example `step 2 (Himalaya): --himalaya-account is required without prompts`.

Step 10 errors (`step 10 (service): `) happen after the config is written. They name the `mailtriage service install` command, with `--config` and this run's `--interval-seconds` and `--limit`, to run once the cause is fixed. On a platform without launchd or systemd the fix is to drop `--service install` instead.
