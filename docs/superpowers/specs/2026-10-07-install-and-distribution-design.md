# Installing mailtriage: install script, Homebrew tap, tested Himalaya versions

Date: 2026-10-07
Status: Design approved in conversation; written spec revised after Codex review round 1.
Builds on: [automatic updates](2026-10-06-auto-update-design.md) and [tray app](2026-10-06-tray-design.md).

## Goal

Installing mailtriage takes one command on macOS and Linux, the way common
command-line tools install:

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/wir-drei-digital/mailtriage/main/install.sh | sh
```

or, for people who manage their Mac with Homebrew:

```sh
brew install wir-drei-digital/tap/mailtriage
```

Either way the user ends up with a Himalaya that mailtriage accepts, and an
install that stays current: script installs update themselves (automatic updates),
brew installs through `brew upgrade`.

## Decisions

| Topic | Decision |
| --- | --- |
| Install paths | `install.sh` (default, macOS and Linux) and a Homebrew tap. `cargo install` stays for developers. |
| Script design | A thin bootstrapper, as rustup does: it downloads and verifies the release archive, then runs the downloaded binary's `mailtriage self install`, which reuses the updater's install transaction. No install logic is duplicated in shell. |
| Updates | Script installs live in `~/.local/bin` and update themselves. Brew installs are updated by `brew upgrade`; mailtriage only notifies (already the case). |
| Himalaya versions | mailtriage accepts a list of **tested** Himalaya versions (now 2.1.0 and 2.2.1) instead of exactly 2.1.0. The list and its release checksums live in one data file compiled into each release. CI tests each version; a weekly workflow tests Himalaya's newest release and proposes adding it. |
| Getting Himalaya | `mailtriage himalaya install` installs a tested pimalaya release privately for mailtriage, using the checksums compiled into that mailtriage. `setup` offers it when no tested Himalaya is found. Neither the script nor the formula installs or depends on a system Himalaya. |
| Setup | After installing, the binary offers `mailtriage setup` when a terminal is available, then the tray login item. |
| Spike result | The Dovecot end-to-end suite passed against Himalaya v2.2.1 in both namespace layouts (run 37638014579, branch `spike/himalaya-2.2.1`, since deleted). |

## Tested Himalaya versions

### The data file

`src/engine/himalaya-versions.json` is the single source, compiled into the binary
with `include_str!` and parsed once:

```json
{"versions":[
  {"version":"2.1.0","roles":{},
   "assets":{"aarch64-darwin":"sha256:a5a787b7…","x86_64-linux":"sha256:683a2ab8…","aarch64-linux":"sha256:c41adab4…"}},
  {"version":"2.2.1","roles":{"inbox":"INBOX"},
   "assets":{"aarch64-darwin":"sha256:a5d97a1f…","x86_64-linux":"sha256:5c5ba272…","aarch64-linux":"sha256:1dc21c3d…"}}
]}
```

Full digests, from GitHub's asset `digest` fields on 2026-10-07:

| Version | aarch64-darwin | x86_64-linux | aarch64-linux |
| --- | --- | --- | --- |
| 2.1.0 | `a5a787b7c4dbf065408e7772908fc75c799626f4cebab8e9c78fafe3e2fa585c` | `683a2ab8e1534f01e6bda3a69e204d564c31fbfbe20511fc7bc60b67f2e85884` | `c41adab4bc220ba816cdbf865a5df8dc3b358b39ec58b4be0ed2f64e46b1d182` |
| 2.2.1 | `a5d97a1f7bbca45e58bddde3f9f17325f614c9cd6d5f17fabf38ddcb030869ab` | `5c5ba2724c162f82d0a0c71b6c03224ed44f8bef7b14ace5da0635d3af2665a0` | `1dc21c3dd6d948929e22e5cae49b9d0b3b30ff88fafd4493fd4460c6252e9d90` |

- `version`: an exact Himalaya version that passed the Dovecot suite.
- `assets`: the SHA-256 of pimalaya's release asset `himalaya.<platform>.tgz` for
  each platform mailtriage builds for. Every entry must have all three, each a
  `sha256:` followed by 64 lowercase hex digits; a unit test enforces it.
- `roles`: the mailbox role names that version's `--mailbox` resolution maps for
  the IMAP backend, and to which mailbox (see [Role names](#role-names)).
- The newest entry is the version `himalaya install` installs by default.

### Accepting a version

- `himalaya --version` must print `himalaya v<a listed version> … +imap …` on its
  first line. Any other version, a newer patch release included, is refused until
  it is listed.
- `engine.expected_version` stays in the config schema and setup keeps writing
  the detected version into it, so configs stay readable by every existing
  schema-3 binary. It is no longer compared: any listed version is accepted
  whatever the field says.

### What users see

- **`doctor`**: the transport block gains `"tested": true|false` next to `version`.
  An untested version makes the transport not ready with
  `error: "Himalaya X is not a tested version (tested: 2.1.0, 2.2.1)"`. Setup's
  `mail` check item gets the fix
  `run mailtriage himalaya install, then mailtriage setup --update --himalaya-binary <the path it prints>`.
- **Setup**, step 2:
  - accepts any listed version;
  - when the Himalaya found (on `PATH` or by `--himalaya-binary`) is missing or
    untested, offers `Install Himalaya 2.2.1 for mailtriage? [Y/n]` and uses
    the installed path; without prompts, `--himalaya-install` does the same and
    otherwise the step fails with the fix above;
  - when the chosen binary lives in a Homebrew keg (its canonical path contains
    `/Cellar/himalaya/`), prints one note: `Homebrew may upgrade Himalaya to a version
    mailtriage has not tested; "brew pin himalaya" holds it, or run mailtriage
    himalaya install for a private copy.`
- **Passes** with an untested version fail as today with the version error (exit 3).

### `mailtriage himalaya install`

```
mailtriage himalaya install [--version X.Y.Z] [--json]
```

- Installs the newest listed version, or `--version` (which must be listed, else
  exit 2), for this platform into
  `<data>/mailtriage/himalaya/<version>/himalaya`, where `<data>` is
  `$XDG_DATA_HOME` when absolute, else `~/.local/share` (on macOS too).
- Downloads `https://github.com/pimalaya/himalaya/releases/download/v<version>/himalaya.<platform>.tgz`
  with the updater's HTTPS client and URL rules, checks it against the compiled
  `assets` digest, unpacks only the top-level `himalaya` regular file with the
  updater's bounded archive reader, stages it exclusively, probes `--version`
  (listed version, `+imap`), then renames it into place.
