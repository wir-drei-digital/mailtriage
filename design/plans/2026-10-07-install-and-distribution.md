# Install and Distribution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Installing mailtriage takes one command (`curl … | sh`, or `brew install wir-drei-digital/tap/mailtriage`) on macOS and Linux and ends with a Himalaya mailtriage accepts: tested Himalaya versions from a compiled data file, a private `mailtriage himalaya install`, `mailtriage self install`/`self uninstall` behind a thin, truncation-safe `install.sh`, `mailtriage-tray quit`, Homebrew `opt` launch paths, and the release, tap and CI automation around them.

**Architecture:** The CLI gains one module tree, `src/distribution/` (`protected` path rule, private `himalaya` install, Homebrew `brew` launch paths, the tray's `login_item`, `self_install`, `self_uninstall`), plus two engine modules: `src/engine/versions.rs` (the tested-versions data file `himalaya-versions.json`, compiled in) and `src/engine/targets.rs` (the mailbox Himalaya opens for `--mailbox NAME`). `self install` reuses the updater's install transaction, split so that its staging-to-record half (`update::install::place`) also installs a local file. The tray gains `quit` (a Unix socket next to `tray.lock`) and the same `opt` path rules. `install.sh` only downloads, verifies and hands over to `mailtriage self install`. Shell helpers (`scripts/add-himalaya-version.sh`, `packaging/homebrew/render.sh` and `publish.sh`) carry the CI logic and are tested from Rust; workflows only call them.

**Tech Stack:** Rust 2021 (stable 1.92), clap 4, serde/serde_json, toml, reqwest blocking (rustls), fs2, flate2/tar, sha2; POSIX `sh` for `install.sh` (dash on Ubuntu, bash as `sh` on macOS); bash and python3 for CI helpers; GitHub Actions; Homebrew. No new crates: `Cargo.toml` and `Cargo.lock` do not change.

**Spec:** `docs/superpowers/specs/2026-10-07-install-and-distribution-design.md`. Read it before every task. Where this plan and the spec disagree, the spec wins, except for the rulings under "Decisions this plan adds". It builds on `docs/superpowers/specs/2026-10-06-auto-update-design.md` (install transaction, cache, restart rule) and `docs/superpowers/specs/2026-10-06-tray-design.md` (tray instances, paths, restart, autostart).

## Execution

Subagent-driven development directly on `main` (no worktree), Opus for every role. One commit per task; push nothing. Pushing `packaging/homebrew/tap/` to `wir-drei-digital/homebrew-tap` is not part of this plan: the main session does that with the user's go-ahead.

Every code block below was compiled and every test run on macOS arm64 (rustc 1.92) in a scratch copy, task by task in this order, with `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings` and `cargo test --workspace --locked` green after each task. `install.sh` was also run under `dash`. Linux-only paths (`/proc/self/exe`, systemd fakes, the tray packages note, `without_a_service_manager_there_are_no_services`) run only in CI. `shellcheck` and `brew style` were not available locally; CI runs them (Tasks 10 and 11).

Once in that run, `tests/key_unavailable.rs`'s `an_unavailable_key_leaves_mail_queued_until_the_key_works` failed with `an account worker is already running` and passed when the suite ran again; no task touches the worker lock or that test. If it fails, rerun before suspecting the task.

## Verified Himalaya resolver behaviour

Read from pimalaya/himalaya's source at tags `v2.1.0` and `v2.2.1` on 2026-10-07; Task 2 pins it in tests.

- `message read --mailbox NAME` resolves `NAME` with `Account::resolve_mailbox` (`src/account/context.rs`): the key is `NAME.to_lowercase()`, looked up in `mailbox_alias`, whose keys were lowercased with `to_lowercase` when loaded; no match returns `NAME` itself. Both versions.
- The alias map is the global `[mailbox] alias` table merged with the account's (`Account::merge`: `mailbox_alias.extend(account's)`), so an account key replaces the global one. `aliases` is accepted as another spelling (`#[serde(rename = "alias", alias = "aliases")]`); both in one table make serde refuse the file. Both versions.
- 2.2.1 then resolves roles for IMAP only through `parse_mailbox` (`ImapMailbox::try_from`), which treats `inbox` in any ASCII case as `INBOX`; `resolve_mailbox_id` is the identity for IMAP. So the 2.2.1 `roles` table is `{"inbox": "INBOX"}` and 2.1.0's is `{}`.
- `imap fetch --mailbox` and `imap status MAILBOX`, which mailtriage also runs, take a raw IMAP name and resolve no alias (`src/imap/fetch.rs`: `self.mailbox_name.inner.try_into()`). Only `message read` resolves.
- `himalaya --version` prints `himalaya vX.Y.Z +features…` on its first line (`long_version!()`).
- Release archives `himalaya.{aarch64-darwin,x86_64-linux,aarch64-linux}.tgz` hold `himalaya` at the top level plus `share/` with man pages, completions and JSON schemas: 98 entries in 2.1.0 and 111 in 2.2.1, no links, up to about 22 MB unpacked. The six digests in the spec match the downloaded assets and the release API's `digest` fields (`sha256:<hex>`).
- `https://github.com/wir-drei-digital/mailtriage/releases/latest` currently redirects to `…/releases` (no release yet), which `install.sh` refuses as an unexpected URL.

## Decisions this plan adds

Rulings on points the spec leaves open. Each says what it costs if wrong.

1. **Test-only switches, debug builds only.** `MAILTRIAGE_TEST_HIMALAYA_VERSIONS` names a data file that replaces the compiled one (tests serve archives with digests they know); `MAILTRIAGE_TEST_TERMINAL=1` makes `self install`/`self uninstall` treat stdin as a terminal. Release builds never read them. `himalaya install` downloads below the existing loopback-only `MAILTRIAGE_UPDATE_URL` (all builds), and `MAILTRIAGE_UPDATE_TEST_HOOK` gains the point `uninstall_service`. Cost: one `cfg` each.
2. **The data file is ascending.** Entries must be strictly increasing `X.Y.Z` versions with lowercase role names and exactly the three platform digests; the newest is the last. A unit test parses the compiled file. Cost: none.
3. **Refusal texts.** `Himalaya X is not a tested version (tested: 2.1.0, 2.2.1)`, `Himalaya X was built without IMAP (+imap)`, and `Himalaya printed no version mailtriage knows (tested: …)`. A pass with an untested Himalaya exits 3 with that text instead of the generic `Operation failed…`. Cost: strings.
4. **`doctor.transport.alias_conflicts`** lists the source folders that conflict, in every filing mode; conflicts alone keep `ready`. A Himalaya config that cannot be parsed makes the transport not ready (`cannot read the Himalaya configuration: …`). Cost: one field.
5. **The read check has two layers.** `Himalaya::fetch` (the only `message read`) refuses a conflicting folder (`alias_conflict:FOLDER: …`, exit 3) and reads nothing with an unparseable config, so every read path, `reclassify` included, is covered; there a refused fetch counts as a failed attempt. A sync pass also keeps such messages queued without an attempt: with filing off the sources are checked, after a failed folder resolution they are checked again, and an unparseable config processes only messages already fetched (`Store::queued_fetched`). Cost: a query and a struct.
6. **Ambiguous alias keys** (two keys of one table equal after lowercasing, with different targets) are a conflict, since Himalaya's pick is unspecified. Cost: none.
7. **Himalaya archive limits** are `archive::HIMALAYA` (1024 entries, 256 MB in total, 200 MB file, 255-byte paths); links still fail the archive. Cost: a constant.
8. **`himalaya install` details.** A matching copy (a regular file whose `--version` names the version with `+imap`) is `current` and nothing is downloaded. It takes `.mailtriage-update.lock` in the version directory; when that is held for 60 s it exits 3 (`another himalaya install is running`), since the spec lists no exit 5 for it. The version is checked before anything is created. Without `HOME` and an absolute `XDG_DATA_HOME` it exits 3. Cost: strings.
9. **Where the protected path rule applies.** `self install` checks DIR's canonical path, so a DIR given as a symlink to a safe directory is fine, and so is `/tmp` → `/private/tmp` on macOS. `himalaya install` checks the canonical data directory plus `mailtriage/himalaya/VERSION` read without following symlinks, so a symlinked version directory is refused. The existing part of a path is checked before anything is created in it; a sticky world-writable directory passes while the user's own directory is being created below it. `self install` also requires DIR to be the user's (`Why::NotYours`, "use a directory you own"). Cost: one function each.
10. **`self install` always replaces**, the same version included (re-running the installer repairs, and services restart onto the file). The probe expects this binary's version, and the tray file must print the same version or it is `skipped`. An installed `mailtriage` that does not run does not block. An existing non-regular `DIR/mailtriage` (a link, a directory) exits 3. On Linux the binary reads its image from `/proc/self/exe`. Cost: none.
11. **Setup runs as `DIR/mailtriage setup --interactive --json`.** `--interactive` is a no-op with a terminal and lets tests drive the prompts through a pipe. `self install`'s own questions take the default at the end of input (`Prompter::confirm_or`). A failed setup prints `setup: "failed"` with exit 3 and the message on stderr. Cost: one flag.
12. **The login item.** `autostart` gains `failed` (exit code unaffected) when `autostart enable` fails. When setup did not run and a tray was installed, the command is printed with setup's default config path (`MAILTRIAGE_CONFIG` or `~/.config/mailtriage/mailtriage.json`) standing in for the config placeholder, also without a terminal (spec step 8; its decision table says only "no"). Cost: one line.
13. **Results that fail.** `self install` (failed tray or setup) and `self uninstall` (a failed step) print their result object and exit 3 through an internal top-level `exit_code` key that `cli::run` removes before printing. Cost: one branch.
14. **`self uninstall` output** adds `failures` (strings; the spec's "the output lists the failures") and lists a removed login item in `removed`. `tray` is `quit`, `not_running`, `other_installation`, `not_installed` or `failed`. DIR must exist (exit 2). Homebrew means DIR or `DIR/mailtriage`, canonicalized, contains `/Cellar/mailtriage/`. Service locks wait 30 s, as service commands do. Cost: fields.
15. **The CLI reads the tray's login item itself** (`distribution::login_item`): the tray crate is not a dependency of the CLI, so the decoder is duplicated; both crates' tests pin the same file texts. Cost: about 60 duplicated lines.
16. **The quit protocol.** The client sends `quit <canonical path>\n`; the tray answers `quit`, `other_installation` or `unknown`, with a 5 s read timeout, and stops listening after a quit. `quit` then waits up to 5 s for `tray.lock`. Output: `{"schema_version":1,"quit":"…"}` with `--json`, else the word. Same installation means equal canonical paths, or both in a keg of the same prefix (by path shape, since the old keg may be deleted). A tray that cannot bind its socket (a path longer than the platform allows) runs without it and says so. Cost: a module.
17. **Homebrew launch paths** are computed from the path's shape (`<prefix>/Cellar/mailtriage/<version>/bin/<name>`) and used when `<prefix>/opt/mailtriage/bin/<name>` exists: by `service install` (other installs keep recording `current_exe()` as before), by autostart for the tray and `--mailtriage`, by the tray for its CLI and for starting windows, and by both restart rules, whose re-exec target is always the `opt` path of a keg. The tray crate duplicates the two functions. Cost: duplication.
18. **`install.sh` details.** A `MAILTRIAGE_INSTALL_URL` that is not `http://127.0.0.1…` or `http://localhost…`, or contains `@`, exits 2 instead of being ignored, so a mistyped mirror never downloads from GitHub silently. Only with that override does `MAILTRIAGE_INSTALL_TTY` replace `/dev/tty` (tests). `--uninstall` skips the tool and platform checks. Environment booleans must be empty, 0 or 1. The truncation test skips the one cut that only drops the trailing newline, which leaves the whole script. Cost: lines.
19. **Release archives** are packed with `COPYFILE_DISABLE=1`, and the build checks their exact layout, because `install.sh` refuses anything else. `install.sh` is attached to releases but not listed in `SHA256SUMS`. Cost: one CI step.
20. **Real runs** are `install-check.yml`, dispatched by hand after a release; a schedule would fail until the first release with the script exists. Cost: a manual step, named in `docs/releases.md`.
21. **The weekly check** does nothing more when `himalaya/VERSION` already exists on the remote (no duplicate pull requests). Its `report` job takes the version from the `test` job's output and falls back to asking for the latest release. Cost: none.
22. **The tap update is a script** (`packaging/homebrew/publish.sh`, bash with a python3 version comparison), so the never-backwards and conflict-retry logic is tested against local bare repositories; `render.sh` stays POSIX `sh`. Cost: one more script.
23. **The tap's test workflow** skips its steps until `Formula/mailtriage.rb` exists, so the first push of its README and workflow is green. Cost: conditions.
24. **The `mail` item's fix** names the concrete path `himalaya install` would print (the newest tested version in the data directory), falling back to `<the path it prints>`; setup's step 2 failure appends `, or pass --himalaya-install`. Cost: strings.
25. **`--himalaya-install` answers the offer**: it acts only when the Himalaya step 2 picked (the flag's, the stored account's or the one on `PATH`) is missing or untested. Cost: an agent that wants a private copy despite a tested one runs `himalaya install` and passes `--himalaya-binary`.
26. **Permitted test edit:** `tests/adapters.rs`'s epoch-reset fake answers `--version`, because `fetch` now needs the reported version's roles. Cost: one line.

## Global Constraints

- Work on `main`; commit at the end of every task; every commit message ends with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Push nothing.
- After every task all three pass: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`. CI runs them on Linux amd64, Linux arm64 and macOS arm64, so a test that depends on the platform is `#[cfg]`-gated or branches on `cfg!`. Where `shellcheck` is installed, also run `shellcheck -s sh install.sh` and `shellcheck scripts/add-himalaya-version.sh packaging/homebrew/render.sh packaging/homebrew/publish.sh`.
- No new crate dependencies; `Cargo.toml`, `tray/Cargo.toml` and `Cargo.lock` stay unchanged.
- Tested versions: exactly 2.1.0 (`roles` `{}`) and 2.2.1 (`roles` `{"inbox":"INBOX"}`), with the spec's six digests verbatim.
- Version acceptance: "`himalaya --version` must print `himalaya v<a listed version> … +imap …` on its first line. Any other version, a newer patch release included, is refused until it is listed." `expected_version` stays in the schema, is written by setup, and is not compared.
- Every `install.sh` request uses `curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL --max-redirs 10 --connect-timeout 30 --max-time 600` (with `=http` only under the loopback test override).
- Platforms: `Darwin arm64` → `macos-arm64`; `Linux x86_64` → `linux-amd64`; `Linux aarch64|arm64` → `linux-arm64`; pimalaya's names `aarch64-darwin`, `x86_64-linux`, `aarch64-linux`.
- Results keep `schema_version: 1` and the error envelope `{"schema_version":1,"error":{"code","message"[,"reason"]}}`: `{"himalaya":{"action","version","path"}}`, `{"self_install":{"dir","cli":{"action","version","path"},"tray":{"action","error"}|null,"on_path","shadowed_by","setup","autostart"}}`, `{"self_uninstall":{"dir","services":[{"account","unit_path","action","error"}],"tray","removed","kept","failures"}}`.
- Tests never touch the real HOME, `~/.cargo/bin`, `~/.local`, the real update cache, the keychain, real `launchctl`/`systemctl` or the user's running service: `HOME`, `XDG_CACHE_HOME`, `XDG_DATA_HOME` and `XDG_CONFIG_HOME` point into temporary directories. Any test that runs `service install`, `self uninstall` or a setup that installs a service puts the fake `launchctl` and `systemctl` from `tests/common` first on `PATH` (on macOS mailtriage otherwise falls back to `/bin/launchctl`). `install.sh` only ever runs with a loopback `MAILTRIAGE_INSTALL_URL`. Tray tests never start the event loop.
- The OpenRouter key is never written into any file, fixture or doc recipe.
- Edits to shared files stay small and additive; docs get new sections rather than rewrites, except the Install sections the spec rewrites.
- Existing tests keep passing unchanged, except the one permitted edit (ruling 26).

## Review Focus

Inputs the spec implies but its own test list would not exercise, most likely to bite first. Each has a pinned test in the task named.

1. **pimalaya's real archive** holds 111 entries (man pages, completions, schemas), far above the 16 a mailtriage release archive may hold. Expected: it installs. Test: Task 3 `an_archive_shaped_like_pimalayas_installs`.
2. **macOS `tar` metadata in mailtriage's own archives.** bsdtar may add `._mailtriage` entries, which `install.sh`'s strict layout check would refuse, breaking every macOS install. Expected: archives hold exactly their files. Tests: Task 10's build step "Check the archives' layout" on the real macOS runner, and `an_archive_that_is_not_the_release_layout_installs_nothing`.
3. **A socket path past the platform limit** (104 bytes on macOS, from a long `HOME` or `XDG_CACHE_HOME`). Expected: the tray still runs; `quit` fails safe (exit 3), so `self uninstall` deletes nothing. Test: Task 7 `a_socket_path_that_is_too_long_is_not_fatal`.
4. **`--dir /usr/local/bin`**, the habit the old README taught with `sudo install`. Expected: refused before anything is written, with "not to you" and the remedy. Test: Task 8 `a_root_owned_directory_is_refused_before_anything_is_written`.
5. **Re-running the one-line installer** over an install of the same version while its service runs. Expected: reinstalled, `.previous` kept, the service restarts onto it, never a downgrade. Tests: Task 8 `a_running_service_restarts_onto_the_installed_file`, Task 10 `the_real_binary_installs_itself`.

## File Structure

| Path | Task | Responsibility |
| --- | --- | --- |
| `src/engine/himalaya-versions.json` (new) | 1 | Tested versions, digests, roles |
| `src/engine/versions.rs` (new) | 1 | Parsing the data file; version acceptance |
| `src/engine/targets.rs` (new) | 2 | Effective target: merged aliases, roles, conflicts |
| `src/engine/himalaya.rs` | 1, 2 | Any tested version; resolver-based `alias_conflicts`; read check in `fetch` |
| `src/engine/fake.rs`, `src/filing/observe.rs`, `src/filing/store.rs` | 2 | `fail_alias_check`; `FolderMap.alias_checked`/`reads_blocked`; `queued_fetched` |
| `src/service.rs` | 1, 2, 3 | Doctor's transport block; version error; read guard in passes; `ErrorKind::UnsafePermissions` |
| `src/setup.rs`, `src/cli.rs` | 1, 3, 4, 8, 9 | Step 2 (tested versions, offer, brew note, mail fix); `himalaya install`, `self install`, `self uninstall`; `exit_code` results |
| `src/distribution/protected.rs` (new) | 3 | The protected path rule |
| `src/distribution/himalaya.rs` (new) | 3, 4 | `mailtriage himalaya install` |
| `src/update/{archive,github,install,platform}.rs` | 3, 8 | Himalaya limits and URL; shared staging; `run_version`; `place` |
| `src/distribution/brew.rs` (new), `src/system_service.rs`, `src/update/restart.rs` | 6 | `opt` launch paths for services and restarts |
| `tray/src/{brew,quit}.rs` (new), `tray/src/{paths,autostart,restart,tray,args,main,instances,lib}.rs` | 6, 7 | Tray `opt` paths; `mailtriage-tray quit` |
| `src/prompt.rs`, `src/distribution/self_install.rs` (new) | 8 | `confirm_or`; `self install` |
| `src/distribution/{login_item,self_uninstall}.rs` (new) | 9 | Tray login item reader; `self uninstall` |
| `.github/scripts/himalaya_matrix.py`, `scripts/add-himalaya-version.sh`, `tests/e2e/run.sh`, `.github/workflows/{e2e,himalaya-compat}.yml` | 5 | E2E matrix and the weekly check |
| `install.sh`, `.github/workflows/{ci,build,release,install-check}.yml` | 10, 11 | The script, its CI and release asset |
| `packaging/homebrew/{mailtriage.rb.in,render.sh,publish.sh}`, `packaging/homebrew/tap/{README.md,.github/workflows/test.yml}` | 11 | Formula, tap update, tap repository files |
| `tests/install_support/mod.rs` (new) | 4, 8 | Fake Himalaya and tray programs |
| `tests/*.rs`, `tray/tests/*.rs` (new files named in each task) | 1–11 | Tests |
| `README.md`, `docs/{guide,hermes,releases,verification,service-api}.md` | 1–11 | Each task documents what it adds |

---

### Task 1: Tested Himalaya versions

The single source of tested versions (`src/engine/himalaya-versions.json`, compiled in with `include_str!`), version acceptance for any listed version, `doctor`'s `transport.tested`, the version error of passes, and setup accepting and recording any tested version. `expected_version` stays in the schema and is written by setup, but nothing compares it any more.

**Files:**
- Create: `src/engine/himalaya-versions.json`, `src/engine/versions.rs`
- Modify: `src/engine/mod.rs`, `src/engine/himalaya.rs`, `src/service.rs`, `src/setup.rs`, `README.md`, `docs/guide.md`, `docs/hermes.md`
- Test: `tests/himalaya_versions.rs` (new)

**Interfaces:**
- Consumes: `update::version::parse_tag(&str) -> Option<semver::Version>` (on `main`).
- Produces:
  - `engine::versions::{DATA, PLATFORMS, TEST_OVERRIDE}`; `Tested { version: String, roles: BTreeMap<String, String>, assets: BTreeMap<String, String> }` with `fn sha256(&self, platform: &str) -> Option<&str>`.
  - `engine::versions::parse(&str) -> Result<Vec<Tested>, String>`, `tested() -> &'static [Tested]`, `find(&str) -> Option<&'static Tested>`, `newest() -> &'static Tested`, `listed() -> String` (`"2.1.0, 2.2.1"`).
  - `engine::versions::Untested { line: String, version: Option<String>, no_imap: bool }` (`Display`, `std::error::Error`); `check_version_output(&[u8]) -> Result<(String, &'static Tested), Untested>`.
  - `Himalaya::version(&self) -> anyhow::Result<String>` (an `Untested` error for other versions) and `Himalaya::tested(&self) -> anyhow::Result<&'static Tested>`. The old `engine::himalaya::check_version_output` is removed (only setup used it).
  - `doctor`: `transport.tested`; for an untested version `transport.version` is still the first line and `transport.error` the `Untested` text.

- [ ] **Step 1: Write the failing tests**

Create `tests/himalaya_versions.rs`:

```rust
#![cfg(unix)]
//! Tested Himalaya versions: any listed version is accepted whatever
//! `expected_version` says, and an untested one is reported by `doctor`,
//! refused by passes (exit 3) and by setup.
use mailtriage::{domain::HimalayaConfig, engine::himalaya::Himalaya};
use serde_json::{json, Value};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, process::Command};

/// A fake Himalaya that prints the `version` file next to it for
/// `--version`, lists the account `work`, passes `account check`, reports
/// capabilities without SPECIAL-USE, lists INBOX, and fails everything else.
const FAKE: &str = r#"#!/bin/sh
dir="$(dirname "$0")"
for a in "$@"; do last="$a"; done
case "$last" in
  --version) cat "$dir/version"; exit 0 ;;
esac
case "$*" in
  *"account list"*) printf '{"accounts":[{"name":"work","default":true,"backends":["imap"]}]}' ;;
  *"account check"*) printf '{"account":"work","backends":[{"backend":"imap","ok":true}]}' ;;
  *"imap raw"*) printf '* CAPABILITY IMAP4rev1 MOVE\r\na1 OK done\r\n* NAMESPACE (("" "/")) NIL NIL\r\na2 OK done\r\n' ;;
  *"imap list"*) printf '{"mailboxes":[{"name":"INBOX","delimiter":"/","attributes":[]}]}' ;;
  *) exit 7 ;;
esac
"#;

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new(version_line: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let f = Self { dir };
        let bin = f.bin();
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("himalaya"), FAKE).unwrap();
        fs::set_permissions(bin.join("himalaya"), fs::Permissions::from_mode(0o755)).unwrap();
        f.set_version(version_line);
        fs::create_dir_all(f.home().join(".config/himalaya")).unwrap();
        fs::write(
            f.toml(),
            "[accounts.work]\nemail = \"work@example.test\"\nimap.server = \"imaps://mail.example.test\"\n",
        )
        .unwrap();
        f
    }

    fn root(&self) -> PathBuf {
        fs::canonicalize(self.dir.path()).unwrap()
    }
    fn bin(&self) -> PathBuf {
        self.root().join("bin")
    }
    fn home(&self) -> PathBuf {
        self.root().join("home")
    }
    fn toml(&self) -> PathBuf {
        self.home().join(".config/himalaya/config.toml")
    }
    fn set_version(&self, line: &str) {
        fs::write(self.bin().join("version"), line).unwrap();
    }

    /// `init` plus an engine for the fake, expecting `expected`.
    fn config(&self, expected: &str) -> PathBuf {
        let path = self.root().join("mailtriage.json");
        let (code, v) = self.run(&["init", "--json", "--config", path.to_str().unwrap()]);
        assert_eq!(code, Some(0), "{v}");
        let mut c: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        c["accounts"]["work"]["engine"] = json!({
            "kind": "himalaya", "binary": self.bin().join("himalaya"), "config": self.toml(),
            "account": "work", "mailboxes": ["INBOX"], "expected_version": expected,
            "timeout_seconds": 5, "max_output_bytes": 100000
        });
        fs::write(&path, c.to_string()).unwrap();
        path
    }

    fn run(&self, args: &[&str]) -> (Option<i32>, Value) {
        let out = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
            .args(args)
            .current_dir(self.root())
            .env("HOME", self.home())
            .env("XDG_CACHE_HOME", self.root().join("cache"))
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin().display()))
            .env_remove("MAILTRIAGE_CONFIG")
            .env_remove("HIMALAYA_CONFIG")
            .env_remove("XDG_CONFIG_HOME")
            .output()
            .unwrap();
        (
            out.status.code(),
            serde_json::from_slice(&out.stdout).unwrap_or(Value::Null),
        )
    }
}

fn engine(f: &Fixture, expected: &str) -> Himalaya {
    Himalaya::new(&HimalayaConfig {
        binary: f.bin().join("himalaya"),
        config: f.toml(),
        account: "work".into(),
        mailboxes: vec!["INBOX".into()],
        expected_version: expected.into(),
        timeout_seconds: 5,
        max_output_bytes: 100_000,
    })
    .unwrap()
}

#[test]
fn a_config_that_expects_2_1_0_accepts_2_2_1() {
    let f = Fixture::new("himalaya v2.2.1 +smtp +imap\nbuild: test\n");
    let h = engine(&f, "2.1.0");
    assert_eq!(h.version().unwrap(), "himalaya v2.2.1 +smtp +imap");
    assert_eq!(h.tested().unwrap().version, "2.2.1");
    // A value no binary ever wrote is not compared either.
    assert!(engine(&f, "9.9.9").version().is_ok());
}

#[test]
fn doctor_reports_whether_the_version_is_tested() {
    let f = Fixture::new("himalaya v2.2.1 +imap\n");
    let config = f.config("2.1.0");
    let config = config.to_str().unwrap();
    let (code, v) = f.run(&["doctor", "--account", "work", "--json", "--config", config]);
    assert_eq!(code, Some(0), "{v}");
    let t = &v["transport"];
    assert_eq!(
        (
            t["ready"].clone(),
            t["tested"].clone(),
            t["version"].clone()
        ),
        (json!(true), json!(true), json!("himalaya v2.2.1 +imap"))
    );
    f.set_version("himalaya v2.2.2 +imap\n");
    let (code, v) = f.run(&["doctor", "--account", "work", "--json", "--config", config]);
    assert_eq!(code, Some(0), "{v}");
    let t = &v["transport"];
    assert_eq!(t["ready"], false, "{v}");
    assert_eq!(t["tested"], false);
    assert_eq!(t["version"], "himalaya v2.2.2 +imap");
    assert_eq!(
        t["error"],
        "Himalaya 2.2.2 is not a tested version (tested: 2.1.0, 2.2.1)"
    );
    assert_eq!(v["ready"], false);
}

#[test]
fn a_pass_with_an_untested_version_exits_3_naming_it() {
    let f = Fixture::new("himalaya v2.2.2 +imap\n");
    let config = f.config("2.2.1");
    let (code, v) = f.run(&[
        "sync",
        "--account",
        "work",
        "--json",
        "--config",
        config.to_str().unwrap(),
    ]);
    assert_eq!(code, Some(3), "{v}");
    assert_eq!(
        v["error"]["message"],
        "Himalaya 2.2.2 is not a tested version (tested: 2.1.0, 2.2.1)"
    );
}

#[test]
fn setup_accepts_any_tested_version_and_writes_it() {
    let f = Fixture::new("himalaya v2.2.1 +imap\n");
    let setup = [
        "setup",
        "--yes",
        "--json",
        "--himalaya-account",
        "work",
        "--provider",
        "fake",
    ];
    let (code, v) = f.run(&setup);
    assert_eq!(code, Some(0), "{v}");
    let written: Value = serde_json::from_slice(
        &fs::read(f.home().join(".config/mailtriage/mailtriage.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        written["accounts"]["work"]["engine"]["expected_version"],
        "2.2.1"
    );
    fs::remove_file(f.home().join(".config/mailtriage/mailtriage.json")).unwrap();
    f.set_version("himalaya v2.2.2 +imap\n");
    let (code, v) = f.run(&setup);
    assert_eq!(code, Some(3), "{v}");
    let message = v["error"]["message"].as_str().unwrap();
    assert!(message.starts_with("step 2 (Himalaya): "), "{message}");
    assert!(
        message.contains("Himalaya 2.2.2 is not a tested version (tested: 2.1.0, 2.2.1)"),
        "{message}"
    );
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test --locked --test himalaya_versions`
Expected: FAIL to compile: E0599, no method named `tested` for `Himalaya`.

- [ ] **Step 3: Implement**

The data file uses the spec's layout, one entry per version with `roles` and the three digests. `versions.rs` carries its own unit tests (the compiled file parses, the roles tables are pinned, malformed files are refused, acceptance cases).

Create `src/engine/himalaya-versions.json`:

```json
{"versions":[
  {"version":"2.1.0","roles":{},
   "assets":{"aarch64-darwin":"sha256:a5a787b7c4dbf065408e7772908fc75c799626f4cebab8e9c78fafe3e2fa585c","x86_64-linux":"sha256:683a2ab8e1534f01e6bda3a69e204d564c31fbfbe20511fc7bc60b67f2e85884","aarch64-linux":"sha256:c41adab4bc220ba816cdbf865a5df8dc3b358b39ec58b4be0ed2f64e46b1d182"}},
  {"version":"2.2.1","roles":{"inbox":"INBOX"},
   "assets":{"aarch64-darwin":"sha256:a5d97a1f7bbca45e58bddde3f9f17325f614c9cd6d5f17fabf38ddcb030869ab","x86_64-linux":"sha256:5c5ba2724c162f82d0a0c71b6c03224ed44f8bef7b14ace5da0635d3af2665a0","aarch64-linux":"sha256:1dc21c3dd6d948929e22e5cae49b9d0b3b30ff88fafd4493fd4460c6252e9d90"}}
]}
```

Create `src/engine/versions.rs`:

```rust
//! The Himalaya versions mailtriage is tested with. `himalaya-versions.json`
//! is the single source: compiled into the binary, parsed once, it lists each
//! tested version, the SHA-256 of pimalaya's release archive per platform,
//! and the mailbox roles that version's `--mailbox` resolution maps.
use semver::Version;
use serde::Deserialize;
use std::{collections::BTreeMap, fmt, sync::OnceLock};

/// The data file as compiled in.
pub const DATA: &str = include_str!("himalaya-versions.json");
/// pimalaya's platform names, one per platform mailtriage releases for.
pub const PLATFORMS: [&str; 3] = ["aarch64-darwin", "x86_64-linux", "aarch64-linux"];
/// Debug builds only: a data file that replaces the compiled one, so tests
/// can serve archives whose digests they know. Release builds never read it.
pub const TEST_OVERRIDE: &str = "MAILTRIAGE_TEST_HIMALAYA_VERSIONS";

/// One tested Himalaya version.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tested {
    /// `X.Y.Z`, without the `v`.
    pub version: String,
    /// Lowercase role name to the mailbox `--mailbox ROLE` opens on IMAP.
    pub roles: BTreeMap<String, String>,
    /// pimalaya platform to `sha256:<64 lowercase hex>` of
    /// `himalaya.<platform>.tgz`.
    pub assets: BTreeMap<String, String>,
}

impl Tested {
    /// The archive's SHA-256 for `platform`, as 64 lowercase hex digits.
    pub fn sha256(&self, platform: &str) -> Option<&str> {
        self.assets.get(platform)?.strip_prefix("sha256:")
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    versions: Vec<Tested>,
}

/// Parses a data file. Every entry needs an exact `X.Y.Z` version, higher
/// than the one before, lowercase role names, and exactly one well-formed
/// digest for each of `PLATFORMS`.
pub fn parse(text: &str) -> Result<Vec<Tested>, String> {
    let file: File = serde_json::from_str(text).map_err(|e| format!("invalid JSON: {e}"))?;
    if file.versions.is_empty() {
        return Err("no versions".into());
    }
    let mut previous: Option<Version> = None;
    for entry in &file.versions {
        let v = &entry.version;
        let version = crate::update::version::parse_tag(&format!("v{v}"))
            .ok_or_else(|| format!("version {v:?} is not X.Y.Z"))?;
        if previous.as_ref().is_some_and(|p| *p >= version) {
            return Err(format!("version {v} is not higher than the one before"));
        }
        previous = Some(version);
        if let Some(role) = entry.roles.keys().find(|r| r.to_lowercase() != **r) {
            return Err(format!("{v}: role {role:?} is not lowercase"));
        }
        if entry.assets.len() != PLATFORMS.len() {
            return Err(format!("{v}: needs a digest for each of {PLATFORMS:?}"));
        }
        for platform in PLATFORMS {
            let hex = entry
                .sha256(platform)
                .ok_or_else(|| format!("{v}: no sha256 digest for {platform}"))?;
            let lower_hex = |b: u8| b.is_ascii_digit() || (b'a'..=b'f').contains(&b);
            if hex.len() != 64 || !hex.bytes().all(lower_hex) {
                return Err(format!("{v}: the {platform} digest is malformed"));
            }
        }
    }
    Ok(file.versions)
}

/// The tested versions, oldest first.
pub fn tested() -> &'static [Tested] {
    static TESTED: OnceLock<Vec<Tested>> = OnceLock::new();
    TESTED.get_or_init(load)
}

#[cfg(debug_assertions)]
fn load() -> Vec<Tested> {
    if let Some(path) = std::env::var_os(TEST_OVERRIDE) {
        let text = std::fs::read_to_string(&path).expect("the test's Himalaya versions file");
        return parse(&text).expect("the test's Himalaya versions file is valid");
    }
    parse(DATA).expect("the compiled Himalaya versions file is valid")
}

#[cfg(not(debug_assertions))]
fn load() -> Vec<Tested> {
    parse(DATA).expect("the compiled Himalaya versions file is valid")
}

/// The entry for `version` (`X.Y.Z`), when it is tested.
pub fn find(version: &str) -> Option<&'static Tested> {
    tested().iter().find(|t| t.version == version)
}

/// The newest tested version: what `himalaya install` installs by default.
pub fn newest() -> &'static Tested {
    tested().last().expect("at least one tested version")
}

/// `2.1.0, 2.2.1`: the tested versions for messages.
pub fn listed() -> String {
    tested()
        .iter()
        .map(|t| t.version.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// A Himalaya that mailtriage does not accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Untested {
    /// The first line `himalaya --version` printed.
    pub line: String,
    /// The version that line names (`v` dropped), when it names one.
    pub version: Option<String>,
    /// The version is tested, but the build lacks `+imap`.
    pub no_imap: bool,
}

impl fmt::Display for Untested {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.version, self.no_imap) {
            (Some(v), true) => write!(f, "Himalaya {v} was built without IMAP (+imap)"),
            (Some(v), false) => write!(
                f,
                "Himalaya {v} is not a tested version (tested: {})",
                listed()
            ),
            (None, _) => write!(
                f,
                "Himalaya printed no version mailtriage knows (tested: {})",
                listed()
            ),
        }
    }
}

impl std::error::Error for Untested {}

/// The first line of `himalaya --version` and its tested entry. The line
/// must be `himalaya v<tested version> …` with `+imap` among its words; any
/// other version, a newer patch release included, is refused.
pub fn check_version_output(output: &[u8]) -> Result<(String, &'static Tested), Untested> {
    let text = String::from_utf8_lossy(output);
    let line = text.lines().next().unwrap_or_default().trim().to_owned();
    let mut words = line.split_ascii_whitespace();
    let version = match (words.next(), words.next()) {
        (Some("himalaya"), Some(v)) => v.strip_prefix('v').map(str::to_owned),
        _ => None,
    };
    let imap = words.any(|w| w == "+imap");
    match version.as_deref().and_then(find) {
        Some(tested) if imap => Ok((line, tested)),
        Some(_) => Err(Untested {
            line,
            version,
            no_imap: true,
        }),
        None => Err(Untested {
            line,
            version,
            no_imap: false,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_data_file_lists_well_formed_digests_oldest_first() {
        let tested = parse(DATA).unwrap();
        let versions: Vec<&str> = tested.iter().map(|t| t.version.as_str()).collect();
        assert_eq!(versions, ["2.1.0", "2.2.1"]);
        assert_eq!(listed(), "2.1.0, 2.2.1");
        assert_eq!(newest().version, "2.2.1");
        assert_eq!(
            find("2.1.0").unwrap().sha256("x86_64-linux"),
            Some("683a2ab8e1534f01e6bda3a69e204d564c31fbfbe20511fc7bc60b67f2e85884")
        );
        assert_eq!(
            find("2.2.1").unwrap().sha256("aarch64-darwin"),
            Some("a5d97a1f7bbca45e58bddde3f9f17325f614c9cd6d5f17fabf38ddcb030869ab")
        );
    }

    /// The roles each version's resolver maps for IMAP, read from
    /// Himalaya's source: 2.1.0 resolves aliases only; 2.2.1 resolves
    /// aliases, then the `inbox` role (to `INBOX`), then the literal name.
    #[test]
    fn the_roles_tables_are_pinned() {
        assert!(find("2.1.0").unwrap().roles.is_empty());
        assert_eq!(
            find("2.2.1").unwrap().roles,
            BTreeMap::from([("inbox".to_owned(), "INBOX".to_owned())])
        );
    }

    #[test]
    fn malformed_files_are_refused() {
        let entry = |version: &str, assets: &str| {
            format!(r#"{{"versions":[{{"version":"{version}","roles":{{}},"assets":{assets}}}]}}"#)
        };
        let hex = "a".repeat(64);
        let all = format!(
            r#"{{"aarch64-darwin":"sha256:{hex}","x86_64-linux":"sha256:{hex}","aarch64-linux":"sha256:{hex}"}}"#
        );
        assert!(parse(&entry("2.3.0", &all)).is_ok());
        for (version, assets, why) in [
            ("v2.3.0", all.clone(), "not X.Y.Z"),
            ("2.3", all.clone(), "not X.Y.Z"),
            (
                "2.3.0",
                format!(r#"{{"aarch64-darwin":"sha256:{hex}","x86_64-linux":"sha256:{hex}"}}"#),
                "needs a digest",
            ),
            ("2.3.0", all.replace("sha256:", "sha512:"), "no sha256"),
            ("2.3.0", all.replacen(&hex, &"A".repeat(64), 1), "malformed"),
            ("2.3.0", all.replacen(&hex, &"a".repeat(63), 1), "malformed"),
        ] {
            let error = parse(&entry(version, &assets)).unwrap_err();
            assert!(error.contains(why), "{version} {assets}: {error}");
        }
        let two = format!(
            r#"{{"versions":[{{"version":"2.2.1","roles":{{}},"assets":{all}}},{{"version":"2.1.0","roles":{{}},"assets":{all}}}]}}"#
        );
        assert!(parse(&two).unwrap_err().contains("not higher"));
        let upper = entry("2.3.0", &all).replace(r#""roles":{}"#, r#""roles":{"Inbox":"INBOX"}"#);
        assert!(parse(&upper).unwrap_err().contains("not lowercase"));
        assert!(parse(r#"{"versions":[]}"#).is_err());
        assert!(parse(r#"{"versions":[],"extra":1}"#).is_err());
    }

    #[test]
    fn only_tested_versions_with_imap_are_accepted() {
        for line in [
            "himalaya v2.1.0 +imap\n",
            "himalaya v2.2.1 +smtp +imap +jmap\nbuild: macos aarch64\n",
        ] {
            let (first, tested) = check_version_output(line.as_bytes()).unwrap();
            assert_eq!(first, line.lines().next().unwrap());
            assert!(line.contains(&format!("v{} ", tested.version)));
        }
        let refused = |line: &str| check_version_output(line.as_bytes()).unwrap_err();
        let newer = refused("himalaya v2.2.2 +imap\n");
        assert_eq!(newer.version.as_deref(), Some("2.2.2"));
        assert_eq!(
            newer.to_string(),
            "Himalaya 2.2.2 is not a tested version (tested: 2.1.0, 2.2.1)"
        );
        assert_eq!(
            refused("himalaya v1.2.0 +imap\n").to_string(),
            "Himalaya 1.2.0 is not a tested version (tested: 2.1.0, 2.2.1)"
        );
        let no_imap = refused("himalaya v2.2.1 +smtp\n");
        assert!(no_imap.no_imap);
        assert_eq!(
            no_imap.to_string(),
            "Himalaya 2.2.1 was built without IMAP (+imap)"
        );
        for odd in ["himalaya 2.1.0 +imap\n", "", "himalaya-cli v2.1.0 +imap\n"] {
            let e = refused(odd);
            assert!(!e.no_imap, "{odd:?}");
        }
        // The version is on the first line only.
        assert!(check_version_output(b"something\nhimalaya v2.1.0 +imap\n").is_err());
    }
}
```

In `src/engine/mod.rs`, replace:

```rust
pub mod fake;
pub mod himalaya;
pub mod raw;

use crate::domain::{EngineConfig, MailboxSnapshot, SourceEnvelope};
use anyhow::Result;
```

with:

```rust
pub mod fake;
pub mod himalaya;
pub mod raw;
pub mod versions;

use crate::domain::{EngineConfig, MailboxSnapshot, SourceEnvelope};
use anyhow::Result;
```

In `src/engine/himalaya.rs`, replace:

```rust
//! Narrow Himalaya v2.1.0 IMAP adapter. Its only writes are folder create and
//! subscribe, UID MOVE and adding \Flagged, the last two through `imap raw`.
use super::{
    raw, ConfigChanged, EngineCapabilities, FolderInfo, MailEngine, WriteOutcome, SPECIAL_USE_ROLES,
};
use crate::domain::{Address, HimalayaConfig, MailboxSnapshot, SourceEnvelope};
use crate::process::{read_bounded, terminate};
```

with:

```rust
//! Narrow Himalaya IMAP adapter for the tested versions in
//! `himalaya-versions.json`. Its only writes are folder create and
//! subscribe, UID MOVE and adding \Flagged, the last two through `imap raw`.
use super::{
    raw,
    versions::{self, Tested},
    ConfigChanged, EngineCapabilities, FolderInfo, MailEngine, WriteOutcome, SPECIAL_USE_ROLES,
};
use crate::domain::{Address, HimalayaConfig, MailboxSnapshot, SourceEnvelope};
use crate::process::{read_bounded, terminate};
```

In `src/engine/himalaya.rs`, replace:

```rust
    /// SHA-256 of the TOML read at open; every spawn re-checks it.
    config_hash: String,
    caps: OnceCell<EngineCapabilities>,
    /// Folders beyond `config.mailboxes` that this pass may touch.
    scope: RefCell<BTreeSet<String>>,
}
```

with:

```rust
    /// SHA-256 of the TOML read at open; every spawn re-checks it.
    config_hash: String,
    caps: OnceCell<EngineCapabilities>,
    /// The tested version `--version` reported, once read.
    tested: OnceCell<&'static Tested>,
    /// Folders beyond `config.mailboxes` that this pass may touch.
    scope: RefCell<BTreeSet<String>>,
}
```

In `src/engine/himalaya.rs`, replace:

```rust
        if config.account.trim().is_empty() || config.mailboxes.is_empty() {
            bail!("Himalaya account and mailboxes must be configured");
        }
        if config.expected_version != "2.1.0" {
            bail!("Himalaya compatibility target must be 2.1.0");
        }
        if config.timeout_seconds == 0 || config.timeout_seconds > 600 {
            bail!("Himalaya timeout must be between 1 and 600 seconds");
        }
```

with:

```rust
        if config.account.trim().is_empty() || config.mailboxes.is_empty() {
            bail!("Himalaya account and mailboxes must be configured");
        }
        if config.timeout_seconds == 0 || config.timeout_seconds > 600 {
            bail!("Himalaya timeout must be between 1 and 600 seconds");
        }
```

In `src/engine/himalaya.rs`, replace:

```rust
            config: config.clone(),
            config_hash: sha256_hex(&toml),
            caps: OnceCell::new(),
            scope: RefCell::new(BTreeSet::new()),
        })
    }

    pub fn version(&self) -> Result<String> {
        check_version_output(
            &self.run(&["--version"], false)?,
            &self.config.expected_version,
        )
    }

    pub fn snapshot(&self, mailbox: &str) -> Result<MailboxSnapshot> {
```

with:

```rust
            config: config.clone(),
            config_hash: sha256_hex(&toml),
            caps: OnceCell::new(),
            tested: OnceCell::new(),
            scope: RefCell::new(BTreeSet::new()),
        })
    }

    /// The first line of `--version`, when it names a tested version with
    /// `+imap`; otherwise a `versions::Untested` error. `expected_version`
    /// is not compared: any tested version is accepted.
    pub fn version(&self) -> Result<String> {
        let (line, tested) = versions::check_version_output(&self.run(&["--version"], false)?)?;
        let _ = self.tested.set(tested);
        Ok(line)
    }

    /// The tested version this binary reports, read once.
    pub fn tested(&self) -> Result<&'static Tested> {
        if let Some(tested) = self.tested.get() {
            return Ok(tested);
        }
        self.version()?;
        self.tested
            .get()
            .copied()
            .ok_or_else(|| anyhow!("Himalaya version unknown"))
    }

    pub fn snapshot(&self, mailbox: &str) -> Result<MailboxSnapshot> {
```

In `src/engine/himalaya.rs`, replace:

```rust
    Ok(())
}

/// The version line of `himalaya --version` when it is `expected` with IMAP.
pub fn check_version_output(output: &[u8], expected: &str) -> Result<String> {
    let text = std::str::from_utf8(output).context("invalid Himalaya version output")?;
    let version = text.lines().next().unwrap_or_default().trim();
    let mut words = version.split_ascii_whitespace();
    let wanted = format!("v{expected}");
    if words.next() != Some("himalaya")
        || words.next() != Some(wanted.as_str())
        || !words.any(|feature| feature == "+imap")
    {
        bail!("unsupported Himalaya version; expected {expected}");
    }
    Ok(version.to_owned())
}

/// Parses `imap list` JSON: an array of rows or an object with `mailboxes`.
fn folder_rows(output: &[u8]) -> Result<Vec<FolderRow>> {
    let value: Value = serde_json::from_slice(output).context("invalid Himalaya list JSON")?;
```

with:

```rust
    Ok(())
}

/// Parses `imap list` JSON: an array of rows or an object with `mailboxes`.
fn folder_rows(output: &[u8]) -> Result<Vec<FolderRow>> {
    let value: Value = serde_json::from_slice(output).context("invalid Himalaya list JSON")?;
```

In `src/service.rs`, replace:

```rust
            )
        };
        let key_present = key_error.is_none();
        let transport = match self.engine(&account).map(|e| e.map(|e| e.version())) {
            Ok(None) => json!({"configured":false,"ready":true}),
            Ok(Some(Ok(v))) => json!({"configured":true,"ready":true,"version":v}),
            _ => {
                json!({"configured":true,"ready":false,"error":"Himalaya version/config check failed"})
            }
        };
        let mut out = json!({"schema_version":1,"account":name,"ready":provider_valid&&key_present&&transport["ready"]==true,"provider":{"kind":self.config.provider.kind,"model":self.config.provider.model,"configuration_valid":provider_valid,"key_source":key_source,"key_present":key_present},"transport":transport,"review_mode":self.config.policy.review_mode,"state_dir":self.config.state_dir,"live_checks_performed":false,"coverage":self.coverage(name)?});
        if let Some(e) = key_error {
```

with:

```rust
            )
        };
        let key_present = key_error.is_none();
        let failed = || json!({"configured":true,"ready":false,"error":"Himalaya version/config check failed"});
        let transport = match self.engine(&account).map(|e| e.map(|e| e.version())) {
            Ok(None) => json!({"configured":false,"ready":true}),
            Ok(Some(Ok(v))) => json!({"configured":true,"ready":true,"version":v,"tested":true}),
            Ok(Some(Err(e))) => match e.downcast_ref::<engine::versions::Untested>() {
                Some(untested) => {
                    json!({"configured":true,"ready":false,"version":untested.line,"tested":false,"error":untested.to_string()})
                }
                None => failed(),
            },
            Err(_) => failed(),
        };
        let mut out = json!({"schema_version":1,"account":name,"ready":provider_valid&&key_present&&transport["ready"]==true,"provider":{"kind":self.config.provider.kind,"model":self.config.provider.model,"configuration_valid":provider_valid,"key_source":key_source,"key_present":key_present},"transport":transport,"review_mode":self.config.policy.review_mode,"state_dir":self.config.state_dir,"live_checks_performed":false,"coverage":self.coverage(name)?});
        if let Some(e) = key_error {
```

In `src/service.rs`, replace:

```rust
        let now = now();
        self.store.sync_filing_mode(name, mode, &now)?;
        if let Some(h) = &engine {
            h.version().map_err(abort_on_config_change)?;
        }
        let verify_binding = self.binding_verifier(name, &account)?;
        // `Some` only with filing on.
```

with:

```rust
        let now = now();
        self.store.sync_filing_mode(name, mode, &now)?;
        if let Some(h) = &engine {
            h.version().map_err(version_error)?;
        }
        let verify_binding = self.binding_verifier(name, &account)?;
        // `Some` only with filing on.
```

In `src/service.rs`, replace:

```rust
    config_err("mail engine configuration changed during operation")
}

fn abort_on_config_change(e: anyhow::Error) -> anyhow::Error {
    if filing::is_config_changed(&e) {
        config_changed()
```

with:

```rust
    config_err("mail engine configuration changed during operation")
}

/// A Himalaya that is not a tested version fails the pass with exit 3 and
/// says which versions are tested; any other version error is as before.
fn version_error(e: anyhow::Error) -> anyhow::Error {
    match e.downcast_ref::<engine::versions::Untested>() {
        Some(untested) => err(3, untested.to_string()),
        None => abort_on_config_change(e),
    }
}

fn abort_on_config_change(e: anyhow::Error) -> anyhow::Error {
    if filing::is_config_changed(&e) {
        config_changed()
```

In `src/setup.rs`, replace:

```rust
        AccountConfig, AppConfig, Category, EngineConfig, FilingConfig, FilingMode, HimalayaConfig,
        ProviderConfig, UpdateMode,
    },
    engine::{self, himalaya},
    filing, process,
    prompt::Prompter,
    provider,
```

with:

```rust
        AccountConfig, AppConfig, Category, EngineConfig, FilingConfig, FilingMode, HimalayaConfig,
        ProviderConfig, UpdateMode,
    },
    engine::{self, versions},
    filing, process,
    prompt::Prompter,
    provider,
```

In `src/setup.rs`, replace:

```rust

pub const DEFAULT_MODEL: &str = "typesafe/jev-1.13";
pub const DEFAULT_KEY_ENV: &str = "OPENROUTER_API_KEY";
const HIMALAYA_VERSION: &str = "2.1.0";
const HIMALAYA_TIMEOUT: Duration = Duration::from_secs(60);
const HIMALAYA_MAX_OUTPUT: usize = 1024 * 1024;

```

with:

```rust

pub const DEFAULT_MODEL: &str = "typesafe/jev-1.13";
pub const DEFAULT_KEY_ENV: &str = "OPENROUTER_API_KEY";
const HIMALAYA_TIMEOUT: Duration = Duration::from_secs(60);
const HIMALAYA_MAX_OUTPUT: usize = 1024 * 1024;

```

In `src/setup.rs`, replace:

```rust

struct HimalayaChoice {
    binary: PathBuf,
    toml: PathBuf,
    account: String,
    email: Option<String>,
```

with:

```rust

struct HimalayaChoice {
    binary: PathBuf,
    /// The tested version `--version` reported, as `X.Y.Z`; written to
    /// `expected_version`.
    version: String,
    toml: PathBuf,
    account: String,
    email: Option<String>,
```

In `src/setup.rs`, replace:

```rust
        config: h.toml.clone(),
        account: h.account.clone(),
        mailboxes: vec!["INBOX".to_owned()],
        expected_version: HIMALAYA_VERSION.to_owned(),
        timeout_seconds: old_engine.as_ref().map_or(60, |e| e.timeout_seconds),
        max_output_bytes: old_engine
            .as_ref()
```

with:

```rust
        config: h.toml.clone(),
        account: h.account.clone(),
        mailboxes: vec!["INBOX".to_owned()],
        expected_version: h.version.clone(),
        timeout_seconds: old_engine.as_ref().map_or(60, |e| e.timeout_seconds),
        max_output_bytes: old_engine
            .as_ref()
```

In `src/setup.rs`, replace:

```rust
        (None, None) => process::find_on_path("himalaya").ok_or_else(|| {
            err(
                3,
                "step 2 (Himalaya): himalaya is not on PATH; install Himalaya v2.1.0 or pass --himalaya-binary",
            )
        })?,
    };
    let version = run_himalaya(&binary, &[OsStr::new("--version")])
        .and_then(|out| himalaya::check_version_output(&out, HIMALAYA_VERSION))
        .map_err(|_| {
            err(
                3,
                format!(
                    "step 2 (Himalaya): {} is not Himalaya v{HIMALAYA_VERSION} with IMAP; install v{HIMALAYA_VERSION} or pass --himalaya-binary",
                    binary.display()
                ),
            )
        })?;
    p.say(&format!("Using {version} at {}.", binary.display()));
    let explicit = match (&args.himalaya_config, stored) {
        (Some(toml), _) => {
            let toml = absolute(toml, "--himalaya-config")?;
```

with:

```rust
        (None, None) => process::find_on_path("himalaya").ok_or_else(|| {
            err(
                3,
                format!(
                    "step 2 (Himalaya): himalaya is not on PATH; install a tested Himalaya ({}) or pass --himalaya-binary",
                    versions::listed()
                ),
            )
        })?,
    };
    let (line, tested) = match run_himalaya(&binary, &[OsStr::new("--version")]) {
        Ok(out) => versions::check_version_output(&out).map_err(|untested| {
            err(
                3,
                format!(
                    "step 2 (Himalaya): {}: {untested}; install a tested Himalaya or pass --himalaya-binary",
                    binary.display()
                ),
            )
        })?,
        Err(_) => {
            return Err(err(
                3,
                format!(
                    "step 2 (Himalaya): {} does not run; install a tested Himalaya ({}) or pass --himalaya-binary",
                    binary.display(),
                    versions::listed()
                ),
            ))
        }
    };
    p.say(&format!("Using {line} at {}.", binary.display()));
    let explicit = match (&args.himalaya_config, stored) {
        (Some(toml), _) => {
            let toml = absolute(toml, "--himalaya-config")?;
```

In `src/setup.rs`, replace:

```rust
        let email = account_email(&toml, &name);
        return Ok(HimalayaChoice {
            binary,
            toml,
            account: name,
            email,
```

with:

```rust
        let email = account_email(&toml, &name);
        return Ok(HimalayaChoice {
            binary,
            version: tested.version.clone(),
            toml,
            account: name,
            email,
```

- [ ] **Step 4: Run the new tests**

Run: `cargo test --locked --test himalaya_versions && cargo test --locked --lib engine::versions`
Expected: PASS (4 and 4 tests).

- [ ] **Step 5: Document it**

In `README.md`, replace:

```markdown
- [Himalaya v2.1.0](https://github.com/pimalaya/himalaya/releases/tag/v2.1.0) with IMAP support (`himalaya --version` shows `+imap`).
```

with:

```markdown
- [Himalaya](https://github.com/pimalaya/himalaya) with IMAP support (`himalaya --version` shows `+imap`), in a version mailtriage is tested with: 2.1.0 or 2.2.1 ([Himalaya versions](docs/guide.md#himalaya-versions)).
```

In `docs/guide.md`, replace:

```markdown
- [Updates](#updates)
- [Daily use](#daily-use)
```

with:

```markdown
- [Updates](#updates)
- [Himalaya versions](#himalaya-versions)
- [Daily use](#daily-use)
```

In `docs/guide.md`, replace:

```markdown
- Himalaya v2.1.0 with IMAP support,
```

with:

```markdown
- Himalaya with IMAP support, in a [tested version](#himalaya-versions) (2.1.0 or 2.2.1),
```

In `docs/guide.md`, replace:

```markdown
- Binary: `--himalaya-binary`, else the stored binary of the account being updated, else the first `himalaya` on `PATH`. Setup stores it as an absolute path. Its `--version` must report `himalaya v2.1.0` with `+imap`; otherwise setup exits 3.
```

with:

```markdown
- Binary: `--himalaya-binary`, else the stored binary of the account being updated, else the first `himalaya` on `PATH`. Setup stores it as an absolute path. Its `--version` must report a [tested version](#himalaya-versions) with `+imap`; otherwise setup exits 3. Setup writes the version it found into `expected_version`.
```

In `docs/guide.md`, replace:

```markdown
| 3 | Himalaya missing or not v2.1.0 with IMAP; `account check` failed; the folders could not be listed; a key tool or key command failed; the config could not be written; `launchctl` or `systemctl` failed. |
```

with:

```markdown
| 3 | Himalaya missing or not a tested version with IMAP; `account check` failed; the folders could not be listed; a key tool or key command failed; the config could not be written; `launchctl` or `systemctl` failed. |
```

In `docs/guide.md`, replace:

```markdown
mailtriage runs the `himalaya` executable for every mailbox operation. It accepts only Himalaya v2.1.0 with IMAP support: the first line of `himalaya --version` must start with `himalaya v2.1.0` and contain `+imap`. Install it from the [v2.1.0 release](https://github.com/pimalaya/himalaya/releases/tag/v2.1.0) or a package manager, then check:
```

with:

```markdown
mailtriage runs the `himalaya` executable for every mailbox operation. It accepts only the [tested versions](#himalaya-versions) with IMAP support: the first line of `himalaya --version` must start with `himalaya v` and a tested version, such as `himalaya v2.2.1`, and contain `+imap`. Install one from [Himalaya's releases](https://github.com/pimalaya/himalaya/releases) or a package manager, then check:
```

In `docs/guide.md`, replace:

```markdown
| `transport.ready` | `true` | The Himalaya configuration file was read and `himalaya --version` reported v2.1.0 with `+imap`. Otherwise `transport.error` is set. |
| `transport.version` | `himalaya v2.1.0 ...` | The first line of `himalaya --version`. |
```

with:

```markdown
| `transport.ready` | `true` | The Himalaya configuration file was read and `himalaya --version` reported a [tested version](#himalaya-versions) with `+imap`. Otherwise `transport.error` is set. |
| `transport.version` | `himalaya v2.2.1 ...` | The first line of `himalaya --version`. |
| `transport.tested` | `true` | The version is one mailtriage is tested with. `false` makes the transport not ready, with `transport.error` `Himalaya X is not a tested version (tested: 2.1.0, 2.2.1)`. |
```

In `docs/guide.md`, replace:

```markdown
| `expected_version` | `"2.1.0"`. Other values are refused. |
```

with:

```markdown
| `expected_version` | The Himalaya version setup found, such as `"2.2.1"`. It must not be empty, but it is not compared: any [tested version](#himalaya-versions) is accepted whatever it says. The field stays so that older mailtriage versions can read the config. |
```

In `docs/guide.md`, replace:

```markdown
`doctor` reports an engine whose `expected_version`, `timeout_seconds` or `max_output_bytes` is out of range as `transport.ready: false`. Configurations written before schema 2 have a `himalaya` block instead of `engine`. mailtriage still reads it and writes it back as `engine` the next time it saves the file. An account cannot have both.
```

with:

```markdown
`doctor` reports an engine whose `timeout_seconds` or `max_output_bytes` is out of range as `transport.ready: false`. Configurations written before schema 2 have a `himalaya` block instead of `engine`. mailtriage still reads it and writes it back as `engine` the next time it saves the file. An account cannot have both.
```

In `docs/guide.md`, replace:

```markdown
This works only when the newer release did not migrate the state database. An older binary refuses a newer database (`database schema is newer than this binary`); then roll forward to a fixed release instead.

## Daily use
```

with:

```markdown
This works only when the newer release did not migrate the state database. An older binary refuses a newer database (`database schema is newer than this binary`); then roll forward to a fixed release instead.

## Himalaya versions

mailtriage runs Himalaya for every mailbox operation and accepts only the versions it is tested with: **2.1.0** and **2.2.1**. The list, with the SHA-256 of each version's release archives, is compiled into mailtriage from `src/engine/himalaya-versions.json`; a newer mailtriage release can add versions.

- The first line of `himalaya --version` must name a tested version, such as `himalaya v2.2.1 …`, and contain `+imap`. Any other version, a newer patch release included, is refused until a mailtriage release tests it.
- Setup writes the version it found into `engine.expected_version`, but mailtriage does not compare it: a config that says `2.1.0` works with Himalaya 2.2.1.
- `doctor` reports `transport.tested`. An untested Himalaya makes the transport not ready, with `"error": "Himalaya 2.2.2 is not a tested version (tested: 2.1.0, 2.2.1)"`. `sync` and `watch` passes exit 3 with that message, and setup refuses it in step 2.

## Daily use
```

In `docs/hermes.md`, replace:

```markdown
   | 3 | Himalaya is missing or not v2.1.0, `himalaya account check` failed, the folders could not be listed, a key tool or key command failed, or `launchctl`/`systemctl` failed. | Report the message to the user. It names the command that shows the cause; fixing it needs a person (credentials, Himalaya, the key store). |
```

with:

```markdown
   | 3 | Himalaya is missing or not a tested version (see [Himalaya versions](guide.md#himalaya-versions)), `himalaya account check` failed, the folders could not be listed, a key tool or key command failed, or `launchctl`/`systemctl` failed. | Report the message to the user. It names the command that shows the cause; fixing it needs a person (credentials, Himalaya, the key store). |
```

- [ ] **Step 6: Full check and commit**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`
Expected: all pass.

```bash
git add README.md \
  docs/guide.md \
  docs/hermes.md \
  src/engine/himalaya-versions.json \
  src/engine/himalaya.rs \
  src/engine/mod.rs \
  src/engine/versions.rs \
  src/service.rs \
  src/setup.rs \
  tests/himalaya_versions.rs
git commit -m "Accept every tested Himalaya version from a compiled data file

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: The mailbox Himalaya opens, checked before every read

`engine::targets::Resolver` computes each folder's effective target the way the tested versions resolve `--mailbox` (see "Verified Himalaya resolver behaviour"). The Himalaya engine uses it for `alias_conflicts` (now with global aliases, `aliases`, roles and ambiguous keys) and refuses any `message read` through a conflict or with an unparseable config. Passes keep such mail queued in every filing mode, and `doctor` reports source conflicts and an unparseable config.

**Files:**
- Create: `src/engine/targets.rs`
- Modify: `src/engine/mod.rs`, `src/engine/himalaya.rs`, `src/filing/observe.rs`, `src/filing/store.rs`, `src/engine/fake.rs`, `src/service.rs`, `docs/guide.md`, `docs/service-api.md`
- Test: `tests/effective_target.rs` (new), `tests/read_guard.rs` (new), `tests/adapters.rs`

**Interfaces:**
- Consumes: `Himalaya::tested()` and `Tested.roles` (Task 1).
- Produces:
  - `engine::targets::{Target { Mailbox(String), Ambiguous }, Resolver, same_mailbox(&str, &str) -> bool}`; `Resolver::from_toml(text: &str, account: &str, roles: &BTreeMap<String, String>) -> Result<Resolver, String>`, `Resolver::target(&self, &str) -> Target`, `Resolver::conflicts(&self, &[String]) -> Vec<String>`.
  - `filing::observe::FolderMap.alias_checked: bool` and `.reads_blocked: bool` (both default `false`); `Store::queued_fetched(&self, account: &str, limit: usize) -> anyhow::Result<Vec<String>>`.
  - `engine::fake::FakeEngine::fail_alias_check(&self, fails: bool)`.
  - `doctor`: `transport.alias_conflicts` (source folders), and `transport.ready: false` with `error` `cannot read the Himalaya configuration: …`.

- [ ] **Step 1: Write the failing tests**

`tests/adapters.rs` gets the one permitted edit (ruling 26): its epoch-reset fake answers `--version`.

Create `tests/effective_target.rs`:

```rust
#![cfg(unix)]
//! The Himalaya engine computes each folder's effective target from the
//! merged alias map and the reported version's roles, and never runs
//! `message read` for a folder that leads elsewhere or with a configuration
//! it cannot parse.
use mailtriage::{
    domain::HimalayaConfig,
    engine::{himalaya::Himalaya, MailEngine},
};
use std::{fs, os::unix::fs::PermissionsExt};

/// Logs every call to `calls.log`; prints the `version` file for
/// `--version`, a fixed status, and a message for `message read`.
const FAKE: &str = r#"#!/bin/sh
dir="$(dirname "$0")"
printf '%s\n' "$*" >> "$dir/calls.log"
for a in "$@"; do last="$a"; done
case "$last" in
  --version) cat "$dir/version"; exit 0 ;;
esac
case "$*" in
  *"imap status"*) printf '{"uid_validity":9,"uid_next":44}' ;;
  *"message read"*) printf 'Subject: hi\r\n\r\nbody' ;;
  *) exit 7 ;;
esac
"#;

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new(version: &str, toml: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("himalaya");
        fs::write(&bin, FAKE).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            dir.path().join("version"),
            format!("himalaya v{version} +imap\n"),
        )
        .unwrap();
        fs::write(dir.path().join("h.toml"), toml).unwrap();
        Self { dir }
    }

    fn engine(&self, mailboxes: &[&str]) -> Himalaya {
        Himalaya::new(&HimalayaConfig {
            binary: self.dir.path().join("himalaya"),
            config: self.dir.path().join("h.toml"),
            account: "work".into(),
            mailboxes: mailboxes.iter().map(|m| m.to_string()).collect(),
            expected_version: "2.1.0".into(),
            timeout_seconds: 5,
            max_output_bytes: 100_000,
        })
        .unwrap()
    }

    fn reads(&self) -> usize {
        fs::read_to_string(self.dir.path().join("calls.log"))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains("message read"))
            .count()
    }
}

fn folders(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| n.to_string()).collect()
}

const TOML: &str = r#"
[mailbox.alias]
news = "Lists/News"
sent = "Sent Items"

[accounts.work]
imap.server = "imaps://x.test"

[accounts.work.mailbox.alias]
SENT = "Sent"
"#;

#[test]
fn global_aliases_count_and_the_account_overrides_them() {
    for version in ["2.1.0", "2.2.1"] {
        let f = Fixture::new(version, TOML);
        let h = f.engine(&["INBOX"]);
        let conflicts = h
            .alias_conflicts(&folders(&["INBOX", "News", "Sent", "Archive"]))
            .unwrap();
        // `news` is global; `sent` is overridden by the account's `SENT`,
        // which leads to `Sent` itself.
        assert_eq!(conflicts, folders(&["News"]), "{version}");
    }
}

#[test]
fn the_inbox_role_of_2_2_1_is_not_a_conflict() {
    let f = Fixture::new(
        "2.2.1",
        "[accounts.work]\nimap.server = \"imaps://x.test\"\n",
    );
    let h = f.engine(&["INBOX"]);
    assert!(h
        .alias_conflicts(&folders(&["INBOX", "Inbox", "inbox"]))
        .unwrap()
        .is_empty());
}

#[test]
fn a_folder_that_leads_elsewhere_is_never_read() {
    let f = Fixture::new(
        "2.2.1",
        "[accounts.work]\nimap.server = \"imaps://x.test\"\n[accounts.work.mailbox.alias]\ninbox = \"Archive\"\n",
    );
    let h = f.engine(&["INBOX"]);
    let error = h.fetch("INBOX", 42).unwrap_err();
    assert!(
        error.to_string().starts_with("alias_conflict:INBOX"),
        "{error}"
    );
    assert_eq!(f.reads(), 0, "message read must not run");
}

#[test]
fn a_configuration_himalaya_cannot_parse_reads_nothing() {
    let f = Fixture::new(
        "2.1.0",
        "[accounts.work]\nimap.server = \"imaps://x.test\"\n[accounts.work.mailbox]\nalias = 3\n",
    );
    let h = f.engine(&["INBOX"]);
    assert!(h.alias_conflicts(&folders(&["INBOX"])).is_err());
    assert!(h.fetch("INBOX", 42).is_err());
    assert_eq!(f.reads(), 0);
    // A readable configuration reads as before.
    fs::write(
        f.dir.path().join("h.toml"),
        "[accounts.work]\nimap.server = \"imaps://x.test\"\n",
    )
    .unwrap();
    let h = f.engine(&["INBOX"]);
    assert_eq!(h.fetch("INBOX", 42).unwrap(), b"Subject: hi\r\n\r\nbody");
    assert_eq!(f.reads(), 1);
}
```

Create `tests/read_guard.rs`:

```rust
//! The read check in every filing mode: a folder whose effective target is
//! another mailbox is never read, and a mail engine configuration that
//! cannot be read or parsed reads nothing.
mod common;
use common::{mail, Harness};
use mailtriage::domain::FilingMode::{DryRun, Live, Off};

fn fetched_from(h: &Harness, folder: &str) -> bool {
    h.fake
        .calls()
        .iter()
        .any(|c| c.starts_with(&format!("fetch {folder}")))
}

#[test]
fn with_filing_off_a_source_that_leads_elsewhere_is_not_read() {
    let h = Harness::new(Off);
    h.fake.deliver("INBOX", &mail("a", "Hello", "body"));
    h.fake.set_alias_conflicts(&["INBOX"]);
    let blocked = h.sync();
    assert_eq!(blocked["discovered"], 1);
    assert_eq!(blocked["fetched"], 0);
    assert_eq!(blocked["pending"], 1, "the message stays queued");
    assert!(!fetched_from(&h, "INBOX"));
    assert!(h.fake.calls().iter().any(|c| c == "alias_conflicts INBOX"));
    h.fake.set_alias_conflicts(&[]);
    let freed = h.sync();
    assert_eq!(freed["fetched"], 1, "no attempt was burned");
    assert!(fetched_from(&h, "INBOX"));
}

#[test]
fn an_unreadable_engine_configuration_reads_nothing_in_any_mode() {
    for mode in [Off, DryRun, Live] {
        let h = Harness::new(mode);
        h.fake.deliver("INBOX", &mail("a", "Hello", "body"));
        h.fake.fail_alias_check(true);
        let out = h.sync();
        assert_eq!(out["fetched"], 0, "{mode:?}: {out}");
        assert_eq!(out["pending"], 1, "{mode:?}");
        assert!(!fetched_from(&h, "INBOX"), "{mode:?}");
        h.fake.fail_alias_check(false);
        assert_eq!(h.sync()["fetched"], 1, "{mode:?}");
    }
}

#[test]
fn a_category_folder_that_leads_elsewhere_is_not_read() {
    let h = Harness::new(Live);
    h.sync();
    // Mail that arrived in a category folder is fetched from there.
    h.fake
        .deliver("Newsletters", &mail("n", "Weekly newsletter", "news"));
    h.fake.set_alias_conflicts(&["Newsletters"]);
    let blocked = h.sync();
    assert!(!fetched_from(&h, "Newsletters"), "{blocked}");
    h.fake.set_alias_conflicts(&[]);
    h.sync();
    assert!(fetched_from(&h, "Newsletters"));
}

#[test]
fn doctor_reports_source_conflicts_and_an_unreadable_configuration() {
    for mode in [Off, Live] {
        let h = Harness::new(mode);
        h.fake.set_alias_conflicts(&["INBOX"]);
        let report = h.service().doctor("work").unwrap();
        assert_eq!(
            report["transport"]["alias_conflicts"],
            serde_json::json!(["INBOX"])
        );
        assert_eq!(report["transport"]["ready"], true, "{mode:?}");
        h.fake.set_alias_conflicts(&[]);
        h.fake.fail_alias_check(true);
        let report = h.service().doctor("work").unwrap();
        let t = &report["transport"];
        assert_eq!(t["ready"], false, "{mode:?}: {t}");
        assert_eq!(
            t["error"],
            "cannot read the Himalaya configuration: it is not valid TOML"
        );
        assert_eq!(report["ready"], false);
    }
}
```

In `tests/adapters.rs`, replace:

```rust
fn himalaya_rejects_epoch_reset_timeout_and_large_output() {
    let temp = TempDir::new().unwrap();
    let counter = temp.path().join("counter");
    let script = format!("#!/bin/sh\ncase \"$*\" in\n *'imap status INBOX'*) n=$(cat '{}'); n=$((n+1)); printf '%s' \"$n\" > '{}'; if [ \"$n\" -eq 1 ]; then printf '{{\"uid_validity\":1,\"uid_next\":3}}'; else printf '{{\"uid_validity\":2,\"uid_next\":3}}'; fi ;;\n *'message read --mailbox INBOX --raw 1'*) printf 'abc' ;;\n *) exit 7 ;;\nesac\n", counter.display(), counter.display());
    fs::write(&counter, "0").unwrap();
    let (_fixture, adapter) = fake_himalaya(&script);
    assert!(adapter
```

with:

```rust
fn himalaya_rejects_epoch_reset_timeout_and_large_output() {
    let temp = TempDir::new().unwrap();
    let counter = temp.path().join("counter");
    let script = format!("#!/bin/sh\ncase \"$*\" in\n *--version*) printf 'himalaya v2.1.0 +imap\\n' ;;\n *'imap status INBOX'*) n=$(cat '{}'); n=$((n+1)); printf '%s' \"$n\" > '{}'; if [ \"$n\" -eq 1 ]; then printf '{{\"uid_validity\":1,\"uid_next\":3}}'; else printf '{{\"uid_validity\":2,\"uid_next\":3}}'; fi ;;\n *'message read --mailbox INBOX --raw 1'*) printf 'abc' ;;\n *) exit 7 ;;\nesac\n", counter.display(), counter.display());
    fs::write(&counter, "0").unwrap();
    let (_fixture, adapter) = fake_himalaya(&script);
    assert!(adapter
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test --locked --test effective_target --test read_guard`
Expected: FAIL to compile: E0599, no method named `fail_alias_check` for `FakeEngine` (in `read_guard`).

- [ ] **Step 3: Implement**

Create `src/engine/targets.rs`:

```rust
//! The mailbox Himalaya opens for `--mailbox NAME` (only `message read`
//! gets one from mailtriage), following each tested version's resolver as
//! read from Himalaya's source (`Account::resolve_mailbox`, `MailboxArg`):
//!
//! 1. the merged alias map: the global `mailbox.alias` table, overridden key
//!    by key by the account's `accounts.<name>.mailbox.alias`; keys are
//!    lowercased (`str::to_lowercase`) when loaded and looked up lowercased,
//!    `aliases` is accepted as another spelling of `alias`;
//! 2. then the version's `roles` table (2.2 and later), by the lowercased name;
//! 3. else the literal name.
//!
//! A folder whose target is not the folder itself, with `INBOX` compared
//! case-insensitively, would read another mailbox: an alias conflict.
use std::collections::BTreeMap;

/// Where `--mailbox NAME` leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Mailbox(String),
    /// Two alias keys in one table differ only in case and name different
    /// mailboxes; which one Himalaya uses is not defined.
    Ambiguous,
}

/// One account's resolver for one Himalaya version.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolver {
    /// Lowercased key to target; `None` for an ambiguous key.
    aliases: BTreeMap<String, Option<String>>,
    /// Lowercase role name to mailbox.
    roles: BTreeMap<String, String>,
}

impl Resolver {
    /// The resolver of `account` in the Himalaya TOML `text`, with a
    /// version's `roles`. `Err` (why) when the TOML cannot be parsed or an
    /// alias table is malformed: Himalaya would refuse the file too, and
    /// mailtriage then reads no folder.
    pub fn from_toml(
        text: &str,
        account: &str,
        roles: &BTreeMap<String, String>,
    ) -> Result<Self, String> {
        let parsed: toml::Value =
            toml::from_str(text).map_err(|_| "it is not valid TOML".to_owned())?;
        let mut aliases = alias_table(parsed.get("mailbox"), "mailbox")?;
        let own = parsed
            .get("accounts")
            .and_then(|accounts| accounts.get(account))
            .and_then(|account| account.get("mailbox"));
        let own = alias_table(own, &format!("accounts.{account}.mailbox"))?;
        aliases.extend(own);
        Ok(Self {
            aliases,
            roles: roles.clone(),
        })
    }

    /// The mailbox `--mailbox folder` opens.
    pub fn target(&self, folder: &str) -> Target {
        let key = folder.to_lowercase();
        match self.aliases.get(&key) {
            Some(Some(native)) => Target::Mailbox(native.clone()),
            Some(None) => Target::Ambiguous,
            None => Target::Mailbox(
                self.roles
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| folder.to_owned()),
            ),
        }
    }

    /// The folders, in order, whose target is not the folder itself.
    pub fn conflicts(&self, folders: &[String]) -> Vec<String> {
        folders
            .iter()
            .filter(|folder| match self.target(folder) {
                Target::Mailbox(native) => !same_mailbox(&native, folder),
                Target::Ambiguous => true,
            })
            .cloned()
            .collect()
    }
}

/// The aliases of one `mailbox` table, keys lowercased. Both spellings at
/// once, a non-table or a non-string target is malformed.
fn alias_table(
    mailbox: Option<&toml::Value>,
    at: &str,
) -> Result<BTreeMap<String, Option<String>>, String> {
    let Some(mailbox) = mailbox else {
        return Ok(BTreeMap::new());
    };
    let mailbox = mailbox
        .as_table()
        .ok_or_else(|| format!("{at} is not a table"))?;
    let table = match (mailbox.get("alias"), mailbox.get("aliases")) {
        (Some(_), Some(_)) => {
            return Err(format!("{at} has both alias and aliases"));
        }
        (Some(table), None) | (None, Some(table)) => table,
        (None, None) => return Ok(BTreeMap::new()),
    };
    let table = table
        .as_table()
        .ok_or_else(|| format!("{at}.alias is not a table"))?;
    let mut aliases: BTreeMap<String, Option<String>> = BTreeMap::new();
    for (key, native) in table {
        let native = native
            .as_str()
            .ok_or_else(|| format!("{at}.alias.{key} is not a string"))?;
        let key = key.to_lowercase();
        match aliases.get(&key) {
            None => {
                aliases.insert(key, Some(native.to_owned()));
            }
            Some(Some(seen)) if seen == native => {}
            Some(_) => {
                aliases.insert(key, None);
            }
        }
    }
    Ok(aliases)
}

/// Whether two native names select the same mailbox: equal, or both INBOX,
/// whose name IMAP treats case-insensitively (RFC 3501 5.1).
pub fn same_mailbox(a: &str, b: &str) -> bool {
    a == b || (a.eq_ignore_ascii_case("INBOX") && b.eq_ignore_ascii_case("INBOX"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v2_2_1() -> BTreeMap<String, String> {
        BTreeMap::from([("inbox".to_owned(), "INBOX".to_owned())])
    }

    fn folders(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn the_account_overrides_global_aliases_key_by_key() {
        let toml = r#"
[mailbox.alias]
news = "Lists/News"
receipts = "Bills"

[accounts.work]
imap.server = "imaps://x.test"

[accounts.work.mailbox.alias]
News = "News"
"#;
        let r = Resolver::from_toml(toml, "work", &BTreeMap::new()).unwrap();
        // The account's `News` (lowercased) replaces the global `news`.
        assert_eq!(r.target("news"), Target::Mailbox("News".into()));
        assert_eq!(r.target("NEWS"), Target::Mailbox("News".into()));
        // The global alias applies where the account has none.
        assert_eq!(r.target("Receipts"), Target::Mailbox("Bills".into()));
        assert_eq!(r.target("Archive"), Target::Mailbox("Archive".into()));
        assert_eq!(
            r.conflicts(&folders(&["News", "Receipts", "Archive", "news"])),
            folders(&["Receipts", "news"])
        );
        // Another account sees only the global table.
        let other = Resolver::from_toml(toml, "home", &BTreeMap::new()).unwrap();
        assert_eq!(other.conflicts(&folders(&["News"])), folders(&["News"]));
    }

    #[test]
    fn keys_compare_case_insensitively_and_unicode_lowercases() {
        let toml = "[accounts.work.mailbox.alias]\n\"ÄRCHIV\" = \"Elsewhere\"\n";
        let r = Resolver::from_toml(toml, "work", &BTreeMap::new()).unwrap();
        assert_eq!(r.target("ärchiv"), Target::Mailbox("Elsewhere".into()));
        assert_eq!(r.target("Ärchiv"), Target::Mailbox("Elsewhere".into()));
    }

    #[test]
    fn the_2_2_1_inbox_role_leads_to_inbox() {
        let r = Resolver::from_toml("", "work", &v2_2_1()).unwrap();
        assert_eq!(r.target("inbox"), Target::Mailbox("INBOX".into()));
        assert_eq!(r.target("Inbox"), Target::Mailbox("INBOX".into()));
        // INBOX itself, in any case, is not a conflict.
        assert!(r
            .conflicts(&folders(&["INBOX", "Inbox", "inbox", "Sent"]))
            .is_empty());
        // 2.1.0 has no roles: the literal name.
        let old = Resolver::from_toml("", "work", &BTreeMap::new()).unwrap();
        assert_eq!(old.target("Inbox"), Target::Mailbox("Inbox".into()));
        // An alias comes before the role.
        let aliased =
            Resolver::from_toml("[mailbox.alias]\ninbox = \"Mail/In\"\n", "work", &v2_2_1())
                .unwrap();
        assert_eq!(aliased.conflicts(&folders(&["INBOX"])), folders(&["INBOX"]));
    }

    #[test]
    fn aliases_spelled_aliases_count_and_ambiguous_keys_conflict() {
        let r = Resolver::from_toml(
            "[accounts.work.mailbox.aliases]\nsent = \"Sent Items\"\n",
            "work",
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(r.conflicts(&folders(&["Sent"])), folders(&["Sent"]));
        let r = Resolver::from_toml(
            "[accounts.work.mailbox.alias]\nWork = \"A\"\nwork = \"B\"\nSame = \"X\"\nsame = \"X\"\n",
            "work",
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(r.target("work"), Target::Ambiguous);
        assert_eq!(r.conflicts(&folders(&["Work", "X"])), folders(&["Work"]));
        // Equal targets are not ambiguous: `same` leads to X.
        assert_eq!(r.target("SAME"), Target::Mailbox("X".into()));
    }

    #[test]
    fn a_config_himalaya_would_refuse_is_an_error() {
        for (toml, why) in [
            ("[accounts.work\n", "not valid TOML"),
            ("mailbox = 3\n", "mailbox is not a table"),
            (
                "[mailbox]\nalias = {a = \"b\"}\naliases = {c = \"d\"}\n",
                "both alias and aliases",
            ),
            ("[accounts.work.mailbox]\nalias = 7\n", "is not a table"),
            (
                "[accounts.work.mailbox.alias]\nsent = 1\n",
                "accounts.work.mailbox.alias.sent is not a string",
            ),
        ] {
            let error = Resolver::from_toml(toml, "work", &BTreeMap::new()).unwrap_err();
            assert!(error.contains(why), "{toml}: {error}");
        }
    }
}
```

In `src/engine/mod.rs`, replace:

```rust
pub mod fake;
pub mod himalaya;
pub mod raw;
pub mod versions;

use crate::domain::{EngineConfig, MailboxSnapshot, SourceEnvelope};
```

with:

```rust
pub mod fake;
pub mod himalaya;
pub mod raw;
pub mod targets;
pub mod versions;

use crate::domain::{EngineConfig, MailboxSnapshot, SourceEnvelope};
```

In `src/engine/himalaya.rs`, replace:

```rust
//! subscribe, UID MOVE and adding \Flagged, the last two through `imap raw`.
use super::{
    raw,
    versions::{self, Tested},
    ConfigChanged, EngineCapabilities, FolderInfo, MailEngine, WriteOutcome, SPECIAL_USE_ROLES,
};
```

with:

```rust
//! subscribe, UID MOVE and adding \Flagged, the last two through `imap raw`.
use super::{
    raw,
    targets::Resolver,
    versions::{self, Tested},
    ConfigChanged, EngineCapabilities, FolderInfo, MailEngine, WriteOutcome, SPECIAL_USE_ROLES,
};
```

In `src/engine/himalaya.rs`, replace:

```rust
        if uid >= before.uid_next {
            bail!("UID is outside mailbox snapshot");
        }
        let uid_text = uid.to_string();
        // No --json: raw mode writes the exact RFC 5322 bytes. No --seen.
        let raw = self.run(
```

with:

```rust
        if uid >= before.uid_next {
            bail!("UID is outside mailbox snapshot");
        }
        self.check_read(mailbox)?;
        let uid_text = uid.to_string();
        // No --json: raw mode writes the exact RFC 5322 bytes. No --seen.
        let raw = self.run(
```

In `src/engine/himalaya.rs`, replace:

```rust
        Ok(raw)
    }

    /// Passes for a configured source mailbox or a folder in the watch scope.
    fn check_mailbox(&self, mailbox: &str) -> Result<()> {
        if !self.config.mailboxes.iter().any(|m| m == mailbox)
```

with:

```rust
        Ok(raw)
    }

    /// `message read --mailbox` resolves aliases and roles: refuses a folder
    /// whose effective target is another mailbox, and reads nothing when
    /// the TOML cannot be read or parsed. Every `message read` passes here,
    /// whatever the filing mode.
    fn check_read(&self, mailbox: &str) -> Result<()> {
        if self.resolver()?.conflicts(&[mailbox.to_owned()]).is_empty() {
            return Ok(());
        }
        Err(err(
            3,
            format!("alias_conflict:{mailbox}: Himalaya resolves this folder to another mailbox (an alias or role); mailtriage does not read it"),
        ))
    }

    /// The account's resolver: the TOML read at open (unchanged since) and
    /// the reported version's roles.
    fn resolver(&self) -> Result<Resolver> {
        let toml = String::from_utf8(self.current_config()?).map_err(|_| {
            err(
                2,
                "cannot read the Himalaya configuration: it is not valid TOML",
            )
        })?;
        let roles = &self.tested()?.roles;
        Resolver::from_toml(&toml, &self.config.account, roles)
            .map_err(|why| err(2, format!("cannot read the Himalaya configuration: {why}")))
    }

    /// Passes for a configured source mailbox or a folder in the watch scope.
    fn check_mailbox(&self, mailbox: &str) -> Result<()> {
        if !self.config.mailboxes.iter().any(|m| m == mailbox)
```

In `src/engine/himalaya.rs`, replace:

```rust
        *self.scope.borrow_mut() = folders.iter().cloned().collect();
    }

    fn alias_conflicts(&self, folders: &[String]) -> Result<Vec<String>> {
        let toml = String::from_utf8(self.current_config()?)
            .map_err(|_| err(2, "invalid Himalaya TOML configuration"))?;
        let parsed: toml::Value =
            toml::from_str(&toml).map_err(|_| err(2, "invalid Himalaya TOML configuration"))?;
        let Some(aliases) = parsed
            .get("accounts")
            .and_then(|v| v.get(&self.config.account))
            .and_then(|v| v.get("mailbox"))
            .and_then(|v| v.get("alias"))
        else {
            return Ok(Vec::new());
        };
        let aliases = aliases
            .as_table()
            .ok_or_else(|| err(2, "invalid Himalaya mailbox alias table"))?
            .iter()
            .map(|(key, native)| {
                native
                    .as_str()
                    .map(|native| (key.as_str(), native))
                    .ok_or_else(|| err(2, "invalid Himalaya mailbox alias table"))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(folders
            .iter()
            .filter(|folder| {
                aliases.iter().any(|(key, native)| {
                    key.eq_ignore_ascii_case(folder) && !same_mailbox(native, folder)
                })
            })
            .cloned()
            .collect())
    }
}

/// Whether two native names select the same mailbox: equal, or both INBOX,
/// whose name IMAP treats case-insensitively (RFC 3501 5.1).
fn same_mailbox(a: &str, b: &str) -> bool {
    a == b || (a.eq_ignore_ascii_case("INBOX") && b.eq_ignore_ascii_case("INBOX"))
}

/// Maps one SELECT-plus-command session. `Err` means the SELECT result was not
/// captured (or SELECT succeeded without UIDVALIDITY), so the session's effect
/// is unknown. A captured SELECT failure is `selected: false` even if the
```

with:

```rust
        *self.scope.borrow_mut() = folders.iter().cloned().collect();
    }

    /// The folders whose effective target (merged aliases, then the
    /// version's roles, then the name) is another mailbox. `Err` when the
    /// TOML cannot be read or parsed.
    fn alias_conflicts(&self, folders: &[String]) -> Result<Vec<String>> {
        Ok(self.resolver()?.conflicts(folders))
    }
}

/// Maps one SELECT-plus-command session. `Err` means the SELECT result was not
/// captured (or SELECT succeeded without UIDVALIDITY), so the session's effect
/// is unknown. A captured SELECT failure is `selected: false` even if the
```

In `src/filing/observe.rs`, replace:

```rust
    /// Folders a client-side alias resolves elsewhere: neither discovered nor
    /// fetched from while the conflict lasts.
    pub alias_conflicts: BTreeSet<String>,
}

impl FolderMap {
```

with:

```rust
    /// Folders a client-side alias resolves elsewhere: neither discovered nor
    /// fetched from while the conflict lasts.
    pub alias_conflicts: BTreeSet<String>,
    /// `alias_conflicts` was established in this pass.
    pub alias_checked: bool,
    /// The mail engine's configuration cannot be read or parsed: no folder
    /// is read (fetched) in this pass.
    pub reads_blocked: bool,
}

impl FolderMap {
```

In `src/filing/observe.rs`, replace:

```rust
        map.writes_allowed = false;
    }
    map.alias_conflicts = conflicts.clone();
    let folders = ctx.engine.list_folders()?;
    map.listed = folders.iter().map(|f| f.name.clone()).collect();
    map.caps = Some(caps);
```

with:

```rust
        map.writes_allowed = false;
    }
    map.alias_conflicts = conflicts.clone();
    map.alias_checked = true;
    let folders = ctx.engine.list_folders()?;
    map.listed = folders.iter().map(|f| f.name.clone()).collect();
    map.caps = Some(caps);
```

In `src/filing/store.rs`, replace:

```rust
        Ok(rows)
    }

    /// Folders named by any rescan-set row.
    pub fn rescan_folders(&self, account: &str) -> Result<BTreeSet<String>> {
        let mut st = self
```

with:

```rust
        Ok(rows)
    }

    /// `queued`, but only messages that need no fetch: while every read is
    /// blocked, the others stay queued without a lease or attempt.
    pub fn queued_fetched(&self, account: &str, limit: usize) -> Result<Vec<String>> {
        let mut st = self.db.prepare("SELECT j.message_id FROM jobs j JOIN messages m ON m.id=j.message_id WHERE m.account=?1
 AND ((j.state IN ('queued','retry') AND j.next_after<=?2) OR (j.state='leased' AND j.lease_until<=?2))
 AND m.normalized IS NOT NULL
 ORDER BY m.observed_at,j.message_id LIMIT ?3")?;
        let rows = st
            .query_map(params![account, now(), limit as i64], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Folders named by any rescan-set row.
    pub fn rescan_folders(&self, account: &str) -> Result<BTreeSet<String>> {
        let mut st = self
```

In `src/engine/fake.rs`, replace:

```rust
    config_changed: bool,
    /// Folders `alias_conflicts` reports when asked about them.
    alias_conflicts: BTreeSet<String>,
    /// Folders whose next `snapshot` fails, one entry per failure.
    snapshot_faults: Vec<String>,
}
```

with:

```rust
    config_changed: bool,
    /// Folders `alias_conflicts` reports when asked about them.
    alias_conflicts: BTreeSet<String>,
    /// `alias_conflicts` fails, as for a configuration it cannot parse.
    alias_check_fails: bool,
    /// Folders whose next `snapshot` fails, one entry per failure.
    snapshot_faults: Vec<String>,
}
```

In `src/engine/fake.rs`, replace:

```rust
                config_changed_from: None,
                config_changed: false,
                alias_conflicts: BTreeSet::new(),
                snapshot_faults: Vec::new(),
            })),
        }
```

with:

```rust
                config_changed_from: None,
                config_changed: false,
                alias_conflicts: BTreeSet::new(),
                alias_check_fails: false,
                snapshot_faults: Vec::new(),
            })),
        }
```

In `src/engine/fake.rs`, replace:

```rust
        self.state().alias_conflicts = folders.iter().map(|f| f.to_string()).collect();
    }

    /// Strict scope (mirrors the Himalaya engine): when set, every
    /// folder-specific trait call on a folder outside `sources` ∪ the last
    /// `set_watch_scope` list returns Err without effect.
```

with:

```rust
        self.state().alias_conflicts = folders.iter().map(|f| f.to_string()).collect();
    }

    /// Makes `alias_conflicts` fail (or succeed again), as the Himalaya
    /// engine does for a configuration it cannot read or parse.
    pub fn fail_alias_check(&self, fails: bool) {
        self.state().alias_check_fails = fails;
    }

    /// Strict scope (mirrors the Himalaya engine): when set, every
    /// folder-specific trait call on a folder outside `sources` ∪ the last
    /// `set_watch_scope` list returns Err without effect.
```

In `src/engine/fake.rs`, replace:

```rust
    fn alias_conflicts(&self, folders: &[String]) -> Result<Vec<String>> {
        let mut s = self.state();
        s.enter(None, format!("alias_conflicts {}", folders.join(",")), &[])?;
        Ok(folders
            .iter()
            .filter(|f| s.alias_conflicts.contains(*f))
```

with:

```rust
    fn alias_conflicts(&self, folders: &[String]) -> Result<Vec<String>> {
        let mut s = self.state();
        s.enter(None, format!("alias_conflicts {}", folders.join(",")), &[])?;
        if s.alias_check_fails {
            bail!("cannot read the Himalaya configuration: it is not valid TOML");
        }
        Ok(folders
            .iter()
            .filter(|f| s.alias_conflicts.contains(*f))
```

In `src/service.rs`, replace:

```rust
            )
        };
        let key_present = key_error.is_none();
        let failed = || json!({"configured":true,"ready":false,"error":"Himalaya version/config check failed"});
        let transport = match self.engine(&account).map(|e| e.map(|e| e.version())) {
            Ok(None) => json!({"configured":false,"ready":true}),
            Ok(Some(Ok(v))) => json!({"configured":true,"ready":true,"version":v,"tested":true}),
            Ok(Some(Err(e))) => match e.downcast_ref::<engine::versions::Untested>() {
                Some(untested) => {
                    json!({"configured":true,"ready":false,"version":untested.line,"tested":false,"error":untested.to_string()})
                }
                None => failed(),
            },
            Err(_) => failed(),
        };
        let mut out = json!({"schema_version":1,"account":name,"ready":provider_valid&&key_present&&transport["ready"]==true,"provider":{"kind":self.config.provider.kind,"model":self.config.provider.model,"configuration_valid":provider_valid,"key_source":key_source,"key_present":key_present},"transport":transport,"review_mode":self.config.policy.review_mode,"state_dir":self.config.state_dir,"live_checks_performed":false,"coverage":self.coverage(name)?});
        if let Some(e) = key_error {
```

with:

```rust
            )
        };
        let key_present = key_error.is_none();
        let transport = match self.engine(&account) {
            Ok(None) => json!({"configured":false,"ready":true}),
            Ok(Some(engine)) => transport_report(engine.as_ref(), &account),
            Err(_) => {
                json!({"configured":true,"ready":false,"error":"Himalaya version/config check failed"})
            }
        };
        let mut out = json!({"schema_version":1,"account":name,"ready":provider_valid&&key_present&&transport["ready"]==true,"provider":{"kind":self.config.provider.kind,"model":self.config.provider.model,"configuration_valid":provider_valid,"key_source":key_source,"key_present":key_present},"transport":transport,"review_mode":self.config.policy.review_mode,"state_dir":self.config.state_dir,"live_checks_performed":false,"coverage":self.coverage(name)?});
        if let Some(e) = key_error {
```

In `src/service.rs`, replace:

```rust
            mode: filing::mode_str(mode).into(),
            ..Default::default()
        };
        // 2. Folder resolution.
        let map = resolve_or_sources(&mut self.store, filing, &account, &mut summary)?;
        // 3–4. Discovery and reconciliation of every watched folder; the
        // messages whose occurrence reconciliation removed are re-evaluated.
        let mut removed = Vec::new();
```

with:

```rust
            mode: filing::mode_str(mode).into(),
            ..Default::default()
        };
        // 2. Folder resolution, and the read check of whatever it did not
        // check (filing off, or a resolution that failed).
        let mut map = resolve_or_sources(&mut self.store, filing, &account, &mut summary)?;
        if let Some(h) = engine.as_deref() {
            guard_reads(h, &mut map, &mut summary)?;
        }
        // 3–4. Discovery and reconciliation of every watched folder; the
        // messages whose occurrence reconciliation removed are re-evaluated.
        let mut removed = Vec::new();
```

In `src/service.rs`, replace:

```rust
            self.recover_intents(ctx, &map, &mut summary)?;
        }
        // 6. Fetch and classify, skipped while the key is unavailable.
        let (done, skipped) = self.fetch_and_classify(
            name,
            &account,
            &generation,
            engine.as_deref(),
            filing.map(|_| &map),
            limit,
        )?;
        // 7–10. Arrivals and re-evaluation, bootstrap and hydration, plan and
        // apply, done inference.
        if let Some(ctx) = filing {
```

with:

```rust
            self.recover_intents(ctx, &map, &mut summary)?;
        }
        // 6. Fetch and classify, skipped while the key is unavailable.
        let from = Fetching {
            engine: engine.as_deref(),
            map: &map,
            filing: filing.is_some(),
        };
        let (done, skipped) = self.fetch_and_classify(name, &account, &generation, &from, limit)?;
        // 7–10. Arrivals and re-evaluation, bootstrap and hydration, plan and
        // apply, done inference.
        if let Some(ctx) = filing {
```

In `src/service.rs`, replace:

```rust
        self.store
            .reconcile_range_ids(scan.name, folder, epoch, cursor, end, &uids, end == through)
    }
    /// Step 5: fetch and classify queued messages. With `map` (filing on), a
    /// message with an occurrence in a source folder gets its placement (mail
    /// seen only in a category folder is placed by arrival resolution), and a
    /// message whose fetch would read through a conflicting alias stays queued.
    /// With jobs to lease and the key unavailable, nothing is leased and the
    /// key error is returned as the reason classification was skipped.
    fn fetch_and_classify(
        &mut self,
        name: &str,
        account: &AccountConfig,
        generation: &str,
        h: Option<&dyn MailEngine>,
        map: Option<&FolderMap>,
        limit: usize,
    ) -> Result<(Processed, Option<String>)> {
        let mut done = Processed::default();
        let no_conflicts = BTreeSet::new();
        let blocked = map.map_or(&no_conflicts, |m| &m.alias_conflicts);
        let ids = self.store.queued_outside(name, limit, blocked)?;
        if !ids.is_empty() {
            if let Some(reason) = self.classification_skipped() {
                return Ok((done, Some(reason)));
```

with:

```rust
        self.store
            .reconcile_range_ids(scan.name, folder, epoch, cursor, end, &uids, end == through)
    }
    /// Step 5: fetch and classify queued messages. A message whose fetch
    /// would read through a conflicting alias stays queued, in every filing
    /// mode, and so does every message that needs a fetch while reads are
    /// blocked. With filing on, a message with an occurrence in a source
    /// folder gets its placement (mail seen only in a category folder is
    /// placed by arrival resolution). With jobs to lease and the key
    /// unavailable, nothing is leased and the key error is returned as the
    /// reason classification was skipped.
    fn fetch_and_classify(
        &mut self,
        name: &str,
        account: &AccountConfig,
        generation: &str,
        from: &Fetching,
        limit: usize,
    ) -> Result<(Processed, Option<String>)> {
        let mut done = Processed::default();
        let (h, map) = (from.engine, from.map);
        let ids = if map.reads_blocked {
            self.store.queued_fetched(name, limit)?
        } else {
            self.store
                .queued_outside(name, limit, &map.alias_conflicts)?
        };
        if !ids.is_empty() {
            if let Some(reason) = self.classification_skipped() {
                return Ok((done, Some(reason)));
```

In `src/service.rs`, replace:

```rust
            if needed_fetch && row.normalized.is_some() {
                done.fetched += 1;
            }
            if let Some(sources) = map.map(|m| m.sources.as_slice()) {
                let occurrences = self.store.occurrences_of(name, &row.id)?;
                if occurrences.iter().any(|(f, _, _)| sources.contains(f)) {
                    self.store.ensure_placement(name, &row.id, sources)?;
```

with:

```rust
            if needed_fetch && row.normalized.is_some() {
                done.fetched += 1;
            }
            if from.filing {
                let sources = map.sources.as_slice();
                let occurrences = self.store.occurrences_of(name, &row.id)?;
                if occurrences.iter().any(|(f, _, _)| sources.contains(f)) {
                    self.store.ensure_placement(name, &row.id, sources)?;
```

In `src/service.rs`, replace:

```rust
    config_err("mail engine configuration changed during operation")
}

/// A Himalaya that is not a tested version fails the pass with exit 3 and
/// says which versions are tested; any other version error is as before.
fn version_error(e: anyhow::Error) -> anyhow::Error {
```

with:

```rust
    config_err("mail engine configuration changed during operation")
}

/// `doctor`'s transport block for a configured engine: the version check
/// (`tested`), then the alias check of the source folders, which every mode
/// reads from. A configuration that cannot be read or parsed makes the
/// transport not ready, since no folder would be read.
fn transport_report(engine: &dyn MailEngine, account: &AccountConfig) -> Value {
    let version = match engine.version() {
        Ok(version) => version,
        Err(e) => {
            return match e.downcast_ref::<engine::versions::Untested>() {
                Some(untested) => {
                    json!({"configured":true,"ready":false,"version":untested.line,"tested":false,"error":untested.to_string()})
                }
                None => {
                    json!({"configured":true,"ready":false,"error":"Himalaya version/config check failed"})
                }
            }
        }
    };
    match engine.alias_conflicts(&observe::sources_of(account)) {
        Ok(conflicts) => {
            json!({"configured":true,"ready":true,"version":version,"tested":true,"alias_conflicts":conflicts})
        }
        Err(e) => {
            let message = e
                .downcast_ref::<ServiceError>()
                .map_or_else(|| e.to_string(), |s| s.message.clone());
            json!({"configured":true,"ready":false,"version":version,"tested":true,"alias_conflicts":[],"error":message})
        }
    }
}

/// A Himalaya that is not a tested version fails the pass with exit 3 and
/// says which versions are tested; any other version error is as before.
fn version_error(e: anyhow::Error) -> anyhow::Error {
```

In `src/service.rs`, replace:

```rust
    Ok(())
}

/// Step 2: the folder map. With filing off, or when resolution fails, only
/// the sources are watched.
fn resolve_or_sources(
```

with:

```rust
    Ok(())
}

/// Where step 6 fetches from: the engine, the pass's folder map (alias
/// conflicts, blocked reads, sources) and whether filing is on.
struct Fetching<'a> {
    engine: Option<&'a dyn MailEngine>,
    map: &'a FolderMap,
    filing: bool,
}

/// The read check for a map that resolution did not check (filing off, or
/// a resolution that failed): a source whose effective target is another
/// mailbox is not read, and a mail engine configuration that cannot be read
/// or parsed blocks every read (fail closed). A configuration change during
/// the check aborts the pass, as elsewhere.
fn guard_reads(
    engine: &dyn MailEngine,
    map: &mut FolderMap,
    summary: &mut FilingSummary,
) -> Result<()> {
    if map.alias_checked {
        return Ok(());
    }
    match engine.alias_conflicts(&map.sources) {
        Ok(conflicts) => {
            for folder in &conflicts {
                summary.problems.push(format!("alias_conflict:{folder}"));
            }
            map.alias_conflicts = conflicts.into_iter().collect();
            map.alias_checked = true;
        }
        Err(e) if filing::is_config_changed(&e) => return Err(config_changed()),
        Err(_) => {
            map.reads_blocked = true;
            summary.problems.push("engine_config_unreadable".into());
        }
    }
    Ok(())
}

/// Step 2: the folder map. With filing off, or when resolution fails, only
/// the sources are watched.
fn resolve_or_sources(
```

- [ ] **Step 4: Run the new tests**

Run: `cargo test --locked --test effective_target --test read_guard --test adapters --test filing_observe --test engine_contract && cargo test --locked --lib engine::targets`
Expected: PASS.

- [ ] **Step 5: Document it**

In `docs/guide.md`, replace:

```markdown
- Do not add a `mailbox.alias` entry to this account that maps a watched folder or category folder name to a different mailbox. With filing on, mailtriage stops scanning such a folder and makes no filing writes until the alias is removed.
```

with:

```markdown
- Do not add a `mailbox.alias` entry, globally or for this account, that maps a watched folder or category folder name to a different mailbox. mailtriage never reads mail from such a folder, whatever the filing mode; its mail waits until the alias is removed. With filing on, it also stops scanning the folder and makes no filing writes. See [Folder names Himalaya resolves](#folder-names-himalaya-resolves).
```

In `docs/guide.md`, replace:

```markdown
| `transport.tested` | `true` | The version is one mailtriage is tested with. `false` makes the transport not ready, with `transport.error` `Himalaya X is not a tested version (tested: 2.1.0, 2.2.1)`. |
```

with:

```markdown
| `transport.tested` | `true` | The version is one mailtriage is tested with. `false` makes the transport not ready, with `transport.error` `Himalaya X is not a tested version (tested: 2.1.0, 2.2.1)`. |
| `transport.alias_conflicts` | `[]` | Watched folders that Himalaya would resolve to another mailbox; mailtriage reads no mail from them. When the Himalaya configuration cannot be parsed, `transport.ready` is `false` and `transport.error` says why: no folder is read. |
```

In `docs/guide.md`, replace:

```markdown
- `doctor` reports `transport.tested`. An untested Himalaya makes the transport not ready, with `"error": "Himalaya 2.2.2 is not a tested version (tested: 2.1.0, 2.2.1)"`. `sync` and `watch` passes exit 3 with that message, and setup refuses it in step 2.

## Daily use
```

with:

```markdown
- `doctor` reports `transport.tested`. An untested Himalaya makes the transport not ready, with `"error": "Himalaya 2.2.2 is not a tested version (tested: 2.1.0, 2.2.1)"`. `sync` and `watch` passes exit 3 with that message, and setup refuses it in step 2.

### Folder names Himalaya resolves

mailtriage gives `--mailbox` only to `message read`, which fetches a message's text, and Himalaya resolves that name before it opens a mailbox:

1. through the merged alias map: the global `mailbox.alias` table, overridden key by key by the account's `accounts.NAME.mailbox.alias`. Keys compare case-insensitively, and `mailbox.aliases` is the same table;
2. then, from 2.2 on, through the version's mailbox roles: 2.2.1 maps only `inbox` to `INBOX` for IMAP;
3. else the name itself.

A watched folder or category folder whose result is another mailbox (with `INBOX` compared case-insensitively) is an alias conflict. mailtriage never reads mail from it, in every filing mode; its mail stays queued without using a retry attempt. With filing on, the pass also reports `alias_conflict:FOLDER`, stops scanning the folder and makes no filing writes. Two alias keys that differ only in case and name different mailboxes count as a conflict too. When the Himalaya configuration cannot be parsed, mailtriage reads no folder at all, and `doctor` reports why in `transport.error`.

## Daily use
```

In `docs/service-api.md`, replace:

```markdown
## Classification without a key
```

with:

```markdown
`transport` carries `configured` and `ready`, and for a configured engine
`version` (the first line of `himalaya --version`), `tested` (whether that is a
tested version) and `alias_conflicts` (the source folders Himalaya resolves to
another mailbox, which no pass reads from). When it is not ready, `error` is
`Himalaya X is not a tested version (tested: …)`, `cannot read the Himalaya
configuration: …` (no folder is read), or `Himalaya version/config check failed`.

## Classification without a key
```

- [ ] **Step 6: Full check and commit**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`
Expected: all pass.

```bash
git add docs/guide.md \
  docs/service-api.md \
  src/engine/fake.rs \
  src/engine/himalaya.rs \
  src/engine/mod.rs \
  src/engine/targets.rs \
  src/filing/observe.rs \
  src/filing/store.rs \
  src/service.rs \
  tests/adapters.rs \
  tests/effective_target.rs \
  tests/read_guard.rs
git commit -m "Resolve each read folder as Himalaya does and never read through a conflict

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: `mailtriage himalaya install` and the protected path rule

The protected path rule (`distribution::protected`) and `mailtriage himalaya install`: the newest tested version (or `--version`) for this platform, downloaded with the updater's client and URL rules, checked against the compiled digest, unpacked with the bounded reader (with limits that fit pimalaya's archives), staged exclusively, probed, revalidated and renamed into `<data>/mailtriage/himalaya/<version>/himalaya`. The updater gains small shared pieces: `Endpoint::himalaya_asset_url`, `archive::HIMALAYA`, public `install::Scratch` and `install::write_staged`, and `platform::run_version`.

**Files:**
- Create: `src/distribution/protected.rs`, `src/distribution/himalaya.rs`, `src/distribution/mod.rs`
- Modify: `src/lib.rs`, `src/service.rs`, `src/update/archive.rs`, `src/update/github.rs`, `src/update/install.rs`, `src/update/platform.rs`, `src/cli.rs`, `docs/guide.md`
- Test: `tests/himalaya_install.rs` (new)

**Interfaces:**
- Consumes: `engine::versions::{find, newest, listed, check_version_output, Tested::sha256}` (Task 1); `update::github::{Net, Endpoint}`, `update::install::{lock, update_lock_wait, remove_leftovers, TEMP_PREFIX}`, `update::release::platform()`, `update::platform::FileIdentity` (on `main`).
- Produces:
  - `distribution::protected::{Why { Symlink, NotDirectory, Owner(u32), NotYours(u32), Writable(u32) }, Unsafe { dir: PathBuf, why: Why }` (`Display`, `std::error::Error`, `fn fix(&self) -> String`), `Meta`, `check(&Path) -> anyhow::Result<()>`, `check_with(&Path, uid: u32, stat: impl Fn(&Path) -> io::Result<Meta>, creating: bool) -> anyhow::Result<()>`, `prepare(&Path) -> anyhow::Result<PathBuf>`, `prepare_below(base: &Path, tail: &[&str]) -> anyhow::Result<PathBuf>`, `unsafe_dir(&anyhow::Error) -> Option<&Unsafe>}`. `NotYours` is used by Task 8.
  - `distribution::himalaya::{data_dir(Option<&OsStr>, Option<&Path>) -> Option<PathBuf>, default_data_dir() -> Option<PathBuf>, binary_path(&Path, &str) -> PathBuf, platform_of(Option<&str>) -> Option<&'static str>, Installed { action: &'static str, version: String, path: PathBuf }` (`fn json(&self) -> Value`), `run(Option<&str>) -> anyhow::Result<Value>`, `install(Option<&str>, &Net, &Path) -> anyhow::Result<Installed>}`.
  - `service::ErrorKind::UnsafePermissions` (reason `unsafe_permissions`).
  - `update::archive::HIMALAYA: Limits`; `update::github::{DOWNLOAD_BASE, HIMALAYA_REPO}`, `Endpoint::himalaya_asset_url(&self, version: &str, asset: &str) -> Url`; `update::install::{Scratch` (`Default`, `add(PathBuf) -> PathBuf`, `keep()`), `write_staged(&Path, &[u8]) -> io::Result<()>}`; `update::platform::run_version(&Path) -> io::Result<process::Captured>`.
  - CLI: `mailtriage himalaya install [--version X.Y.Z] [--json]`.

- [ ] **Step 1: Write the failing tests**

Create `tests/himalaya_install.rs`:

```rust
#![cfg(unix)]
//! `mailtriage himalaya install` against the update tests' loopback server
//! and a test-only table of digests (debug builds read it from
//! `MAILTRIAGE_TEST_HIMALAYA_VERSIONS`). HOME and XDG_DATA_HOME are
//! temporary; nothing outside them is written.
mod update_support;
use mailtriage::{distribution::himalaya::platform_of, update::release};
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
};
use update_support::{archive, run, sha256_hex, Server};

/// pimalaya's name for this host's platform.
fn platform() -> &'static str {
    platform_of(release::platform()).expect("a platform pimalaya releases for")
}

fn asset_path(version: &str) -> String {
    format!(
        "/pimalaya/himalaya/releases/download/v{version}/himalaya.{}.tgz",
        platform()
    )
}

/// A Himalaya release archive whose `himalaya` prints `version_line`.
fn himalaya_archive(version_line: &str) -> Vec<u8> {
    let script = format!("#!/bin/sh\nprintf '%s\\n' '{version_line}'\n");
    archive(&[
        ("himalaya", script.as_bytes()),
        ("share/man/himalaya.1.gz", b"man"),
        ("share/completions/himalaya.bash", b"complete"),
    ])
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    server: Server,
}

impl Fixture {
    /// Serves `archive` as 2.2.1 for this platform, with `digest` in the
    /// test table (2.1.0 is listed with digests nothing matches).
    fn new(archive: &[u8], digest: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let server = Server::start();
        server.reply(&asset_path("2.2.1"), update_support::Reply::ok(archive));
        let other = "0".repeat(64);
        let assets = |digest: &str| {
            let mut assets = serde_json::Map::new();
            for p in ["aarch64-darwin", "x86_64-linux", "aarch64-linux"] {
                let d = if p == platform() {
                    digest
                } else {
                    other.as_str()
                };
                assets.insert(p.into(), json!(format!("sha256:{d}")));
            }
            Value::Object(assets)
        };
        let table = json!({"versions": [
            {"version": "2.1.0", "roles": {}, "assets": assets(&other)},
            {"version": "2.2.1", "roles": {"inbox": "INBOX"}, "assets": assets(digest)},
        ]});
        fs::write(root.join("versions.json"), table.to_string()).unwrap();
        fs::create_dir_all(root.join("home")).unwrap();
        Self {
            _dir: dir,
            root,
            server,
        }
    }

    fn data(&self) -> PathBuf {
        self.root.join("data")
    }

    fn installed(&self) -> PathBuf {
        self.data().join("mailtriage/himalaya/2.2.1/himalaya")
    }

    /// `mailtriage himalaya install --json ARGS` under umask 077.
    fn install(&self, args: &[&str]) -> (Option<i32>, Value, String) {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "umask 077; exec \"$@\"", "sh"])
            .arg(env!("CARGO_BIN_EXE_mailtriage"))
            .args(["himalaya", "install", "--json"])
            .args(args)
            .current_dir(&self.root)
            .env("HOME", self.root.join("home"))
            .env("XDG_DATA_HOME", self.data())
            .env("MAILTRIAGE_UPDATE_URL", &self.server.base)
            .env(
                "MAILTRIAGE_TEST_HIMALAYA_VERSIONS",
                self.root.join("versions.json"),
            )
            .env_remove("MAILTRIAGE_CONFIG");
        run(&mut command)
    }
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

#[test]
fn a_tested_release_is_installed_privately_then_reported_current() {
    let data = himalaya_archive("himalaya v2.2.1 +smtp +imap");
    let f = Fixture::new(&data, &sha256_hex(&data));
    let (code, v, err) = f.install(&[]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(
        v,
        json!({"schema_version": 1, "himalaya": {
            "action": "installed", "version": "2.2.1",
            "path": f.installed().to_str().unwrap(),
        }})
    );
    assert_eq!(mode(&f.installed()), 0o755);
    // The directory chain is 0755 whatever the umask.
    for dir in [
        "",
        "mailtriage",
        "mailtriage/himalaya",
        "mailtriage/himalaya/2.2.1",
    ] {
        assert_eq!(mode(&f.data().join(dir)), 0o755, "{dir}");
    }
    // Only the binary was unpacked.
    let names: Vec<String> = fs::read_dir(f.installed().parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(
        !names.iter().any(|n| n == "share" || n.ends_with(".tmp")),
        "{names:?}"
    );
    let downloads = f.server.count("/pimalaya/");
    let (code, v, _) = f.install(&["--version", "2.2.1"]);
    assert_eq!(code, Some(0));
    assert_eq!(v["himalaya"]["action"], "current");
    assert_eq!(
        f.server.count("/pimalaya/"),
        downloads,
        "nothing downloaded"
    );
}

#[test]
fn a_checksum_mismatch_installs_nothing() {
    let data = himalaya_archive("himalaya v2.2.1 +imap");
    let f = Fixture::new(&data, &"a".repeat(64));
    let (code, v, _) = f.install(&[]);
    assert_eq!(code, Some(3), "{v}");
    assert!(v["error"]["message"]
        .as_str()
        .unwrap()
        .starts_with("checksum mismatch for himalaya."));
    assert!(!f.installed().exists());
}

#[test]
fn an_archive_without_himalaya_or_a_binary_that_does_not_run_installs_nothing() {
    for (data, why) in [
        (
            archive(&[("bin/himalaya", b"#!/bin/sh\n")]),
            "has no top-level himalaya",
        ),
        (
            himalaya_archive("himalaya v2.2.2 +imap"),
            "the downloaded Himalaya does not run here: Himalaya 2.2.2 is not a tested version",
        ),
        (
            himalaya_archive("himalaya v2.2.1 +smtp"),
            "built without IMAP",
        ),
    ] {
        let f = Fixture::new(&data, &sha256_hex(&data));
        let (code, v, _) = f.install(&[]);
        assert_eq!(code, Some(3), "{v}");
        let message = v["error"]["message"].as_str().unwrap();
        assert!(message.contains(why), "{message}");
        let dir = f.installed().parent().unwrap().to_path_buf();
        let left: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n != ".mailtriage-update.lock")
            .collect();
        assert!(left.is_empty(), "{left:?}");
    }
}

#[test]
fn an_unlisted_version_exits_2_before_any_download() {
    let data = himalaya_archive("himalaya v2.2.1 +imap");
    let f = Fixture::new(&data, &sha256_hex(&data));
    let (code, v, _) = f.install(&["--version", "2.2.2"]);
    assert_eq!(code, Some(2));
    assert_eq!(
        v["error"]["message"],
        "Himalaya 2.2.2 is not a tested version (tested: 2.1.0, 2.2.1)"
    );
    assert_eq!(f.server.count("/"), 0);
    assert!(!f.data().exists(), "nothing created");
}

#[test]
fn a_shared_writable_data_directory_is_refused() {
    let data = himalaya_archive("himalaya v2.2.1 +imap");
    let f = Fixture::new(&data, &sha256_hex(&data));
    fs::create_dir_all(f.data().join("mailtriage")).unwrap();
    fs::set_permissions(
        f.data().join("mailtriage"),
        fs::Permissions::from_mode(0o775),
    )
    .unwrap();
    let (code, v, _) = f.install(&[]);
    assert_eq!(code, Some(3), "{v}");
    assert_eq!(v["error"]["reason"], "unsafe_permissions");
    let message = v["error"]["message"].as_str().unwrap();
    assert!(
        message.contains(&format!(
            "{} is writable by group or others",
            f.data().join("mailtriage").display()
        )),
        "{message}"
    );
    assert!(
        !f.data().join("mailtriage/himalaya").exists(),
        "nothing created inside"
    );
    assert_eq!(f.server.count("/"), 0);
}

#[test]
fn a_symlinked_version_directory_is_refused() {
    let data = himalaya_archive("himalaya v2.2.1 +imap");
    let f = Fixture::new(&data, &sha256_hex(&data));
    let elsewhere = f.root.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::create_dir_all(f.data().join("mailtriage/himalaya")).unwrap();
    symlink(&elsewhere, f.data().join("mailtriage/himalaya/2.2.1")).unwrap();
    let (code, v, _) = f.install(&[]);
    assert_eq!(code, Some(3), "{v}");
    assert_eq!(v["error"]["reason"], "unsafe_permissions");
    assert!(v["error"]["message"]
        .as_str()
        .unwrap()
        .contains("is a symlink"));
    assert_eq!(
        fs::read_dir(&elsewhere).unwrap().count(),
        0,
        "nothing written there"
    );
}

/// pimalaya's archives carry man pages, completions and JSON schemas: 111
/// entries in 2.2.1, far above the 16 a mailtriage release may hold.
#[test]
fn an_archive_shaped_like_pimalayas_installs() {
    let script = "#!/bin/sh\nprintf '%s\\n' 'himalaya v2.2.1 +imap'\n";
    let docs: Vec<(String, Vec<u8>)> = (0..120)
        .map(|i| (format!("share/schemas/himalaya-{i}.json"), b"{}".to_vec()))
        .collect();
    let mut entries: Vec<(&str, &[u8])> = vec![("himalaya", script.as_bytes())];
    entries.extend(docs.iter().map(|(n, d)| (n.as_str(), d.as_slice())));
    let data = archive(&entries);
    let f = Fixture::new(&data, &sha256_hex(&data));
    let (code, v, err) = f.install(&[]);
    assert_eq!(code, Some(0), "{v} {err}");
    assert_eq!(v["himalaya"]["action"], "installed");
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test --locked --test himalaya_install`
Expected: FAIL to compile: E0433, could not find `distribution` in `mailtriage`.

- [ ] **Step 3: Implement**

`protected.rs` and `himalaya.rs` carry their own unit tests (rule tables with fake metadata, real directories under a temporary directory, data directory and platform names). `platform::probe` keeps its behaviour; its `ETXTBSY` retry moves into `run_version`, which `himalaya install` shares.

Create `src/distribution/protected.rs`:

```rust
//! The protected path rule: a directory that holds a program mailtriage
//! installs and runs must not be changeable by anyone but this user or root.
//! It and every ancestor up to `/`, each read without following symlinks,
//! must be no symlink, owned by the user or root, and not writable by group
//! or others. A world-writable directory with the sticky bit (such as
//! `/tmp`) passes when the next directory down the path belongs to the user.
use crate::setup::shell_line;
use anyhow::{Context, Result};
use std::{
    fmt, fs, io,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

/// Why a directory fails the rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    Symlink,
    NotDirectory,
    /// Owned by this other uid.
    Owner(u32),
    /// Owned by this uid (root included) where only the user's own will do:
    /// `self install`'s directory.
    NotYours(u32),
    /// Writable by group or others; its permission bits.
    Writable(u32),
}

/// A directory that fails the rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsafe {
    pub dir: PathBuf,
    pub why: Why,
}

impl fmt::Display for Unsafe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let dir = self.dir.display();
        match self.why {
            Why::Symlink => write!(f, "{dir} is a symlink"),
            Why::NotDirectory => write!(f, "{dir} is not a directory"),
            Why::Owner(uid) => write!(f, "{dir} belongs to uid {uid}, not to you or root"),
            Why::NotYours(uid) => write!(f, "{dir} belongs to uid {uid}, not to you"),
            Why::Writable(mode) => write!(
                f,
                "{dir} is writable by group or others (mode {:o})",
                mode & 0o7777
            ),
        }
    }
}

impl std::error::Error for Unsafe {}

impl Unsafe {
    /// What makes the directory pass.
    pub fn fix(&self) -> String {
        match self.why {
            Why::Writable(_) => format!(
                "run {}",
                shell_line(&[Path::new("chmod"), Path::new("go-w"), &self.dir])
            ),
            Why::Symlink => "use the directory the link points to".to_owned(),
            Why::NotDirectory | Why::Owner(_) | Why::NotYours(_) => {
                "use a directory you own".to_owned()
            }
        }
    }
}

/// What the rule reads of a path, without following a final symlink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Meta {
    pub symlink: bool,
    pub dir: bool,
    pub uid: u32,
    pub mode: u32,
}

fn lstat(path: &Path) -> io::Result<Meta> {
    let meta = fs::symlink_metadata(path)?;
    Ok(Meta {
        symlink: meta.file_type().is_symlink(),
        dir: meta.is_dir(),
        uid: meta.uid(),
        mode: meta.mode(),
    })
}

fn uid() -> u32 {
    crate::system_service::current_uid()
}

/// Checks `path` (absolute and existing) and every ancestor for this user.
pub fn check(path: &Path) -> Result<()> {
    check_with(path, uid(), lstat, false)
}

/// The rule for `uid`, reading each path with `stat`, from `/` down; an
/// error reading one is returned as is. With `creating`, the caller is about
/// to create a directory in `path`, so a sticky, world-writable `path`
/// passes for now: its next directory down will be the user's.
pub fn check_with(
    path: &Path,
    uid: u32,
    stat: impl Fn(&Path) -> io::Result<Meta>,
    creating: bool,
) -> Result<()> {
    let mut chain: Vec<&Path> = path.ancestors().collect();
    chain.reverse();
    for (at, dir) in chain.iter().enumerate() {
        let meta = stat(dir).with_context(|| format!("cannot read {}", dir.display()))?;
        let fail = |why| -> Result<()> {
            Err(Unsafe {
                dir: dir.to_path_buf(),
                why,
            }
            .into())
        };
        if meta.symlink {
            return fail(Why::Symlink);
        }
        if !meta.dir {
            return fail(Why::NotDirectory);
        }
        if meta.uid != uid && meta.uid != 0 {
            return fail(Why::Owner(meta.uid));
        }
        if meta.mode & 0o022 == 0 {
            continue;
        }
        let sticky_world = meta.mode & 0o1002 == 0o1002;
        let child_is_users = match chain.get(at + 1) {
            Some(child) => stat(child).is_ok_and(|c| c.uid == uid && !c.symlink),
            None => creating,
        };
        if !(sticky_world && child_is_users) {
            return fail(Why::Writable(meta.mode & 0o7777));
        }
    }
    Ok(())
}

/// Creates the directory `path` with mode 0755 whatever the umask; an
/// existing entry is left as it is (the rule checks it).
fn make_dir(path: &Path) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) => fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("cannot set the mode of {}", path.display())),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e).with_context(|| format!("cannot create {}", path.display())),
    }
}

/// Makes the absolute directory `dir` exist under the rule and returns its
/// canonical path: its deepest existing ancestor is canonicalized and
/// checked before anything is created in it; the missing directories below
/// it are created one at a time with mode 0755 whatever the umask; then the
/// whole canonical path is checked.
pub fn prepare(dir: &Path) -> Result<PathBuf> {
    prepare_dir(dir, false)
}

/// `prepare`; with `creating`, the caller creates more directories in
/// `dir` and checks the full path afterwards.
fn prepare_dir(dir: &Path, creating: bool) -> Result<PathBuf> {
    let mut existing = dir;
    let mut missing = Vec::new();
    while fs::metadata(existing).is_err() {
        missing.push(
            existing
                .file_name()
                .with_context(|| format!("cannot create {}", dir.display()))?,
        );
        existing = existing
            .parent()
            .with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let mut path = fs::canonicalize(existing)
        .with_context(|| format!("cannot resolve {}", existing.display()))?;
    check_with(&path, uid(), lstat, creating || !missing.is_empty())?;
    for part in missing.iter().rev() {
        path.push(part);
        make_dir(&path)?;
    }
    check_with(&path, uid(), lstat, creating)?;
    Ok(path)
}

/// Makes `base.join(tail…)` exist under the rule and returns it: `base`
/// (absolute) as `prepare` does, then each component of `tail`, read
/// without following a symlink, is checked when it exists, before anything
/// is created in it, and created with mode 0755 when it is missing; then
/// the whole path is checked.
pub fn prepare_below(base: &Path, tail: &[&str]) -> Result<PathBuf> {
    let mut dir = prepare_dir(base, !tail.is_empty())?;
    for part in tail {
        dir.push(part);
        if fs::symlink_metadata(&dir).is_ok() {
            check_with(&dir, uid(), lstat, true)?;
        } else {
            make_dir(&dir)?;
        }
    }
    check(&dir)?;
    Ok(dir)
}

/// The `Unsafe` inside `error`, when the rule refused a directory.
pub fn unsafe_dir(error: &anyhow::Error) -> Option<&Unsafe> {
    error.downcast_ref::<Unsafe>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const ME: u32 = 501;

    fn meta(uid: u32, mode: u32) -> Meta {
        Meta {
            symlink: false,
            dir: true,
            uid,
            mode,
        }
    }

    fn rule(path: &str, table: &[(&str, Meta)]) -> Result<(), Unsafe> {
        let table: HashMap<PathBuf, Meta> =
            table.iter().map(|(p, m)| (PathBuf::from(p), *m)).collect();
        let stat = |p: &Path| {
            table
                .get(p)
                .copied()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        };
        check_with(Path::new(path), ME, stat, false)
            .map_err(|e| e.downcast::<Unsafe>().expect("an Unsafe"))
    }

    fn root() -> (&'static str, Meta) {
        ("/", meta(0, 0o755))
    }

    #[test]
    fn a_private_chain_passes() {
        assert_eq!(
            rule(
                "/home/me/.local/bin",
                &[
                    root(),
                    ("/home", meta(0, 0o755)),
                    ("/home/me", meta(ME, 0o750)),
                    ("/home/me/.local", meta(ME, 0o700)),
                    ("/home/me/.local/bin", meta(ME, 0o755)),
                ]
            ),
            Ok(())
        );
    }

    #[test]
    fn a_group_writable_ancestor_fails_with_its_directory() {
        let error = rule(
            "/opt/shared/bin",
            &[
                root(),
                ("/opt", meta(0, 0o755)),
                ("/opt/shared", meta(ME, 0o775)),
                ("/opt/shared/bin", meta(ME, 0o755)),
            ],
        )
        .unwrap_err();
        assert_eq!(error.dir, PathBuf::from("/opt/shared"));
        assert_eq!(error.why, Why::Writable(0o775));
        assert_eq!(
            error.to_string(),
            "/opt/shared is writable by group or others (mode 775)"
        );
        assert_eq!(error.fix(), "run chmod go-w /opt/shared");
    }

    #[test]
    fn a_symlink_or_a_stranger_in_the_chain_fails() {
        let mut link = meta(ME, 0o777);
        link.symlink = true;
        let error = rule(
            "/home/me/bin",
            &[root(), ("/home", meta(0, 0o755)), ("/home/me", link)],
        )
        .unwrap_err();
        assert_eq!((error.dir, error.why), ("/home/me".into(), Why::Symlink));
        let error = rule(
            "/home/other/bin",
            &[
                root(),
                ("/home", meta(0, 0o755)),
                ("/home/other", meta(1000, 0o755)),
            ],
        )
        .unwrap_err();
        assert_eq!(error.why, Why::Owner(1000));
    }

    #[test]
    fn a_sticky_world_writable_directory_passes_only_above_the_users_own() {
        let tmp = ("/tmp", meta(0, 0o1777));
        assert_eq!(
            rule("/tmp/mine", &[root(), tmp, ("/tmp/mine", meta(ME, 0o755))]),
            Ok(())
        );
        // Someone else's directory below the sticky one.
        let error = rule(
            "/tmp/theirs/x",
            &[
                root(),
                tmp,
                ("/tmp/theirs", meta(0, 0o755)),
                ("/tmp/theirs/x", meta(ME, 0o755)),
            ],
        )
        .unwrap_err();
        assert_eq!(error.dir, PathBuf::from("/tmp"));
        // The sticky directory itself as the target.
        assert_eq!(
            rule("/tmp", &[root(), tmp]).unwrap_err().dir,
            PathBuf::from("/tmp")
        );
        // World-writable without the sticky bit, or sticky but only
        // group-writable.
        let open = ("/tmp", meta(0, 0o777));
        assert!(rule("/tmp/mine", &[root(), open, ("/tmp/mine", meta(ME, 0o755))]).is_err());
        let group = ("/tmp", meta(0, 0o1775));
        assert!(rule(
            "/tmp/mine",
            &[root(), group, ("/tmp/mine", meta(ME, 0o755))]
        )
        .is_err());
    }

    #[test]
    fn real_directories_are_created_0755_and_checked() {
        let tmp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap();
        // A sticky, world-writable directory passes above the user's own.
        let sticky = root.join("sticky");
        fs::create_dir(&sticky).unwrap();
        fs::set_permissions(&sticky, fs::Permissions::from_mode(0o1777)).unwrap();
        let made = prepare(&sticky.join("mine/bin")).unwrap();
        assert_eq!(made, sticky.join("mine/bin"));
        for dir in [sticky.join("mine"), sticky.join("mine/bin")] {
            let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o7777;
            assert_eq!(mode, 0o755, "{}", dir.display());
        }
        let error = check(&sticky).unwrap_err();
        assert_eq!(unsafe_dir(&error).unwrap().dir, sticky);
        // A group-writable ancestor: refused before anything is created in it.
        let group = root.join("group");
        fs::create_dir(&group).unwrap();
        fs::set_permissions(&group, fs::Permissions::from_mode(0o775)).unwrap();
        let error = prepare(&group.join("x")).unwrap_err();
        assert_eq!(unsafe_dir(&error).unwrap().dir, group);
        assert!(!group.join("x").exists());
        // A symlink below the base is refused, not followed.
        fs::create_dir(root.join("real")).unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("link")).unwrap();
        let error = prepare_below(&root, &["link", "x"]).unwrap_err();
        assert_eq!(unsafe_dir(&error).unwrap().why, Why::Symlink);
        assert!(!root.join("real/x").exists());
    }
}
```

Create `src/distribution/himalaya.rs`:

```rust
//! `mailtriage himalaya install`: a tested pimalaya release, installed
//! privately for mailtriage into `<data>/mailtriage/himalaya/<version>/`.
//! It never touches any other `himalaya`.
use super::protected;
use crate::{
    engine::versions::{self, Tested},
    process::Ending,
    service::{err, err_kind, ErrorKind},
    update::{
        archive,
        github::{self, Endpoint, Net},
        install::{self, Scratch},
        platform::{self, FileIdentity},
        release,
    },
};
use anyhow::Result;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

/// `$XDG_DATA_HOME` when it is absolute, else `~/.local/share` (on macOS
/// too); `None` without either.
pub fn data_dir(xdg_data_home: Option<&OsStr>, home: Option<&Path>) -> Option<PathBuf> {
    match xdg_data_home.map(Path::new).filter(|p| p.is_absolute()) {
        Some(xdg) => Some(xdg.to_path_buf()),
        None => home
            .filter(|h| !h.as_os_str().is_empty())
            .map(|h| h.join(".local/share")),
    }
}

/// This user's data directory, from `XDG_DATA_HOME` and `HOME`.
pub fn default_data_dir() -> Option<PathBuf> {
    data_dir(
        std::env::var_os("XDG_DATA_HOME").as_deref(),
        std::env::var_os("HOME").map(PathBuf::from).as_deref(),
    )
}

/// The directories below the data directory that hold one version.
fn tail(version: &str) -> [&str; 3] {
    ["mailtriage", "himalaya", version]
}

/// Where `himalaya install` puts `version`: `<data>/mailtriage/himalaya/<version>/himalaya`.
pub fn binary_path(data: &Path, version: &str) -> PathBuf {
    let mut path = data.to_path_buf();
    path.extend(tail(version));
    path.join("himalaya")
}

/// pimalaya's platform name for mailtriage's release platform.
pub fn platform_of(platform: Option<&str>) -> Option<&'static str> {
    match platform? {
        "macos-arm64" => Some("aarch64-darwin"),
        "linux-amd64" => Some("x86_64-linux"),
        "linux-arm64" => Some("aarch64-linux"),
        _ => None,
    }
}

/// What `install` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// `installed`, or `current` when a matching copy was already there.
    pub action: &'static str,
    pub version: String,
    pub path: PathBuf,
}

impl Installed {
    pub fn json(&self) -> Value {
        json!({"schema_version": 1, "himalaya": {
            "action": self.action,
            "version": self.version,
            "path": self.path,
        }})
    }
}

/// `mailtriage himalaya install [--version X.Y.Z]` with this user's data
/// directory and the release endpoint (the loopback override in tests).
pub fn run(version: Option<&str>) -> Result<Value> {
    let data = default_data_dir().ok_or_else(|| {
        err(
            3,
            "cannot find the data directory: set HOME, or XDG_DATA_HOME to an absolute path",
        )
    })?;
    let net = Net::new(Endpoint::from_env())
        .map_err(|e| err(3, format!("cannot start an HTTPS client: {e}")))?;
    Ok(install(version, &net, &data)?.json())
}

/// Installs `version` (default: the newest tested one) into `data`. Errors
/// carry exit codes: 2 for a version that is not tested or a platform
/// without a release, 3 for an unsafe directory (reason
/// `unsafe_permissions`), the network, the checksum, the archive or a
/// binary that does not run.
pub fn install(version: Option<&str>, net: &Net, data: &Path) -> Result<Installed> {
    let tested = match version {
        Some(v) => versions::find(v).ok_or_else(|| {
            err(
                2,
                format!(
                    "Himalaya {v} is not a tested version (tested: {})",
                    versions::listed()
                ),
            )
        })?,
        None => versions::newest(),
    };
    let platform = platform_of(release::platform()).ok_or_else(|| {
        err(
            2,
            "pimalaya publishes no Himalaya mailtriage can use for this platform; install a tested Himalaya yourself",
        )
    })?;
    let dir = protected::prepare_below(data, &tail(&tested.version)).map_err(unsafe_or_io)?;
    let lock = install::lock(&dir, install::update_lock_wait())
        .map_err(|e| err(3, format!("{e:#}")))?
        .ok_or_else(|| err(3, "another himalaya install is running; try again"))?;
    install::remove_leftovers(&dir);
    let path = dir.join("himalaya");
    let installed = |action| Installed {
        action,
        version: tested.version.clone(),
        path: path.clone(),
    };
    let existing = fs::symlink_metadata(&path).is_ok_and(|m| m.is_file());
    if existing && probe(&path, tested).is_ok() {
        return Ok(installed("current"));
    }
    let asset = format!("himalaya.{platform}.tgz");
    let url = net.endpoint().himalaya_asset_url(&tested.version, &asset);
    let bytes = net
        .download(url.as_str(), 0, github::MAX_ASSET_BYTES)
        .map_err(|e| {
            err(
                3,
                format!("cannot download {asset} of v{}: {e}", tested.version),
            )
        })?;
    let expected = tested
        .sha256(platform)
        .ok_or_else(|| err(3, format!("no digest for {asset}")))?;
    if hex(&Sha256::digest(&bytes)) != expected {
        return Err(err(
            3,
            format!("checksum mismatch for {asset} of v{}", tested.version),
        ));
    }
    let binary = archive::extract(&bytes, "himalaya", &archive::HIMALAYA)
        .map_err(|e| err(3, format!("{asset}: {e}")))?;
    drop(bytes);
    let mut scratch = Scratch::default();
    let staged = scratch.add(dir.join(format!(
        "{}{}.tmp",
        install::TEMP_PREFIX,
        uuid::Uuid::new_v4()
    )));
    install::write_staged(&staged, &binary)
        .map_err(|e| err(3, format!("cannot write {}: {e}", staged.display())))?;
    let identity = FileIdentity::read(&staged)?;
    probe(&staged, tested).map_err(|cause| {
        err(
            3,
            format!("the downloaded Himalaya does not run here: {cause}"),
        )
    })?;
    if FileIdentity::read(&staged).ok() != Some(identity) {
        return Err(err(
            3,
            "the downloaded Himalaya changed before it was installed; try again",
        ));
    }
    fs::rename(&staged, &path)
        .map_err(|e| err(3, format!("cannot install {}: {e}", path.display())))?;
    scratch.keep();
    if let Ok(dir) = fs::File::open(&dir) {
        let _ = dir.sync_all();
    }
    drop(lock);
    Ok(installed("installed"))
}

/// The rule's refusal as exit 3 with reason `unsafe_permissions`; any other
/// problem with the directory as exit 3.
fn unsafe_or_io(error: anyhow::Error) -> anyhow::Error {
    match protected::unsafe_dir(&error) {
        Some(found) => err_kind(
            3,
            ErrorKind::UnsafePermissions,
            format!("unsafe_permissions: {found}; {}", found.fix()),
        ),
        None => err(3, format!("{error:#}")),
    }
}

/// `program --version` names `tested` with IMAP; otherwise why not.
fn probe(program: &Path, tested: &Tested) -> Result<(), String> {
    let out = platform::run_version(program).map_err(|_| "could not start".to_owned())?;
    match out.ending {
        Ending::TimedOut => Err("timed out".into()),
        Ending::Overflowed => Err("printed too much".into()),
        Ending::Exited(status) if !status.success() => {
            Err(format!("exited with {}", status.code().unwrap_or(-1)))
        }
        Ending::Exited(_) => match versions::check_version_output(&out.stdout) {
            Ok((_, found)) if found.version == tested.version => Ok(()),
            Ok((line, _)) => Err(format!("printed {line:?}")),
            Err(untested) => Err(untested.to_string()),
        },
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_data_directory_follows_xdg_on_every_platform() {
        let home = Some(Path::new("/h"));
        assert_eq!(
            data_dir(Some(OsStr::new("/x")), home),
            Some(PathBuf::from("/x"))
        );
        assert_eq!(
            data_dir(Some(OsStr::new("rel")), home),
            Some(PathBuf::from("/h/.local/share"))
        );
        assert_eq!(data_dir(None, home), Some(PathBuf::from("/h/.local/share")));
        assert_eq!(data_dir(None, Some(Path::new(""))), None);
        assert_eq!(data_dir(None, None), None);
        assert_eq!(
            binary_path(Path::new("/x"), "2.2.1"),
            PathBuf::from("/x/mailtriage/himalaya/2.2.1/himalaya")
        );
    }

    #[test]
    fn each_release_platform_has_a_pimalaya_platform() {
        assert_eq!(platform_of(Some("macos-arm64")), Some("aarch64-darwin"));
        assert_eq!(platform_of(Some("linux-amd64")), Some("x86_64-linux"));
        assert_eq!(platform_of(Some("linux-arm64")), Some("aarch64-linux"));
        assert_eq!(platform_of(None), None);
        for platform in ["macos-arm64", "linux-amd64", "linux-arm64"] {
            let name = platform_of(Some(platform)).unwrap();
            assert!(versions::PLATFORMS.contains(&name), "{name}");
        }
    }
}
```

Create `src/distribution/mod.rs`:

```rust
//! Installing mailtriage and what it needs (spec:
//! docs/superpowers/specs/2026-10-07-install-and-distribution-design.md):
//! the protected path rule and a private, tested Himalaya.
pub mod himalaya;
pub mod protected;
```

In `src/lib.rs`, replace:

```rust
pub mod categories;
pub mod config;
pub mod domain;
pub mod engine;
pub mod filing;
```

with:

```rust
pub mod categories;
pub mod config;
pub mod distribution;
pub mod domain;
pub mod engine;
pub mod filing;
```

In `src/service.rs`, replace:

```rust
    ServiceConfigUnknown,
    /// Another service command for the account held the service lock.
    ServiceBusy,
}

impl ErrorKind {
```

with:

```rust
    ServiceConfigUnknown,
    /// Another service command for the account held the service lock.
    ServiceBusy,
    /// A directory mailtriage would install a program into fails the
    /// protected path rule.
    UnsafePermissions,
}

impl ErrorKind {
```

In `src/service.rs`, replace:

```rust
            Self::ServiceConfigMismatch => Some("service_config_mismatch"),
            Self::ServiceConfigUnknown => Some("service_config_unknown"),
            Self::ServiceBusy => Some("service_busy"),
        }
    }
}
```

with:

```rust
            Self::ServiceConfigMismatch => Some("service_config_mismatch"),
            Self::ServiceConfigUnknown => Some("service_config_unknown"),
            Self::ServiceBusy => Some("service_busy"),
            Self::UnsafePermissions => Some("unsafe_permissions"),
        }
    }
}
```

In `src/service.rs`, replace:

```rust
            Some("service_config_unknown")
        );
        assert_eq!(ErrorKind::ServiceBusy.reason(), Some("service_busy"));
    }

    /// A mailbox identity that changed mid-operation is a binding conflict.
```

with:

```rust
            Some("service_config_unknown")
        );
        assert_eq!(ErrorKind::ServiceBusy.reason(), Some("service_busy"));
        assert_eq!(
            ErrorKind::UnsafePermissions.reason(),
            Some("unsafe_permissions")
        );
    }

    /// A mailbox identity that changed mid-operation is a binding conflict.
```

In `src/update/archive.rs`, replace:

```rust
    max_path: 255,
};

/// The contents of the single top-level regular file `name` in the gzip
/// tar `data`. The whole stream is read first: more entries than allowed,
/// more decompressed bytes than allowed, a path that is too long, absolute
```

with:

```rust
    max_path: 255,
};

/// The limits for pimalaya's Himalaya archives, which also carry man pages,
/// shell completions and JSON schemas (about 110 entries in 2.2.1).
pub const HIMALAYA: Limits = Limits {
    max_entries: 1024,
    max_total: 256 * 1024 * 1024,
    max_file: 200 * 1024 * 1024,
    max_path: 255,
};

/// The contents of the single top-level regular file `name` in the gzip
/// tar `data`. The whole stream is read first: more entries than allowed,
/// more decompressed bytes than allowed, a path that is too long, absolute
```

In `src/update/github.rs`, replace:

```rust

/// The API base URL.
pub const API_BASE: &str = "https://api.github.com";
/// The hidden variable that replaces `API_BASE` in tests; honoured only for
/// a loopback host.
pub const URL_OVERRIDE: &str = "MAILTRIAGE_UPDATE_URL";
```

with:

```rust

/// The API base URL.
pub const API_BASE: &str = "https://api.github.com";
/// Where release assets are downloaded from.
pub const DOWNLOAD_BASE: &str = "https://github.com";
/// The repository Himalaya releases come from.
pub const HIMALAYA_REPO: &str = "pimalaya/himalaya";
/// The hidden variable that replaces `API_BASE` in tests; honoured only for
/// a loopback host.
pub const URL_OVERRIDE: &str = "MAILTRIAGE_UPDATE_URL";
```

In `src/update/github.rs`, replace:

```rust
        url
    }

    /// Whether a request, or a redirect, may go to `url`.
    pub fn allows(&self, url: &Url) -> bool {
        github_url(url)
```

with:

```rust
        url
    }

    /// A Himalaya release asset:
    /// `https://github.com/pimalaya/himalaya/releases/download/v<version>/<asset>`,
    /// below the loopback base instead when the override is set.
    pub fn himalaya_asset_url(&self, version: &str, asset: &str) -> Url {
        let mut url = self
            .loopback
            .clone()
            .unwrap_or_else(|| Url::parse(DOWNLOAD_BASE).expect("DOWNLOAD_BASE is a URL"));
        let base = url.path().trim_end_matches('/').to_owned();
        url.set_path(&format!(
            "{base}/{HIMALAYA_REPO}/releases/download/v{version}/{asset}"
        ));
        url.set_query(None);
        url
    }

    /// Whether a request, or a redirect, may go to `url`.
    pub fn allows(&self, url: &Url) -> bool {
        github_url(url)
```

In `src/update/github.rs`, replace:

```rust
        );
    }

    #[test]
    fn the_next_link_is_found_among_others() {
        let header = r#"<https://api.github.com/r?per_page=30&page=1>; rel="prev", <https://api.github.com/r?per_page=30&page=3>; rel="next", <https://api.github.com/r?per_page=30&page=9>; rel="last""#;
```

with:

```rust
        );
    }

    #[test]
    fn himalaya_assets_come_from_pimalayas_releases() {
        assert_eq!(
            Endpoint::github()
                .himalaya_asset_url("2.2.1", "himalaya.x86_64-linux.tgz")
                .as_str(),
            "https://github.com/pimalaya/himalaya/releases/download/v2.2.1/himalaya.x86_64-linux.tgz"
        );
        let local = Endpoint::with_override(Some("http://127.0.0.1:9/base"));
        let url = local.himalaya_asset_url("2.1.0", "himalaya.aarch64-darwin.tgz");
        assert_eq!(
            url.as_str(),
            "http://127.0.0.1:9/base/pimalaya/himalaya/releases/download/v2.1.0/himalaya.aarch64-darwin.tgz"
        );
        assert!(local.allows(&url));
    }

    #[test]
    fn the_next_link_is_found_among_others() {
        let header = r#"<https://api.github.com/r?per_page=30&page=1>; rel="prev", <https://api.github.com/r?per_page=30&page=3>; rel="next", <https://api.github.com/r?per_page=30&page=9>; rel="last""#;
```

In `src/update/install.rs`, replace:

```rust
}

/// This attempt's temporary files, removed unless kept.
struct Scratch(Vec<PathBuf>);

impl Scratch {
    fn add(&mut self, path: PathBuf) -> PathBuf {
        self.0.push(path.clone());
        path
    }
    fn keep(&mut self) {
        self.0.clear();
    }
}
```

with:

```rust
}

/// This attempt's temporary files, removed unless kept.
#[derive(Default)]
pub struct Scratch(Vec<PathBuf>);

impl Scratch {
    /// Removes `path` when this is dropped, unless `keep` was called.
    pub fn add(&mut self, path: PathBuf) -> PathBuf {
        self.0.push(path.clone());
        path
    }
    /// The files are kept: the commit point was reached.
    pub fn keep(&mut self) {
        self.0.clear();
    }
}
```

In `src/update/install.rs`, replace:

```rust

/// Created exclusively with mode 0600 (never following an existing link),
/// fsynced and closed, then made executable.
fn write_staged(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
```

with:

```rust

/// Created exclusively with mode 0600 (never following an existing link),
/// fsynced and closed, then made executable.
pub fn write_staged(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
```

In `src/update/platform.rs`, replace:

```rust
/// not start`, `killed by signal N`, `timed out`, `exited with N`, or
/// `printed "…"`, each with the first line of stderr when there is one.
pub fn probe(program: &Path, component: Component) -> Result<Version, String> {
    let mut attempt = 0;
    let out = loop {
        match process::run_captured(
            program,
            &["--version"],
            PROBE_TIMEOUT,
            PROBE_MAX_STDOUT,
            PROBE_MAX_STDERR,
        ) {
            Ok(out) => break out,
            Err(e) if e.raw_os_error() == Some(TEXT_FILE_BUSY) && attempt < BUSY_RETRIES => {
                attempt += 1;
                std::thread::sleep(BUSY_PAUSE);
            }
            Err(_) => return Err("could not start".to_owned()),
        }
    };
    let cause = match out.ending {
        Ending::TimedOut => "timed out".to_owned(),
        Ending::Overflowed => "printed too much".to_owned(),
```

with:

```rust
/// not start`, `killed by signal N`, `timed out`, `exited with N`, or
/// `printed "…"`, each with the first line of stderr when there is one.
pub fn probe(program: &Path, component: Component) -> Result<Version, String> {
    let out = run_version(program).map_err(|_| "could not start".to_owned())?;
    let cause = match out.ending {
        Ending::TimedOut => "timed out".to_owned(),
        Ending::Overflowed => "printed too much".to_owned(),
```

In `src/update/platform.rs`, replace:

```rust
    }
}

fn shorten(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &text[..end]),
```

with:

```rust
    }
}

/// `program --version` with a 10 s limit and at most 4 KB of stdout and of
/// stderr; a start that meets `ETXTBSY` is retried briefly. `Err` only when
/// the program cannot be started.
pub fn run_version(program: &Path) -> std::io::Result<process::Captured> {
    let mut attempt = 0;
    loop {
        match process::run_captured(
            program,
            &["--version"],
            PROBE_TIMEOUT,
            PROBE_MAX_STDOUT,
            PROBE_MAX_STDERR,
        ) {
            Err(e) if e.raw_os_error() == Some(TEXT_FILE_BUSY) && attempt < BUSY_RETRIES => {
                attempt += 1;
                std::thread::sleep(BUSY_PAUSE);
            }
            result => return result,
        }
    }
}

fn shorten(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &text[..end]),
```

In `src/cli.rs`, replace:

```rust
use clap::{ArgGroup, Args, Parser, Subcommand};
use mailtriage::{
    config,
    domain::{Category, FilingMode, UpdateMode},
    engine::ConfigChanged,
    prompt::{self, Prompter},
```

with:

```rust
use clap::{ArgGroup, Args, Parser, Subcommand};
use mailtriage::{
    config, distribution,
    domain::{Category, FilingMode, UpdateMode},
    engine::ConfigChanged,
    prompt::{self, Prompter},
```

In `src/cli.rs`, replace:

```rust
    },
    /// Install the newest stable release from GitHub; needs no config.
    Update(UpdateArg),
}

#[derive(Args)]
```

with:

```rust
    },
    /// Install the newest stable release from GitHub; needs no config.
    Update(UpdateArg),
    /// The Himalaya mailtriage runs.
    Himalaya {
        #[command(subcommand)]
        command: HimalayaCommand,
    },
}

#[derive(Subcommand)]
enum HimalayaCommand {
    /// Install a tested Himalaya release for mailtriage only, under
    /// ~/.local/share/mailtriage/himalaya; needs no config.
    Install(HimalayaInstallArg),
}

#[derive(Args)]
struct HimalayaInstallArg {
    /// A tested version (default: the newest tested one).
    #[arg(long = "version", value_name = "X.Y.Z")]
    version: Option<String>,
}

#[derive(Args)]
```

In `src/cli.rs`, replace:

```rust
        Command::Update(arg) => {
            update::command::run(arg.check, &update::install::EnvHooks).map_err(service_error)
        }
    }
}

```

with:

```rust
        Command::Update(arg) => {
            update::command::run(arg.check, &update::install::EnvHooks).map_err(service_error)
        }
        Command::Himalaya {
            command: HimalayaCommand::Install(arg),
        } => distribution::himalaya::run(arg.version.as_deref()).map_err(service_error),
    }
}

```

- [ ] **Step 4: Run the new tests**

Run: `cargo test --locked --test himalaya_install && cargo test --locked --lib -- distribution update::github`
Expected: PASS (7 integration tests; the unit tests of `protected`, `himalaya` and `github`).

- [ ] **Step 5: Document it**

In `docs/guide.md`, replace:

```markdown
A watched folder or category folder whose result is another mailbox (with `INBOX` compared case-insensitively) is an alias conflict. mailtriage never reads mail from it, in every filing mode; its mail stays queued without using a retry attempt. With filing on, the pass also reports `alias_conflict:FOLDER`, stops scanning the folder and makes no filing writes. Two alias keys that differ only in case and name different mailboxes count as a conflict too. When the Himalaya configuration cannot be parsed, mailtriage reads no folder at all, and `doctor` reports why in `transport.error`.

## Daily use
```

with:

````markdown
A watched folder or category folder whose result is another mailbox (with `INBOX` compared case-insensitively) is an alias conflict. mailtriage never reads mail from it, in every filing mode; its mail stays queued without using a retry attempt. With filing on, the pass also reports `alias_conflict:FOLDER`, stops scanning the folder and makes no filing writes. Two alias keys that differ only in case and name different mailboxes count as a conflict too. When the Himalaya configuration cannot be parsed, mailtriage reads no folder at all, and `doctor` reports why in `transport.error`.

### A private Himalaya

```sh
mailtriage himalaya install [--version X.Y.Z] [--json]
```

installs a tested Himalaya release for mailtriage alone; it never touches another `himalaya`.

- It installs the newest tested version, or `--version`, which must be tested (else exit 2), into `DATA/mailtriage/himalaya/VERSION/himalaya`. `DATA` is `$XDG_DATA_HOME` when that is an absolute path, else `~/.local/share`, on macOS too.
- It downloads that version's `himalaya.PLATFORM.tgz` from pimalaya's GitHub releases over HTTPS, with the same URL rules as `mailtriage update`, and checks it against the SHA-256 compiled into mailtriage. It unpacks only the `himalaya` executable into a new file, checks that it runs and prints that version with `+imap`, and moves it into place with a rename.
- The directories are created with mode 0755. Before anything is written there, the version's directory and each of its parents must be safe: no symlink, owned by you or root, and not writable by group or others; a world-writable directory with the sticky bit, such as `/tmp`, is fine above one of yours. Otherwise it exits 3 with the reason `unsafe_permissions`, the directory and the fix, such as `chmod go-w DIR`.
- Run again, it reports `current` and downloads nothing.
- Point an account at it with `mailtriage setup --update --himalaya-binary PATH`, using the path it printed.

```json
{"schema_version":1,"himalaya":{"action":"installed","version":"2.2.1","path":"/Users/alice/.local/share/mailtriage/himalaya/2.2.1/himalaya"}}
```

`action` is `installed` or `current`. Exit codes: 0; 2 for a version that is not tested, or a platform for which pimalaya has no build mailtriage can use; 3 for a network error, a checksum mismatch, a bad archive, a binary that does not run, an unsafe directory, or another `himalaya install` that held its directory's lock for 60 seconds.

## Daily use
````

In `docs/guide.md`, replace:

```markdown
Some errors also carry a machine-readable `reason` in the error object, for scripts that react to a class of error rather than to its message: `config_changed` (`mailtriage.json` or the Himalaya configuration changed during the command), `config_busy` (another command is editing `mailtriage.json`), `account_busy` (another worker for the account is running), `binding_conflict` (the account binding changed, see [Account binding](#account-binding)), `categories_changed` (`categories apply --expect-digest` found other categories), `service_config_mismatch` (the account's service runs another config, see [Service commands](#service-commands)), `service_config_unknown` (the config of the account's service cannot be told) and `service_busy` (another service command for the account held the service lock for 30 s). An error without a reason has no `reason` key.
```

with:

```markdown
Some errors also carry a machine-readable `reason` in the error object, for scripts that react to a class of error rather than to its message: `config_changed` (`mailtriage.json` or the Himalaya configuration changed during the command), `config_busy` (another command is editing `mailtriage.json`), `account_busy` (another worker for the account is running), `binding_conflict` (the account binding changed, see [Account binding](#account-binding)), `categories_changed` (`categories apply --expect-digest` found other categories), `service_config_mismatch` (the account's service runs another config, see [Service commands](#service-commands)), `service_config_unknown` (the config of the account's service cannot be told) `service_busy` (another service command for the account held the service lock for 30 s) and `unsafe_permissions` (a directory mailtriage would install a program into is not safe; see [A private Himalaya](#a-private-himalaya)). An error without a reason has no `reason` key.
```

- [ ] **Step 6: Full check and commit**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`
Expected: all pass.

```bash
git add docs/guide.md \
  src/cli.rs \
  src/distribution/himalaya.rs \
  src/distribution/mod.rs \
  src/distribution/protected.rs \
  src/lib.rs \
  src/service.rs \
  src/update/archive.rs \
  src/update/github.rs \
  src/update/install.rs \
  src/update/platform.rs \
  tests/himalaya_install.rs
git commit -m "Add mailtriage himalaya install and the protected path rule

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Setup offers a private Himalaya and notes Homebrew's

Setup's step 2: when the Himalaya it picked (flag, stored account, or `PATH`) is missing or untested, it says why and offers `Install Himalaya 2.2.1 for mailtriage? [Y/n]`; `--himalaya-install` answers yes, also without prompts; otherwise it fails with the fix. A Himalaya in a Homebrew keg gets the one `brew pin` note. Step 9's `mail` item names the fix for an untested version. The shared test helpers start here.

**Files:**
- Modify: `src/distribution/himalaya.rs`, `src/setup.rs`, `src/cli.rs`, `README.md`, `docs/guide.md`, `docs/hermes.md`
- Test: `tests/install_support/mod.rs` (new), `tests/setup_himalaya.rs` (new)

**Interfaces:**
- Consumes: `distribution::himalaya::{default_data_dir, binary_path, run}` (Task 3), `engine::versions` (Task 1).
- Produces:
  - `setup::SetupArgs.himalaya_install: bool`; CLI `setup --himalaya-install`.
  - `setup::BREW_PIN_NOTE: &str`; `setup::himalaya_install_fix() -> String` (`run mailtriage himalaya install, then mailtriage setup --update --himalaya-binary PATH`).
  - `distribution::himalaya::install_default() -> anyhow::Result<Installed>` (the newest version, this user's data directory and endpoint).
  - `tests/install_support/mod.rs`: `fake_himalaya(version_line: &str) -> String` (a python fake with its state in `MT_FAKE_DIR`; `flip` prints 2.1.0 once, then 2.2.2) and `write_exe(&Path, &str)`.

- [ ] **Step 1: Write the failing tests**

Create `tests/install_support/mod.rs`:

```rust
#![allow(dead_code)]
//! Shared by the install tests: a fake Himalaya. Nothing here touches the
//! real HOME.
use std::{fs, os::unix::fs::PermissionsExt, path::Path};

/// A fake Himalaya whose `--version` prints `@VERSION@` (`flip`: 2.1.0 the
/// first time, 2.2.2 afterwards). It lists the IMAP account `work`, passes
/// `account check`, reports capabilities without SPECIAL-USE and lists
/// INBOX. Its state lives in `MT_FAKE_DIR`, so a copy installed elsewhere
/// behaves the same.
const FAKE_HIMALAYA: &str = r#"#!/usr/bin/env python3
import json, os, sys
state = os.environ["MT_FAKE_DIR"]
argv = sys.argv[1:]
with open(os.path.join(state, "calls.log"), "a") as log:
    log.write(json.dumps([sys.argv[0]] + argv) + "\n")
def out(text):
    sys.stdout.write(text)
    sys.stdout.flush()
if argv and argv[-1] == "--version":
    line = "@VERSION@"
    if line == "flip":
        # Tested at the first call, untested afterwards.
        flag = os.path.join(state, "flipped")
        line = "himalaya v2.2.2 +imap" if os.path.exists(flag) else "himalaya v2.1.0 +imap"
        open(flag, "a").close()
    out(line + "\nbuild: test\n")
    sys.exit(0)
words = " ".join(argv)
if "account list" in words:
    out(json.dumps({"accounts": [{"name": "work", "default": True, "backends": ["imap"]}]}))
elif "account check" in words:
    out(json.dumps({"account": "work", "backends": [{"backend": "imap", "ok": True}]}))
elif "imap raw" in words:
    out('* CAPABILITY IMAP4rev1 MOVE\r\na1 OK done\r\n* NAMESPACE (("" "/")) NIL NIL\r\na2 OK done\r\n')
elif "imap list" in words:
    out(json.dumps({"mailboxes": [{"name": "INBOX", "delimiter": "/", "attributes": []}]}))
else:
    sys.exit(7)
"#;

/// The fake Himalaya printing `version_line` (or `flip`).
pub fn fake_himalaya(version_line: &str) -> String {
    FAKE_HIMALAYA.replace("@VERSION@", version_line)
}

/// Writes an executable file, creating its directory.
pub fn write_exe(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
```

Create `tests/setup_himalaya.rs`:

```rust
#![cfg(unix)]
//! Setup's step 2 with a Himalaya that is missing or untested: it offers a
//! private copy (`himalaya install`, served by the update tests' loopback
//! server with a test-only digest table), `--himalaya-install` answers the
//! offer without prompts, and a Homebrew keg gets the `brew pin` note.
mod install_support;
mod update_support;
use install_support::{fake_himalaya as fake, write_exe};
use mailtriage::{distribution::himalaya::platform_of, update::release};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    os::unix::fs::symlink,
    path::PathBuf,
    process::{Command, Output, Stdio},
};
use update_support::{archive, sha256_hex, Reply, Server};

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    server: Server,
}

impl Fixture {
    /// A home with a Himalaya config for `work`, an empty `bin/` on PATH,
    /// and 2.2.1 for this platform served as a fake that prints
    /// `himalaya v2.2.1 +imap`.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        for d in ["bin", "home/.config/himalaya", "state"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        fs::write(
            root.join("home/.config/himalaya/config.toml"),
            "[accounts.work]\nemail = \"work@example.test\"\nimap.server = \"imaps://mail.example.test\"\n",
        )
        .unwrap();
        let server = Server::start();
        let platform = platform_of(release::platform()).unwrap();
        let tgz = archive(&[("himalaya", fake("himalaya v2.2.1 +imap").as_bytes())]);
        server.reply(
            &format!("/pimalaya/himalaya/releases/download/v2.2.1/himalaya.{platform}.tgz"),
            Reply::ok(tgz.clone()),
        );
        let none = format!("sha256:{}", "0".repeat(64));
        let assets = |digest: &str| {
            let mut assets = serde_json::Map::new();
            for p in ["aarch64-darwin", "x86_64-linux", "aarch64-linux"] {
                let d = if p == platform { digest } else { none.as_str() };
                assets.insert(p.into(), json!(d));
            }
            Value::Object(assets)
        };
        let table = json!({"versions": [
            {"version": "2.1.0", "roles": {}, "assets": assets(&none)},
            {"version": "2.2.1", "roles": {"inbox": "INBOX"},
             "assets": assets(&format!("sha256:{}", sha256_hex(&tgz)))},
        ]});
        fs::write(root.join("versions.json"), table.to_string()).unwrap();
        Self {
            _dir: dir,
            root,
            server,
        }
    }

    fn private(&self) -> PathBuf {
        self.root.join("data/mailtriage/himalaya/2.2.1/himalaya")
    }

    fn config(&self) -> Value {
        serde_json::from_slice(
            &fs::read(self.root.join("home/.config/mailtriage/mailtriage.json")).unwrap(),
        )
        .unwrap()
    }

    fn setup(&self, args: &[&str], stdin: &str) -> (Output, Value) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
            .arg("setup")
            .args(args)
            .args(["--json", "--provider", "fake", "--service", "skip"])
            .current_dir(&self.root)
            .env("HOME", self.root.join("home"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("bin").display()),
            )
            .env("TZ", "Europe/Berlin")
            .env("MT_FAKE_DIR", self.root.join("state"))
            .env("MAILTRIAGE_UPDATE_URL", &self.server.base)
            .env(
                "MAILTRIAGE_TEST_HIMALAYA_VERSIONS",
                self.root.join("versions.json"),
            )
            .env_remove("MAILTRIAGE_CONFIG")
            .env_remove("HIMALAYA_CONFIG")
            .env_remove("XDG_CONFIG_HOME")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
        let out = child.wait_with_output().unwrap();
        let value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
        (out, value)
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn prompts_offer_a_private_himalaya_for_an_untested_one() {
    let f = Fixture::new();
    write_exe(&f.root.join("bin/himalaya"), &fake("himalaya v2.2.2 +imap"));
    // Yes to the private copy, then every default.
    let (out, v) = f.setup(&["--interactive"], &"\n".repeat(12));
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("Install Himalaya 2.2.1 for mailtriage? [Y/n]"),
        "{err}"
    );
    assert!(
        err.contains("Himalaya 2.2.2 is not a tested version"),
        "{err}"
    );
    let engine = &f.config()["accounts"]["work"]["engine"];
    assert_eq!(engine["binary"], f.private().to_str().unwrap());
    assert_eq!(engine["expected_version"], "2.2.1");
}

#[test]
fn declining_the_private_himalaya_fails_with_the_fix() {
    let f = Fixture::new();
    write_exe(&f.root.join("bin/himalaya"), &fake("himalaya v2.2.2 +imap"));
    let (out, v) = f.setup(&["--interactive"], "n\n");
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    let message = v["error"]["message"].as_str().unwrap();
    assert!(
        message.ends_with(&format!(
            "run mailtriage himalaya install, then mailtriage setup --update --himalaya-binary {}, or pass --himalaya-install",
            f.private().display()
        )),
        "{message}"
    );
    assert!(!f.private().exists());
    assert_eq!(f.server.count("/pimalaya/"), 0);
}

#[test]
fn himalaya_install_answers_the_offer_without_prompts() {
    let f = Fixture::new();
    // Nothing on PATH: missing counts like untested.
    let (out, v) = f.setup(&["--yes", "--himalaya-account", "work"], "");
    assert_eq!(out.status.code(), Some(3), "{v}");
    let message = v["error"]["message"].as_str().unwrap();
    assert!(
        message.starts_with("step 2 (Himalaya): himalaya is not on PATH; "),
        "{message}"
    );
    let (out, v) = f.setup(
        &["--yes", "--himalaya-account", "work", "--himalaya-install"],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    assert_eq!(
        f.config()["accounts"]["work"]["engine"]["binary"],
        f.private().to_str().unwrap()
    );
    // A tested Himalaya found on PATH is used as it is.
    fs::remove_file(f.root.join("home/.config/mailtriage/mailtriage.json")).unwrap();
    write_exe(&f.root.join("bin/himalaya"), &fake("himalaya v2.1.0 +imap"));
    let (out, _) = f.setup(
        &["--yes", "--himalaya-account", "work", "--himalaya-install"],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        f.config()["accounts"]["work"]["engine"]["binary"],
        f.root.join("bin/himalaya").to_str().unwrap()
    );
}

#[test]
fn a_himalaya_in_a_homebrew_keg_gets_the_pin_note() {
    let f = Fixture::new();
    let keg = f.root.join("brew/Cellar/himalaya/2.1.0/bin/himalaya");
    write_exe(&keg, &fake("himalaya v2.1.0 +imap"));
    fs::create_dir_all(f.root.join("brew/bin")).unwrap();
    symlink(&keg, f.root.join("brew/bin/himalaya")).unwrap();
    let linked = f.root.join("brew/bin/himalaya");
    let args = ["--yes", "--himalaya-account", "work", "--himalaya-binary"];
    let (out, _) = f.setup(&[&args[..], &[linked.to_str().unwrap()]].concat(), "");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let note = "Homebrew may upgrade Himalaya to a version mailtriage has not tested; \"brew pin himalaya\" holds it, or run mailtriage himalaya install for a private copy.";
    assert_eq!(stderr(&out).matches(note).count(), 1, "{}", stderr(&out));
    // Not for a Himalaya outside a keg.
    fs::remove_file(f.root.join("home/.config/mailtriage/mailtriage.json")).unwrap();
    write_exe(&f.root.join("bin/himalaya"), &fake("himalaya v2.1.0 +imap"));
    let (out, _) = f.setup(&["--yes", "--himalaya-account", "work"], "");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(!stderr(&out).contains("brew pin"), "{}", stderr(&out));
}

#[test]
fn the_mail_item_names_the_private_himalaya_for_an_untested_version() {
    let f = Fixture::new();
    write_exe(&f.root.join("bin/himalaya"), &fake("flip"));
    let (out, v) = f.setup(&["--yes", "--himalaya-account", "work"], "");
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    let items = v["setup"]["doctor"]["items"].as_array().unwrap();
    let mail = items.iter().find(|i| i["check"] == "mail").unwrap();
    assert_eq!(mail["ready"], false);
    assert_eq!(
        mail["error"],
        "Himalaya 2.2.2 is not a tested version (tested: 2.1.0, 2.2.1)"
    );
    assert_eq!(
        mail["fix"],
        format!(
            "run mailtriage himalaya install, then mailtriage setup --update --himalaya-binary {}",
            f.private().display()
        )
    );
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test --locked --test setup_himalaya`
Expected: FAIL, all 5 tests. For example `prompts_offer_a_private_himalaya_for_an_untested_one` exits 3 with `step 2 (Himalaya): …: Himalaya 2.2.2 is not a tested version (tested: 2.1.0, 2.2.1); …` and no offer, and `himalaya_install_answers_the_offer_without_prompts` exits 2 with `unexpected argument '--himalaya-install'`.

- [ ] **Step 3: Implement**

In `src/distribution/himalaya.rs`, replace:

```rust
/// `mailtriage himalaya install [--version X.Y.Z]` with this user's data
/// directory and the release endpoint (the loopback override in tests).
pub fn run(version: Option<&str>) -> Result<Value> {
    let data = default_data_dir().ok_or_else(|| {
        err(
            3,
```

with:

```rust
/// `mailtriage himalaya install [--version X.Y.Z]` with this user's data
/// directory and the release endpoint (the loopback override in tests).
pub fn run(version: Option<&str>) -> Result<Value> {
    Ok(install_here(version)?.json())
}

/// The newest tested version, installed as `run` does; for setup.
pub fn install_default() -> Result<Installed> {
    install_here(None)
}

fn install_here(version: Option<&str>) -> Result<Installed> {
    let data = default_data_dir().ok_or_else(|| {
        err(
            3,
```

In `src/distribution/himalaya.rs`, replace:

```rust
    })?;
    let net = Net::new(Endpoint::from_env())
        .map_err(|e| err(3, format!("cannot start an HTTPS client: {e}")))?;
    Ok(install(version, &net, &data)?.json())
}

/// Installs `version` (default: the newest tested one) into `data`. Errors
```

with:

```rust
    })?;
    let net = Net::new(Endpoint::from_env())
        .map_err(|e| err(3, format!("cannot start an HTTPS client: {e}")))?;
    install(version, &net, &data)
}

/// Installs `version` (default: the newest tested one) into `data`. Errors
```

In `src/setup.rs`, replace:

```rust
//! and its state directory: it makes no IMAP changes, never handles the API
//! key and never selects `live` filing.
use crate::{
    config,
    domain::{
        AccountConfig, AppConfig, Category, EngineConfig, FilingConfig, FilingMode, HimalayaConfig,
        ProviderConfig, UpdateMode,
    },
    engine::{self, versions},
    filing, process,
    prompt::Prompter,
    provider,
```

with:

```rust
//! and its state directory: it makes no IMAP changes, never handles the API
//! key and never selects `live` filing.
use crate::{
    config, distribution,
    domain::{
        AccountConfig, AppConfig, Category, EngineConfig, FilingConfig, FilingMode, HimalayaConfig,
        ProviderConfig, UpdateMode,
    },
    engine::{
        self,
        versions::{self, Tested},
    },
    filing, process,
    prompt::Prompter,
    provider,
```

In `src/setup.rs`, replace:

```rust
    /// Stdin is a terminal, so a key tool can prompt even with `--yes`.
    pub terminal: bool,
    pub himalaya_binary: Option<PathBuf>,
    pub himalaya_config: Option<PathBuf>,
    pub himalaya_account: Option<String>,
    pub account: Option<String>,
```

with:

```rust
    /// Stdin is a terminal, so a key tool can prompt even with `--yes`.
    pub terminal: bool,
    pub himalaya_binary: Option<PathBuf>,
    /// `--himalaya-install`: answer yes to installing a private Himalaya
    /// when the one found is missing or untested, also without prompts.
    pub himalaya_install: bool,
    pub himalaya_config: Option<PathBuf>,
    pub himalaya_account: Option<String>,
    pub account: Option<String>,
```

In `src/setup.rs`, replace:

```rust
    p: &mut Prompter,
    stored: Option<&HimalayaConfig>,
) -> Result<HimalayaChoice> {
    let binary = match (&args.himalaya_binary, stored) {
        (Some(binary), _) => absolute(binary, "--himalaya-binary")?,
        (None, Some(stored)) => stored.binary.clone(),
        (None, None) => process::find_on_path("himalaya").ok_or_else(|| {
            err(
                3,
                format!(
                    "step 2 (Himalaya): himalaya is not on PATH; install a tested Himalaya ({}) or pass --himalaya-binary",
                    versions::listed()
                ),
            )
        })?,
    };
    let (line, tested) = match run_himalaya(&binary, &[OsStr::new("--version")]) {
        Ok(out) => versions::check_version_output(&out).map_err(|untested| {
            err(
                3,
                format!(
                    "step 2 (Himalaya): {}: {untested}; install a tested Himalaya or pass --himalaya-binary",
                    binary.display()
                ),
            )
        })?,
        Err(_) => {
            return Err(err(
                3,
                format!(
                    "step 2 (Himalaya): {} does not run; install a tested Himalaya ({}) or pass --himalaya-binary",
                    binary.display(),
                    versions::listed()
                ),
            ))
        }
    };
    p.say(&format!("Using {line} at {}.", binary.display()));
    let explicit = match (&args.himalaya_config, stored) {
        (Some(toml), _) => {
            let toml = absolute(toml, "--himalaya-config")?;
```

with:

```rust
    p: &mut Prompter,
    stored: Option<&HimalayaConfig>,
) -> Result<HimalayaChoice> {
    let found = match (&args.himalaya_binary, stored) {
        (Some(binary), _) => Some(absolute(binary, "--himalaya-binary")?),
        (None, Some(stored)) => Some(stored.binary.clone()),
        (None, None) => process::find_on_path("himalaya"),
    };
    let checked = match &found {
        Some(binary) => himalaya_version(binary).map(|version| (binary.clone(), version)),
        None => Err("himalaya is not on PATH".to_owned()),
    };
    let (binary, (line, tested)) = match checked {
        Ok(found) => found,
        Err(why) => private_himalaya(args, p, &why)?,
    };
    p.say(&format!("Using {line} at {}.", binary.display()));
    if in_homebrew_keg(&binary) {
        p.say(BREW_PIN_NOTE);
    }
    let explicit = match (&args.himalaya_config, stored) {
        (Some(toml), _) => {
            let toml = absolute(toml, "--himalaya-config")?;
```

In `src/setup.rs`, replace:

```rust
    }
}

/// Himalaya v2.1.0's config search order without `--config` (verified on
/// macOS; Linux follows the `dirs` crate): the platform config dir, then
/// `~/.config`, then `~/.himalayarc`.
```

with:

```rust
    }
}

/// Printed once when the chosen Himalaya is in a Homebrew keg.
pub const BREW_PIN_NOTE: &str = "Homebrew may upgrade Himalaya to a version mailtriage has not tested; \"brew pin himalaya\" holds it, or run mailtriage himalaya install for a private copy.";

/// The first line of `binary --version` and its tested entry, or why the
/// binary cannot be used.
fn himalaya_version(binary: &Path) -> Result<(String, &'static Tested), String> {
    match run_himalaya(binary, &[OsStr::new("--version")]) {
        Ok(out) => {
            versions::check_version_output(&out).map_err(|u| format!("{}: {u}", binary.display()))
        }
        Err(_) => Err(format!("{} does not run", binary.display())),
    }
}

/// Step 2 for a Himalaya that is missing or untested (`why`): offers the
/// private copy (default yes); `--himalaya-install` answers yes, also
/// without prompts. Otherwise the step fails with the fix.
fn private_himalaya(
    args: &SetupArgs,
    p: &mut Prompter,
    why: &str,
) -> Result<(PathBuf, (String, &'static Tested))> {
    let newest = &versions::newest().version;
    let wanted = args.himalaya_install
        || (p.enabled() && {
            p.say(&format!("{why}."));
            p.confirm(&format!("Install Himalaya {newest} for mailtriage?"), true)?
        });
    if !wanted {
        return Err(err(
            3,
            format!(
                "step 2 (Himalaya): {why}; {}, or pass --himalaya-install",
                himalaya_install_fix()
            ),
        ));
    }
    p.say(&format!("Installing Himalaya {newest} for mailtriage."));
    let installed = distribution::himalaya::install_default().map_err(|e| {
        err(
            service::exit_code(&e),
            format!(
                "step 2 (Himalaya): could not install Himalaya {newest}: {}",
                error_text(&e)
            ),
        )
    })?;
    p.say(&format!(
        "Installed Himalaya {} at {}.",
        installed.version,
        installed.path.display()
    ));
    let version = himalaya_version(&installed.path)
        .map_err(|why| err(3, format!("step 2 (Himalaya): {why}")))?;
    Ok((installed.path, version))
}

/// The fix for a missing or untested Himalaya: the private copy, then setup
/// pointed at the path `himalaya install` prints.
pub fn himalaya_install_fix() -> String {
    let path = distribution::himalaya::default_data_dir()
        .map(|data| distribution::himalaya::binary_path(&data, &versions::newest().version));
    let path = path.map_or_else(|| "<the path it prints>".to_owned(), |p| shell_line(&[p]));
    format!(
        "run mailtriage himalaya install, then mailtriage setup --update --himalaya-binary {path}"
    )
}

/// Whether `binary` is in a Homebrew keg: its canonical path contains
/// `/Cellar/himalaya/`.
fn in_homebrew_keg(binary: &Path) -> bool {
    fs::canonicalize(binary).is_ok_and(|p| p.to_string_lossy().contains("/Cellar/himalaya/"))
}

/// Himalaya v2.1.0's config search order without `--config` (verified on
/// macOS; Linux follows the `dirs` crate): the platform config dir, then
/// `~/.config`, then `~/.himalayarc`.
```

In `src/setup.rs`, replace:

```rust
                key["key_error"].as_str().map(str::to_owned),
                key_fix,
            ));
            items.push(check_item(
                "mail",
                report["transport"]["ready"] == true,
                None,
                format!(
                    "run `{}`",
                    shell_line(&[
                        engine.binary.as_os_str(),
                        OsStr::new("--config"),
                        engine.config.as_os_str(),
                        OsStr::new("--account"),
                        OsStr::new(&engine.account),
                        OsStr::new("account"),
                        OsStr::new("check"),
                    ])
                ),
            ));
            if let Some(filing) = report.get("filing") {
                let problems = filing["problems"].as_array().map_or(0, Vec::len);
                items.push(check_item(
```

with:

```rust
                key["key_error"].as_str().map(str::to_owned),
                key_fix,
            ));
            let transport = &report["transport"];
            let (error, fix) = if transport["tested"] == false {
                (
                    transport["error"].as_str().map(str::to_owned),
                    himalaya_install_fix(),
                )
            } else {
                let check = shell_line(&[
                    engine.binary.as_os_str(),
                    OsStr::new("--config"),
                    engine.config.as_os_str(),
                    OsStr::new("--account"),
                    OsStr::new(&engine.account),
                    OsStr::new("account"),
                    OsStr::new("check"),
                ]);
                (None, format!("run `{check}`"))
            };
            items.push(check_item("mail", transport["ready"] == true, error, fix));
            if let Some(filing) = report.get("filing") {
                let problems = filing["problems"].as_array().map_or(0, Vec::len);
                items.push(check_item(
```

In `src/cli.rs`, replace:

```rust
    interactive: bool,
    #[arg(long)]
    himalaya_binary: Option<PathBuf>,
    #[arg(long)]
    himalaya_config: Option<PathBuf>,
    #[arg(long)]
```

with:

```rust
    interactive: bool,
    #[arg(long)]
    himalaya_binary: Option<PathBuf>,
    /// Install a tested Himalaya for mailtriage when the one found is
    /// missing or untested, without asking.
    #[arg(long)]
    himalaya_install: bool,
    #[arg(long)]
    himalaya_config: Option<PathBuf>,
    #[arg(long)]
```

In `src/cli.rs`, replace:

```rust
            update: self.update,
            terminal,
            himalaya_binary: self.himalaya_binary.clone(),
            himalaya_config: self.himalaya_config.clone(),
            himalaya_account: self.himalaya_account.clone(),
            account: self.account.clone(),
```

with:

```rust
            update: self.update,
            terminal,
            himalaya_binary: self.himalaya_binary.clone(),
            himalaya_install: self.himalaya_install,
            himalaya_config: self.himalaya_config.clone(),
            himalaya_account: self.himalaya_account.clone(),
            account: self.account.clone(),
```

- [ ] **Step 4: Run the new tests**

Run: `cargo test --locked --test setup_himalaya --test setup --test himalaya_versions`
Expected: PASS.

- [ ] **Step 5: Document it**

In `README.md`, replace:

```markdown
- [Himalaya](https://github.com/pimalaya/himalaya) with IMAP support (`himalaya --version` shows `+imap`), in a version mailtriage is tested with: 2.1.0 or 2.2.1 ([Himalaya versions](docs/guide.md#himalaya-versions)).
```

with:

```markdown
- [Himalaya](https://github.com/pimalaya/himalaya) with IMAP support, in a version mailtriage is tested with (2.1.0 or 2.2.1). `mailtriage setup` offers to install one for mailtriage when none is found ([Himalaya versions](docs/guide.md#himalaya-versions)).
```

In `docs/guide.md`, replace:

```markdown
| 2. Himalaya | Finds the Himalaya binary, its config file and the account, and runs `himalaya account check`. Can create an account with `himalaya configure`. | `--himalaya-binary`, `--himalaya-config`, `--himalaya-account` |
```

with:

```markdown
| 2. Himalaya | Finds the Himalaya binary, its config file and the account, and runs `himalaya account check`. Can install a tested Himalaya for mailtriage, and create an account with `himalaya configure`. | `--himalaya-binary`, `--himalaya-install`, `--himalaya-config`, `--himalaya-account` |
```

In `docs/guide.md`, replace:

```markdown
- Binary: `--himalaya-binary`, else the stored binary of the account being updated, else the first `himalaya` on `PATH`. Setup stores it as an absolute path. Its `--version` must report a [tested version](#himalaya-versions) with `+imap`; otherwise setup exits 3. Setup writes the version it found into `expected_version`.
```

with:

```markdown
- Binary: `--himalaya-binary`, else the stored binary of the account being updated, else the first `himalaya` on `PATH`. Setup stores it as an absolute path. Its `--version` must report a [tested version](#himalaya-versions) with `+imap`. Setup writes the version it found into `expected_version`.
- When that Himalaya is missing or untested, setup says why and asks `Install Himalaya 2.2.1 for mailtriage? [Y/n]` (default yes). It then installs a [private Himalaya](#a-private-himalaya) and uses it. Without prompts, `--himalaya-install` answers yes; otherwise setup exits 3 with the fix `run mailtriage himalaya install, then mailtriage setup --update --himalaya-binary PATH`. With a tested Himalaya found, `--himalaya-install` changes nothing.
- When the chosen Himalaya is in a Homebrew keg (its path with symlinks resolved contains `/Cellar/himalaya/`), setup prints once: `Homebrew may upgrade Himalaya to a version mailtriage has not tested; "brew pin himalaya" holds it, or run mailtriage himalaya install for a private copy.`
```

In `docs/guide.md`, replace:

```markdown
**Step 9, check.** Setup runs `doctor` for the account. It prints each item (`provider`, `key`, `mail`, `filing` when filing is on, and `update` when `updates` is `auto` but the binary may not be replaced) as `ok`, or as `not ready` with the one command that fixes it. The `update` item does not make `doctor.ready` false, as in `doctor` itself. If `doctor` itself fails, for example because the state database cannot be opened, setup reports a single not-ready `state` item instead. Setup exits 0 even when an item is not ready.
```

with:

```markdown
**Step 9, check.** Setup runs `doctor` for the account. It prints each item (`provider`, `key`, `mail`, `filing` when filing is on, and `update` when `updates` is `auto` but the binary may not be replaced) as `ok`, or as `not ready` with the one command that fixes it. The `update` item does not make `doctor.ready` false, as in `doctor` itself. If `doctor` itself fails, for example because the state database cannot be opened, setup reports a single not-ready `state` item instead. For an untested Himalaya, the `mail` item's fix is `run mailtriage himalaya install, then mailtriage setup --update --himalaya-binary PATH`, with the path that command installs to. Setup exits 0 even when an item is not ready.
```

In `docs/guide.md`, replace:

```markdown
- Point an account at it with `mailtriage setup --update --himalaya-binary PATH`, using the path it printed.
```

with:

```markdown
- Point an account at it with `mailtriage setup --update --himalaya-binary PATH`, using the path it printed. Setup offers this itself when it finds no tested Himalaya (step 2).
```

In `docs/guide.md`, replace:

```markdown
`action` is `installed` or `current`. Exit codes: 0; 2 for a version that is not tested, or a platform for which pimalaya has no build mailtriage can use; 3 for a network error, a checksum mismatch, a bad archive, a binary that does not run, an unsafe directory, or another `himalaya install` that held its directory's lock for 60 seconds.

## Daily use
```

with:

```markdown
`action` is `installed` or `current`. Exit codes: 0; 2 for a version that is not tested, or a platform for which pimalaya has no build mailtriage can use; 3 for a network error, a checksum mismatch, a bad archive, a binary that does not run, an unsafe directory, or another `himalaya install` that held its directory's lock for 60 seconds.

### Homebrew's Himalaya

`brew upgrade` may move Homebrew's `himalaya` to a version mailtriage has not tested; mailtriage then refuses it until a mailtriage release tests that version. Either hold it with `brew pin himalaya` (and `brew unpin himalaya` once mailtriage tests the newer one), or give mailtriage its own copy with `mailtriage himalaya install` and `mailtriage setup --update --himalaya-binary PATH`. Setup says so when the Himalaya it uses is Homebrew's.

## Daily use
```

In `docs/hermes.md`, replace:

```markdown
   - `--himalaya-account NAME` is required: an account with IMAP in the Himalaya configuration. Add `--himalaya-binary PATH` when `himalaya` is not on the agent's `PATH`, and `--himalaya-config PATH` when the file is not in Himalaya's default location. Setup stores both as absolute paths.
```

with:

```markdown
   - `--himalaya-account NAME` is required: an account with IMAP in the Himalaya configuration. Add `--himalaya-install` to install a tested Himalaya for mailtriage when none is found or the one found is untested, `--himalaya-binary PATH` when `himalaya` is not on the agent's `PATH`, and `--himalaya-config PATH` when the file is not in Himalaya's default location. Setup stores both as absolute paths.
```

In `docs/hermes.md`, replace:

```markdown
   | 3 | Himalaya is missing or not a tested version (see [Himalaya versions](guide.md#himalaya-versions)), `himalaya account check` failed, the folders could not be listed, a key tool or key command failed, or `launchctl`/`systemctl` failed. | Report the message to the user. It names the command that shows the cause; fixing it needs a person (credentials, Himalaya, the key store). |
```

with:

```markdown
   | 3 | Himalaya is missing or not a tested version and `--himalaya-install` was not given (see [Himalaya versions](guide.md#himalaya-versions)), installing the private Himalaya failed, `himalaya account check` failed, the folders could not be listed, a key tool or key command failed, or `launchctl`/`systemctl` failed. | Report the message to the user. It names the command that shows the cause; fixing it needs a person (credentials, Himalaya, the key store). |
```

- [ ] **Step 6: Full check and commit**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`
Expected: all pass.

```bash
git add README.md \
  docs/guide.md \
  docs/hermes.md \
  src/cli.rs \
  src/distribution/himalaya.rs \
  src/setup.rs \
  tests/install_support/mod.rs \
  tests/setup_himalaya.rs
git commit -m "Offer a private Himalaya in setup and note Homebrew's

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: End-to-end matrix from the data file and the weekly Himalaya check

The Dovecot workflow reads its matrix from the data file and never changes when a version is added; `tests/e2e/run.sh` accepts any listed version. `scripts/add-himalaya-version.sh` adds a version from a release's asset digests (failing closed) with the previous roles, and `.github/workflows/himalaya-compat.yml` runs it weekly in three jobs (test, propose, report). Both helpers are tested from Rust on copies.

**Files:**
- Create: `.github/scripts/himalaya_matrix.py`, `scripts/add-himalaya-version.sh`, `.github/workflows/himalaya-compat.yml`
- Modify: `tests/e2e/run.sh`, `.github/workflows/e2e.yml`, `docs/releases.md`, `docs/verification.md`
- Test: `tests/himalaya_scripts.rs` (new)

**Interfaces:**
- Consumes: `engine::versions::{parse, DATA, Tested::sha256}` (Task 1) in tests.
- Produces:
  - `python3 .github/scripts/himalaya_matrix.py DATA_FILE` prints one JSON line `[{"version":"2.1.0","sha256":"<x86_64-linux hex>"},…]`.
  - `scripts/add-himalaya-version.sh X.Y.Z`: exit 0 after appending the entry in the file's layout; 1 when the version is listed, not higher, not the release's tag, a draft or prerelease, or a digest is missing or malformed (the file is unchanged); 2 for usage. `HIMALAYA_VERSIONS_FILE` and `HIMALAYA_RELEASE_JSON` replace the data file and the API (tests); `GH_TOKEN` or `GITHUB_TOKEN` is sent when set.
  - `tests/e2e/run.sh` accepts `himalaya v<listed> …` for every listed version.

- [ ] **Step 1: Write the failing tests**

Create `tests/himalaya_scripts.rs`:

```rust
#![cfg(unix)]
//! The CI helpers that read and extend `src/engine/himalaya-versions.json`:
//! the end-to-end matrix and `scripts/add-himalaya-version.sh`, which the
//! weekly check runs. They run on copies; the real data file is only read.
use mailtriage::engine::versions;
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn data_file() -> PathBuf {
    repo().join("src/engine/himalaya-versions.json")
}

#[test]
fn the_e2e_matrix_lists_every_tested_version_with_its_linux_digest() {
    let out = Command::new("python3")
        .arg(repo().join(".github/scripts/himalaya_matrix.py"))
        .arg(data_file())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let matrix: Value = serde_json::from_slice(&out.stdout).unwrap();
    let expected: Vec<Value> = versions::parse(versions::DATA)
        .unwrap()
        .iter()
        .map(|t| json!({"version": t.version, "sha256": t.sha256("x86_64-linux").unwrap()}))
        .collect();
    assert_eq!(matrix, Value::from(expected));
}

/// A release of pimalaya/himalaya as the API returns it, with a digest for
/// each platform except those in `without`.
fn release(version: &str, without: &[&str]) -> Value {
    let assets: Vec<Value> = [
        "aarch64-darwin",
        "x86_64-linux",
        "aarch64-linux",
        "x86_64-darwin",
        "x86_64-windows",
    ]
    .iter()
    .enumerate()
    .map(|(i, platform)| {
        let mut asset = json!({"name": format!("himalaya.{platform}.tgz"), "size": 6_000_000});
        if !without.contains(platform) {
            asset["digest"] = json!(format!("sha256:{}", format!("{i}").repeat(64)));
        }
        asset
    })
    .collect();
    json!({"tag_name": format!("v{version}"), "draft": false, "prerelease": false, "assets": assets})
}

struct Copy {
    dir: tempfile::TempDir,
}

impl Copy {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::copy(data_file(), dir.path().join("versions.json")).unwrap();
        Self { dir }
    }

    fn path(&self) -> PathBuf {
        self.dir.path().join("versions.json")
    }

    fn text(&self) -> String {
        fs::read_to_string(self.path()).unwrap()
    }

    fn add(&self, version: &str, release: &Value) -> Output {
        let fixture = self.dir.path().join("release.json");
        fs::write(&fixture, release.to_string()).unwrap();
        Command::new("bash")
            .arg(repo().join("scripts/add-himalaya-version.sh"))
            .arg(version)
            .env("HIMALAYA_VERSIONS_FILE", self.path())
            .env("HIMALAYA_RELEASE_JSON", &fixture)
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .output()
            .unwrap()
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn the_helper_adds_the_release_digests_and_the_previous_roles() {
    let copy = Copy::new();
    let before = copy.text();
    let out = copy.add("2.3.0", &release("2.3.0", &[]));
    assert!(out.status.success(), "{}", stderr(&out));
    let after = copy.text();
    // The listed entries keep their bytes; the new one comes last.
    let kept = before.strip_suffix("\n]}\n").unwrap();
    assert!(after.starts_with(&format!("{kept},\n")), "{after}");
    let tested = versions::parse(&after).unwrap();
    let new = tested.last().unwrap();
    assert_eq!(new.version, "2.3.0");
    assert_eq!(new.roles, tested[tested.len() - 2].roles);
    assert_eq!(new.sha256("aarch64-darwin"), Some(&*"0".repeat(64)));
    assert_eq!(new.sha256("x86_64-linux"), Some(&*"1".repeat(64)));
    assert_eq!(new.sha256("aarch64-linux"), Some(&*"2".repeat(64)));
    assert!(String::from_utf8_lossy(&out.stdout).contains("roles copied from"));
}

#[test]
fn a_missing_digest_fails_and_changes_nothing() {
    let copy = Copy::new();
    let before = copy.text();
    let out = copy.add("2.3.0", &release("2.3.0", &["aarch64-linux"]));
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("no sha256 digest for himalaya.aarch64-linux.tgz"),
        "{}",
        stderr(&out)
    );
    assert_eq!(copy.text(), before);
}

#[test]
fn a_listed_lower_or_unstable_version_is_refused() {
    let copy = Copy::new();
    let before = copy.text();
    for (version, release, why) in [
        ("2.2.1", release("2.2.1", &[]), "already listed"),
        ("2.2.0", release("2.2.0", &[]), "not higher"),
        ("2.3.0", release("2.3.1", &[]), "not v2.3.0"),
        (
            "2.3.0",
            {
                let mut r = release("2.3.0", &[]);
                r["prerelease"] = json!(true);
                r
            },
            "prerelease",
        ),
    ] {
        let out = copy.add(version, &release);
        assert_eq!(out.status.code(), Some(1), "{version}");
        assert!(stderr(&out).contains(why), "{version}: {}", stderr(&out));
    }
    let out = copy.add("v2.3.0", &release("2.3.0", &[]));
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(copy.text(), before);
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test --locked --test himalaya_scripts`
Expected: FAIL, all 4 tests: python3 cannot open `.github/scripts/himalaya_matrix.py`, and bash cannot open `scripts/add-himalaya-version.sh`.

- [ ] **Step 3: Implement**

`tests/e2e/run.sh` is not a Rust test file but belongs here; it is listed under Step 3. Make the two new scripts executable: `chmod +x .github/scripts/himalaya_matrix.py scripts/add-himalaya-version.sh` (the workflows call them through `python3` and `bash`, so the mode is for people). Check the workflows parse: `ruby -ryaml -e 'YAML.load_file(ARGV[0])' .github/workflows/e2e.yml` and the same for `himalaya-compat.yml` (or any YAML parser). Check `run.sh`'s version check by hand without Docker: `MT_E2E_HIMALAYA=<a script printing "himalaya v2.2.2 +imap"> bash tests/e2e/run.sh flat 31143` must print `e2e: expected a tested Himalaya (2.1.0 2.2.1), got: himalaya v2.2.2 +imap` and exit 1.

Create `.github/scripts/himalaya_matrix.py`:

```python
#!/usr/bin/env python3
"""The Dovecot job's matrix: every tested Himalaya version and the SHA-256
of its x86_64-linux release archive, from src/engine/himalaya-versions.json.

Usage: himalaya_matrix.py DATA_FILE

Prints one line of JSON: [{"version": "2.1.0", "sha256": "<64 hex>"}, ...].
"""
import json
import sys


def main(path):
    with open(path) as data:
        versions = json.load(data)['versions']
    matrix = []
    for entry in versions:
        digest = entry['assets']['x86_64-linux']
        if not digest.startswith('sha256:'):
            raise SystemExit(f"{entry['version']}: no sha256 digest for x86_64-linux")
        matrix.append({'version': entry['version'], 'sha256': digest[len('sha256:'):]})
    print(json.dumps(matrix, separators=(',', ':')))


if __name__ == '__main__':
    main(sys.argv[1])
```

Create `scripts/add-himalaya-version.sh`:

```sh
#!/usr/bin/env bash
# Adds a Himalaya version to src/engine/himalaya-versions.json:
#
#   scripts/add-himalaya-version.sh X.Y.Z
#
# The digests come from the asset `digest` fields of pimalaya/himalaya's
# release vX.Y.Z (one per platform mailtriage needs; a missing one fails and
# changes nothing). `roles` is copied from the newest listed version: read
# the new version's `--mailbox` resolver and correct it by hand when it
# changed. The version must be a stable release higher than every listed
# one.
#
# Environment: GH_TOKEN or GITHUB_TOKEN is sent to the GitHub API when set.
# For tests, HIMALAYA_VERSIONS_FILE replaces the data file and
# HIMALAYA_RELEASE_JSON names a file with the release JSON instead of the API.
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: $0 X.Y.Z" >&2
  exit 2
fi
version=$1
if ! printf '%s\n' "$version" | grep -Eqx '(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)'; then
  echo "add-himalaya-version: $version is not X.Y.Z" >&2
  exit 2
fi

root="$(cd "$(dirname "$0")/.." && pwd)"
data="${HIMALAYA_VERSIONS_FILE:-$root/src/engine/himalaya-versions.json}"
release_file="$(mktemp)"
trap 'rm -f "$release_file"' EXIT

if [ -n "${HIMALAYA_RELEASE_JSON:-}" ]; then
  cp "$HIMALAYA_RELEASE_JSON" "$release_file"
else
  token="${GH_TOKEN:-${GITHUB_TOKEN:-}}"
  auth=()
  if [ -n "$token" ]; then
    auth=(-H "Authorization: Bearer $token")
  fi
  curl --proto '=https' --tlsv1.2 -fsSL --max-time 60 \
    -H 'Accept: application/vnd.github+json' \
    -H 'X-GitHub-Api-Version: 2022-11-28' \
    ${auth[@]+"${auth[@]}"} \
    -o "$release_file" \
    "https://api.github.com/repos/pimalaya/himalaya/releases/tags/v$version"
fi

python3 - "$data" "$version" "$release_file" <<'PY'
import json
import re
import sys

PLATFORMS = ['aarch64-darwin', 'x86_64-linux', 'aarch64-linux']
DIGEST = re.compile(r'sha256:[0-9a-f]{64}')

data_path, version, release_path = sys.argv[1:]


def fail(message):
    print(f'add-himalaya-version: {message}', file=sys.stderr)
    sys.exit(1)


def parts(v):
    return tuple(int(p) for p in v.split('.'))


def render(entries):
    lines = []
    for e in entries:
        roles = json.dumps(e['roles'], separators=(',', ':'))
        assets = json.dumps({p: e['assets'][p] for p in PLATFORMS}, separators=(',', ':'))
        lines.append(f'  {{"version":{json.dumps(e["version"])},"roles":{roles},\n   "assets":{assets}}}')
    return '{"versions":[\n' + ',\n'.join(lines) + '\n]}\n'


with open(data_path) as f:
    entries = json.load(f)['versions']
with open(release_path) as f:
    release = json.load(f)

if any(e['version'] == version for e in entries):
    fail(f'{version} is already listed')
if parts(version) <= parts(entries[-1]['version']):
    fail(f'{version} is not higher than {entries[-1]["version"]}')
if release.get('tag_name') != f'v{version}':
    fail(f'the release is {release.get("tag_name")!r}, not v{version}')
if release.get('draft') or release.get('prerelease'):
    fail(f'v{version} is a draft or a prerelease')

assets = {}
for platform in PLATFORMS:
    name = f'himalaya.{platform}.tgz'
    found = [a for a in release.get('assets', []) if a.get('name') == name]
    if len(found) != 1:
        fail(f'the release has no single {name}')
    digest = found[0].get('digest') or ''
    if not DIGEST.fullmatch(digest):
        fail(f'the release has no sha256 digest for {name}')
    assets[platform] = digest

entries.append({'version': version, 'roles': dict(entries[-1]['roles']), 'assets': assets})
with open(data_path, 'w') as f:
    f.write(render(entries))
print(f'Added Himalaya {version}; roles copied from {entries[-2]["version"]}: {json.dumps(entries[-1]["roles"])}')
PY
```

In `tests/e2e/run.sh`, replace:

```sh
#
#   MT_E2E_HIMALAYA=/path/to/himalaya bash tests/e2e/run.sh flat|prefix PORT
#
# Needs Docker, python3, cargo and a Himalaya v2.1.0 binary. The container is
# reachable on 127.0.0.1:PORT only and is always stopped when this script exits.
set -euo pipefail

fail() {
```

with:

```sh
#
#   MT_E2E_HIMALAYA=/path/to/himalaya bash tests/e2e/run.sh flat|prefix PORT
#
# Needs Docker, python3, cargo and a Himalaya binary of a version listed in
# src/engine/himalaya-versions.json. The container is reachable on
# 127.0.0.1:PORT only and is always stopped when this script exits.
set -euo pipefail

fail() {
```

In `tests/e2e/run.sh`, replace:

```sh
# Resolve the Himalaya binary before changing directory; the test runs
# mailtriage from a temporary directory, so the path must be absolute.
himalaya=${MT_E2E_HIMALAYA:-}
[ -n "$himalaya" ] || fail "set MT_E2E_HIMALAYA to a Himalaya v2.1.0 binary"
case "$himalaya" in
  */*) ;;
  *) himalaya=$(command -v "$himalaya") || fail "MT_E2E_HIMALAYA not found on PATH: $MT_E2E_HIMALAYA" ;;
esac
[ -f "$himalaya" ] && [ -x "$himalaya" ] || fail "MT_E2E_HIMALAYA is not an executable file: $himalaya"
himalaya="$(cd "$(dirname "$himalaya")" && pwd)/$(basename "$himalaya")"
version=$("$himalaya" --version) || fail "$himalaya --version failed"
version=${version%%$'\n'*}
case "$version" in
  "himalaya v2.1.0 "*) ;;
  *) fail "expected Himalaya v2.1.0, got: $version" ;;
esac

command -v python3 >/dev/null 2>&1 || fail "python3 is required"
command -v cargo >/dev/null 2>&1 || fail "cargo is required"
command -v docker >/dev/null 2>&1 || fail "the docker CLI is required for the Dovecot container"
docker info >/dev/null 2>&1 ||
```

with:

```sh
# Resolve the Himalaya binary before changing directory; the test runs
# mailtriage from a temporary directory, so the path must be absolute.
himalaya=${MT_E2E_HIMALAYA:-}
[ -n "$himalaya" ] || fail "set MT_E2E_HIMALAYA to a tested Himalaya binary"
case "$himalaya" in
  */*) ;;
  *) himalaya=$(command -v "$himalaya") || fail "MT_E2E_HIMALAYA not found on PATH: $MT_E2E_HIMALAYA" ;;
esac
[ -f "$himalaya" ] && [ -x "$himalaya" ] || fail "MT_E2E_HIMALAYA is not an executable file: $himalaya"
himalaya="$(cd "$(dirname "$himalaya")" && pwd)/$(basename "$himalaya")"
command -v python3 >/dev/null 2>&1 || fail "python3 is required"
version=$("$himalaya" --version) || fail "$himalaya --version failed"
version=${version%%$'\n'*}
data="$(cd "$(dirname "$0")/../.." && pwd)/src/engine/himalaya-versions.json"
listed=$(python3 -c 'import json, sys; print(" ".join(v["version"] for v in json.load(open(sys.argv[1]))["versions"]))' "$data") ||
  fail "cannot read $data"
tested=
for v in $listed; do
  case "$version" in "himalaya v$v "*) tested=$v ;; esac
done
[ -n "$tested" ] || fail "expected a tested Himalaya ($listed), got: $version"
command -v cargo >/dev/null 2>&1 || fail "cargo is required"
command -v docker >/dev/null 2>&1 || fail "the docker CLI is required for the Dovecot container"
docker info >/dev/null 2>&1 ||
```

In `.github/workflows/e2e.yml`, replace:

```yaml

env:
  CARGO_TERM_COLOR: always
  # Official Himalaya v2.1.0 release asset, pinned by SHA-256.
  HIMALAYA_URL: https://github.com/pimalaya/himalaya/releases/download/v2.1.0/himalaya.x86_64-linux.tgz
  HIMALAYA_SHA256: 683a2ab8e1534f01e6bda3a69e204d564c31fbfbe20511fc7bc60b67f2e85884

jobs:
  dovecot:
    name: Dovecot (flat and INBOX. prefix)
    runs-on: ubuntu-24.04
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v7.0.1
        with:
```

with:

```yaml

env:
  CARGO_TERM_COLOR: always

jobs:
  versions:
    name: Tested Himalaya versions
    runs-on: ubuntu-24.04
    timeout-minutes: 5
    outputs:
      matrix: ${{ steps.read.outputs.matrix }}
    steps:
      - uses: actions/checkout@v7.0.1
        with:
          persist-credentials: false
      # Every version in the data file, with its x86_64-linux archive digest;
      # this workflow never changes when a version is added.
      - id: read
        run: |
          set -euo pipefail
          matrix="$(python3 .github/scripts/himalaya_matrix.py src/engine/himalaya-versions.json)"
          echo "matrix=$matrix" >> "$GITHUB_OUTPUT"

  dovecot:
    name: Dovecot, Himalaya ${{ matrix.version }} (flat and INBOX. prefix)
    needs: versions
    runs-on: ubuntu-24.04
    timeout-minutes: 30
    strategy:
      fail-fast: false
      matrix:
        include: ${{ fromJSON(needs.versions.outputs.matrix) }}
    steps:
      - uses: actions/checkout@v7.0.1
        with:
```

In `.github/workflows/e2e.yml`, replace:

```yaml
      - uses: Swatinem/rust-cache@v2
        with:
          key: e2e
      - name: Install pinned Himalaya v2.1.0
        run: |
          set -euo pipefail
          dir="$RUNNER_TEMP/himalaya-v2.1.0"
          mkdir -p "$dir"
          cd "$dir"
          curl -fsSL -o himalaya.x86_64-linux.tgz "$HIMALAYA_URL"
          echo "$HIMALAYA_SHA256  himalaya.x86_64-linux.tgz" | sha256sum -c -
          tar -xzf himalaya.x86_64-linux.tgz himalaya
          ./himalaya --version
```

with:

```yaml
      - uses: Swatinem/rust-cache@v2
        with:
          key: e2e
      - name: Install Himalaya ${{ matrix.version }}, pinned by SHA-256
        env:
          HIMALAYA_VERSION: ${{ matrix.version }}
          HIMALAYA_SHA256: ${{ matrix.sha256 }}
        run: |
          set -euo pipefail
          dir="$RUNNER_TEMP/himalaya-v$HIMALAYA_VERSION"
          mkdir -p "$dir"
          cd "$dir"
          curl --proto '=https' --tlsv1.2 -fsSL -o himalaya.x86_64-linux.tgz \
            "https://github.com/pimalaya/himalaya/releases/download/v$HIMALAYA_VERSION/himalaya.x86_64-linux.tgz"
          echo "$HIMALAYA_SHA256  himalaya.x86_64-linux.tgz" | sha256sum -c -
          tar -xzf himalaya.x86_64-linux.tgz himalaya
          ./himalaya --version
```

Create `.github/workflows/himalaya-compat.yml`:

````yaml
name: Himalaya compatibility

# Weekly: test pimalaya/himalaya's newest stable release with the Dovecot
# end-to-end suite. A passing release is proposed as a pull request that adds
# it to src/engine/himalaya-versions.json; a failing one opens or updates an
# issue. The candidate binary only ever runs in the read-only `test` job.

on:
  schedule:
    - cron: '23 5 * * 1'
  workflow_dispatch:

permissions:
  contents: read

concurrency:
  group: himalaya-compat
  cancel-in-progress: false

env:
  CARGO_TERM_COLOR: always

jobs:
  test:
    name: Test the newest Himalaya
    runs-on: ubuntu-24.04
    timeout-minutes: 45
    permissions:
      contents: read
    outputs:
      version: ${{ steps.newest.outputs.version }}
      listed: ${{ steps.newest.outputs.listed }}
    steps:
      - uses: actions/checkout@v7.0.1
        with:
          persist-credentials: false
      - name: Newest stable Himalaya release
        id: newest
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          set -euo pipefail
          tag="$(gh api repos/pimalaya/himalaya/releases/latest --jq .tag_name)"
          version="${tag#v}"
          if ! printf '%s\n' "$version" | grep -Eqx '(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)'; then
            echo "::error::the newest Himalaya release has the tag $tag"
            exit 1
          fi
          listed="$(python3 -c 'import json, sys; print(str(any(v["version"] == sys.argv[2] for v in json.load(open(sys.argv[1]))["versions"])).lower())' src/engine/himalaya-versions.json "$version")"
          echo "version=$version" >> "$GITHUB_OUTPUT"
          echo "listed=$listed" >> "$GITHUB_OUTPUT"
          if [ "$listed" = true ]; then
            echo "::notice::Himalaya $version is already tested"
          fi
      - name: Add the candidate to the data file
        if: steps.newest.outputs.listed == 'false'
        env:
          GH_TOKEN: ${{ github.token }}
          VERSION: ${{ steps.newest.outputs.version }}
        run: bash scripts/add-himalaya-version.sh "$VERSION"
      - uses: dtolnay/rust-toolchain@stable
        if: steps.newest.outputs.listed == 'false'
      - uses: Swatinem/rust-cache@v2
        if: steps.newest.outputs.listed == 'false'
        with:
          key: e2e
      - name: Install the candidate, pinned by its digest
        if: steps.newest.outputs.listed == 'false'
        env:
          VERSION: ${{ steps.newest.outputs.version }}
        run: |
          set -euo pipefail
          digest="$(python3 -c 'import json, sys; print([v for v in json.load(open(sys.argv[1]))["versions"] if v["version"] == sys.argv[2]][0]["assets"]["x86_64-linux"][len("sha256:"):])' src/engine/himalaya-versions.json "$VERSION")"
          dir="$RUNNER_TEMP/himalaya-v$VERSION"
          mkdir -p "$dir"
          cd "$dir"
          curl --proto '=https' --tlsv1.2 -fsSL -o himalaya.x86_64-linux.tgz \
            "https://github.com/pimalaya/himalaya/releases/download/v$VERSION/himalaya.x86_64-linux.tgz"
          echo "$digest  himalaya.x86_64-linux.tgz" | sha256sum -c -
          tar -xzf himalaya.x86_64-linux.tgz himalaya
          ./himalaya --version
          echo "MT_E2E_HIMALAYA=$PWD/himalaya" >> "$GITHUB_ENV"
      - id: build
        if: steps.newest.outputs.listed == 'false'
        run: cargo build --locked
      - name: End to end, flat namespace
        if: steps.newest.outputs.listed == 'false'
        run: |
          set -o pipefail
          bash tests/e2e/run.sh flat 31143 2>&1 | tee "$RUNNER_TEMP/e2e-flat.log"
      - name: End to end, INBOX. prefix namespace
        if: ${{ !cancelled() && steps.build.outcome == 'success' }}
        run: |
          set -o pipefail
          bash tests/e2e/run.sh prefix 31144 2>&1 | tee "$RUNNER_TEMP/e2e-prefix.log"
      - uses: actions/upload-artifact@v7.0.1
        if: steps.newest.outputs.listed == 'false'
        with:
          name: himalaya-versions
          path: src/engine/himalaya-versions.json
          if-no-files-found: error
          retention-days: 7
      - uses: actions/upload-artifact@v7.0.1
        if: failure()
        with:
          name: e2e-logs
          path: ${{ runner.temp }}/e2e-*.log
          if-no-files-found: ignore
          retention-days: 14

  propose:
    name: Propose the candidate
    needs: test
    if: needs.test.outputs.listed == 'false'
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    permissions:
      contents: write
      pull-requests: write
      actions: write
    steps:
      - uses: actions/checkout@v7.0.1
      - uses: actions/download-artifact@v8.0.1
        with:
          name: himalaya-versions
          path: ${{ runner.temp }}/candidate
      - name: Branch, pull request and CI runs
        env:
          GH_TOKEN: ${{ github.token }}
          VERSION: ${{ needs.test.outputs.version }}
          RUN_URL: ${{ github.server_url }}/${{ github.repository }}/actions/runs/${{ github.run_id }}
        run: |
          set -euo pipefail
          branch="himalaya/$VERSION"
          if git ls-remote --exit-code --heads origin "$branch" > /dev/null; then
            echo "::notice::$branch exists already; its pull request is open or was decided"
            exit 0
          fi
          cp "$RUNNER_TEMP/candidate/himalaya-versions.json" src/engine/himalaya-versions.json
          git switch -c "$branch"
          git add src/engine/himalaya-versions.json
          git -c user.name='github-actions[bot]' \
            -c user.email='41898282+github-actions[bot]@users.noreply.github.com' \
            commit -m "Test Himalaya $VERSION"
          git push origin "$branch"
          cat > "$RUNNER_TEMP/body.md" <<BODY
          The weekly check passed the Dovecot end-to-end suite against Himalaya $VERSION in both namespace layouts: $RUN_URL

          This adds $VERSION to \`src/engine/himalaya-versions.json\` with the digests from its release and \`roles\` copied from the previous version. Before merging, read this version's \`--mailbox\` resolver in Himalaya's source (\`Account::resolve_mailbox\`, \`MailboxArg\` and the IMAP backend's mailbox parsing) for role changes; correct \`roles\` and the tests that pin it (\`src/engine/versions.rs\`, \`src/engine/targets.rs\`) if it changed.

          CI and the end-to-end matrix were started on this branch.
          BODY
          gh pr create --base main --head "$branch" --title "Test Himalaya $VERSION" --body-file "$RUNNER_TEMP/body.md"
          gh workflow run ci.yml --ref "$branch"
          gh workflow run e2e.yml --ref "$branch"

  report:
    name: Report the failure
    needs: test
    if: ${{ failure() && needs.test.result == 'failure' }}
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    permissions:
      issues: write
    steps:
      - uses: actions/download-artifact@v8.0.1
        continue-on-error: true
        with:
          name: e2e-logs
          path: ${{ runner.temp }}/logs
      - name: Open or comment on the issue
        env:
          GH_TOKEN: ${{ github.token }}
          GH_REPO: ${{ github.repository }}
          VERSION: ${{ needs.test.outputs.version }}
          RUN_URL: ${{ github.server_url }}/${{ github.repository }}/actions/runs/${{ github.run_id }}
        run: |
          set -euo pipefail
          if [ -z "$VERSION" ]; then
            VERSION="$(gh api repos/pimalaya/himalaya/releases/latest --jq .tag_name)"
            VERSION="${VERSION#v}"
          fi
          title="Himalaya $VERSION fails the end-to-end suite"
          {
            echo "The weekly check failed for Himalaya $VERSION: $RUN_URL"
            echo
            for log in "$RUNNER_TEMP"/logs/e2e-*.log; do
              [ -f "$log" ] || continue
              echo "Last lines of $(basename "$log"):"
              echo
              echo '```text'
              tail -n 60 "$log"
              echo '```'
              echo
            done
          } > "$RUNNER_TEMP/body.md"
          number="$(gh issue list --state open --search "\"$title\" in:title" --json number,title \
            --jq "map(select(.title == \"$title\")) | .[0].number // empty")"
          if [ -n "$number" ]; then
            gh issue comment "$number" --body-file "$RUNNER_TEMP/body.md"
          else
            gh issue create --title "$title" --body-file "$RUNNER_TEMP/body.md"
          fi
````

- [ ] **Step 4: Run the new tests**

Run: `cargo test --locked --test himalaya_scripts`
Expected: PASS (4 tests).

- [ ] **Step 5: Document it**

In `docs/releases.md`, replace:

```markdown
## Publish a version
```

with:

````markdown
## Tested Himalaya versions

`src/engine/himalaya-versions.json` lists the Himalaya versions mailtriage
accepts, with the SHA-256 of each one's release archives; every release
compiles it in. The Dovecot end-to-end workflow (`e2e.yml`) reads it and runs
once per listed version, so the workflow never changes when a version is added.

**The weekly check.** `.github/workflows/himalaya-compat.yml` runs every
Monday and on demand, in three jobs:

1. `test` (read-only) reads pimalaya/himalaya's newest stable release and
   stops when it is listed. Otherwise it adds it with
   `scripts/add-himalaya-version.sh` and runs the Dovecot suite against it in
   both namespace layouts. The candidate binary runs only in this job.
2. `propose`, when the suite passed: pushes the edited file to the branch
   `himalaya/VERSION`, opens the pull request `Test Himalaya VERSION`, and
   starts `ci.yml` and `e2e.yml` on the branch (a branch pushed with the
   workflow's token starts no workflow by itself). When the branch exists
   already, it does nothing.
3. `report`, when the suite failed: opens the issue `Himalaya VERSION fails the
   end-to-end suite`, or comments on the open one, with the end of the
   failing log.

Before merging such a pull request, read the new version's `--mailbox`
resolver in Himalaya's source for role changes: the script copies `roles` from
the previous version. The workflow needs one repository setting, made once:
Settings → Actions → General → Workflow permissions → "Allow GitHub Actions
to create and approve pull requests".

**Adding a version by hand:**

```sh
scripts/add-himalaya-version.sh 2.3.0
```

It reads the release's asset digests from the GitHub API (set `GH_TOKEN` to
avoid the anonymous rate limit), fails without changing anything when a
platform's digest is missing, and appends the entry with the previous
version's `roles`. Check the roles, push, and let `e2e.yml` run the suite for
every listed version.

## Publish a version
````

In `docs/verification.md`, replace:

```markdown
`main`, on pull requests and on demand. It uses the real `mailtriage` binary,
the official Himalaya v2.1.0 Linux x86_64 release (`himalaya.x86_64-linux.tgz`,
SHA-256 `683a2ab8e1534f01e6bda3a69e204d564c31fbfbe20511fc7bc60b67f2e85884`) and
a `dovecot/dovecot:2.3.21` container in two namespace layouts: no prefix with
```

with:

```markdown
`main`, on pull requests and on demand, once for every Himalaya version in
`src/engine/himalaya-versions.json`. It uses the real `mailtriage` binary, the
official Linux x86_64 release of that version (`himalaya.x86_64-linux.tgz`,
pinned by the SHA-256 in the same file) and a `dovecot/dovecot:2.3.21`
container in two namespace layouts: no prefix with
```

In `docs/verification.md`, replace:

```markdown
Docker with a running daemon, Python 3, Cargo and a Himalaya v2.1.0 binary.
```

with:

```markdown
Docker with a running daemon, Python 3, Cargo and a Himalaya binary of a tested version.
```

- [ ] **Step 6: Full check and commit**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`
Expected: all pass.

```bash
git add .github/scripts/himalaya_matrix.py \
  .github/workflows/e2e.yml \
  .github/workflows/himalaya-compat.yml \
  docs/releases.md \
  docs/verification.md \
  scripts/add-himalaya-version.sh \
  tests/e2e/run.sh \
  tests/himalaya_scripts.rs
git commit -m "Test every listed Himalaya version and check the newest weekly

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Homebrew's `opt` path for services, the tray and restarts

For a binary in `<prefix>/Cellar/mailtriage/<version>/bin/` whose `<prefix>/opt/mailtriage/bin/<name>` exists, mailtriage uses that `opt` path as the launch path: `service install` records it, autostart records it for the tray and `--mailtriage`, the tray runs it as its CLI and starts windows from its own, and both restart rules follow a retargeted `opt` (after `brew upgrade`, or when the old keg is gone) and re-execute the `opt` path. The updater still classifies by canonical path (`managed_by_homebrew`).

**Files:**
- Create: `src/distribution/brew.rs`, `tray/src/brew.rs`
- Modify: `src/distribution/mod.rs`, `src/system_service.rs`, `src/update/restart.rs`, `tray/src/lib.rs`, `tray/src/paths.rs`, `tray/src/autostart.rs`, `tray/src/restart.rs`, `tray/src/tray.rs`, `docs/guide.md`
- Test: `tests/brew_paths.rs` (new), `tray/tests/brew.rs` (new)

**Interfaces:**
- Consumes: `update::restart::{Image, Restarter}`, `system_service::unit_for`, tray `paths::resolve_cli`, `autostart::Env::detect`, `restart::Restarter` (on `main`).
- Produces:
  - `distribution::brew::{opt_path(&Path) -> Option<PathBuf>, launch_path(&Path) -> PathBuf}`.
  - `update::restart::Image.launch: Option<PathBuf>`; `Image::watched(&self) -> (PathBuf, bool)` (the file to watch, and whether `opt` was retargeted); `Image::target(&self) -> PathBuf` (the path `exec` runs).
  - Tray: `mailtriage_tray::brew::{opt_path, launch_path, same_installation(&Path, &Path) -> bool}`; `restart::Restarter::launch_path(&self) -> PathBuf`; `Restarter::check` returns the launch path.

- [ ] **Step 1: Write the failing tests**

Create `tests/brew_paths.rs`:

```rust
#![cfg(unix)]
//! A mailtriage in a Homebrew keg: `<prefix>/Cellar/mailtriage/<version>/bin/`
//! with `<prefix>/opt/mailtriage` linked to it, as brew lays it out. The
//! service records the `opt` path, the updater only notifies, and a running
//! `watch` follows `opt` to the new keg after `brew upgrade`. The real
//! binary is copied into the fake kegs; launchctl and systemctl are fakes.
mod common;
mod update_support;
use common::{write_tool, LAUNCHCTL, SYSTEMCTL};
use mailtriage::{system_service::Manager, update::service_files};
use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
};
use update_support::{run, Sandbox, Server};

/// Copies the binary under test into the keg of `version` under `prefix`.
fn keg(prefix: &Path, version: &str) -> PathBuf {
    let bin = prefix.join("Cellar/mailtriage").join(version).join("bin");
    fs::create_dir_all(&bin).unwrap();
    let exe = bin.join("mailtriage");
    fs::copy(env!("CARGO_BIN_EXE_mailtriage"), &exe).unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
    exe
}

/// Points `<prefix>/opt/mailtriage` at the keg of `version`; returns the
/// `opt` path of the binary.
fn link_opt(prefix: &Path, version: &str) -> PathBuf {
    fs::create_dir_all(prefix.join("opt")).unwrap();
    let opt = prefix.join("opt/mailtriage");
    let _ = fs::remove_file(&opt);
    symlink(prefix.join("Cellar/mailtriage").join(version), &opt).unwrap();
    opt.join("bin/mailtriage")
}

fn manager() -> Manager {
    service_files::platform_manager().expect("macOS or Linux")
}

#[test]
fn service_install_records_the_opt_path() {
    let sandbox = Sandbox::new();
    let root = sandbox.root();
    let prefix = root.join("brew");
    let keg_exe = keg(&prefix, "1.2.3");
    let opt_exe = link_opt(&prefix, "1.2.3");
    let tools = root.join("tools");
    fs::create_dir_all(&tools).unwrap();
    write_tool(&tools, "launchctl", LAUNCHCTL);
    write_tool(&tools, "systemctl", SYSTEMCTL);
    let config = sandbox.config("cfg", "off");
    for started_as in [&opt_exe, &keg_exe] {
        let (code, out, err) = run(std::process::Command::new(started_as)
            .args([
                "service",
                "install",
                "--account",
                "work",
                "--json",
                "--config",
            ])
            .arg(&config)
            .env("HOME", &sandbox.home)
            .env("XDG_CACHE_HOME", &sandbox.xdg)
            .env("MT_FAKE_HOME", &sandbox.home)
            .env("PATH", format!("{}:/usr/bin:/bin", tools.display()))
            .env_remove("MAILTRIAGE_CONFIG"));
        assert_eq!(code, Some(0), "{out} {err}");
        let files = service_files::list(manager(), &sandbox.home);
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].executable.as_deref(),
            Some(opt_exe.as_path()),
            "started as {}",
            started_as.display()
        );
    }
}

#[test]
fn the_updater_still_only_notifies_for_a_keg() {
    let sandbox = Sandbox::new();
    let prefix = sandbox.root().join("brew");
    let keg_exe = keg(&prefix, "1.2.3");
    let opt_exe = link_opt(&prefix, "1.2.3");
    let server = Server::start();
    server.list(&[]);
    let (code, out, err) = run(std::process::Command::new(&opt_exe)
        .args(["update", "--check", "--json"])
        .env("HOME", &sandbox.home)
        .env("XDG_CACHE_HOME", &sandbox.xdg)
        .env("MAILTRIAGE_UPDATE_URL", &server.base)
        .env_remove("MAILTRIAGE_CONFIG"));
    assert_eq!(code, Some(0), "{out} {err}");
    let install = &out["update"]["install"];
    assert_eq!(install["reason"], "managed_by_homebrew");
    // The cache key stays the canonical keg path.
    assert_eq!(install["path"], keg_exe.to_str().unwrap());
}

#[test]
fn watch_follows_opt_to_the_new_keg_when_the_old_one_is_deleted() {
    let sandbox = Sandbox::new();
    let prefix = sandbox.root().join("brew");
    keg(&prefix, "1.2.3");
    let opt_exe = link_opt(&prefix, "1.2.3");
    let server = Server::start();
    let config = sandbox.config("cfg", "off");
    let mut watch = update_support::Watch::spawn(
        std::process::Command::new(&opt_exe)
            .args([
                "watch",
                "--account",
                "work",
                "--interval-seconds",
                "1",
                "--json",
            ])
            .current_dir(sandbox.root())
            .env("HOME", &sandbox.home)
            .env("XDG_CACHE_HOME", &sandbox.xdg)
            .env("MAILTRIAGE_UPDATE_URL", &server.base)
            .env("MAILTRIAGE_CONFIG", &config)
            .env("PATH", "/usr/bin:/bin"),
    );
    watch.wait_passes(1);
    // `brew upgrade`: a new keg, `opt` retargeted, the old keg deleted.
    keg(&prefix, "1.2.4");
    link_opt(&prefix, "1.2.4");
    fs::remove_dir_all(prefix.join("Cellar/mailtriage/1.2.3")).unwrap();
    let restarting = watch.event("restarting");
    assert_eq!(restarting["update"]["pid"], watch.child.id());
    let before = watch.passes();
    watch.wait_passes(before + 1);
    assert!(watch.events("error").is_empty(), "{:#?}", watch.seen);
    assert_eq!(watch.events("restarting").len(), 1, "{:#?}", watch.seen);
    watch.stop();
}
```

Create `tray/tests/brew.rs`:

```rust
//! A tray in a Homebrew keg (`<prefix>/Cellar/mailtriage/<version>/bin/`,
//! `<prefix>/opt/mailtriage` linked to it): it runs, records and
//! re-executes `opt` paths, which outlive `brew upgrade`.
mod support;
use mailtriage_tray::{
    instances::Windows,
    paths::{self, Env, Resolved},
    restart::{self, Restarter},
};
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};
use support::{write_script, AUTOSTART_LAUNCHCTL};

/// `<prefix>/Cellar/mailtriage/<version>/bin` with a `mailtriage` script and
/// a `mailtriage-tray` script printing that version.
fn keg(prefix: &Path, version: &str) -> PathBuf {
    let bin = prefix.join("Cellar/mailtriage").join(version).join("bin");
    fs::create_dir_all(&bin).unwrap();
    write_script(
        &bin.join("mailtriage"),
        &format!("#!/bin/sh\necho 'mailtriage {version}'\n"),
    );
    write_script(
        &bin.join("mailtriage-tray"),
        &format!("#!/bin/sh\necho 'mailtriage-tray {version}'\n"),
    );
    bin
}

fn link_opt(prefix: &Path, version: &str) -> PathBuf {
    fs::create_dir_all(prefix.join("opt")).unwrap();
    let opt = prefix.join("opt/mailtriage");
    let _ = fs::remove_file(&opt);
    symlink(prefix.join("Cellar/mailtriage").join(version), &opt).unwrap();
    opt.join("bin")
}

fn root() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    (dir, root)
}

#[test]
fn the_cli_path_is_the_opt_path_however_it_was_found() {
    let (_dir, root) = root();
    let bin = keg(&root, "1.2.3");
    let opt = link_opt(&root, "1.2.3");
    let env = Env {
        cwd: root.clone(),
        own_exe: Some(bin.join("mailtriage-tray")),
        ..Env::default()
    };
    let expected = opt.join("mailtriage");
    assert_eq!(
        paths::resolve_cli(Some(&bin.join("mailtriage")), &env),
        Ok(expected.clone())
    );
    assert_eq!(paths::resolve_cli(None, &env), Ok(expected.clone()));
    let on_path = Env {
        cwd: root.clone(),
        path: Some(bin.clone().into_os_string()),
        ..Env::default()
    };
    assert_eq!(paths::resolve_cli(None, &on_path), Ok(expected.clone()));
    // The restart passes it on as resolved.
    let resolved = Resolved {
        cli: expected.clone(),
        config: root.join("mailtriage.json"),
    };
    let command = restart::command(&opt.join("mailtriage-tray"), &resolved, &Windows::default());
    let args: Vec<_> = command.get_args().map(|a| a.to_owned()).collect();
    assert!(args.contains(&expected.into_os_string()));
}

#[test]
fn a_tray_in_a_keg_follows_opt_to_the_new_keg() {
    let (_dir, root) = root();
    let old = keg(&root, "1.2.3").join("mailtriage-tray");
    let opt = link_opt(&root, "1.2.3").join("mailtriage-tray");
    let mut r = Restarter::new(old.clone(), restart::identity(&old).unwrap());
    assert_eq!(r.launch_path(), opt);
    let t = Instant::now();
    assert_eq!(r.check(t), None);
    keg(&root, "1.2.4");
    link_opt(&root, "1.2.4");
    fs::remove_dir_all(root.join("Cellar/mailtriage/1.2.3")).unwrap();
    assert_eq!(r.check(t), Some(opt));
}

#[test]
fn start_at_login_records_the_opt_paths() {
    let (_dir, root) = root();
    let bin = keg(&root, "1.2.3");
    // The real tray binary in the keg, next to the fake CLI.
    fs::copy(
        env!("CARGO_BIN_EXE_mailtriage-tray"),
        bin.join("mailtriage-tray"),
    )
    .unwrap();
    let opt = link_opt(&root, "1.2.3");
    write_script(&root.join("launchctl"), AUTOSTART_LAUNCHCTL);
    fs::write(root.join("mailtriage.json"), "{}").unwrap();
    let out = Command::new(bin.join("mailtriage-tray"))
        .args(["autostart", "enable", "--json", "--config"])
        .arg(root.join("mailtriage.json"))
        .arg("--mailtriage")
        .arg(bin.join("mailtriage"))
        .current_dir(&root)
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("xdg"))
        .env("PATH", format!("{}:/usr/bin:/bin", root.display()))
        .env_remove("MAILTRIAGE_CONFIG")
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    assert_eq!(out.status.code(), Some(0), "{v}");
    assert_eq!(
        v["autostart"]["mailtriage"],
        opt.join("mailtriage").to_str().unwrap()
    );
    let item = fs::read_to_string(v["autostart"]["path"].as_str().unwrap()).unwrap();
    let tray = opt.join("mailtriage-tray");
    assert!(item.contains(tray.to_str().unwrap()), "{item}");
    assert!(!item.contains("/Cellar/"), "{item}");
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test --locked --test brew_paths; cargo test --locked -p mailtriage-tray --test brew`
Expected: `brew_paths` FAILS 2 of 3 tests: `service_install_records_the_opt_path` finds the keg path where it expects `…/opt/mailtriage/bin/mailtriage`, and `watch_follows_opt_to_the_new_keg_when_the_old_one_is_deleted` panics with `watch never showed restarting` after about a minute. (`the_updater_still_only_notifies_for_a_keg` already passes; it pins behaviour that must not change.) `tray/tests/brew.rs` FAILS to compile: E0599, no method named `launch_path` for `Restarter`.

- [ ] **Step 3: Implement**

`src/update/restart.rs` gains the unit test `a_keg_follows_its_opt_path_when_brew_upgrade_retargets_it`; `brew.rs` in both crates carries unit tests with the same cases.

Create `src/distribution/brew.rs`:

```rust
//! Homebrew kegs. Brew keeps each version in
//! `<prefix>/Cellar/mailtriage/<version>/bin/`, and `brew upgrade` deletes
//! the old keg; `<prefix>/opt/mailtriage` always points to the current one.
//! For a program in a keg, mailtriage records and re-executes that `opt`
//! path (the launch path) instead of the keg path.
use std::path::{Path, PathBuf};

/// `<prefix>/opt/mailtriage/bin/<name>` for a path shaped
/// `<prefix>/Cellar/mailtriage/<version>/bin/<name>`, by its shape alone.
pub fn opt_path(canonical: &Path) -> Option<PathBuf> {
    let name = canonical.file_name()?;
    let bin = canonical.parent()?;
    let keg = bin.parent()?;
    let formula = keg.parent()?;
    let cellar = formula.parent()?;
    let shaped = bin.file_name()? == "bin"
        && keg.file_name().is_some()
        && formula.file_name()? == "mailtriage"
        && cellar.file_name()? == "Cellar";
    if !shaped {
        return None;
    }
    Some(cellar.parent()?.join("opt/mailtriage/bin").join(name))
}

/// The launch path of the program whose canonical path is `canonical`: its
/// `opt` path when it has one and that exists, else `canonical` itself.
pub fn launch_path(canonical: &Path) -> PathBuf {
    opt_path(canonical)
        .filter(|opt| opt.exists())
        .unwrap_or_else(|| canonical.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn a_keg_path_has_an_opt_path() {
        assert_eq!(
            opt_path(Path::new(
                "/opt/homebrew/Cellar/mailtriage/1.2.3/bin/mailtriage"
            )),
            Some(PathBuf::from("/opt/homebrew/opt/mailtriage/bin/mailtriage"))
        );
        assert_eq!(
            opt_path(Path::new(
                "/home/linuxbrew/.linuxbrew/Cellar/mailtriage/1.2.3_1/bin/mailtriage-tray"
            )),
            Some(PathBuf::from(
                "/home/linuxbrew/.linuxbrew/opt/mailtriage/bin/mailtriage-tray"
            ))
        );
        for plain in [
            "/Users/a/.local/bin/mailtriage",
            "/opt/homebrew/Cellar/himalaya/2.2.1/bin/himalaya",
            "/opt/homebrew/Cellar/mailtriage/1.2.3/libexec/mailtriage",
            "/Cellar/mailtriage/bin/mailtriage",
            "mailtriage",
        ] {
            assert_eq!(opt_path(Path::new(plain)), None, "{plain}");
        }
    }

    #[test]
    fn the_launch_path_is_the_opt_path_only_when_it_exists() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let keg = root.join("Cellar/mailtriage/1.2.3/bin/mailtriage");
        fs::create_dir_all(keg.parent().unwrap()).unwrap();
        fs::write(&keg, "").unwrap();
        assert_eq!(launch_path(&keg), keg);
        fs::create_dir_all(root.join("opt")).unwrap();
        std::os::unix::fs::symlink(
            root.join("Cellar/mailtriage/1.2.3"),
            root.join("opt/mailtriage"),
        )
        .unwrap();
        assert_eq!(
            launch_path(&keg),
            root.join("opt/mailtriage/bin/mailtriage")
        );
        let plain = root.join("bin/mailtriage");
        assert_eq!(launch_path(&plain), plain);
    }
}
```

In `src/distribution/mod.rs`, replace:

```rust
//! Installing mailtriage and what it needs (spec:
//! docs/superpowers/specs/2026-10-07-install-and-distribution-design.md):
//! the protected path rule and a private, tested Himalaya.
pub mod himalaya;
pub mod protected;
```

with:

```rust
//! Installing mailtriage and what it needs (spec:
//! docs/superpowers/specs/2026-10-07-install-and-distribution-design.md):
//! the protected path rule and a private, tested Himalaya.
pub mod brew;
pub mod himalaya;
pub mod protected;
```

In `src/system_service.rs`, replace:

```rust
        .map_or_else(|| PathBuf::from("logs"), |dir| dir.join("logs"));
    Ok(Unit {
        account: account.to_owned(),
        exe: std::env::current_exe()?,
        config,
        interval_seconds,
        limit,
```

with:

```rust
        .map_or_else(|| PathBuf::from("logs"), |dir| dir.join("logs"));
    Ok(Unit {
        account: account.to_owned(),
        exe: launch_exe()?,
        config,
        interval_seconds,
        limit,
```

In `src/system_service.rs`, replace:

```rust
    })
}

/// `service install` for an account of `service`'s config, with a note
/// when the key comes from an environment variable the service lacks.
pub fn install_account(
```

with:

```rust
    })
}

/// The executable a service runs: this program, or, for a Homebrew keg,
/// its `opt` path, which `brew upgrade` keeps pointing at the current keg.
fn launch_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let opt = fs::canonicalize(&exe)
        .ok()
        .and_then(|canonical| crate::distribution::brew::opt_path(&canonical))
        .filter(|opt| opt.exists());
    Ok(opt.unwrap_or(exe))
}

/// `service install` for an account of `service`'s config, with a note
/// when the key comes from an environment variable the service lacks.
pub fn install_account(
```

In `src/update/restart.rs`, replace:

```rust
//! Restarting `watch` onto a replaced binary: the installation path and
//! the identity of the running image are recorded at start; between
//! passes a changed file is probed and re-executed with the original
//! arguments and environment, keeping the PID.
use super::{
    events,
    install::Hooks,
    platform::{self, FileIdentity},
    version, CLI,
};
use serde_json::{json, Value};
use std::{
    io::Write,
```

with:

```rust
//! Restarting `watch` onto a replaced binary: the installation path and
//! the identity of the running image are recorded at start; between
//! passes a changed file is probed and re-executed with the original
//! arguments and environment, keeping the PID. A binary in a Homebrew keg
//! also follows its `opt` path, which `brew upgrade` retargets to a new keg.
use super::{
    events,
    install::Hooks,
    platform::{self, FileIdentity},
    version, CLI,
};
use crate::distribution::brew;
use serde_json::{json, Value};
use std::{
    io::Write,
```

In `src/update/restart.rs`, replace:

```rust
    /// The file was replaced between launch and the recording: restart
    /// before the first pass.
    pub replaced_at_start: bool,
}

impl Image {
    /// Records the installation path and the image identity. On Linux the
    /// image is `/proc/self/exe`, which follows the loaded file even after
    /// it was replaced. On macOS it is the installation path at start, plus
```

with:

```rust
    /// The file was replaced between launch and the recording: restart
    /// before the first pass.
    pub replaced_at_start: bool,
    /// For a Homebrew keg, its `opt` path: followed when it leads to
    /// another file than `path`, and always the path re-executed.
    pub launch: Option<PathBuf>,
}

impl Image {
    /// The file to watch: the `opt` path when it leads to another file than
    /// the running one (after `brew upgrade`, or when the old keg is gone),
    /// else the installation path; and whether it was retargeted.
    pub fn watched(&self) -> (PathBuf, bool) {
        match &self.launch {
            Some(opt) if std::fs::canonicalize(opt).ok().as_deref() != Some(&*self.path) => {
                (opt.clone(), true)
            }
            _ => (self.path.clone(), false),
        }
    }

    /// The path `exec` runs: the `opt` path of a keg, else the installation
    /// path.
    pub fn target(&self) -> PathBuf {
        self.launch.clone().unwrap_or_else(|| self.path.clone())
    }

    /// Records the installation path and the image identity. On Linux the
    /// image is `/proc/self/exe`, which follows the loaded file even after
    /// it was replaced. On macOS it is the installation path at start, plus
```

In `src/update/restart.rs`, replace:

```rust
                path.display()
            )
        })?;
        let replaced_at_start = if cfg!(target_os = "linux") {
            FileIdentity::read(&path).ok() != Some(identity)
        } else {
            platform::probe(&path, CLI).is_ok_and(|found| found.to_string() != version::RUNNING)
        };
        Ok(Self {
            path,
            identity,
            replaced_at_start,
        })
    }
}
```

with:

```rust
                path.display()
            )
        })?;
        let launch = brew::opt_path(&path).filter(|opt| opt.exists());
        let retargeted = launch
            .as_deref()
            .is_some_and(|opt| std::fs::canonicalize(opt).ok().as_deref() != Some(&*path));
        let replaced_at_start = retargeted
            || if cfg!(target_os = "linux") {
                FileIdentity::read(&path).ok() != Some(identity)
            } else {
                platform::probe(&path, CLI).is_ok_and(|found| found.to_string() != version::RUNNING)
            };
        Ok(Self {
            path,
            identity,
            replaced_at_start,
            launch,
        })
    }
}
```

In `src/update/restart.rs`, replace:

```rust
    pub fn check(&mut self, stopped: &dyn Fn() -> bool, hooks: &dyn Hooks) {
        let decision = self.decide(stopped);
        let Some(image) = &self.image else { return };
        let path = image.path.clone();
        match decision {
            Decision::Stay | Decision::Stopped => {}
            Decision::Failed(kind, message) => self.fail(kind, &message),
```

with:

```rust
    pub fn check(&mut self, stopped: &dyn Fn() -> bool, hooks: &dyn Hooks) {
        let decision = self.decide(stopped);
        let Some(image) = &self.image else { return };
        let path = image.target();
        match decision {
            Decision::Stay | Decision::Stopped => {}
            Decision::Failed(kind, message) => self.fail(kind, &message),
```

In `src/update/restart.rs`, replace:

```rust
        let Some(image) = &self.image else {
            return Decision::Stay;
        };
        let current = FileIdentity::read(&image.path).ok();
        if current == Some(image.identity) && !image.replaced_at_start {
            return Decision::Stay;
        }
        if let Some((identity, retry_at)) = self.failed {
```

with:

```rust
        let Some(image) = &self.image else {
            return Decision::Stay;
        };
        let (file, retargeted) = image.watched();
        let current = FileIdentity::read(&file).ok();
        if current == Some(image.identity) && !image.replaced_at_start && !retargeted {
            return Decision::Stay;
        }
        if let Some((identity, retry_at)) = self.failed {
```

In `src/update/restart.rs`, replace:

```rust
            }
        }
        let Some(probed) = current else {
            return Decision::Failed(
                Failure::Missing,
                format!("{} is missing", image.path.display()),
            );
        };
        let to = match platform::probe(&image.path, CLI) {
            Ok(to) => to,
            Err(cause) => {
                return Decision::Failed(
                    Failure::Probe,
                    format!(
                        "the replaced binary at {} does not run: {cause}",
                        image.path.display()
                    ),
                )
            }
        };
        if FileIdentity::read(&image.path).ok() != Some(probed) {
            // Replaced again while probing: the next check probes the new file.
            return Decision::Stay;
        }
```

with:

```rust
            }
        }
        let Some(probed) = current else {
            return Decision::Failed(Failure::Missing, format!("{} is missing", file.display()));
        };
        let to = match platform::probe(&file, CLI) {
            Ok(to) => to,
            Err(cause) => {
                return Decision::Failed(
                    Failure::Probe,
                    format!(
                        "the replaced binary at {} does not run: {cause}",
                        file.display()
                    ),
                )
            }
        };
        if FileIdentity::read(&file).ok() != Some(probed) {
            // Replaced again while probing: the next check probes the new file.
            return Decision::Stay;
        }
```

In `src/update/restart.rs`, replace:

```rust
    /// retry: 1 min, doubling up to 1 h; at once for another identity.
    fn fail(&mut self, kind: Failure, message: &str) {
        let Some(image) = &self.image else { return };
        let current = FileIdentity::read(&image.path).ok();
        if !self.reported.contains(&(current, kind)) {
            self.reported.push((current, kind));
            (self.emit)(&events::error(message));
```

with:

```rust
    /// retry: 1 min, doubling up to 1 h; at once for another identity.
    fn fail(&mut self, kind: Failure, message: &str) {
        let Some(image) = &self.image else { return };
        let current = FileIdentity::read(&image.watched().0).ok();
        if !self.reported.contains(&(current, kind)) {
            self.reported.push((current, kind));
            (self.emit)(&events::error(message));
```

In `src/update/restart.rs`, replace:

```rust
            path: path.to_path_buf(),
            identity: FileIdentity::read(path).unwrap(),
            replaced_at_start: false,
        };
        let (emit, seen) = collector();
        (Restarter::new(Ok(image), emit), seen)
```

with:

```rust
            path: path.to_path_buf(),
            identity: FileIdentity::read(path).unwrap(),
            replaced_at_start: false,
            launch: None,
        };
        let (emit, seen) = collector();
        (Restarter::new(Ok(image), emit), seen)
```

In `src/update/restart.rs`, replace:

```rust
        assert!(r.image().is_none());
        assert_eq!(*seen.borrow(), vec![events::error("no image")]);
    }
}
```

with:

```rust
        assert!(r.image().is_none());
        assert_eq!(*seen.borrow(), vec![events::error("no image")]);
    }

    /// `<root>/Cellar/mailtriage/<version>/bin/mailtriage`, printing that
    /// version.
    fn keg(root: &Path, version: &str) -> std::path::PathBuf {
        let bin = root.join("Cellar/mailtriage").join(version).join("bin");
        fs::create_dir_all(&bin).unwrap();
        script(&bin.join("mailtriage"), version, 0o755);
        bin.join("mailtriage")
    }

    /// Points `<root>/opt/mailtriage` at the keg of `version`, as brew does.
    fn link_opt(root: &Path, version: &str) {
        fs::create_dir_all(root.join("opt")).unwrap();
        let opt = root.join("opt/mailtriage");
        let _ = fs::remove_file(&opt);
        std::os::unix::fs::symlink(root.join("Cellar/mailtriage").join(version), opt).unwrap();
    }

    #[test]
    fn a_keg_follows_its_opt_path_when_brew_upgrade_retargets_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let old = keg(&root, "0.1.0");
        link_opt(&root, "0.1.0");
        let opt = root.join("opt/mailtriage/bin/mailtriage");
        let image = Image {
            path: old.clone(),
            identity: FileIdentity::read(&old).unwrap(),
            replaced_at_start: false,
            launch: Some(opt.clone()),
        };
        // The re-exec target is always the opt path.
        assert_eq!(image.target(), opt);
        let (emit, _) = collector();
        let mut r = Restarter::new(Ok(image), emit);
        assert_eq!(r.decide(&|| false), Decision::Stay);
        keg(&root, "0.2.0");
        link_opt(&root, "0.2.0");
        let upgraded = Decision::Exec(semver::Version::new(0, 2, 0));
        assert_eq!(r.decide(&|| false), upgraded);
        // The old keg is gone too: still the new one.
        fs::remove_dir_all(root.join("Cellar/mailtriage/0.1.0")).unwrap();
        assert_eq!(r.decide(&|| false), upgraded);
    }
}
```

Create `tray/src/brew.rs`:

```rust
//! Homebrew kegs, as the CLI's `distribution::brew` sees them (the tray
//! does not depend on the CLI crate; both are tested with the same cases).
//! Brew keeps each version in `<prefix>/Cellar/mailtriage/<version>/bin/`,
//! `brew upgrade` deletes the old keg, and `<prefix>/opt/mailtriage` points
//! to the current one. The tray records, runs and re-executes `opt` paths.
use std::path::{Path, PathBuf};

/// `<prefix>/opt/mailtriage/bin/<name>` for a path shaped
/// `<prefix>/Cellar/mailtriage/<version>/bin/<name>`, by its shape alone.
pub fn opt_path(canonical: &Path) -> Option<PathBuf> {
    let name = canonical.file_name()?;
    let bin = canonical.parent()?;
    let keg = bin.parent()?;
    let formula = keg.parent()?;
    let cellar = formula.parent()?;
    let shaped = bin.file_name()? == "bin"
        && keg.file_name().is_some()
        && formula.file_name()? == "mailtriage"
        && cellar.file_name()? == "Cellar";
    if !shaped {
        return None;
    }
    Some(cellar.parent()?.join("opt/mailtriage/bin").join(name))
}

/// The launch path of the program whose canonical path is `canonical`: its
/// `opt` path when it has one and that exists, else `canonical` itself.
pub fn launch_path(canonical: &Path) -> PathBuf {
    opt_path(canonical)
        .filter(|opt| opt.exists())
        .unwrap_or_else(|| canonical.to_path_buf())
}

/// Whether two canonical paths are one installation: equal, or both in a
/// keg of the same prefix (one may be a deleted older keg).
pub fn same_installation(a: &Path, b: &Path) -> bool {
    a == b || opt_path(a).is_some_and(|opt| opt_path(b) == Some(opt))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_keg_path_has_an_opt_path() {
        assert_eq!(
            opt_path(Path::new(
                "/opt/homebrew/Cellar/mailtriage/1.2.3/bin/mailtriage-tray"
            )),
            Some(PathBuf::from(
                "/opt/homebrew/opt/mailtriage/bin/mailtriage-tray"
            ))
        );
        for plain in [
            "/Users/a/.local/bin/mailtriage-tray",
            "/opt/homebrew/Cellar/himalaya/2.2.1/bin/himalaya",
            "/opt/homebrew/Cellar/mailtriage/1.2.3/libexec/mailtriage-tray",
            "/Cellar/mailtriage/bin/mailtriage-tray",
        ] {
            assert_eq!(opt_path(Path::new(plain)), None, "{plain}");
        }
    }

    #[test]
    fn kegs_of_one_prefix_are_one_installation() {
        let old = Path::new("/opt/homebrew/Cellar/mailtriage/1.2.3/bin/mailtriage-tray");
        let new = Path::new("/opt/homebrew/Cellar/mailtriage/1.2.4/bin/mailtriage-tray");
        let other = Path::new("/usr/local/Cellar/mailtriage/1.2.4/bin/mailtriage-tray");
        let plain = Path::new("/Users/a/.local/bin/mailtriage-tray");
        assert!(same_installation(old, new));
        assert!(same_installation(plain, plain));
        assert!(!same_installation(old, other));
        assert!(!same_installation(
            plain,
            Path::new("/Users/b/.local/bin/mailtriage-tray")
        ));
        assert!(!same_installation(old, plain));
    }
}
```

In `tray/src/lib.rs`, replace:

```rust
//! it and send actions back.
pub mod args;
pub mod autostart;
pub mod cli;
pub mod controller;
pub mod editor;
```

with:

```rust
//! it and send actions back.
pub mod args;
pub mod autostart;
pub mod brew;
pub mod cli;
pub mod controller;
pub mod editor;
```

In `tray/src/paths.rs`, replace:

```rust
//! Which `mailtriage` and which config: resolved once at start, made
//! absolute and canonical, and passed explicitly to every command, window
//! and login item.
use crate::cli::{self, Cli, Failure, Request};
use std::{
    ffi::{OsStr, OsString},
    io,
```

with:

```rust
//! Which `mailtriage` and which config: resolved once at start, made
//! absolute and canonical, and passed explicitly to every command, window
//! and login item.
use crate::{
    brew,
    cli::{self, Cli, Failure, Request},
};
use std::{
    ffi::{OsStr, OsString},
    io,
```

In `tray/src/paths.rs`, replace:

```rust
}

/// `--mailtriage` when given, else `mailtriage` next to this program's
/// canonical path, else the first `mailtriage` on `PATH`; canonicalized.
/// On failure, the places looked at.
pub fn resolve_cli(flag: Option<&Path>, env: &Env) -> Result<PathBuf, Vec<String>> {
    if let Some(flag) = flag {
        return canonical(flag, &env.cwd)
            .ok()
            .filter(|p| is_executable(p))
            .ok_or_else(|| vec![flag.display().to_string()]);
    }
    let mut looked = vec![];
    if let Some(dir) = env.own_exe.as_deref().and_then(Path::parent) {
        let next = dir.join("mailtriage");
        if is_executable(&next) {
            return canonical(&next, &env.cwd).map_err(|_| vec![next.display().to_string()]);
        }
        looked.push(next.display().to_string());
    }
```

with:

```rust
}

/// `--mailtriage` when given, else `mailtriage` next to this program's
/// canonical path, else the first `mailtriage` on `PATH`; canonicalized,
/// and for a Homebrew keg its `opt` path, which outlives `brew upgrade`.
/// On failure, the places looked at.
pub fn resolve_cli(flag: Option<&Path>, env: &Env) -> Result<PathBuf, Vec<String>> {
    if let Some(flag) = flag {
        return canonical(flag, &env.cwd)
            .ok()
            .filter(|p| is_executable(p))
            .map(|p| brew::launch_path(&p))
            .ok_or_else(|| vec![flag.display().to_string()]);
    }
    let mut looked = vec![];
    if let Some(dir) = env.own_exe.as_deref().and_then(Path::parent) {
        let next = dir.join("mailtriage");
        if is_executable(&next) {
            return canonical(&next, &env.cwd)
                .map(|p| brew::launch_path(&p))
                .map_err(|_| vec![next.display().to_string()]);
        }
        looked.push(next.display().to_string());
    }
```

In `tray/src/paths.rs`, replace:

```rust
            let candidate = dir.join("mailtriage");
            if is_executable(&candidate) {
                if let Ok(found) = canonical(&candidate, &env.cwd) {
                    return Ok(found);
                }
            }
        }
```

with:

```rust
            let candidate = dir.join("mailtriage");
            if is_executable(&candidate) {
                if let Ok(found) = canonical(&candidate, &env.cwd) {
                    return Ok(brew::launch_path(&found));
                }
            }
        }
```

In `tray/src/autostart.rs`, replace:

```rust
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| error(3, "HOME is not set"))?;
        let tray = std::env::current_exe()
            .and_then(fs::canonicalize)
            .map_err(|e| error(3, format!("cannot find this program: {e}")))?;
        let launchctl = std::env::var_os("PATH")
            .and_then(|path| {
```

with:

```rust
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| error(3, "HOME is not set"))?;
        // A Homebrew keg records its `opt` path, which `brew upgrade` keeps.
        let tray = std::env::current_exe()
            .and_then(fs::canonicalize)
            .map(|p| crate::brew::launch_path(&p))
            .map_err(|e| error(3, format!("cannot find this program: {e}")))?;
        let launchctl = std::env::var_os("PATH")
            .and_then(|path| {
```

In `tray/src/restart.rs`, replace:

```rust
//! The update spec's restart rule for the tray process: when the tray's
//! file is replaced by one that runs, re-execute it with the paths resolved
//! at start, never the original arguments.
use crate::{instances::Windows, paths::Resolved};
use std::{
    io,
```

with:

```rust
//! The update spec's restart rule for the tray process: when the tray's
//! file is replaced by one that runs, re-execute it with the paths resolved
//! at start, never the original arguments. A tray in a Homebrew keg also
//! follows its `opt` path, which `brew upgrade` retargets to a new keg, and
//! always re-executes that path.
use crate::{instances::Windows, paths::Resolved};
use std::{
    io,
```

In `tray/src/restart.rs`, replace:

```rust
pub struct Restarter {
    path: PathBuf,
    image: Identity,
    /// Restart at the first check (macOS: the file printed another version
    /// at start).
    at_once: bool,
```

with:

```rust
pub struct Restarter {
    path: PathBuf,
    image: Identity,
    /// For a Homebrew keg, its `opt` path.
    launch: Option<PathBuf>,
    /// Restart at the first check (macOS: the file printed another version
    /// at start).
    at_once: bool,
```

In `tray/src/restart.rs`, replace:

```rust

impl Restarter {
    pub fn new(path: PathBuf, image: Identity) -> Self {
        Self {
            path,
            image,
            at_once: false,
            failures: 0,
            retry_at: None,
```

with:

```rust

impl Restarter {
    pub fn new(path: PathBuf, image: Identity) -> Self {
        let launch = crate::brew::opt_path(&path).filter(|opt| opt.exists());
        Self {
            path,
            image,
            launch,
            at_once: false,
            failures: 0,
            retry_at: None,
```

In `tray/src/restart.rs`, replace:

```rust
        &self.path
    }

    /// The path to re-execute when the file changed and the new one runs;
    /// after a failure it waits 1 minute, doubling up to 1 hour, unless the
    /// file changes again.
    pub fn check(&mut self, now: Instant) -> Option<PathBuf> {
        let current = identity(&self.path).ok();
        if current == Some(self.image) && !self.at_once {
            return None;
        }
        if self.retry_at.is_some_and(|at| now < at) && current == self.failed {
            return None;
        }
        let ready =
            current.is_some() && probe(&self.path).is_ok() && identity(&self.path).ok() == current;
        if ready {
            return Some(self.path.clone());
        }
        self.fail(now, current);
        None
```

with:

```rust
        &self.path
    }

    /// The path the tray starts windows from and re-executes: the `opt`
    /// path of a keg, else its own path.
    pub fn launch_path(&self) -> PathBuf {
        self.launch.clone().unwrap_or_else(|| self.path.clone())
    }

    /// The file to watch, and whether a keg's `opt` path now leads to
    /// another file than the running one.
    fn watched(&self) -> (PathBuf, bool) {
        match &self.launch {
            Some(opt) if std::fs::canonicalize(opt).ok().as_deref() != Some(&*self.path) => {
                (opt.clone(), true)
            }
            _ => (self.path.clone(), false),
        }
    }

    /// The path to re-execute when the file changed (or a keg's `opt` was
    /// retargeted) and the new one runs; after a failure it waits 1 minute,
    /// doubling up to 1 hour, unless the file changes again.
    pub fn check(&mut self, now: Instant) -> Option<PathBuf> {
        let (file, retargeted) = self.watched();
        let current = identity(&file).ok();
        if current == Some(self.image) && !self.at_once && !retargeted {
            return None;
        }
        if self.retry_at.is_some_and(|at| now < at) && current == self.failed {
            return None;
        }
        let ready = current.is_some() && probe(&file).is_ok() && identity(&file).ok() == current;
        if ready {
            return Some(self.launch_path());
        }
        self.fail(now, current);
        None
```

In `tray/src/restart.rs`, replace:

```rust
    /// The `exec` of the path `check` returned failed: the old code keeps
    /// running, and the file waits as after a failed probe.
    pub fn exec_failed(&mut self, now: Instant) {
        let current = identity(&self.path).ok();
        self.fail(now, current);
    }

```

with:

```rust
    /// The `exec` of the path `check` returned failed: the old code keeps
    /// running, and the file waits as after a failed probe.
    pub fn exec_failed(&mut self, now: Instant) {
        let current = identity(&self.watched().0).ok();
        self.fail(now, current);
    }

```

In `tray/src/tray.rs`, replace:

```rust
    };
    let windows = Windows::from_env(std::env::var(WINDOWS_ENV).ok().as_deref());
    let restarter = Restarter::start(controller::TRAY_VERSION);
    let tray_exe = restarter
        .as_ref()
        .map(|r| r.path().to_path_buf())
        .or_else(|| env.own_exe.clone())
        .unwrap_or_else(|| PathBuf::from("mailtriage-tray"));
    let controller = Controller::new(
        Flags {
```

with:

```rust
    };
    let windows = Windows::from_env(std::env::var(WINDOWS_ENV).ok().as_deref());
    let restarter = Restarter::start(controller::TRAY_VERSION);
    // Windows start from the launch path: a keg's `opt` path outlives
    // `brew upgrade`, which deletes the keg this tray runs from.
    let tray_exe = restarter
        .as_ref()
        .map(Restarter::launch_path)
        .or_else(|| env.own_exe.as_deref().map(crate::brew::launch_path))
        .unwrap_or_else(|| PathBuf::from("mailtriage-tray"));
    let controller = Controller::new(
        Flags {
```

- [ ] **Step 4: Run the new tests**

Run: `cargo test --locked --test brew_paths --test update_restart && cargo test --locked --lib -- update::restart distribution::brew && cargo test --locked -p mailtriage-tray --test brew --test tray --test autostart --lib`
Expected: PASS.

- [ ] **Step 5: Document it**

In `docs/guide.md`, replace:

```markdown
`--interval-seconds` (1 to 86400, default 60) and `--limit` (1 to 500, default 100) are passed to `watch`. The executable is the one that ran `service install`. `mailtriage setup` runs the same install in its last step.
```

with:

```markdown
`--interval-seconds` (1 to 86400, default 60) and `--limit` (1 to 500, default 100) are passed to `watch`. The executable is the one that ran `service install`; for a Homebrew install it is its `opt` path, `$(brew --prefix)/opt/mailtriage/bin/mailtriage`, which `brew upgrade` keeps pointing at the current version. `mailtriage setup` runs the same install in its last step.
```

In `docs/guide.md`, replace:

```markdown
It never does this during a pass, and after Ctrl-C or SIGTERM it stops instead. It works in every `updates` mode and needs no `service install`; moving the binary to another path does need `service install`.
```

with:

```markdown
It never does this during a pass, and after Ctrl-C or SIGTERM it stops instead. It works in every `updates` mode and needs no `service install`; moving the binary to another path does need `service install`.

A Homebrew install keeps each version in its own directory, and `brew upgrade` deletes the old one. `watch` therefore also follows `$(brew --prefix)/opt/mailtriage/bin/mailtriage`: when that leads to another file than the running one, it runs `--version` on it and re-executes the `opt` path, between passes, as above.
```

In `docs/guide.md`, replace:

```markdown
The tray uses the `mailtriage` next to its own executable, else the first on your `PATH`; `--mailtriage PATH` overrides both. It uses the config that `mailtriage service status` finds (see [Where mailtriage finds the config](#where-mailtriage-finds-the-config)); `--config PATH` overrides it. Both are resolved once at start and made absolute. A later change of `PATH`, the working directory or a symlink therefore never redirects a running tray; restart it to follow one.
```

with:

```markdown
The tray uses the `mailtriage` next to its own executable, else the first on your `PATH`; `--mailtriage PATH` overrides both. For a Homebrew install it uses the `opt` path, `$(brew --prefix)/opt/mailtriage/bin/mailtriage`, which outlives `brew upgrade`. It uses the config that `mailtriage service status` finds (see [Where mailtriage finds the config](#where-mailtriage-finds-the-config)); `--config PATH` overrides it. Both are resolved once at start and made absolute. A later change of `PATH`, the working directory or a symlink therefore never redirects a running tray; restart it to follow one.
```

In `docs/guide.md`, replace:

```markdown
- When `mailtriage-tray` is replaced on disk by a version that runs, the running tray restarts itself onto it within about 15 s, with the same config and `mailtriage`. An open categories window keeps running.
```

with:

```markdown
- When `mailtriage-tray` is replaced on disk by a version that runs, the running tray restarts itself onto it within about 15 s, with the same config and `mailtriage`. An open categories window keeps running. After `brew upgrade`, a Homebrew tray restarts onto its `opt` path.
```

In `docs/guide.md`, replace:

```markdown
- The login item records the tray's absolute path with `--config` and `--mailtriage`, both absolute and resolved as above. Without `--config`, `enable` runs `mailtriage service status --json` once to learn the config. On macOS it also records your current `PATH`, as `service install` does. Run `enable` again after you move `mailtriage`, the tray or the config.
```

with:

```markdown
- The login item records the tray's absolute path with `--config` and `--mailtriage`, both absolute and resolved as above. Without `--config`, `enable` runs `mailtriage service status --json` once to learn the config. On macOS it also records your current `PATH`, as `service install` does. Run `enable` again after you move `mailtriage`, the tray or the config. For a Homebrew install it records the `opt` paths of both, which survive `brew upgrade`.
```

- [ ] **Step 6: Full check and commit**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`
Expected: all pass.

```bash
git add docs/guide.md \
  src/distribution/brew.rs \
  src/distribution/mod.rs \
  src/system_service.rs \
  src/update/restart.rs \
  tests/brew_paths.rs \
  tray/src/autostart.rs \
  tray/src/brew.rs \
  tray/src/lib.rs \
  tray/src/paths.rs \
  tray/src/restart.rs \
  tray/src/tray.rs \
  tray/tests/brew.rs
git commit -m "Record and follow Homebrew's opt path for the service, tray and restarts

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: `mailtriage-tray quit`

The running tray listens on `tray.sock` next to `tray.lock` in the cache directory (created after taking the lock, an old socket file removed first). `mailtriage-tray quit [--json]` sends `quit <its canonical path>`; the tray quits like its menu's Quit only for its own installation (Homebrew kegs of one prefix count as one) and answers `other_installation` otherwise. No PID is ever signalled. Tests run the tray's own listener in the test process, which holds `tray.lock`, against the real `mailtriage-tray quit` binary.

**Files:**
- Create: `tray/src/quit.rs`
- Modify: `tray/src/instances.rs`, `tray/src/lib.rs`, `tray/src/args.rs`, `tray/src/main.rs`, `tray/src/tray.rs`, `docs/guide.md`
- Test: `tray/tests/quit.rs` (new)

**Interfaces:**
- Consumes: `brew::same_installation` (Task 6); `instances::tray_lock`, `paths::{Env, cache_dir}` (on `main`).
- Produces:
  - `mailtriage_tray::quit::{SOCKET, WAIT, NO_ANSWER, listen(cache: &Path, own: PathBuf, on_quit: impl Fn() + Send + 'static) -> io::Result<()>, listen_or_warn(cache: &Path, own: PathBuf, on_quit: impl Fn() + Send + 'static), Outcome { Quit, NotRunning, OtherInstallation }` (`as_str`), `lock_free(&Path) -> bool`, `quit(cache: &Path, own: &Path) -> Result<Outcome, String>`, `run(json_mode: bool) -> i32}`.
  - `instances::TRAY_LOCK = "tray.lock"`; `args::Sub::Quit { json: bool }`; `tray.rs`'s `UserEvent::Quit`, handled as `Action::Quit`.
  - CLI contract used by Task 9: `mailtriage-tray quit --json` prints `{"schema_version":1,"quit":"quit"|"not_running"|"other_installation"}` and exits 0, or `{"schema_version":1,"error":{"code":3,"message":"the tray does not respond; quit it from its menu"}}` and exits 3.

- [ ] **Step 1: Write the failing tests**

Create `tray/tests/quit.rs`:

```rust
//! `mailtriage-tray quit` through the binary against a tray stand-in: this
//! test process takes `tray.lock` and runs the tray's own listener, whose
//! quit releases the lock as the tray's exit would. No menu bar is needed.
//! The cache lives under /tmp, so the socket path stays short.
use mailtriage_tray::{instances, quit};
use serde_json::Value;
use std::{
    fs,
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Self {
        Self {
            dir: tempfile::Builder::new().tempdir_in("/tmp").unwrap(),
        }
    }

    fn cache(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.dir.path().join("Library/Caches/mailtriage")
        } else {
            self.dir.path().join("cache/mailtriage")
        }
    }

    /// `<program> quit --json` with this HOME and cache.
    fn quit(&self, program: &Path) -> (Option<i32>, Value) {
        let out = Command::new(program)
            .args(["quit", "--json"])
            .env("HOME", self.dir.path())
            .env("XDG_CACHE_HOME", self.dir.path().join("cache"))
            .stdin(Stdio::null())
            .output()
            .unwrap();
        (
            out.status.code(),
            serde_json::from_slice(&out.stdout).unwrap_or(Value::Null),
        )
    }
}

/// A copy of the tray binary at `<root>/<dir>/mailtriage-tray`, canonical.
fn installation(root: &Path, dir: &str) -> PathBuf {
    let path = root.join(dir).join("mailtriage-tray");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_mailtriage-tray"), &path).unwrap();
    fs::canonicalize(path).unwrap()
}

/// A running tray of the installation at `own`: the lock and the listener.
/// The receiver gets a message when it quits.
fn tray(cache: &Path, own: PathBuf) -> mpsc::Receiver<()> {
    let lock = Arc::new(Mutex::new(Some(
        instances::tray_lock(cache)
            .unwrap()
            .expect("the lock is free"),
    )));
    let (sent, quits) = mpsc::channel();
    quit::listen(cache, own, move || {
        // The tray exits: its lock is released.
        lock.lock().unwrap().take();
        let _ = sent.send(());
    })
    .unwrap();
    quits
}

#[test]
fn a_running_tray_of_this_installation_quits() {
    let home = Home::new();
    let root = fs::canonicalize(home.dir.path()).unwrap();
    let own = installation(&root, "a");
    let quits = tray(&home.cache(), own.clone());
    // A categories window: a process the tray started, which keeps running.
    let mut window = Command::new("/bin/sh")
        .args(["-c", "sleep 30"])
        .spawn()
        .unwrap();
    let (code, v) = home.quit(&own);
    assert_eq!((code, v["quit"].clone()), (Some(0), "quit".into()), "{v}");
    assert!(quits.recv_timeout(Duration::from_secs(1)).is_ok());
    assert!(quit::lock_free(&home.cache()));
    assert!(
        window.try_wait().unwrap().is_none(),
        "the window still runs"
    );
    window.kill().unwrap();
    let _ = window.wait();
    // Nothing runs now: not_running.
    let (code, v) = home.quit(&own);
    assert_eq!((code, v["quit"].clone()), (Some(0), "not_running".into()));
}

#[test]
fn another_installations_tray_keeps_running() {
    let home = Home::new();
    let root = fs::canonicalize(home.dir.path()).unwrap();
    let theirs = installation(&root, "b");
    let quits = tray(&home.cache(), theirs);
    // This test's tray binary is another installation than `b`.
    let (code, v) = home.quit(Path::new(env!("CARGO_BIN_EXE_mailtriage-tray")));
    assert_eq!(
        (code, v["quit"].clone()),
        (Some(0), "other_installation".into()),
        "{v}"
    );
    assert!(quits.recv_timeout(Duration::from_millis(300)).is_err());
    assert!(
        !quit::lock_free(&home.cache()),
        "B's tray still holds its lock"
    );
}

#[test]
fn a_stale_socket_with_a_free_lock_is_not_running() {
    let home = Home::new();
    fs::create_dir_all(home.cache()).unwrap();
    drop(UnixListener::bind(home.cache().join(quit::SOCKET)).unwrap());
    assert!(home.cache().join(quit::SOCKET).exists());
    let (code, v) = home.quit(Path::new(env!("CARGO_BIN_EXE_mailtriage-tray")));
    assert_eq!((code, v["quit"].clone()), (Some(0), "not_running".into()));
}

#[test]
fn a_held_lock_without_an_answer_exits_3() {
    let home = Home::new();
    let _held = instances::tray_lock(&home.cache()).unwrap().unwrap();
    let (code, v) = home.quit(Path::new(env!("CARGO_BIN_EXE_mailtriage-tray")));
    assert_eq!(code, Some(3));
    assert_eq!(
        v["error"]["message"],
        "the tray does not respond; quit it from its menu"
    );
}

#[test]
fn a_new_tray_replaces_an_old_socket_file() {
    let home = Home::new();
    fs::create_dir_all(home.cache()).unwrap();
    fs::write(home.cache().join(quit::SOCKET), "left over").unwrap();
    let root = fs::canonicalize(home.dir.path()).unwrap();
    let own = installation(&root, "a");
    let quits = tray(&home.cache(), own.clone());
    let (code, v) = home.quit(&own);
    assert_eq!((code, v["quit"].clone()), (Some(0), "quit".into()), "{v}");
    assert!(quits.recv_timeout(Duration::from_secs(1)).is_ok());
}

/// A cache path longer than a Unix socket path may be (104 bytes on macOS):
/// the tray runs without its socket, and `quit` fails safe (exit 3) while
/// it holds its lock.
#[test]
fn a_socket_path_that_is_too_long_is_not_fatal() {
    let home = Home::new();
    let deep = home.cache().join("x".repeat(120));
    fs::create_dir_all(&deep).unwrap();
    let _held = instances::tray_lock(&deep).unwrap().unwrap();
    assert!(quit::listen(&deep, PathBuf::from("/a/mailtriage-tray"), || {}).is_err());
    quit::listen_or_warn(&deep, PathBuf::from("/a/mailtriage-tray"), || {});
    assert_eq!(
        quit::quit(&deep, Path::new("/a/mailtriage-tray")),
        Err(quit::NO_ANSWER.to_owned())
    );
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test --locked -p mailtriage-tray --test quit`
Expected: FAIL to compile: E0432, unresolved import `mailtriage_tray::quit`.

- [ ] **Step 3: Implement**

`tray.rs` only wires the listener to the event loop (a GUI path no test runs); everything testable is in `quit.rs`, which carries unit tests for the answers and the lock check. The cache paths in the tests live under `/tmp`, so the socket path stays below macOS's 104-byte limit.

Create `tray/src/quit.rs`:

```rust
//! `mailtriage-tray quit`: ends the running tray of this installation
//! without signalling any process. The tray listens on `tray.sock` next to
//! `tray.lock` in the cache directory (mode 0700); `quit` sends
//! `quit <its canonical path>`, and the tray quits, as its menu's Quit does,
//! only when that path is its own installation's.
use crate::{brew, instances::TRAY_LOCK, paths};
use fs2::FileExt;
use serde_json::json;
use std::{
    fs::{self, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    os::unix::{ffi::OsStrExt, net::UnixListener, net::UnixStream},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

/// The socket's file name in the cache directory.
pub const SOCKET: &str = "tray.sock";
/// How long `quit` waits for an answer, and then for `tray.lock` to be free.
pub const WAIT: Duration = Duration::from_secs(5);
/// What `quit` says when the tray holds its lock but does not answer.
pub const NO_ANSWER: &str = "the tray does not respond; quit it from its menu";

/// Listens on `<cache>/tray.sock` for the tray whose canonical path is
/// `own`, on a thread of its own. Call it only while holding `tray.lock`:
/// that proves no other tray owns an old socket file, which is removed
/// first. `on_quit` runs after the answer to a request from this
/// installation; the thread then stops listening.
pub fn listen(cache: &Path, own: PathBuf, on_quit: impl Fn() + Send + 'static) -> io::Result<()> {
    let path = cache.join(SOCKET);
    match fs::remove_file(&path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let listener = UnixListener::bind(&path)?;
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            if answer(stream, &own) {
                on_quit();
                return;
            }
        }
    });
    Ok(())
}

/// Answers one connection; `true` when it asked this installation to quit.
fn answer(stream: UnixStream, own: &Path) -> bool {
    let _ = stream.set_read_timeout(Some(WAIT));
    let mut line = Vec::new();
    let mut reader = BufReader::new(&stream);
    if reader.read_until(b'\n', &mut line).is_err() {
        return false;
    }
    let line = line.strip_suffix(b"\n").unwrap_or(&line);
    let (reply, quit) = match line.strip_prefix(b"quit ") {
        Some(path) => {
            let theirs = Path::new(std::ffi::OsStr::from_bytes(path));
            if brew::same_installation(own, theirs) {
                ("quit\n", true)
            } else {
                ("other_installation\n", false)
            }
        }
        None => ("unknown\n", false),
    };
    let mut writer = &stream;
    let _ = writer.write_all(reply.as_bytes());
    let _ = writer.flush();
    quit
}

/// What `quit` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The tray of this installation quit and released `tray.lock`.
    Quit,
    /// No tray runs: nothing listens and `tray.lock` is free.
    NotRunning,
    /// A tray of another installation runs; it keeps running.
    OtherInstallation,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Quit => "quit",
            Outcome::NotRunning => "not_running",
            Outcome::OtherInstallation => "other_installation",
        }
    }
}

/// Whether no process holds `<cache>/tray.lock` (a missing file is free).
pub fn lock_free(cache: &Path) -> bool {
    let Ok(file) = OpenOptions::new().read(true).open(cache.join(TRAY_LOCK)) else {
        return true;
    };
    let free = file.try_lock_exclusive().is_ok();
    let _ = FileExt::unlock(&file);
    free
}

/// Asks the tray listening in `cache` to quit on behalf of `own` (this
/// program's canonical path). `Err` is the exit-3 message: the tray holds
/// its lock but does not answer, or did not stop within `WAIT`.
pub fn quit(cache: &Path, own: &Path) -> Result<Outcome, String> {
    let Ok(mut stream) = UnixStream::connect(cache.join(SOCKET)) else {
        return if lock_free(cache) {
            Ok(Outcome::NotRunning)
        } else {
            Err(NO_ANSWER.to_owned())
        };
    };
    let bytes = own.as_os_str().as_bytes();
    if bytes.contains(&b'\n') {
        return Err(format!("{} cannot be sent to the tray", own.display()));
    }
    let _ = stream.set_read_timeout(Some(WAIT));
    let mut request = b"quit ".to_vec();
    request.extend_from_slice(bytes);
    request.push(b'\n');
    if stream.write_all(&request).is_err() {
        return Err(NO_ANSWER.to_owned());
    }
    let mut reply = String::new();
    let _ = BufReader::new(&stream).read_line(&mut reply);
    match reply.trim_end() {
        "other_installation" => Ok(Outcome::OtherInstallation),
        "quit" => {
            let deadline = Instant::now() + WAIT;
            while !lock_free(cache) {
                if Instant::now() >= deadline {
                    return Err(NO_ANSWER.to_owned());
                }
                thread::sleep(Duration::from_millis(50));
            }
            Ok(Outcome::Quit)
        }
        _ => Err(NO_ANSWER.to_owned()),
    }
}

/// `mailtriage-tray quit [--json]`: prints the outcome and returns the exit
/// code (0, or 3 when the tray did not stop).
pub fn run(json_mode: bool) -> i32 {
    let env = paths::Env::current();
    let result = match (
        paths::cache_dir(env.home.as_deref(), env.xdg_cache_home.as_deref()),
        env.own_exe,
    ) {
        (Some(cache), Some(own)) => quit(&cache, &own),
        (None, _) => Err("HOME is not set".to_owned()),
        (_, None) => Err("cannot find this program's path".to_owned()),
    };
    match result {
        Ok(outcome) => {
            let value = json!({"schema_version": 1, "quit": outcome.as_str()});
            if json_mode {
                println!("{value}");
            } else {
                println!("{}", outcome.as_str());
            }
            0
        }
        Err(message) => {
            if json_mode {
                println!(
                    "{}",
                    json!({"schema_version": 1, "error": {"code": 3, "message": message}})
                );
            } else {
                eprintln!("mailtriage-tray: {message}");
            }
            3
        }
    }
}

/// Starts the listener for a tray that holds `tray.lock` in `cache`; a tray
/// whose socket cannot be created (for example a path longer than the
/// system allows) runs without it and says so.
pub fn listen_or_warn(cache: &Path, own: PathBuf, on_quit: impl Fn() + Send + 'static) {
    if let Err(e) = listen(cache, own, on_quit) {
        eprintln!(
            "mailtriage-tray: cannot listen on {}: {e}; mailtriage-tray quit will not reach this tray",
            cache.join(SOCKET).display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instances;

    #[test]
    fn requests_are_answered_by_installation() {
        let own = Path::new("/opt/homebrew/Cellar/mailtriage/1.2.3/bin/mailtriage-tray");
        for (theirs, reply, quits) in [
            (own.to_path_buf(), "quit\n", true),
            (
                PathBuf::from("/opt/homebrew/Cellar/mailtriage/1.2.4/bin/mailtriage-tray"),
                "quit\n",
                true,
            ),
            (
                PathBuf::from("/home/a/.local/bin/mailtriage-tray"),
                "other_installation\n",
                false,
            ),
        ] {
            let (mut client, server) = UnixStream::pair().unwrap();
            let mut request = b"quit ".to_vec();
            request.extend_from_slice(theirs.as_os_str().as_bytes());
            request.push(b'\n');
            client.write_all(&request).unwrap();
            assert_eq!(answer(server, own), quits);
            let mut got = String::new();
            BufReader::new(&client).read_line(&mut got).unwrap();
            assert_eq!(got, reply);
        }
    }

    #[test]
    fn instances_share_the_lock_file_name() {
        let dir = tempfile::Builder::new().tempdir_in("/tmp").unwrap();
        assert!(lock_free(dir.path()));
        let held = instances::tray_lock(dir.path()).unwrap().unwrap();
        assert!(!lock_free(dir.path()));
        drop(held);
        assert!(lock_free(dir.path()));
    }
}
```

In `tray/src/instances.rs`, replace:

```rust
    path::Path,
};

/// The variable that hands the window PIDs to a re-executed tray.
pub const WINDOWS_ENV: &str = "MAILTRIAGE_TRAY_WINDOWS";

```

with:

```rust
    path::Path,
};

/// The tray's lock file in the cache directory.
pub const TRAY_LOCK: &str = "tray.lock";

/// The variable that hands the window PIDs to a re-executed tray.
pub const WINDOWS_ENV: &str = "MAILTRIAGE_TRAY_WINDOWS";

```

In `tray/src/instances.rs`, replace:

```rust

/// The tray's lock, `tray.lock`.
pub fn tray_lock(cache: &Path) -> io::Result<Option<File>> {
    try_lock(&cache.join("tray.lock"))
}

/// `editor-<first 16 hex of SHA-256 of the absolute config path>`.
```

with:

```rust

/// The tray's lock, `tray.lock`.
pub fn tray_lock(cache: &Path) -> io::Result<Option<File>> {
    try_lock(&cache.join(TRAY_LOCK))
}

/// `editor-<first 16 hex of SHA-256 of the absolute config path>`.
```

In `tray/src/lib.rs`, replace:

```rust
pub mod instances;
pub mod model;
pub mod paths;
pub mod restart;
pub mod tray;
```

with:

```rust
pub mod instances;
pub mod model;
pub mod paths;
pub mod quit;
pub mod restart;
pub mod tray;
```

In `tray/src/args.rs`, replace:

```rust
        #[arg(long)]
        account: Option<String>,
    },
    /// Start the tray when you log in.
    Autostart {
        #[command(subcommand)]
```

with:

```rust
        #[arg(long)]
        account: Option<String>,
    },
    /// Quit the running tray of this installation; a tray of another
    /// installation keeps running.
    Quit {
        /// Print JSON.
        #[arg(long)]
        json: bool,
    },
    /// Start the tray when you log in.
    Autostart {
        #[command(subcommand)]
```

In `tray/src/main.rs`, replace:

```rust
use clap::Parser;
use mailtriage_tray::{
    args::{Args, Sub},
    autostart, editor, tray,
};

fn main() {
```

with:

```rust
use clap::Parser;
use mailtriage_tray::{
    args::{Args, Sub},
    autostart, editor, quit, tray,
};

fn main() {
```

In `tray/src/main.rs`, replace:

```rust
        None => tray::run(&args),
        Some(Sub::Autostart { action, json }) => autostart::run(&args, *action, *json),
        Some(Sub::Categories { account }) => editor::run(&args, account.clone()),
    };
    std::process::exit(code);
}
```

with:

```rust
        None => tray::run(&args),
        Some(Sub::Autostart { action, json }) => autostart::run(&args, *action, *json),
        Some(Sub::Categories { account }) => editor::run(&args, account.clone()),
        Some(Sub::Quit { json }) => quit::run(*json),
    };
    std::process::exit(code);
}
```

In `tray/src/tray.rs`, replace:

```rust
    controller::{self, Controller, Done, Flags, Work},
    icons,
    instances::{self, Windows, WINDOWS_ENV},
    model::{
        health::IconState,
        menu::{Entry, Menu},
    },
    paths,
    restart::{self, Restarter},
};
use chrono::{Local, Utc};
```

with:

```rust
    controller::{self, Controller, Done, Flags, Work},
    icons,
    instances::{self, Windows, WINDOWS_ENV},
    model::menu::Action,
    model::{
        health::IconState,
        menu::{Entry, Menu},
    },
    paths, quit,
    restart::{self, Restarter},
};
use chrono::{Local, Utc};
```

In `tray/src/tray.rs`, replace:

```rust
enum UserEvent {
    Menu(String),
    Done(Box<Done>),
}

/// Appends `entries` through `append`; each clickable entry's id is its
```

with:

```rust
enum UserEvent {
    Menu(String),
    Done(Box<Done>),
    /// `mailtriage-tray quit` from this installation.
    Quit,
}

/// Appends `entries` through `append`; each clickable entry's id is its
```

In `tray/src/tray.rs`, replace:

```rust
        .map(Restarter::launch_path)
        .or_else(|| env.own_exe.as_deref().map(crate::brew::launch_path))
        .unwrap_or_else(|| PathBuf::from("mailtriage-tray"));
    let controller = Controller::new(
        Flags {
            mailtriage: args.mailtriage.clone(),
```

with:

```rust
        .map(Restarter::launch_path)
        .or_else(|| env.own_exe.as_deref().map(crate::brew::launch_path))
        .unwrap_or_else(|| PathBuf::from("mailtriage-tray"));
    let own = env.own_exe.clone();
    let controller = Controller::new(
        Flags {
            mailtriage: args.mailtriage.clone(),
```

In `tray/src/tray.rs`, replace:

```rust
        autostart::Env::detect().ok(),
        windows,
    );
    event_loop(controller, restarter, lock)
}

fn spawn(
```

with:

```rust
        autostart::Env::detect().ok(),
        windows,
    );
    event_loop(controller, restarter, lock, cache, own)
}

fn spawn(
```

In `tray/src/tray.rs`, replace:

```rust
    mut controller: Controller,
    mut restarter: Option<Restarter>,
    lock: std::fs::File,
) -> ! {
    // Only macOS mutates the loop (its activation policy).
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
```

with:

```rust
    mut controller: Controller,
    mut restarter: Option<Restarter>,
    lock: std::fs::File,
    cache: PathBuf,
    own: Option<PathBuf>,
) -> ! {
    // Only macOS mutates the loop (its activation policy).
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
```

In `tray/src/tray.rs`, replace:

```rust
            let _ = proxy.send_event(UserEvent::Menu(event.id.0));
        }));
    }
    let running = Arc::new(AtomicUsize::new(0));
    let mut clipboard = arboard::Clipboard::new().ok();
    let mut tray: Option<TrayIcon> = None;
```

with:

```rust
            let _ = proxy.send_event(UserEvent::Menu(event.id.0));
        }));
    }
    // After the lock, which proves no other tray owns an old socket.
    if let Some(own) = own {
        let proxy = proxy.clone();
        quit::listen_or_warn(&cache, own, move || {
            let _ = proxy.send_event(UserEvent::Quit);
        });
    }
    let running = Arc::new(AtomicUsize::new(0));
    let mut clipboard = arboard::Clipboard::new().ok();
    let mut tray: Option<TrayIcon> = None;
```

In `tray/src/tray.rs`, replace:

```rust
                }
            }
            Event::UserEvent(UserEvent::Done(done)) => work = controller.done(*done, now),
            _ => {}
        }
        if Instant::now() >= next_refresh {
```

with:

```rust
                }
            }
            Event::UserEvent(UserEvent::Done(done)) => work = controller.done(*done, now),
            // As the menu's Quit.
            Event::UserEvent(UserEvent::Quit) => work = controller.act(Action::Quit, now),
            _ => {}
        }
        if Instant::now() >= next_refresh {
```

- [ ] **Step 4: Run the new tests**

Run: `cargo test --locked -p mailtriage-tray --test quit --test tray --lib`
Expected: PASS (6 quit tests).

- [ ] **Step 5: Document it**

In `docs/guide.md`, replace:

````markdown
mailtriage-tray autostart enable|disable|status [--config PATH] [--mailtriage PATH] [--json]
```

### What the tray runs
````

with:

````markdown
mailtriage-tray autostart enable|disable|status [--config PATH] [--mailtriage PATH] [--json]
mailtriage-tray quit [--json]
```

### What the tray runs
````

In `docs/guide.md`, replace:

```markdown
- One tray runs per user. A second start prints `mailtriage-tray is already running` and exits 0.
```

with:

```markdown
- One tray runs per user. A second start prints `mailtriage-tray is already running` and exits 0.
- `mailtriage-tray quit` quits the running tray of this installation, as its menu's Quit does, and prints `quit`, `not_running` (no tray runs) or `other_installation` (the running tray belongs to a `mailtriage-tray` elsewhere, which keeps running); with `--json`, `{"schema_version":1,"quit":"quit"}`. It talks to the tray over `tray.sock` next to `tray.lock` in the cache directory and signals no process. It exits 3 with `the tray does not respond; quit it from its menu` when a tray holds its lock but does not answer within 5 seconds. Open categories windows keep running.
```

- [ ] **Step 6: Full check and commit**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`
Expected: all pass.

```bash
git add docs/guide.md \
  tray/src/args.rs \
  tray/src/instances.rs \
  tray/src/lib.rs \
  tray/src/main.rs \
  tray/src/quit.rs \
  tray/src/tray.rs \
  tray/tests/quit.rs
git commit -m "Add mailtriage-tray quit over a socket scoped to the installation

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: `mailtriage self install`

`self install --dir DIR [--tray-file PATH] [--no-setup] [--yes] [--json]` installs this binary into DIR with the updater's transaction, split here into a reusable `update::install::place` (staging, smoke test, revalidation, backup when there is a file, rename, publish, directory sync, record). Then the tray the same way, the PATH report, setup through the installed binary, and the login item with setup's config. Results that fail after the install (a failed tray or setup) still print with exit 3.

**Files:**
- Create: `src/distribution/self_install.rs`
- Modify: `src/update/install.rs`, `src/prompt.rs`, `src/distribution/mod.rs`, `src/cli.rs`, `docs/guide.md`
- Test: `tests/install_support/mod.rs`, `tests/self_install.rs` (new)

**Interfaces:**
- Consumes: `distribution::protected::{prepare, unsafe_dir, Unsafe, Why::NotYours}` (Task 3); `update::install::{lock, update_lock_wait, Hooks, EnvHooks}`, `update::cache::Cache`, `update::platform::{probe, FileIdentity}`, `config::setup_path`, `setup::shell_line` (on `main`).
- Produces:
  - `update::install::{Placement<'a> { component: Component, path: &'a Path, before: Option<FileIdentity>, version: &'a Version, cache: Option<&'a Cache>, hooks: &'a dyn Hooks }, Placed { previous_path: Option<PathBuf>, warnings: Vec<String> }, DoesNotRun(pub String)` (`Display` keeps `the new binary does not run here: …`), `place(&Placement, &InstallLock, &[u8]) -> anyhow::Result<Placed>}`; `install::install` uses `place` with unchanged messages and behaviour.
  - `prompt::Prompter::confirm_or(&mut self, question: &str, default: bool) -> bool`.
  - `distribution::self_install::{TEST_TERMINAL, TRAY_PACKAGES, Args { dir, tray_file, no_setup, yes }, terminal() -> bool, run(&Args, &dyn Hooks, &mut Prompter) -> anyhow::Result<Value>, PathReport { on_path, shadowed_by, lines }, path_report(&Path, Option<&OsStr>, Option<&OsStr>, bool) -> PathReport, path_line(&Path, Option<&OsStr>, bool) -> String}`.
  - CLI: `mailtriage self install …`; `cli::run` turns a top-level `exit_code` in a result into the exit code and does not print it.
  - `tests/install_support::fake_tray(version: &str) -> String` (prints `mailtriage-tray VERSION`, logs other calls to `tray.log`, answers `quit` from `quit.json`/`quit.code`).

- [ ] **Step 1: Write the failing tests**

In `tests/install_support/mod.rs`, replace:

```rust
#![allow(dead_code)]
//! Shared by the install tests: a fake Himalaya. Nothing here touches the
//! real HOME.
use std::{fs, os::unix::fs::PermissionsExt, path::Path};

/// A fake Himalaya whose `--version` prints `@VERSION@` (`flip`: 2.1.0 the
```

with:

```rust
#![allow(dead_code)]
//! Shared by the install tests: fake Himalaya and tray programs. Nothing
//! here touches the real HOME.
use std::{fs, os::unix::fs::PermissionsExt, path::Path};

/// A fake Himalaya whose `--version` prints `@VERSION@` (`flip`: 2.1.0 the
```

In `tests/install_support/mod.rs`, replace:

```rust
    FAKE_HIMALAYA.replace("@VERSION@", version_line)
}

/// Writes an executable file, creating its directory.
pub fn write_exe(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
```

with:

```rust
    FAKE_HIMALAYA.replace("@VERSION@", version_line)
}

/// A fake tray: `--version` prints `mailtriage-tray VERSION`; any other
/// call appends its arguments to `tray.log` next to the file it runs from.
/// `quit` prints `quit.json` from there (default: the result `quit`) and
/// exits with `quit.code` (default 0).
pub fn fake_tray(version: &str) -> String {
    format!(
        r#"#!/bin/sh
dir="$(dirname "$0")"
if [ "$1" = --version ]; then echo 'mailtriage-tray {version}'; exit 0; fi
echo "$*" >> "$dir/tray.log"
if [ "$1" = quit ]; then
  if [ -f "$dir/quit.json" ]; then cat "$dir/quit.json"; else echo '{{"schema_version":1,"quit":"quit"}}'; fi
  exit "$(cat "$dir/quit.code" 2>/dev/null || echo 0)"
fi
"#
    )
}

/// Writes an executable file, creating its directory.
pub fn write_exe(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
```

Create `tests/self_install.rs`:

```rust
#![cfg(unix)]
//! `mailtriage self install` through the binary: a copy of the binary under
//! test in a "download" directory installs itself into DIR. HOME, the cache
//! and the data directory are temporary; launchctl, systemctl, Himalaya and
//! the tray are fakes; stdin counts as a terminal only through the debug
//! builds' `MAILTRIAGE_TEST_TERMINAL`.
mod common;
mod install_support;
mod update_support;
use common::{write_tool, LAUNCHCTL, SYSTEMCTL};
use fs2::FileExt;
use install_support::{fake_himalaya, fake_tray, write_exe};
use mailtriage::{system_service::Manager, update::service_files};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};
use update_support::{cache_dir, Server, Watch};

const RUNNING: &str = env!("CARGO_PKG_VERSION");

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        for d in [
            "download",
            "home/.config/himalaya",
            "xdg",
            "data",
            "tools",
            "state",
        ] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        let download = root.join("download/mailtriage");
        fs::copy(env!("CARGO_BIN_EXE_mailtriage"), &download).unwrap();
        fs::set_permissions(&download, fs::Permissions::from_mode(0o755)).unwrap();
        write_tool(&root.join("tools"), "launchctl", LAUNCHCTL);
        write_tool(&root.join("tools"), "systemctl", SYSTEMCTL);
        write_exe(
            &root.join("tools/himalaya"),
            &fake_himalaya("himalaya v2.1.0 +imap"),
        );
        fs::write(
            root.join("home/.config/himalaya/config.toml"),
            "[accounts.work]\nemail = \"work@example.test\"\nimap.server = \"imaps://mail.example.test\"\n",
        )
        .unwrap();
        Self { _dir: dir, root }
    }

    fn dir(&self) -> PathBuf {
        self.root.join("bin")
    }

    fn installed(&self) -> PathBuf {
        self.dir().join("mailtriage")
    }

    /// `program ARGS` with this fixture's environment; `stdin` is piped in.
    fn run_program(
        &self,
        program: &Path,
        args: &[&str],
        stdin: &str,
        env: &[(&str, &str)],
    ) -> (Output, Value) {
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(&self.root)
            .env("HOME", self.root.join("home"))
            .env("XDG_CACHE_HOME", self.root.join("xdg"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("tools").display()),
            )
            .env("MT_FAKE_DIR", self.root.join("state"))
            .env("MT_FAKE_HOME", self.root.join("home"))
            .env("TZ", "Europe/Berlin")
            .env("SHELL", "/bin/zsh")
            .env_remove("MAILTRIAGE_CONFIG")
            .env_remove("HIMALAYA_CONFIG")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("MAILTRIAGE_TEST_TERMINAL")
            .env_remove("MAILTRIAGE_UPDATE_TEST_HOOK")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = command.spawn().unwrap();
        let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
        let out = child.wait_with_output().unwrap();
        let value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
        (out, value)
    }

    /// The downloaded copy's `self install --dir DIR --json ARGS`.
    fn install(&self, args: &[&str], stdin: &str, env: &[(&str, &str)]) -> (Output, Value) {
        let dir = self.dir();
        let all = [
            &["self", "install", "--json", "--dir", dir.to_str().unwrap()][..],
            args,
        ]
        .concat();
        self.run_program(&self.root.join("download/mailtriage"), &all, stdin, env)
    }

    fn cache(&self) -> Value {
        fs::read(cache_dir(&self.root.join("home"), &self.root.join("xdg")).join("update.json"))
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or(Value::Null)
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

#[test]
fn a_fresh_install_creates_the_directory_0755_whatever_the_umask() {
    let f = Fixture::new();
    let dir = f.root.join("new/bin");
    let out = Command::new("/bin/sh")
        .args(["-c", "umask 002; exec \"$@\"", "sh"])
        .arg(f.root.join("download/mailtriage"))
        .args(["self", "install", "--json", "--no-setup", "--dir"])
        .arg(&dir)
        .env("HOME", f.root.join("home"))
        .env("XDG_CACHE_HOME", f.root.join("xdg"))
        .env("PATH", "/usr/bin:/bin")
        .env_remove("MAILTRIAGE_TEST_TERMINAL")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    let s = &v["self_install"];
    assert_eq!(s["dir"], dir.to_str().unwrap());
    assert_eq!(
        s["cli"],
        json!({"action": "installed", "version": RUNNING, "path": dir.join("mailtriage")})
    );
    assert_eq!(s["tray"], Value::Null);
    assert_eq!(s["setup"], "skipped");
    assert_eq!(s["autostart"], "skipped");
    assert_eq!(mode(&f.root.join("new")), 0o755);
    assert_eq!(mode(&dir), 0o755);
    assert_eq!(mode(&dir.join("mailtriage")), 0o755);
    assert!(!dir.join("mailtriage.previous").exists());
    let version = Command::new(dir.join("mailtriage"))
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&version.stdout),
        format!("mailtriage {RUNNING}\n")
    );
    assert!(stderr(&out).contains(&format!("Next: run {}/mailtriage setup", dir.display())));
}

#[test]
fn an_unsafe_directory_is_refused_and_nothing_installed() {
    let f = Fixture::new();
    let shared = f.root.join("shared");
    fs::create_dir(&shared).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o775)).unwrap();
    symlink(&shared, f.root.join("link")).unwrap();
    for dir in [shared.clone(), f.root.join("link")] {
        let (out, v) = f.run_program(
            &f.root.join("download/mailtriage"),
            &[
                "self",
                "install",
                "--json",
                "--no-setup",
                "--dir",
                dir.to_str().unwrap(),
            ],
            "",
            &[],
        );
        assert_eq!(out.status.code(), Some(3), "{v}");
        assert_eq!(v["error"]["reason"], "unsafe_permissions");
        let message = v["error"]["message"].as_str().unwrap();
        assert!(
            message.contains(&format!(
                "run chmod go-w {}, or pass another --dir",
                shared.display()
            )),
            "{message}"
        );
        assert!(!shared.join("mailtriage").exists());
    }
}

#[test]
fn a_newer_installed_version_is_never_downgraded() {
    let f = Fixture::new();
    let newer = "#!/bin/sh\necho 'mailtriage 9.9.9'\n";
    write_exe(&f.installed(), newer);
    fs::set_permissions(f.dir(), fs::Permissions::from_mode(0o755)).unwrap();
    let (out, v) = f.install(&["--no-setup"], "", &[]);
    assert_eq!(out.status.code(), Some(2), "{v}");
    assert_eq!(
        v["error"]["message"],
        format!(
            "{} is 9.9.9, newer than {RUNNING}; to go back, follow the guide's rollback steps",
            f.installed().display()
        )
    );
    assert_eq!(fs::read_to_string(f.installed()).unwrap(), newer);
}

#[test]
fn a_held_installation_lock_exits_5() {
    let f = Fixture::new();
    fs::create_dir_all(f.dir()).unwrap();
    fs::set_permissions(f.dir(), fs::Permissions::from_mode(0o755)).unwrap();
    let lock = fs::File::create(f.dir().join(".mailtriage-update.lock")).unwrap();
    lock.lock_exclusive().unwrap();
    let (out, v) = f.install(
        &["--no-setup"],
        "",
        &[("MAILTRIAGE_UPDATE_TEST_LOCK_WAIT_MS", "300")],
    );
    assert_eq!(out.status.code(), Some(5), "{v}");
    assert!(!f.installed().exists());
}

#[test]
fn a_running_service_restarts_onto_the_installed_file() {
    let f = Fixture::new();
    fs::create_dir_all(f.dir()).unwrap();
    fs::set_permissions(f.dir(), fs::Permissions::from_mode(0o755)).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_mailtriage"), f.installed()).unwrap();
    let server = Server::start();
    let config = f.root.join("cfg/mailtriage.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let (out, _) = f.run_program(
        &f.installed(),
        &["init", "--json", "--config", config.to_str().unwrap()],
        "",
        &[],
    );
    assert!(out.status.success());
    update_support::set_updates(&config, "off");
    let mut watch = Watch::spawn(
        Command::new(f.installed())
            .args([
                "watch",
                "--account",
                "work",
                "--interval-seconds",
                "1",
                "--json",
            ])
            .current_dir(&f.root)
            .env("HOME", f.root.join("home"))
            .env("XDG_CACHE_HOME", f.root.join("xdg"))
            .env("MAILTRIAGE_UPDATE_URL", &server.base)
            .env("MAILTRIAGE_CONFIG", &config),
    );
    watch.wait_passes(1);
    let (out, v) = f.install(&["--no-setup"], "", &[]);
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    let restarting = watch.event("restarting");
    assert_eq!(restarting["update"]["pid"], watch.child.id());
    assert!(f.dir().join("mailtriage.previous").exists());
    watch.stop();
}

#[test]
fn the_tray_is_installed_or_skipped_keeping_the_old_one() {
    let f = Fixture::new();
    let good = f.root.join("download/mailtriage-tray");
    write_exe(&good, &fake_tray(RUNNING));
    let (out, v) = f.install(
        &["--no-setup", "--tray-file", good.to_str().unwrap()],
        "",
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    assert_eq!(
        v["self_install"]["tray"],
        json!({"action": "installed", "error": null})
    );
    let installed = f.dir().join("mailtriage-tray");
    assert_eq!(fs::read_to_string(&installed).unwrap(), fake_tray(RUNNING));
    // A tray that does not run here (here: the wrong version) is skipped.
    let bad = f.root.join("download/other-tray");
    write_exe(&bad, &fake_tray("9.9.9"));
    let (out, v) = f.install(
        &["--no-setup", "--tray-file", bad.to_str().unwrap()],
        "",
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{v}");
    let tray = &v["self_install"]["tray"];
    assert_eq!(tray["action"], "skipped");
    assert_eq!(
        tray["error"],
        format!("mailtriage-tray does not run here: printed version 9.9.9, expected {RUNNING}")
    );
    assert_eq!(fs::read_to_string(&installed).unwrap(), fake_tray(RUNNING));
    assert_eq!(
        stderr(&out).contains("sudo apt install libgtk-3-0 libayatana-appindicator3-1 libxdo3"),
        cfg!(target_os = "linux")
    );
}

#[test]
fn the_cache_records_each_component_and_clears_a_previous_failure() {
    let f = Fixture::new();
    fs::create_dir_all(f.dir()).unwrap();
    fs::set_permissions(f.dir(), fs::Permissions::from_mode(0o755)).unwrap();
    let cache = cache_dir(&f.root.join("home"), &f.root.join("xdg"));
    fs::create_dir_all(&cache).unwrap();
    let key = f.installed().to_str().unwrap().to_owned();
    let failed = json!({"version": "0.0.1", "failures": 3,
        "last_error": {"at": "2026-10-01T00:00:00Z", "message": "checksum mismatch"},
        "next_attempt_at": "2099-01-01T00:00:00Z"});
    fs::write(
        cache.join("update.json"),
        json!({"schema_version": 1, "installs": {key.clone(): failed}}).to_string(),
    )
    .unwrap();
    let tray = f.root.join("download/mailtriage-tray");
    write_exe(&tray, &fake_tray(RUNNING));
    let (out, v) = f.install(
        &["--no-setup", "--tray-file", tray.to_str().unwrap()],
        "",
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{v}");
    let installs = &f.cache()["installs"];
    for path in [
        key,
        f.dir().join("mailtriage-tray").to_str().unwrap().to_owned(),
    ] {
        let entry = &installs[&path];
        assert_eq!(entry["version"], RUNNING, "{path}");
        assert_eq!(entry["failures"], 0, "{path}");
        assert_eq!(entry["last_error"], Value::Null, "{path}");
        assert_eq!(entry["next_attempt_at"], Value::Null, "{path}");
    }
}

#[test]
fn path_and_shadowing_are_reported() {
    let f = Fixture::new();
    write_exe(&f.root.join("other/mailtriage"), "#!/bin/sh\n");
    let path = format!(
        "{}:{}:/usr/bin:/bin",
        f.root.join("other").display(),
        f.dir().display()
    );
    let (out, v) = f.install(&["--no-setup"], "", &[("PATH", &path)]);
    assert_eq!(out.status.code(), Some(0), "{v}");
    assert_eq!(v["self_install"]["on_path"], true);
    assert_eq!(
        v["self_install"]["shadowed_by"],
        f.root.join("other/mailtriage").to_str().unwrap()
    );
    assert!(stderr(&out).contains("stays in use and is not updated by this install"));
    let (out, v) = f.install(&["--no-setup"], "", &[("PATH", "/usr/bin:/bin")]);
    assert_eq!(v["self_install"]["on_path"], false);
    assert_eq!(v["self_install"]["shadowed_by"], Value::Null);
    assert!(
        stderr(&out).contains(&format!(
            "echo 'export PATH=\"{}:$PATH\"' >> ~/.zshrc",
            f.dir().display()
        )),
        "{}",
        stderr(&out)
    );
}

#[test]
fn setup_is_offered_only_with_a_terminal_and_never_with_yes() {
    let f = Fixture::new();
    for (args, env) in [
        (vec![], vec![]),
        (vec!["--yes"], vec![("MAILTRIAGE_TEST_TERMINAL", "1")]),
        (vec!["--no-setup"], vec![("MAILTRIAGE_TEST_TERMINAL", "1")]),
    ] {
        let (out, v) = f.install(&args, "\n", &env);
        assert_eq!(out.status.code(), Some(0), "{v}");
        assert_eq!(v["self_install"]["setup"], "skipped");
        assert!(
            !stderr(&out).contains("Run mailtriage setup now?"),
            "{args:?}"
        );
        assert!(stderr(&out).contains("Next: run "), "{args:?}");
    }
}

#[test]
fn setup_runs_from_the_installed_binary_then_the_login_item_uses_its_config() {
    let f = Fixture::new();
    // An existing config, so setup's interactive run updates it.
    let (out, v) = f.run_program(
        &f.root.join("download/mailtriage"),
        &[
            "setup",
            "--yes",
            "--json",
            "--himalaya-account",
            "work",
            "--provider",
            "fake",
        ],
        "",
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    let config = v["setup"]["config"].as_str().unwrap().to_owned();
    let tray = f.root.join("download/mailtriage-tray");
    write_exe(&tray, &fake_tray(RUNNING));
    // Yes to setup; Enter for every setup question, the service (yes)
    // included; yes to the login item.
    let (out, v) = f.install(
        &["--tray-file", tray.to_str().unwrap()],
        &"\n".repeat(20),
        &[("MAILTRIAGE_TEST_TERMINAL", "1")],
    );
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    let s = &v["self_install"];
    assert_eq!(
        (s["setup"].clone(), s["autostart"].clone()),
        (json!("ran"), json!("enabled"))
    );
    assert!(stderr(&out).contains("Run mailtriage setup now? [Y/n]"));
    assert!(stderr(&out).contains("Start the tray at login? [Y/n]"));
    let login = fs::read_to_string(f.dir().join("tray.log")).unwrap();
    assert_eq!(
        login.trim_end(),
        format!(
            "autostart enable --config {config} --mailtriage {} --json",
            f.installed().display()
        )
    );
    // The download directory is gone; the service runs the installed file.
    fs::remove_dir_all(f.root.join("download")).unwrap();
    let manager = service_files::platform_manager().unwrap_or(Manager::Systemd);
    let services = service_files::list(manager, &f.root.join("home"));
    assert_eq!(services.len(), 1, "{}", stderr(&out));
    assert_eq!(
        services[0].executable.as_deref(),
        Some(f.installed().as_path())
    );
    let (out, v) = f.run_program(
        &f.installed(),
        &[
            "service",
            "status",
            "--account",
            "work",
            "--json",
            "--config",
            &config,
        ],
        "",
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    assert_eq!(v["service"]["installed"], true);
}

/// The old habit of `sudo install … /usr/local/bin`: a root-owned directory
/// passes the protected path rule but is not the user's, so nothing is
/// written there and the message says what to do.
#[test]
fn a_root_owned_directory_is_refused_before_anything_is_written() {
    let f = Fixture::new();
    let (out, v) = f.run_program(
        &f.root.join("download/mailtriage"),
        &[
            "self",
            "install",
            "--json",
            "--no-setup",
            "--dir",
            "/usr/bin",
        ],
        "",
        &[],
    );
    if mailtriage_is_root() {
        return;
    }
    assert_eq!(out.status.code(), Some(3), "{v}");
    assert_eq!(v["error"]["reason"], "unsafe_permissions");
    assert_eq!(
        v["error"]["message"],
        "unsafe_permissions: /usr/bin belongs to uid 0, not to you; use a directory you own, or pass another --dir"
    );
}

fn mailtriage_is_root() -> bool {
    let out = Command::new("id").arg("-u").output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim() == "0"
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test --locked --test self_install`
Expected: FAIL, all 11 tests: `mailtriage self install` exits 2 with `unrecognized subcommand 'self'` (Step 1's `fake_tray` helper compiles on its own).

- [ ] **Step 3: Implement**

First split the transaction (`src/update/install.rs`) and run the updater's tests to see that it keeps its behaviour: `cargo test --locked --test update_install --test update_command --test tray_update --test update_watch` must pass before going on.

In `src/update/install.rs`, replace:

```rust
    if hex(&Sha256::digest(&bytes)) != expected {
        bail!("checksum mismatch for {}", asset.name);
    }
    // 7. Unpack into an exclusively created file.
    let binary = archive::extract(&bytes, job.component.name, &archive::RELEASE)
        .map_err(|e| anyhow!("{}: {e}", asset.name))?;
    drop(bytes);
    let mut scratch = Scratch(Vec::new());
    let staged = scratch.add(dir.join(format!("{TEMP_PREFIX}{}.tmp", uuid::Uuid::new_v4())));
    write_staged(&staged, &binary)
        .map_err(|e| anyhow!("cannot write the new binary into {}: {e}", dir.display()))?;
    let staged_identity = FileIdentity::read(&staged)?;
    // 8. Smoke test.
    match platform::probe(&staged, job.component) {
        Ok(found) if found == candidate => {}
        Ok(found) => {
            bail!("the new binary does not run here: printed version {found}, expected {candidate}")
        }
        Err(cause) => bail!("the new binary does not run here: {cause}"),
    }
    // 9.1 Revalidate.
    job.hooks.at("revalidate")?;
    if FileIdentity::read(path).ok() != Some(installed_identity)
        || FileIdentity::read(&staged).ok() != Some(staged_identity)
    {
        bail!("the installed binary changed during the update; try again");
    }
    // 9.2 Backup copy; `.previous` is not touched yet.
    let backup = scratch.add(dir.join(format!("{TEMP_PREFIX}{}.prev", uuid::Uuid::new_v4())));
    job.hooks
        .at("backup")
        .and_then(|_| copy_or_link(path, &backup).map_err(Into::into))
        .map_err(|e| anyhow!("cannot back up {}: {e}", path.display()))?;
    // 9.3 The commit point.
    job.hooks
        .at("commit")
        .and_then(|_| fs::rename(&staged, path).map_err(Into::into))
        .map_err(|e| anyhow!("cannot replace {}: {e}", path.display()))?;
    scratch.keep();
    let mut warnings = Vec::new();
    // 9.4 Publish the backup.
    let previous = previous_path(path);
    let previous_path = match job
        .hooks
        .at("publish")
        .and_then(|_| fs::rename(&backup, &previous).map_err(Into::into))
    {
        Ok(()) => previous,
        Err(e) => {
            warnings.push(format!(
                "installed; the previous binary stays at {} because {} could not be replaced: {e}",
                backup.display(),
                previous.display()
            ));
            backup
        }
    };
    // 9.5 Make the renames durable.
    if let Err(e) = job
        .hooks
        .at("sync_dir")
        .and_then(|_| File::open(dir)?.sync_all().map_err(Into::into))
```

with:

```rust
    if hex(&Sha256::digest(&bytes)) != expected {
        bail!("checksum mismatch for {}", asset.name);
    }
    // 7. Unpack, then place it.
    let binary = archive::extract(&bytes, job.component.name, &archive::RELEASE)
        .map_err(|e| anyhow!("{}: {e}", asset.name))?;
    drop(bytes);
    let placement = Placement {
        component: job.component,
        path,
        before: Some(installed_identity),
        version: &candidate,
        cache: job.cache,
        hooks: job.hooks,
    };
    let placed = place(&placement, _lock, &binary)?;
    Ok(Outcome::Installed(Installed {
        from: installed,
        to: candidate,
        previous_path: placed.previous_path.unwrap_or_else(|| previous_path(path)),
        warnings: placed.warnings,
    }))
}

/// What `place` puts where: an installation path, what it held when its
/// version was read, and the version the new binary must print.
pub struct Placement<'a> {
    pub component: Component,
    /// The installation path; the held installation lock is its directory's.
    pub path: &'a Path,
    /// The installation path's identity when its version was read; `None`
    /// when there was no file, so there is nothing to back up.
    pub before: Option<FileIdentity>,
    /// The new binary must print `<component> <version>`.
    pub version: &'a Version,
    /// Where step 10 records the installation; `None` records nothing.
    pub cache: Option<&'a Cache>,
    pub hooks: &'a dyn Hooks,
}

/// What `place` did from its commit point on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    /// Where the previous binary is: `<path>.previous`, or the temporary
    /// backup when publishing it failed; `None` without a previous binary.
    pub previous_path: Option<PathBuf>,
    /// Problems after the commit point; the binary counts as installed.
    pub warnings: Vec<String>,
}

/// The new binary does not run here (step 8, the smoke test); nothing was
/// replaced. The cause: `could not start`, `printed version X, expected Y`, …
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoesNotRun(pub String);

impl std::fmt::Display for DoesNotRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the new binary does not run here: {}", self.0)
    }
}

impl std::error::Error for DoesNotRun {}

/// Steps 7 to 10 for `binary` (already verified), under the held
/// installation lock: write it to an exclusively created file next to the
/// installation path, smoke-test it, revalidate both files, back up the
/// installed binary when there is one, rename the new one over the path
/// (the commit point), publish the backup as `<path>.previous`, sync the
/// directory and record the installation. Errors before the commit point
/// leave the path and `<path>.previous` untouched and remove this attempt's
/// files; a binary that does not run is a `DoesNotRun` error.
pub fn place(p: &Placement, _lock: &InstallLock, binary: &[u8]) -> Result<Placed> {
    let path = p.path;
    let dir = path
        .parent()
        .context("the installation path has no directory")?;
    let mut scratch = Scratch(Vec::new());
    let staged = scratch.add(dir.join(format!("{TEMP_PREFIX}{}.tmp", uuid::Uuid::new_v4())));
    write_staged(&staged, binary)
        .map_err(|e| anyhow!("cannot write the new binary into {}: {e}", dir.display()))?;
    let staged_identity = FileIdentity::read(&staged)?;
    // 8. Smoke test.
    match platform::probe(&staged, p.component) {
        Ok(found) if found == *p.version => {}
        Ok(found) => {
            return Err(
                DoesNotRun(format!("printed version {found}, expected {}", p.version)).into(),
            )
        }
        Err(cause) => return Err(DoesNotRun(cause).into()),
    }
    // 9.1 Revalidate.
    p.hooks.at("revalidate")?;
    if FileIdentity::read(path).ok() != p.before
        || FileIdentity::read(&staged).ok() != Some(staged_identity)
    {
        bail!("the installed binary changed during the update; try again");
    }
    // 9.2 Backup copy; `.previous` is not touched yet.
    let backup = match p.before {
        Some(_) => {
            let backup =
                scratch.add(dir.join(format!("{TEMP_PREFIX}{}.prev", uuid::Uuid::new_v4())));
            p.hooks
                .at("backup")
                .and_then(|_| copy_or_link(path, &backup).map_err(Into::into))
                .map_err(|e| anyhow!("cannot back up {}: {e}", path.display()))?;
            Some(backup)
        }
        None => None,
    };
    // 9.3 The commit point.
    p.hooks
        .at("commit")
        .and_then(|_| fs::rename(&staged, path).map_err(Into::into))
        .map_err(|e| anyhow!("cannot replace {}: {e}", path.display()))?;
    scratch.keep();
    let mut warnings = Vec::new();
    // 9.4 Publish the backup.
    let previous_path = backup.map(|backup| {
        let previous = previous_path(path);
        match p
            .hooks
            .at("publish")
            .and_then(|_| fs::rename(&backup, &previous).map_err(Into::into))
        {
            Ok(()) => previous,
            Err(e) => {
                warnings.push(format!(
                    "installed; the previous binary stays at {} because {} could not be replaced: {e}",
                    backup.display(),
                    previous.display()
                ));
                backup
            }
        }
    });
    // 9.5 Make the renames durable.
    if let Err(e) = p
        .hooks
        .at("sync_dir")
        .and_then(|_| File::open(dir)?.sync_all().map_err(Into::into))
```

In `src/update/install.rs`, replace:

```rust
        warnings.push(format!("installed; syncing {} failed: {e}", dir.display()));
    }
    // 10. Record.
    if let Some(cache) = job.cache {
        let key = install_key(path);
        let identity = FileIdentity::read(path).ok();
        let recorded = job.hooks.at("record").and_then(|_| {
            cache.update(|c| {
                let entry = c.installs.entry(key).or_default();
                entry.version = Some(candidate.to_string());
                entry.at = Some(schedule::stamp(Utc::now()));
                entry.last_error = None;
                entry.failures = 0;
```

with:

```rust
        warnings.push(format!("installed; syncing {} failed: {e}", dir.display()));
    }
    // 10. Record.
    if let Some(cache) = p.cache {
        let key = install_key(path);
        let identity = FileIdentity::read(path).ok();
        let recorded = p.hooks.at("record").and_then(|_| {
            cache.update(|c| {
                let entry = c.installs.entry(key).or_default();
                entry.version = Some(p.version.to_string());
                entry.at = Some(schedule::stamp(Utc::now()));
                entry.last_error = None;
                entry.failures = 0;
```

In `src/update/install.rs`, replace:

```rust
            warnings.push(format!("installed; recording the update failed: {e:#}"));
        }
    }
    Ok(Outcome::Installed(Installed {
        from: installed,
        to: candidate,
        previous_path,
        warnings,
    }))
}

/// `<path>.previous`.
```

with:

```rust
            warnings.push(format!("installed; recording the update failed: {e:#}"));
        }
    }
    Ok(Placed {
        previous_path,
        warnings,
    })
}

/// `<path>.previous`.
```

In `src/prompt.rs`, replace:

```rust
        }
    }

    /// A numbered menu (1-based on screen); returns the 0-based index.
    pub fn choose(&mut self, question: &str, options: &[String], default: usize) -> Result<usize> {
        self.menu(question, options, &[default]);
```

with:

```rust
        }
    }

    /// `confirm`, except that the end of input answers `default`.
    pub fn confirm_or(&mut self, question: &str, default: bool) -> bool {
        self.confirm(question, default).unwrap_or(default)
    }

    /// A numbered menu (1-based on screen); returns the 0-based index.
    pub fn choose(&mut self, question: &str, options: &[String], default: usize) -> Result<usize> {
        self.menu(question, options, &[default]);
```

Create `src/distribution/self_install.rs`:

```rust
//! `mailtriage self install --dir DIR`: installs this binary (and a tray)
//! into DIR with the updater's install transaction, reports PATH, and offers
//! setup and the tray's login item. Every program it runs is `DIR/mailtriage`
//! or `DIR/mailtriage-tray`, never a bare name from `PATH`.
use super::protected::{self, Unsafe, Why};
use crate::{
    config,
    prompt::Prompter,
    service::{err, err_kind, ErrorKind},
    setup::shell_line,
    update::{
        cache::Cache,
        github,
        install::{self, DoesNotRun, Hooks, Placement},
        platform::{self, FileIdentity},
        version, CLI, TRAY,
    },
};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    ffi::{OsStr, OsString},
    fs,
    io::{self, IsTerminal, Read},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Debug builds only: treat stdin as a terminal, so tests can answer the
/// prompts through a pipe. Release builds never read it.
pub const TEST_TERMINAL: &str = "MAILTRIAGE_TEST_TERMINAL";
/// Printed when the tray does not run on Linux.
pub const TRAY_PACKAGES: &str = "the tray needs GTK 3 and the Ayatana AppIndicator library (Debian/Ubuntu: sudo apt install libgtk-3-0 libayatana-appindicator3-1 libxdo3)";

/// `self install`'s flags.
#[derive(Debug, Clone, Default)]
pub struct Args {
    pub dir: PathBuf,
    pub tray_file: Option<PathBuf>,
    pub no_setup: bool,
    pub yes: bool,
}

/// Whether stdin is a terminal (or, in debug builds, `TEST_TERMINAL` is 1).
pub fn terminal() -> bool {
    #[cfg(debug_assertions)]
    if std::env::var_os(TEST_TERMINAL).is_some_and(|v| v == "1") {
        return true;
    }
    io::stdin().is_terminal()
}

/// Runs `self install`. Errors carry exit codes: 2 for a refused
/// downgrade, 3 for an unsafe directory (reason `unsafe_permissions`) or a
/// failed CLI transaction, 5 when the installation lock is held. A failed
/// tray transaction or a failed setup leaves the binaries installed: the
/// result then carries `exit_code` 3, which the CLI strips.
pub fn run(args: &Args, hooks: &dyn Hooks, p: &mut Prompter) -> Result<Value> {
    let cwd = std::env::current_dir().map_err(|e| err(2, format!("--dir: {e}")))?;
    let dir = install_dir(&cwd.join(&args.dir))?;
    let lock = install::lock(&dir, install::update_lock_wait())
        .map_err(|e| err(3, format!("{e:#}")))?
        .ok_or_else(|| err(5, "another update is running"))?;
    let running = version::running();
    let cli = dir.join(CLI.name);
    let before = regular_file(&cli)?;
    if before.is_some() {
        if let Ok(found) = platform::probe(&cli, CLI) {
            if version::is_newer(&found, &running) {
                return Err(err(
                    2,
                    format!(
                        "{} is {found}, newer than {running}; to go back, follow the guide's rollback steps",
                        cli.display()
                    ),
                ));
            }
        }
    }
    let cache = Cache::for_user();
    let own = own_binary()?;
    let placement = Placement {
        component: CLI,
        path: &cli,
        before,
        version: &running,
        cache: cache.as_ref(),
        hooks,
    };
    let placed = install::place(&placement, &lock, &own).map_err(|e| err(3, format!("{e:#}")))?;
    drop(own);
    for warning in &placed.warnings {
        p.say(&format!("mailtriage: {warning}"));
    }
    p.say(&format!(
        "Installed mailtriage {running} at {}.",
        cli.display()
    ));
    let mut failed = false;
    let tray = args.tray_file.as_deref().map(|file| {
        let result = install_tray(&dir, file, &running, cache.as_ref(), hooks, &lock, p);
        failed |= result["action"] == "failed";
        result
    });
    drop(lock);
    let tray_installed = tray.as_ref().is_some_and(|t| t["action"] == "installed");
    // PATH.
    let report = path_report(
        &dir,
        std::env::var_os("PATH").as_deref(),
        std::env::var_os("SHELL").as_deref(),
        cfg!(target_os = "macos"),
    );
    for line in &report.lines {
        p.say(line);
    }
    // Setup, then the login item.
    let asking = !args.no_setup && !args.yes && terminal();
    let setup_line = shell_line(&[cli.as_os_str(), OsStr::new("setup")]);
    let (setup, config) = if asking && p.confirm_or("Run mailtriage setup now?", true) {
        match run_setup(&cli) {
            Ok(config) => ("ran", Some(config)),
            Err(message) => {
                p.say(&format!("mailtriage: setup failed: {message}"));
                failed = true;
                ("failed", None)
            }
        }
    } else {
        p.say(&format!("Next: run {setup_line}"));
        ("skipped", None)
    };
    let tray_exe = dir.join(TRAY.name);
    let autostart = match (&config, tray_installed) {
        (Some(config), true) => {
            if p.confirm_or("Start the tray at login?", true) {
                match enable_login_item(&tray_exe, config, &cli) {
                    Ok(()) => "enabled",
                    Err(message) => {
                        p.say(&format!("mailtriage: start at login failed: {message}"));
                        "failed"
                    }
                }
            } else {
                "skipped"
            }
        }
        (None, true) if setup != "failed" => {
            let config = default_config().unwrap_or_else(|| PathBuf::from("CONFIG"));
            p.say(&format!(
                "To start the tray at login after setup: {}",
                login_item_line(&tray_exe, &config, &cli)
            ));
            "skipped"
        }
        _ => "skipped",
    };
    let mut out = json!({"schema_version": 1, "self_install": {
        "dir": dir,
        "cli": {"action": "installed", "version": running.to_string(), "path": cli},
        "tray": tray,
        "on_path": report.on_path,
        "shadowed_by": report.shadowed_by,
        "setup": setup,
        "autostart": autostart,
    }});
    if failed {
        out["exit_code"] = json!(3);
    }
    Ok(out)
}

/// DIR, absolute: created when missing (mode 0755 whatever the umask),
/// canonicalized, owned by this user, and passing the protected path rule.
fn install_dir(dir: &Path) -> Result<PathBuf> {
    let refuse = |found: &Unsafe| {
        err_kind(
            3,
            ErrorKind::UnsafePermissions,
            format!(
                "unsafe_permissions: {found}; {}, or pass another --dir",
                found.fix()
            ),
        )
    };
    let dir = protected::prepare(dir).map_err(|e| match protected::unsafe_dir(&e) {
        Some(found) => refuse(found),
        None => err(3, format!("--dir: {e:#}")),
    })?;
    let owner = fs::metadata(&dir)
        .map_err(|e| err(3, format!("cannot read {}: {e}", dir.display())))?
        .uid();
    if owner != crate::system_service::current_uid() {
        return Err(refuse(&Unsafe {
            dir,
            why: Why::NotYours(owner),
        }));
    }
    Ok(dir)
}

/// The identity of the regular file at `path`; `None` when there is none.
/// Anything else there (a link, a directory) is refused: exit 3.
fn regular_file(path: &Path) -> Result<Option<FileIdentity>> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() => Ok(Some(FileIdentity::of(&meta))),
        Ok(_) => Err(err(
            3,
            format!(
                "{} is not a regular file; move it away first",
                path.display()
            ),
        )),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(err(3, format!("cannot read {}: {e}", path.display()))),
    }
}

/// The bytes of the binary this process runs (at most 200 MB): on Linux
/// `/proc/self/exe`, which is the running image even after a replacement.
fn own_binary() -> Result<Vec<u8>> {
    let path = if cfg!(target_os = "linux") {
        PathBuf::from("/proc/self/exe")
    } else {
        std::env::current_exe().map_err(|e| err(3, format!("cannot find this program: {e}")))?
    };
    read_capped(&path).map_err(|e| err(3, format!("cannot read {}: {e}", path.display())))
}

fn read_capped(path: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(github::MAX_ASSET_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > github::MAX_ASSET_BYTES {
        return Err(io::Error::other("it is larger than 200 MB"));
    }
    Ok(bytes)
}

/// The tray's transaction, after the CLI's commit and under the same lock:
/// `{"action": "installed"|"skipped"|"failed", "error"}`. A tray that does
/// not run here is skipped and any existing one kept.
fn install_tray(
    dir: &Path,
    file: &Path,
    running: &semver::Version,
    cache: Option<&Cache>,
    hooks: &dyn Hooks,
    lock: &install::InstallLock,
    p: &mut Prompter,
) -> Value {
    let path = dir.join(TRAY.name);
    let result = (|| -> Result<()> {
        let before = regular_file(&path)?;
        let meta = fs::symlink_metadata(file)
            .map_err(|e| err(3, format!("--tray-file: {}: {e}", file.display())))?;
        if !meta.is_file() {
            return Err(err(
                3,
                format!("--tray-file: {} is not a regular file", file.display()),
            ));
        }
        let bytes = read_capped(file)
            .map_err(|e| err(3, format!("--tray-file: {}: {e}", file.display())))?;
        let placement = Placement {
            component: TRAY,
            path: &path,
            before,
            version: running,
            cache,
            hooks,
        };
        let placed = install::place(&placement, lock, &bytes)?;
        for warning in placed.warnings {
            p.say(&format!("mailtriage-tray: {warning}"));
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            p.say(&format!("Installed mailtriage-tray at {}.", path.display()));
            json!({"action": "installed", "error": null})
        }
        Err(e) => match e.downcast_ref::<DoesNotRun>() {
            Some(cause) => {
                let message = TRAY.does_not_run(&cause.0);
                p.say(&format!("Skipped the tray: {message}"));
                if cfg!(target_os = "linux") {
                    p.say(TRAY_PACKAGES);
                }
                json!({"action": "skipped", "error": message})
            }
            None => {
                let message = format!("{e:#}");
                p.say(&format!("mailtriage-tray: {message}"));
                json!({"action": "failed", "error": message})
            }
        },
    }
}

/// What `self install` says about PATH.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathReport {
    /// DIR is a directory of PATH.
    pub on_path: bool,
    /// Another `mailtriage` that PATH finds before DIR's.
    pub shadowed_by: Option<PathBuf>,
    /// The lines to print.
    pub lines: Vec<String>,
}

/// The PATH report for DIR (canonical) with `PATH`, the user's `SHELL`, and
/// whether this is macOS (bash reads `~/.bash_profile` there).
pub fn path_report(
    dir: &Path,
    path: Option<&OsStr>,
    shell: Option<&OsStr>,
    macos: bool,
) -> PathReport {
    let dirs: Vec<PathBuf> = path
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();
    let is_dir = |d: &PathBuf| fs::canonicalize(d).is_ok_and(|d| d == dir);
    let on_path = dirs.iter().any(is_dir);
    let own = dir.join(CLI.name);
    let shadowed_by = dirs
        .iter()
        .take_while(|d| !is_dir(d))
        .filter(|d| !d.as_os_str().is_empty())
        .map(|d| d.join(CLI.name))
        .find(|candidate| {
            executable(candidate) && fs::canonicalize(candidate).ok().as_deref() != Some(&*own)
        });
    let mut lines = Vec::new();
    if !on_path {
        lines.push(format!(
            "{} is not on your PATH. For new terminals, run: {}",
            dir.display(),
            path_line(dir, shell, macos)
        ));
    }
    if let Some(other) = &shadowed_by {
        lines.push(format!(
            "Your PATH finds {} first: it stays in use and is not updated by this install.",
            other.display()
        ));
    }
    PathReport {
        on_path,
        shadowed_by,
        lines,
    }
}

/// The line that puts DIR on PATH for the user's shell: zsh appends to
/// `~/.zshrc`, bash to `~/.bashrc` (`~/.bash_profile` on macOS), fish runs
/// `fish_add_path`, any other shell gets a POSIX `export`.
pub fn path_line(dir: &Path, shell: Option<&OsStr>, macos: bool) -> String {
    let shell = shell
        .and_then(|s| Path::new(s).file_name())
        .and_then(OsStr::to_str)
        .unwrap_or("");
    let export = format!("export PATH=\"{}:$PATH\"", double_quoted(dir));
    let append = |file: &str| format!("echo {} >> {file}", single_quoted(&export));
    match shell {
        "zsh" => append("~/.zshrc"),
        "bash" if macos => append("~/.bash_profile"),
        "bash" => append("~/.bashrc"),
        "fish" => shell_line(&[OsStr::new("fish_add_path"), dir.as_os_str()]),
        _ => export,
    }
}

/// `dir` for the inside of double quotes: `\`, `"`, `$` and `` ` `` escaped.
fn double_quoted(dir: &Path) -> String {
    let mut out = String::new();
    for c in dir.to_string_lossy().chars() {
        if matches!(c, '\\' | '"' | '$' | '`') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `text` as one single-quoted shell word.
fn single_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Runs the installed `DIR/mailtriage setup --interactive --json` with this
/// terminal as stdin and stderr; returns the config it wrote, or its error
/// message.
fn run_setup(cli: &Path) -> Result<PathBuf, String> {
    let out = Command::new(cli)
        .args(["setup", "--interactive", "--json"])
        .stdin(Stdio::inherit())
        .stderr(Stdio::inherit())
        .stdout(Stdio::piped())
        .output()
        .map_err(|e| format!("cannot start {}: {e}", cli.display()))?;
    let value: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    if out.status.success() {
        return value["setup"]["config"]
            .as_str()
            .map(PathBuf::from)
            .ok_or_else(|| "setup printed no config".to_owned());
    }
    Err(value["error"]["message"].as_str().map_or_else(
        || format!("setup exited with {}", out.status),
        str::to_owned,
    ))
}

/// The login item command for the tray at `tray`.
fn login_item_line(tray: &Path, config: &Path, cli: &Path) -> String {
    shell_line(&login_item_args(tray, config, cli))
}

fn login_item_args(tray: &Path, config: &Path, cli: &Path) -> Vec<OsString> {
    vec![
        tray.as_os_str().to_owned(),
        "autostart".into(),
        "enable".into(),
        "--config".into(),
        config.as_os_str().to_owned(),
        "--mailtriage".into(),
        cli.as_os_str().to_owned(),
    ]
}

/// Runs `DIR/mailtriage-tray autostart enable --config CONFIG --mailtriage
/// DIR/mailtriage`; its error message on failure.
fn enable_login_item(tray: &Path, config: &Path, cli: &Path) -> Result<(), String> {
    let args = login_item_args(tray, config, cli);
    let out = Command::new(&args[0])
        .args(&args[1..])
        .arg("--json")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot start {}: {e}", tray.display()))?;
    if out.status.success() {
        return Ok(());
    }
    let value: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    Err(value["error"]["message"]
        .as_str()
        .map_or_else(|| format!("it exited with {}", out.status), str::to_owned))
}

/// The config setup writes by default: `MAILTRIAGE_CONFIG`, else
/// `~/.config/mailtriage/mailtriage.json`.
fn default_config() -> Option<PathBuf> {
    config::setup_path(
        None,
        std::env::var_os("MAILTRIAGE_CONFIG").as_deref(),
        std::env::var_os("HOME").map(PathBuf::from).as_deref(),
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_line_fits_the_shell() {
        let dir = Path::new("/Users/a/.local/bin");
        let line = |shell: &str, macos| path_line(dir, Some(OsStr::new(shell)), macos);
        assert_eq!(
            line("/bin/zsh", true),
            r#"echo 'export PATH="/Users/a/.local/bin:$PATH"' >> ~/.zshrc"#
        );
        assert_eq!(
            line("/bin/bash", true),
            r#"echo 'export PATH="/Users/a/.local/bin:$PATH"' >> ~/.bash_profile"#
        );
        assert_eq!(
            line("/usr/bin/bash", false),
            r#"echo 'export PATH="/Users/a/.local/bin:$PATH"' >> ~/.bashrc"#
        );
        assert_eq!(
            line("/usr/local/bin/fish", false),
            "fish_add_path /Users/a/.local/bin"
        );
        assert_eq!(
            line("/bin/dash", false),
            r#"export PATH="/Users/a/.local/bin:$PATH""#
        );
        assert_eq!(
            path_line(dir, None, false),
            r#"export PATH="/Users/a/.local/bin:$PATH""#
        );
        // Characters the shell would interpret stay literal.
        let odd = Path::new("/home/it's \"$x\"/bin");
        assert_eq!(
            path_line(odd, Some(OsStr::new("zsh")), false),
            r#"echo 'export PATH="/home/it'\''s \"\$x\"/bin:$PATH"' >> ~/.zshrc"#
        );
    }
}
```

In `src/distribution/mod.rs`, replace:

```rust
//! Installing mailtriage and what it needs (spec:
//! docs/superpowers/specs/2026-10-07-install-and-distribution-design.md):
//! the protected path rule and a private, tested Himalaya.
pub mod brew;
pub mod himalaya;
pub mod protected;
```

with:

```rust
//! Installing mailtriage and what it needs (spec:
//! docs/superpowers/specs/2026-10-07-install-and-distribution-design.md):
//! the protected path rule, a private tested Himalaya, Homebrew launch
//! paths, and `self install`.
pub mod brew;
pub mod himalaya;
pub mod protected;
pub mod self_install;
```

In `src/cli.rs`, replace:

```rust
        #[command(subcommand)]
        command: HimalayaCommand,
    },
}

#[derive(Subcommand)]
```

with:

```rust
        #[command(subcommand)]
        command: HimalayaCommand,
    },
    /// Install this mailtriage into a directory; needs no config.
    #[command(name = "self")]
    SelfCmd {
        #[command(subcommand)]
        command: SelfCommand,
    },
}

#[derive(Subcommand)]
enum SelfCommand {
    /// Install this binary (and a tray) into DIR with the updater's
    /// transaction, then offer setup and the tray's login item.
    Install(SelfInstallArg),
}

#[derive(Args)]
struct SelfInstallArg {
    /// The directory to install into, created when missing.
    #[arg(long)]
    dir: PathBuf,
    /// A mailtriage-tray binary to install next to mailtriage.
    #[arg(long)]
    tray_file: Option<PathBuf>,
    /// Do not offer setup or the login item.
    #[arg(long)]
    no_setup: bool,
    /// Never prompt.
    #[arg(long)]
    yes: bool,
}

#[derive(Subcommand)]
```

In `src/cli.rs`, replace:

```rust
        }
    };
    match execute(&cli) {
        Ok(value) => {
            let partial = value
                .get("partial")
                .and_then(Value::as_bool)
```

with:

```rust
        }
    };
    match execute(&cli) {
        Ok(mut value) => {
            // A result printed with a failure exit code, such as a `self
            // install` whose setup failed; the key itself is not printed.
            let code = value
                .as_object_mut()
                .and_then(|object| object.remove("exit_code"))
                .and_then(|code| code.as_i64());
            let partial = value
                .get("partial")
                .and_then(Value::as_bool)
```

In `src/cli.rs`, replace:

```rust
            if print_value(&value, cli.json).is_err() {
                return 3;
            }
            if partial {
                4
            } else {
                0
            }
        }
        Err(error) => {
```

with:

```rust
            if print_value(&value, cli.json).is_err() {
                return 3;
            }
            match code {
                Some(code) => code as i32,
                None if partial => 4,
                None => 0,
            }
        }
        Err(error) => {
```

In `src/cli.rs`, replace:

```rust
        Command::Himalaya {
            command: HimalayaCommand::Install(arg),
        } => distribution::himalaya::run(arg.version.as_deref()).map_err(service_error),
    }
}

```

with:

```rust
        Command::Himalaya {
            command: HimalayaCommand::Install(arg),
        } => distribution::himalaya::run(arg.version.as_deref()).map_err(service_error),
        Command::SelfCmd {
            command: SelfCommand::Install(arg),
        } => {
            let args = distribution::self_install::Args {
                dir: arg.dir.clone(),
                tray_file: arg.tray_file.clone(),
                no_setup: arg.no_setup,
                yes: arg.yes,
            };
            let mut prompt = Prompter::new(prompt::stdin_unbuffered(), io::stderr(), true);
            distribution::self_install::run(&args, &update::install::EnvHooks, &mut prompt)
                .map_err(service_error)
        }
    }
}

```

- [ ] **Step 4: Run the new tests**

Run: `cargo test --locked --test self_install --test update_install --test update_command && cargo test --locked --lib distribution::self_install`
Expected: PASS (11 `self_install` tests).

- [ ] **Step 5: Document it**

In `docs/guide.md`, replace:

```markdown
`~/.local/bin` must be on your `PATH`; the examples below assume `mailtriage` is. If `command -v mailtriage` prints nothing, add `export PATH="$HOME/.local/bin:$PATH"` to your shell profile (`~/.zprofile` on macOS, `~/.bashrc` on Linux) and open a new terminal. The background service records the absolute path of the executable that installs it, so install the binary in its final place first.
```

with:

````markdown
`~/.local/bin` must be on your `PATH`; the examples below assume `mailtriage` is. If `command -v mailtriage` prints nothing, add `export PATH="$HOME/.local/bin:$PATH"` to your shell profile (`~/.zprofile` on macOS, `~/.bashrc` on Linux) and open a new terminal. The background service records the absolute path of the executable that installs it, so install the binary in its final place first.

### `mailtriage self install`

The install script runs this command; you or an agent can run it on a binary you placed yourself:

```sh
mailtriage self install --dir DIR [--tray-file PATH] [--no-setup] [--yes] [--json]
```

1. It creates `DIR` and its missing parents with mode 0755. `DIR`, with symlinks resolved, must be yours and not writable by group or others, and each of its parents must pass the rule of [a private Himalaya's](#a-private-himalaya) directory. Otherwise it exits 3 with `unsafe_permissions` and the fix: `chmod go-w DIR`, or another `--dir`.
2. It never goes back: when `DIR/mailtriage` prints a newer version, it exits 2 and points to [Rolling back by hand](#rolling-back-by-hand).
3. It installs this binary as `DIR/mailtriage` the way `mailtriage update` installs a release: under the installation lock, from a new file that must run with `--version`, keeping the old binary as `DIR/mailtriage.previous`, with a rename. Running services switch to it by themselves.
4. With `--tray-file`, it installs that `mailtriage-tray` next to it the same way; it must be the same version. A tray that does not run here (on Linux without GTK, for example) is skipped, any existing tray is kept, and on Linux it names the packages the tray needs.
5. It records both in the update cache, clearing any earlier install error.
6. It says when `DIR` is not on your `PATH`, with the line to add for your shell (zsh, bash, fish, or a POSIX `export`), and when another `mailtriage` comes first on your `PATH`: that one stays in use and is not updated by this install.
7. With a terminal, unless `--no-setup` or `--yes`, it asks `Run mailtriage setup now? [Y/n]` and runs `DIR/mailtriage setup`, so the service it installs runs `DIR/mailtriage`. After a successful setup with the tray installed, it asks `Start the tray at login? [Y/n]` and runs `DIR/mailtriage-tray autostart enable --config CONFIG --mailtriage DIR/mailtriage` with the config setup wrote. Otherwise it prints those commands.

```json
{"schema_version":1,"self_install":{"dir":"/Users/alice/.local/bin","cli":{"action":"installed","version":"0.3.0","path":"/Users/alice/.local/bin/mailtriage"},"tray":{"action":"installed","error":null},"on_path":true,"shadowed_by":null,"setup":"ran","autostart":"enabled"}}
```

- `tray`: `null` without `--tray-file`, else `action` `installed`, `skipped` or `failed`, with the reason in `error`.
- `setup`: `ran`, `skipped` or `failed`. `autostart`: `enabled`, `skipped` or `failed`.

Exit codes: 0; 2 for invalid flags or a refused downgrade; 3 for an unsafe directory, a failed install, a failed tray install, or a failed setup (the binaries stay installed); 5 when another update or install held the installation lock for 60 seconds.
````

- [ ] **Step 6: Full check and commit**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`
Expected: all pass.

```bash
git add docs/guide.md \
  src/cli.rs \
  src/distribution/mod.rs \
  src/distribution/self_install.rs \
  src/prompt.rs \
  src/update/install.rs \
  tests/install_support/mod.rs \
  tests/self_install.rs
git commit -m "Add mailtriage self install

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: `mailtriage self uninstall`

`self uninstall [--dir DIR] [--yes] [--json]` removes only the installation in DIR: Homebrew refused, confirmation (default no; no terminal needs `--yes`), the installation lock held to the end, every marked service whose decoded executable is `DIR/mailtriage` uninstalled under its account's service lock (re-checked under the lock), the tray's login item removed by the CLI itself (booted out on macOS), `DIR/mailtriage-tray quit`, and only then the files and their cache entries.

**Files:**
- Create: `src/distribution/login_item.rs`, `src/distribution/self_uninstall.rs`
- Modify: `src/distribution/mod.rs`, `src/cli.rs`, `docs/guide.md`
- Test: `tests/self_uninstall.rs` (new)

**Interfaces:**
- Consumes: `self_install::terminal` (Task 8); `mailtriage-tray quit --json` (Task 7); `service_files::{list, executable, plist_arguments}`, `service_control::{lock, SERVICE_LOCK_WAIT}`, `system_service::{uninstall, bootout, is_marked}`, `update::install::{lock, previous_path, LOCK_FILE}` (on `main`).
- Produces:
  - `distribution::login_item::{LABEL, path(macos: bool, home: &Path, xdg_config_home: Option<&OsStr>) -> PathBuf, tray_of(macos: bool, text: &str) -> Option<PathBuf>, desktop_arguments(&str) -> Option<Vec<String>>}`.
  - `distribution::self_uninstall::{Args { dir: Option<PathBuf>, yes: bool }, run(&Args, &dyn Hooks, &mut Prompter) -> anyhow::Result<Value>}`; hook point `uninstall_service` (debug builds).
  - CLI: `mailtriage self uninstall …`.

- [ ] **Step 1: Write the failing tests**

Create `tests/self_uninstall.rs`:

```rust
#![cfg(unix)]
//! `mailtriage self uninstall` through the binary, run from a copy in DIR:
//! services, the tray's login item and the running tray of this
//! installation go, then its files; everything else stays. HOME and the
//! cache are temporary; launchctl, systemctl and the tray are fakes, so no
//! real job is touched.
mod common;
mod install_support;
mod update_support;
use common::{write_tool, LAUNCHCTL, SYSTEMCTL};
use fs2::FileExt;
use install_support::{fake_tray, write_exe};
use mailtriage::{
    system_service::{self, Manager, Unit},
    update::service_files,
};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};
use update_support::cache_dir;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

fn manager() -> Manager {
    service_files::platform_manager().expect("macOS or Linux")
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        for d in ["bin", "home", "xdg", "tools", "other"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        fs::set_permissions(root.join("bin"), fs::Permissions::from_mode(0o755)).unwrap();
        let cli = root.join("bin/mailtriage");
        fs::copy(env!("CARGO_BIN_EXE_mailtriage"), &cli).unwrap();
        fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(cli.with_file_name("mailtriage.previous"), "old").unwrap();
        write_exe(&root.join("other/mailtriage"), "#!/bin/sh\n");
        write_tool(&root.join("tools"), "launchctl", LAUNCHCTL);
        write_tool(&root.join("tools"), "systemctl", SYSTEMCTL);
        Self { _dir: dir, root }
    }

    fn cli(&self) -> PathBuf {
        self.root.join("bin/mailtriage")
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    /// A marked service file for `account` that runs `exe`.
    fn service(&self, account: &str, exe: &Path) -> PathBuf {
        let dir = system_service::unit_dir(manager(), &self.home());
        fs::create_dir_all(&dir).unwrap();
        let unit = Unit {
            account: account.into(),
            exe: exe.to_path_buf(),
            config: self.root.join("mailtriage.json"),
            interval_seconds: 60,
            limit: 100,
            log_dir: self.root.join("logs"),
            path_env: None,
        };
        let (path, text) = match manager() {
            Manager::Launchd => (
                dir.join(format!("{}.plist", system_service::label(account))),
                system_service::plist(&unit),
            ),
            Manager::Systemd => (
                dir.join(system_service::unit_name(account)),
                system_service::systemd_unit(&unit),
            ),
        };
        fs::write(&path, text).unwrap();
        path
    }

    /// The tray's login item naming `tray`, as `autostart enable` writes it.
    fn login_item(&self, tray: &Path) -> PathBuf {
        let (path, text) = if cfg!(target_os = "macos") {
            (
                self.home().join("Library/LaunchAgents/digital.wirdrei.mailtriage-tray.plist"),
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\">\n<dict>\n  <key>XMailtriageManaged</key>\n  <true/>\n  <key>Label</key>\n  <string>digital.wirdrei.mailtriage-tray</string>\n  <key>ProgramArguments</key>\n  <array>\n    <string>{}</string>\n    <string>--config</string>\n    <string>/c/mailtriage.json</string>\n    <string>--mailtriage</string>\n    <string>{}</string>\n  </array>\n</dict>\n</plist>\n",
                    tray.display(),
                    self.cli().display()
                ),
            )
        } else {
            (
                self.home().join(".config/autostart/mailtriage-tray.desktop"),
                format!(
                    "# managed by mailtriage\n[Desktop Entry]\nType=Application\nName=mailtriage\nExec={} --config /c/mailtriage.json --mailtriage {}\nNoDisplay=true\n",
                    tray.display(),
                    self.cli().display()
                ),
            )
        };
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }

    fn uninstall(&self, args: &[&str], stdin: &str, env: &[(&str, &str)]) -> (Output, Value) {
        let mut command = Command::new(self.cli());
        command
            .args(["self", "uninstall", "--json"])
            .args(args)
            .current_dir(&self.root)
            .env("HOME", self.home())
            .env("XDG_CACHE_HOME", self.root.join("xdg"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("MT_FAKE_HOME", self.home())
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("tools").display()),
            )
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("MAILTRIAGE_TEST_TERMINAL")
            .env_remove("MAILTRIAGE_UPDATE_TEST_HOOK")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = command.spawn().unwrap();
        let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
        let out = child.wait_with_output().unwrap();
        let value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
        (out, value)
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn only_this_installations_services_and_files_go() {
    let f = Fixture::new();
    let mine = f.service("work", &f.cli());
    let theirs = f.service("home", &f.root.join("other/mailtriage"));
    let brew = f.service(
        "brew",
        Path::new("/opt/homebrew/opt/mailtriage/bin/mailtriage"),
    );
    let cache = cache_dir(&f.home(), &f.root.join("xdg"));
    fs::create_dir_all(&cache).unwrap();
    let other_key = f.root.join("other/mailtriage").to_str().unwrap().to_owned();
    fs::write(
        cache.join("update.json"),
        json!({"schema_version": 1, "installs": {
            f.cli().to_str().unwrap(): {"version": "0.1.0"},
            other_key.clone(): {"version": "0.1.0"},
        }})
        .to_string(),
    )
    .unwrap();
    let (out, v) = f.uninstall(&["--yes"], "", &[]);
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    let s = &v["self_uninstall"];
    assert_eq!(s["dir"], f.root.join("bin").to_str().unwrap());
    assert_eq!(
        s["services"],
        json!([{"account": "work", "unit_path": mine, "action": "uninstalled", "error": null}])
    );
    assert_eq!(s["tray"], "not_installed");
    assert!(!mine.exists());
    assert!(theirs.exists() && brew.exists());
    assert!(!f.cli().exists());
    assert!(!f.root.join("bin/mailtriage.previous").exists());
    assert_eq!(
        s["removed"],
        json!([f.cli(), f.root.join("bin/mailtriage.previous")])
    );
    // The lock file stays, and is listed with what was kept.
    let lock = f.root.join("bin/.mailtriage-update.lock");
    assert!(lock.exists());
    assert_eq!(s["kept"][0], lock.to_str().unwrap());
    let installs: Value =
        serde_json::from_slice(&fs::read(cache.join("update.json")).unwrap()).unwrap();
    assert!(installs["installs"]
        .get(f.cli().to_str().unwrap())
        .is_none());
    assert!(installs["installs"].get(&other_key).is_some());
    assert!(stderr(&out).contains("no mail was touched"));
}

#[test]
fn a_service_that_names_another_executable_once_locked_is_skipped() {
    let f = Fixture::new();
    let unit = f.service("work", &f.cli());
    // Between the listing and the locked re-read, the file is rewritten for
    // another installation.
    let replacement = f.root.join("replacement");
    let other = f.service("work", &f.root.join("other/mailtriage"));
    fs::rename(&other, &replacement).unwrap();
    f.service("work", &f.cli());
    let hook = f.root.join("hook");
    write_exe(
        &hook,
        &format!(
            "#!/bin/sh\n[ \"$1\" = uninstall_service ] && cp '{}' '{}'\nexit 0\n",
            replacement.display(),
            unit.display()
        ),
    );
    let (out, v) = f.uninstall(
        &["--yes"],
        "",
        &[("MAILTRIAGE_UPDATE_TEST_HOOK", hook.to_str().unwrap())],
    );
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    assert_eq!(v["self_uninstall"]["services"][0]["action"], "skipped");
    assert!(unit.exists(), "the other installation's service stays");
}

#[test]
fn a_failing_service_removal_keeps_every_file() {
    let f = Fixture::new();
    if mailtriage_uid() == 0 {
        return;
    }
    let unit = f.service("work", &f.cli());
    let units = unit.parent().unwrap().to_path_buf();
    fs::set_permissions(&units, fs::Permissions::from_mode(0o555)).unwrap();
    let (out, v) = f.uninstall(&["--yes"], "", &[]);
    fs::set_permissions(&units, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(out.status.code(), Some(3), "{v}");
    let s = &v["self_uninstall"];
    assert_eq!(s["services"][0]["action"], "failed");
    assert_eq!(s["removed"], json!([]));
    assert!(!s["failures"].as_array().unwrap().is_empty());
    assert!(f.cli().exists() && unit.exists());
    assert!(stderr(&out).contains("Nothing was removed"));
}

fn mailtriage_uid() -> u32 {
    let out = Command::new("id").arg("-u").output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

#[cfg(target_os = "linux")]
#[test]
fn without_a_service_manager_there_are_no_services() {
    let f = Fixture::new();
    let unit = f.service("work", &f.cli());
    let (out, v) = f.uninstall(&["--yes"], "", &[("PATH", "/nonexistent")]);
    assert_eq!(out.status.code(), Some(0), "{v}");
    assert_eq!(v["self_uninstall"]["services"], json!([]));
    assert!(unit.exists());
}

#[test]
fn a_homebrew_installation_is_refused() {
    let f = Fixture::new();
    let keg = f.root.join("brew/Cellar/mailtriage/1.2.3/bin");
    fs::create_dir_all(&keg).unwrap();
    fs::copy(f.cli(), keg.join("mailtriage")).unwrap();
    let (out, v) = f.uninstall(&["--yes", "--dir", keg.to_str().unwrap()], "", &[]);
    assert_eq!(out.status.code(), Some(2), "{v}");
    assert_eq!(
        v["error"]["message"],
        "installed by Homebrew; run brew uninstall mailtriage"
    );
    assert!(keg.join("mailtriage").exists());
}

#[test]
fn the_tray_quits_and_its_login_item_goes() {
    let f = Fixture::new();
    let tray = f.root.join("bin/mailtriage-tray");
    write_exe(&tray, &fake_tray("0.1.0"));
    let item = f.login_item(&tray);
    // launchd has the login job loaded.
    fs::write(f.root.join("tools/loaded"), "").unwrap();
    let (out, v) = f.uninstall(&["--yes"], "", &[]);
    assert_eq!(out.status.code(), Some(0), "{v} {}", stderr(&out));
    let s = &v["self_uninstall"];
    assert_eq!(s["tray"], "quit");
    assert!(!item.exists());
    assert!(!tray.exists() && !f.cli().exists());
    assert!(s["removed"].as_array().unwrap().contains(&json!(item)));
    if cfg!(target_os = "macos") {
        let log = fs::read_to_string(f.root.join("tools/launchctl.log")).unwrap();
        assert!(
            log.lines().any(|l| l.starts_with("bootout gui/")
                && l.ends_with("/digital.wirdrei.mailtriage-tray")),
            "{log}"
        );
    }
    assert!(stderr(&out).contains("Close any open categories window"));
}

#[test]
fn a_tray_that_does_not_stop_keeps_every_file() {
    let f = Fixture::new();
    let tray = f.root.join("bin/mailtriage-tray");
    write_exe(&tray, &fake_tray("0.1.0"));
    fs::write(
        f.root.join("bin/quit.json"),
        r#"{"schema_version":1,"error":{"code":3,"message":"the tray does not respond; quit it from its menu"}}"#,
    )
    .unwrap();
    fs::write(f.root.join("bin/quit.code"), "3").unwrap();
    let (out, v) = f.uninstall(&["--yes"], "", &[]);
    assert_eq!(out.status.code(), Some(3), "{v}");
    let s = &v["self_uninstall"];
    assert_eq!(s["tray"], "failed");
    assert_eq!(
        s["failures"],
        json!(["tray: the tray does not respond; quit it from its menu"])
    );
    assert!(tray.exists() && f.cli().exists());
}

#[test]
fn another_installations_tray_is_left_alone() {
    let f = Fixture::new();
    let tray = f.root.join("bin/mailtriage-tray");
    write_exe(&tray, &fake_tray("0.1.0"));
    fs::write(
        f.root.join("bin/quit.json"),
        r#"{"schema_version":1,"quit":"other_installation"}"#,
    )
    .unwrap();
    let (out, v) = f.uninstall(&["--yes"], "", &[]);
    assert_eq!(out.status.code(), Some(0), "{v}");
    assert_eq!(v["self_uninstall"]["tray"], "other_installation");
    assert!(!tray.exists());
}

#[test]
fn an_update_holding_the_lock_delays_uninstall_and_the_lock_file_survives() {
    let f = Fixture::new();
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(f.root.join("bin/.mailtriage-update.lock"))
        .unwrap();
    lock.lock_exclusive().unwrap();
    let (out, v) = f.uninstall(
        &["--yes"],
        "",
        &[("MAILTRIAGE_UPDATE_TEST_LOCK_WAIT_MS", "300")],
    );
    assert_eq!(out.status.code(), Some(5), "{v}");
    assert!(f.cli().exists());
    drop(lock);
    let (out, _) = f.uninstall(&["--yes"], "", &[]);
    assert_eq!(out.status.code(), Some(0));
    assert!(f.root.join("bin/.mailtriage-update.lock").exists());
}

#[test]
fn it_asks_first_and_refuses_without_a_terminal() {
    let f = Fixture::new();
    let (out, v) = f.uninstall(&[], "", &[]);
    assert_eq!(out.status.code(), Some(2), "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("--yes"));
    let terminal = [("MAILTRIAGE_TEST_TERMINAL", "1")];
    for answer in ["n\n", "\n", ""] {
        let (out, v) = f.uninstall(&[], answer, &terminal);
        assert_eq!(out.status.code(), Some(2), "{answer:?}: {v}");
        assert_eq!(
            v["error"]["message"],
            "uninstall cancelled; nothing was changed"
        );
        assert!(stderr(&out).contains("[y/N]"));
        assert!(f.cli().exists());
    }
    let (out, v) = f.uninstall(&[], "y\n", &terminal);
    assert_eq!(out.status.code(), Some(0), "{v}");
    assert!(!f.cli().exists());
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test --locked --test self_uninstall`
Expected: FAIL, all 9 tests (10 on Linux): `mailtriage self uninstall` exits 2 with `unrecognized subcommand 'uninstall'`.

- [ ] **Step 3: Implement**

`login_item.rs` carries unit tests that decode the exact plist and `.desktop` texts `tray/tests/autostart.rs` pins for the tray's writers, so both crates read and write the same files.

Create `src/distribution/login_item.rs`:

```rust
//! The tray's login item as `mailtriage-tray autostart enable` writes it
//! (tray/src/autostart.rs): a launchd agent on macOS, an XDG autostart
//! entry on Linux, both marked. `self uninstall` reads and removes it
//! itself, so it needs no tray binary.
use crate::update::service_files::plist_arguments;
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

/// The launchd label of the tray's login item.
pub const LABEL: &str = "digital.wirdrei.mailtriage-tray";
const PLIST_MARKER: &str = "<key>XMailtriageManaged</key>";
const DESKTOP_MARKER: &str = "# managed by mailtriage";

/// Where the login item lives: `~/Library/LaunchAgents/<label>.plist` on
/// macOS; on Linux `$XDG_CONFIG_HOME/autostart/mailtriage-tray.desktop`
/// when `XDG_CONFIG_HOME` is absolute, else under `~/.config`.
pub fn path(macos: bool, home: &Path, xdg_config_home: Option<&OsStr>) -> PathBuf {
    if macos {
        return home
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist"));
    }
    xdg_config_home
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"))
        .join("autostart/mailtriage-tray.desktop")
}

/// The tray program a login item's `text` starts: its first argument.
/// `None` without mailtriage's marker or when it cannot be decoded.
pub fn tray_of(macos: bool, text: &str) -> Option<PathBuf> {
    let arguments = if macos {
        if !text.contains(PLIST_MARKER) {
            return None;
        }
        plist_arguments(text)?
    } else {
        if text.lines().next() != Some(DESKTOP_MARKER) {
            return None;
        }
        desktop_arguments(text.lines().find_map(|l| l.strip_prefix("Exec="))?)?
    };
    arguments.into_iter().next().map(PathBuf::from)
}

/// The arguments of an `Exec=` value: the inverse of the tray's
/// `desktop_arg` (the string-level `\` escape, then the Desktop Entry
/// quoting, then `%%` for `%`).
pub fn desktop_arguments(value: &str) -> Option<Vec<String>> {
    let mut unescaped = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            unescaped.push(chars.next()?);
        } else {
            unescaped.push(c);
        }
    }
    let mut args = vec![];
    let mut chars = unescaped.chars().peekable();
    loop {
        while chars.peek() == Some(&' ') {
            chars.next();
        }
        let Some(&first) = chars.peek() else {
            break;
        };
        let mut arg = String::new();
        if first == '"' {
            chars.next();
            loop {
                match chars.next()? {
                    '\\' => arg.push(chars.next()?),
                    '"' => break,
                    c => arg.push(c),
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c == ' ' {
                    break;
                }
                arg.push(c);
                chars.next();
            }
        }
        args.push(arg.replace("%%", "%"));
    }
    Some(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact files tray/tests/autostart.rs pins for the tray's writers.
    #[test]
    fn the_trays_login_items_decode_to_the_tray() {
        let plist = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\">\n<dict>\n  <key>XMailtriageManaged</key>\n  <true/>\n  <key>Label</key>\n  <string>digital.wirdrei.mailtriage-tray</string>\n  <key>ProgramArguments</key>\n  <array>\n    <string>/Apps/mail tray/mailtriage-tray</string>\n    <string>--config</string>\n    <string>/Users/a &amp; b/50% $HOME \"x\"/mailtriage.json</string>\n    <string>--mailtriage</string>\n    <string>/opt/mail triage/mailtriage</string>\n  </array>\n</dict>\n</plist>\n";
        assert_eq!(
            tray_of(true, plist),
            Some(PathBuf::from("/Apps/mail tray/mailtriage-tray"))
        );
        let desktop = "# managed by mailtriage\n[Desktop Entry]\nType=Application\nName=mailtriage\nExec=\"/Apps/mail tray/mailtriage-tray\" --config \"/Users/a & b/50%% \\\\$HOME \\\\\"x\\\\\"/mailtriage.json\" --mailtriage \"/opt/mail triage/mailtriage\"\nNoDisplay=true\n";
        assert_eq!(
            tray_of(false, desktop),
            Some(PathBuf::from("/Apps/mail tray/mailtriage-tray"))
        );
        let exec = desktop
            .lines()
            .find_map(|l| l.strip_prefix("Exec="))
            .unwrap();
        assert_eq!(
            desktop_arguments(exec).unwrap(),
            [
                "/Apps/mail tray/mailtriage-tray",
                "--config",
                "/Users/a & b/50% $HOME \"x\"/mailtriage.json",
                "--mailtriage",
                "/opt/mail triage/mailtriage"
            ]
        );
        // Files mailtriage did not write are not its login item.
        assert_eq!(
            tray_of(true, &plist.replace("XMailtriageManaged", "Other")),
            None
        );
        assert_eq!(
            tray_of(false, &desktop.replacen("# managed by mailtriage\n", "", 1)),
            None
        );
    }

    #[test]
    fn the_login_item_path_follows_the_platform() {
        let home = Path::new("/h");
        assert_eq!(
            path(true, home, None),
            PathBuf::from("/h/Library/LaunchAgents/digital.wirdrei.mailtriage-tray.plist")
        );
        assert_eq!(
            path(false, home, Some(OsStr::new("/x"))),
            PathBuf::from("/x/autostart/mailtriage-tray.desktop")
        );
        assert_eq!(
            path(false, home, Some(OsStr::new("rel"))),
            PathBuf::from("/h/.config/autostart/mailtriage-tray.desktop")
        );
    }
}
```

Create `src/distribution/self_uninstall.rs`:

```rust
//! `mailtriage self uninstall [--dir DIR]`: removes only the installation
//! in DIR (default: the running executable's directory): its services, the
//! tray's login item and running tray, then its programs. Files are removed
//! only when every service and tray step succeeded.
use super::{himalaya, login_item, self_install};
use crate::{
    process::Ending,
    prompt::Prompter,
    service::err,
    service_control,
    setup::shell_line,
    system_service::{self, Context, Manager},
    update::{
        cache::{install_key, Cache},
        install::{self, Hooks},
        platform, service_files, CLI, TRAY,
    },
};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

/// How long `mailtriage-tray quit` may take: its answer and the wait for
/// the tray's lock are 5 s each.
const QUIT_TIMEOUT: Duration = Duration::from_secs(30);

/// `self uninstall`'s flags.
#[derive(Debug, Clone, Default)]
pub struct Args {
    pub dir: Option<PathBuf>,
    pub yes: bool,
}

/// Runs `self uninstall`. Errors carry exit codes: 2 for a Homebrew
/// installation, a refused confirmation, or no terminal without `--yes`;
/// 5 when the installation lock is held for 60 s. A failed service or tray
/// step removes no file: the result then carries `exit_code` 3, which the
/// CLI strips.
pub fn run(args: &Args, hooks: &dyn Hooks, p: &mut Prompter) -> Result<Value> {
    let dir = match &args.dir {
        Some(dir) => std::env::current_dir()
            .map_err(|e| err(2, format!("--dir: {e}")))?
            .join(dir),
        None => platform::installation_path()
            .map_err(|e| err(2, e))?
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| {
                err(
                    2,
                    "cannot tell which directory this mailtriage is in; pass --dir",
                )
            })?,
    };
    let dir =
        fs::canonicalize(&dir).map_err(|e| err(2, format!("--dir: {}: {e}", dir.display())))?;
    let cli = dir.join(CLI.name);
    let tray = dir.join(TRAY.name);
    let keg = |path: &Path| {
        fs::canonicalize(path)
            .unwrap_or_else(|_| path.to_path_buf())
            .to_string_lossy()
            .contains("/Cellar/mailtriage/")
    };
    if keg(&dir) || keg(&cli) {
        return Err(err(
            2,
            "installed by Homebrew; run brew uninstall mailtriage",
        ));
    }
    if !args.yes {
        if !self_install::terminal() {
            return Err(err(
                2,
                "self uninstall asks before it removes anything; run it in a terminal or pass --yes",
            ));
        }
        let question = format!(
            "Uninstall mailtriage from {}? Its services stop and its programs are removed.",
            dir.display()
        );
        if !p.confirm_or(&question, false) {
            return Err(err(2, "uninstall cancelled; nothing was changed"));
        }
    }
    // Held through the file removal, so no update or install recreates the
    // binaries meanwhile. The lock file is never deleted.
    let lock = install::lock(&dir, install::update_lock_wait())
        .map_err(|e| err(3, format!("{e:#}")))?
        .ok_or_else(|| {
            err(
                5,
                "another update or install is running in this directory; try again",
            )
        })?;
    let mut failures = Vec::new();
    let mut removed = Vec::new();
    let ctx = Context::detect().ok();
    let services = match &ctx {
        Some(ctx) => uninstall_services(ctx, &cli, hooks, &mut failures),
        None => vec![],
    };
    // The login item, without the tray binary.
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from);
    let mut login = false;
    if let Some(home) = &home {
        let macos = cfg!(target_os = "macos");
        let item = login_item::path(macos, home, std::env::var_os("XDG_CONFIG_HOME").as_deref());
        let ours = fs::read_to_string(&item)
            .ok()
            .and_then(|text| login_item::tray_of(macos, &text))
            .is_some_and(|program| names(&program, &tray));
        if ours {
            match remove_login_item(&item, ctx.as_ref()) {
                Ok(()) => {
                    removed.push(item);
                    login = true;
                }
                Err(e) => failures.push(format!("login item {}: {e}", item.display())),
            }
        }
    }
    // The running tray, through its own `quit`.
    let tray_result = if fs::symlink_metadata(&tray).is_ok() {
        p.say("Close any open categories window; it keeps running until you do.");
        match quit_tray(&tray) {
            Ok(result) => result,
            Err(message) => {
                failures.push(format!("tray: {message}"));
                "failed".to_owned()
            }
        }
    } else if login {
        "not_running".to_owned()
    } else {
        "not_installed".to_owned()
    };
    // The files, only when every step above succeeded.
    if failures.is_empty() {
        for path in [
            cli.clone(),
            install::previous_path(&cli),
            tray.clone(),
            install::previous_path(&tray),
        ] {
            if fs::symlink_metadata(&path).is_err() {
                continue;
            }
            match fs::remove_file(&path) {
                Ok(()) => removed.push(path),
                Err(e) => failures.push(format!("cannot remove {}: {e}", path.display())),
            }
        }
        if let Some(cache) = Cache::for_user() {
            let keys = [install_key(&cli), install_key(&tray)];
            if let Err(e) = cache.update(|c| {
                for key in &keys {
                    c.installs.remove(key);
                }
            }) {
                p.say(&format!(
                    "mailtriage: the update cache keeps entries for {}: {e:#}",
                    dir.display()
                ));
            }
        }
    } else {
        for failure in &failures {
            p.say(&format!("mailtriage: {failure}"));
        }
        p.say("Nothing was removed; fix the failures above and run self uninstall again.");
    }
    drop(lock);
    let kept = kept(&dir, home.as_deref(), p);
    let mut out = json!({"schema_version": 1, "self_uninstall": {
        "dir": dir,
        "services": services,
        "tray": tray_result,
        "removed": removed,
        "kept": kept,
        "failures": failures,
    }});
    if !failures.is_empty() {
        out["exit_code"] = json!(3);
    }
    Ok(out)
}

/// Whether `program` is `target`: equal once canonicalized (a file that is
/// gone compares as written).
fn names(program: &Path, target: &Path) -> bool {
    fs::canonicalize(program).unwrap_or_else(|_| program.to_path_buf()) == target
}

/// Uninstalls every marked service file whose executable is `cli`, as
/// `service uninstall` does, under that account's service lock; a file
/// that names another executable once the lock is held is skipped. Each
/// gets `{account, unit_path, action, error}`.
fn uninstall_services(
    ctx: &Context,
    cli: &Path,
    hooks: &dyn Hooks,
    failures: &mut Vec<String>,
) -> Vec<Value> {
    let mut results = vec![];
    for file in service_files::list(ctx.manager, &ctx.home) {
        let ours = file
            .executable
            .as_deref()
            .is_some_and(|exe| names(exe, cli));
        if !ours {
            continue;
        }
        let result = (|| -> Result<&'static str> {
            let _lock =
                service_control::lock(ctx, &file.account, service_control::SERVICE_LOCK_WAIT)?;
            hooks.at("uninstall_service")?;
            let still_ours = fs::read_to_string(&file.unit_path)
                .ok()
                .filter(|text| system_service::is_marked(ctx.manager, text))
                .and_then(|text| service_files::executable(ctx.manager, &text))
                .is_some_and(|exe| names(&exe, cli));
            if !still_ours {
                return Ok("skipped");
            }
            system_service::uninstall(ctx, &file.account)?;
            Ok("uninstalled")
        })();
        let (action, error) = match result {
            Ok(action) => (action, None),
            Err(e) => {
                let message = error_text(&e);
                failures.push(format!("service {}: {message}", file.account));
                ("failed", Some(message))
            }
        };
        results.push(json!({
            "account": file.account,
            "unit_path": file.unit_path,
            "action": action,
            "error": error,
        }));
    }
    results
}

/// Removes the login item at `item`, the same way `autostart disable` does;
/// on macOS also boots out its loaded job.
fn remove_login_item(item: &Path, ctx: Option<&Context>) -> Result<()> {
    fs::remove_file(item)?;
    if let Some(ctx) = ctx.filter(|c| c.manager == Manager::Launchd) {
        let target = format!("gui/{}/{}", ctx.uid, login_item::LABEL);
        system_service::bootout(ctx, &target)?;
    }
    Ok(())
}

/// `DIR/mailtriage-tray quit --json`: `quit`, `not_running` or
/// `other_installation`; otherwise why the tray did not stop.
fn quit_tray(tray: &Path) -> Result<String, String> {
    let out = crate::process::run_bounded(tray, &["quit", "--json"], QUIT_TIMEOUT, 64 * 1024)
        .map_err(|e| format!("cannot start {}: {e}", tray.display()))?;
    let exited_0 = matches!(out.ending, Ending::Exited(status) if status.success());
    let value: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    match (exited_0, value["quit"].as_str()) {
        (true, Some(result @ ("quit" | "not_running" | "other_installation"))) => {
            Ok(result.to_owned())
        }
        _ => Err(value["error"]["message"]
            .as_str()
            .unwrap_or("the tray does not respond; quit it from its menu")
            .to_owned()),
    }
}

/// What stays, said once: the lock file, the private Himalaya (other
/// configs may use it) and the config directory. No mail was touched.
fn kept(dir: &Path, home: Option<&Path>, p: &mut Prompter) -> Vec<PathBuf> {
    let mut kept = vec![dir.join(install::LOCK_FILE)];
    if let Some(private) = himalaya::default_data_dir()
        .map(|data| data.join("mailtriage/himalaya"))
        .filter(|path| path.exists())
    {
        p.say(&format!(
            "Kept the private Himalaya in {}: other configs may use it. Delete it with {} once none does.",
            private.display(),
            shell_line(&[Path::new("rm"), Path::new("-rf"), &private])
        ));
        kept.push(private);
    }
    if let Some(config) = home.map(|h| h.join(".config/mailtriage")) {
        p.say(&format!(
            "Your config, state and logs stay in {} (the default place); no mail was touched.",
            config.display()
        ));
        if config.exists() {
            kept.push(config);
        }
    }
    kept
}

fn error_text(error: &anyhow::Error) -> String {
    error
        .downcast_ref::<crate::service::ServiceError>()
        .map_or_else(|| format!("{error:#}"), |e| e.message.clone())
}
```

In `src/distribution/mod.rs`, replace:

```rust
//! Installing mailtriage and what it needs (spec:
//! docs/superpowers/specs/2026-10-07-install-and-distribution-design.md):
//! the protected path rule, a private tested Himalaya, Homebrew launch
//! paths, and `self install`.
pub mod brew;
pub mod himalaya;
pub mod protected;
pub mod self_install;
```

with:

```rust
//! Installing mailtriage and what it needs (spec:
//! docs/superpowers/specs/2026-10-07-install-and-distribution-design.md):
//! the protected path rule, a private tested Himalaya, Homebrew launch
//! paths, the tray's login item, and `self install` and `self uninstall`.
pub mod brew;
pub mod himalaya;
pub mod login_item;
pub mod protected;
pub mod self_install;
pub mod self_uninstall;
```

In `src/cli.rs`, replace:

```rust
    /// Install this binary (and a tray) into DIR with the updater's
    /// transaction, then offer setup and the tray's login item.
    Install(SelfInstallArg),
}

#[derive(Args)]
```

with:

```rust
    /// Install this binary (and a tray) into DIR with the updater's
    /// transaction, then offer setup and the tray's login item.
    Install(SelfInstallArg),
    /// Remove the installation in DIR: its services, the tray's login item,
    /// the running tray, then its programs. Config, state and mail stay.
    Uninstall(SelfUninstallArg),
}

#[derive(Args)]
struct SelfUninstallArg {
    /// The installation's directory (default: this program's directory).
    #[arg(long)]
    dir: Option<PathBuf>,
    /// Do not ask for confirmation.
    #[arg(long)]
    yes: bool,
}

#[derive(Args)]
```

In `src/cli.rs`, replace:

```rust
            distribution::self_install::run(&args, &update::install::EnvHooks, &mut prompt)
                .map_err(service_error)
        }
    }
}

```

with:

```rust
            distribution::self_install::run(&args, &update::install::EnvHooks, &mut prompt)
                .map_err(service_error)
        }
        Command::SelfCmd {
            command: SelfCommand::Uninstall(arg),
        } => {
            let args = distribution::self_uninstall::Args {
                dir: arg.dir.clone(),
                yes: arg.yes,
            };
            let mut prompt = Prompter::new(prompt::stdin_unbuffered(), io::stderr(), true);
            distribution::self_uninstall::run(&args, &update::install::EnvHooks, &mut prompt)
                .map_err(service_error)
        }
    }
}

```

- [ ] **Step 4: Run the new tests**

Run: `cargo test --locked --test self_uninstall && cargo test --locked --lib distribution::login_item`
Expected: PASS (9 tests on macOS; 10 on Linux with `without_a_service_manager_there_are_no_services`).

- [ ] **Step 5: Document it**

In `docs/guide.md`, replace:

```markdown
Exit codes: 0; 2 for invalid flags or a refused downgrade; 3 for an unsafe directory, a failed install, a failed tray install, or a failed setup (the binaries stay installed); 5 when another update or install held the installation lock for 60 seconds.
```

with:

````markdown
Exit codes: 0; 2 for invalid flags or a refused downgrade; 3 for an unsafe directory, a failed install, a failed tray install, or a failed setup (the binaries stay installed); 5 when another update or install held the installation lock for 60 seconds.

### Uninstall

```sh
mailtriage self uninstall [--dir DIR] [--yes] [--json]
```

`self uninstall` removes the installation in `DIR`, by default the directory of the `mailtriage` that runs it, and nothing else:

1. A Homebrew install is refused: `installed by Homebrew; run brew uninstall mailtriage` (exit 2). Without `--yes` it asks first (default no); without a terminal it needs `--yes` (exit 2).
2. It takes the installation lock, waiting up to 60 seconds (else exit 5), so no update recreates the binaries meanwhile.
3. It uninstalls every background service whose executable is `DIR/mailtriage`, as `service uninstall` does, under each account's service lock. Services of other installations, Homebrew's included, stay.
4. It removes the tray's login item when that starts `DIR/mailtriage-tray` (on macOS it also stops the login job), then runs `DIR/mailtriage-tray quit`. Close any open categories window yourself.
5. Only when all of that worked, it deletes `DIR/mailtriage`, `DIR/mailtriage-tray` and their `.previous` copies, and their entries in the update cache. When a step failed, it deletes nothing, lists the failures and exits 3.

It keeps the installation lock file, the private Himalaya under `~/.local/share/mailtriage/himalaya` (other configs may use it; it prints how to delete it), and your config, state and logs (`~/.config/mailtriage/` by default). No mail is touched.

```json
{"schema_version":1,"self_uninstall":{"dir":"/Users/alice/.local/bin","services":[{"account":"work","unit_path":"/Users/alice/Library/LaunchAgents/digital.wirdrei.mailtriage.work.plist","action":"uninstalled","error":null}],"tray":"quit","removed":["/Users/alice/Library/LaunchAgents/digital.wirdrei.mailtriage-tray.plist","/Users/alice/.local/bin/mailtriage","/Users/alice/.local/bin/mailtriage.previous","/Users/alice/.local/bin/mailtriage-tray"],"kept":["/Users/alice/.local/bin/.mailtriage-update.lock","/Users/alice/.config/mailtriage"],"failures":[]}}
```

- `services[].action`: `uninstalled`, `skipped` (by the time its lock was held, the file named another executable) or `failed`, with `error`.
- `tray`: `quit`, `not_running`, `other_installation` (another installation's tray, which keeps running), `not_installed`, or `failed`.

Exit codes: 0; 2 for a Homebrew install, a refused confirmation, or no terminal without `--yes`; 3 for a failed service or tray step (nothing was deleted); 5 when the installation lock was held for 60 seconds.
````

In `docs/guide.md`, replace:

```markdown
`setup` and `service` have their own cases; see [Setup exit codes](#setup-exit-codes) and [Background service](#background-service).
```

with:

```markdown
`setup`, `service`, `himalaya install`, `self install` and `self uninstall` have their own cases; see [Setup exit codes](#setup-exit-codes), [Background service](#background-service), [A private Himalaya](#a-private-himalaya), [`mailtriage self install`](#mailtriage-self-install) and [Uninstall](#uninstall).
```

- [ ] **Step 6: Full check and commit**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`
Expected: all pass.

```bash
git add docs/guide.md \
  src/cli.rs \
  src/distribution/login_item.rs \
  src/distribution/mod.rs \
  src/distribution/self_uninstall.rs \
  tests/self_uninstall.rs
git commit -m "Add mailtriage self uninstall

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: `install.sh`

The bootstrapper at the repository root: one brace group (truncation-safe), `set -eu`, curl only, platform from `uname`, one temporary directory, the latest-URL check, `SHA256SUMS` and archive-layout checks, extraction of only the binary, and the hand-over to `self install` or `self uninstall` with the terminal (or `/dev/null`) as stdin. CI gains `shellcheck -s sh`, the build checks the archive layout (packed with `COPYFILE_DISABLE=1`), the release attaches a copy with its default version, and `install-check.yml` runs the script against real releases on demand.

**Files:**
- Create: `install.sh`, `.github/workflows/install-check.yml`
- Modify: `.github/workflows/ci.yml`, `.github/workflows/build.yml`, `.github/workflows/release.yml`, `README.md`, `docs/guide.md`, `docs/hermes.md`, `docs/releases.md`
- Test: `tests/install_script.rs` (new)

**Interfaces:**
- Consumes: `mailtriage self install` (Task 8) and `self uninstall` (Task 9); the update tests' loopback server (`tests/update_support`).
- Produces:
  - `install.sh [--version X.Y.Z] [--dir DIR] [--tray|--no-tray] [--no-setup] [--yes] [--uninstall]` with `MAILTRIAGE_VERSION`, `MAILTRIAGE_INSTALL_DIR`, `MAILTRIAGE_TRAY`, `MAILTRIAGE_NO_SETUP`, `MAILTRIAGE_YES`; test-only `MAILTRIAGE_INSTALL_URL` (loopback) and `MAILTRIAGE_INSTALL_TTY`. Exit codes 0, 1, 2, or the hand-over's.
  - The line `default_version=''` (column 0) that the release workflow rewrites to `default_version='X.Y.Z'`.

- [ ] **Step 1: Write the failing tests**

Create `tests/install_script.rs`:

```rust
#![cfg(unix)]
//! `install.sh`, run by `sh` against the update tests' loopback server
//! (`MAILTRIAGE_INSTALL_URL`, honoured only for loopback URLs). A fake
//! `uname` picks the platform; HOME and XDG_DATA_HOME are temporary; the
//! archives' `mailtriage` is a script that records how it was handed over,
//! except in the one run that installs the real binary.
mod update_support;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};
use update_support::{archive, Reply, Server};

const VERSION: &str = "1.2.3";

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("install.sh")
}

fn hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The fake `mailtriage`: records its arguments and what it read on stdin
/// in `$MT_MARK`.
const RECORDER: &[u8] =
    b"#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$MT_MARK/args\"\ncat > \"$MT_MARK/stdin\"\nexit \"${MT_EXIT:-0}\"\n";

fn cli_archive(binary: &[u8]) -> Vec<u8> {
    archive(&[
        ("mailtriage", binary),
        ("LICENSE", b"MIT\n"),
        ("README.md", b"# mailtriage\n"),
    ])
}

fn tray_archive() -> Vec<u8> {
    archive(&[("mailtriage-tray", RECORDER), ("LICENSE", b"MIT\n")])
}

/// A gzip tar with a symlink named `mailtriage` besides the other files.
fn archive_with_link() -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    header.set_mode(0o777);
    builder
        .append_link(&mut header, "mailtriage", "/bin/sh")
        .unwrap();
    for (name, data) in [("LICENSE", &b"MIT\n"[..]), ("README.md", b"# m\n")] {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        builder.append_data(&mut header, name, data).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    server: Server,
}

impl Fixture {
    /// Release 1.2.3 for linux-amd64 with `cli` and the tray, as the newest.
    fn new(cli: &[u8]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        for d in ["fakebin", "home", "data", "mark"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        let uname = root.join("fakebin/uname");
        fs::write(
            &uname,
            "#!/bin/sh\ntouch \"$MT_MARK/uname-ran\"\ncase \"$1\" in -s) echo \"$MT_OS\" ;; -m) echo \"$MT_ARCH\" ;; esac\n",
        )
        .unwrap();
        fs::set_permissions(&uname, fs::Permissions::from_mode(0o755)).unwrap();
        let f = Self {
            _dir: dir,
            root,
            server: Server::start(),
        };
        f.latest(&format!("/releases/tag/v{VERSION}"));
        f.publish(&cli_archive(cli), &tray_archive(), None);
        f
    }

    /// `/releases/latest` redirects to `target`.
    fn latest(&self, target: &str) {
        self.server.reply(
            "/releases/latest",
            Reply::redirect(&self.server.url(target)),
        );
        self.server.reply(target, Reply::ok("release page"));
    }

    /// Serves both archives and a `SHA256SUMS` (computed unless given).
    fn publish(&self, cli: &[u8], tray: &[u8], sums: Option<String>) {
        let download = format!("/releases/download/v{VERSION}");
        let cli_name = format!("mailtriage-v{VERSION}-linux-amd64.tar.gz");
        let tray_name = format!("mailtriage-tray-v{VERSION}-linux-amd64.tar.gz");
        let sums = sums.unwrap_or_else(|| {
            format!(
                "{}  {cli_name}\n{}  {tray_name}\n{}  mailtriage-v{VERSION}-macos-arm64.tar.gz\n",
                hex(cli),
                hex(tray),
                "0".repeat(64)
            )
        });
        self.server
            .reply(&format!("{download}/{cli_name}"), Reply::ok(cli.to_vec()));
        self.server
            .reply(&format!("{download}/{tray_name}"), Reply::ok(tray.to_vec()));
        self.server
            .reply(&format!("{download}/SHA256SUMS"), Reply::ok(sums));
    }

    fn mark(&self, name: &str) -> Option<String> {
        fs::read_to_string(self.root.join("mark").join(name)).ok()
    }

    fn args(&self) -> Vec<String> {
        self.mark("args")
            .map(|a| a.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    /// `sh SCRIPT ARGS` with the fake uname for `os arch`, the server as
    /// GitHub, `tty` as the terminal, and `env`.
    fn run_script(&self, script: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut command = Command::new("sh");
        command
            .arg(script)
            .args(args)
            .current_dir(&self.root)
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("fakebin").display()),
            )
            .env("HOME", self.root.join("home"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("MT_MARK", self.root.join("mark"))
            .env("MT_OS", "Linux")
            .env("MT_ARCH", "x86_64")
            .env("MAILTRIAGE_INSTALL_URL", &self.server.base)
            .env("MAILTRIAGE_INSTALL_TTY", self.root.join("no-tty"))
            .stdin(Stdio::null());
        for (key, value) in env {
            command.env(key, value);
        }
        command.output().unwrap()
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        self.run_script(&script(), args, env)
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn a_fresh_install_hands_over_to_self_install() {
    let f = Fixture::new(RECORDER);
    fs::write(f.root.join("tty"), "answers from the terminal").unwrap();
    let tty = f.root.join("tty");
    let out = f.run(&[], &[("MAILTRIAGE_INSTALL_TTY", tty.to_str().unwrap())]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let dir = f.root.join("home/.local/bin");
    assert_eq!(
        f.args(),
        ["self", "install", "--dir", dir.to_str().unwrap()]
    );
    assert_eq!(
        f.mark("stdin").as_deref(),
        Some("answers from the terminal")
    );
    // Linux without a display: no tray.
    let tray = format!("/releases/download/v{VERSION}/mailtriage-tray-");
    assert_eq!(f.server.count(&tray), 0);
    // With a display, the tray comes along, from the temporary directory,
    // which is gone afterwards.
    let out = f.run(
        &["--no-setup", "--yes"],
        &[
            ("DISPLAY", ":0"),
            ("MAILTRIAGE_INSTALL_TTY", tty.to_str().unwrap()),
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let args = f.args();
    assert_eq!(
        args[..4],
        ["self", "install", "--dir", dir.to_str().unwrap()]
    );
    assert_eq!(args[4], "--tray-file");
    assert!(args[5].ends_with("/mailtriage-tray"));
    assert!(
        !Path::new(&args[5]).exists(),
        "the temporary directory is removed"
    );
    assert_eq!(args[6..], ["--no-setup", "--yes"]);
    assert_eq!(
        f.mark("stdin").as_deref(),
        Some(""),
        "--yes reads /dev/null"
    );
    assert_eq!(f.server.count(&tray), 1);
}

#[test]
fn without_a_terminal_the_hand_over_reads_dev_null_and_its_exit_code_is_kept() {
    let f = Fixture::new(RECORDER);
    let out = f.run(
        &["--version", VERSION, "--dir", "/tmp/x y"],
        &[("MT_EXIT", "5")],
    );
    assert_eq!(out.status.code(), Some(5), "{}", stderr(&out));
    assert_eq!(f.args(), ["self", "install", "--dir", "/tmp/x y"]);
    assert_eq!(f.mark("stdin").as_deref(), Some(""));
    assert_eq!(
        f.server.count("/releases/latest"),
        0,
        "--version needs no lookup"
    );
}

#[test]
fn the_tray_archive_is_fetched_only_when_wanted() {
    let f = Fixture::new(RECORDER);
    let tray = format!("/releases/download/v{VERSION}/mailtriage-tray-");
    let out = f.run(&["--no-tray"], &[("DISPLAY", ":0")]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(f.server.count(&tray), 0);
    let out = f.run(&["--tray"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(f.server.count(&tray), 1);
    // macOS wants the tray by default.
    let out = f.run(&[], &[("MT_OS", "Darwin"), ("MT_ARCH", "arm64")]);
    assert_eq!(out.status.code(), Some(1), "macos-arm64 is not served here");
    assert!(
        f.server.count(&format!(
            "/releases/download/v{VERSION}/mailtriage-v{VERSION}-macos-arm64"
        )) == 1
    );
}

#[test]
fn an_unexpected_latest_url_exits_1() {
    for target in [
        "/releases/tag/v1.2",
        "/releases/tag/v01.2.3",
        "/releases/tag/1.2.3",
        "/elsewhere/tag/v1.2.3",
    ] {
        let f = Fixture::new(RECORDER);
        f.latest(target);
        let out = f.run(&[], &[]);
        assert_eq!(out.status.code(), Some(1), "{target}: {}", stderr(&out));
        assert!(stderr(&out).contains("unexpected URL"), "{target}");
        assert_eq!(f.mark("args"), None);
    }
}

#[test]
fn a_checksum_mismatch_or_a_bad_sums_file_installs_nothing() {
    let cli = cli_archive(RECORDER);
    let tray = tray_archive();
    let name = format!("mailtriage-v{VERSION}-linux-amd64.tar.gz");
    for (sums, why) in [
        (format!("{}  {name}\n", "a".repeat(64)), "checksum mismatch"),
        (
            format!("{}  {name}\n{}  {name}\n", hex(&cli), hex(&cli)),
            "exactly one line",
        ),
        (String::new(), "exactly one line"),
        (format!("{} {name}\n", hex(&cli)), "malformed"),
        (
            format!("{}  {name}\n", hex(&cli).to_uppercase()),
            "malformed",
        ),
    ] {
        let f = Fixture::new(RECORDER);
        f.publish(&cli, &tray, Some(sums.clone()));
        let out = f.run(&["--no-tray"], &[]);
        assert_eq!(out.status.code(), Some(1), "{sums:?}: {}", stderr(&out));
        assert!(stderr(&out).contains(why), "{sums:?}: {}", stderr(&out));
        assert_eq!(f.mark("args"), None);
    }
}

#[test]
fn an_archive_that_is_not_the_release_layout_installs_nothing() {
    for (archive, why) in [
        (
            archive(&[
                ("mailtriage", RECORDER),
                ("LICENSE", b"MIT\n"),
                ("README.md", b"r\n"),
                ("extra", b"x\n"),
            ]),
            "unexpected entry: extra",
        ),
        (archive_with_link(), "a link"),
        (
            archive(&[
                ("mailtriage", RECORDER),
                ("mailtriage", b"#!/bin/sh\n"),
                ("LICENSE", b"MIT\n"),
                ("README.md", b"r\n"),
            ]),
            "mailtriage twice",
        ),
        (
            archive(&[("mailtriage", RECORDER), ("LICENSE", b"MIT\n")]),
            "has no README.md",
        ),
        (
            archive(&[
                ("bin/mailtriage", RECORDER),
                ("LICENSE", b"MIT\n"),
                ("README.md", b"r\n"),
            ]),
            "unexpected entry",
        ),
    ] {
        let f = Fixture::new(RECORDER);
        f.publish(&archive, &tray_archive(), None);
        let out = f.run(&["--no-tray"], &[]);
        assert_eq!(out.status.code(), Some(1), "{why}: {}", stderr(&out));
        assert!(stderr(&out).contains(why), "{why}: {}", stderr(&out));
        assert_eq!(f.mark("args"), None);
    }
}

#[test]
fn unsupported_platforms_and_invalid_options_exit_2() {
    let f = Fixture::new(RECORDER);
    let out = f.run(&[], &[("MT_OS", "FreeBSD"), ("MT_ARCH", "amd64")]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains(
        "mailtriage has no build for FreeBSD amd64; see the guide's Install section to build from source"
    ));
    let out = f.run(&[], &[("MT_OS", "Darwin"), ("MT_ARCH", "x86_64")]);
    assert_eq!(out.status.code(), Some(2));
    for (args, env) in [
        (vec!["--version", "1.2"], vec![]),
        (vec!["--version", "v1.2.3"], vec![]),
        (vec!["--bogus"], vec![]),
        (vec!["--dir"], vec![]),
        (vec![], vec![("MAILTRIAGE_TRAY", "yes")]),
        (
            vec![],
            vec![("MAILTRIAGE_INSTALL_URL", "http://example.com")],
        ),
        (
            vec![],
            vec![("MAILTRIAGE_INSTALL_URL", "http://127.0.0.1.evil.example/")],
        ),
        (
            vec![],
            vec![("MAILTRIAGE_INSTALL_URL", "http://u@127.0.0.1:1/")],
        ),
    ] {
        let out = f.run(&args, &env);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{args:?} {env:?}: {}",
            stderr(&out)
        );
    }
    assert_eq!(f.server.count("/"), 0, "nothing was requested");
}

#[test]
fn uninstall_hands_over_to_self_uninstall_without_a_download() {
    let f = Fixture::new(RECORDER);
    let dir = f.root.join("bin");
    let out = f.run(&["--uninstall", "--dir", dir.to_str().unwrap()], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains(&format!("nothing is installed in {}", dir.display())));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("mailtriage"), RECORDER).unwrap();
    fs::set_permissions(dir.join("mailtriage"), fs::Permissions::from_mode(0o755)).unwrap();
    let out = f.run(
        &["--uninstall", "--dir", dir.to_str().unwrap(), "--yes"],
        &[],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        f.args(),
        ["self", "uninstall", "--dir", dir.to_str().unwrap(), "--yes"]
    );
    assert_eq!(f.server.count("/"), 0);
}

#[test]
fn a_truncated_script_runs_nothing() {
    let f = Fixture::new(RECORDER);
    let whole = fs::read(script()).unwrap();
    let cut_script = f.root.join("cut.sh");
    for cut in whole.len() - 64..whole.len() {
        let head = &whole[..cut];
        // Dropping only the trailing newline leaves the whole script.
        if head.trim_ascii_end() == whole.trim_ascii_end() {
            continue;
        }
        fs::write(&cut_script, head).unwrap();
        let out = f.run_script(&cut_script, &["--version", "9.9.9"], &[]);
        assert_ne!(out.status.code(), Some(0), "cut at {cut}");
        assert_eq!(f.mark("uname-ran"), None, "cut at {cut} ran");
    }
    assert_eq!(f.server.count("/"), 0);
    // The whole script runs and keeps its arguments.
    let out = f.run_script(&script(), &["--version", "9.9.9", "--no-tray"], &[]);
    assert_eq!(out.status.code(), Some(1), "v9.9.9 is not served");
    assert!(f.mark("uname-ran").is_some());
    assert_eq!(
        f.server
            .count("/releases/download/v9.9.9/mailtriage-v9.9.9-linux-amd64.tar.gz"),
        1
    );
}

#[test]
fn the_real_binary_installs_itself() {
    let binary = fs::read(env!("CARGO_BIN_EXE_mailtriage")).unwrap();
    let f = Fixture::new(&binary);
    let dir = f.root.join("bin");
    let out = f.run(
        &[
            "--yes",
            "--no-setup",
            "--no-tray",
            "--dir",
            dir.to_str().unwrap(),
        ],
        &[("XDG_CACHE_HOME", f.root.join("cache").to_str().unwrap())],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["self_install"]["cli"]["action"], "installed");
    let version = Command::new(dir.join("mailtriage"))
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&version.stdout),
        format!("mailtriage {}\n", env!("CARGO_PKG_VERSION"))
    );
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test --locked --test install_script`
Expected: FAIL, all 10 tests: `sh` cannot open `install.sh` (exit 127), and `a_truncated_script_runs_nothing` panics reading it.

- [ ] **Step 3: Implement**

Never run `install.sh` without a loopback `MAILTRIAGE_INSTALL_URL`: it would contact GitHub and, once releases exist, install into the real `~/.local/bin`. Check its syntax with `sh -n install.sh` and, where installed, `dash -n install.sh` and `shellcheck -s sh install.sh`. Check the release step's substitution by hand: `RELEASE_VERSION=1.2.3; sed "s/^default_version=''\$/default_version='$RELEASE_VERSION'/" install.sh | grep -x "default_version='1.2.3'"` prints the line.

Create `install.sh`:

```sh
#!/bin/sh
# Installs mailtriage from its GitHub releases:
#
#   curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/wir-drei-digital/mailtriage/main/install.sh | sh
#   curl ... | sh -s -- [--version X.Y.Z] [--dir DIR] [--tray|--no-tray] [--no-setup] [--yes] [--uninstall]
#
# It downloads and verifies the release archive, then hands over to the
# downloaded binary's `mailtriage self install`, which installs it into DIR
# (default ~/.local/bin) and offers setup. It never uses sudo, never edits
# shell startup files and never touches a himalaya. Exit codes: 0 done;
# 1 a failed step; 2 invalid options, an unsupported platform or a refused
# downgrade; otherwise the exit code of `mailtriage self install`.
#
# The whole script after this comment is one brace group: sh reads all of
# it before running anything, so a download that broke off runs nothing.
{
set -eu

# The release this copy installs by default; the release workflow sets it
# in the copy attached to each release. Empty: the newest release.
default_version=''

repo_url='https://github.com/wir-drei-digital/mailtriage'
max_binary_bytes=209715200

say() {
  printf 'mailtriage install: %s\n' "$*" >&2
}

# A failed step: exit 1.
fail() {
  say "$1"
  exit 1
}

# Invalid options, an unsupported platform: exit 2.
refuse() {
  say "$1"
  exit 2
}

usage() {
  cat >&2 <<'USAGE'
usage: install.sh [--version X.Y.Z] [--dir DIR] [--tray|--no-tray] [--no-setup] [--yes] [--uninstall]

  --version X.Y.Z  install this release instead of the newest (MAILTRIAGE_VERSION)
  --dir DIR        install directory, default ~/.local/bin (MAILTRIAGE_INSTALL_DIR)
  --tray           install the tray app too (MAILTRIAGE_TRAY=1)
  --no-tray        do not install the tray app (MAILTRIAGE_TRAY=0)
  --no-setup       do not offer setup or the login item (MAILTRIAGE_NO_SETUP=1)
  --yes            never prompt (MAILTRIAGE_YES=1)
  --uninstall      uninstall the installation in DIR
USAGE
}

# X.Y.Z with decimal parts and no leading zeros.
is_version() {
  case "$1" in
    '' | *[!0-9.]* | .* | *. | *..*) return 1 ;;
  esac
  old_ifs=$IFS
  IFS=.
  # The value holds only digits and dots: splitting it is safe.
  # shellcheck disable=SC2086
  set -- $1
  IFS=$old_ifs
  [ "$#" -eq 3 ] || return 1
  for part in "$@"; do
    case "$part" in
      0?*) return 1 ;;
    esac
  done
}

# A boolean from the environment: empty, 0 or 1.
flag_value() {
  case "$2" in
    '' | 0 | 1) printf '%s' "$2" ;;
    *) refuse "$1 must be 0 or 1" ;;
  esac
}

need() {
  command -v "$1" >/dev/null 2>&1 || fail "missing tool: $1"
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

# GET $1 into the file $2 (or with $3 = -w, print the effective URL).
fetch() {
  curl --proto "$proto" --proto-redir "$proto" --tlsv1.2 -fsSL \
    --max-redirs 10 --connect-timeout 30 --max-time 600 "$@"
}

# Checks the archive $1 in $tmp against its single SHA256SUMS line.
verify() {
  matches=$(awk -v name="$1" '$2 == name' "$tmp/SHA256SUMS")
  count=$(printf '%s\n' "$matches" | awk 'NF' | wc -l | tr -d ' ')
  [ "$count" = 1 ] || fail "SHA256SUMS must have exactly one line for $1, not $count"
  expected=$(printf '%s\n' "$matches" | awk 'length($1) == 64 && $1 ~ /^[0-9a-f]+$/ && $0 == $1 "  " $2 { print $1 }')
  [ -n "$expected" ] || fail "SHA256SUMS has a malformed line for $1"
  [ "$(sha256 "$tmp/$1")" = "$expected" ] || fail "checksum mismatch for $1"
}

# The archive $1 in $tmp lists exactly the top-level regular files $2...,
# each once; then only the first of them is extracted into $tmp.
unpack() {
  archive=$1
  shift
  listing=$(tar -tzvf "$tmp/$archive") || fail "cannot read $archive"
  seen=' '
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    case "$line" in
      *' -> '* | *' link to '*) fail "$archive holds a link" ;;
      -*) ;;
      *) fail "$archive holds something other than a regular file" ;;
    esac
    name=${line##* }
    case " $* " in
      *" $name "*) ;;
      *) fail "$archive holds an unexpected entry: $name" ;;
    esac
    case "$seen" in
      *" $name "*) fail "$archive holds $name twice" ;;
    esac
    seen="$seen$name "
  done <<LISTING
$listing
LISTING
  for name in "$@"; do
    case "$seen" in
      *" $name "*) ;;
      *) fail "$archive has no $name" ;;
    esac
  done
  tar -xzf "$tmp/$archive" -C "$tmp" "$1" || fail "cannot extract $1 from $archive"
  if [ ! -f "$tmp/$1" ] || [ -L "$tmp/$1" ]; then
    fail "$1 in $archive is not a regular file"
  fi
  size=$(wc -c <"$tmp/$1" | tr -d ' ')
  [ "$size" -le "$max_binary_bytes" ] || fail "$1 in $archive is larger than 200 MB"
  chmod 0755 "$tmp/$1"
}

# The stdin for the hand-over: the terminal when it can be opened and
# --yes is not given, else /dev/null. The script itself arrives on stdin.
input() {
  if [ "$yes" != 1 ] && (: <"$tty") 2>/dev/null; then
    printf '%s' "$tty"
  else
    printf '%s' /dev/null
  fi
}

main() {
  version=${MAILTRIAGE_VERSION:-}
  dir=${MAILTRIAGE_INSTALL_DIR:-}
  tray=$(flag_value MAILTRIAGE_TRAY "${MAILTRIAGE_TRAY:-}")
  no_setup=$(flag_value MAILTRIAGE_NO_SETUP "${MAILTRIAGE_NO_SETUP:-}")
  yes=$(flag_value MAILTRIAGE_YES "${MAILTRIAGE_YES:-}")
  uninstall=
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --version)
        [ "$#" -ge 2 ] || refuse "--version needs X.Y.Z"
        version=$2
        shift 2
        ;;
      --version=*)
        version=${1#--version=}
        shift
        ;;
      --dir)
        [ "$#" -ge 2 ] || refuse "--dir needs a directory"
        dir=$2
        shift 2
        ;;
      --dir=*)
        dir=${1#--dir=}
        shift
        ;;
      --tray)
        tray=1
        shift
        ;;
      --no-tray)
        tray=0
        shift
        ;;
      --no-setup)
        no_setup=1
        shift
        ;;
      --yes)
        yes=1
        shift
        ;;
      --uninstall)
        uninstall=1
        shift
        ;;
      -h | --help)
        usage
        exit 0
        ;;
      *)
        usage
        refuse "unknown option: $1"
        ;;
    esac
  done
  if [ -z "$dir" ]; then
    [ -n "${HOME:-}" ] || refuse "HOME is not set; pass --dir"
    dir="$HOME/.local/bin"
  fi
  [ -z "$version" ] || is_version "$version" || refuse "--version must be X.Y.Z, not $version"

  # Tests replace GitHub with a loopback server; only then is plain HTTP
  # allowed, and only then is the terminal replaceable.
  base=$repo_url
  proto='=https'
  tty=/dev/tty
  if [ -n "${MAILTRIAGE_INSTALL_URL:-}" ]; then
    case "$MAILTRIAGE_INSTALL_URL" in
      *@*) refuse "MAILTRIAGE_INSTALL_URL must be a loopback URL" ;;
      http://127.0.0.1 | http://127.0.0.1:* | http://127.0.0.1/*) ;;
      http://localhost | http://localhost:* | http://localhost/*) ;;
      *) refuse "MAILTRIAGE_INSTALL_URL must be a loopback URL" ;;
    esac
    base=${MAILTRIAGE_INSTALL_URL%/}
    proto='=http'
    tty=${MAILTRIAGE_INSTALL_TTY:-/dev/tty}
  fi

  if [ "$uninstall" = 1 ]; then
    [ -f "$dir/mailtriage" ] || fail "nothing is installed in $dir"
    set -- self uninstall --dir "$dir"
    if [ "$yes" = 1 ]; then set -- "$@" --yes; fi
    status=0
    "$dir/mailtriage" "$@" <"$(input)" || status=$?
    exit "$status"
  fi

  need curl
  need tar
  need mktemp
  need uname
  command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1 ||
    fail "missing tool: sha256sum or shasum"
  os=$(uname -s)
  arch=$(uname -m)
  case "$os $arch" in
    'Darwin arm64') platform=macos-arm64 ;;
    'Linux x86_64') platform=linux-amd64 ;;
    'Linux aarch64' | 'Linux arm64') platform=linux-arm64 ;;
    *) refuse "mailtriage has no build for $os $arch; see the guide's Install section to build from source" ;;
  esac
  if [ -z "$tray" ]; then
    case "$os" in
      Darwin) tray=1 ;;
      *) if [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then tray=1; else tray=0; fi ;;
    esac
  fi

  tmp=$(mktemp -d) || fail "cannot create a temporary directory"
  trap 'rm -rf "$tmp"' EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM

  version=${version:-$default_version}
  if [ -z "$version" ]; then
    latest=$(fetch -o /dev/null -w '%{url_effective}' "$base/releases/latest") ||
      fail "cannot find the newest release at $base/releases/latest"
    tag=${latest#"$base/releases/tag/v"}
    if [ "$tag" = "$latest" ] || ! is_version "$tag"; then
      fail "the newest release is at an unexpected URL: $latest"
    fi
    version=$tag
  fi

  download="$base/releases/download/v$version"
  cli_archive="mailtriage-v$version-$platform.tar.gz"
  tray_archive="mailtriage-tray-v$version-$platform.tar.gz"
  say "installing mailtriage $version ($platform) into $dir"
  fetch -o "$tmp/$cli_archive" "$download/$cli_archive" || fail "cannot download $cli_archive"
  fetch -o "$tmp/SHA256SUMS" "$download/SHA256SUMS" || fail "cannot download SHA256SUMS of v$version"
  if [ "$tray" = 1 ]; then
    fetch -o "$tmp/$tray_archive" "$download/$tray_archive" || fail "cannot download $tray_archive"
  fi
  verify "$cli_archive"
  unpack "$cli_archive" mailtriage LICENSE README.md
  if [ "$tray" = 1 ]; then
    verify "$tray_archive"
    unpack "$tray_archive" mailtriage-tray LICENSE
  fi

  set -- self install --dir "$dir"
  if [ "$tray" = 1 ]; then set -- "$@" --tray-file "$tmp/mailtriage-tray"; fi
  if [ "$no_setup" = 1 ]; then set -- "$@" --no-setup; fi
  if [ "$yes" = 1 ]; then set -- "$@" --yes; fi
  status=0
  "$tmp/mailtriage" "$@" <"$(input)" || status=$?
  exit "$status"
}

main "$@"
}
```

In `.github/workflows/ci.yml`, replace:

```yaml
    uses: ./.github/workflows/build.yml
    with:
      ref: ${{ github.sha }}
```

with:

```yaml
    uses: ./.github/workflows/build.yml
    with:
      ref: ${{ github.sha }}

  shell:
    name: Shell scripts
    runs-on: ubuntu-24.04
    timeout-minutes: 5
    steps:
      - uses: actions/checkout@v7.0.1
        with:
          persist-credentials: false
      # install.sh runs under any POSIX sh (dash on Ubuntu, bash on macOS).
      - run: shellcheck -s sh install.sh
      - run: shellcheck scripts/add-himalaya-version.sh
```

In `.github/workflows/build.yml`, replace:

```yaml
          "$binary" list --account work --view all --json
          tray="$GITHUB_WORKSPACE/target/release/mailtriage-tray"
          test "$("$tray" --version)" = "mailtriage-tray $VERSION"
      - name: Package binary and checksum
        env:
          VERSION: ${{ steps.version.outputs.version }}
          PLATFORM: ${{ matrix.platform }}
        run: |
          set -euo pipefail
          mkdir -p dist package
```

with:

```yaml
          "$binary" list --account work --view all --json
          tray="$GITHUB_WORKSPACE/target/release/mailtriage-tray"
          test "$("$tray" --version)" = "mailtriage-tray $VERSION"
      # COPYFILE_DISABLE keeps macOS tar from adding ._* metadata entries:
      # install.sh accepts exactly the release layout.
      - name: Package binary and checksum
        env:
          VERSION: ${{ steps.version.outputs.version }}
          PLATFORM: ${{ matrix.platform }}
          COPYFILE_DISABLE: 1
        run: |
          set -euo pipefail
          mkdir -p dist package
```

In `.github/workflows/build.yml`, replace:

```yaml
        env:
          VERSION: ${{ steps.version.outputs.version }}
          PLATFORM: ${{ matrix.platform }}
        run: |
          set -euo pipefail
          mkdir -p dist-tray tray-package
```

with:

```yaml
        env:
          VERSION: ${{ steps.version.outputs.version }}
          PLATFORM: ${{ matrix.platform }}
          COPYFILE_DISABLE: 1
        run: |
          set -euo pipefail
          mkdir -p dist-tray tray-package
```

In `.github/workflows/build.yml`, replace:

```yaml
          tar -czf "dist-tray/$archive" -C tray-package mailtriage-tray LICENSE
          cd dist-tray
          shasum -a 256 "$archive" > "$archive.sha256"
      # Named tray-*, not mailtriage-*: the release job downloads both
      # patterns and checks three CLI and three tray archives.
      - uses: actions/upload-artifact@v7.0.1
```

with:

```yaml
          tar -czf "dist-tray/$archive" -C tray-package mailtriage-tray LICENSE
          cd dist-tray
          shasum -a 256 "$archive" > "$archive.sha256"
      # The layout install.sh checks: top-level regular files, nothing else.
      - name: Check the archives' layout
        run: |
          set -euo pipefail
          layout() { tar -tzvf "$1" | awk '{print substr($1, 1, 1), $NF}' | LC_ALL=C sort; }
          test "$(layout dist/mailtriage-v*.tar.gz)" = "$(printf -- '- LICENSE\n- README.md\n- mailtriage')"
          test "$(layout dist-tray/mailtriage-tray-v*.tar.gz)" = "$(printf -- '- LICENSE\n- mailtriage-tray')"
      # Named tray-*, not mailtriage-*: the release job downloads both
      # patterns and checks three CLI and three tray archives.
      - uses: actions/upload-artifact@v7.0.1
```

In `.github/workflows/release.yml`, replace:

```yaml
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@v7.0.1
        with:
          ref: ${{ needs.prepare.outputs.sha }}
```

with:

```yaml
    permissions:
      contents: write
    steps:
      # Cone mode also checks out the files at the root, install.sh included.
      - uses: actions/checkout@v7.0.1
        with:
          ref: ${{ needs.prepare.outputs.sha }}
```

In `.github/workflows/release.yml`, replace:

```yaml
          test "${#trays[@]}" -eq 3
          cat "${manifests[@]}" > SHA256SUMS
          sha256sum --check SHA256SUMS
      - name: Publish GitHub Release
        env:
          GH_TOKEN: ${{ github.token }}
```

with:

```yaml
          test "${#trays[@]}" -eq 3
          cat "${manifests[@]}" > SHA256SUMS
          sha256sum --check SHA256SUMS
      # The copy attached to the release installs that release by default.
      - name: Attach install.sh
        env:
          RELEASE_VERSION: ${{ needs.prepare.outputs.version }}
        run: |
          set -euo pipefail
          sed "s/^default_version=''\$/default_version='$RELEASE_VERSION'/" install.sh > dist/install.sh
          grep -qx "default_version='$RELEASE_VERSION'" dist/install.sh
      - name: Publish GitHub Release
        env:
          GH_TOKEN: ${{ github.token }}
```

In `.github/workflows/release.yml`, replace:

```yaml
          PRERELEASE: ${{ needs.prepare.outputs.prerelease }}
        run: |
          set -euo pipefail
          assets=(dist/*.tar.gz dist/*.sha256 dist/SHA256SUMS)
          if gh release view "$RELEASE_TAG" --json isDraft > /tmp/existing-release.json 2>/dev/null; then
            jq -e '.isDraft == true' /tmp/existing-release.json > /dev/null || {
              echo 'Release is already published; assets will not be overwritten.' >&2
```

with:

```yaml
          PRERELEASE: ${{ needs.prepare.outputs.prerelease }}
        run: |
          set -euo pipefail
          assets=(dist/*.tar.gz dist/*.sha256 dist/SHA256SUMS dist/install.sh)
          if gh release view "$RELEASE_TAG" --json isDraft > /tmp/existing-release.json 2>/dev/null; then
            jq -e '.isDraft == true' /tmp/existing-release.json > /dev/null || {
              echo 'Release is already published; assets will not be overwritten.' >&2
```

Create `.github/workflows/install-check.yml`:

```yaml
name: Install check

# The install script from main against the real GitHub release, on each
# platform. Dispatch it once a release is published (see docs/releases.md).
on:
  workflow_dispatch:

permissions:
  contents: read

jobs:
  install:
    name: install.sh (${{ matrix.os }})
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-24.04, ubuntu-24.04-arm, macos-15]
    runs-on: ${{ matrix.os }}
    timeout-minutes: 15
    steps:
      - name: Install with the script from main
        run: |
          set -euo pipefail
          curl --proto '=https' --tlsv1.2 -fsSL \
            https://raw.githubusercontent.com/wir-drei-digital/mailtriage/main/install.sh |
            sh -s -- --yes --no-setup
      - name: Run it, and install a private Himalaya
        run: |
          set -euo pipefail
          "$HOME/.local/bin/mailtriage" --version
          "$HOME/.local/bin/mailtriage" himalaya install --json
```

- [ ] **Step 4: Run the new tests**

Run: `cargo test --locked --test install_script`
Expected: PASS (10 tests). Also run them once with `dash` as `sh` where it is installed (put a `sh` symlink to `dash` first on the test's `PATH`, or run the cases by hand with `dash install.sh`).

- [ ] **Step 5: Document it**

In `README.md`, replace:

````markdown
## Install

From a release (macOS arm64, Linux amd64 or Linux arm64), with the GitHub CLI; on Linux, set `PLATFORM` to `linux-amd64` or `linux-arm64` and use `sha256sum --check` in place of `shasum -a 256 --check`:

```sh
VERSION=0.1.0 PLATFORM=macos-arm64
gh release download "v$VERSION" --repo wir-drei-digital/mailtriage --pattern "mailtriage-v$VERSION-$PLATFORM.tar.gz*" &&
  shasum -a 256 --check "mailtriage-v$VERSION-$PLATFORM.tar.gz.sha256" &&
  tar -xzf "mailtriage-v$VERSION-$PLATFORM.tar.gz" &&
  install -d ~/.local/bin &&
  install -m 0755 mailtriage ~/.local/bin/mailtriage
```

The macOS executable is unsigned and not notarized. `~/.local/bin` must be on your `PATH`: if `command -v mailtriage` prints nothing, add `export PATH="$HOME/.local/bin:$PATH"` to your shell profile (`~/.zprofile` on macOS, `~/.bashrc` on Linux) and open a new terminal.

From source, with a stable Rust toolchain:

```sh
cargo build --release --locked &&
  install -d ~/.local/bin &&
  install -m 0755 target/release/mailtriage ~/.local/bin/mailtriage
```

mailtriage keeps itself up to date: the background service installs new releases by itself, and `mailtriage update` installs one now. A root-owned or otherwise unsafe install, for example one made with `sudo` into `/usr/local/bin`, is not replaced: mailtriage only reports new releases for it ([Binaries mailtriage does not replace](docs/guide.md#binaries-mailtriage-does-not-replace)).
````

with:

````markdown
## Install

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/wir-drei-digital/mailtriage/main/install.sh | sh
```

The script installs mailtriage, and on macOS the tray app, into `~/.local/bin` (macOS arm64, Linux amd64 or arm64), then offers `mailtriage setup`. Script installs keep themselves up to date. The guide's [Install](docs/guide.md#install) section has the options, building from source and uninstalling.
````

In `docs/guide.md`, replace:

````markdown
You need:

- macOS arm64, Linux amd64 or Linux arm64,
- Himalaya with IMAP support, in a [tested version](#himalaya-versions) (2.1.0 or 2.2.1),
- an IMAP account whose server supports UID and UIDVALIDITY (the MOVE extension too, if you want filing),
- an OpenRouter API key,
- for the background service: launchd (macOS) or systemd (Linux).

From a GitHub release: each release carries `mailtriage-vVERSION-linux-amd64.tar.gz`, `-linux-arm64.tar.gz` and `-macos-arm64.tar.gz`, a `.sha256` file per archive and a combined `SHA256SUMS`. Each archive holds the `mailtriage` executable, the README and the license. For example, with the GitHub CLI; on Linux, set `PLATFORM` to `linux-amd64` or `linux-arm64` and use `sha256sum --check` in place of `shasum -a 256 --check`:

```sh
VERSION=0.1.0 PLATFORM=macos-arm64
gh release download "v$VERSION" --repo wir-drei-digital/mailtriage --pattern "mailtriage-v$VERSION-$PLATFORM.tar.gz*" &&
  shasum -a 256 --check "mailtriage-v$VERSION-$PLATFORM.tar.gz.sha256" &&
  tar -xzf "mailtriage-v$VERSION-$PLATFORM.tar.gz" &&
  install -d ~/.local/bin &&
  install -m 0755 mailtriage ~/.local/bin/mailtriage
```

The macOS executable is unsigned and not notarized. See the [release guide](releases.md) for how releases are made. A root-owned or otherwise unsafe install, for example one made with `sudo` into `/usr/local/bin`, is not replaced: mailtriage only reports new releases for it (see [Binaries mailtriage does not replace](#binaries-mailtriage-does-not-replace)).

From source, with a stable Rust toolchain:
````

with:

````markdown
You need:

- macOS arm64, Linux amd64 or Linux arm64,
- an IMAP account whose server supports UID and UIDVALIDITY (the MOVE extension too, if you want filing),
- an OpenRouter API key,
- for the background service: launchd (macOS) or systemd (Linux).

mailtriage reads mail through Himalaya, in a [tested version](#himalaya-versions) with IMAP support; setup installs one for mailtriage when none is found.

There are three ways to install, and each stays current differently:

| Way | Installs into | Updated by |
| --- | --- | --- |
| [The install script](#the-install-script) | `~/.local/bin` | mailtriage itself ([Updates](#updates)) |
| Homebrew | Homebrew's prefix | `brew upgrade mailtriage` |
| [From source](#from-source) with Cargo | where you put it | you |

The macOS executables are unsigned and not notarized. See the [release guide](releases.md) for how releases are made. A root-owned or otherwise unsafe install, for example one made with `sudo` into `/usr/local/bin`, is not replaced: mailtriage only reports new releases for it (see [Binaries mailtriage does not replace](#binaries-mailtriage-does-not-replace)).

### The install script

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/wir-drei-digital/mailtriage/main/install.sh | sh
```

The script downloads the newest release's archive and `SHA256SUMS` over HTTPS from GitHub, checks the archive's checksum and that it holds exactly `mailtriage`, `LICENSE` and `README.md`, and hands over to the downloaded binary's [`mailtriage self install`](#mailtriage-self-install), which installs it into `~/.local/bin` and offers setup. It needs `curl`, `tar`, `mktemp`, `uname`, and `sha256sum` or `shasum`. It never uses `sudo`, never edits your shell's startup files, and never touches a `himalaya`. Each release also carries `install.sh` with that release as its default version.

Options follow `sh -s --`, for example:

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/wir-drei-digital/mailtriage/main/install.sh | sh -s -- --version 0.3.0 --no-tray
```

| Option | Environment | Meaning |
| --- | --- | --- |
| `--version X.Y.Z` | `MAILTRIAGE_VERSION` | Install this release instead of the newest. Never a downgrade. |
| `--dir DIR` | `MAILTRIAGE_INSTALL_DIR` | The install directory, default `~/.local/bin`. |
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
| Start the tray at login (after setup succeeded) | asks, default yes | no; prints the command | no |
| Uninstall | asks, default no | yes | refused (exit 2; use `--yes`) |

The end of input at a question counts as its default. Exit codes: 0 done; 1 a failed step, which the message names: a missing tool, a network error, a checksum mismatch, an unexpected archive or latest-release URL; 2 invalid options, an unsupported platform, or a refused downgrade; any other code is `mailtriage self install`'s. A download that breaks off runs nothing: the whole script is one `{ … }` group, which `sh` reads completely before running it.

The script never downgrades. To go back to an older release, follow [Rolling back by hand](#rolling-back-by-hand).

### From source

With a stable Rust toolchain:
````

In `docs/guide.md`, replace:

````markdown
### Uninstall

```sh
mailtriage self uninstall [--dir DIR] [--yes] [--json]
```

`self uninstall` removes the installation in `DIR`, by default the directory of the `mailtriage` that runs it, and nothing else:
````

with:

````markdown
### Uninstall

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/wir-drei-digital/mailtriage/main/install.sh | sh -s -- --uninstall
mailtriage self uninstall [--dir DIR] [--yes] [--json]
```

The script's `--uninstall` runs `DIR/mailtriage self uninstall --dir DIR`, with `--yes` when given, and downloads nothing; it asks through your terminal like the install does. `self uninstall` removes the installation in `DIR`, by default the directory of the `mailtriage` that runs it, and nothing else:
````

In `docs/hermes.md`, replace:

```markdown
1. Install a release binary on the same host as the state directory (see [Install](guide.md#install)). The examples use `/opt/mailtriage/mailtriage`.
```

with:

````markdown
1. Install mailtriage on the same host as the state directory, as the user that will run it, without prompts:

   ```sh
   curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/wir-drei-digital/mailtriage/main/install.sh | sh -s -- --yes --no-tray --no-setup
   ```

   It installs into `~/.local/bin` (`--dir DIR` for another directory) and prints the [`self install`](guide.md#mailtriage-self-install) result; exit 0 means done, 1 a failed download or check, 2 invalid options, an unsupported platform or a refused downgrade. A host where provisioning owns the binary can unpack a release archive itself instead (see [Install](guide.md#install)). The examples use `/opt/mailtriage/mailtriage`.
````

In `docs/hermes.md`, replace:

```markdown
   /opt/mailtriage/mailtriage setup --yes --himalaya-account work --key-store pass --key-stored --json
```

with:

```markdown
   /opt/mailtriage/mailtriage setup --yes --himalaya-install --himalaya-account work --key-store pass --key-stored --json
```

In `docs/releases.md`, replace:

```markdown
- A `.sha256` file for each archive, and a combined `SHA256SUMS` that lists
  all six archives.
```

with:

```markdown
- A `.sha256` file for each archive, and a combined `SHA256SUMS` that lists
  all six archives.
- `install.sh`, the install script, with this release as its default version.

The build checks that every archive holds exactly its files at the top level
(`mailtriage`, `LICENSE`, `README.md`; the tray's `mailtriage-tray`,
`LICENSE`), because the install script refuses anything else. macOS builds
pack them with `COPYFILE_DISABLE=1`, so no `._*` metadata entries appear.

After a release, run **Install check** (`install-check.yml`) from GitHub
Actions. It runs the install script from `main` against that release on
Linux amd64, Linux arm64 and macOS with `--yes --no-setup`, then
`mailtriage --version` and `mailtriage himalaya install`.
```

- [ ] **Step 6: Full check and commit**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`
Expected: all pass.

```bash
git add .github/workflows/build.yml \
  .github/workflows/ci.yml \
  .github/workflows/install-check.yml \
  .github/workflows/release.yml \
  README.md \
  docs/guide.md \
  docs/hermes.md \
  docs/releases.md \
  install.sh \
  tests/install_script.rs
git commit -m "Add the install script and attach it to releases

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: Homebrew tap

The formula template (`packaging/homebrew/mailtriage.rb.in`, placeholders for the version and four checksums), `render.sh VERSION SHA256SUMS` (fails when an archive is missing or doubled), `publish.sh TAP_DIR FORMULA VERSION` (never backwards, no commit for an identical formula, one retry after a push conflict), the release workflow's `homebrew` job (highest stable release only, concurrency group `homebrew-tap`, notice without `HOMEBREW_TAP_TOKEN`), the macOS CI check (`ruby -c`, `brew style`), and the tap repository's README and test workflow under `packaging/homebrew/tap/`.

**Files:**
- Create: `packaging/homebrew/mailtriage.rb.in`, `packaging/homebrew/render.sh`, `packaging/homebrew/publish.sh`, `packaging/homebrew/tap/README.md`, `packaging/homebrew/tap/.github/workflows/test.yml`
- Modify: `.github/workflows/release.yml`, `.github/workflows/ci.yml`, `README.md`, `docs/guide.md`, `docs/releases.md`, `docs/verification.md`
- Test: `tests/homebrew.rs` (new)

**Interfaces:**
- Consumes: `.github/scripts/is_highest_release.py` (on `main`); the release's `SHA256SUMS`.
- Produces:
  - `sh packaging/homebrew/render.sh X.Y.Z SHA256SUMS` → the formula on stdout; exit 1 (nothing on stdout) for a missing, doubled or malformed archive line; exit 2 for usage or a bad version.
  - `bash packaging/homebrew/publish.sh TAP_DIR FORMULA X.Y.Z`: exit 0 when the tap is updated, already has this formula, or has a higher version; non-zero when the retried push fails too.
  - `packaging/homebrew/tap/README.md`, `packaging/homebrew/tap/.github/workflows/test.yml` (pushed to `wir-drei-digital/homebrew-tap` by the main session, not by this plan).

- [ ] **Step 1: Write the failing tests**

Create `tests/homebrew.rs`:

```rust
#![cfg(unix)]
//! The Homebrew formula: `render.sh` fills the template from a release's
//! `SHA256SUMS`, and `publish.sh` updates a tap checkout without ever
//! moving it backwards. The tap here is a local bare repository; git runs
//! with no global or system configuration.
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn digest(n: u8) -> String {
    format!("{n:x}").repeat(64)
}

/// A `SHA256SUMS` listing the four archives of `version` (digests 1 to 4)
/// and a few others.
fn sums(version: &str) -> String {
    [
        (
            digest(1),
            format!("mailtriage-v{version}-macos-arm64.tar.gz"),
        ),
        (
            digest(2),
            format!("mailtriage-tray-v{version}-macos-arm64.tar.gz"),
        ),
        (
            digest(3),
            format!("mailtriage-v{version}-linux-amd64.tar.gz"),
        ),
        (
            digest(4),
            format!("mailtriage-v{version}-linux-arm64.tar.gz"),
        ),
        (
            digest(5),
            format!("mailtriage-tray-v{version}-linux-amd64.tar.gz"),
        ),
    ]
    .iter()
    .map(|(d, n)| format!("{d}  {n}\n"))
    .collect()
}

fn render(dir: &Path, version: &str, sums: &str) -> Output {
    let file = dir.join("SHA256SUMS");
    fs::write(&file, sums).unwrap();
    Command::new("sh")
        .arg(repo().join("packaging/homebrew/render.sh"))
        .arg(version)
        .arg(&file)
        .output()
        .unwrap()
}

#[test]
fn the_template_renders_every_checksum_and_parses() {
    let dir = tempfile::tempdir().unwrap();
    let out = render(dir.path(), "1.2.3", &sums("1.2.3"));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let formula = String::from_utf8(out.stdout).unwrap();
    assert!(!formula.contains('@'), "{formula}");
    assert!(formula.contains("  version \"1.2.3\"\n"));
    for n in 1..=4 {
        assert!(
            formula.contains(&format!("sha256 \"{}\"", digest(n))),
            "{n}"
        );
    }
    assert!(!formula.contains(&digest(5)));
    assert!(formula.contains("/releases/download/v1.2.3/mailtriage-tray-v1.2.3-macos-arm64.tar.gz"));
    assert!(formula.contains("depends_on arch: :arm64"));
    assert!(formula.contains("assert_match \"mailtriage #{version}\""));
    // `ruby -c` where Ruby is installed (the macOS CI job runs it anyway).
    let path = dir.path().join("mailtriage.rb");
    fs::write(&path, &formula).unwrap();
    if let Ok(out) = Command::new("ruby").arg("-c").arg(&path).output() {
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn a_missing_doubled_or_malformed_archive_fails_and_prints_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let all = sums("1.2.3");
    let tray = "mailtriage-tray-v1.2.3-macos-arm64.tar.gz";
    let without: String = all
        .lines()
        .filter(|l| !l.ends_with(tray))
        .map(|l| format!("{l}\n"))
        .collect();
    let doubled = format!("{all}{}  {tray}\n", digest(9));
    let malformed = all.replace(
        &format!("{}  {tray}", digest(2)),
        &format!("{} {tray}", digest(2)),
    );
    for (sums, why) in [
        (without, format!("0 lines for {tray}")),
        (doubled, format!("2 lines for {tray}")),
        (malformed, format!("malformed line for {tray}")),
    ] {
        let out = render(dir.path(), "1.2.3", &sums);
        assert_eq!(out.status.code(), Some(1));
        assert!(out.stdout.is_empty());
        assert!(String::from_utf8_lossy(&out.stderr).contains(&why), "{why}");
    }
    let out = render(dir.path(), "v1.2.3", &all);
    assert_eq!(out.status.code(), Some(2));
}

/// git with no global or system configuration and a fixed identity.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.test")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.test")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

struct Tap {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Tap {
    /// A bare "remote" whose main branch holds the formula of `version`, and
    /// a clone of it in `tap/`.
    fn new(version: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        git(&root, &["init", "-q", "--bare", "-b", "main", "remote.git"]);
        git(&root, &["clone", "-q", "remote.git", "seed"]);
        let tap = Tap { _dir: dir, root };
        tap.push_from("seed", version);
        git(&tap.root, &["clone", "-q", "remote.git", "tap"]);
        tap
    }

    fn formula(&self, version: &str) -> PathBuf {
        let path = self.root.join(format!("mailtriage-{version}.rb"));
        let out = render(&self.root, version, &sums(version));
        assert!(out.status.success());
        fs::write(&path, out.stdout).unwrap();
        path
    }

    /// Commits the formula of `version` in the clone `name` and pushes it.
    fn push_from(&self, name: &str, version: &str) {
        let clone = self.root.join(name);
        fs::create_dir_all(clone.join("Formula")).unwrap();
        fs::copy(self.formula(version), clone.join("Formula/mailtriage.rb")).unwrap();
        git(&clone, &["add", "Formula/mailtriage.rb"]);
        git(
            &clone,
            &["commit", "-q", "-m", &format!("mailtriage {version}")],
        );
        git(&clone, &["push", "-q", "origin", "HEAD:main"]);
    }

    fn publish(&self, version: &str) -> Output {
        Command::new("bash")
            .arg(repo().join("packaging/homebrew/publish.sh"))
            .arg(self.root.join("tap"))
            .arg(self.formula(version))
            .arg(version)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap()
    }

    /// The remote's commit subjects, newest first.
    fn log(&self) -> Vec<String> {
        git(
            &self.root.join("remote.git"),
            &["log", "--format=%s", "main"],
        )
        .lines()
        .map(str::to_owned)
        .collect()
    }
}

#[test]
fn a_newer_release_is_committed_and_pushed() {
    let tap = Tap::new("1.2.3");
    let out = tap.publish("1.2.4");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(tap.log(), ["mailtriage 1.2.4", "mailtriage 1.2.3"]);
}

#[test]
fn the_tap_never_moves_backwards_and_an_identical_formula_is_not_committed() {
    let tap = Tap::new("1.2.3");
    let out = tap.publish("1.2.2");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("higher than 1.2.2"));
    let out = tap.publish("1.2.3");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("already has this formula"));
    let out = tap.publish("1.10.0");
    assert!(out.status.success(), "1.10.0 is higher than 1.2.3");
    assert_eq!(tap.log(), ["mailtriage 1.10.0", "mailtriage 1.2.3"]);
}

#[test]
fn a_push_conflict_is_fetched_decided_again_and_retried_once() {
    let tap = Tap::new("1.2.3");
    // Another job pushed 1.2.4 after this checkout was made.
    git(&tap.root, &["clone", "-q", "remote.git", "other"]);
    tap.push_from("other", "1.2.4");
    let out = tap.publish("1.2.5");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        tap.log(),
        ["mailtriage 1.2.5", "mailtriage 1.2.4", "mailtriage 1.2.3"]
    );
    // When the other job pushed a higher one, nothing is pushed.
    let other = tap.root.join("other");
    git(&other, &["fetch", "-q", "origin", "main"]);
    git(&other, &["reset", "-q", "--hard", "origin/main"]);
    tap.push_from("other", "1.3.0");
    let out = tap.publish("1.2.6");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(tap.log()[0], "mailtriage 1.3.0");
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test --locked --test homebrew`
Expected: FAIL, all 5 tests: `sh` cannot open `packaging/homebrew/render.sh`.

- [ ] **Step 3: Implement**

Make the scripts executable: `chmod +x packaging/homebrew/render.sh packaging/homebrew/publish.sh` (the workflows call them through `sh` and `bash`). `brew style` is not run locally (it would install gems into Homebrew); the `formula` CI job runs it on macOS. `ruby -c` runs in the test where Ruby is installed.

Create `packaging/homebrew/mailtriage.rb.in`:

```ruby
# Rendered from packaging/homebrew/mailtriage.rb.in by mailtriage's release
# workflow (wir-drei-digital/mailtriage); edit the template there.
class Mailtriage < Formula
  desc "Local email classification and attention queries for humans and agents"
  homepage "https://github.com/wir-drei-digital/mailtriage"
  version "@VERSION@"
  license "MIT"

  on_macos do
    url "https://github.com/wir-drei-digital/mailtriage/releases/download/v@VERSION@/mailtriage-v@VERSION@-macos-arm64.tar.gz"
    sha256 "@SHA256_MACOS_ARM64@"

    depends_on arch: :arm64

    resource "tray" do
      url "https://github.com/wir-drei-digital/mailtriage/releases/download/v@VERSION@/mailtriage-tray-v@VERSION@-macos-arm64.tar.gz"
      sha256 "@SHA256_TRAY_MACOS_ARM64@"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/wir-drei-digital/mailtriage/releases/download/v@VERSION@/mailtriage-v@VERSION@-linux-amd64.tar.gz"
      sha256 "@SHA256_LINUX_AMD64@"
    end
    on_arm do
      url "https://github.com/wir-drei-digital/mailtriage/releases/download/v@VERSION@/mailtriage-v@VERSION@-linux-arm64.tar.gz"
      sha256 "@SHA256_LINUX_ARM64@"
    end
  end

  def install
    bin.install "mailtriage"
    resource("tray").stage { bin.install "mailtriage-tray" } if OS.mac?
  end

  def caveats
    <<~EOS
      Run `mailtriage setup`: it uses a tested Himalaya on your PATH or installs
      a private one for mailtriage.
      Updates come with `brew upgrade mailtriage`.
      On macOS, `mailtriage-tray autostart enable` starts the tray at login.
    EOS
  end

  test do
    assert_match "mailtriage #{version}", shell_output("#{bin}/mailtriage --version")
  end
end
```

Create `packaging/homebrew/render.sh`:

```sh
#!/bin/sh
# Renders the Homebrew formula of release X.Y.Z from its SHA256SUMS:
#
#   packaging/homebrew/render.sh X.Y.Z SHA256SUMS > mailtriage.rb
#
# Fails, printing nothing on stdout, when one of the four archives the
# formula names (the CLI for macos-arm64, linux-amd64 and linux-arm64, the
# tray for macos-arm64) is missing from SHA256SUMS, listed twice, or listed
# with a malformed line.
set -eu

if [ "$#" -ne 2 ]; then
  echo "usage: $0 X.Y.Z SHA256SUMS" >&2
  exit 2
fi
version=$1
sums=$2
here=$(cd "$(dirname "$0")" && pwd)
if ! printf '%s\n' "$version" | grep -Eqx '(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)'; then
  echo "render: $version is not X.Y.Z" >&2
  exit 2
fi
[ -f "$sums" ] || {
  echo "render: no $sums" >&2
  exit 1
}

# The SHA-256 SHA256SUMS lists for the archive "$1-v$version-$2.tar.gz".
digest() {
  name="$1-v$version-$2.tar.gz"
  count=$(awk -v name="$name" '$2 == name' "$sums" | wc -l | tr -d ' ')
  if [ "$count" != 1 ]; then
    echo "render: SHA256SUMS has $count lines for $name" >&2
    exit 1
  fi
  found=$(awk -v name="$name" 'length($1) == 64 && $1 ~ /^[0-9a-f]+$/ && $0 == $1 "  " name { print $1 }' "$sums")
  if [ -z "$found" ]; then
    echo "render: SHA256SUMS has a malformed line for $name" >&2
    exit 1
  fi
  printf '%s' "$found"
}

macos=$(digest mailtriage macos-arm64)
tray=$(digest mailtriage-tray macos-arm64)
amd64=$(digest mailtriage linux-amd64)
arm64=$(digest mailtriage linux-arm64)
sed -e "s/@VERSION@/$version/g" \
  -e "s/@SHA256_MACOS_ARM64@/$macos/g" \
  -e "s/@SHA256_TRAY_MACOS_ARM64@/$tray/g" \
  -e "s/@SHA256_LINUX_AMD64@/$amd64/g" \
  -e "s/@SHA256_LINUX_ARM64@/$arm64/g" \
  "$here/mailtriage.rb.in"
```

Create `packaging/homebrew/publish.sh`:

```sh
#!/usr/bin/env bash
# Puts a rendered formula into a checkout of the tap and pushes it, never
# moving the tap backwards:
#
#   packaging/homebrew/publish.sh TAP_DIR FORMULA X.Y.Z
#
# - The tap's Formula/mailtriage.rb has a higher version: nothing changes.
# - It has this version and the same file: nothing to commit.
# - Otherwise it commits "mailtriage X.Y.Z" and pushes. When the push is
#   refused because the tap moved meanwhile, it fetches, decides again and
#   tries once more.
set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: $0 TAP_DIR FORMULA X.Y.Z" >&2
  exit 2
fi
tap=$1
formula=$2
version=$3
target="$tap/Formula/mailtriage.rb"
branch="$(git -C "$tap" rev-parse --abbrev-ref HEAD)"

# Whether version $1 is higher than version $2 (X.Y.Z, compared as numbers).
higher() {
  python3 - "$1" "$2" <<'PY'
import sys
def parts(v):
    return tuple(int(p) for p in v.split('.'))
sys.exit(0 if parts(sys.argv[1]) > parts(sys.argv[2]) else 1)
PY
}

# The tap's formula version, empty without one.
current() {
  if [ -f "$target" ]; then
    sed -n 's/^  version "\(.*\)"$/\1/p' "$target" | head -n 1
  fi
}

# Commits and pushes when needed; fails only when the push is refused.
attempt() {
  local now
  now="$(current)"
  if [ -n "$now" ] && higher "$now" "$version"; then
    echo "::notice::the tap has mailtriage $now, higher than $version; it keeps it"
    return 0
  fi
  if [ "$now" = "$version" ] && cmp -s "$formula" "$target"; then
    echo "the tap already has this formula for mailtriage $version"
    return 0
  fi
  mkdir -p "$tap/Formula"
  cp "$formula" "$target"
  git -C "$tap" add Formula/mailtriage.rb
  git -C "$tap" -c user.name='github-actions[bot]' \
    -c user.email='41898282+github-actions[bot]@users.noreply.github.com' \
    commit -q -m "mailtriage $version"
  git -C "$tap" push -q origin "HEAD:$branch"
}

if ! attempt; then
  echo "the tap moved meanwhile; fetching and trying once more"
  git -C "$tap" fetch -q origin "$branch"
  git -C "$tap" reset -q --hard "origin/$branch"
  attempt
fi
```

Create `packaging/homebrew/tap/README.md`:

````markdown
# wir-drei-digital/homebrew-tap

Homebrew formulae from wir-drei-digital.

```sh
brew install wir-drei-digital/tap/mailtriage
```

[mailtriage](https://github.com/wir-drei-digital/mailtriage) classifies your
email on your machine. `brew upgrade mailtriage` installs new releases; then
run `mailtriage setup`, which uses a tested Himalaya on your `PATH` or installs a
private one. On macOS the formula also installs `mailtriage-tray`.

`Formula/mailtriage.rb` is written by mailtriage's release workflow from
`packaging/homebrew/mailtriage.rb.in` in that repository, and only for its
highest stable release. Change the template there, not the file here.

`.github/workflows/test.yml` installs and tests the formula on macOS and
Ubuntu after every push.
````

Create `packaging/homebrew/tap/.github/workflows/test.yml`:

```yaml
name: Test the formula

on:
  push:
  pull_request:
  workflow_dispatch:

permissions:
  contents: read

jobs:
  test:
    name: brew install and brew test (${{ matrix.os }})
    strategy:
      fail-fast: false
      matrix:
        os: [macos-15, ubuntu-24.04]
    runs-on: ${{ matrix.os }}
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v7.0.1
        with:
          persist-credentials: false
      # Sets up Homebrew with this repository as the tap under test.
      - uses: Homebrew/actions/setup-homebrew@main
        if: hashFiles('Formula/mailtriage.rb') != ''
      - name: Install
        if: hashFiles('Formula/mailtriage.rb') != ''
        run: brew install wir-drei-digital/tap/mailtriage
      - name: Test
        if: hashFiles('Formula/mailtriage.rb') != ''
        run: brew test mailtriage
      - name: Run it
        if: hashFiles('Formula/mailtriage.rb') != ''
        run: mailtriage --version
```

In `.github/workflows/release.yml`, replace:

```yaml
            fi
          fi
          gh release view "$RELEASE_TAG" --json url --jq '.url' >> "$GITHUB_STEP_SUMMARY"
```

with:

```yaml
            fi
          fi
          gh release view "$RELEASE_TAG" --json url --jq '.url' >> "$GITHUB_STEP_SUMMARY"

  # The Homebrew tap follows the highest stable release, one update at a
  # time, and never moves backwards. Without HOMEBREW_TAP_TOKEN (contents
  # write on wir-drei-digital/homebrew-tap only) it ends with a notice; the
  # release stays published either way, and this job can be re-run.
  homebrew:
    name: Homebrew tap
    needs: [prepare, publish]
    if: needs.prepare.outputs.prerelease == 'false'
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    concurrency:
      group: homebrew-tap
      cancel-in-progress: false
    permissions:
      contents: read
    env:
      HOMEBREW_TAP_TOKEN: ${{ secrets.HOMEBREW_TAP_TOKEN }}
      GH_TOKEN: ${{ github.token }}
      GH_REPO: ${{ github.repository }}
      RELEASE_TAG: ${{ needs.prepare.outputs.tag }}
      RELEASE_VERSION: ${{ needs.prepare.outputs.version }}
    steps:
      - uses: actions/checkout@v7.0.1
        with:
          ref: ${{ needs.prepare.outputs.sha }}
          sparse-checkout: |
            .github/scripts
            packaging/homebrew
          persist-credentials: false
      - name: Only the highest stable release
        id: highest
        run: |
          set -euo pipefail
          gh release list --exclude-drafts --exclude-pre-releases --limit 1000 \
            --json tagName > "$RUNNER_TEMP/published-releases.json"
          if python3 .github/scripts/is_highest_release.py "$RELEASE_TAG" "$RUNNER_TEMP/published-releases.json"; then
            echo "go=true" >> "$GITHUB_OUTPUT"
          else
            echo "::notice::$RELEASE_TAG is not the highest stable release; the tap keeps its formula"
            echo "go=false" >> "$GITHUB_OUTPUT"
          fi
      - name: Without HOMEBREW_TAP_TOKEN
        if: steps.highest.outputs.go == 'true' && env.HOMEBREW_TAP_TOKEN == ''
        run: echo "::notice::HOMEBREW_TAP_TOKEN is not set, so the tap was not updated; set it and re-run this job"
      - name: Render the formula
        if: steps.highest.outputs.go == 'true' && env.HOMEBREW_TAP_TOKEN != ''
        run: |
          set -euo pipefail
          gh release download "$RELEASE_TAG" --pattern SHA256SUMS --dir "$RUNNER_TEMP"
          sh packaging/homebrew/render.sh "$RELEASE_VERSION" "$RUNNER_TEMP/SHA256SUMS" > "$RUNNER_TEMP/mailtriage.rb"
          ruby -c "$RUNNER_TEMP/mailtriage.rb"
      - uses: actions/checkout@v7.0.1
        if: steps.highest.outputs.go == 'true' && env.HOMEBREW_TAP_TOKEN != ''
        with:
          repository: wir-drei-digital/homebrew-tap
          token: ${{ secrets.HOMEBREW_TAP_TOKEN }}
          path: tap
      - name: Commit and push, never backwards
        if: steps.highest.outputs.go == 'true' && env.HOMEBREW_TAP_TOKEN != ''
        run: bash packaging/homebrew/publish.sh tap "$RUNNER_TEMP/mailtriage.rb" "$RELEASE_VERSION"
```

In `.github/workflows/ci.yml`, replace:

```yaml
          persist-credentials: false
      # install.sh runs under any POSIX sh (dash on Ubuntu, bash on macOS).
      - run: shellcheck -s sh install.sh
      - run: shellcheck scripts/add-himalaya-version.sh
```

with:

```yaml
          persist-credentials: false
      # install.sh runs under any POSIX sh (dash on Ubuntu, bash on macOS).
      - run: shellcheck -s sh install.sh
      - run: shellcheck scripts/add-himalaya-version.sh packaging/homebrew/render.sh packaging/homebrew/publish.sh

  formula:
    name: Homebrew formula
    runs-on: macos-15
    timeout-minutes: 15
    steps:
      - uses: actions/checkout@v7.0.1
        with:
          persist-credentials: false
      - name: Render the template with dummy checksums
        run: |
          set -euo pipefail
          for name in mailtriage-v0.0.0-macos-arm64.tar.gz mailtriage-tray-v0.0.0-macos-arm64.tar.gz \
            mailtriage-v0.0.0-linux-amd64.tar.gz mailtriage-v0.0.0-linux-arm64.tar.gz; do
            printf '%064d  %s\n' 0 "$name"
          done > "$RUNNER_TEMP/SHA256SUMS"
          sh packaging/homebrew/render.sh 0.0.0 "$RUNNER_TEMP/SHA256SUMS" > "$RUNNER_TEMP/mailtriage.rb"
          ruby -c "$RUNNER_TEMP/mailtriage.rb"
      - run: brew style "$RUNNER_TEMP/mailtriage.rb"
```

- [ ] **Step 4: Run the new tests**

Run: `cargo test --locked --test homebrew`
Expected: PASS (5 tests).

- [ ] **Step 5: Document it**

In `README.md`, replace:

```markdown
The script installs mailtriage, and on macOS the tray app, into `~/.local/bin` (macOS arm64, Linux amd64 or arm64), then offers `mailtriage setup`. Script installs keep themselves up to date. The guide's [Install](docs/guide.md#install) section has the options, building from source and uninstalling.
```

with:

````markdown
or, with Homebrew:

```sh
brew install wir-drei-digital/tap/mailtriage
```

The script installs mailtriage, and on macOS the tray app, into `~/.local/bin` (macOS arm64, Linux amd64 or arm64), then offers `mailtriage setup`; script installs keep themselves up to date. Homebrew installs update with `brew upgrade`. The guide's [Install](docs/guide.md#install) section has the options, building from source and uninstalling.
````

In `docs/guide.md`, replace:

```markdown
| Homebrew | Homebrew's prefix | `brew upgrade mailtriage` |
```

with:

```markdown
| [Homebrew](#homebrew) | Homebrew's prefix | `brew upgrade mailtriage` |
```

In `docs/guide.md`, replace:

```markdown
The script never downgrades. To go back to an older release, follow [Rolling back by hand](#rolling-back-by-hand).
```

with:

````markdown
The script never downgrades. To go back to an older release, follow [Rolling back by hand](#rolling-back-by-hand).

### Homebrew

```sh
brew install wir-drei-digital/tap/mailtriage
```

The formula in [wir-drei-digital/homebrew-tap](https://github.com/wir-drei-digital/homebrew-tap) installs the release archive for macOS arm64, with `mailtriage-tray`, or for Linux amd64 or arm64. It has no Himalaya dependency: `mailtriage setup` uses a tested `himalaya` on your `PATH` or installs a private one. Then:

- `brew upgrade mailtriage` installs new releases. For a Homebrew install mailtriage only reports them (`managed_by_homebrew`).
- The background service and the tray's login item record `$(brew --prefix)/opt/mailtriage/bin/…`, which `brew upgrade` keeps pointing at the current version; running services and the tray switch to it by themselves.
- To uninstall, run `mailtriage service uninstall --account NAME` for each account, then `brew uninstall mailtriage`; `mailtriage self uninstall` refuses a Homebrew install.
````

In `docs/releases.md`, replace:

```markdown
No additional secret is required: only the publish job receives
`contents: write` through GitHub's built-in token. Model API keys and mailbox
```

with:

```markdown
Publishing needs no additional secret: only the publish job receives
`contents: write` through GitHub's built-in token. The Homebrew tap needs
`HOMEBREW_TAP_TOKEN` (see [Homebrew tap](#homebrew-tap)). Model API keys and mailbox
```

In `docs/releases.md`, replace:

```markdown
## Retry a failed release
```

with:

```markdown
## Homebrew tap

The release workflow's `homebrew` job updates `Formula/mailtriage.rb` in
[wir-drei-digital/homebrew-tap](https://github.com/wir-drei-digital/homebrew-tap)
after a stable release is published:

1. It runs only for the highest published stable release (the same check as
   the Latest marking), one at a time in the concurrency group
   `homebrew-tap`. GitHub keeps one waiting run per group, so a newer waiting
   run replaces an older one; rerun a replaced one only when it was the
   highest.
2. It renders `packaging/homebrew/mailtriage.rb.in` with
   `packaging/homebrew/render.sh VERSION SHA256SUMS`, which fails when an
   archive is missing from `SHA256SUMS`, and checks the result with `ruby -c`.
3. It checks out the tap with the secret `HOMEBREW_TAP_TOKEN` and runs
   `packaging/homebrew/publish.sh`. When the tap already has a higher
   version, nothing changes; the same version with the same file needs no
   commit; otherwise it commits `mailtriage X.Y.Z` and pushes, and after a
   push conflict it fetches, decides again and retries once.

`HOMEBREW_TAP_TOKEN` is a fine-grained personal access token with
**Contents: Read and write** on `wir-drei-digital/homebrew-tap` only, stored as
an Actions secret of this repository. Without it the job ends with a notice
and the release stays published; set it and rerun the job.

The tap's own workflow installs and tests the formula on macOS and Ubuntu
after every push. Its README and workflow are kept in
`packaging/homebrew/tap/`. CI here renders the template with dummy checksums
on macOS and runs `ruby -c` and `brew style` on it.

## Retry a failed release
```

In `docs/verification.md`, replace:

```markdown
| Step 9: `update` exits 0 with `update.tray.action` `skipped` and an `error` starting `mailtriage-tray does not run here:` that names the missing library; nothing is downloaded for the tray: no `mailtriage-tray.previous`, and its `sha256sum` is unchanged | | | |
```

with:

```markdown
| Step 9: `update` exits 0 with `update.tray.action` `skipped` and an `error` starting `mailtriage-tray does not run here:` that names the missing library; nothing is downloaded for the tray: no `mailtriage-tray.previous`, and its `sha256sum` is unchanged | | | |

## Install paths on a real machine (human check)

The automated tests run `install.sh` against a loopback server and never install from Homebrew. After the first release with the install script and the tap formula, check once on macOS arm64 and once on Linux (a desktop for the tray, and a server):

1. On a machine without mailtriage, run the one-line install from the README in a terminal, answer setup's questions, and accept the login item (macOS).
2. Run `mailtriage --version`, `mailtriage service status --json` and, on macOS, log out and in again.
3. Run the install line again.
4. Run it with `-s -- --uninstall` and confirm.
5. With Homebrew: `brew install wir-drei-digital/tap/mailtriage`, `mailtriage setup` (installing the service), then wait for one pass.
6. After the next release: `brew upgrade mailtriage` with the service running, and wait one interval.
7. `mailtriage self uninstall --dir "$(brew --prefix)/bin"`.

| Check | Result | Evidence | Date |
| --- | --- | --- | --- |
| Step 1: the script installs into `~/.local/bin`, prints the PATH line when it is not on `PATH`, runs setup from `~/.local/bin/mailtriage` (the service file names it), and on macOS enables the login item | | | |
| Step 1: setup offered the private Himalaya when no tested one was on `PATH`, and the account uses `~/.local/share/mailtriage/himalaya/VERSION/himalaya` | | | |
| Step 2: the service runs; on macOS the tray starts at login | | | |
| Step 3: the second run reinstalls the same version; the service restarts onto it (`restarting` event) | | | |
| Step 4: services, login item, tray and binaries are gone; config, state and the private Himalaya stay | | | |
| Step 5: the service file and the tray's login item name `$(brew --prefix)/opt/mailtriage/bin/…`; `mailtriage update --check --json` reports `managed_by_homebrew` | | | |
| Step 6: the running service restarts onto the new version by itself (`restarting` event, `last_pass.version`), and so does the tray | | | |
| Step 7: refused with `installed by Homebrew; run brew uninstall mailtriage` (exit 2) | | | |
```

- [ ] **Step 6: Full check and commit**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --locked`
Expected: all pass.

```bash
git add .github/workflows/ci.yml \
  .github/workflows/release.yml \
  README.md \
  docs/guide.md \
  docs/releases.md \
  docs/verification.md \
  packaging/homebrew/mailtriage.rb.in \
  packaging/homebrew/publish.sh \
  packaging/homebrew/render.sh \
  packaging/homebrew/tap/.github/workflows/test.yml \
  packaging/homebrew/tap/README.md \
  tests/homebrew.rs
git commit -m "Publish a Homebrew formula to the tap after each highest release

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Human gate

The automated tests never reach GitHub or Homebrew, never run a real release or a real Himalaya, and run `install.sh` only against a loopback server. After the first release with this feature:

1. In GitHub, set Settings → Actions → General → Workflow permissions → "Allow GitHub Actions to create and approve pull requests", and add the secret `HOMEBREW_TAP_TOKEN` (`docs/releases.md`).
2. The main session pushes `packaging/homebrew/tap/` to `wir-drei-digital/homebrew-tap`, with the user's go-ahead.
3. Dispatch **Install check** and run "Install paths on a real machine" from `docs/verification.md` on macOS arm64 and Linux.

## Spec coverage

| Spec section | Task |
| --- | --- |
| Tested Himalaya versions: the data file, accepting a version, what users see (doctor, passes) | 1 |
| Role names: effective target, conflicts, every read path, fail closed | 2 |
| `mailtriage himalaya install`; protected path rule | 3 |
| Setup step 2: private Himalaya offer, `--himalaya-install`, `brew pin` note; the `mail` item's fix | 4 |
| CI: e2e matrix from the data file, `run.sh`, weekly check, `scripts/add-himalaya-version.sh` | 5 |
| Brew installs inside mailtriage: stable launch path, restart rule for kegs | 6 |
| `mailtriage-tray quit` | 7 |
| `self install` | 8 |
| `self uninstall` | 9 |
| `install.sh` (shape, safety, steps, hand-over, decision table, uninstall), its tests, shellcheck, release asset, real runs | 10 |
| Homebrew tap: formula template, `render.sh`, release job, tap repository files, macOS template check | 11 |
| Docs (README, guide, releases, hermes, verification) | each task, last in 10 and 11 |
