# Installing mailtriage: install script, Homebrew tap, tested Himalaya versions

Date: 2026-10-07
Status: Design approved in conversation; written spec awaiting review.
Builds on: [automatic updates](2026-10-06-auto-update-design.md) and [tray app](2026-10-06-tray-design.md).

## Goal

Installing mailtriage takes one command on macOS and Linux, the way common
command-line tools install:

```sh
curl -fsSL https://raw.githubusercontent.com/wir-drei-digital/mailtriage/main/install.sh | sh
```

or, for people who manage their Mac with Homebrew:

```sh
brew install wir-drei-digital/tap/mailtriage
```

Either way the user ends up with a Himalaya that mailtriage accepts, and an
install that stays current: the script's installs update themselves (automatic
updates), brew's through `brew upgrade`.

## Decisions

| Topic | Decision |
| --- | --- |
| Install paths | `install.sh` (default, macOS and Linux) and a Homebrew tap. `cargo install` stays for developers. |
| Updates | Script installs live in `~/.local/bin` and update themselves. Brew installs are updated by `brew upgrade`; mailtriage only notifies (already the case). |
| Himalaya | mailtriage accepts a list of **tested** Himalaya versions (now 2.1.0 and 2.2.1) instead of exactly 2.1.0. CI tests each one; a weekly workflow tests Himalaya's newest release and proposes adding it. |
| Himalaya in the script | Use a tested `himalaya` on `PATH`; otherwise install the newest tested pimalaya release privately for mailtriage. |
| Himalaya in brew | `depends_on "himalaya"`. |
| Setup | The script offers `mailtriage setup` when a terminal is available; otherwise it prints the command. |
| Spike result | The Dovecot end-to-end suite passed against Himalaya v2.2.1 in both namespace layouts (run 37638014579, branch `spike/himalaya-2.2.1`, not merged). |

## Tested Himalaya versions

### The list

`src/engine/himalaya.rs` holds one list, `TESTED_VERSIONS: &[&str] = &["2.1.0", "2.2.1"]`.

- `himalaya --version` must print `himalaya v<one of the list> … +imap …` on its
  first line, as today for 2.1.0. Any other version, a newer patch release
  included, is refused until it is added to the list.
- `engine.expected_version` is no longer checked. Existing configs that hold
  `"2.1.0"` keep working with 2.2.1. The field becomes optional (`serde(default)`)
  and config validation no longer requires it; setup stops writing it. Configs are
  schema 3, which older binaries refuse, so no older binary reads a config without
  the field.