- Idempotent: an existing, matching copy is reported as `current`.
- Output: `{"schema_version":1,"himalaya":{"action":"installed"|"current","version","path"}}`.
  Exit codes: 0; 2 invalid or unlisted version, unsupported platform; 3 network,
  checksum, archive or probe failure.
- It never touches any other `himalaya`.

### Role names

From 2.2 on, `--mailbox NAME` resolves configured aliases, then mailbox roles, then
the literal name. mailtriage passes `--mailbox` only to `message read`.

- **Effective target.** For every folder mailtriage reads, it computes the mailbox
  Himalaya will actually open, following that version's rules:
  1. the **merged alias map**: global `mailbox.alias.*` overridden by the account's
     `accounts.<name>.mailbox.alias.*`, keys compared the way Himalaya compares them
     (case-insensitively);
  2. then, for 2.2 and later, the version's `roles` table (2.2.1 IMAP: `inbox` →
     `INBOX` only, per its resolver);
  3. else the literal name.
- **Conflict.** A folder whose effective target is not the folder itself (with
  `INBOX` compared case-insensitively) is an `alias_conflict:<folder>` and is never
  read.
- **Every read path, every filing mode.** The check runs before any `message read`,
  also with filing off, for source mailboxes and category folders alike. When the
  Himalaya config cannot be read or parsed, no folder is read (fail closed) and
  `doctor` reports the error.
- The plan reads each tested version's resolver from Himalaya's source and pins
  the `roles` table and the case rules in tests.

### CI

- **End-to-end matrix.** A first job in `.github/workflows/e2e.yml` reads the data
  file and emits the matrix (`fromJSON`); the Dovecot job runs once per listed
  version with the `x86_64-linux` asset and its digest. `tests/e2e/run.sh` accepts
  any version listed in the data file. The workflow file never changes when a
  version is added.
