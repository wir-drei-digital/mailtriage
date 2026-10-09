# Himalaya versions

mailtriage runs Himalaya for every mailbox operation and accepts only the versions it is tested with: **2.1.0** and **2.2.1**. The list, with the SHA-256 of each version's release archives, is compiled into mailtriage from `src/engine/himalaya-versions.json`; a newer mailtriage release can add versions.

- The first line of `himalaya --version` must name a tested version, such as `himalaya v2.2.1 …`, and contain `+imap`. Any other version, a newer patch release included, is refused until a mailtriage release tests it.
- Setup writes the version it found into `engine.expected_version`, but mailtriage does not compare it: a config that says `2.1.0` works with Himalaya 2.2.1.
- `doctor` reports `transport.tested`. An untested Himalaya makes the transport not ready, with `"error": "Himalaya 2.2.2 is not a tested version (tested: 2.1.0, 2.2.1)"`. `sync` and `watch` passes exit 3 with that message, and setup refuses it in step 2.

## Folder names Himalaya resolves

mailtriage gives `--mailbox` only to `message read`, which fetches a message's text, and Himalaya resolves that name before it opens a mailbox:

1. through the merged alias map: the global `mailbox.alias` table, overridden key by key by the account's `accounts.NAME.mailbox.alias`. Keys compare case-insensitively, and `mailbox.aliases` is the same table;
2. then, from 2.2 on, through the version's mailbox roles: 2.2.1 maps only `inbox` to `INBOX` for IMAP;
3. else the name itself.

A watched folder or category folder whose result is another mailbox (with `INBOX` compared case-insensitively) is an alias conflict. mailtriage never reads mail from it, in every filing mode; its mail stays queued without using a retry attempt. With filing on, the pass also reports `alias_conflict:FOLDER`, stops scanning the folder and makes no filing writes. Two alias keys that differ only in case and name different mailboxes count as a conflict too. When the Himalaya configuration cannot be parsed, mailtriage reads no folder at all, and `doctor` reports why in `transport.error`.

## A private Himalaya

```sh
mailtriage himalaya install [--version X.Y.Z] [--json]
```

installs a tested Himalaya release for mailtriage alone; it never touches another `himalaya`.

- It installs the newest tested version, or `--version`, which must be tested (else exit 2), into `DATA/mailtriage/himalaya/VERSION/himalaya`. `DATA` is `$XDG_DATA_HOME` when that is an absolute path, else `~/.local/share`, on macOS too.
- It downloads that version's `himalaya.PLATFORM.tgz` from pimalaya's GitHub releases over HTTPS, with the same URL rules as `mailtriage update`, and checks it against the SHA-256 compiled into mailtriage. It unpacks only the `himalaya` executable into a new file, checks that it runs and prints that version with `+imap`, and moves it into place with a rename.
- The directories are created with mode 0755. Before anything is written there, the version's directory and each of its parents must be safe: no symlink, owned by you or root, and not writable by group or others; a world-writable directory with the sticky bit, such as `/tmp`, is fine above one of yours. Otherwise it exits 3 with the reason `unsafe_permissions`, the directory and the fix, such as `chmod go-w DIR`.
- Run again, it reports `current` and downloads nothing.
- Point an account at it with `mailtriage setup --update --himalaya-binary PATH`, using the path it printed. Setup offers this itself when it finds no tested Himalaya (step 2).

```json
{"schema_version":1,"himalaya":{"action":"installed","version":"2.2.1","path":"/Users/alice/.local/share/mailtriage/himalaya/2.2.1/himalaya"}}
```

`action` is `installed` or `current`. Exit codes: 0; 2 for a version that is not tested, or a platform for which pimalaya has no build mailtriage can use; 3 for a network error, a checksum mismatch, a bad archive, a binary that does not run, an unsafe directory, or another `himalaya install` that held its directory's lock for 60 seconds.

## Homebrew's Himalaya

`brew upgrade` may move Homebrew's `himalaya` to a version mailtriage has not tested; mailtriage then refuses it until a mailtriage release tests that version. Either hold it with `brew pin himalaya` (and `brew unpin himalaya` once mailtriage tests the newer one), or give mailtriage its own copy with `mailtriage himalaya install` and `mailtriage setup --update --himalaya-binary PATH`. Setup says so when the Himalaya it uses is Homebrew's.