- The version list and every place that pins a version stay in sync: a test reads
  `install.sh`'s checksum table and the end-to-end workflow's matrix and asserts
  both name exactly the versions in `TESTED_VERSIONS` (the script may carry only the
  newest version's checksums; the matrix carries all).

### What users see

- **`doctor`**: the transport block gains `"tested": true|false` next to `version`.
  An untested version makes the transport not ready, with
  `error: "Himalaya X is not a tested version (tested: 2.1.0, 2.2.1)"`. Setup's
  `mail` check item gets the fix: `install a tested Himalaya (run the mailtriage
  installer again, or see the guide's "Himalaya versions"), or run mailtriage update
  for a mailtriage that supports X`.
- **Setup** step 2 accepts any tested version. When the chosen binary lives in a
  Homebrew prefix (its canonical path contains `/Cellar/himalaya/`), setup prints
  one note: `Homebrew may upgrade Himalaya to a version mailtriage has not tested
  yet; run "brew pin himalaya" to hold it.` It never runs `brew` itself.
- **Passes** with an untested version fail as today with a version error (exit 3);
  the restart and update machinery is unaffected.

### Role names (Himalaya 2.2 and later)

From 2.2 on, `--mailbox NAME` resolves configured aliases first, then mailbox
**role names** (`sent`, `trash`, …), then the literal name. mailtriage passes
`--mailbox` only to `message read`. The existing alias-conflict check
(`alias_conflicts`, reported as `alias_conflict:<folder>`) extends to roles: with
a Himalaya whose version is 2.2 or later, a folder mailtriage reads whose name
equals, case-insensitively, a role name that Himalaya resolves, and that is not
the mailbox the server reports with that role (`INBOX` for `inbox`), is a
conflict and is not read. The plan takes the exact role names from Himalaya's
source for each tested version and pins them in a test.

### CI

- **End-to-end matrix.** `.github/workflows/e2e.yml` runs the Dovecot suite once per
  tested version, each Himalaya asset pinned by URL and SHA-256 (x86_64 Linux).
- **Weekly check.** A new workflow, `.github/workflows/himalaya-compat.yml`
  (schedule weekly, plus `workflow_dispatch`):
  1. Reads pimalaya/himalaya's newest stable release (`gh api`).
  2. If its version is already tested, stops.
  3. Otherwise downloads its x86_64 Linux asset (checksum from the release's asset
     digest), runs the Dovecot suite against it in both layouts.
  4. On success, runs `scripts/add-himalaya-version.sh VERSION` (also usable by hand),
     which adds the version to `TESTED_VERSIONS`, the end-to-end matrix and the
     script's checksum table (digests for the three platforms from the GitHub API),
     and opens a pull request `Test Himalaya VERSION` with the run's link.
  5. On failure, opens (or comments on) an issue `Himalaya VERSION fails the
     end-to-end suite` with the failing step's log tail.
  - Permissions: `contents: write`, `pull-requests: write`, `issues: write`. Pull
    requests opened with `GITHUB_TOKEN` trigger no workflows, so the pull request
    body says the suite already passed in the linked run; CI runs again on `main`
    after merge. Creating pull requests from Actions must be allowed in the
    repository settings (a one-time setting; `docs/releases.md` says so).

## `install.sh`

### Where it lives

- `install.sh` at the repository root, served from
  `https://raw.githubusercontent.com/wir-drei-digital/mailtriage/main/install.sh`.
- The release workflow attaches a copy to every release with its default version
  set to that release (`MAILTRIAGE_VERSION` default replaced at release time), for
  anyone who wants a pinned script.
- README's install section starts with the one-line `curl … | sh`.

### Interface

```
curl -fsSL …/install.sh | sh -s -- [--version X.Y.Z] [--dir DIR] [--tray|--no-tray]
                                     [--no-setup] [--yes] [--uninstall]
```

| Option | Environment | Meaning |
| --- | --- | --- |
| `--version X.Y.Z` | `MAILTRIAGE_VERSION` | Install this release instead of the newest. |
| `--dir DIR` | `MAILTRIAGE_INSTALL_DIR` | Install directory, default `~/.local/bin`. |
| `--tray` / `--no-tray` | `MAILTRIAGE_TRAY=1/0` | Force or skip the tray (default below). |
| `--no-setup` | `MAILTRIAGE_NO_SETUP=1` | Do not offer setup. |
| `--yes` | `MAILTRIAGE_YES=1` | Never prompt; take every default. |
| `--uninstall` | | Remove what the script installed (below). |

Exit codes: 0 done; 1 a failed step (the message names it); 2 invalid options or an
unsupported platform.

### Shape and safety

- POSIX `sh`, `set -eu`, every step inside functions, and the whole script run by a
  single `main "$@"` on its last line, so a partly downloaded script runs nothing.
- Tools: `curl` (else `wget`), `tar`, `mktemp`, and `shasum -a 256` (else
  `sha256sum`); a missing tool exits 1 naming it. All downloads use HTTPS with
  `--proto '=https' --tlsv1.2` (curl) or `--https-only` (wget).
- Platform from `uname -s`/`uname -m`: `Darwin`+`arm64` → `macos-arm64`;
  `Linux`+`x86_64` → `linux-amd64`; `Linux`+`aarch64|arm64` → `linux-arm64`. Anything
  else exits 2: `mailtriage has no build for <os> <arch>; see the guide's Install
  section for building from source`.
- Everything is downloaded into one `mktemp -d` directory, removed on exit (`trap`).
- It never uses `sudo`, never edits shell startup files, and never touches a
  `himalaya` it did not install.

### Steps

1. **Version.** `--version` or the newest release: the tag in the final URL of
   `https://github.com/wir-drei-digital/mailtriage/releases/latest` (the release
   workflow marks only the highest stable release Latest). The tag must be
   `vX.Y.Z`. With `--version` older than an installed `mailtriage --version`, warn
   that an older binary refuses a database a newer one migrated, and ask (default
   no; `--yes` proceeds).
2. **Download and verify.** `mailtriage-vX.Y.Z-PLATFORM.tar.gz` and `SHA256SUMS` from
   `…/releases/download/vX.Y.Z/`. Exactly one `SHA256SUMS` line must name the archive,
   and its hash must match; else exit 1 and install nothing.
3. **Install.** Unpack; copy `mailtriage` into `DIR` (created with `mkdir -p`) as
   `.mailtriage-install.<random>` and `mv` it over `DIR/mailtriage`, the same atomic
   swap the updater uses, so running services restart themselves onto it. Run
   `DIR/mailtriage --version` and require `mailtriage X.Y.Z`.
4. **Tray.** Default: on macOS yes; on Linux only when `DISPLAY` or
   `WAYLAND_DISPLAY` is set. When installing it, download and verify
   `mailtriage-tray-vX.Y.Z-PLATFORM.tar.gz` the same way and swap it in. Run
   `mailtriage-tray --version`; if that fails on Linux, remove the tray again and
   print: `the tray needs GTK 3 and the Ayatana AppIndicator library (Debian/Ubuntu:
   sudo apt install libgtk-3-0 libayatana-appindicator3-1 libxdo3)`.
5. **Himalaya.**
   - If `himalaya` on `PATH` prints a tested version with `+imap`, use it.
   - Otherwise install the newest tested version privately: download
     `himalaya.<arch>-<os>.tgz` (`aarch64-darwin`, `x86_64-linux`, `aarch64-linux`)
     from `https://github.com/pimalaya/himalaya/releases/download/vVERSION/`, check it
     against the SHA-256 pinned in the script's table, unpack the `himalaya` entry
     into `${XDG_DATA_HOME:-$HOME/.local/share}/mailtriage/himalaya/VERSION/himalaya`,
     and check its `--version`. Nothing else is touched.
6. **PATH.** If `DIR` is not on `PATH`, print the exact line for the user's shell
   (`$SHELL` basename `zsh` → `~/.zshrc`, `bash` → `~/.bashrc` (macOS:
   `~/.bash_profile`), `fish` → `fish_add_path DIR`; otherwise a POSIX `export`).
   If another `mailtriage` comes earlier on `PATH`, print which one wins and why the
   updater will not update it.
7. **Login item.** With the tray installed and a terminal available (and not
   `--yes`): `Start the tray at login? [Y/n]` → `mailtriage-tray autostart enable`.
8. **Setup.** With a terminal available, and without `--yes` or `--no-setup`:
   `Run mailtriage setup now? [Y/n]` → `DIR/mailtriage setup --himalaya-binary
   <path from step 5>`. Otherwise print that command. "A terminal is available"
   means `/dev/tty` can be opened; every prompt and setup's own input read from
   `/dev/tty`, because the script itself arrives on stdin.

Re-running the script installs the newest release over the old one; with a current
install it says so and still offers setup.

### `--uninstall`

1. `mailtriage service uninstall --all` (new, below), then
   `mailtriage-tray autostart disable` when the tray is present.
2. Remove `DIR/mailtriage`, `DIR/mailtriage-tray`, their `.previous` copies and the
   private Himalaya directory.
3. Print, without removing anything, where the config, state and logs are
   (`~/.config/mailtriage/` by default) and that the user's mail is untouched.

### New CLI option: `service uninstall --all`

`mailtriage service uninstall --all [--json]` removes every service file that
mailtriage wrote for this user (its marker is present), whatever config or account
it names: launchd bootout then delete, systemd `disable --now`, delete,
`daemon-reload`, exactly as the per-account uninstall. It takes each account's
service lock. Result: `{"schema_version":1,"service":{"action":"uninstalled_all","removed":[{"account","unit_path"}]}}`.
`--all` and `--account` are mutually exclusive (exit 2).

### Testing the script

- **shellcheck** (`-s sh`) in CI.
- **Hermetic runs** from a Rust integration test, reusing the update tests' loopback
  server:
  - `MAILTRIAGE_INSTALL_URL` and `MAILTRIAGE_HIMALAYA_URL` replace the GitHub base
    URLs; the script honours them only when their host is `127.0.0.1` or
    `localhost`, and only then allows plain HTTP.
  - `HOME`, `XDG_DATA_HOME` and `PATH` point into temporary directories; a fake
    `uname` on `PATH` selects the platform; the "binaries" are shell scripts that
    answer `--version`.
  - Cases: a fresh install with a private Himalaya; a tested `himalaya` already on
    `PATH` (no download); a checksum mismatch (nothing installed); an unsupported
    platform (exit 2); `--version` older than installed with `--yes`; tray on macOS,
    skipped on Linux without `DISPLAY`, removed again when `--version` fails;
    non-interactive run (no `/dev/tty`: prints the setup command); re-run over an
    install; `--uninstall` (calls `service uninstall --all`, removes files, keeps
    config).
- **Real runs** after the first real release: a CI job runs the script from `main`
  against GitHub on Linux and macOS with `--yes --no-setup` and checks
  `mailtriage --version`.

## Homebrew tap

### Repository and formula

- Repository: `wir-drei-digital/homebrew-tap` (public; created 2026-10-07).
  `Formula/mailtriage.rb` is written by our release workflow.
- The formula:
  - `desc`, `homepage`, `license "MIT"`, `version`.
  - macOS: `depends_on arch: :arm64`; `url`/`sha256` of
    `mailtriage-vX.Y.Z-macos-arm64.tar.gz`; a `resource "tray"` for
    `mailtriage-tray-vX.Y.Z-macos-arm64.tar.gz`.
  - Linux: `on_intel`/`on_arm` with the two Linux archives; no tray.
  - `depends_on "himalaya"`.
  - `install`: `bin.install "mailtriage"`; on macOS the tray resource's
    `mailtriage-tray` too.
  - `test do`: `assert_match "mailtriage #{version}", shell_output("#{bin}/mailtriage --version")`.
  - `caveats`: run `mailtriage setup`; updates come with `brew upgrade mailtriage`;
    `brew pin himalaya` holds Himalaya on a tested version.
- Template: `packaging/homebrew/mailtriage.rb.in` in this repository, rendered by
  `packaging/homebrew/render.sh VERSION SHA256SUMS` (placeholders for the version and
  the four archive checksums).

### Release automation

A new job in `release.yml`, after a **stable** release is published:

1. Render the formula from the release's `SHA256SUMS`.
2. Check out `wir-drei-digital/homebrew-tap` with the secret `HOMEBREW_TAP_TOKEN`
   (a fine-grained token with contents write access to the tap only), write
   `Formula/mailtriage.rb`, commit `mailtriage X.Y.Z`, push.
- Without the secret the job ends successfully with a notice; the release stays
  published either way, and the job can be re-run.
- The tap repository gets a workflow that, on every push, runs
  `brew install wir-drei-digital/tap/mailtriage` and `brew test mailtriage` on
  macOS and Ubuntu.
- In this repository CI checks the template: render with dummy checksums, then
  `ruby -c` and `brew style` on the result (macOS job).

### Brew installs inside mailtriage

- The updater already reports `managed_by_homebrew` and only notifies.
- **Stable path.** Brew keeps each version under
  `<prefix>/Cellar/mailtriage/<version>/bin/`, and `brew cleanup` later deletes old
  ones. When the canonical executable path has that shape and
  `<prefix>/opt/mailtriage/bin/<name>` exists and resolves to the same file,
  mailtriage uses that `opt` path instead:
  - `service install` records it in the plist or unit;
  - `mailtriage-tray autostart enable` records it for the tray and for
    `--mailtriage`;
  - the restart rule (in `watch` and in the tray) watches it. After
    `brew upgrade`, the `opt` link points to the new version, its identity changes,
    and running processes re-execute onto it between passes, as after our own
    update.
- `setup` prints no `brew pin` note for mailtriage itself, only for Himalaya.

## Docs

- `README.md`: install section = the `curl … | sh` line, the `brew install` line, and
  a link to the guide.
- `docs/guide.md`: Install lists the three paths and who updates each (script →
  mailtriage, brew → `brew upgrade`, cargo → you); the script's options and
  `--uninstall`; a "Himalaya versions" section (the tested list, the private copy,
  `brew pin himalaya`, what `doctor` says); `service uninstall --all`.
- `docs/releases.md`: the tap job and `HOMEBREW_TAP_TOKEN`; the weekly Himalaya
  check and its repository setting; adding a tested Himalaya version by hand with
  `scripts/add-himalaya-version.sh`.
- `docs/hermes.md`: the non-interactive install line
  (`curl … | sh -s -- --yes --no-tray --no-setup`), then `setup --yes …`.
- `docs/verification.md`: a human check of both install paths on a real Mac and a
  real Linux host after the first release.

## Errors

| Situation | Behaviour |
| --- | --- |
| Unsupported platform, invalid option | Script exits 2 with the reason. |
| Missing tool, network error, checksum mismatch, `--version` check fails | Script exits 1 naming the step; nothing half-installed (temp files only). |
| Tray does not run on Linux | Tray removed again, the packages named, the install continues. |
| No tested Himalaya and the private download fails | Exit 1 after mailtriage is installed; the message says how to get Himalaya and to re-run the script. |
| Untested Himalaya at run time | `doctor` not ready with the fix; passes fail with the version error (exit 3). |
| Tap secret missing | Release job ends with a notice; nothing else changes. |

## Testing (Rust and CI)

- Version list: `himalaya v2.1.0 +imap`, `v2.2.1 +imap` accepted; `v2.2.2`, `v2.1.0`
  without `+imap`, `v1.2.0` refused; an old config with `expected_version` and one
  without both load.
- Role conflicts: with a 2.2.1 version string, a folder named `Sent` that is not the
  sent mailbox is an `alias_conflict`; `INBOX` is not; with 2.1.0 roles are ignored.
- `doctor` `tested` true/false and setup's fix text; setup's `brew pin` note for a
  Cellar path only.
- Stable brew path: a fake `<prefix>/Cellar/mailtriage/1.2.3/bin/mailtriage` with
  `<prefix>/opt/mailtriage` linking to it → `service install` records the `opt`
  path; the restart rule fires when the `opt` link is moved to another version's
  file.
- `service uninstall --all`: two configs' services removed, foreign files left alone,
  `--all` with `--account` exits 2.
- The consistency test for versions across code, script and workflow.
- The script tests above; the template render test.

## Out of scope

- Windows and Intel Macs.
- homebrew-core.
- Signing and notarizing macOS binaries.
- nix, apt and other package managers.
- A download domain of our own.
- Bundling Himalaya inside mailtriage's archives.