- **Weekly check**, `.github/workflows/himalaya-compat.yml` (schedule weekly, plus
  `workflow_dispatch`), in three jobs:
  1. **test** (`contents: read`): read pimalaya/himalaya's newest stable release; if
     listed, stop. Otherwise run `scripts/add-himalaya-version.sh VERSION` on the
     checkout. It adds the entry: digests from the release's asset `digest` fields,
     fail closed when one is missing, and `roles` copied from the previous entry.
     Then build and run the Dovecot suite against the candidate in both layouts.
     Upload the edited data file as an artifact.
  2. **propose** (needs test; `contents: write`, `pull-requests: write`): commit the
     artifact to branch `himalaya/VERSION`, push, and open the pull request
     `Test Himalaya VERSION`. Its body links the run and asks the reviewer to check
     the version's resolver for role changes. Then it dispatches `ci.yml` and
     `e2e.yml` on the branch (`workflow_dispatch` runs start even from
     `GITHUB_TOKEN`).
  3. **report** (on test failure; `issues: write`): open, or comment on, the issue
     `Himalaya VERSION fails the end-to-end suite` with the failing step's log tail.
  - The candidate binary only ever runs in the read-only job.
  - Actions must be allowed to create pull requests, a one-time repository setting
    that `docs/releases.md` names.

## `install.sh`: the bootstrapper

### Where it lives

- `install.sh` at the repository root, served from
  `https://raw.githubusercontent.com/wir-drei-digital/mailtriage/main/install.sh`.
- The release workflow attaches a copy to every release with its default version set
  to that release.
- README's install section starts with the one-line command above.

### Interface

```
curl … | sh -s -- [--version X.Y.Z] [--dir DIR] [--tray|--no-tray] [--no-setup] [--yes] [--uninstall]
```

| Option | Environment | Meaning |
| --- | --- | --- |
| `--version X.Y.Z` | `MAILTRIAGE_VERSION` | Install this release instead of the newest. Never a downgrade (below). |
| `--dir DIR` | `MAILTRIAGE_INSTALL_DIR` | Install directory, default `~/.local/bin`. |
| `--tray` / `--no-tray` | `MAILTRIAGE_TRAY=1/0` | Force or skip the tray. |
| `--no-setup` | `MAILTRIAGE_NO_SETUP=1` | Do not offer setup or the login item. |
| `--yes` | `MAILTRIAGE_YES=1` | Never prompt; see the decision table. |
| `--uninstall` | | Uninstall the installation in `DIR` (below). |

Exit codes: 0 done; 1 a failed step (the message names it); 2 invalid options, an
unsupported platform, or a refused downgrade.

### Shape and safety

- **Truncation-safe.** The whole script, after the shebang, is one brace group
  `{ … }` ending at the file's last line. `sh` parses the group completely before
  running anything, so a truncated download is a syntax error and runs nothing.
  Inside: `set -eu`, functions, then `main "$@"` as the group's last command.
- **curl only.** It needs `curl`, `tar`, `mktemp`, `uname` and `shasum -a 256` or
  `sha256sum`; a missing one exits 1 naming it. There is no wget fallback.
- **Transport.** Every request uses
  `curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL --max-redirs 10 --connect-timeout 30 --max-time 600`.
- **Platform** from `uname -s`/`uname -m`: `Darwin arm64` → `macos-arm64`;
  `Linux x86_64` → `linux-amd64`; `Linux aarch64|arm64` → `linux-arm64`. Anything
  else exits 2: `mailtriage has no build for <os> <arch>; see the guide's Install
  section to build from source`.
- **One temporary directory** from `mktemp -d`, removed on exit by `trap`. The
  script itself writes nothing anywhere else.
- It never uses `sudo`, never edits shell startup files, and never touches a
  `himalaya`.

### What it does

1. **Version.**
   - With `--version`: that version.
   - Otherwise the newest release: `curl … -o /dev/null -w '%{url_effective}'` on
     `https://github.com/wir-drei-digital/mailtriage/releases/latest`. The effective
     URL must match exactly
     `https://github.com/wir-drei-digital/mailtriage/releases/tag/vX.Y.Z` (decimal
     parts), else exit 1.
2. **Download and verify.** `mailtriage-vX.Y.Z-PLATFORM.tar.gz`, `SHA256SUMS`, and,
   when the tray is wanted, `mailtriage-tray-vX.Y.Z-PLATFORM.tar.gz`, from
   `https://github.com/wir-drei-digital/mailtriage/releases/download/vX.Y.Z/`, into
   the temporary directory.
   - `SHA256SUMS` must have exactly one line for each archive, and the hashes must
     match; else exit 1.
   - Only the member `mailtriage` (and `mailtriage-tray`) is extracted, into the
     temporary directory: `tar -xzf ARCHIVE -C "$tmp" mailtriage`.
3. **Hand over.** Run
   `"$tmp/mailtriage" self install --dir DIR [--tray-file "$tmp/mailtriage-tray"] [--no-setup] [--yes]`.
   - Its stdin is `/dev/tty` when that can be opened and `--yes` is not given,
     otherwise `/dev/null`. The script itself arrives on stdin.
   - Its exit code is the script's.
4. **Uninstall** (`--uninstall`): no download. Run
   `"DIR/mailtriage" self uninstall --dir DIR [--yes]`, with stdin chosen the same
   way. If `DIR/mailtriage` is missing, exit 1 saying nothing is installed there.

Tray default: on macOS yes; on Linux only when `DISPLAY` or `WAYLAND_DISPLAY` is set.

### Decision table for prompts

| Question | Terminal, no `--yes` | `--yes` | No terminal |
| --- | --- | --- | --- |
| Downgrade below the installed version | refused (exit 2) | refused (exit 2) | refused (exit 2) |
| Run setup now | ask, default yes | no (prints the command) | no (prints the command) |
| Install a private Himalaya (inside setup) | ask, default yes | not reached | not reached |
| Start the tray at login (after setup succeeded) | ask, default yes | no (prints the command) | no |
| Uninstall confirmation | ask, default no | yes | refused (exit 2, use `--yes`) |

End of input at a prompt counts as its default.

## `mailtriage self install` and `self uninstall`

New subcommands that the script runs. They are ordinary CLI commands, so agents can
use them on a binary they placed themselves.

### `self install`

```
mailtriage self install --dir DIR [--tray-file PATH] [--no-setup] [--yes] [--json]
```

1. **Directory.** Resolve `DIR` to an absolute path. Missing directories, the
   missing part of the path included, are created with mode 0755 regardless of the
   umask. The canonical directory must pass the updater's checks: owned by the user,
   not group- or world-writable. Otherwise exit 3 with `unsafe_permissions`, the
   directory, and a remedy (`chmod go-w DIR`, or another `--dir`).
2. **No downgrade.** If `DIR/mailtriage` exists and prints a version newer than
   this binary's, exit 2:
   `DIR/mailtriage is X, newer than Y; to go back, follow the guide's rollback steps`.
3. **CLI.** Install this binary's own file (`current_exe`) at `DIR/mailtriage` with
   the updater's transaction for the `mailtriage` component:
   - the installation lock;
   - exclusive staging;
   - the probe;
   - revalidation;
   - the `.previous` backup;
   - the rename commit point;
   - the directory fsync.
   Running services restart themselves onto it through the restart rule.
4. **Tray.** With `--tray-file`, the same transaction for the `mailtriage-tray`
   component, after the CLI's commit. A failed probe skips the tray and keeps any
   existing one. On Linux it prints:
   `the tray needs GTK 3 and the Ayatana AppIndicator library (Debian/Ubuntu: sudo apt install libgtk-3-0 libayatana-appindicator3-1 libxdo3)`.
5. **Record.** Under the cache lock, write each installed component's `installs`
   entry as a successful update does: version, identity, and cleared `last_error`,
   `failures` and `next_attempt_at`.
6. **PATH report.**
   - If `DIR` is not on `PATH`, print the exact line for the user's shell: zsh →
     `~/.zshrc`; bash → `~/.bashrc`, or `~/.bash_profile` on macOS; fish →
     `fish_add_path DIR`; otherwise a POSIX `export`.
   - If another `mailtriage` comes first on `PATH`, name it, and say that it stays
     in use and is not updated by this install.
7. **Setup.** Unless `--no-setup` or `--yes`, and stdin is a terminal: ask
   `Run mailtriage setup now? [Y/n]`, then run the setup flow in this process with
   this terminal. Its step 2 offers the private Himalaya. Otherwise print
   `DIR/mailtriage setup`.
8. **Login item.** When the tray was installed and setup succeeded in step 7: ask
   `Start the tray at login? [Y/n]` and run
   `DIR/mailtriage-tray autostart enable --config <the config setup wrote> --mailtriage DIR/mailtriage`.
   When setup was skipped, print that command with the config placeholder.

- **All paths are explicit:** every command it runs is `DIR/mailtriage` or
  `DIR/mailtriage-tray`, never a bare name from `PATH`.
- **Output:**
  `{"schema_version":1,"self_install":{"dir","cli":{"action","version","path"},"tray":{"action","error"}|null,"on_path":bool,"shadowed_by":null|path,"setup":"ran"|"skipped"|"failed","autostart":"enabled"|"skipped"}}`.
- **Exit codes:**
  - 0;
  - 2 for invalid arguments or a refused downgrade;
  - 3 for an unsafe directory, a failed transaction, or a failed setup (the
    install itself stays);
  - 5 when the installation lock is held.

### `self uninstall`

```
mailtriage self uninstall [--dir DIR] [--yes] [--json]
```

`DIR` defaults to the running executable's directory. It removes only the
installation in `DIR`:

1. **Refusals.** A Homebrew keg (the canonical path contains `/Cellar/mailtriage/`)
   exits 2 with `installed by Homebrew; run brew uninstall mailtriage`. Without
   `--yes`, it asks for confirmation (default no).
2. **Services.** Every service file with mailtriage's marker whose decoded
   executable canonicalizes to `DIR/mailtriage` is uninstalled as
   `service uninstall` does, under that account's service lock.
   - Under the lock, it re-reads the file and skips it if it now names another
     executable.
   - Services of other installations, Homebrew included, are left alone.
   - Without a service manager there are no services, and that is not an error.
3. **Tray.**
   - If the login item names `DIR/mailtriage-tray`, run
     `DIR/mailtriage-tray autostart disable`. On macOS, also boot out its login job
     when it is loaded.
   - Then run `DIR/mailtriage-tray quit` (new, below).
4. **Files.** Only when steps 2 and 3 fully succeeded, remove:
   - `DIR/mailtriage` and `DIR/mailtriage-tray` and their `.previous` copies;
   - `DIR/.mailtriage-update.lock`;
   - those paths' `installs` entries in the update cache;
   - the private Himalaya directory `<data>/mailtriage/himalaya/` when no remaining
     config names a binary inside it.
5. **Kept.** It removes nothing else. It prints where the config, state and logs are
   (`~/.config/mailtriage/` by default) and that no mail was touched.

- **Output:**
  `{"schema_version":1,"self_uninstall":{"dir","services":[{"account","unit_path","action","error"}],"tray":"…","removed":[…],"kept":[…]}}`.
- **Exit codes:**
  - 0;
  - 2 for a Homebrew install, a refused confirmation, or no terminal without
    `--yes`;
  - 3 for any failed service or tray step, in which case no file is removed and the
    output lists the failures.

### `mailtriage-tray quit`

The tray writes its PID to `tray.pid` next to `tray.lock`, as the windows do.
`mailtriage-tray quit [--json]` reads it. When that process is running and its
executable is this tray (`/proc/<pid>/exe` on Linux, `proc_pidpath` on macOS), it
sends `SIGTERM` and waits up to 5 s for the lock to be released.

- The tray treats `SIGTERM` like Quit.
- Open categories windows are separate processes and keep running; `self uninstall`
  prints `close any open categories window`.
- Result: `quit`, `not_running`, or exit 3 when the tray did not stop.

## Homebrew tap

### Repository and formula

- **Repository:** `wir-drei-digital/homebrew-tap` (public; created 2026-10-07). Our
  release workflow writes its `Formula/mailtriage.rb`.
- **The formula:**
  - `desc`, `homepage`, `license "MIT"`, `version`;
  - **macOS:** `depends_on arch: :arm64`; `url` and `sha256` of
    `mailtriage-vX.Y.Z-macos-arm64.tar.gz`; a `resource "tray"` for the macOS tray
    archive;
  - **Linux:** `on_intel` and `on_arm` with the two Linux archives; no tray;
  - **no Himalaya dependency:** `mailtriage setup` uses a tested `himalaya` on `PATH`
    or installs a private one;
  - `install`: `bin.install "mailtriage"`; on macOS,
    `resource("tray").stage { bin.install "mailtriage-tray" }`;
  - `test do`:
    `assert_match "mailtriage #{version}", shell_output("#{bin}/mailtriage --version")`;
  - `caveats`:
    - run `mailtriage setup`, which installs a tested Himalaya if needed;
    - updates come with `brew upgrade mailtriage`;
    - on macOS, `mailtriage-tray autostart enable` starts the tray at login.
- **Template:** `packaging/homebrew/mailtriage.rb.in`, rendered by
  `packaging/homebrew/render.sh VERSION SHA256SUMS`, with placeholders for the version
  and the four archive checksums. The renderer fails when an archive is missing from
  `SHA256SUMS`.

### Release automation

A new job `homebrew` in `release.yml` runs after a stable release is published, in its
own concurrency group `homebrew-tap` (queued, never cancelled):

1. **Only the highest release.** Skip unless this release is the highest published
   stable release, using the same check as the Latest marking.
2. **Render** the formula and check it with `ruby -c`.
3. **Check out the tap** with the secret `HOMEBREW_TAP_TOKEN`: a fine-grained token
   with contents write access to the tap only.
4. **Never move backwards.** Read the tap formula's current `version`. If it is
   higher than this release, skip. If it is equal and the file is identical, succeed
   without a commit.
5. **Commit and push** `mailtriage X.Y.Z`. On a push conflict, fetch, repeat step 4,
   and retry once.

- Without the secret, the job ends successfully with a notice. The release stays
  published either way, and the job can be re-run.
- **CI in the tap repo:** a workflow runs `brew install wir-drei-digital/tap/mailtriage`
  and `brew test mailtriage` on macOS and Ubuntu on every push.
- **CI in this repo:** a macOS job renders the template with dummy checksums and runs
  `ruby -c` and `brew style` on it.

### Brew installs inside mailtriage

- **Ownership by canonical path.** The updater classifies an installation by its
  canonical path. A keg is `managed_by_homebrew`, so it only notifies; that is
  unchanged. Cache keys stay canonical paths.
- **Stable launch path.** Brew keeps each version in
  `<prefix>/Cellar/mailtriage/<version>/bin/`, and `brew upgrade` deletes the old
  keg. When a canonical path has that shape and `<prefix>/opt/mailtriage/bin/<name>`
  exists, mailtriage uses that `opt` path as the **launch path** in four places:
  - `service install` records it;
  - `mailtriage-tray autostart enable` records it for the tray and for
    `--mailtriage`;
  - the tray keeps it as the CLI path it runs, passes to windows and re-executes
    with, instead of the canonical keg path;
  - the restart rule's re-exec target.
- **Restart rule for kegs.** The restart rule (in `watch` and in the tray) also
  compares the canonical target of the `opt` path with the canonical path of the
  running image. When they differ, which happens after `brew upgrade` or when the
  old keg is gone, it probes the `opt` path and re-executes it. That covers a
  retarget before the process recorded its paths. On macOS, the existing start-up
  `--version` comparison covers it too.
- `setup` prints no `brew pin` note for mailtriage itself, only for a Homebrew
  Himalaya.

## Docs

- **`README.md`:** the install section is the `curl … | sh` line, the `brew install`
  line, and a link to the guide.
- **`docs/guide.md`:**
  - Install lists the three paths and who updates each (script → mailtriage,
    brew → `brew upgrade`, cargo → you);
  - the script's options and the decision table;
  - `self install` and `self uninstall`;
  - a "Himalaya versions" section: the tested list, `himalaya install`,
    `brew pin himalaya`, and what `doctor` says;
  - rollback stays as documented, and the script never downgrades.
- **`docs/releases.md`:**
  - the tap job and `HOMEBREW_TAP_TOKEN`;
  - the weekly Himalaya check and its repository setting;
  - adding a version by hand with `scripts/add-himalaya-version.sh`.
- **`docs/hermes.md`:** the non-interactive line
  `curl … | sh -s -- --yes --no-tray --no-setup`, then `setup --yes --himalaya-install …`.
- **`docs/verification.md`:** a human check of both install paths, and of
  `brew upgrade` with a running service, on a real Mac and a real Linux host after
  the first release.

## Errors

| Situation | Behaviour |
| --- | --- |
| Unsupported platform, invalid option, downgrade | Script exits 2 with the reason; nothing installed. |
| Missing tool, network error, checksum mismatch, unexpected latest URL | Script exits 1 naming the step; nothing installed. |
| Unsafe install directory | `self install` exits 3 with the remedy; nothing installed. |
| Probe fails for the CLI | The transaction aborts before its commit point; the existing install stays. |
| Tray does not run on Linux | Tray skipped, existing tray kept, packages named; exit 0. |
| Setup fails | The binaries stay installed; exit 3 with setup's message. |
| Untested Himalaya at run time | `doctor` not ready with the fix; passes fail with the version error (exit 3). |
| Uninstall cannot remove a service or stop the tray | Exit 3, no files removed, failures listed. |
| Tap secret missing, or the release is not the highest | The tap job ends with a notice; nothing else changes. |

## Testing

**Rust**

- **Versions:**
  - the data file parses; every entry has three well-formed digests;
  - `v2.1.0 +imap` and `v2.2.1 +imap` are accepted; `v2.2.2`, a listed version
    without `+imap`, and `v1.2.0` are refused;
  - a config whose `expected_version` says `2.1.0` accepts 2.2.1.
- **Effective target:**
  - a global alias, an account override, and case-insensitive keys;
  - 2.2.1's `inbox` role;
  - a folder whose target differs is a conflict;
  - the check runs with filing off and for source mailboxes;
  - an unreadable Himalaya config reads nothing.
- **`himalaya install`** with the update tests' loopback server and a test-only
  digest table: installed, current, checksum mismatch, a missing `himalaya` entry, a
  failed probe, an unlisted `--version`.
- **Setup:** offers the private Himalaya when none is tested; `--himalaya-install`;
  the Homebrew note only for a keg path.
- **`self install`:**
  - a fresh install into a missing directory (mode 0755 under umask 002);
  - a group-writable directory and a symlinked directory that resolves to one
    (exit 3);
  - a refused downgrade;
  - a running service restarting onto the new file;
  - tray installed, or skipped with the existing one kept when its probe fails;
  - cache entries cleared of a previous failure;
  - PATH and shadowing reports;
  - setup offered only with a terminal;
  - the login item after setup with setup's config.
- **`self uninstall`:**
  - services of this installation removed, another installation's and Homebrew's
    kept;
  - a service that now names another executable skipped;
  - a failing service removal keeps every file (exit 3);
  - no service manager;
  - Homebrew refused;
  - the tray quit and its login item disabled.
- **`mailtriage-tray quit`:** a running tray stops; a stale PID or another process is
  left alone.
- **Brew paths,** with a fake `<prefix>/Cellar/mailtriage/1.2.3/bin/` and `opt` link:
  - `service install` and autostart record the `opt` path;
  - the updater still reports `managed_by_homebrew`;
  - the restart rule fires when `opt` is retargeted and when the old keg is deleted;
  - the tray keeps the `opt` CLI path across a restart.

**Script**

- `shellcheck -s sh` in CI.
- **Truncation:** the script cut at every byte offset of its last 64 bytes runs
  nothing (a marker command is never reached), and the full script keeps its
  arguments.
- **Hermetic runs** from a Rust integration test with the loopback server:
  - `MAILTRIAGE_INSTALL_URL` replaces the GitHub base URL. It is honoured only for
    `127.0.0.1` or `localhost`, and only then is plain HTTP allowed.
  - A fake `uname` on `PATH` selects the platform, and `HOME` and `XDG_DATA_HOME`
    are temporary.
  - Cases:
    - a fresh install hands over to `self install` with the right arguments and
      stdin;
    - an unexpected latest URL (exit 1);
    - a checksum mismatch and a duplicate `SHA256SUMS` line (exit 1, nothing
      installed);
    - an unsupported platform (exit 2);
    - the tray archive fetched only when the tray is wanted;
    - `--uninstall` runs `self uninstall`;
    - no `/dev/tty` gives `/dev/null` as stdin.
- **Real runs** after the first real release: CI runs the script from `main` against
  GitHub on Linux and macOS with `--yes --no-setup`, then checks `mailtriage --version`
  and `mailtriage himalaya install`.

**CI files**

- The e2e matrix comes from the data file.
- The weekly workflow's helper script is tested on a copy of the data file: an
  entry added; a missing digest fails it.
- The formula template renders and passes `ruby -c`.

## Out of scope

- Windows and Intel Macs.
- homebrew-core.
- Signing and notarizing macOS binaries.
- nix, apt and other package managers.
- A download domain of our own.
- A wget fallback.
- Bundling Himalaya inside mailtriage's archives.
- A global `service uninstall --all`.
