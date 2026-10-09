# Automatic Updates Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An installed mailtriage keeps itself current from GitHub Releases: `mailtriage update [--check]` installs the newest stable release with a verified, revalidated rename, `watch` checks about once a day, installs in `auto` mode, and re-executes itself onto a replaced binary between passes, and `service status`, `doctor` and `setup` show the update state.

**Architecture:** All updater code lives in a new module tree `src/update/` (library crate), one file per responsibility: `version`, `github` (URL rules, paginated release list, bounded downloads), `release` (candidate, archive names, `SHA256SUMS`), `cache` (`update.json` under `update.lock`), `schedule`, `check`, `platform` (file identity, replaceability, `--version` probes), `archive`, `install` (the transaction under `.mailtriage-update.lock`), `service_files`, `command` (`mailtriage update`), `events`, `restart`, `watch` (the work between passes) and `report` (the status block). The shared files only gain small, additive hooks: a config field and schema 3, setup's locked write, a stderr-capturing runner beside `run_bounded`, the `update` subcommand, one `between` callback in `watch_loop`, and one line each in `service status`, `doctor` and setup step 9. Updates are component-keyed (`Component`, `release.archives`, a per-component install job) so the tray spec can add `mailtriage-tray` without reshaping this code.

**Tech Stack:** Rust 2021 (stable 1.92), clap 4, serde/serde_json, reqwest 0.12 blocking with rustls (already a dependency), fs2 locks, chrono, uuid. New crates: `semver` 1, `flate2` 1.1 (pure-Rust backend), `tar` 0.4 (no default features). Tests use a hand-written loopback HTTP server on `std::net`.

**Spec:** `design/specs/2026-10-06-auto-update-design.md`. Read it before every task. Where this plan and the spec disagree, the spec wins, except for the rulings under "Decisions this plan adds". The tray spec (`design/specs/2026-10-06-tray-design.md`) extends this updater; nothing of the tray is implemented here.

## Parallel development

This plan follows `design/plans/2026-10-06-parallel-development.md`:

- Branch `feature/auto-update` in the worktree `.worktrees/auto-update`, created from the shared `main` commit, which already has the machine-readable error `reason` (`ErrorKind`, `err_kind`, `CliError.reason`) and refile's plan but not refile's code.
- **Phase A** (Tasks 1 to 8) needs nothing from refile. The SDD run executes Phase A, then stops and reports.
- **Phase B** (Task 9: the heartbeat `version` column, schema v7, which needs refile's v6, every test that names a schema number, and `service status`'s `last_pass.version`) runs after the controller has merged `feature/refile` into this branch.
- Merge order into `main`: refile, then this branch, then tray. Nothing is pushed or merged by the SDD run.

## Decisions this plan adds

Rulings on points the spec leaves open. Each says what it costs if wrong.

1. **Test hooks only in debug builds.** `MAILTRIAGE_UPDATE_TEST_HOOK` names a program that runs with the hook point as its only argument (`checked`, `revalidate`, `backup`, `commit`, `publish`, `sync_dir`, `record`, `exec`); a non-zero exit fails that step. `MAILTRIAGE_UPDATE_TEST_LOCK_WAIT_MS` shortens `update`'s 60 s lock wait. Both are read only under `cfg(debug_assertions)`, so `cargo test` (a debug build) can use them and release binaries never read them; the tests that need them are `#[cfg(debug_assertions)]` too. `MAILTRIAGE_UPDATE_URL` is honoured in every build, but only for 127.0.0.1, `[::1]` or `localhost`. Cost if wrong: one `cfg` per hook.
2. **The restart rule applies in every `updates` mode,** `off` included: it makes no network call, and a provisioning tool that replaces the binary also wants the service to switch. Cost: one condition in `WatchUpdates::before_pass`.
3. **Readers take no installation lock.** `update --check`, `service status` and `doctor` run `<binary> --version` without the installation lock (they change nothing, and a directory that is not writable cannot even hold the lock file); installs read it under the lock as the spec says. Cache readers take `update.lock` shared only when it exists and create no file. Cost: a reading during an install may show the old version.
4. **An installed binary whose `--version` fails** is compared with the running version: `update` installs only a candidate newer than the running process, and reports `from` as the running version. Cost: one fallback.
5. **A binary that may not be replaced but is already current** gets `action: current` (exit 0); the refusal (exit 3) applies only when there is something to install. Cost: users of Homebrew builds would otherwise see exit 3 after every `update`.
6. **Missing `SHA256SUMS`** fails with `release vX.Y.Z has no SHA256SUMS`; a missing or doubled archive keeps the spec's `release vX.Y.Z has no PLATFORM archive`. Cost: one message.
7. **Archive entries.** A leading `./` is ignored; regular files other than `mailtriage` (`README.md`, `LICENSE`, and the `._*` AppleDouble files macOS `tar` may add), directories and pax global headers are skipped; links and special files fail the archive. Cost: one match arm per kind.
8. **Smoke test causes** add `exited with N` (a glibc that is too old makes the loader exit 1 or 127) and `printed too much` to the spec's list; the smoke test also fails when the binary prints another version: `printed version X, expected Y`. Cost: strings.
9. **Event text.** With `--json` an event is one JSON line on stdout; without it, one text line on stdout: `update: restarting onto 0.3.0 (was 0.2.0, pid 1234)`, `update: mailtriage 0.3.0 is available (running 0.2.0): URL[; FIX]`, `update error: MESSAGE`. (Passes without `--json` are pretty-printed JSON, so "the way `watch` prints passes" cannot mean one line there.) Cost: one function.
10. **`installs[path].identity`.** The cache's installation entry gains an optional `identity` (device, inode, size, mtime, ctime, mode), written by every reading and every install. `watch` runs `--version` only when the file's identity differs from it, also across restarts. Cost: one optional field that older readers ignore.
11. **`watch` takes the installation lock once, before its reservation.** A busy lock is neither a failure nor a reservation: the next pass tries again. Cost: a busy lock delays an install by one interval.
12. **Cache problems in `watch`** (cannot write `update.json`) are printed once per process; other update errors once per occurrence (their backoff keeps them rare). Cost: one flag.
13. **Server retry hints are capped at 24 hours** (`Retry-After`, `X-RateLimit-Reset`), so a bad header cannot stop checks for weeks. Cost: one `min`.
14. **`notify`** compares the cached release with the installed version whatever the release's age; the 48 h limit applies to `auto` installs. `auto` behaves like `notify` when the installation path is unknown (the restart rule is off). Cost: one condition each.
15. **`update` records successes, not failures,** in `installs`, so a failed manual run does not delay the background installer. Cost: none.
16. **`--check` output has `warnings`** (for example a cache that cannot be written), like `update`'s. Cost: one field.
17. **Setup's `doctor.ready` ignores the `update` item,** as `doctor`'s top-level `ready` ignores the `update` block; the item's `error` is the reason (`managed_by_homebrew`, …). Cost: one line.
18. **`config::read_updates_mode`** reads only the JSON, `schema_version` (1 to 3) and `updates`, so a config that fails validation for another reason still yields its mode, and a release whose passes fail can still update itself. The cache key of a config is its canonical path, else its absolute path. Cost: one function.
19. **An invalid `updates`** is a typed error (`config::InvalidUpdates`); `Service::open` reports exactly `updates must be auto, notify or off` instead of the generic invalid-configuration message. Cost: three lines.
20. **Proxies.** GitHub requests honour the system proxy settings as reqwest does; the loopback override disables proxies so tests are hermetic. Cost: one builder call.
21. **Randomness** for jitter comes from `uuid::Uuid::new_v4` (already a dependency); no `rand`. Cost: none.
22. **Memory.** The archive is downloaded into memory (at most 200 MB), `SHA256SUMS` at most 1 MB, one release-list page at most 10 MB. Cost: memory for one archive during an install.
23. **Linux image identity** is `fs::metadata("/proc/self/exe")`; the installation path falls back to an absolute, existing `argv[0]` when the executable link ends in ` (deleted)`. The `update` module uses Unix APIs like the rest of the crate (macOS and Linux only). Cost: none on supported platforms.
24. **`services` in `update`'s output** are the marked files in the platform's directory under `HOME` (`~/Library/LaunchAgents`, `~/.config/systemd/user`) whose names follow `label(account)`/`unit_name(account)` for a valid account name; no `launchctl`/`systemctl` lookup. The tray's `digital.wirdrei.mailtriage-tray.plist` is not listed. Cost: none.
25. **Fix texts** (`install.fix`): ``run `brew upgrade mailtriage` ``; ``update mailtriage through nix (for example `nix profile upgrade`)``; `make DIR writable for this user, or set "updates" to "notify"`; `install mailtriage into a directory that only this user owns and can write, such as ~/.local/bin, or set "updates" to "notify" (now: PATH)`; `releases have no archive for this platform; build mailtriage from source, or set "updates" to "off"`. Cost: strings.
26. **After a successful background install** the restart check runs again at once, so `watch` switches before its pass. A stop request that arrives during the update step skips the pass. Cost: one call.
27. **`update` without a stable release** exits 3 with `no stable release of mailtriage was found`; `--check` reports `latest: null` and exits 0. Exit 5 (`another update is running`) carries no `reason`; the spec names none. Cost: strings.
28. **`run_bounded` is refactored** to share one private runner with the new `run_captured`; its behaviour (stderr discarded) is unchanged and its tests keep passing. Cost: none.
29. **Release workflow.** The publish job gets a job-level concurrency group `release-publish` (`cancel-in-progress: false`). The Latest decision is a small script, `.github/scripts/is_highest_release.py`, run against `gh release list --exclude-drafts --exclude-pre-releases --json tagName`; the publish job checks out only `.github/scripts` at the release commit. Cost: one checkout step.
30. **Phase B guard.** The newer-schema guard becomes `LATEST`, derived from the last entry of `MIGRATIONS`, so a later migration cannot forget it. Cost: none.

## Global Constraints

- Work on `feature/auto-update` in `.worktrees/auto-update`. Commit at the end of every task; every commit message ends with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Push nothing, merge nothing.
- After every task all three pass: `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`. CI runs them on Linux amd64, Linux arm64 and macOS arm64, so a test that depends on the platform is `#[cfg]`-gated or branches on `cfg!`.
- New dependencies exactly: `flate2 = { version = "1.1", default-features = false, features = ["rust_backend"] }`, `semver = "1"`, `tar = { version = "0.4.44", default-features = false }`. `Cargo.lock` gains exactly `adler2`, `crc32fast`, `filetime`, `flate2`, `miniz_oxide`, `semver`, `simd-adler32` and `tar`, nothing else changes.
- Source: "The highest stable release of `wir-drei-digital/mailtriage` (public) on GitHub, read from the release list. No token. GitHub's "Latest" flag is not relied on."
- Release list: `GET https://api.github.com/repos/wir-drei-digital/mailtriage/releases?per_page=30` with `User-Agent: mailtriage/<running>`, `Accept: application/vnd.github+json` and `X-GitHub-Api-Version: 2022-11-28`, following `Link: rel="next"`; more than 20 pages, or any page that fails, fails the check.
- URL rules, for the initial URL and every redirect: "scheme `https`, port 443, no user name or password; host `github.com`, or ending in `.github.com` or `.githubusercontent.com`; at most 10 redirects". API call 30 s, each download 300 s, assets at most 200 MB.
- Archive: "at most 16 entries, at most 256 MB decompressed in total, entry paths at most 255 bytes; exactly one entry named `mailtriage` at the top level, and it is a regular file of at most 200 MB".
- Smoke test: `<staged> --version` with a 10 s limit through the stderr-capturing runner (stderr at most 4 KB); stdout exactly `mailtriage X.Y.Z` (the candidate), optionally followed by one newline.
- Replacing is always a rename over the installation path; nothing ever writes into the file of a running binary.
- Config: "Updater-capable binaries read schemas 1 to 3 and write `schema_version: 3` with an explicit `updates` on every config write". Invalid value: exit 2, `updates must be auto, notify or off`.
- `update` exit codes: 0 updated/current/`--check` done; 2 invalid flags; 3 network, GitHub, release, archive, smoke test, changed binary, not replaceable; 5 installation lock held for 60 s (`another update is running`).
- JSON keeps `schema_version: 1` and the error envelope `{"schema_version":1,"error":{"code","message"[,"reason"]}}`. Events are `{"schema_version":1,"update":{"event":…}}`.
- Tests never touch the real `HOME`, cache, service directories or `~/.cargo/bin`, never run the real `launchctl` or `systemctl`, and never reach GitHub: `HOME` and `XDG_CACHE_HOME` point into temporary directories, the binary under test is copied into a temporary directory and run from there, and a loopback server stands in for GitHub.
- The OpenRouter key is never written into any file, fixture or doc recipe.
- Edits to shared files (`src/cli.rs`, `src/service.rs`, `src/system_service.rs`, `src/store.rs`, `docs/guide.md`, `docs/agents/index.md`, `README.md`, `.github/workflows/*`, `Cargo.toml`) stay small and additive; docs get new sections rather than rewrites.
- The classification generation hash and the binding identity golden values must not change (`updates` enters neither).
- Existing tests keep passing unchanged, with these permitted edits:
  - `tests/config_v2.rs`: the schema written is 3 (`legacy_himalaya_config_loads_as_engine_and_saves_as_v3`) and the rejected versions are `[0, 4]` (Task 1);
  - `src/cli.rs` `mod tests`: the `run` helper passes `|_| {}` as `watch_loop`'s new `between` argument (Task 5);
  - `tests/cli.rs` and `tests/filing_cli.rs`: their `run` helpers set `HOME` and `XDG_CACHE_HOME` to the test directory, because `doctor` now reads the update cache and the service directory (Task 7);
  - Phase B: assertions that pin the latest schema number or the newer-schema guard (Task 9).
- Every database migration is additive (new tables, nullable or defaulted columns).

## Review Focus

Inputs the spec implies but its own test list would not exercise, most likely to bite first. Each has a pinned test in the task named.

1. **The real macOS release archive.** The build workflow runs `tar -czf` on macOS, whose `bsdtar` may add pax headers and `._mailtriage` AppleDouble entries. Expected: the archive still installs. Tests: Task 3 `the_binary_is_extracted_and_other_files_are_skipped` and `pax_headers_are_understood`.
2. **A binary that belongs to root or sits in a shared directory** (the README's `sudo install … /usr/local/bin`, a group-writable `/opt`). Expected: reported with a fix, never replaced; `update` exits 3, `--check` exits 0, `watch` in `auto` prints `available` with the reason. Tests: Task 3 `a_private_directory_is_replaceable_and_a_group_writable_one_is_not`; Task 4 `a_binary_that_is_not_replaceable_is_refused_with_its_fix`; Task 6 `auto_reports_a_binary_it_may_not_replace`.
3. **Replacing a running binary on Linux.** Writing into the file of a running program fails with `ETXTBSY` (and on macOS gets a signed process killed). Expected: the swap is a rename and the old inode survives as `.previous`. Test: Task 3 `a_newer_release_replaces_the_binary_and_keeps_the_previous_one` asserts `.previous` keeps the original inode and the installation path gets a new one.
4. **A watcher killed in the middle of the network work** (launchd and systemd restart a crashed `watch` every 30 s). Expected: no new request before `next_check_at` or `next_attempt_at`. Tests: Task 6 `a_failed_check_is_not_retried_after_a_restart` and `a_download_killed_midway_is_not_repeated_before_its_time`.
5. **Old and new processes on one state database during a rolling update.** Expected: a connection opened before the v7 migration still inserts heartbeats afterwards, and its rows read as `version: null`. Test: Task 9 `a_connection_opened_at_6_still_inserts_after_the_migration`.

## File Structure

| Path | Task | Responsibility |
| --- | --- | --- |
| `src/domain.rs` | 1 | `UpdateMode`, `AppConfig.updates` |
| `src/config.rs` | 1 | Schema 3, `InvalidUpdates`, `from_bytes`, `read_updates_mode` |
| `src/service.rs` | 1 | `Service::open` names the `updates` rule |
| `src/setup.rs` | 1, 7 | `--updates`; step 8 writes under the config lock with the unchanged-file guard; step 9's `update` item |
| `Cargo.toml`, `Cargo.lock` | 2 | `semver`, `flate2`, `tar` |
| `src/lib.rs` | 2 | `pub mod update;` |
| `src/update/mod.rs` (new) | 2–7 | `REPO`, `Component`, `CLI`, `COMPONENTS`, the module list |
| `src/update/version.rs` (new) | 2 | SemVer precedence, release tags, `--version` output |
| `src/update/github.rs` (new) | 2 | `Endpoint` (URL rules, loopback override), `Net` (release pages, downloads, redirects), retry hints |
| `src/update/release.rs` (new) | 2 | `CachedRelease`, `Asset`, platform names, `select`, `expected_sha256` |
| `src/update/cache.rs` (new) | 2, 3 | Cache directory, `update.json` types, `Cache::read`/`update` under `update.lock` |
| `src/update/schedule.rs` (new) | 2 | Reservations, success jitter, backoff, timestamps |
| `src/update/check.rs` (new) | 2 | `refresh`: reservation, release list, result recorded |
| `src/process.rs` | 3 | `run_captured` (keeps the start of stderr) beside `run_bounded` |
| `src/update/platform.rs` (new) | 3 | `FileIdentity`, `Blocker`, `blocker`, `installation_path`, `probe` |
| `src/update/archive.rs` (new) | 3 | Bounded gzip tar extraction |
| `src/update/install.rs` (new) | 3 | Installation lock, leftovers, `Hooks`, the install transaction |
| `src/system_service.rs` | 3, 4, 7 | `current_uid`/`is_marked` crate-visible, `unit_dir`, the `update` block in `status_account` |
| `src/update/service_files.rs` (new) | 4 | Decoding plist and unit files; listing marked service files |
| `src/update/command.rs` (new) | 4 | `mailtriage update [--check]` |
| `src/cli.rs` | 1, 4, 5, 7 | `--updates`, `update` subcommand, `watch_loop`'s `between` hook, `doctor`'s block |
| `src/update/events.rs` (new) | 5 | Update events and their text lines |
| `src/update/restart.rs` (new) | 5 | `Image`, `Restarter`: the restart rule |
| `src/update/watch.rs` (new) | 5, 6 | `WatchUpdates`: restart checks, then the update step |
| `src/update/report.rs` (new) | 7 | `update_block`, `doctor_block` |
| `src/store.rs` | 9 | Schema v7: `pass_heartbeats.version` |
| `.github/workflows/release.yml`, `.github/scripts/is_highest_release.py` (new) | 8 | One publish at a time; Latest only for the highest release |
| `tests/update_support/mod.rs` (new) | 2–5 | Loopback server, release fixtures, the sandboxed binary, the `watch` harness |
| `tests/update_config.rs` (new) | 1 | Schema 3 and `updates` |
| `tests/update_check.rs` (new) | 2 | Refresh against the loopback server |
| `tests/update_install.rs` (new) | 3 | The install transaction and fault injection |
| `tests/update_command.rs` (new) | 4 | `mailtriage update` through the binary |
| `tests/update_restart.rs` (new) | 5 | Real re-exec, failed probe, failed `exec` |
| `tests/update_watch.rs` (new) | 6 | The update step in `watch` |
| `tests/update_status.rs` (new) | 7, 9 | `service status`, `doctor`, `last_pass.version` |
| `tests/setup.rs`, `tests/config_v2.rs`, `tests/heartbeat.rs`, `tests/filing_store.rs` | 1, 7, 9 | Additions and the permitted edits |
| `docs/guide.md`, `docs/agents/index.md`, `docs/development/service-api.md`, `docs/development/releases.md`, `docs/development/verification.md`, `README.md` | 1, 4–9 | Each task documents what it adds |

---

## Phase A

### Task 1: The `updates` setting and config schema 3

**Files:**
- Create: `tests/update_config.rs`
- Modify:
  - `src/domain.rs` (`AppConfig`, new `UpdateMode`)
  - `src/config.rs` (`default_config`, `normalize`, `check_schema_version`, `load`, new `SCHEMA_VERSION`, `UPDATES_RULE`, `InvalidUpdates`, `from_bytes`, `read_updates_mode`)
  - `src/service.rs` (`Service::open`, the `config::load` error)
  - `src/setup.rs` (`SetupArgs`, `run` steps 1 and 8 and the result, `load_target`, new `write_config` and `config_lock`)
  - `src/cli.rs` (`SetupArg.updates`, `SetupArg::to_args`)
  - `tests/config_v2.rs` (permitted edit), `tests/setup.rs` (new tests)
  - `docs/guide.md`, `docs/agents/index.md`, `docs/development/service-api.md`

**Interfaces:**
- Consumes: `service::{err, err_kind, ErrorKind::{ConfigBusy, ConfigChanged}}` (on `main`).
- Produces:
  - `domain::UpdateMode { Auto (default), Notify, Off }` with `ALL`, `as_str(self) -> &'static str`, `parse(&str) -> Option<UpdateMode>`; serde lowercase.
  - `AppConfig.updates: UpdateMode` (`#[serde(default)]`, serialized after `schema_version`).
  - `config::SCHEMA_VERSION: u32 = 3`, `config::UPDATES_RULE: &str = "updates must be auto, notify or off"`, `config::InvalidUpdates` (an `std::error::Error` whose text is `UPDATES_RULE`).
  - `config::from_bytes(&[u8]) -> anyhow::Result<AppConfig>` (`load` without the read).
  - `config::read_updates_mode(&Path) -> anyhow::Result<UpdateMode>` (JSON, `schema_version` 1..=3 and `updates` only).
  - `setup::SetupArgs.updates: Option<UpdateMode>`; setup's result gains `"updates": MODE`.
  - CLI: `mailtriage setup --updates auto|notify|off`.

- [ ] **Step 1: Write the failing tests**

Create `tests/update_config.rs`:

```rust
//! Config schema 3 and its `updates` key.
use mailtriage::{config, domain::UpdateMode};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn write(dir: &Path, value: &Value) -> PathBuf {
    let path = dir.join("mailtriage.json");
    fs::write(&path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    path
}

fn on_disk(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn default_json() -> Value {
    serde_json::to_value(config::default_config()).unwrap()
}

fn mailtriage(dir: &Path, args: &[&str]) -> (Option<i32>, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
        .current_dir(dir)
        .args(args)
        .env("HOME", dir)
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env_remove("MAILTRIAGE_CONFIG")
        .output()
        .unwrap();
    (
        out.status.code(),
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null),
    )
}

#[test]
fn schema_3_is_written_with_an_explicit_updates() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mailtriage.json");
    config::save(&path, &config::default_config()).unwrap();
    let written = on_disk(&path);
    assert_eq!(written["schema_version"], 3);
    assert_eq!(written["updates"], "auto");
    let mut c = config::default_config();
    c.updates = UpdateMode::Off;
    config::save(&path, &c).unwrap();
    assert_eq!(on_disk(&path)["updates"], "off");
    assert_eq!(config::load(&path).unwrap().updates, UpdateMode::Off);
}

#[test]
fn schemas_1_and_2_read_as_auto() {
    let dir = tempfile::tempdir().unwrap();
    for version in [1, 2] {
        let mut c = default_json();
        c["schema_version"] = json!(version);
        c.as_object_mut().unwrap().remove("updates");
        let path = write(dir.path(), &c);
        assert_eq!(config::load(&path).unwrap().updates, UpdateMode::Auto);
        assert_eq!(config::read_updates_mode(&path).unwrap(), UpdateMode::Auto);
    }
}

#[test]
fn an_invalid_updates_value_is_refused_with_its_rule() {
    let dir = tempfile::tempdir().unwrap();
    for bad in [json!("sometimes"), json!("AUTO"), json!(null), json!(true)] {
        let mut c = default_json();
        c["updates"] = bad.clone();
        let path = write(dir.path(), &c);
        let e = config::load(&path).unwrap_err();
        assert!(
            e.downcast_ref::<config::InvalidUpdates>().is_some(),
            "{bad}"
        );
        assert!(config::read_updates_mode(&path).is_err(), "{bad}");
    }
    let (code, v) = mailtriage(dir.path(), &["sync", "--account", "work", "--json"]);
    assert_eq!(code, Some(2), "{v}");
    assert_eq!(v["error"]["message"], "updates must be auto, notify or off");
}

#[test]
fn the_updates_mode_is_read_without_the_rest_of_validation() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = default_json();
    c["updates"] = json!("notify");
    c["accounts"] = json!({});
    let path = write(dir.path(), &c);
    assert!(config::load(&path).is_err());
    assert_eq!(
        config::read_updates_mode(&path).unwrap(),
        UpdateMode::Notify
    );
    c["schema_version"] = json!(4);
    let path = write(dir.path(), &c);
    assert!(config::read_updates_mode(&path).is_err());
    assert!(config::read_updates_mode(&dir.path().join("missing.json")).is_err());
}

#[test]
fn init_and_categories_apply_write_schema_3_and_keep_updates() {
    let dir = tempfile::tempdir().unwrap();
    let (code, _) = mailtriage(dir.path(), &["init", "--json"]);
    assert_eq!(code, Some(0));
    let path = dir.path().join("mailtriage.json");
    assert_eq!(on_disk(&path)["schema_version"], 3);
    assert_eq!(on_disk(&path)["updates"], "auto");

    let mut c = on_disk(&path);
    c["updates"] = json!("off");
    write(dir.path(), &c);
    let (_, export) = mailtriage(
        dir.path(),
        &["categories", "export", "--account", "work", "--json"],
    );
    let mut categories = export["categories"].clone();
    categories[0]["description"] = json!("People I write with");
    let file = dir.path().join("categories.json");
    fs::write(&file, categories.to_string()).unwrap();
    let (code, v) = mailtriage(
        dir.path(),
        &[
            "categories",
            "apply",
            "--account",
            "work",
            "--file",
            file.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(code, Some(0), "{v}");
    let written = on_disk(&path);
    assert_eq!(written["updates"], "off");
    assert_eq!(written["schema_version"], 3);
    assert_eq!(
        written["accounts"]["work"]["categories"][0]["description"],
        "People I write with"
    );
}
```

Append to `tests/setup.rs` (the key command runs in step 5, between setup's read in step 1 and its write in step 8, so it can rewrite the config the way another command would):

```rust
#[test]
fn setup_writes_updates_and_every_path_keeps_an_existing_value() {
    let f = Fixture::new();
    let (out, v) = f.run(&WORK_ENV, "");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(v["setup"]["updates"], "auto");
    assert_eq!(f.config()["updates"], "auto");
    assert_eq!(f.config()["schema_version"], 3);

    let (out, v) = f.run(
        &[
            "setup",
            "--yes",
            "--update",
            "--json",
            "--himalaya-account",
            "work",
            "--updates",
            "off",
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(v["setup"]["updates"], "off");
    assert_eq!(f.config()["updates"], "off");

    // Adding an account interactively: Himalaya account 1 (home), then
    // Enter for identity, time zone, brief, folders, classifier, filing.
    let add = format!("1\n{}", "\n".repeat(6));
    // Updating from the menu: Update (1), home (1), then seven Enters.
    let menu = format!("1\n1\n{}", "\n".repeat(7));
    let runs: [(&[&str], &str); 4] = [
        (
            &["setup", "--interactive", "--json", "--account", "home"],
            &add,
        ),
        (&["setup", "--interactive", "--json"], &menu),
        (
            &[
                "setup",
                "--yes",
                "--update",
                "--json",
                "--himalaya-account",
                "work",
            ],
            "",
        ),
        (
            &[
                "setup",
                "--yes",
                "--update",
                "--json",
                "--account",
                "home",
                "--himalaya-account",
                "home",
            ],
            "",
        ),
    ];
    for (args, input) in runs {
        let (out, v) = f.run(args, input);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {}", stderr(&out));
        assert_eq!(v["setup"]["updates"], "off", "{args:?}");
        assert_eq!(f.config()["updates"], "off", "{args:?}");
    }

    let (out, _) = f.run(
        &[
            "setup",
            "--yes",
            "--update",
            "--json",
            "--himalaya-account",
            "work",
            "--updates",
            "notify",
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(f.config()["updates"], "notify");
}

/// The key command runs in step 5, between reading the config (step 1)
/// and writing it (step 8); here it rewrites the config, as another
/// command would.
#[test]
fn setup_refuses_to_save_over_a_config_changed_since_it_read_it() {
    let f = Fixture::new();
    assert_eq!(f.run(&WORK_ENV, "").0.status.code(), Some(0));
    let mut altered = f.config();
    altered["accounts"]["work"]["brief"] = json!("Changed by another command");
    let altered_path = f.home.join("altered.json");
    fs::write(&altered_path, serde_json::to_vec_pretty(&altered).unwrap()).unwrap();
    let command = format!(
        "cp '{}' '{}' && echo sk-or-fixture",
        altered_path.display(),
        f.config_path().display()
    );
    let (out, v) = f.run(
        &[
            "setup",
            "--yes",
            "--update",
            "--json",
            "--himalaya-account",
            "work",
            "--key-command",
            &command,
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(5), "{}", stderr(&out));
    assert_eq!(v["error"]["reason"], "config_changed", "{v}");
    assert!(message(&v).starts_with("step 8 (write): "), "{v}");
    assert_eq!(f.config(), altered);
}

#[test]
fn setup_refuses_while_another_command_holds_the_config_lock() {
    use fs2::FileExt;
    let f = Fixture::new();
    assert_eq!(f.run(&WORK_ENV, "").0.status.code(), Some(0));
    let before = f.config();
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(f.config_path().with_extension("lock"))
        .unwrap();
    lock.lock_exclusive().unwrap();
    let (out, v) = f.run(
        &[
            "setup",
            "--yes",
            "--update",
            "--json",
            "--himalaya-account",
            "work",
            "--brief",
            "Runs a bakery",
        ],
        "",
    );
    assert_eq!(out.status.code(), Some(5), "{}", stderr(&out));
    assert_eq!(v["error"]["reason"], "config_busy", "{v}");
    assert_eq!(f.config(), before);
}
```

`tests/config_v2.rs` pins the schema that is written. Make the permitted edits:

In `tests/config_v2.rs`, replace:

```rust
fn legacy_himalaya_config_loads_as_engine_and_saves_as_v2() {
```

with:

```rust
fn legacy_himalaya_config_loads_as_engine_and_saves_as_v3() {
```

In `tests/config_v2.rs`, replace:

```rust
    assert_eq!(loaded.schema_version, 2);
```

with:

```rust
    assert_eq!(loaded.schema_version, 3);
```

In `tests/config_v2.rs`, replace:

```rust
    assert_eq!(on_disk["schema_version"], 2);
```

with:

```rust
    assert_eq!(on_disk["schema_version"], 3);
```

In `tests/config_v2.rs`, replace:

```rust
    for version in [0, 3] {
```

with:

```rust
    for version in [0, 4] {
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --locked --test update_config --test config_v2 --test setup`
Expected: compile errors: `UpdateMode` not found in `domain`, `read_updates_mode` and `InvalidUpdates` not found in `config`.

- [ ] **Step 3: `UpdateMode` in `src/domain.rs`**

In `src/domain.rs`, replace:

```rust
pub struct AppConfig {
    pub schema_version: u32,
    pub state_dir: PathBuf,
```

with:

```rust
pub struct AppConfig {
    pub schema_version: u32,
    /// What `watch` does about new releases; a config without it means `auto`.
    #[serde(default)]
    pub updates: UpdateMode,
    pub state_dir: PathBuf,
```

In `src/domain.rs`, replace:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
```

with:

```rust
/// The `updates` setting: `auto` installs new releases in the background,
/// `notify` only reports them, `off` makes no network calls.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum UpdateMode {
    #[default]
    Auto,
    Notify,
    Off,
}
impl UpdateMode {
    pub const ALL: [UpdateMode; 3] = [UpdateMode::Auto, UpdateMode::Notify, UpdateMode::Off];
    pub fn as_str(self) -> &'static str {
        match self {
            UpdateMode::Auto => "auto",
            UpdateMode::Notify => "notify",
            UpdateMode::Off => "off",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.as_str() == value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
```

- [ ] **Step 4: Schema 3 in `src/config.rs`**

In `src/config.rs`, replace:

```rust
use crate::domain::{
    AccountConfig, AppConfig, Category, EngineConfig, FilingConfig, FilingMode, PolicyConfig,
    ProviderConfig,
};
use anyhow::{anyhow, bail, Context, Result};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs::{self, OpenOptions},
```

with:

```rust
use crate::domain::{
    AccountConfig, AppConfig, Category, EngineConfig, FilingConfig, FilingMode, PolicyConfig,
    ProviderConfig, UpdateMode,
};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fmt,
    fs::{self, OpenOptions},
```

In `src/config.rs`, replace:

```rust
    AppConfig {
        schema_version: 2,
        state_dir: "./mailtriage-state".into(),
```

with:

```rust
    AppConfig {
        schema_version: SCHEMA_VERSION,
        updates: UpdateMode::Auto,
        state_dir: "./mailtriage-state".into(),
```

In `src/config.rs`, replace:

```rust
/// Moves the legacy `himalaya` block into `engine` and marks the config as schema 2.
/// Refuses unknown schema versions so a newer file is never rewritten as schema 2.
pub fn normalize(config: &mut AppConfig) -> Result<()> {
```

with:

```rust
/// The schema every config write produces. Schema 3 adds `updates`;
/// binaries before it accept only 1 and 2, so they refuse a schema 3 file
/// instead of rewriting it without `updates`.
pub const SCHEMA_VERSION: u32 = 3;

/// The message for an invalid `updates` value (exit 2).
pub const UPDATES_RULE: &str = "updates must be auto, notify or off";

/// A config whose `updates` value is not `auto`, `notify` or `off`.
#[derive(Debug)]
pub struct InvalidUpdates;
impl fmt::Display for InvalidUpdates {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(UPDATES_RULE)
    }
}
impl std::error::Error for InvalidUpdates {}

/// The `updates` value of a parsed config file: missing means `auto`.
fn updates_value(value: &Value) -> Result<UpdateMode> {
    match value.get("updates") {
        None => Ok(UpdateMode::Auto),
        Some(v) => v
            .as_str()
            .and_then(UpdateMode::parse)
            .ok_or_else(|| InvalidUpdates.into()),
    }
}

/// Moves the legacy `himalaya` block into `engine` and marks the config as
/// schema 3. Refuses unknown schema versions so a newer file is never
/// rewritten as schema 3.
pub fn normalize(config: &mut AppConfig) -> Result<()> {
```

In `src/config.rs`, replace:

```rust
    config.schema_version = 2;
    Ok(())
}

fn check_schema_version(version: u32) -> Result<()> {
    if !(1..=2).contains(&version) {
```

with:

```rust
    config.schema_version = SCHEMA_VERSION;
    Ok(())
}

fn check_schema_version(version: u32) -> Result<()> {
    if !(1..=SCHEMA_VERSION).contains(&version) {
```

In `src/config.rs`, replace:

```rust
pub fn load(path: &Path) -> Result<AppConfig> {
    let data = fs::read(path).with_context(|| format!("read config {}", path.display()))?;
    let mut config: AppConfig = serde_json::from_slice(&data).context("parse config JSON")?;
    normalize(&mut config)?;
    validate(&config)?;
    Ok(config)
}
```

with:

```rust
pub fn load(path: &Path) -> Result<AppConfig> {
    let data = fs::read(path).with_context(|| format!("read config {}", path.display()))?;
    from_bytes(&data)
}

/// `load` for bytes already read. An invalid `updates` value is the error
/// `InvalidUpdates`.
pub fn from_bytes(data: &[u8]) -> Result<AppConfig> {
    let value: Value = serde_json::from_slice(data).context("parse config JSON")?;
    updates_value(&value)?;
    let mut config: AppConfig = serde_json::from_value(value).context("parse config JSON")?;
    normalize(&mut config)?;
    validate(&config)?;
    Ok(config)
}

/// The `updates` mode of the config at `path`, read without the rest of
/// validation, so `watch` can update itself while its passes fail on
/// another config problem. Errors when the file cannot be read, is not
/// JSON, has an unsupported `schema_version` or an invalid `updates`.
pub fn read_updates_mode(path: &Path) -> Result<UpdateMode> {
    let data = fs::read(path).with_context(|| format!("read config {}", path.display()))?;
    let value: Value = serde_json::from_slice(&data).context("parse config JSON")?;
    let version = value
        .get("schema_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("config has no schema_version"))?;
    check_schema_version(u32::try_from(version).unwrap_or(u32::MAX))?;
    updates_value(&value)
}
```

- [ ] **Step 5: `Service::open` names the rule (`src/service.rs`)**

In `src/service.rs`, replace:

```rust
        let mut cfg = config::load(&path).map_err(|_| {
            err(
```

with:

```rust
        let mut cfg = config::load(&path).map_err(|e| {
            if e.downcast_ref::<config::InvalidUpdates>().is_some() {
                return err(2, config::UPDATES_RULE);
            }
            err(
```

- [ ] **Step 6: Setup: `--updates` and the locked, guarded write (`src/setup.rs`)**

Step 1 keeps the bytes it parsed; step 8 takes the exclusive config lock (`mailtriage.lock` next to the file, the lock `Service` uses), compares the file with those bytes and only then saves.

In `src/setup.rs`, replace:

```rust
    domain::{
        AccountConfig, AppConfig, Category, EngineConfig, FilingConfig, FilingMode, HimalayaConfig,
        ProviderConfig,
    },
```

with:

```rust
    domain::{
        AccountConfig, AppConfig, Category, EngineConfig, FilingConfig, FilingMode, HimalayaConfig,
        ProviderConfig, UpdateMode,
    },
```

In `src/setup.rs`, replace:

```rust
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
```

with:

```rust
use anyhow::{anyhow, Result};
use fs2::FileExt;
use serde_json::{json, Value};
use std::{
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
```

In `src/setup.rs`, replace:

```rust
    pub interval_seconds: u64,
    pub limit: usize,
}

/// What step 1 decided.
```

with:

```rust
    pub interval_seconds: u64,
    pub limit: usize,
    /// `--updates`; `None` keeps the existing config's value (a new config
    /// gets `auto`).
    pub updates: Option<UpdateMode>,
}

/// What step 1 decided.
```

In `src/setup.rs`, replace:

```rust
    // 1. Config.
    let (mut cfg, intent) = load_target(args, path, p)?;
```

with:

```rust
    // 1. Config, and the bytes it was read from for the write in step 8.
    let (mut cfg, intent, read) = load_target(args, path, p)?;
```

In `src/setup.rs`, replace:

```rust
    cfg.provider = provider;
    config::validate(&cfg).map_err(|e| {
```

with:

```rust
    cfg.provider = provider;
    cfg.updates = args.updates.unwrap_or(cfg.updates);
    config::validate(&cfg).map_err(|e| {
```

In `src/setup.rs`, replace:

```rust
    config::save(path, &cfg).map_err(|_| unwritable())?;
    let path = fs::canonicalize(path).map_err(|_| unwritable())?;
```

with:

```rust
    write_config(path, &cfg, read.as_deref(), unwritable)?;
    let path = fs::canonicalize(path).map_err(|_| unwritable())?;
```

In `src/setup.rs`, replace:

```rust
        "filing": filing::mode_str(mode),
        "doctor": doctor,
```

with:

```rust
        "filing": filing::mode_str(mode),
        "updates": cfg.updates.as_str(),
        "doctor": doctor,
```

In `src/setup.rs`, replace:

```rust
/// Step 1: the config to change and what to do with it.
fn load_target(args: &SetupArgs, path: &Path, p: &mut Prompter) -> Result<(AppConfig, Intent)> {
    if !path.exists() {
        let mut cfg = config::default_config();
        cfg.accounts.clear();
        cfg.state_dir = PathBuf::from("state");
        return Ok((cfg, Intent::Create));
    }
    let cfg = config::load(path).map_err(|e| {
```

with:

```rust
/// Step 8's write: under the exclusive config lock, and only when the file
/// still holds the bytes step 1 read (`None`: there was no file).
fn write_config(
    path: &Path,
    cfg: &AppConfig,
    read: Option<&[u8]>,
    unwritable: impl Fn() -> anyhow::Error,
) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|_| unwritable())?;
    let lock = config_lock(path).map_err(|_| unwritable())?;
    lock.try_lock_exclusive().map_err(|_| {
        err_kind(
            5,
            ErrorKind::ConfigBusy,
            "step 8 (write): configuration is being edited; nothing was written; run setup again when the other command has finished",
        )
    })?;
    if fs::read(path).ok().as_deref() != read {
        return Err(err_kind(
            5,
            ErrorKind::ConfigChanged,
            format!(
                "step 8 (write): {} changed since setup read it; nothing was written; run setup again",
                path.display()
            ),
        ));
    }
    config::save(path, cfg).map_err(|_| unwritable())
}

/// The lock file of the commands that edit `path`: `mailtriage.lock` next
/// to `mailtriage.json`, as `Service` uses it.
fn config_lock(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path.with_extension("lock"))
}

/// Step 1: the config to change, what to do with it, and the bytes it was
/// read from (`None` when there is no config yet).
fn load_target(
    args: &SetupArgs,
    path: &Path,
    p: &mut Prompter,
) -> Result<(AppConfig, Intent, Option<Vec<u8>>)> {
    if !path.exists() {
        let mut cfg = config::default_config();
        cfg.accounts.clear();
        cfg.state_dir = PathBuf::from("state");
        return Ok((cfg, Intent::Create, None));
    }
    let bytes = fs::read(path).map_err(|e| {
        err(
            2,
            format!(
                "step 1 (config): cannot read {} ({e}); fix it or pass --config",
                path.display()
            ),
        )
    })?;
    let cfg = config::from_bytes(&bytes).map_err(|e| {
```

In `src/setup.rs`, replace:

```rust
        return Ok((cfg, named.unwrap_or(Intent::UpdateOrAdd)));
    }
    p.say(&format!("A config already exists at {}.", path.display()));
    if let Some(intent) = named {
        return Ok((cfg, intent));
    }
```

with:

```rust
        return Ok((cfg, named.unwrap_or(Intent::UpdateOrAdd), Some(bytes)));
    }
    p.say(&format!("A config already exists at {}.", path.display()));
    if let Some(intent) = named {
        return Ok((cfg, intent, Some(bytes)));
    }
```

In `src/setup.rs`, replace:

```rust
            let name = names[pick].clone();
            Ok((cfg, Intent::Update(name)))
        }
        1 => Ok((cfg, Intent::Add)),
```

with:

```rust
            let name = names[pick].clone();
            Ok((cfg, Intent::Update(name), Some(bytes)))
        }
        1 => Ok((cfg, Intent::Add, Some(bytes))),
```

- [ ] **Step 7: The CLI flag (`src/cli.rs`)**

In `src/cli.rs`, replace:

```rust
    domain::{Category, FilingMode},
```

with:

```rust
    domain::{Category, FilingMode, UpdateMode},
```

In `src/cli.rs`, replace:

```rust
    #[arg(long, default_value_t = 100, value_parser = parse_limit)]
    limit: usize,
}

impl SetupArg {
```

with:

```rust
    #[arg(long, default_value_t = 100, value_parser = parse_limit)]
    limit: usize,
    /// What `watch` does about new releases (default: keep the config's value; auto for a new config).
    #[arg(long, value_parser = ["auto", "notify", "off"])]
    updates: Option<String>,
}

impl SetupArg {
```

In `src/cli.rs`, replace:

```rust
            interval_seconds: self.interval_seconds,
            limit: self.limit,
        }
    }
}
```

with:

```rust
            interval_seconds: self.interval_seconds,
            limit: self.limit,
            updates: self.updates.as_deref().and_then(UpdateMode::parse),
        }
    }
}
```

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test --locked --test update_config --test config_v2 --test setup && cargo test --locked --lib golden`
Expected: all PASS; the generation hash and binding identity golden values are unchanged.

- [ ] **Step 9: Document the setting**

The `#updates` anchor these edits link to is added by Task 4 on this branch.

In `docs/guide.md`, replace:

```markdown
{
  "schema_version": 2,
  "state_dir": "state",
```

with:

```markdown
{
  "schema_version": 3,
  "updates": "auto",
  "state_dir": "state",
```

In `docs/guide.md`, replace:

```markdown
Each command that opens the configuration checks the whole file. A file that breaks a rule below is refused with exit code 2 and `invalid configuration; check required fields, categories and provider settings`; the message does not name the field.
```

with:

```markdown
Each command that opens the configuration checks the whole file. A file that breaks a rule below is refused with exit code 2 and `invalid configuration; check required fields, categories and provider settings`; the message does not name the field, except for an invalid `updates`, which exits 2 with `updates must be auto, notify or off`.
```

In `docs/guide.md`, replace:

```markdown
| `schema_version` | `2`, as written by `init` and `setup`. |
```

with:

```markdown
| `schema_version` | `3`, as written by `init`, `setup` and every command that edits the file. mailtriage reads schemas 1 to 3. Releases before automatic updates read only 1 and 2, so they refuse a schema 3 file instead of rewriting it without `updates`. |
| `updates` | What `watch` does about new releases: `"auto"` installs them, `"notify"` only reports them, `"off"` makes no network call. A config without the key means `"auto"`. See [Updates](#updates). |
```

In `docs/guide.md`, replace:

```markdown
| 8. Write | Validates and writes the config. | none |
```

with:

```markdown
| 8. Write | Validates and writes the config, including `updates`. | `--updates` |
```

In `docs/guide.md`, replace:

```markdown
**Step 8, write.** Setup validates the whole config and writes it atomically with mode 0600.
```

with:

```markdown
**Step 8, write.** `--updates auto|notify|off` sets [`updates`](#updates), with no prompt: a new config gets `auto`, and an existing one keeps its value unless `--updates` is given. Setup validates the whole config and writes it atomically with mode 0600, under the configuration lock (`mailtriage.lock`). When another command holds that lock, setup exits 5 (reason `config_busy`); when the file changed since step 1 read it, setup exits 5 (reason `config_changed`). Either way it writes nothing; run it again.
```

In `docs/guide.md`, replace:

```markdown
"provider":"openrouter","service":null}}
```

with:

```markdown
"provider":"openrouter","service":null,"updates":"auto"}}
```

In `docs/guide.md`, replace:

```markdown
| `filing` | `off`, `dry_run` or `live`. |
| `doctor` |
```

with:

```markdown
| `filing` | `off`, `dry_run` or `live`. |
| `updates` | The config's `updates` after this run: `auto`, `notify` or `off`. |
| `doctor` |
```

In `docs/guide.md`, replace:

```markdown
the account is bound to another mailbox (its identity, Himalaya account or IMAP server would change); a service file exists that mailtriage did not write. |
```

with:

```markdown
the account is bound to another mailbox (its identity, Himalaya account or IMAP server would change); a service file exists that mailtriage did not write; another command is editing the config (`config_busy`), or it changed since setup read it (`config_changed`). |
```

In `docs/agents/index.md`, replace:

```markdown
   - Optional: `--account`, `--identity`, `--timezone`, `--brief`, `--mailbox` (repeat for several folders), `--filing off|dry-run`, `--interval-seconds`, `--limit`.
```

with:

```markdown
   - Optional: `--account`, `--identity`, `--timezone`, `--brief`, `--mailbox` (repeat for several folders), `--filing off|dry-run`, `--interval-seconds`, `--limit`, `--updates auto|notify|off` (what `watch` does about new releases; see [Updates](guide.md#updates)).
```

In `docs/development/service-api.md`, replace:

```markdown
  "filing": "dry_run",
  "doctor": {"ready": false, "items": [
```

with:

```markdown
  "filing": "dry_run",
  "updates": "auto",
  "doctor": {"ready": false, "items": [
```

In `docs/development/service-api.md`, replace:

```markdown
- `filing`: `off`, `dry_run` or `live`.
- `doctor.items[].check`
```

with:

```markdown
- `filing`: `off`, `dry_run` or `live`.
- `updates`: the config's `updates` after this run (`auto`, `notify` or `off`);
  `--updates` sets it, otherwise an existing config keeps its value.
- `doctor.items[].check`
```

In `docs/development/service-api.md`, replace:

```markdown
manager failure, unwritable config; 5 config exists without `--update`, a
binding-changing update, unmarked service file.
```

with:

```markdown
manager failure, unwritable config; 5 config exists without `--update`, a
binding-changing update, unmarked service file, another command holding the
config lock (`config_busy`), or a config that changed since step 1 read it
(`config_changed`); step 8 writes under the exclusive config lock.
```

- [ ] **Step 10: Full check and commit**

Run: `cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`
Expected: all pass.

```bash
git add src/domain.rs src/config.rs src/service.rs src/setup.rs src/cli.rs tests/update_config.rs tests/config_v2.rs tests/setup.rs docs/guide.md docs/agents/index.md docs/development/service-api.md
git commit -m "Add the updates setting and config schema 3

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Release information from GitHub

**Files:**
- Create: `src/update/mod.rs`, `src/update/version.rs`, `src/update/github.rs`, `src/update/release.rs`, `src/update/cache.rs`, `src/update/schedule.rs`, `src/update/check.rs`, `tests/update_support/mod.rs`, `tests/update_check.rs`
- Modify: `Cargo.toml`, `Cargo.lock`, `src/lib.rs`

**Interfaces:**
- Consumes: `domain::UpdateMode` (Task 1).
- Produces:
  - `update::REPO`, `update::Component { name: &'static str }` with `archive_name(self, &semver::Version, platform: &str) -> String`, `update::CLI`, `update::COMPONENTS: &[Component]`.
  - `update::version::{RUNNING: &str, running() -> semver::Version, parse_tag(&str) -> Option<Version>, parse_version_output(program: &str, stdout: &[u8]) -> Option<Version>, is_newer(candidate: &Version, installed: &Version) -> bool}`.
  - `update::github::Endpoint` (`github()`, `from_env()`, `with_override(Option<&str>)`, `releases_url(&self) -> Url`, `allows(&self, &Url) -> bool`), `github_url(&Url) -> bool`, `URL_OVERRIDE`, `MAX_PAGES`, `MAX_ASSET_BYTES`, `MAX_SUMS_BYTES`, `CheckError { message, retry_after, rate_reset }`, `ReleaseJson`, `AssetJson`, `Net` (`new(Endpoint) -> anyhow::Result<Net>`, `endpoint(&self)`, `releases(&self) -> Result<Vec<ReleaseJson>, CheckError>`, `download(&self, url: &str, listed_size: u64, max: u64) -> anyhow::Result<Vec<u8>>`), `next_link(&str) -> Option<String>`, `retry_after(&HeaderMap, DateTime<Utc>)`, `rate_reset(&HeaderMap)`.
  - `update::release::{CachedRelease { version, release_url, published_at, archives: BTreeMap<String, Option<Asset>>, sums: Option<Asset> }, Asset { name, url, size }, platform() -> Option<&'static str>, platform_for(arch, os, gnu) -> Option<&'static str>, select(&[ReleaseJson], Option<&str>, &[Component]) -> Option<CachedRelease>, expected_sha256(sums: &str, name: &str) -> Result<String, String>}`.
  - `update::cache::{FILE, LOCK, cache_dir(macos, home, xdg) -> Option<PathBuf>, default_dir(), CacheFile, ErrorRecord { at, message }, ConfigEntry { mode, notified_version }, InstallEntry { version, at, last_error, failures, next_attempt_at }, Cache }` with `Cache::new(PathBuf)`, `Cache::for_user() -> Option<Cache>`, `dir()`, `read(&self) -> CacheFile`, `update<T>(&self, FnOnce(&mut CacheFile) -> T) -> anyhow::Result<T>`.
  - `update::schedule::{reservation, after_success, after_failure, install_backoff, due, stamp, parse, random_unit}`.
  - `update::check::{Reservation::{Required, BestEffort}, Checked { release, checked_at, warnings }, refresh(&Net, &Cache, Reservation) -> anyhow::Result<Checked>}`.
  - Tests: `tests/update_support/mod.rs` with `LIST`, `Reply`, `Request`, `Server` (`start`, `reply`, `on_request`, `requests`, `count`, `wait_for`, `url`, `list`, `publish`, `publish_with_sums`, `release_json`), `download_path`, `platform`, `sha256_hex`.

- [ ] **Step 1: Add the dependencies**

In `Cargo.toml`, replace:

```toml
fs2 = "0.4"
```

with:

```toml
fs2 = "0.4"
flate2 = { version = "1.1", default-features = false, features = ["rust_backend"] }
```

In `Cargo.toml`, replace:

```toml
rusqlite = { version = "0.37", features = ["bundled"] }
```

with:

```toml
rusqlite = { version = "0.37", features = ["bundled"] }
semver = "1"
```

In `Cargo.toml`, replace:

```toml
sha2 = "0.10"
```

with:

```toml
sha2 = "0.10"
tar = { version = "0.4.44", default-features = false }
```

`flate2` and `tar` are used from Task 3 on (and by the test support below); `default-features = false` keeps `tar` from pulling in `xattr` and `flate2` on its pure-Rust backend.

Run: `cargo build && git diff --stat Cargo.lock && git diff Cargo.lock | grep '^+name'`
Expected: the build succeeds and the lock file gains exactly `adler2`, `crc32fast`, `filetime`, `flate2` (1.1.x), `miniz_oxide`, `semver` (1.0.x), `simd-adler32` and `tar` (0.4.4x). From here on use `--locked`.

- [ ] **Step 2: Write the failing integration tests**

The support module is shared by every update test file (each includes it with `mod update_support;`). It starts with the loopback server; later tasks append fixtures and harnesses to it.

Create `tests/update_support/mod.rs`:

```rust
#![allow(dead_code)]
//! Shared by the update tests: a loopback HTTP server that plays GitHub,
//! release fixtures, a copy of the binary under test in a temporary
//! directory, and a harness for `watch`. Nothing here touches the real
//! HOME, cache or binaries.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

/// The first page of the release list, as `Endpoint::releases_url` asks.
pub const LIST: &str = "/repos/wir-drei-digital/mailtriage/releases?per_page=30";

/// One canned answer.
#[derive(Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub delay: Duration,
}

impl Reply {
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            headers: vec![],
            body: body.into(),
            delay: Duration::ZERO,
        }
    }
    pub fn status(status: u16) -> Self {
        Self {
            status,
            ..Self::ok("")
        }
    }
    pub fn redirect(location: &str) -> Self {
        Self::status(302).header("Location", location)
    }
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
    pub fn delayed(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

/// A request the server received: its target and headers (names lowercase).
#[derive(Clone, Debug)]
pub struct Request {
    pub target: String,
    pub headers: HashMap<String, String>,
}

type Hook = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct State {
    replies: HashMap<String, Reply>,
    hooks: HashMap<String, Hook>,
    log: Vec<Request>,
}

/// An HTTP/1.1 server on 127.0.0.1 with one thread per connection. Unknown
/// targets get 404.
pub struct Server {
    pub base: String,
    state: Arc<Mutex<State>>,
}

impl Server {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let state = Arc::new(Mutex::new(State::default()));
        let shared = Arc::clone(&state);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let state = Arc::clone(&shared);
                thread::spawn(move || serve(stream, &state));
            }
        });
        Self { base, state }
    }

    /// Answers `target` (path and query) with `reply`.
    pub fn reply(&self, target: &str, reply: Reply) {
        self.state
            .lock()
            .unwrap()
            .replies
            .insert(target.into(), reply);
    }

    /// Runs `hook` when `target` is requested, before answering.
    pub fn on_request(&self, target: &str, hook: impl Fn() + Send + Sync + 'static) {
        self.state
            .lock()
            .unwrap()
            .hooks
            .insert(target.into(), Arc::new(hook));
    }

    pub fn requests(&self) -> Vec<Request> {
        self.state.lock().unwrap().log.clone()
    }

    /// How many requests targeted something starting with `prefix`.
    pub fn count(&self, prefix: &str) -> usize {
        self.requests()
            .iter()
            .filter(|r| r.target.starts_with(prefix))
            .count()
    }

    /// Waits up to 30 s until `count` requests targeted `prefix`.
    pub fn wait_for(&self, prefix: &str, count: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while self.count(prefix) < count {
            assert!(
                std::time::Instant::now() < deadline,
                "no request for {prefix}; got {:?}",
                self.requests()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn url(&self, target: &str) -> String {
        format!("{}{target}", self.base)
    }

    /// Serves `releases` as the one-page release list.
    pub fn list(&self, releases: &[Value]) {
        self.reply(LIST, Reply::ok(Value::from(releases.to_vec()).to_string()));
    }

    /// Publishes stable `version` with `archive` as this host's archive and
    /// a matching `SHA256SUMS`, as the only release. Returns the archive name.
    pub fn publish(&self, version: &str, archive: &[u8]) -> String {
        let name = format!("mailtriage-v{version}-{}.tar.gz", platform());
        let sums = format!("{}  {name}\n", sha256_hex(archive));
        self.publish_with_sums(version, archive, &sums);
        name
    }

    /// `publish` with the given `SHA256SUMS` text.
    pub fn publish_with_sums(&self, version: &str, archive: &[u8], sums: &str) {
        let name = format!("mailtriage-v{version}-{}.tar.gz", platform());
        self.reply(&download_path(version, &name), Reply::ok(archive));
        self.reply(&download_path(version, "SHA256SUMS"), Reply::ok(sums));
        self.list(&[self.release_json(
            version,
            &[(&name, archive.len()), ("SHA256SUMS", sums.len())],
        )]);
    }

    /// One entry of the release list, its assets served under `/download/`.
    pub fn release_json(&self, version: &str, assets: &[(&str, usize)]) -> Value {
        json!({
            "tag_name": format!("v{version}"),
            "draft": false,
            "prerelease": false,
            "html_url": format!("https://github.com/wir-drei-digital/mailtriage/releases/tag/v{version}"),
            "published_at": "2026-11-02T09:00:00Z",
            "assets": assets.iter().map(|(name, size)| json!({
                "name": name,
                "size": size,
                "browser_download_url": self.url(&download_path(version, name)),
            })).collect::<Vec<_>>(),
        })
    }
}

pub fn download_path(version: &str, name: &str) -> String {
    format!("/download/v{version}/{name}")
}

fn serve(stream: TcpStream, state: &Mutex<State>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let target = line.split_whitespace().nth(1).unwrap_or("").to_owned();
    let mut headers = HashMap::new();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
            break;
        }
        if let Some((name, value)) = header.trim_end().split_once(':') {
            headers.insert(name.to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let (reply, hook) = {
        let mut state = state.lock().unwrap();
        state.log.push(Request {
            target: target.clone(),
            headers,
        });
        (
            state
                .replies
                .get(&target)
                .cloned()
                .unwrap_or(Reply::status(404)),
            state.hooks.get(&target).cloned(),
        )
    };
    if let Some(hook) = hook {
        hook();
    }
    thread::sleep(reply.delay);
    let mut head = format!(
        "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n",
        reply.status,
        reply.body.len()
    );
    for (name, value) in &reply.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let mut out = stream;
    let _ = out.write_all(head.as_bytes());
    let _ = out.write_all(&reply.body);
    let _ = out.flush();
}

/// This host's platform name in archive names.
pub fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos-arm64"
    } else if cfg!(target_arch = "aarch64") {
        "linux-arm64"
    } else {
        "linux-amd64"
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
```

Create `tests/update_check.rs`:

```rust
#![cfg(unix)]
//! Refreshing release information against a loopback server: pagination,
//! the reservation, scheduling, retry hints, redirects and limits.
mod update_support;
use chrono::{DateTime, Duration, Utc};
use mailtriage::update::{
    cache::Cache,
    check::{self, Reservation},
    github::{Endpoint, Net},
    version,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use update_support::{Reply, Server, LIST};

fn setup() -> (Server, Net, Cache, tempfile::TempDir) {
    let server = Server::start();
    let net = Net::new(Endpoint::with_override(Some(&server.base))).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().join("cache"));
    (server, net, cache, dir)
}

fn at(text: &Option<String>) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text.as_deref().unwrap())
        .unwrap()
        .with_timezone(&Utc)
}

fn rc(n: usize) -> Value {
    json!({"tag_name": format!("v9.0.0-rc.{n}"), "draft": false, "prerelease": true, "assets": []})
}

#[test]
fn the_highest_stable_release_may_sit_on_page_two() {
    let (server, net, cache, _dir) = setup();
    let page2 = format!("{LIST}&page=2");
    let first: Vec<Value> = (0..30).map(rc).collect();
    server.reply(
        LIST,
        Reply::ok(Value::from(first).to_string())
            .header("Link", &format!("<{}>; rel=\"next\"", server.url(&page2))),
    );
    let second = vec![
        server.release_json("0.9.9", &[]),
        server.release_json("0.10.0", &[]),
        json!({"tag_name": "v1.0.0", "draft": true, "prerelease": false, "assets": []}),
    ];
    server.reply(&page2, Reply::ok(Value::from(second).to_string()));
    let checked = check::refresh(&net, &cache, Reservation::Required).unwrap();
    assert_eq!(checked.release.unwrap().version, "0.10.0");
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    let headers = &requests[0].headers;
    assert_eq!(
        headers["user-agent"],
        format!("mailtriage/{}", version::RUNNING)
    );
    assert_eq!(headers["accept"], "application/vnd.github+json");
    assert_eq!(headers["x-github-api-version"], "2022-11-28");
    assert_eq!(cache.read().release.unwrap().version, "0.10.0");
}

#[test]
fn a_failing_page_fails_the_whole_check() {
    let (server, net, cache, _dir) = setup();
    let page2 = format!("{LIST}&page=2");
    server.reply(
        LIST,
        Reply::ok(Value::from(vec![server.release_json("0.3.0", &[])]).to_string())
            .header("Link", &format!("<{}>; rel=\"next\"", server.url(&page2))),
    );
    server.reply(&page2, Reply::status(500));
    let e = check::refresh(&net, &cache, Reservation::Required).unwrap_err();
    assert_eq!(
        e.to_string(),
        "cannot read the release list: GitHub answered 500"
    );
    let file = cache.read();
    assert_eq!(file.release, None);
    assert_eq!(file.check_failures, 1);
    assert_eq!(
        file.last_check_error.unwrap().message,
        "cannot read the release list: GitHub answered 500"
    );
}

#[test]
fn more_than_twenty_pages_or_a_page_outside_github_fail() {
    let (server, net, cache, _dir) = setup();
    for page in 0..=20 {
        let target = if page == 0 {
            LIST.to_owned()
        } else {
            format!("{LIST}&page={page}")
        };
        let next = server.url(&format!("{LIST}&page={}", page + 1));
        server.reply(
            &target,
            Reply::ok("[]").header("Link", &format!("<{next}>; rel=\"next\"")),
        );
    }
    let e = check::refresh(&net, &cache, Reservation::Required).unwrap_err();
    assert_eq!(e.to_string(), "the release list has more than 20 pages");
    assert_eq!(server.count(LIST), 20);

    server.reply(
        LIST,
        Reply::ok("[]").header("Link", "<https://evil.example/releases>; rel=\"next\""),
    );
    let e = check::refresh(&net, &cache, Reservation::Required).unwrap_err();
    assert_eq!(e.to_string(), "a release list page is outside GitHub");
}

#[test]
fn the_reservation_is_written_before_the_request() {
    let (server, net, cache, dir) = setup();
    server.list(&[]);
    let seen = Arc::new(Mutex::new(None));
    let (file, store) = (dir.path().join("cache/update.json"), Arc::clone(&seen));
    server.on_request(LIST, move || {
        let text = std::fs::read_to_string(&file).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        *store.lock().unwrap() = value["next_check_at"].as_str().map(str::to_owned);
    });
    let before = Utc::now();
    let checked = check::refresh(&net, &cache, Reservation::Required).unwrap();
    assert_eq!(checked.release, None);
    let reserved = at(&seen.lock().unwrap().clone());
    assert!(reserved >= before + Duration::minutes(59), "{reserved}");
    assert!(reserved <= Utc::now() + Duration::hours(1));
}

#[test]
fn success_schedules_a_day_plus_jitter() {
    let (server, net, cache, _dir) = setup();
    server.list(&[server.release_json("0.3.0", &[])]);
    let before = Utc::now();
    check::refresh(&net, &cache, Reservation::Required).unwrap();
    let file = cache.read();
    let next = at(&file.next_check_at);
    assert!(next >= before + Duration::hours(24), "{next}");
    assert!(next <= Utc::now() + Duration::hours(25), "{next}");
    assert_eq!(file.check_failures, 0);
    assert_eq!(file.last_check_error, None);
    assert!(at(&file.checked_at) >= before - Duration::seconds(1));
}

#[test]
fn rate_limits_and_retry_after_set_the_next_check() {
    let (server, net, cache, _dir) = setup();
    let reset = Utc::now() + Duration::hours(3);
    server.reply(
        LIST,
        Reply::status(403)
            .header("X-RateLimit-Remaining", "0")
            .header("X-RateLimit-Reset", &reset.timestamp().to_string()),
    );
    let e = check::refresh(&net, &cache, Reservation::Required).unwrap_err();
    assert_eq!(
        e.to_string(),
        "cannot read the release list: GitHub's rate limit was reached"
    );
    assert_eq!(
        at(&cache.read().next_check_at).timestamp(),
        reset.timestamp()
    );

    server.reply(LIST, Reply::status(429).header("Retry-After", "18000"));
    let before = Utc::now();
    check::refresh(&net, &cache, Reservation::Required).unwrap_err();
    let file = cache.read();
    assert_eq!(file.check_failures, 2);
    let next = at(&file.next_check_at);
    assert!(
        next >= before + Duration::hours(5) - Duration::seconds(1),
        "{next}"
    );
}

#[test]
fn without_a_writable_cache_watch_makes_no_request() {
    let server = Server::start();
    let net = Net::new(Endpoint::with_override(Some(&server.base))).unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("file"), "").unwrap();
    let cache = Cache::new(dir.path().join("file/cache"));
    server.list(&[]);
    let e = check::refresh(&net, &cache, Reservation::Required).unwrap_err();
    assert!(
        e.to_string().starts_with("cannot write the update cache"),
        "{e}"
    );
    assert_eq!(server.requests().len(), 0);
    let checked = check::refresh(&net, &cache, Reservation::BestEffort).unwrap();
    assert_eq!(server.requests().len(), 1);
    assert_eq!(checked.warnings.len(), 2, "{:?}", checked.warnings);
}

#[test]
fn ten_redirects_are_followed_and_the_eleventh_fails() {
    let server = Server::start();
    let net = Net::new(Endpoint::with_override(Some(&server.base))).unwrap();
    for hop in 0..11 {
        server.reply(
            &format!("/hop/{hop}"),
            Reply::redirect(&server.url(&format!("/hop/{}", hop + 1))),
        );
    }
    server.reply("/hop/10", Reply::ok("ten"));
    assert_eq!(net.download(&server.url("/hop/0"), 3, 100).unwrap(), b"ten");
    server.reply("/hop/10", Reply::redirect(&server.url("/hop/11")));
    server.reply("/hop/11", Reply::ok("eleven"));
    let e = net.download(&server.url("/hop/0"), 6, 100).unwrap_err();
    assert_eq!(e.to_string(), "more than 10 redirects");
}

#[test]
fn downloads_stay_inside_the_rules_and_the_size_limit() {
    let server = Server::start();
    let net = Net::new(Endpoint::with_override(Some(&server.base))).unwrap();
    server.reply("/away", Reply::redirect("https://evil.example/x"));
    let e = net.download(&server.url("/away"), 1, 100).unwrap_err();
    assert_eq!(e.to_string(), "a redirect left GitHub");
    server.reply(
        "/plain",
        Reply::redirect("http://objects.githubusercontent.com/x"),
    );
    assert!(net.download(&server.url("/plain"), 1, 100).is_err());
    assert_eq!(
        net.download(&server.url("/big"), 101, 100)
            .unwrap_err()
            .to_string(),
        "it is larger than 100 bytes"
    );
    server.reply("/big", Reply::ok(vec![b'x'; 101]));
    assert!(net.download(&server.url("/big"), 1, 100).is_err());
    assert!(net
        .download("https://evil.example/x", 1, 100)
        .unwrap_err()
        .to_string()
        .contains("outside GitHub"));
    // Only the loopback origin of the override is allowed, not other ports.
    assert!(net.download("http://127.0.0.1:9/x", 1, 100).is_err());
}
```

- [ ] **Step 3: Run them to verify they fail**

Run: `cargo test --locked --test update_check`
Expected: compile error: unresolved import `mailtriage::update`.

- [ ] **Step 4: The module root and versions**

Add `pub mod update;` to `src/lib.rs` after `pub mod system_service;`.

Create `src/update/mod.rs`:

```rust
//! Automatic updates from GitHub Releases (spec:
//! design/specs/2026-10-06-auto-update-design.md): release
//! information, installing a release, restarting `watch` onto a replaced
//! binary, and reporting what is installed.
pub mod cache;
pub mod check;
pub mod github;
pub mod release;
pub mod schedule;
pub mod version;

/// The repository releases come from.
pub const REPO: &str = "wir-drei-digital/mailtriage";

/// A program the updater installs. This spec defines `mailtriage`; the tray
/// spec adds `mailtriage-tray`, installed the same way next to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Component {
    /// The key in the cache's `release.archives`, the archive name prefix,
    /// the file name inside the archive and next to the CLI, and the first
    /// word of its `--version` output.
    pub name: &'static str,
}

/// The `mailtriage` CLI.
pub const CLI: Component = Component { name: "mailtriage" };

/// Every component a release check records archives for.
pub const COMPONENTS: &[Component] = &[CLI];

impl Component {
    /// `NAME-vX.Y.Z-PLATFORM.tar.gz`.
    pub fn archive_name(self, version: &semver::Version, platform: &str) -> String {
        format!("{}-v{version}-{platform}.tar.gz", self.name)
    }
}
```

Create `src/update/version.rs`:

```rust
//! Versions: SemVer precedence, release tags and `--version` output.
use semver::Version;
use std::cmp::Ordering;

/// The version compiled into this process.
pub const RUNNING: &str = env!("CARGO_PKG_VERSION");

/// `RUNNING`, parsed.
pub fn running() -> Version {
    Version::parse(RUNNING).expect("the package version is SemVer")
}

/// The version of a stable release tag: exactly `vX.Y.Z`, decimal numbers
/// without leading zeros. Anything else (`v1.2.3-rc.1`, `1.2.3`, `v1.2`,
/// `v01.2.3`) is not a stable release.
pub fn parse_tag(tag: &str) -> Option<Version> {
    let parts: Vec<&str> = tag.strip_prefix('v')?.split('.').collect();
    let [major, minor, patch] = parts[..] else {
        return None;
    };
    let number = |part: &str| -> Option<u64> {
        let decimal = !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
        (decimal && (part == "0" || !part.starts_with('0')))
            .then(|| part.parse().ok())
            .flatten()
    };
    Some(Version::new(number(major)?, number(minor)?, number(patch)?))
}

/// The version `program --version` printed: exactly `<program> <semver>`,
/// optionally followed by one newline.
pub fn parse_version_output(program: &str, stdout: &[u8]) -> Option<Version> {
    let text = std::str::from_utf8(stdout).ok()?;
    let line = text.strip_suffix('\n').unwrap_or(text);
    let version = line.strip_prefix(program)?.strip_prefix(' ')?;
    if version.contains(char::is_whitespace) {
        return None;
    }
    Version::parse(version).ok()
}

/// Whether `candidate` replaces `installed`: strictly greater by SemVer
/// precedence (build metadata ignored), so never a downgrade or a reinstall.
pub fn is_newer(candidate: &Version, installed: &Version) -> bool {
    candidate.cmp_precedence(installed) == Ordering::Greater
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn precedence_follows_semver() {
        assert!(is_newer(&v("0.10.0"), &v("0.9.9")));
        assert!(is_newer(&v("0.3.0"), &v("0.3.0-rc.1")));
        assert!(is_newer(&v("0.3.1"), &v("0.3.0")));
        // A running release candidate never installs an older stable release.
        assert!(!is_newer(&v("0.3.0"), &v("0.4.0-rc.1")));
        // Equal and older versions are never installed.
        assert!(!is_newer(&v("0.3.0"), &v("0.3.0")));
        assert!(!is_newer(&v("0.2.9"), &v("0.3.0")));
        assert!(!is_newer(&v("0.3.0"), &v("0.3.0+build.7")));
    }

    #[test]
    fn only_exact_stable_tags_are_releases() {
        assert_eq!(parse_tag("v1.2.3"), Some(v("1.2.3")));
        assert_eq!(parse_tag("v0.10.0"), Some(v("0.10.0")));
        assert_eq!(parse_tag("v0.0.0"), Some(v("0.0.0")));
        for tag in [
            "v1.2.3-rc.1",
            "1.2.3",
            "v1.2",
            "v01.2.3",
            "v1.02.3",
            "v1.2.3.4",
            "v1.2.3+b",
            "v1..3",
            "v-1.2.3",
            "V1.2.3",
            "v1.2.3 ",
        ] {
            assert_eq!(parse_tag(tag), None, "{tag}");
        }
    }

    #[test]
    fn version_output_is_one_exact_line() {
        assert_eq!(
            parse_version_output("mailtriage", b"mailtriage 0.3.0\n"),
            Some(v("0.3.0"))
        );
        assert_eq!(
            parse_version_output("mailtriage", b"mailtriage 0.4.0-rc.1"),
            Some(v("0.4.0-rc.1"))
        );
        for bad in [
            &b"mailtriage 0.3.0\n\n"[..],
            b"mailtriage  0.3.0\n",
            b"mailtriage-tray 0.3.0\n",
            b"mailtriage 0.3\n",
            b"mailtriage 0.3.0\r\n",
            b"Mailtriage 0.3.0\n",
            b"",
        ] {
            assert_eq!(parse_version_output("mailtriage", bad), None, "{bad:?}");
        }
    }
}
```

- [ ] **Step 5: URL rules, the release list and downloads**

`Net` follows a redirect only when `Endpoint::allows` accepts its target, and never more than 10 (`attempt.previous()` holds the first URL too, so the 11th redirect sees 11 entries). A per-request `timeout` in reqwest 0.12 bounds the whole request including the body.

Create `src/update/github.rs`:

```rust
//! GitHub access: the URL rules every request and redirect passes, the
//! paginated release list, and bounded downloads. No token is sent.
use super::version;
use chrono::{DateTime, TimeZone, Utc};
use reqwest::{
    blocking::{Client, Response},
    header::{HeaderMap, ACCEPT},
    redirect, Url,
};
use serde::Deserialize;
use std::{fmt, io::Read, time::Duration};

/// The API base URL.
pub const API_BASE: &str = "https://api.github.com";
/// The hidden variable that replaces `API_BASE` in tests; honoured only for
/// a loopback host.
pub const URL_OVERRIDE: &str = "MAILTRIAGE_UPDATE_URL";
/// The release list is read page by page; more pages fail the check.
pub const MAX_PAGES: usize = 20;
/// The largest asset (listed `size` or actual body) a download accepts.
pub const MAX_ASSET_BYTES: u64 = 200 * 1024 * 1024;
/// The largest `SHA256SUMS` accepted.
pub const MAX_SUMS_BYTES: u64 = 1024 * 1024;
const MAX_PAGE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_REDIRECTS: usize = 10;
const API_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// Where release information comes from.
#[derive(Debug, Clone)]
pub struct Endpoint {
    api_base: Url,
    /// Set only when `MAILTRIAGE_UPDATE_URL` names a loopback host: then
    /// plain HTTP and that origin are allowed too.
    loopback: Option<Url>,
}

impl Endpoint {
    pub fn github() -> Self {
        Self {
            api_base: Url::parse(API_BASE).expect("API_BASE is a URL"),
            loopback: None,
        }
    }

    /// GitHub, or `MAILTRIAGE_UPDATE_URL` when it names 127.0.0.1, ::1 or
    /// localhost.
    pub fn from_env() -> Self {
        Self::with_override(std::env::var(URL_OVERRIDE).ok().as_deref())
    }

    pub fn with_override(value: Option<&str>) -> Self {
        match value.and_then(|v| Url::parse(v).ok()).filter(is_loopback) {
            Some(url) => Self {
                api_base: url.clone(),
                loopback: Some(url),
            },
            None => Self::github(),
        }
    }

    /// The first page of the release list.
    pub fn releases_url(&self) -> Url {
        let mut url = self.api_base.clone();
        let base = self.api_base.path().trim_end_matches('/').to_owned();
        url.set_path(&format!("{base}/repos/{}/releases", super::REPO));
        url.set_query(Some("per_page=30"));
        url
    }

    /// Whether a request, or a redirect, may go to `url`.
    pub fn allows(&self, url: &Url) -> bool {
        github_url(url)
            || self.loopback.as_ref().is_some_and(|base| {
                url.scheme() == base.scheme()
                    && url.host_str() == base.host_str()
                    && url.port_or_known_default() == base.port_or_known_default()
                    && url.username().is_empty()
                    && url.password().is_none()
            })
    }
}

fn is_loopback(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"))
}

/// GitHub's own URLs: HTTPS on port 443 without credentials, to
/// `github.com` or a host ending in `.github.com` or
/// `.githubusercontent.com`.
pub fn github_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && url.host_str().is_some_and(|host| {
            host == "github.com"
                || host.ends_with(".github.com")
                || host.ends_with(".githubusercontent.com")
        })
}

/// A failed check, with the server's hints for when to retry.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckError {
    pub message: String,
    /// From `Retry-After`.
    pub retry_after: Option<DateTime<Utc>>,
    /// From `X-RateLimit-Reset` when `X-RateLimit-Remaining` is 0.
    pub rate_reset: Option<DateTime<Utc>>,
}

impl CheckError {
    fn plain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retry_after: None,
            rate_reset: None,
        }
    }
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CheckError {}

/// One entry of the release list; unknown fields are ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseJson {
    pub tag_name: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub assets: Vec<AssetJson>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetJson {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
}

/// An HTTP client that follows only allowed redirects, at most 10.
pub struct Net {
    endpoint: Endpoint,
    client: Client,
}

impl Net {
    pub fn new(endpoint: Endpoint) -> anyhow::Result<Self> {
        let rules = endpoint.clone();
        let policy = redirect::Policy::custom(move |attempt| {
            // `previous` holds the first URL too, so the 11th redirect sees 11.
            if attempt.previous().len() > MAX_REDIRECTS {
                attempt.error("more than 10 redirects")
            } else if rules.allows(attempt.url()) {
                attempt.follow()
            } else {
                attempt.error("a redirect left GitHub")
            }
        });
        let mut builder = Client::builder()
            .redirect(policy)
            .user_agent(format!("mailtriage/{}", version::RUNNING));
        if endpoint.loopback.is_some() {
            builder = builder.no_proxy();
        }
        Ok(Self {
            endpoint,
            client: builder.build()?,
        })
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Every page of the release list, following `Link: rel="next"`. Any
    /// failing page, a page URL outside the rules, or more than 20 pages
    /// fails the whole list.
    pub fn releases(&self) -> Result<Vec<ReleaseJson>, CheckError> {
        let mut url = self.endpoint.releases_url();
        let mut all = Vec::new();
        for _ in 0..MAX_PAGES {
            let (page, next) = self.page(&url)?;
            all.extend(page);
            match next {
                Some(next) => url = next,
                None => return Ok(all),
            }
        }
        Err(CheckError::plain(format!(
            "the release list has more than {MAX_PAGES} pages"
        )))
    }

    fn page(&self, url: &Url) -> Result<(Vec<ReleaseJson>, Option<Url>), CheckError> {
        if !self.endpoint.allows(url) {
            return Err(CheckError::plain("a release list page is outside GitHub"));
        }
        let response = self
            .client
            .get(url.clone())
            .timeout(API_TIMEOUT)
            .header(ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .map_err(|e| {
                CheckError::plain(format!("cannot read the release list: {}", describe(&e)))
            })?;
        let status = response.status();
        if !status.is_success() {
            let headers = response.headers();
            let rate_reset = rate_reset(headers);
            let what = if rate_reset.is_some() || status.as_u16() == 429 {
                "GitHub's rate limit was reached".to_owned()
            } else {
                format!("GitHub answered {}", status.as_u16())
            };
            return Err(CheckError {
                message: format!("cannot read the release list: {what}"),
                retry_after: retry_after(headers, Utc::now()),
                rate_reset,
            });
        }
        let next = response
            .headers()
            .get("link")
            .and_then(|v| v.to_str().ok())
            .and_then(next_link)
            .map(|link| Url::parse(&link))
            .transpose()
            .map_err(|_| CheckError::plain("GitHub sent an invalid next-page link"))?;
        let body = read_capped(response, MAX_PAGE_BYTES)
            .map_err(|e| CheckError::plain(format!("cannot read the release list: {e}")))?;
        let releases = serde_json::from_slice(&body)
            .map_err(|_| CheckError::plain("GitHub sent a release list mailtriage cannot read"))?;
        Ok((releases, next))
    }

    /// Downloads `url` into memory, refusing a listed or actual size over
    /// `max` bytes. 300 s for the whole download.
    pub fn download(&self, url: &str, listed_size: u64, max: u64) -> anyhow::Result<Vec<u8>> {
        if listed_size > max {
            anyhow::bail!("it is larger than {}", size_text(max));
        }
        let url = Url::parse(url).map_err(|_| anyhow::anyhow!("its URL is invalid"))?;
        if !self.endpoint.allows(&url) {
            anyhow::bail!("its URL is outside GitHub");
        }
        let response = self
            .client
            .get(url)
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .map_err(|e| anyhow::anyhow!("{}", describe(&e)))?;
        if !response.status().is_success() {
            anyhow::bail!("GitHub answered {}", response.status().as_u16());
        }
        read_capped(response, max).map_err(anyhow::Error::msg)
    }
}

/// The body, refused once it exceeds `max` bytes.
fn read_capped(response: Response, max: u64) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    response
        .take(max + 1)
        .read_to_end(&mut body)
        .map_err(|e| format!("the download broke off ({e})"))?;
    if body.len() as u64 > max {
        return Err(format!("it is larger than {}", size_text(max)));
    }
    Ok(body)
}

/// `200 MB`, or bytes below one megabyte.
fn size_text(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    if bytes >= MB {
        format!("{} MB", bytes / MB)
    } else {
        format!("{bytes} bytes")
    }
}

/// A short reason for a failed request, without URLs or headers.
fn describe(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        return "timed out".into();
    }
    if error.is_redirect() {
        let mut source = std::error::Error::source(error);
        while let Some(inner) = source {
            if inner.source().is_none() {
                return inner.to_string();
            }
            source = inner.source();
        }
        return "a redirect was refused".into();
    }
    "cannot connect".into()
}

/// The target of the `rel="next"` entry of a `Link` header.
pub fn next_link(value: &str) -> Option<String> {
    value.split(',').find_map(|part| {
        let (target, params) = part.trim().split_once(';')?;
        let target = target.trim().strip_prefix('<')?.strip_suffix('>')?;
        let next = params.split(';').any(|param| {
            param.split_once('=').is_some_and(|(key, value)| {
                key.trim().eq_ignore_ascii_case("rel")
                    && value
                        .trim()
                        .trim_matches('"')
                        .split_ascii_whitespace()
                        .any(|rel| rel.eq_ignore_ascii_case("next"))
            })
        });
        next.then(|| target.to_owned())
    })
}

/// `Retry-After` as seconds or an HTTP date.
pub fn retry_after(headers: &HeaderMap, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let value = headers.get("retry-after")?.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<i64>() {
        return Some(now + chrono::Duration::seconds(seconds.clamp(0, 365 * 86_400)));
    }
    DateTime::parse_from_rfc2822(value)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// `X-RateLimit-Reset` (epoch seconds) when `X-RateLimit-Remaining` is 0.
pub fn rate_reset(headers: &HeaderMap) -> Option<DateTime<Utc>> {
    let header = |name: &str| headers.get(name)?.to_str().ok()?.trim().parse::<i64>().ok();
    if header("x-ratelimit-remaining")? != 0 {
        return None;
    }
    Utc.timestamp_opt(header("x-ratelimit-reset")?, 0).single()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    #[test]
    fn github_hosts_over_https_are_allowed() {
        let github = Endpoint::github();
        for ok in [
            "https://api.github.com/repos/x/y/releases?per_page=30",
            "https://github.com/wir-drei-digital/mailtriage/releases/download/v1.0.0/a.tar.gz",
            "https://objects.githubusercontent.com/x",
            "https://release-assets.githubusercontent.com/x",
            "https://github.com:443/x",
        ] {
            assert!(github.allows(&url(ok)), "{ok}");
        }
        for refused in [
            "https://github.com.evil.example/x",
            "https://evilgithubusercontent.com/x",
            "https://evil.example/github.com",
            "http://github.com/x",
            "http://objects.githubusercontent.com/x",
            "https://github.com:8443/x",
            "https://user@github.com/x",
            "https://user:pw@api.github.com/x",
            "ftp://github.com/x",
            "https://140.82.112.3/x",
        ] {
            assert!(!github.allows(&url(refused)), "{refused}");
        }
    }

    #[test]
    fn the_override_is_honoured_only_for_loopback_hosts() {
        for base in [
            "http://127.0.0.1:8080",
            "http://[::1]:9",
            "http://localhost:1",
        ] {
            let endpoint = Endpoint::with_override(Some(base));
            assert!(
                endpoint.allows(&url(&format!("{base}/download/a"))),
                "{base}"
            );
            assert!(endpoint.releases_url().as_str().starts_with(base), "{base}");
            // GitHub stays allowed; another loopback port does not.
            assert!(endpoint.allows(&url("https://objects.githubusercontent.com/x")));
            assert!(!endpoint.allows(&url("http://127.0.0.1:2/x")), "{base}");
        }
        for refused in [
            "http://example.com:8080",
            "http://10.0.0.1",
            "file:///tmp/x",
            "nonsense",
        ] {
            let endpoint = Endpoint::with_override(Some(refused));
            assert_eq!(
                endpoint.releases_url().as_str(),
                "https://api.github.com/repos/wir-drei-digital/mailtriage/releases?per_page=30"
            );
            assert!(!endpoint.allows(&url("http://127.0.0.1:8080/x")));
        }
        assert_eq!(
            Endpoint::github().releases_url().as_str(),
            "https://api.github.com/repos/wir-drei-digital/mailtriage/releases?per_page=30"
        );
    }

    #[test]
    fn the_next_link_is_found_among_others() {
        let header = r#"<https://api.github.com/r?per_page=30&page=1>; rel="prev", <https://api.github.com/r?per_page=30&page=3>; rel="next", <https://api.github.com/r?per_page=30&page=9>; rel="last""#;
        assert_eq!(
            next_link(header).as_deref(),
            Some("https://api.github.com/r?per_page=30&page=3")
        );
        assert_eq!(next_link(r#"<https://x/a>; rel="last""#), None);
        assert_eq!(
            next_link("<https://x/a>; rel=next").as_deref(),
            Some("https://x/a")
        );
        assert_eq!(next_link(""), None);
    }

    #[test]
    fn retry_hints_come_from_their_headers() {
        let now = Utc.with_ymd_and_hms(2026, 11, 3, 8, 0, 0).unwrap();
        let mut headers = HeaderMap::new();
        assert_eq!(retry_after(&headers, now), None);
        headers.insert("retry-after", HeaderValue::from_static("120"));
        assert_eq!(
            retry_after(&headers, now),
            Some(now + chrono::Duration::seconds(120))
        );
        headers.insert(
            "retry-after",
            HeaderValue::from_static("Tue, 03 Nov 2026 10:00:00 GMT"),
        );
        assert_eq!(
            retry_after(&headers, now),
            Some(Utc.with_ymd_and_hms(2026, 11, 3, 10, 0, 0).unwrap())
        );
        headers.insert("x-ratelimit-reset", HeaderValue::from_static("1793700000"));
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("3"));
        assert_eq!(rate_reset(&headers), None);
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("0"));
        assert_eq!(
            rate_reset(&headers),
            Utc.timestamp_opt(1_793_700_000, 0).single()
        );
    }
}
```

- [ ] **Step 6: Choosing the candidate and reading `SHA256SUMS`**

Create `src/update/release.rs`:

```rust
//! Choosing the candidate release, the platform's archive name and the
//! `SHA256SUMS` line for it.
use super::{
    github::{AssetJson, ReleaseJson},
    version, Component,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The release information the cache keeps: the candidate and, per
/// component, its archive for this platform (`None` when the release has
/// none, exactly one being required).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedRelease {
    pub version: String,
    pub release_url: String,
    pub published_at: Option<String>,
    pub archives: BTreeMap<String, Option<Asset>>,
    pub sums: Option<Asset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// The platform part of archive names for this build, or `None` for a
/// target without release archives.
pub fn platform() -> Option<&'static str> {
    platform_for(
        std::env::consts::ARCH,
        std::env::consts::OS,
        cfg!(target_env = "gnu"),
    )
}

/// `linux-amd64` (x86_64 Linux GNU), `linux-arm64` (aarch64 Linux GNU),
/// `macos-arm64` (aarch64 macOS); nothing else.
pub fn platform_for(arch: &str, os: &str, gnu: bool) -> Option<&'static str> {
    match (arch, os, gnu) {
        ("x86_64", "linux", true) => Some("linux-amd64"),
        ("aarch64", "linux", true) => Some("linux-arm64"),
        ("aarch64", "macos", _) => Some("macos-arm64"),
        _ => None,
    }
}

/// The highest stable release: drafts, prereleases and tags that are not
/// exactly `vX.Y.Z` are ignored. `None` when no release remains.
pub fn select(
    releases: &[ReleaseJson],
    platform: Option<&str>,
    components: &[Component],
) -> Option<CachedRelease> {
    let (version, release) = releases
        .iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter_map(|r| Some((version::parse_tag(&r.tag_name)?, r)))
        .max_by(|(a, _), (b, _)| a.cmp_precedence(b))?;
    let unique = |name: &str| -> Option<Asset> {
        let mut found = release.assets.iter().filter(|a| a.name == name);
        match (found.next(), found.next()) {
            (Some(asset), None) => Some(asset_of(asset)),
            _ => None,
        }
    };
    let archives = components
        .iter()
        .map(|c| {
            let asset = platform.and_then(|p| unique(&c.archive_name(&version, p)));
            (c.name.to_owned(), asset)
        })
        .collect();
    Some(CachedRelease {
        version: version.to_string(),
        release_url: release.html_url.clone(),
        published_at: release.published_at.clone(),
        archives,
        sums: unique("SHA256SUMS"),
    })
}

fn asset_of(asset: &AssetJson) -> Asset {
    Asset {
        name: asset.name.clone(),
        url: asset.browser_download_url.clone(),
        size: asset.size,
    }
}

/// The SHA-256 `SHA256SUMS` lists for `name`. Every non-empty line must be
/// `<64 lowercase hex>  <name>`, and exactly one must name `name`.
pub fn expected_sha256(sums: &str, name: &str) -> Result<String, String> {
    let mut found = Vec::new();
    for line in sums.lines().filter(|l| !l.is_empty()) {
        let (hash, file) = line
            .split_once("  ")
            .filter(|(hash, file)| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    && !file.is_empty()
            })
            .ok_or_else(|| "SHA256SUMS has a malformed line".to_owned())?;
        if file == name {
            found.push(hash.to_owned());
        }
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(format!("SHA256SUMS has no line for {name}")),
        _ => Err(format!("SHA256SUMS has several lines for {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::CLI;

    fn release(tag: &str, draft: bool, prerelease: bool, assets: &[&str]) -> ReleaseJson {
        ReleaseJson {
            tag_name: tag.into(),
            draft,
            prerelease,
            html_url: format!("https://github.com/r/releases/tag/{tag}"),
            published_at: Some("2026-11-02T09:00:00Z".into()),
            assets: assets
                .iter()
                .map(|name| AssetJson {
                    name: (*name).into(),
                    browser_download_url: format!(
                        "https://github.com/r/releases/download/{tag}/{name}"
                    ),
                    size: 10,
                })
                .collect(),
        }
    }

    #[test]
    fn each_supported_target_has_an_archive_name() {
        assert_eq!(platform_for("x86_64", "linux", true), Some("linux-amd64"));
        assert_eq!(platform_for("aarch64", "linux", true), Some("linux-arm64"));
        assert_eq!(platform_for("aarch64", "macos", false), Some("macos-arm64"));
        for (arch, os, gnu) in [
            ("x86_64", "macos", false),
            ("x86_64", "linux", false),
            ("x86_64", "windows", false),
            ("arm", "linux", true),
            ("aarch64", "freebsd", false),
        ] {
            assert_eq!(platform_for(arch, os, gnu), None, "{arch} {os}");
        }
        let v = semver::Version::new(0, 3, 0);
        assert_eq!(
            CLI.archive_name(&v, "macos-arm64"),
            "mailtriage-v0.3.0-macos-arm64.tar.gz"
        );
    }

    #[test]
    fn the_highest_stable_release_wins_wherever_it_is_listed() {
        let list = vec![
            release("v1.2.3-rc.1", false, true, &[]),
            release("v9.0.0", true, false, &[]),
            release("v8.0.0", false, true, &[]),
            release("1.2.3", false, false, &[]),
            release("v1.2", false, false, &[]),
            release("v01.2.3", false, false, &[]),
            release("v0.9.9", false, false, &[]),
            release(
                "v0.10.0",
                false,
                false,
                &["mailtriage-v0.10.0-linux-amd64.tar.gz", "SHA256SUMS"],
            ),
            release("v0.2.0", false, false, &[]),
        ];
        let chosen = select(&list, Some("linux-amd64"), &[CLI]).unwrap();
        assert_eq!(chosen.version, "0.10.0");
        assert_eq!(
            chosen.release_url,
            "https://github.com/r/releases/tag/v0.10.0"
        );
        assert_eq!(
            chosen.archives["mailtriage"].as_ref().unwrap().name,
            "mailtriage-v0.10.0-linux-amd64.tar.gz"
        );
        assert_eq!(chosen.sums.as_ref().unwrap().name, "SHA256SUMS");
        assert!(select(&list[..6], Some("linux-amd64"), &[CLI]).is_none());
    }

    #[test]
    fn a_missing_or_doubled_asset_is_recorded_as_none() {
        let doubled = release(
            "v1.0.0",
            false,
            false,
            &[
                "mailtriage-v1.0.0-macos-arm64.tar.gz",
                "mailtriage-v1.0.0-macos-arm64.tar.gz",
                "SHA256SUMS",
            ],
        );
        let chosen = select(&[doubled], Some("macos-arm64"), &[CLI]).unwrap();
        assert_eq!(chosen.archives["mailtriage"], None);
        assert!(chosen.sums.is_some());
        let bare = release("v1.0.0", false, false, &[]);
        let chosen = select(std::slice::from_ref(&bare), Some("macos-arm64"), &[CLI]).unwrap();
        assert_eq!(
            (chosen.archives["mailtriage"].clone(), chosen.sums),
            (None, None)
        );
        let unsupported = select(&[bare], None, &[CLI]).unwrap();
        assert_eq!(unsupported.archives["mailtriage"], None);
    }

    #[test]
    fn sha256sums_needs_exactly_one_well_formed_line() {
        let hash = "a".repeat(64);
        let name = "mailtriage-v1.0.0-linux-amd64.tar.gz";
        let other = format!("{}  mailtriage-v1.0.0-macos-arm64.tar.gz\n", "b".repeat(64));
        let line = format!("{hash}  {name}\n");
        assert_eq!(
            expected_sha256(&format!("{other}{line}"), name),
            Ok(hash.clone())
        );
        assert_eq!(
            expected_sha256(&other, name),
            Err(format!("SHA256SUMS has no line for {name}"))
        );
        assert_eq!(
            expected_sha256(&format!("{line}{line}"), name),
            Err(format!("SHA256SUMS has several lines for {name}"))
        );
        for bad in [
            format!("{}  {name}\n", "A".repeat(64)),
            format!("{}  {name}\n", "a".repeat(63)),
            format!("{hash} {name}\n"),
            format!("{hash}  \n"),
            format!("{line}garbage\n"),
        ] {
            assert_eq!(
                expected_sha256(&bad, name),
                Err("SHA256SUMS has a malformed line".to_owned()),
                "{bad:?}"
            );
        }
    }
}
```

- [ ] **Step 7: The cache file and the schedule**

`Cache::read` creates nothing (it is also used by `service status` and `doctor`, which must not write into the real cache directory). `Cache::update` is the only writer: exclusive lock, read, change, atomic replace.

Create `src/update/cache.rs`:

```rust
//! The per-user update cache: `update.json`, read and written under
//! `update.lock`, replaced atomically.
use super::release::CachedRelease;
use crate::domain::UpdateMode;
use anyhow::{anyhow, Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

pub const FILE: &str = "update.json";
pub const LOCK: &str = "update.lock";
/// How long a writer waits for the cache lock, which is held only for a
/// read or a write.
const LOCK_WAIT: Duration = Duration::from_secs(10);

/// `~/Library/Caches/mailtriage` on macOS; elsewhere
/// `$XDG_CACHE_HOME/mailtriage` when that is absolute, else
/// `~/.cache/mailtriage`. `None` without the directory it needs.
pub fn cache_dir(
    macos: bool,
    home: Option<&Path>,
    xdg_cache_home: Option<&OsStr>,
) -> Option<PathBuf> {
    let home = home.filter(|h| !h.as_os_str().is_empty());
    if macos {
        return home.map(|h| h.join("Library/Caches/mailtriage"));
    }
    match xdg_cache_home.map(Path::new).filter(|p| p.is_absolute()) {
        Some(xdg) => Some(xdg.join("mailtriage")),
        None => home.map(|h| h.join(".cache/mailtriage")),
    }
}

/// This user's cache directory, from `HOME` and `XDG_CACHE_HOME`.
pub fn default_dir() -> Option<PathBuf> {
    cache_dir(
        cfg!(target_os = "macos"),
        std::env::var_os("HOME").map(PathBuf::from).as_deref(),
        std::env::var_os("XDG_CACHE_HOME").as_deref(),
    )
}

/// `update.json`. Unknown fields are dropped on the next write.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CacheFile {
    #[serde(default)]
    pub schema_version: u32,
    /// The candidate of the last successful check; `None` when it found no
    /// stable release (or before the first check: then `checked_at` is
    /// `None` too).
    #[serde(default)]
    pub release: Option<CachedRelease>,
    #[serde(default)]
    pub checked_at: Option<String>,
    #[serde(default)]
    pub next_check_at: Option<String>,
    #[serde(default)]
    pub check_failures: u32,
    #[serde(default)]
    pub last_check_error: Option<ErrorRecord>,
    /// By canonical config path.
    #[serde(default)]
    pub configs: BTreeMap<String, ConfigEntry>,
    /// By canonical installation path.
    #[serde(default)]
    pub installs: BTreeMap<String, InstallEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorRecord {
    pub at: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigEntry {
    pub mode: UpdateMode,
    #[serde(default)]
    pub notified_version: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallEntry {
    /// The installed version: recorded by an install, or by the last
    /// reading of `--version`.
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub at: Option<String>,
    #[serde(default)]
    pub last_error: Option<ErrorRecord>,
    #[serde(default)]
    pub failures: u32,
    #[serde(default)]
    pub next_attempt_at: Option<String>,
}

/// The cache in one directory.
#[derive(Debug, Clone)]
pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// This user's cache; `None` without `HOME` (see `default_dir`).
    pub fn for_user() -> Option<Self> {
        default_dir().map(Self::new)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `update.json`; a missing or unreadable file counts as empty. Takes
    /// the cache lock shared when its file exists, and creates nothing.
    pub fn read(&self) -> CacheFile {
        let _lock = OpenOptions::new()
            .read(true)
            .open(self.dir.join(LOCK))
            .ok()
            .filter(|lock| wait_for(|| lock.try_lock_shared().is_ok()));
        self.read_unlocked()
    }

    fn read_unlocked(&self) -> CacheFile {
        fs::read(self.dir.join(FILE))
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default()
    }

    /// Applies `change` to the current contents under the exclusive cache
    /// lock and replaces the file atomically (an exclusively created
    /// temporary file, fsync, rename). Creates the directory (mode 0700).
    pub fn update<T>(&self, change: impl FnOnce(&mut CacheFile) -> T) -> Result<T> {
        create_private_dir(&self.dir)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.dir.join(LOCK))
            .with_context(|| format!("open {}", self.dir.join(LOCK).display()))?;
        if !wait_for(|| lock.try_lock_exclusive().is_ok()) {
            return Err(anyhow!("the update cache is locked by another process"));
        }
        let mut file = self.read_unlocked();
        let out = change(&mut file);
        file.schema_version = 1;
        self.write(&file)?;
        drop(lock);
        Ok(out)
    }

    fn write(&self, file: &CacheFile) -> Result<()> {
        let tmp = self
            .dir
            .join(format!(".{FILE}.{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut out = options.open(&tmp)?;
            serde_json::to_writer_pretty(&mut out, file)?;
            out.write_all(b"\n")?;
            out.sync_all()?;
            fs::rename(&tmp, self.dir.join(FILE))?;
            if let Ok(dir) = File::open(&self.dir) {
                let _ = dir.sync_all();
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result.with_context(|| format!("write {}", self.dir.join(FILE).display()))
    }
}

fn create_private_dir(dir: &Path) -> Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(dir)
        .with_context(|| format!("create {}", dir.display()))
}

/// Polls `try_lock` until it succeeds or `LOCK_WAIT` passes.
fn wait_for(mut try_lock: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        if try_lock() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cache_directory_follows_the_platform_rules() {
        let home = Some(Path::new("/h"));
        assert_eq!(
            cache_dir(true, home, Some(OsStr::new("/x"))),
            Some(PathBuf::from("/h/Library/Caches/mailtriage"))
        );
        assert_eq!(
            cache_dir(false, home, Some(OsStr::new("/x"))),
            Some(PathBuf::from("/x/mailtriage"))
        );
        assert_eq!(
            cache_dir(false, home, Some(OsStr::new("relative"))),
            Some(PathBuf::from("/h/.cache/mailtriage"))
        );
        assert_eq!(
            cache_dir(false, home, None),
            Some(PathBuf::from("/h/.cache/mailtriage"))
        );
        assert_eq!(cache_dir(true, None, None), None);
        assert_eq!(cache_dir(false, Some(Path::new("")), None), None);
        assert_eq!(
            cache_dir(false, None, Some(OsStr::new("/x"))),
            Some(PathBuf::from("/x/mailtriage"))
        );
    }

    #[test]
    fn a_missing_or_unreadable_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().join("c"));
        assert_eq!(cache.read(), CacheFile::default());
        assert!(!dir.path().join("c").exists(), "reading created files");
        fs::create_dir_all(dir.path().join("c")).unwrap();
        fs::write(dir.path().join("c").join(FILE), "{not json").unwrap();
        assert_eq!(cache.read(), CacheFile::default());
    }

    #[test]
    fn updates_are_written_atomically_and_privately() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().join("c"));
        cache
            .update(|c| c.next_check_at = Some("2026-11-03T09:00:00Z".into()))
            .unwrap();
        let file = cache.read();
        assert_eq!(file.schema_version, 1);
        assert_eq!(file.next_check_at.as_deref(), Some("2026-11-03T09:00:00Z"));
        let text = fs::read_to_string(dir.path().join("c").join(FILE)).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        for key in ["release", "checked_at", "last_check_error"] {
            assert!(value[key].is_null(), "{key}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&dir.path().join("c")), 0o700);
            assert_eq!(mode(&dir.path().join("c").join(FILE)), 0o600);
        }
        let leftovers: Vec<_> = fs::read_dir(dir.path().join("c"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn an_unwritable_cache_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("file"), "").unwrap();
        let cache = Cache::new(dir.path().join("file").join("c"));
        assert!(cache.update(|c| c.check_failures = 1).is_err());
    }
}
```

Create `src/update/schedule.rs`:

```rust
//! When to check and when to retry an install: reservations, the daily
//! check with jitter, and capped exponential backoff. Pure functions; the
//! callers pass the time and the random draws.
use chrono::{DateTime, Duration, SecondsFormat, Utc};

/// A check or install reserves this long before its request, so a process
/// that dies during the request does not retry sooner after a restart.
pub fn reservation(now: DateTime<Utc>) -> DateTime<Utc> {
    now + Duration::hours(1)
}

/// After a successful check: 24 h plus `jitter_minutes` (0 to 60).
pub fn after_success(now: DateTime<Utc>, jitter_minutes: i64) -> DateTime<Utc> {
    now + Duration::hours(24) + Duration::minutes(jitter_minutes.clamp(0, 60))
}

/// After the `failures`-th failed check in a row: 1 h × 2^(failures − 1),
/// at most 24 h, scaled by `1 + jitter` (jitter −0.1 to 0.1); later when
/// `Retry-After` or the rate-limit reset says so (each capped at 24 h).
pub fn after_failure(
    now: DateTime<Utc>,
    failures: u32,
    jitter: f64,
    retry_after: Option<DateTime<Utc>>,
    rate_reset: Option<DateTime<Utc>>,
) -> DateTime<Utc> {
    let doubled = 60i64 << failures.clamp(1, 6).saturating_sub(1);
    let minutes = doubled.min(24 * 60) as f64 * (1.0 + jitter.clamp(-0.1, 0.1));
    let backoff = now + Duration::seconds((minutes * 60.0).round() as i64);
    let cap = now + Duration::hours(24);
    [retry_after, rate_reset]
        .into_iter()
        .flatten()
        .map(|hint| hint.min(cap))
        .fold(backoff, DateTime::max)
}

/// After the `failures`-th failed install in a row: 1 h, doubling, at most 24 h.
pub fn install_backoff(now: DateTime<Utc>, failures: u32) -> DateTime<Utc> {
    let doubled = 1i64 << failures.clamp(1, 6).saturating_sub(1);
    now + Duration::hours(doubled.min(24))
}

/// Whether the time `at` (RFC 3339; missing or unreadable means now) has come.
pub fn due(at: Option<&str>, now: DateTime<Utc>) -> bool {
    at.and_then(parse).is_none_or(|at| at <= now)
}

/// Timestamps in `update.json` and in output: RFC 3339, seconds, `Z`.
pub fn stamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub fn parse(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// A random number in 0..1 for jitter, from the UUID generator mailtriage
/// already uses.
pub fn random_unit() -> f64 {
    (uuid::Uuid::new_v4().as_u128() >> 75) as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 3, 8, 0, 0).unwrap()
    }

    #[test]
    fn success_waits_a_day_plus_up_to_an_hour() {
        assert_eq!(after_success(now(), 0), now() + Duration::hours(24));
        assert_eq!(after_success(now(), 60), now() + Duration::hours(25));
        assert_eq!(after_success(now(), 600), now() + Duration::hours(25));
        for _ in 0..100 {
            let at = after_success(now(), (random_unit() * 61.0) as i64);
            assert!(at >= now() + Duration::hours(24) && at <= now() + Duration::hours(25));
        }
    }

    #[test]
    fn failures_double_from_an_hour_up_to_a_day() {
        let hours =
            |failures| (after_failure(now(), failures, 0.0, None, None) - now()).num_minutes() / 60;
        assert_eq!(
            [1, 2, 3, 4, 5, 6, 7, 50].map(hours),
            [1, 2, 4, 8, 16, 24, 24, 24]
        );
        let low = after_failure(now(), 1, -0.1, None, None) - now();
        let high = after_failure(now(), 1, 0.1, None, None) - now();
        assert_eq!((low.num_minutes(), high.num_minutes()), (54, 66));
        let capped = after_failure(now(), 9, 0.1, None, None) - now();
        assert_eq!(capped.num_minutes(), 24 * 66);
    }

    #[test]
    fn server_hints_win_when_later() {
        let later = now() + Duration::hours(5);
        let sooner = now() + Duration::minutes(5);
        assert_eq!(after_failure(now(), 1, 0.0, Some(later), None), later);
        assert_eq!(after_failure(now(), 1, 0.0, None, Some(later)), later);
        assert_eq!(
            after_failure(now(), 1, 0.0, Some(sooner), Some(sooner)),
            now() + Duration::hours(1)
        );
        let far = now() + Duration::days(30);
        assert_eq!(
            after_failure(now(), 1, 0.0, Some(far), None),
            now() + Duration::hours(24)
        );
    }

    #[test]
    fn install_backoff_doubles_and_is_capped() {
        let hours = |failures| (install_backoff(now(), failures) - now()).num_hours();
        assert_eq!([1, 2, 3, 4, 5, 6, 30].map(hours), [1, 2, 4, 8, 16, 24, 24]);
    }

    #[test]
    fn reservations_and_due_times() {
        assert_eq!(reservation(now()), now() + Duration::hours(1));
        assert!(due(None, now()));
        assert!(due(Some("garbage"), now()));
        assert!(due(Some(&stamp(now())), now()));
        assert!(!due(Some(&stamp(now() + Duration::seconds(1))), now()));
        assert_eq!(stamp(now()), "2026-11-03T08:00:00Z");
        assert_eq!(parse("2026-11-03T08:00:00Z"), Some(now()));
    }
}
```

- [ ] **Step 8: One refresh**

Create `src/update/check.rs`:

```rust
//! Refreshing the release information: a reservation, the release list,
//! then the result or the failure recorded in the cache.
use super::{
    cache::{Cache, ErrorRecord},
    github::Net,
    release::{self, CachedRelease},
    schedule, COMPONENTS,
};
use anyhow::{anyhow, Result};
use chrono::Utc;

/// Whether the reservation must be written before the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reservation {
    /// `watch`: without a written reservation there is no request.
    Required,
    /// `update`: a cache that cannot be written is a warning.
    BestEffort,
}

/// A successful check.
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    /// The highest stable release, `None` when there is none.
    pub release: Option<CachedRelease>,
    pub checked_at: String,
    /// Cache writes that failed (`BestEffort` only).
    pub warnings: Vec<String>,
}

/// One refresh: `next_check_at = now + 1 h` first, then the release list.
/// Success stores `release`, schedules the next check in 24 h plus up to an
/// hour and clears the failures; failure counts it and backs off. Errors
/// are the check's message.
pub fn refresh(net: &Net, cache: &Cache, reservation: Reservation) -> Result<Checked> {
    let mut warnings = Vec::new();
    let reserved = cache.update(|c| {
        c.next_check_at = Some(schedule::stamp(schedule::reservation(Utc::now())));
    });
    if let Err(e) = reserved {
        let message = format!("cannot write the update cache: {e:#}");
        match reservation {
            Reservation::Required => return Err(anyhow!("{message}; no check was made")),
            Reservation::BestEffort => warnings.push(message),
        }
    }
    match net.releases() {
        Ok(list) => {
            let release = release::select(&list, release::platform(), COMPONENTS);
            let now = Utc::now();
            let checked_at = schedule::stamp(now);
            let jitter = (schedule::random_unit() * 61.0) as i64;
            let recorded = cache.update(|c| {
                c.release = release.clone();
                c.checked_at = Some(checked_at.clone());
                c.next_check_at = Some(schedule::stamp(schedule::after_success(now, jitter)));
                c.check_failures = 0;
                c.last_check_error = None;
            });
            if let Err(e) = recorded {
                warnings.push(format!("recording the check failed: {e:#}"));
            }
            Ok(Checked {
                release,
                checked_at,
                warnings,
            })
        }
        Err(error) => {
            let now = Utc::now();
            let jitter = schedule::random_unit() * 0.2 - 0.1;
            let _ = cache.update(|c| {
                c.check_failures = c.check_failures.saturating_add(1);
                c.next_check_at = Some(schedule::stamp(schedule::after_failure(
                    now,
                    c.check_failures,
                    jitter,
                    error.retry_after,
                    error.rate_reset,
                )));
                c.last_check_error = Some(ErrorRecord {
                    at: schedule::stamp(now),
                    message: error.message.clone(),
                });
            });
            Err(anyhow!(error.message))
        }
    }
}
```

- [ ] **Step 9: Run the tests to verify they pass**

Run: `cargo test --locked --lib update:: && cargo test --locked --test update_check`
Expected: all PASS (the unit tests of `version`, `github`, `release`, `cache` and `schedule`, and the nine integration tests).

- [ ] **Step 10: Full check and commit**

Run: `cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`
Expected: all pass.

```bash
git add Cargo.toml Cargo.lock src/lib.rs src/update tests/update_support/mod.rs tests/update_check.rs
git commit -m "Check GitHub for the newest stable release

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Installing a release

**Files:**
- Create: `src/update/platform.rs`, `src/update/archive.rs`, `src/update/install.rs`, `tests/update_install.rs`
- Modify: `src/process.rs` (`run_bounded`, new `Captured` and `run_captured`, tests), `src/system_service.rs` (`current_uid` crate-visible), `src/update/mod.rs`, `src/update/cache.rs` (`InstallEntry.identity`), `tests/update_support/mod.rs` (append)

**Interfaces:**
- Consumes: Task 2's `Net`, `CachedRelease`, `Cache`, `schedule`, `version`, `Component`, `CLI`, `release::{platform, expected_sha256}`, `github::{MAX_ASSET_BYTES, MAX_SUMS_BYTES}`.
- Produces:
  - `process::run_captured<S: AsRef<OsStr>>(program: &Path, args: &[S], timeout: Duration, max_stdout: usize, max_stderr: usize) -> io::Result<Captured>`; `process::Captured { stdout, stderr, ending }` (no `Debug`).
  - `system_service::current_uid() -> u32` is `pub(crate)`.
  - `update::platform::{FileIdentity { dev, ino, size, mtime, mtime_nsec, ctime, ctime_nsec, mode }` (`of(&Metadata)`, `read(&Path) -> io::Result<Self>`; serde), `Blocker::{UnsupportedPlatform, ManagedByHomebrew, ManagedByNix, UnsafePermissions, NotWritable}` (`reason(self) -> &'static str`, `fix(self, &Path) -> String`), `blocker(path: &Path, platform: Option<&str>) -> Option<Blocker>`, `package_manager(&Path) -> Option<Blocker>`, `installation_path() -> Result<PathBuf, String>`, `PROBE_TIMEOUT`, `probe(program: &Path, Component) -> Result<semver::Version, String>}`.
  - `update::archive::{Limits { max_entries, max_total, max_file, max_path }, RELEASE, extract(data: &[u8], name: &str, &Limits) -> Result<Vec<u8>, String>}`.
  - `update::install::{LOCK_FILE, TEMP_PREFIX, TEST_HOOK, TEST_LOCK_WAIT, Hooks` (trait: `fn at(&self, point: &str) -> anyhow::Result<()>`), `NoHooks`, `EnvHooks`, `update_lock_wait() -> Duration`, `InstallLock`, `lock(dir: &Path, wait: Duration) -> anyhow::Result<Option<InstallLock>>`, `remove_leftovers(&Path)`, `Job<'a> { net, component, path, release, fallback, cache, hooks }`, `Outcome::{Current { installed }, Installed(Installed)}`, `Installed { from, to, previous_path, warnings }`, `install(&Job, &InstallLock, before_download: &mut dyn FnMut() -> anyhow::Result<()>) -> anyhow::Result<Outcome>`, `previous_path(&Path) -> PathBuf}`.
  - `cache::InstallEntry.identity: Option<platform::FileIdentity>` (omitted from JSON when `None`).
  - Hook points: `revalidate`, `backup`, `commit`, `publish`, `sync_dir`, `record`.
  - Tests: `update_support::{archive, release_archive, fake_binary, replace_file}`.

The tray spec's second component reuses `install` unchanged: a `Job` with `component: Component { name: "mailtriage-tray" }`, the tray's path, and the same lock.

- [ ] **Step 1: Write the failing tests**

Append the release fixtures to `tests/update_support/mod.rs` (a `use` may follow other items; nothing above imports `Path`):

```rust
use std::path::Path;

/// A gzip tar archive of regular files with mode 0755.
pub fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    for (name, data) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_entry_type(tar::EntryType::Regular);
        builder.append_data(&mut header, name, *data).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

/// A release archive as the release workflow builds it.
pub fn release_archive(binary: &[u8]) -> Vec<u8> {
    archive(&[
        ("mailtriage", binary),
        ("LICENSE", b"MIT License\n"),
        ("README.md", b"# mailtriage\n"),
    ])
}

/// The fake release binary: `--version` prints `mailtriage VERSION`; any
/// other call appends its arguments to `marker`.
pub fn fake_binary(version: &str, marker: &Path) -> Vec<u8> {
    format!(
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'mailtriage {version}'; exit 0; fi\necho \"$@\" >> '{}'\n",
        marker.display()
    )
    .into_bytes()
}

/// Writes `data` to `path` with `mode` through a new file renamed over
/// it, so `path` gets a new inode.
pub fn replace_file(path: &Path, data: &[u8], mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let tmp = path.with_extension("replacing");
    std::fs::write(&tmp, data).unwrap();
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode)).unwrap();
    std::fs::rename(&tmp, path).unwrap();
}
```

Create `tests/update_install.rs`:

```rust
#![cfg(unix)]
//! The install transaction against a loopback server, with an installed
//! script standing in for the binary: success, refusals, revalidation and
//! a fault injected before each step of the commit.
mod update_support;
use anyhow::{bail, Result};
use mailtriage::update::{
    cache::Cache,
    github::{Endpoint, Net},
    install::{self, Hooks, Job, NoHooks, Outcome},
    release::{self, CachedRelease},
    CLI, COMPONENTS,
};
use semver::Version;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    time::{Duration, Instant},
};
use update_support::{fake_binary, release_archive, replace_file, sha256_hex, Server};

const OLD: &str = "#!/bin/sh\necho 'mailtriage 0.0.1'\n";

struct Setup {
    _dir: tempfile::TempDir,
    server: Server,
    net: Net,
    cache: Cache,
    path: PathBuf,
    marker: PathBuf,
}

impl Setup {
    /// An installed `mailtriage 0.0.1` script and a server publishing 9.9.9.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let bin = root.join("bin");
        fs::create_dir(&bin).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        let path = bin.join("mailtriage");
        fs::write(&path, OLD).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        let server = Server::start();
        let marker = root.join("marker");
        server.publish("9.9.9", &release_archive(&fake_binary("9.9.9", &marker)));
        let net = Net::new(Endpoint::with_override(Some(&server.base))).unwrap();
        let cache = Cache::new(root.join("cache"));
        Self {
            _dir: dir,
            server,
            net,
            cache,
            path,
            marker,
        }
    }

    fn release(&self) -> CachedRelease {
        let list = self.net.releases().unwrap();
        release::select(&list, release::platform(), COMPONENTS).unwrap()
    }

    fn run(&self, hooks: &dyn Hooks, fallback: &str) -> Result<Outcome> {
        let release = self.release();
        let fallback = Version::parse(fallback).unwrap();
        let job = Job {
            net: &self.net,
            component: CLI,
            path: &self.path,
            release: &release,
            fallback: &fallback,
            cache: Some(&self.cache),
            hooks,
        };
        let lock = install::lock(self.path.parent().unwrap(), Duration::ZERO)
            .unwrap()
            .unwrap();
        install::install(&job, &lock, &mut || Ok(()))
    }

    fn installed(&self) -> String {
        fs::read_to_string(&self.path).unwrap()
    }

    fn previous(&self) -> PathBuf {
        install::previous_path(&self.path)
    }

    /// Temporary files of an attempt left in the directory.
    fn leftovers(&self) -> Vec<String> {
        fs::read_dir(self.path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.starts_with(install::TEMP_PREFIX))
            .collect()
    }
}

/// Fails the step named `.0`.
struct FailAt(&'static str);

impl Hooks for FailAt {
    fn at(&self, point: &str) -> Result<()> {
        if point == self.0 {
            bail!("injected at {point}");
        }
        Ok(())
    }
}

/// Runs `.1` at the step named `.0`.
struct RunAt<F: Fn()>(&'static str, F);

impl<F: Fn()> Hooks for RunAt<F> {
    fn at(&self, point: &str) -> Result<()> {
        if point == self.0 {
            (self.1)();
        }
        Ok(())
    }
}

fn installed(outcome: Outcome) -> install::Installed {
    match outcome {
        Outcome::Installed(done) => done,
        other => panic!("not installed: {other:?}"),
    }
}

#[test]
fn a_newer_release_replaces_the_binary_and_keeps_the_previous_one() {
    use std::os::unix::fs::MetadataExt;
    let s = Setup::new();
    let original = fs::metadata(&s.path).unwrap().ino();
    fs::write(
        s.path
            .parent()
            .unwrap()
            .join(".mailtriage-update-stale.tmp"),
        "old",
    )
    .unwrap();
    let done = installed(s.run(&NoHooks, "0.1.0").unwrap());
    assert_eq!(done.from, Some(Version::new(0, 0, 1)));
    assert_eq!(done.to, Version::new(9, 9, 9));
    assert_eq!(done.previous_path, s.previous());
    assert!(done.warnings.is_empty(), "{:?}", done.warnings);
    assert_eq!(fs::read(&s.path).unwrap(), fake_binary("9.9.9", &s.marker));
    assert_eq!(fs::read_to_string(s.previous()).unwrap(), OLD);
    // Swapped by rename: the old file is the backup, the path a new file.
    assert_eq!(fs::metadata(s.previous()).unwrap().ino(), original);
    assert_ne!(fs::metadata(&s.path).unwrap().ino(), original);
    assert_eq!(
        fs::metadata(&s.path).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert!(s.leftovers().is_empty(), "{:?}", s.leftovers());
    let entry = &s.cache.read().installs[s.path.to_str().unwrap()];
    assert_eq!(entry.version.as_deref(), Some("9.9.9"));
    assert_eq!((entry.failures, entry.next_attempt_at.clone()), (0, None));
    assert!(entry.identity.is_some());
    assert!(s.path.parent().unwrap().join(install::LOCK_FILE).exists());
}

#[test]
fn nothing_is_downloaded_when_the_installed_version_is_current() {
    let s = Setup::new();
    replace_file(&s.path, b"#!/bin/sh\necho 'mailtriage 9.9.9'\n", 0o755);
    assert_eq!(
        s.run(&NoHooks, "0.1.0").unwrap(),
        Outcome::Current {
            installed: Some(Version::new(9, 9, 9))
        }
    );
    // An unreadable installed version is compared with the fallback.
    replace_file(&s.path, b"#!/bin/sh\nexit 1\n", 0o755);
    assert_eq!(
        s.run(&NoHooks, "9.9.9").unwrap(),
        Outcome::Current { installed: None }
    );
    assert_eq!(s.server.count("/download"), 0);
    installed(s.run(&NoHooks, "0.1.0").unwrap());
}

/// Serves a release that must be refused.
type Prepare = fn(&Setup);

#[test]
fn refused_releases_leave_the_binary_and_no_files() {
    let cases: [(&str, Prepare); 4] = [
        ("checksum mismatch for mailtriage-v9.9.9-", |s| {
            let archive = release_archive(&fake_binary("9.9.9", &s.marker));
            let name = format!("mailtriage-v9.9.9-{}.tar.gz", update_support::platform());
            let wrong = format!("{}  {name}\n", sha256_hex(b"something else"));
            s.server.publish_with_sums("9.9.9", &archive, &wrong);
        }),
        (
            "the new binary does not run here: printed version 9.9.8, expected 9.9.9",
            |s| {
                let archive = release_archive(&fake_binary("9.9.8", &s.marker));
                s.server.publish("9.9.9", &archive);
            },
        ),
        (
            "the new binary does not run here: killed by signal 9",
            |s| {
                s.server
                    .publish("9.9.9", &release_archive(b"#!/bin/sh\nkill -9 $$\n"));
            },
        ),
        (
            "release v9.9.9: SHA256SUMS has no line for mailtriage-v9.9.9-",
            |s| {
                let archive = release_archive(&fake_binary("9.9.9", &s.marker));
                s.server.publish_with_sums("9.9.9", &archive, "");
            },
        ),
    ];
    for (expected, prepare) in cases {
        let s = Setup::new();
        prepare(&s);
        let e = s.run(&NoHooks, "0.1.0").unwrap_err().to_string();
        assert!(e.contains(expected), "{expected}: {e}");
        assert_eq!(s.installed(), OLD, "{expected}");
        assert!(!s.previous().exists(), "{expected}");
        assert!(s.leftovers().is_empty(), "{expected}: {:?}", s.leftovers());
        assert!(s.cache.read().installs.is_empty(), "{expected}");
    }
}

#[test]
fn a_release_without_this_platforms_archive_downloads_nothing() {
    let s = Setup::new();
    s.server
        .list(&[s.server.release_json("9.9.9", &[("SHA256SUMS", 10)])]);
    let e = s.run(&NoHooks, "0.1.0").unwrap_err().to_string();
    assert_eq!(
        e,
        format!(
            "release v9.9.9 has no {} archive",
            update_support::platform()
        )
    );
    assert_eq!(s.server.count("/download"), 0);
}

#[test]
fn a_binary_replaced_during_the_update_is_kept() {
    let s = Setup::new();
    let theirs = b"#!/bin/sh\necho 'mailtriage 5.0.0'\n";
    let path = s.path.clone();
    let hooks = RunAt("revalidate", move || replace_file(&path, theirs, 0o755));
    let e = s.run(&hooks, "0.1.0").unwrap_err().to_string();
    assert_eq!(
        e,
        "the installed binary changed during the update; try again"
    );
    assert_eq!(fs::read(&s.path).unwrap(), theirs);
    assert!(s.leftovers().is_empty(), "{:?}", s.leftovers());
}

#[test]
fn a_fault_before_the_commit_point_changes_nothing() {
    for point in ["backup", "commit"] {
        let s = Setup::new();
        fs::write(s.previous(), "older").unwrap();
        let e = s.run(&FailAt(point), "0.1.0").unwrap_err().to_string();
        assert!(e.contains(&format!("injected at {point}")), "{e}");
        assert_eq!(s.installed(), OLD, "{point}");
        assert_eq!(
            fs::read_to_string(s.previous()).unwrap(),
            "older",
            "{point}"
        );
        assert!(s.leftovers().is_empty(), "{point}: {:?}", s.leftovers());
    }
}

#[test]
fn a_fault_after_the_commit_point_is_a_warning_and_nothing_is_reversed() {
    let new = |s: &Setup| fake_binary("9.9.9", &s.marker);

    let s = Setup::new();
    fs::write(s.previous(), "older").unwrap();
    let done = installed(s.run(&FailAt("publish"), "0.1.0").unwrap());
    assert_eq!(fs::read(&s.path).unwrap(), new(&s));
    assert_eq!(fs::read_to_string(s.previous()).unwrap(), "older");
    assert_ne!(done.previous_path, s.previous());
    assert_eq!(fs::read_to_string(&done.previous_path).unwrap(), OLD);
    assert!(
        done.warnings[0].starts_with("installed; the previous binary stays at "),
        "{:?}",
        done.warnings
    );

    let s = Setup::new();
    let done = installed(s.run(&FailAt("sync_dir"), "0.1.0").unwrap());
    assert_eq!(fs::read(&s.path).unwrap(), new(&s));
    assert_eq!(fs::read_to_string(s.previous()).unwrap(), OLD);
    assert!(
        done.warnings[0].starts_with("installed; syncing "),
        "{:?}",
        done.warnings
    );

    let s = Setup::new();
    let done = installed(s.run(&FailAt("record"), "0.1.0").unwrap());
    assert_eq!(fs::read(&s.path).unwrap(), new(&s));
    assert_eq!(
        done.warnings,
        vec!["installed; recording the update failed: injected at record".to_owned()]
    );
    assert!(s.cache.read().installs.is_empty());
}

#[test]
fn a_refused_reservation_stops_before_any_download() {
    let s = Setup::new();
    let release = s.release();
    let fallback = Version::new(0, 1, 0);
    let job = Job {
        net: &s.net,
        component: CLI,
        path: &s.path,
        release: &release,
        fallback: &fallback,
        cache: None,
        hooks: &NoHooks,
    };
    let lock = install::lock(s.path.parent().unwrap(), Duration::ZERO)
        .unwrap()
        .unwrap();
    let e =
        install::install(&job, &lock, &mut || bail!("cannot write the update cache")).unwrap_err();
    assert_eq!(e.to_string(), "cannot write the update cache");
    assert_eq!(s.server.count("/download"), 0);
    assert_eq!(s.installed(), OLD);
}

#[test]
fn the_installation_lock_is_exclusive_and_waits() {
    let dir = tempfile::tempdir().unwrap();
    let held = install::lock(dir.path(), Duration::ZERO).unwrap().unwrap();
    assert!(install::lock(dir.path(), Duration::ZERO).unwrap().is_none());
    let start = Instant::now();
    assert!(install::lock(dir.path(), Duration::from_millis(300))
        .unwrap()
        .is_none());
    assert!(start.elapsed() >= Duration::from_millis(300));
    drop(held);
    assert!(install::lock(dir.path(), Duration::ZERO).unwrap().is_some());
    assert!(dir.path().join(".mailtriage-update.lock").exists());
}

#[test]
fn leftovers_are_removed_but_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        ".mailtriage-update-a.tmp",
        ".mailtriage-update-b.prev",
        ".mailtriage-update.lock",
        "mailtriage",
        "mailtriage.previous",
    ] {
        fs::write(dir.path().join(name), "x").unwrap();
    }
    fs::create_dir(dir.path().join(".mailtriage-update-dir")).unwrap();
    install::remove_leftovers(dir.path());
    let mut names: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            ".mailtriage-update-dir",
            ".mailtriage-update.lock",
            "mailtriage",
            "mailtriage.previous"
        ]
    );
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --locked --test update_install`
Expected: compile errors: unresolved import `mailtriage::update::install`.

- [ ] **Step 3: A runner that keeps stderr (`src/process.rs`)**

In `src/process.rs`, replace:

```rust
//! Child processes with limits: an argument array (no shell), stdin closed,
//! stderr discarded, a deadline and a cap on stdout.
```

with:

```rust
//! Child processes with limits: an argument array (no shell), stdin closed,
//! a deadline and a cap on stdout. `run_bounded` discards stderr;
//! `run_captured` keeps its start.
```

Replace the whole `run_bounded` function (from its doc comment `/// Runs \`program\` with \`args\`` to the closing brace before `find_on_path`) with the version below. Its body becomes the private `run`, which pipes stderr only when a cap is given; `run_bounded` keeps discarding stderr.

```rust
/// Runs `program` with `args`; `Err` only when it cannot be started.
pub fn run_bounded<S: AsRef<OsStr>>(
    program: &Path,
    args: &[S],
    timeout: Duration,
    max_stdout: usize,
) -> std::io::Result<Bounded> {
    let out = run(program, args, timeout, max_stdout, None)?;
    Ok(Bounded {
        stdout: out.stdout,
        ending: out.ending,
    })
}

/// What `run_captured` collected. No `Debug`, like `Bounded`.
pub struct Captured {
    pub stdout: Vec<u8>,
    /// The first bytes of stderr, up to the cap; the rest was read and dropped.
    pub stderr: Vec<u8>,
    pub ending: Ending,
}

/// `run_bounded` that also keeps the first `max_stderr` bytes of stderr.
/// More stderr does not end the child; it is read and dropped. For
/// `--version` probes, whose stderr explains why a binary does not run;
/// key commands keep using `run_bounded`, which discards stderr.
pub fn run_captured<S: AsRef<OsStr>>(
    program: &Path,
    args: &[S],
    timeout: Duration,
    max_stdout: usize,
    max_stderr: usize,
) -> std::io::Result<Captured> {
    run(program, args, timeout, max_stdout, Some(max_stderr))
}

fn run<S: AsRef<OsStr>>(
    program: &Path,
    args: &[S],
    timeout: Duration,
    max_stdout: usize,
    max_stderr: Option<usize>,
) -> std::io::Result<Captured> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(if max_stderr.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let overflow = Arc::new(AtomicBool::new(false));
    let collected = Arc::new(Mutex::new(Vec::new()));
    let reader = {
        let overflow = Arc::clone(&overflow);
        let collected = Arc::clone(&collected);
        thread::spawn(move || {
            let _ = read_bounded(stdout, max_stdout, &overflow, |chunk| {
                collected
                    .lock()
                    .map(|mut all| all.extend_from_slice(chunk))
                    .is_ok()
            });
        })
    };
    let errors = Arc::new(Mutex::new(Vec::new()));
    let error_reader = match (max_stderr, child.stderr.take()) {
        (Some(cap), Some(mut stderr)) => {
            let errors = Arc::clone(&errors);
            Some(thread::spawn(move || {
                let mut buffer = [0u8; 4096];
                while let Ok(size) = stderr.read(&mut buffer) {
                    if size == 0 {
                        break;
                    }
                    if let Ok(mut kept) = errors.lock() {
                        let room = cap.saturating_sub(kept.len());
                        kept.extend_from_slice(&buffer[..size.min(room)]);
                    }
                }
            }))
        }
        _ => None,
    };
    let readers_done =
        || reader.is_finished() && error_reader.as_ref().is_none_or(|r| r.is_finished());
    let deadline = Instant::now() + timeout;
    let mut status = None;
    let ending = loop {
        if overflow.load(Ordering::Relaxed) {
            terminate(&mut child);
            break Ending::Overflowed;
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(done) => status = done,
                Err(e) => {
                    terminate(&mut child);
                    return Err(e);
                }
            }
        }
        if let (Some(status), true) = (status, readers_done()) {
            break if overflow.load(Ordering::Relaxed) {
                Ending::Overflowed
            } else {
                Ending::Exited(status)
            };
        }
        if Instant::now() >= deadline {
            terminate(&mut child);
            break Ending::TimedOut;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let take = |buffer: &Mutex<Vec<u8>>| {
        std::mem::take(&mut *buffer.lock().unwrap_or_else(|e| e.into_inner()))
    };
    Ok(Captured {
        stdout: take(&collected),
        stderr: take(&errors),
        ending,
    })
}
```

In `src/process.rs`, replace:

```rust
        <Bounded as AmbiguousIfDebug<_>>::check();
    }
}
```

with:

```rust
        <Bounded as AmbiguousIfDebug<_>>::check();
        <Captured as AmbiguousIfDebug<_>>::check();
    }

    #[cfg(unix)]
    #[test]
    fn captured_runs_keep_the_start_of_stderr() {
        let sh = Path::new("/bin/sh");
        let script = "echo out; head -c 10000 /dev/zero | tr '\\0' e >&2; exit 3";
        let out = run_captured(sh, &["-c", script], Duration::from_secs(10), 4096, 4096).unwrap();
        assert_eq!(out.stdout, b"out\n");
        assert_eq!(out.stderr, vec![b'e'; 4096]);
        assert!(matches!(out.ending, Ending::Exited(s) if s.code() == Some(3)));
        let slow =
            run_captured(sh, &["-c", "sleep 30"], Duration::from_millis(200), 10, 10).unwrap();
        assert!(matches!(slow.ending, Ending::TimedOut));
        assert!(run_captured(
            Path::new("/nonexistent/x"),
            &["--version"],
            Duration::from_secs(1),
            10,
            10
        )
        .is_err());
    }
}
```

- [ ] **Step 4: Facts about an installed file (`src/update/platform.rs`)**

In `src/system_service.rs`, make both `current_uid` functions (the `#[cfg(unix)]` one and the `#[cfg(not(unix))]` one) `pub(crate) fn current_uid() -> u32`; nothing else changes there.

Create `src/update/platform.rs`:

```rust
//! Facts about an installed file: its identity, whether mailtriage may
//! replace it, and what its `--version` prints.
use super::{version, Component};
use crate::process::{self, Ending};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Component as PathPart, Path},
    time::Duration,
};

/// How long a `--version` run may take, and how much output it may print.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_MAX_STDOUT: usize = 4096;
const PROBE_MAX_STDERR: usize = 4096;

/// A file's identity: device, inode, size, modification time, change time
/// and mode. A rename over the path, a rewrite or a `chmod` changes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileIdentity {
    pub dev: u64,
    pub ino: u64,
    pub size: u64,
    pub mtime: i64,
    pub mtime_nsec: i64,
    pub ctime: i64,
    pub ctime_nsec: i64,
    pub mode: u32,
}

impl FileIdentity {
    pub fn of(meta: &fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            dev: meta.dev(),
            ino: meta.ino(),
            size: meta.size(),
            mtime: meta.mtime(),
            mtime_nsec: meta.mtime_nsec(),
            ctime: meta.ctime(),
            ctime_nsec: meta.ctime_nsec(),
            mode: meta.mode(),
        }
    }

    /// The identity of the file `path` names (symlinks followed).
    pub fn read(path: &Path) -> std::io::Result<Self> {
        fs::metadata(path).map(|meta| Self::of(&meta))
    }
}

/// The installation path: the running executable's canonical path. On
/// Linux, when that file was replaced since the start (its link then ends
/// in ` (deleted)`), `argv[0]` when it is absolute and exists.
pub fn installation_path() -> Result<std::path::PathBuf, String> {
    if let Ok(path) = std::env::current_exe().and_then(fs::canonicalize) {
        return Ok(path);
    }
    std::env::args_os()
        .next()
        .map(std::path::PathBuf::from)
        .filter(|argv0| argv0.is_absolute())
        .and_then(|argv0| fs::canonicalize(argv0).ok())
        .ok_or_else(|| "cannot find the file this mailtriage runs from".to_owned())
}

/// Why mailtriage will not replace a binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocker {
    UnsupportedPlatform,
    ManagedByHomebrew,
    ManagedByNix,
    UnsafePermissions,
    NotWritable,
}

impl Blocker {
    /// `install.reason` in output.
    pub fn reason(self) -> &'static str {
        match self {
            Blocker::UnsupportedPlatform => "unsupported_platform",
            Blocker::ManagedByHomebrew => "managed_by_homebrew",
            Blocker::ManagedByNix => "managed_by_nix",
            Blocker::UnsafePermissions => "unsafe_permissions",
            Blocker::NotWritable => "not_writable",
        }
    }

    /// `install.fix`: one sentence for the binary at `path`.
    pub fn fix(self, path: &Path) -> String {
        let dir = path.parent().unwrap_or(Path::new("/")).display();
        match self {
            Blocker::UnsupportedPlatform => {
                "releases have no archive for this platform; build mailtriage from source, or set \"updates\" to \"off\"".into()
            }
            Blocker::ManagedByHomebrew => "run `brew upgrade mailtriage`".into(),
            Blocker::ManagedByNix => {
                "update mailtriage through nix (for example `nix profile upgrade`)".into()
            }
            Blocker::UnsafePermissions => format!(
                "install mailtriage into a directory that only this user owns and can write, such as ~/.local/bin, or set \"updates\" to \"notify\" (now: {})",
                path.display()
            ),
            Blocker::NotWritable => {
                format!("make {dir} writable for this user, or set \"updates\" to \"notify\"")
            }
        }
    }
}

/// Why the binary at `path` (canonical) may not be replaced on `platform`
/// (`None`: no release archives for this build), checked in this order:
/// no archive, Homebrew, nix, ownership and permissions, writability.
pub fn blocker(path: &Path, platform: Option<&str>) -> Option<Blocker> {
    if platform.is_none() {
        return Some(Blocker::UnsupportedPlatform);
    }
    if let Some(manager) = package_manager(path) {
        return Some(manager);
    }
    let dir = path.parent()?;
    let uid = crate::system_service::current_uid();
    if !safe(path, uid, false) || !safe(dir, uid, true) {
        return Some(Blocker::UnsafePermissions);
    }
    (!writable(dir)).then_some(Blocker::NotWritable)
}

/// Homebrew (a path component `Cellar` followed by `mailtriage`) or nix
/// (under `/nix/store/`).
pub fn package_manager(path: &Path) -> Option<Blocker> {
    if path.starts_with("/nix/store") {
        return Some(Blocker::ManagedByNix);
    }
    let parts: Vec<_> = path.components().collect();
    parts
        .windows(2)
        .any(|pair| {
            pair[0] == PathPart::Normal("Cellar".as_ref())
                && pair[1] == PathPart::Normal("mailtriage".as_ref())
        })
        .then_some(Blocker::ManagedByHomebrew)
}

/// Owned by `uid`, the right kind, and not writable by group or others.
fn safe(path: &Path, uid: u32, directory: bool) -> bool {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path).is_ok_and(|meta| {
        let kind = if directory {
            meta.is_dir()
        } else {
            meta.is_file()
        };
        kind && meta.uid() == uid && meta.mode() & 0o022 == 0
    })
}

/// Whether this process may create files in `dir` (access(2), so nothing
/// is created).
fn writable(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    unsafe extern "C" {
        fn access(path: *const std::ffi::c_char, mode: i32) -> i32;
    }
    const W_OK: i32 = 2;
    const X_OK: i32 = 1;
    let Ok(path) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `path` is a valid NUL-terminated string for the call's duration.
    unsafe { access(path.as_ptr(), W_OK | X_OK) == 0 }
}

/// Runs `program --version` with a 10 s limit and returns the version it
/// printed as `<component> <semver>`. The error names the cause: `could
/// not start`, `killed by signal N`, `timed out`, `exited with N`, or
/// `printed "…"`, each with the first line of stderr when there is one.
pub fn probe(program: &Path, component: Component) -> Result<Version, String> {
    let out = process::run_captured(
        program,
        &["--version"],
        PROBE_TIMEOUT,
        PROBE_MAX_STDOUT,
        PROBE_MAX_STDERR,
    )
    .map_err(|_| "could not start".to_owned())?;
    let cause = match out.ending {
        Ending::TimedOut => "timed out".to_owned(),
        Ending::Overflowed => "printed too much".to_owned(),
        Ending::Exited(status) => {
            use std::os::unix::process::ExitStatusExt;
            if let Some(signal) = status.signal() {
                format!("killed by signal {signal}")
            } else if status.success() {
                match version::parse_version_output(component.name, &out.stdout) {
                    Some(found) => return Ok(found),
                    None => format!(
                        "printed {:?}",
                        shorten(&String::from_utf8_lossy(&out.stdout), 80)
                    ),
                }
            } else {
                format!("exited with {}", status.code().unwrap_or(-1))
            }
        }
    };
    let stderr = String::from_utf8_lossy(&out.stderr);
    match stderr.lines().map(str::trim).find(|l| !l.is_empty()) {
        Some(line) => Err(format!("{cause}; stderr: {}", shorten(line, 200))),
        None => Err(cause),
    }
}

fn shorten(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::CLI;
    use std::os::unix::fs::PermissionsExt;

    fn script(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn package_managers_are_recognised_by_path() {
        assert_eq!(
            package_manager(Path::new(
                "/opt/homebrew/Cellar/mailtriage/0.3.0/bin/mailtriage"
            )),
            Some(Blocker::ManagedByHomebrew)
        );
        assert_eq!(
            package_manager(Path::new(
                "/usr/local/Cellar/mailtriage/0.3.0/bin/mailtriage"
            )),
            Some(Blocker::ManagedByHomebrew)
        );
        assert_eq!(
            package_manager(Path::new("/nix/store/abc-mailtriage-0.3.0/bin/mailtriage")),
            Some(Blocker::ManagedByNix)
        );
        for plain in [
            "/Users/alice/.local/bin/mailtriage",
            "/opt/Cellar/other/mailtriage",
            "/opt/mailtriage/Cellar/bin/mailtriage",
            "/nix/storefront/mailtriage",
        ] {
            assert_eq!(package_manager(Path::new(plain)), None, "{plain}");
        }
    }

    #[test]
    fn a_private_directory_is_replaceable_and_a_group_writable_one_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir(&bin).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        let exe = script(&bin, "mailtriage", "echo mailtriage 0.1.0");
        let exe = fs::canonicalize(exe).unwrap();
        assert_eq!(blocker(&exe, Some("linux-amd64")), None);
        assert_eq!(blocker(&exe, None), Some(Blocker::UnsupportedPlatform));
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o775)).unwrap();
        assert_eq!(
            blocker(&exe, Some("linux-amd64")),
            Some(Blocker::UnsafePermissions)
        );
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o757)).unwrap();
        assert_eq!(
            blocker(&exe, Some("linux-amd64")),
            Some(Blocker::UnsafePermissions)
        );
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        if crate::system_service::current_uid() != 0 {
            fs::set_permissions(&bin, fs::Permissions::from_mode(0o555)).unwrap();
            assert_eq!(
                blocker(&exe, Some("linux-amd64")),
                Some(Blocker::NotWritable)
            );
            fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(
            Blocker::NotWritable.fix(Path::new("/opt/mailtriage/mailtriage")),
            "make /opt/mailtriage writable for this user, or set \"updates\" to \"notify\""
        );
        assert_eq!(
            Blocker::ManagedByHomebrew.fix(&exe),
            "run `brew upgrade mailtriage`"
        );
    }

    #[test]
    fn identity_changes_with_content_and_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = script(dir.path(), "a", "true");
        let first = FileIdentity::read(&path).unwrap();
        assert_eq!(FileIdentity::read(&path).unwrap(), first);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let chmodded = FileIdentity::read(&path).unwrap();
        assert_ne!(chmodded, first);
        let other = script(dir.path(), "b", "true");
        fs::rename(&other, &path).unwrap();
        assert_ne!(FileIdentity::read(&path).unwrap().ino, chmodded.ino);
    }

    #[test]
    fn probes_name_why_a_binary_does_not_run() {
        let dir = tempfile::tempdir().unwrap();
        let ok = script(dir.path(), "ok", "echo 'mailtriage 0.3.0'");
        assert_eq!(probe(&ok, CLI), Ok(Version::new(0, 3, 0)));
        let wrong = script(
            dir.path(),
            "wrong",
            "echo 'mailtriage 0.3'; echo 'some detail' >&2",
        );
        assert_eq!(
            probe(&wrong, CLI),
            Err("printed \"mailtriage 0.3\\n\"; stderr: some detail".into())
        );
        let failing = script(
            dir.path(),
            "failing",
            "echo '/lib/libc.so.6: version GLIBC_2.39 not found' >&2; exit 1",
        );
        assert_eq!(
            probe(&failing, CLI),
            Err("exited with 1; stderr: /lib/libc.so.6: version GLIBC_2.39 not found".into())
        );
        let killed = script(dir.path(), "killed", "kill -9 $$");
        assert_eq!(probe(&killed, CLI), Err("killed by signal 9".into()));
        let plain = dir.path().join("plain");
        fs::write(&plain, "not a program").unwrap();
        assert_eq!(probe(&plain, CLI), Err("could not start".into()));
        assert_eq!(
            probe(&dir.path().join("missing"), CLI),
            Err("could not start".into())
        );
    }
}
```

- [ ] **Step 5: Reading an archive (`src/update/archive.rs`)**

`tar::Archive` folds GNU long names and pax local headers into the next entry, so `path_bytes()` is the full path. The decompressed byte count includes headers and padding; draining the decoder after the last entry verifies the gzip trailer (CRC and length), which catches truncation.

Create `src/update/archive.rs`:

```rust
//! Reading a release archive: a gzip tar stream, read to its end under
//! limits before anything from it is used.
use flate2::read::GzDecoder;
use std::io::{self, Read};
use tar::EntryType;

/// Limits on one archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_entries: usize,
    /// Decompressed bytes in total, tar headers and padding included.
    pub max_total: u64,
    /// The size of the wanted file.
    pub max_file: u64,
    /// Bytes in an entry path.
    pub max_path: usize,
}

/// The limits for release archives.
pub const RELEASE: Limits = Limits {
    max_entries: 16,
    max_total: 256 * 1024 * 1024,
    max_file: 200 * 1024 * 1024,
    max_path: 255,
};

/// The contents of the single top-level regular file `name` in the gzip
/// tar `data`. The whole stream is read first: more entries than allowed,
/// more decompressed bytes than allowed, a path that is too long, absolute
/// or holds `..`, a link or special file, no `name` or a second one, or a
/// truncated stream fail the archive. Other regular files (`README.md`,
/// `LICENSE`, macOS `._*` metadata) and directories are skipped.
pub fn extract(data: &[u8], name: &str, limits: &Limits) -> Result<Vec<u8>, String> {
    let fail = |why: &str| format!("bad archive: {why}");
    let counted = Counted {
        inner: GzDecoder::new(data),
        left: limits.max_total,
    };
    let mut archive = tar::Archive::new(counted);
    let mut found: Option<Vec<u8>> = None;
    let mut entries = 0usize;
    for entry in archive.entries().map_err(|e| fail(&reason(&e)))? {
        let mut entry = entry.map_err(|e| fail(&reason(&e)))?;
        entries += 1;
        if entries > limits.max_entries {
            return Err(fail(&format!("more than {} entries", limits.max_entries)));
        }
        let kind = entry.header().entry_type();
        if kind == EntryType::XGlobalHeader {
            continue;
        }
        let path = entry.path_bytes().into_owned();
        if path.len() > limits.max_path {
            return Err(fail(&format!(
                "an entry path is longer than {} bytes",
                limits.max_path
            )));
        }
        if path.starts_with(b"/") {
            return Err(fail("an entry has an absolute path"));
        }
        let path = path.strip_prefix(b"./").unwrap_or(&path);
        if path.split(|b| *b == b'/').any(|part| part == b"..") {
            return Err(fail("an entry path contains .."));
        }
        let regular = matches!(kind, EntryType::Regular | EntryType::Continuous);
        if !regular && kind != EntryType::Directory {
            return Err(fail("it contains a link or a special file"));
        }
        if !regular || path != name.as_bytes() {
            continue;
        }
        if found.is_some() {
            return Err(fail(&format!("it contains two {name} entries")));
        }
        let size = entry.size();
        if size > limits.max_file {
            return Err(fail(&format!(
                "{name} is larger than {} bytes",
                limits.max_file
            )));
        }
        let mut contents = Vec::with_capacity(size.min(16 * 1024 * 1024) as usize);
        entry
            .read_to_end(&mut contents)
            .map_err(|e| fail(&reason(&e)))?;
        if contents.len() as u64 != size {
            return Err(fail("the stream is truncated"));
        }
        found = Some(contents);
    }
    // The rest of the stream: tar padding and the gzip trailer, whose
    // checksum and length catch truncation and corruption.
    io::copy(&mut archive.into_inner(), &mut io::sink()).map_err(|e| fail(&reason(&e)))?;
    found.ok_or_else(|| fail(&format!("it has no top-level {name}")))
}

/// A reader that fails once more than `left` bytes come through.
struct Counted<R> {
    inner: R,
    left: u64,
}

const TOO_LARGE: &str = "it decompresses to more than the limit";

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let size = self.inner.read(buf)?;
        self.left = self
            .left
            .checked_sub(size as u64)
            .ok_or_else(|| io::Error::other(TOO_LARGE))?;
        Ok(size)
    }
}

fn reason(error: &io::Error) -> String {
    if error.to_string() == TOO_LARGE {
        return TOO_LARGE.to_owned();
    }
    match error.kind() {
        io::ErrorKind::UnexpectedEof => "the stream is truncated".to_owned(),
        _ => "it is not a valid gzip tar stream".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::GzEncoder, Compression};

    /// A gzip tar with raw headers, so tests can write what `tar::Builder`
    /// refuses (`..`, absolute paths).
    fn tar_gz(entries: &[(&[u8], EntryType, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        for (path, kind, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.as_mut_bytes()[..path.len()].copy_from_slice(path);
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_entry_type(*kind);
            if *kind == EntryType::Symlink {
                header.set_link_name("/etc/passwd").unwrap();
            }
            header.set_cksum();
            builder.append(&header, *data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn release(binary: &[u8]) -> Vec<u8> {
        tar_gz(&[
            (b"mailtriage", EntryType::Regular, binary),
            (b"LICENSE", EntryType::Regular, b"MIT"),
            (b"README.md", EntryType::Regular, b"readme"),
        ])
    }

    fn error(data: &[u8]) -> String {
        extract(data, "mailtriage", &RELEASE).unwrap_err()
    }

    #[test]
    fn the_binary_is_extracted_and_other_files_are_skipped() {
        assert_eq!(
            extract(&release(b"#!bin"), "mailtriage", &RELEASE).unwrap(),
            b"#!bin"
        );
        let with_metadata = tar_gz(&[
            (b"./", EntryType::Directory, b""),
            (b"./._mailtriage", EntryType::Regular, b"apple"),
            (b"./mailtriage", EntryType::Regular, b"#!bin"),
        ]);
        assert_eq!(
            extract(&with_metadata, "mailtriage", &RELEASE).unwrap(),
            b"#!bin"
        );
    }

    #[test]
    fn pax_headers_are_understood() {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        builder
            .append_pax_extensions([("LIBARCHIVE.xattr.com.apple.provenance", &b"x"[..])])
            .unwrap();
        let mut header = tar::Header::new_ustar();
        header.set_size(5);
        header.set_mode(0o755);
        builder
            .append_data(&mut header, "mailtriage", &b"#!bin"[..])
            .unwrap();
        let data = builder.into_inner().unwrap().finish().unwrap();
        assert_eq!(extract(&data, "mailtriage", &RELEASE).unwrap(), b"#!bin");
    }

    #[test]
    fn unsafe_entries_fail_the_archive() {
        assert_eq!(
            error(&tar_gz(&[(b"mailtriage", EntryType::Symlink, b"")])),
            "bad archive: it contains a link or a special file"
        );
        assert_eq!(
            error(&tar_gz(&[(b"mailtriage", EntryType::Link, b"")])),
            "bad archive: it contains a link or a special file"
        );
        assert_eq!(
            error(&tar_gz(&[(b"../mailtriage", EntryType::Regular, b"x")])),
            "bad archive: an entry path contains .."
        );
        assert_eq!(
            error(&tar_gz(&[(b"a/../../x", EntryType::Regular, b"x")])),
            "bad archive: an entry path contains .."
        );
        assert_eq!(
            error(&tar_gz(&[(
                b"/usr/bin/mailtriage",
                EntryType::Regular,
                b"x"
            )])),
            "bad archive: an entry has an absolute path"
        );
    }

    #[test]
    fn the_binary_must_be_there_exactly_once() {
        assert_eq!(
            error(&tar_gz(&[(b"bin/mailtriage", EntryType::Regular, b"x")])),
            "bad archive: it has no top-level mailtriage"
        );
        assert_eq!(
            error(&tar_gz(&[
                (b"mailtriage", EntryType::Regular, b"x"),
                (b"./mailtriage", EntryType::Regular, b"y"),
            ])),
            "bad archive: it contains two mailtriage entries"
        );
        assert_eq!(
            error(&tar_gz(&[(b"mailtriage", EntryType::Directory, b"")])),
            "bad archive: it has no top-level mailtriage"
        );
    }

    #[test]
    fn release_limits_are_the_specs() {
        assert_eq!(
            RELEASE,
            Limits {
                max_entries: 16,
                max_total: 256 * 1024 * 1024,
                max_file: 200 * 1024 * 1024,
                max_path: 255,
            }
        );
    }

    /// The same checks with small limits, so the test needs no 256 MB.
    #[test]
    fn limits_are_enforced() {
        let many: Vec<(&[u8], EntryType, &[u8])> =
            vec![(b"README.md", EntryType::Regular, b"r"); 17];
        assert_eq!(error(&tar_gz(&many)), "bad archive: more than 16 entries");
        let small = Limits {
            max_entries: 16,
            max_total: 64 * 1024,
            max_file: 32 * 1024,
            max_path: 255,
        };
        // A decompression bomb: little gzip, much tar.
        let zeros = vec![0u8; 100 * 1024];
        let bomb = tar_gz(&[(b"README.md", EntryType::Regular, &zeros)]);
        assert!(bomb.len() < 4096);
        assert_eq!(
            extract(&bomb, "mailtriage", &small).unwrap_err(),
            "bad archive: it decompresses to more than the limit"
        );
        let big = tar_gz(&[(b"mailtriage", EntryType::Regular, &zeros[..40 * 1024])]);
        assert_eq!(
            extract(&big, "mailtriage", &small).unwrap_err(),
            "bad archive: mailtriage is larger than 32768 bytes"
        );
        let long = vec![b'a'; 256];
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        builder
            .append_data(&mut header, String::from_utf8(long).unwrap(), &b""[..])
            .unwrap();
        let data = builder.into_inner().unwrap().finish().unwrap();
        assert_eq!(
            error(&data),
            "bad archive: an entry path is longer than 255 bytes"
        );
    }

    #[test]
    fn a_truncated_or_corrupt_stream_fails() {
        let whole = release(&vec![7u8; 50_000]);
        assert_eq!(
            error(&whole[..whole.len() - 10]),
            "bad archive: the stream is truncated"
        );
        assert_eq!(
            error(&whole[..whole.len() / 2]),
            "bad archive: the stream is truncated"
        );
        assert_eq!(
            error(b"not gzip at all"),
            "bad archive: it is not a valid gzip tar stream"
        );
        let mut corrupt = whole.clone();
        let at = corrupt.len() - 6;
        corrupt[at] ^= 0xff;
        assert!(extract(&corrupt, "mailtriage", &RELEASE).is_err());
    }
}
```

- [ ] **Step 6: The transaction (`src/update/install.rs`)**

Steps 4 to 10 of the spec, in order. The commit point is `fs::rename(staged, path)`; `Scratch` removes this attempt's files on every error before it, and `keep()` disarms it at the commit point so a backup that could not be published stays where it is.

Create `src/update/install.rs`:

```rust
//! Installing a release (spec steps 4 to 10): under the installation lock,
//! download, verify, unpack, smoke-test, then replace the binary by rename.
//! The rename is the commit point; later problems are warnings.
use super::{
    archive,
    cache::Cache,
    github::{self, Net},
    platform::{self, FileIdentity},
    release::{self, CachedRelease},
    schedule, version, Component,
};
use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use fs2::FileExt;
use semver::Version;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

/// The installation lock, in the installation path's directory. Created
/// when missing and never deleted.
pub const LOCK_FILE: &str = ".mailtriage-update.lock";
/// Temporary files of an update attempt: `.tmp` (the staged binary) and
/// `.prev` (the backup before it is published).
pub const TEMP_PREFIX: &str = ".mailtriage-update-";
/// The hidden test hook (debug builds only).
pub const TEST_HOOK: &str = "MAILTRIAGE_UPDATE_TEST_HOOK";
/// The hidden override of `update`'s lock wait in milliseconds (debug builds only).
pub const TEST_LOCK_WAIT: &str = "MAILTRIAGE_UPDATE_TEST_LOCK_WAIT_MS";

/// Points where tests inject faults. Production code uses `EnvHooks`,
/// which does nothing in release builds.
pub trait Hooks {
    /// Called before the step named `point`; an error fails that step.
    fn at(&self, point: &str) -> Result<()>;
}

/// No hooks.
pub struct NoHooks;

impl Hooks for NoHooks {
    fn at(&self, _point: &str) -> Result<()> {
        Ok(())
    }
}

/// In debug builds, `MAILTRIAGE_UPDATE_TEST_HOOK` names a program that runs
/// with the hook point as its only argument; a non-zero exit fails the
/// step. Release builds never read the variable.
pub struct EnvHooks;

impl Hooks for EnvHooks {
    #[cfg(debug_assertions)]
    fn at(&self, point: &str) -> Result<()> {
        use std::process::{Command, Stdio};
        let Some(program) = std::env::var_os(TEST_HOOK) else {
            return Ok(());
        };
        let status = Command::new(program)
            .arg(point)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .status()?;
        if !status.success() {
            bail!("the test hook failed at {point}");
        }
        Ok(())
    }

    #[cfg(not(debug_assertions))]
    fn at(&self, _point: &str) -> Result<()> {
        Ok(())
    }
}

/// How long `update` waits for the installation lock: 60 s.
pub fn update_lock_wait() -> Duration {
    #[cfg(debug_assertions)]
    if let Some(ms) = std::env::var(TEST_LOCK_WAIT)
        .ok()
        .and_then(|v| v.parse().ok())
    {
        return Duration::from_millis(ms);
    }
    Duration::from_secs(60)
}

/// The held installation lock.
pub struct InstallLock {
    _file: File,
}

/// Takes the installation lock of `dir`, trying for up to `wait` (zero:
/// once). `None` when another updater holds it.
pub fn lock(dir: &Path, wait: Duration) -> Result<Option<InstallLock>> {
    let path = dir.join(LOCK_FILE);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    let deadline = Instant::now() + wait;
    loop {
        if file.try_lock_exclusive().is_ok() {
            return Ok(Some(InstallLock { _file: file }));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// Removes leftover temporary files of earlier attempts in `dir`. Only
/// called under the installation lock, so no cooperating updater is using them.
pub fn remove_leftovers(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let stale = entry.file_name().to_string_lossy().starts_with(TEMP_PREFIX)
            && entry
                .file_type()
                .is_ok_and(|t| t.is_file() || t.is_symlink());
        if stale {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// What to install where.
pub struct Job<'a> {
    pub net: &'a Net,
    pub component: Component,
    /// The canonical installation path.
    pub path: &'a Path,
    pub release: &'a CachedRelease,
    /// Compared with the candidate when the installed version cannot be
    /// read: the running version, so an unreadable file is never
    /// downgraded below it.
    pub fallback: &'a Version,
    /// Where step 10 records the installation; `None` records nothing.
    pub cache: Option<&'a Cache>,
    pub hooks: &'a dyn Hooks,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The candidate is not newer than the installed version.
    Current {
        installed: Option<Version>,
    },
    Installed(Installed),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Installed {
    pub from: Option<Version>,
    pub to: Version,
    /// Where the previous binary is: `<path>.previous`, or the temporary
    /// backup when publishing it failed.
    pub previous_path: PathBuf,
    /// Problems after the commit point; the update still counts as done.
    pub warnings: Vec<String>,
}

/// Steps 4 to 10 under the held installation lock. `before_download` runs
/// once the candidate is known to be newer (`watch` writes its reservation
/// there); its error stops the install. Errors leave the installation path
/// and `<path>.previous` untouched and remove this attempt's files.
pub fn install(
    job: &Job,
    _lock: &InstallLock,
    before_download: &mut dyn FnMut() -> Result<()>,
) -> Result<Outcome> {
    let path = job.path;
    let dir = path
        .parent()
        .context("the installation path has no directory")?;
    // 4. Stale files, then the installed version and identity.
    remove_leftovers(dir);
    let installed_identity =
        FileIdentity::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let installed = platform::probe(path, job.component).ok();
    let candidate = Version::parse(&job.release.version).context("invalid release version")?;
    if !version::is_newer(&candidate, installed.as_ref().unwrap_or(job.fallback)) {
        return Ok(Outcome::Current { installed });
    }
    let platform_name = release::platform().unwrap_or("this platform");
    let asset = job
        .release
        .archives
        .get(job.component.name)
        .cloned()
        .flatten()
        .ok_or_else(|| anyhow!("release v{candidate} has no {platform_name} archive"))?;
    let sums = job
        .release
        .sums
        .clone()
        .ok_or_else(|| anyhow!("release v{candidate} has no SHA256SUMS"))?;
    before_download()?;
    // 5. Download. 6. Verify.
    let sums_text = job
        .net
        .download(&sums.url, sums.size, github::MAX_SUMS_BYTES)
        .map_err(|e| anyhow!("cannot download SHA256SUMS of v{candidate}: {e}"))?;
    let expected = release::expected_sha256(&String::from_utf8_lossy(&sums_text), &asset.name)
        .map_err(|e| anyhow!("release v{candidate}: {e}"))?;
    let bytes = job
        .net
        .download(&asset.url, asset.size, github::MAX_ASSET_BYTES)
        .map_err(|e| anyhow!("cannot download {}: {e}", asset.name))?;
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
    {
        warnings.push(format!("installed; syncing {} failed: {e}", dir.display()));
    }
    // 10. Record.
    if let Some(cache) = job.cache {
        let key = path.to_string_lossy().into_owned();
        let identity = FileIdentity::read(path).ok();
        let recorded = job.hooks.at("record").and_then(|_| {
            cache.update(|c| {
                let entry = c.installs.entry(key).or_default();
                entry.version = Some(candidate.to_string());
                entry.at = Some(schedule::stamp(Utc::now()));
                entry.last_error = None;
                entry.failures = 0;
                entry.next_attempt_at = None;
                entry.identity = identity;
            })
        });
        if let Err(e) = recorded {
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
pub fn previous_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".previous");
    PathBuf::from(name)
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

impl Drop for Scratch {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

/// Created exclusively with mode 0600 (never following an existing link),
/// fsynced and closed, then made executable.
fn write_staged(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents)?;
    file.sync_all()?;
    drop(file);
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

/// A hard link to `path` at `backup`, or, where the filesystem refuses
/// hard links, a copy into an exclusively created file.
fn copy_or_link(path: &Path, backup: &Path) -> io::Result<()> {
    if fs::hard_link(path, backup).is_ok() {
        return Ok(());
    }
    let mut source = File::open(path)?;
    let mut copy = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(backup)?;
    io::copy(&mut source, &mut copy)?;
    copy.sync_all()?;
    fs::set_permissions(backup, fs::metadata(path)?.permissions())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
```

In `src/update/cache.rs`, `InstallEntry` gains the identity that `install` records:

In `src/update/cache.rs`, replace:

```rust
    #[serde(default)]
    pub next_attempt_at: Option<String>,
}
```

with:

```rust
    #[serde(default)]
    pub next_attempt_at: Option<String>,
    /// The file's identity when `version` was recorded; another identity
    /// means `--version` must run again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<super::platform::FileIdentity>,
}
```

In `src/update/mod.rs`, add `pub mod archive;`, `pub mod install;` and `pub mod platform;` to the module list, keeping it alphabetical.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test --locked --lib -- process:: update::platform update::archive && cargo test --locked --test update_install --test key_command`
Expected: all PASS; the key-command tests show `run_bounded` unchanged.

- [ ] **Step 8: Full check and commit**

Run: `cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`
Expected: all pass.

```bash
git add src/process.rs src/system_service.rs src/update tests/update_support/mod.rs tests/update_install.rs
git commit -m "Install a release with a verified, revalidated swap

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: `mailtriage update`

**Files:**
- Create: `src/update/service_files.rs`, `src/update/command.rs`, `tests/update_command.rs`
- Modify: `src/system_service.rs` (`Context::unit_path`, new `unit_dir`, `is_marked` crate-visible), `src/update/mod.rs`, `src/cli.rs` (`Command::Update`, `UpdateArg`, `execute`), `tests/update_support/mod.rs` (append), `docs/guide.md`, `docs/development/service-api.md`

**Interfaces:**
- Consumes: Tasks 2 and 3 (`check::refresh`, `Reservation::BestEffort`, `platform::{installation_path, probe, blocker, Blocker}`, `install::{lock, update_lock_wait, install, Job, Outcome, Hooks, EnvHooks}`), `service::err`.
- Produces:
  - `system_service::unit_dir(Manager, home: &Path) -> PathBuf`; `system_service::is_marked(Manager, &str) -> bool` is `pub(crate)`.
  - `update::service_files::{ServiceFile { account, manager, unit_path, executable: Option<PathBuf> }, platform_manager() -> Option<Manager>, list(Manager, home: &Path) -> Vec<ServiceFile>, executable(Manager, text: &str) -> Option<PathBuf>, plist_arguments(&str) -> Option<Vec<String>>, unit_arguments(&str) -> Option<Vec<String>>}`. The argument decoders are what the tray spec uses to decode `--config` from a unit file.
  - `update::command::run(check_only: bool, hooks: &dyn Hooks) -> anyhow::Result<Value>`; `update::command::install_block(&Path, Option<Blocker>) -> Value`.
  - CLI: `mailtriage update [--check] [--json]`; no config is resolved.
  - Hook point: `checked` (after the check, before the installed version is read).
  - Tests: `update_support::{Sandbox { dir, bin, home, xdg }` (`new()`, `at(relative)`, `root()`, `command(&Server)`, `cache_file()`, `cache()`), `cache_dir(home, xdg)`, `run(&mut Command) -> (Option<i32>, Value, String)}`.

- [ ] **Step 1: Write the failing tests**

Append the sandbox to `tests/update_support/mod.rs`. `PATH` is only `/usr/bin:/bin`, so no real `launchctl`/`systemctl` from elsewhere is found, and `update` never runs one anyway:

```rust
use std::path::PathBuf;

/// A private temporary directory with an `install/` directory (0755)
/// holding a copy of the binary under test, and `home/` and `xdg/` for
/// `HOME` and `XDG_CACHE_HOME`.
pub struct Sandbox {
    pub dir: tempfile::TempDir,
    pub bin: PathBuf,
    pub home: PathBuf,
    pub xdg: PathBuf,
}

impl Sandbox {
    pub fn new() -> Self {
        Self::at("install")
    }

    /// The copy at `<tmp>/<relative>/mailtriage`.
    pub fn at(relative: &str) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let install = root.join(relative);
        std::fs::create_dir_all(&install).unwrap();
        let mut walk = install.clone();
        while walk != root {
            std::fs::set_permissions(&walk, std::fs::Permissions::from_mode(0o755)).unwrap();
            walk.pop();
        }
        let bin = install.join("mailtriage");
        std::fs::copy(env!("CARGO_BIN_EXE_mailtriage"), &bin).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let home = root.join("home");
        let xdg = root.join("xdg");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&xdg).unwrap();
        Self {
            dir,
            bin,
            home,
            xdg,
        }
    }

    pub fn root(&self) -> PathBuf {
        std::fs::canonicalize(self.dir.path()).unwrap()
    }

    /// The copy run with this sandbox's HOME and XDG_CACHE_HOME, the
    /// server as GitHub, and no inherited config or test hook.
    pub fn command(&self, server: &Server) -> std::process::Command {
        let mut command = std::process::Command::new(&self.bin);
        command
            .current_dir(self.root())
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.xdg)
            .env("MAILTRIAGE_UPDATE_URL", &server.base)
            .env("PATH", "/usr/bin:/bin")
            .env_remove("MAILTRIAGE_CONFIG")
            .env_remove("MAILTRIAGE_UPDATE_TEST_HOOK")
            .env_remove("MAILTRIAGE_UPDATE_TEST_LOCK_WAIT_MS");
        command
    }

    /// `update.json` in this sandbox's cache directory.
    pub fn cache_file(&self) -> PathBuf {
        cache_dir(&self.home, &self.xdg).join("update.json")
    }

    pub fn cache(&self) -> Value {
        std::fs::read(self.cache_file())
            .ok()
            .and_then(|d| serde_json::from_slice(&d).ok())
            .unwrap_or(Value::Null)
    }
}

/// The cache directory mailtriage uses for these HOME and XDG_CACHE_HOME.
pub fn cache_dir(home: &Path, xdg: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Caches/mailtriage")
    } else {
        xdg.join("mailtriage")
    }
}

/// Runs `command`; returns its exit code and stdout as JSON.
pub fn run(command: &mut std::process::Command) -> (Option<i32>, Value, String) {
    let out = command.output().unwrap();
    (
        out.status.code(),
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}
```

Create `tests/update_command.rs`:

```rust
#![cfg(unix)]
//! `mailtriage update` through a copy of the binary in a temporary
//! directory, with a loopback server as GitHub.
mod update_support;
use mailtriage::system_service::{self, Manager, Unit};
use serde_json::{json, Value};
use std::{fs, os::unix::fs::PermissionsExt, path::Path, time::Duration};
use update_support::{
    download_path, fake_binary, platform, release_archive, run, sha256_hex, Reply, Sandbox, Server,
};

const RUNNING: &str = env!("CARGO_PKG_VERSION");

fn original() -> Vec<u8> {
    fs::read(env!("CARGO_BIN_EXE_mailtriage")).unwrap()
}

/// Files of update attempts left next to the binary.
fn leftovers(bin: &Path) -> Vec<String> {
    fs::read_dir(bin.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with(".mailtriage-update-"))
        .collect()
}

/// A marked service file for `account` running `exe`, in the sandbox HOME.
fn service_file(home: &Path, account: &str, exe: &Path) {
    let manager = if cfg!(target_os = "macos") {
        Manager::Launchd
    } else {
        Manager::Systemd
    };
    let unit = Unit {
        account: account.into(),
        exe: exe.to_path_buf(),
        config: home.join("mailtriage.json"),
        interval_seconds: 60,
        limit: 100,
        log_dir: home.join("logs"),
        path_env: None,
    };
    let dir = system_service::unit_dir(manager, home);
    fs::create_dir_all(&dir).unwrap();
    let (name, text) = match manager {
        Manager::Launchd => (
            format!("digital.wirdrei.mailtriage.{account}.plist"),
            system_service::plist(&unit),
        ),
        Manager::Systemd => (
            format!("mailtriage-{account}.service"),
            system_service::systemd_unit(&unit),
        ),
    };
    fs::write(dir.join(name), text).unwrap();
}

/// A hook script: runs `body` when called with `point`.
#[cfg(debug_assertions)]
fn hook(dir: &Path, point: &str, body: &str) -> std::path::PathBuf {
    let path = dir.join("hook.sh");
    fs::write(
        &path,
        format!("#!/bin/sh\n[ \"$1\" = {point} ] || exit 0\n{body}\n"),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn update_installs_a_newer_release_and_lists_the_services() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    let marker = sandbox.root().join("marker");
    server.publish("9.9.9", &release_archive(&fake_binary("9.9.9", &marker)));
    service_file(&sandbox.home, "work", &sandbox.bin);
    service_file(
        &sandbox.home,
        "other",
        Path::new("/opt/elsewhere/mailtriage"),
    );
    let (code, v, stderr) = run(sandbox.command(&server).args(["update", "--json"]));
    assert_eq!(code, Some(0), "{v} {stderr}");
    let u = &v["update"];
    assert_eq!(u["action"], "updated");
    assert_eq!(
        (u["from"].as_str(), u["to"].as_str()),
        (Some(RUNNING), Some("9.9.9"))
    );
    assert_eq!(u["path"], sandbox.bin.to_str().unwrap());
    let previous = format!("{}.previous", sandbox.bin.display());
    assert_eq!(u["previous_path"], previous);
    assert_eq!(u["warnings"], json!([]));
    let services = u["services"].as_array().unwrap();
    assert_eq!(services.len(), 2, "{u}");
    assert_eq!(
        (
            services[0]["account"].as_str(),
            services[0]["same_binary"].as_bool()
        ),
        (Some("other"), Some(false))
    );
    assert_eq!(
        (
            services[1]["account"].as_str(),
            services[1]["same_binary"].as_bool()
        ),
        (Some("work"), Some(true))
    );
    assert_eq!(services[1]["executable"], sandbox.bin.to_str().unwrap());
    assert_eq!(
        fs::read(&sandbox.bin).unwrap(),
        fake_binary("9.9.9", &marker)
    );
    assert_eq!(fs::read(&previous).unwrap(), original());
    assert!(leftovers(&sandbox.bin).is_empty());
    let cache = sandbox.cache();
    assert_eq!(
        cache["installs"][sandbox.bin.to_str().unwrap()]["version"],
        "9.9.9"
    );
    assert_eq!(cache["release"]["version"], "9.9.9");
    assert!(!marker.exists(), "the new binary only ran --version");
}

#[test]
fn update_reports_current_without_downloading() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    server.publish(RUNNING, &release_archive(b"never used"));
    let (code, v, _) = run(sandbox.command(&server).args(["update", "--json"]));
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(
        v["update"],
        json!({"action":"current","from":RUNNING,"to":RUNNING,"path":sandbox.bin,"warnings":[]})
    );
    assert_eq!(server.count("/download"), 0);
}

/// The installed file is newer than the running process: the hook swaps
/// it right after the check, as another updater would.
#[test]
#[cfg(debug_assertions)]
fn the_installed_version_decides_not_the_running_one() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    let marker = sandbox.root().join("marker");
    server.publish("9.9.9", &release_archive(&fake_binary("9.9.9", &marker)));
    let newer = sandbox.root().join("newer");
    fs::write(&newer, fake_binary("9.9.9", &marker)).unwrap();
    fs::set_permissions(&newer, fs::Permissions::from_mode(0o755)).unwrap();
    let script = hook(
        &sandbox.root(),
        "checked",
        &format!("mv '{}' '{}'", newer.display(), sandbox.bin.display()),
    );
    let (code, v, stderr) = run(sandbox
        .command(&server)
        .env("MAILTRIAGE_UPDATE_TEST_HOOK", &script)
        .args(["update", "--json"]));
    assert_eq!(code, Some(0), "{v} {stderr}");
    assert_eq!(v["update"]["action"], "current");
    assert_eq!(v["update"]["from"], "9.9.9");
    assert_eq!(server.count("/download"), 0);
}

/// Serves a release `update` must refuse; the fake binary logs to `marker`.
type Prepare = fn(&Server, &Path);

#[test]
fn refused_updates_exit_3_and_leave_the_binary() {
    let cases: [(&str, Prepare); 3] = [
        (
            "checksum mismatch for mailtriage-v9.9.9-",
            |server, marker| {
                let archive = release_archive(&fake_binary("9.9.9", marker));
                let name = format!("mailtriage-v9.9.9-{}.tar.gz", platform());
                let wrong = format!("{}  {name}\n", sha256_hex(b"x"));
                server.publish_with_sums("9.9.9", &archive, &wrong);
            },
        ),
        (
            "the new binary does not run here: printed version 9.9.8, expected 9.9.9",
            |server, marker| {
                server.publish("9.9.9", &release_archive(&fake_binary("9.9.8", marker)));
            },
        ),
        ("a redirect left GitHub", |server, marker| {
            let name = server.publish("9.9.9", &release_archive(&fake_binary("9.9.9", marker)));
            let away = Reply::redirect("https://evil.example/a.tar.gz");
            server.reply(&download_path("9.9.9", &name), away);
        }),
    ];
    for (expected, prepare) in cases {
        let sandbox = Sandbox::new();
        let server = Server::start();
        prepare(&server, &sandbox.root().join("marker"));
        let (code, v, _) = run(sandbox.command(&server).args(["update", "--json"]));
        assert_eq!(code, Some(3), "{expected}: {v}");
        let message = v["error"]["message"].as_str().unwrap();
        assert!(message.contains(expected), "{expected}: {message}");
        assert_eq!(fs::read(&sandbox.bin).unwrap(), original(), "{expected}");
        assert!(leftovers(&sandbox.bin).is_empty(), "{expected}");
    }
}

#[test]
fn a_binary_that_is_not_replaceable_is_refused_with_its_fix() {
    let brew = Sandbox::at("opt/homebrew/Cellar/mailtriage/0.1.0/bin");
    let server = Server::start();
    server.publish(
        "9.9.9",
        &release_archive(&fake_binary("9.9.9", Path::new("/dev/null"))),
    );
    let (code, v, _) = run(brew.command(&server).args(["update", "--json"]));
    assert_eq!(code, Some(3), "{v}");
    let message = v["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("(managed_by_homebrew): run `brew upgrade mailtriage`"),
        "{message}"
    );
    assert_eq!(fs::read(&brew.bin).unwrap(), original());

    let (code, v, _) = run(brew.command(&server).args(["update", "--check", "--json"]));
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(
        v["update"]["install"],
        json!({"path": brew.bin, "replaceable": false, "reason": "managed_by_homebrew", "fix": "run `brew upgrade mailtriage`"})
    );
    assert_eq!(v["update"]["available"], true);

    let locked = Sandbox::new();
    let dir = locked.bin.parent().unwrap().to_path_buf();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    let (code, v, _) = run(locked.command(&server).args(["update", "--json"]));
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    if mailtriage_is_root() {
        return;
    }
    assert_eq!(code, Some(3), "{v}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("(not_writable): make "),
        "{v}"
    );
    assert_eq!(fs::read(&locked.bin).unwrap(), original());
    assert!(leftovers(&locked.bin).is_empty());
}

fn mailtriage_is_root() -> bool {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
}

#[test]
fn check_reports_without_changing_the_binary() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    server.list(&[]);
    let (code, v, _) = run(sandbox
        .command(&server)
        .args(["update", "--check", "--json"]));
    assert_eq!(code, Some(0), "{v}");
    let u = &v["update"];
    assert_eq!(
        (u["current"].as_str(), u["installed"].as_str()),
        (Some(RUNNING), Some(RUNNING))
    );
    assert_eq!(
        (u["latest"].clone(), u["available"].clone()),
        (Value::Null, json!(false))
    );
    assert!(u["checked_at"].as_str().unwrap().ends_with('Z'));

    server.publish("9.9.9", &release_archive(b"unused"));
    let (code, v, _) = run(sandbox
        .command(&server)
        .args(["update", "--check", "--json"]));
    assert_eq!(code, Some(0), "{v}");
    let u = &v["update"];
    assert_eq!(
        (u["latest"].as_str(), u["available"].as_bool()),
        (Some("9.9.9"), Some(true))
    );
    assert_eq!(
        u["release_url"],
        "https://github.com/wir-drei-digital/mailtriage/releases/tag/v9.9.9"
    );
    assert_eq!(u["published_at"], "2026-11-02T09:00:00Z");
    assert_eq!(
        u["install"],
        json!({"path": sandbox.bin, "replaceable": true, "reason": null, "fix": null})
    );
    assert_eq!(server.count("/download"), 0);
    assert_eq!(fs::read(&sandbox.bin).unwrap(), original());
    assert_eq!(sandbox.cache()["release"]["version"], "9.9.9");

    server.reply(update_support::LIST, Reply::status(503));
    let (code, v, _) = run(sandbox
        .command(&server)
        .args(["update", "--check", "--json"]));
    assert_eq!(code, Some(3), "{v}");
    assert_eq!(
        v["error"]["message"],
        "cannot read the release list: GitHub answered 503"
    );
}

#[test]
fn update_needs_a_release_and_a_cache_directory() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    server.list(&[]);
    let (code, v, _) = run(sandbox.command(&server).args(["update", "--json"]));
    assert_eq!(
        (code, v["error"]["message"].as_str()),
        (Some(3), Some("no stable release of mailtriage was found"))
    );
    let (code, v, _) = run(sandbox
        .command(&server)
        .env_remove("HOME")
        .env_remove("XDG_CACHE_HOME")
        .args(["update", "--json"]));
    assert_eq!(code, Some(3), "{v}");
    assert!(
        v["error"]["message"].as_str().unwrap().contains("HOME"),
        "{v}"
    );
}

#[test]
#[cfg(debug_assertions)]
fn a_held_installation_lock_is_exit_5() {
    use fs2::FileExt;
    let sandbox = Sandbox::new();
    let server = Server::start();
    server.publish(
        "9.9.9",
        &release_archive(&fake_binary("9.9.9", Path::new("/dev/null"))),
    );
    let lock = fs::File::create(
        sandbox
            .bin
            .parent()
            .unwrap()
            .join(".mailtriage-update.lock"),
    )
    .unwrap();
    lock.lock_exclusive().unwrap();
    let (code, v, _) = run(sandbox
        .command(&server)
        .env("MAILTRIAGE_UPDATE_TEST_LOCK_WAIT_MS", "300")
        .args(["update", "--json"]));
    assert_eq!(code, Some(5), "{v}");
    assert_eq!(v["error"]["message"], "another update is running");
    assert_eq!(fs::read(&sandbox.bin).unwrap(), original());
}

/// Two caches (different HOME and XDG_CACHE_HOME) against one binary: the
/// installation lock serializes them, and the second finds it current.
#[test]
fn updaters_with_different_caches_serialize_on_the_installation_lock() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    let archive = release_archive(&fake_binary("9.9.9", &sandbox.root().join("marker")));
    let name = server.publish("9.9.9", &archive);
    let path = download_path("9.9.9", &name);
    server.reply(&path, Reply::ok(archive).delayed(Duration::from_secs(1)));
    let (home2, xdg2) = (sandbox.root().join("home2"), sandbox.root().join("xdg2"));
    fs::create_dir_all(&home2).unwrap();
    fs::create_dir_all(&xdg2).unwrap();
    let spawn = |command: &mut std::process::Command| {
        command
            .args(["update", "--json"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    };
    let first = spawn(&mut sandbox.command(&server));
    let second = spawn(
        sandbox
            .command(&server)
            .env("HOME", &home2)
            .env("XDG_CACHE_HOME", &xdg2),
    );
    let mut actions: Vec<String> = [first, second]
        .into_iter()
        .map(|child| {
            let out = child.wait_with_output().unwrap();
            assert_eq!(out.status.code(), Some(0));
            let v: Value = serde_json::from_slice(&out.stdout).unwrap();
            v["update"]["action"].as_str().unwrap().to_owned()
        })
        .collect();
    actions.sort();
    assert_eq!(actions, ["current", "updated"]);
    assert_eq!(server.count(&path), 1);
}

#[test]
#[cfg(debug_assertions)]
fn a_binary_replaced_during_the_update_is_kept() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    server.publish(
        "9.9.9",
        &release_archive(&fake_binary("9.9.9", Path::new("/dev/null"))),
    );
    let theirs = sandbox.root().join("theirs");
    fs::write(&theirs, "#!/bin/sh\necho 'mailtriage 5.0.0'\n").unwrap();
    // Renamed into place, as installers do: on Linux, writing into the
    // file of the running binary fails (ETXTBSY).
    let script = hook(
        &sandbox.root(),
        "revalidate",
        &format!(
            "cp '{0}' '{0}.new' && mv '{0}.new' '{1}'",
            theirs.display(),
            sandbox.bin.display()
        ),
    );
    let (code, v, _) = run(sandbox
        .command(&server)
        .env("MAILTRIAGE_UPDATE_TEST_HOOK", &script)
        .args(["update", "--json"]));
    assert_eq!(code, Some(3), "{v}");
    assert_eq!(
        v["error"]["message"],
        "the installed binary changed during the update; try again"
    );
    assert_eq!(fs::read(&sandbox.bin).unwrap(), fs::read(&theirs).unwrap());
    assert!(leftovers(&sandbox.bin).is_empty());
}

#[test]
#[cfg(debug_assertions)]
fn faults_through_the_test_hook() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    let marker = sandbox.root().join("marker");
    server.publish("9.9.9", &release_archive(&fake_binary("9.9.9", &marker)));
    let script = hook(&sandbox.root(), "commit", "exit 1");
    let (code, v, _) = run(sandbox
        .command(&server)
        .env("MAILTRIAGE_UPDATE_TEST_HOOK", &script)
        .args(["update", "--json"]));
    assert_eq!(code, Some(3), "{v}");
    assert_eq!(fs::read(&sandbox.bin).unwrap(), original());
    assert!(leftovers(&sandbox.bin).is_empty());

    let script = hook(&sandbox.root(), "record", "exit 1");
    let (code, v, _) = run(sandbox
        .command(&server)
        .env("MAILTRIAGE_UPDATE_TEST_HOOK", &script)
        .args(["update", "--json"]));
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(
        v["update"]["warnings"],
        json!(["installed; recording the update failed: the test hook failed at record"])
    );
    assert_eq!(
        fs::read(&sandbox.bin).unwrap(),
        fake_binary("9.9.9", &marker)
    );
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --locked --test update_command`
Expected: compile error: `unit_dir` not found in `system_service`.

- [ ] **Step 3: Service file locations (`src/system_service.rs`)**

In `src/system_service.rs`, replace:

```rust
    pub fn unit_path(&self, account: &str) -> PathBuf {
        match self.manager {
            Manager::Launchd => self
                .home
                .join("Library/LaunchAgents")
                .join(format!("{}.plist", label(account))),
            Manager::Systemd => self
                .home
                .join(".config/systemd/user")
                .join(unit_name(account)),
        }
    }
}
```

with:

```rust
    pub fn unit_path(&self, account: &str) -> PathBuf {
        let dir = unit_dir(self.manager, &self.home);
        match self.manager {
            Manager::Launchd => dir.join(format!("{}.plist", label(account))),
            Manager::Systemd => dir.join(unit_name(account)),
        }
    }
}

/// The directory that holds `manager`'s per-user service files.
pub fn unit_dir(manager: Manager, home: &Path) -> PathBuf {
    match manager {
        Manager::Launchd => home.join("Library/LaunchAgents"),
        Manager::Systemd => home.join(".config/systemd/user"),
    }
}
```

In `src/system_service.rs`, replace:

```rust
fn is_marked(manager: Manager, text: &str) -> bool {
```

with:

```rust
pub(crate) fn is_marked(manager: Manager, text: &str) -> bool {
```

- [ ] **Step 4: Reading service files (`src/update/service_files.rs`)**

The exact inverse of `plist` (`xml`: `&`, `<`, `>` escaped) and `systemd_unit` (`systemd_arg`: `%`/`$` doubled, then quoted with `\\` and `\"` when the word holds whitespace, a quote, a backslash or `;`). Anything the writers cannot produce decodes to `None`.

Create `src/update/service_files.rs`:

```rust
//! Reading the service files mailtriage wrote: the exact inverse of
//! `system_service::plist` and `system_service::systemd_unit`.
use crate::{
    config,
    system_service::{self, Manager, LABEL_PREFIX},
};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// A service file mailtriage wrote (its marker is present).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceFile {
    pub account: String,
    pub manager: Manager,
    pub unit_path: PathBuf,
    /// Decoded from the file; `None` when it cannot be decoded.
    pub executable: Option<PathBuf>,
}

/// The manager whose files this platform uses, without asking for the tool.
pub fn platform_manager() -> Option<Manager> {
    if cfg!(target_os = "macos") {
        Some(Manager::Launchd)
    } else if cfg!(target_os = "linux") {
        Some(Manager::Systemd)
    } else {
        None
    }
}

/// Every marked service file in `manager`'s directory under `home`, by
/// account name.
pub fn list(manager: Manager, home: &Path) -> Vec<ServiceFile> {
    let Ok(entries) = fs::read_dir(system_service::unit_dir(manager, home)) else {
        return vec![];
    };
    let mut files: Vec<ServiceFile> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let account = account_of(manager, &name)?;
            let text = fs::read_to_string(entry.path()).ok()?;
            system_service::is_marked(manager, &text).then(|| ServiceFile {
                account,
                manager,
                unit_path: entry.path(),
                executable: executable(manager, &text),
            })
        })
        .collect();
    files.sort_by(|a, b| a.account.cmp(&b.account));
    files
}

/// The account a service file name belongs to.
fn account_of(manager: Manager, file_name: &str) -> Option<String> {
    let account = match manager {
        Manager::Launchd => file_name
            .strip_prefix(LABEL_PREFIX)?
            .strip_prefix('.')?
            .strip_suffix(".plist")?,
        Manager::Systemd => file_name
            .strip_prefix("mailtriage-")?
            .strip_suffix(".service")?,
    };
    config::valid_account_name(account).then(|| account.to_owned())
}

/// The executable a service file runs: the first program argument.
pub fn executable(manager: Manager, text: &str) -> Option<PathBuf> {
    let arguments = match manager {
        Manager::Launchd => plist_arguments(text)?,
        Manager::Systemd => unit_arguments(text)?,
    };
    arguments
        .into_iter()
        .next()
        .filter(|exe| Path::new(exe).is_absolute())
        .map(PathBuf::from)
}

/// The `<string>`s of `ProgramArguments`, XML entities decoded.
pub fn plist_arguments(text: &str) -> Option<Vec<String>> {
    let (_, rest) = text.split_once("<key>ProgramArguments</key>")?;
    let mut rest = rest.trim_start().strip_prefix("<array>")?;
    let mut arguments = Vec::new();
    loop {
        rest = rest.trim_start();
        if rest.starts_with("</array>") {
            return Some(arguments);
        }
        let (value, after) = rest.strip_prefix("<string>")?.split_once("</string>")?;
        arguments.push(xml_decode(value)?);
        rest = after;
    }
}

fn xml_decode(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let (entity, after) = rest[at + 1..].split_once(';')?;
        out.push(match entity {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            _ => return None,
        });
        rest = after;
    }
    if rest.contains('<') {
        return None;
    }
    out.push_str(rest);
    Some(out)
}

/// The words of the unit's `ExecStart=`, `systemd_arg`'s quoting and its
/// `%`/`$` doubling undone.
pub fn unit_arguments(text: &str) -> Option<Vec<String>> {
    let line = text.lines().find_map(|l| l.strip_prefix("ExecStart="))?;
    let mut words = Vec::new();
    let mut chars = line.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        if chars.peek().is_none() {
            return Some(words);
        }
        let mut word = String::new();
        if chars.next_if_eq(&'"').is_some() {
            loop {
                match chars.next()? {
                    '\\' => word.push(chars.next()?),
                    '"' => break,
                    c => word.push(c),
                }
            }
            if chars.peek().is_some_and(|c| !c.is_whitespace()) {
                return None;
            }
        } else {
            while let Some(c) = chars.next_if(|c| !c.is_whitespace()) {
                if matches!(c, '"' | '\'' | '\\' | ';') {
                    return None;
                }
                word.push(c);
            }
        }
        words.push(undouble(&word)?);
    }
}

/// `%%` → `%` and `$$` → `$`; a single one was not written by mailtriage.
fn undouble(word: &str) -> Option<String> {
    let mut out = String::with_capacity(word.len());
    let mut chars = word.chars();
    while let Some(c) = chars.next() {
        if matches!(c, '%' | '$') && chars.next()? != c {
            return None;
        }
        out.push(c);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system_service::{plist, systemd_unit, Unit};

    fn unit(exe: &str) -> Unit {
        Unit {
            account: "work".into(),
            exe: PathBuf::from(exe),
            config: PathBuf::from("/Users/a & b/.config/mailtriage/mailtriage.json"),
            interval_seconds: 60,
            limit: 100,
            log_dir: PathBuf::from("/tmp/logs"),
            path_env: Some("/usr/bin:/bin".into()),
        }
    }

    #[test]
    fn service_files_decode_back_to_their_arguments() {
        for exe in [
            "/usr/local/bin/mailtriage",
            "/opt/mail triage/bin/mailtriage",
            "/opt/50% off/mailtriage",
            "/opt/$HOME/mailtriage",
            "/opt/say \"hi\"/mailtriage",
            "/opt/it's/mailtriage",
            "/opt/back\\slash/mailtriage",
            "/opt/a & b/<x>/mailtriage",
            "/opt/semi;colon/mailtriage",
            "/opt/%%$$/mailtriage",
        ] {
            let unit = unit(exe);
            let expected = unit.arguments();
            assert_eq!(
                plist_arguments(&plist(&unit)),
                Some(expected.clone()),
                "{exe}"
            );
            assert_eq!(
                unit_arguments(&systemd_unit(&unit)),
                Some(expected),
                "{exe}"
            );
            assert_eq!(
                executable(Manager::Launchd, &plist(&unit)),
                Some(PathBuf::from(exe))
            );
            assert_eq!(
                executable(Manager::Systemd, &systemd_unit(&unit)),
                Some(PathBuf::from(exe))
            );
        }
    }

    #[test]
    fn files_that_cannot_be_decoded_give_no_executable() {
        assert_eq!(executable(Manager::Launchd, "<plist></plist>"), None);
        assert_eq!(
            executable(
                Manager::Launchd,
                "<key>ProgramArguments</key><array><string>/a&bogus;b</string></array>"
            ),
            None
        );
        assert_eq!(executable(Manager::Systemd, "[Service]\n"), None);
        assert_eq!(
            executable(Manager::Systemd, "ExecStart=\"/unterminated\n"),
            None
        );
        assert_eq!(executable(Manager::Systemd, "ExecStart=/a%b watch\n"), None);
        assert_eq!(
            executable(Manager::Systemd, "ExecStart=relative watch\n"),
            None
        );
    }

    #[test]
    fn listed_files_are_marked_and_named_for_an_account() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        for manager in [Manager::Launchd, Manager::Systemd] {
            let units = system_service::unit_dir(manager, home);
            fs::create_dir_all(&units).unwrap();
            let text = |u: &Unit| match manager {
                Manager::Launchd => plist(u),
                Manager::Systemd => systemd_unit(u),
            };
            let name = |account: &str| match manager {
                Manager::Launchd => format!("digital.wirdrei.mailtriage.{account}.plist"),
                Manager::Systemd => format!("mailtriage-{account}.service"),
            };
            fs::write(units.join(name("work")), text(&unit("/opt/a/mailtriage"))).unwrap();
            fs::write(units.join(name("home")), text(&unit("/opt/b/mailtriage"))).unwrap();
            fs::write(units.join(name("theirs")), "not written by mailtriage").unwrap();
            fs::write(
                units.join("digital.wirdrei.mailtriage-tray.plist"),
                text(&unit("/x")),
            )
            .unwrap();
            fs::write(units.join("other.service"), text(&unit("/x"))).unwrap();
            let found = list(manager, home);
            let accounts: Vec<_> = found.iter().map(|f| f.account.as_str()).collect();
            assert_eq!(accounts, ["home", "work"], "{manager:?}");
            assert_eq!(
                found[1].executable,
                Some(PathBuf::from("/opt/a/mailtriage"))
            );
            assert_eq!(found[1].unit_path, units.join(name("work")));
        }
        assert!(list(Manager::Launchd, &home.join("missing")).is_empty());
    }
}
```

- [ ] **Step 5: The command (`src/update/command.rs`)**

Order: installation path, cache directory, fresh check (`BestEffort`: a cache write failure is a warning), the `checked` hook, the installed version and replaceability, then for `--check` the report; otherwise current (exit 0) when the candidate is not newer, the refusal (exit 3) when the binary may not be replaced, the installation lock (60 s, exit 5) and the transaction.

Create `src/update/command.rs`:

```rust
//! `mailtriage update [--check]`: refresh the release information, then
//! report it or install the candidate. Needs no config.
use super::{
    cache::Cache,
    check::{self, Reservation},
    github::{Endpoint, Net},
    install::{self, Hooks, Job, Outcome},
    platform::{self, Blocker},
    release, service_files, version, CLI,
};
use crate::service::err;
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Runs `update`; `check_only` is `--check`. Errors carry the exit code:
/// 3 for network, release, archive and replaceability problems, 5 when
/// another update held the installation lock.
pub fn run(check_only: bool, hooks: &dyn Hooks) -> Result<Value> {
    let path = platform::installation_path().map_err(|e| err(3, e))?;
    let cache =
        Cache::for_user().ok_or_else(|| err(3, "cannot find the update cache: HOME is not set"))?;
    let net = Net::new(Endpoint::from_env())
        .map_err(|e| err(3, format!("cannot start an HTTPS client: {e}")))?;
    let checked =
        check::refresh(&net, &cache, Reservation::BestEffort).map_err(|e| err(3, e.to_string()))?;
    hooks.at("checked").map_err(|e| err(3, e.to_string()))?;
    let running = version::running();
    let installed = platform::probe(&path, CLI).ok();
    let blocker = platform::blocker(&path, release::platform());
    if check_only {
        let latest = checked.release.as_ref().map(|r| r.version.clone());
        let available = match (&latest, &installed) {
            (Some(latest), Some(installed)) => semver::Version::parse(latest)
                .is_ok_and(|latest| version::is_newer(&latest, installed)),
            _ => false,
        };
        return Ok(json!({"schema_version": 1, "update": {
            "current": version::RUNNING,
            "installed": installed.map(|v| v.to_string()),
            "latest": latest,
            "available": available,
            "release_url": checked.release.as_ref().map(|r| r.release_url.clone()),
            "published_at": checked.release.as_ref().and_then(|r| r.published_at.clone()),
            "checked_at": checked.checked_at,
            "install": install_block(&path, blocker),
            "warnings": checked.warnings,
        }}));
    }
    let release = checked
        .release
        .clone()
        .ok_or_else(|| err(3, "no stable release of mailtriage was found"))?;
    let candidate = semver::Version::parse(&release.version)
        .map_err(|_| err(3, "the release version is invalid"))?;
    let baseline = installed.clone().unwrap_or_else(|| running.clone());
    if !version::is_newer(&candidate, &baseline) {
        return Ok(current(&path, &baseline, checked.warnings));
    }
    if let Some(blocker) = blocker {
        return Err(err(
            3,
            format!(
                "{} cannot be replaced ({}): {}",
                path.display(),
                blocker.reason(),
                blocker.fix(&path)
            ),
        ));
    }
    let dir = path.parent().unwrap_or(Path::new("/"));
    let lock = install::lock(dir, install::update_lock_wait())
        .map_err(|e| err(3, format!("{e:#}")))?
        .ok_or_else(|| err(5, "another update is running"))?;
    let job = Job {
        net: &net,
        component: CLI,
        path: &path,
        release: &release,
        fallback: &running,
        cache: Some(&cache),
        hooks,
    };
    let outcome =
        install::install(&job, &lock, &mut || Ok(())).map_err(|e| err(3, format!("{e:#}")))?;
    drop(lock);
    match outcome {
        Outcome::Current { installed } => Ok(current(
            &path,
            installed.as_ref().unwrap_or(&running),
            checked.warnings,
        )),
        Outcome::Installed(done) => {
            let mut warnings = checked.warnings;
            warnings.extend(done.warnings);
            Ok(json!({"schema_version": 1, "update": {
                "action": "updated",
                "from": done.from.unwrap_or(running).to_string(),
                "to": done.to.to_string(),
                "path": path,
                "previous_path": done.previous_path,
                "warnings": warnings,
                "services": services(&path),
            }}))
        }
    }
}

fn current(path: &Path, version: &semver::Version, warnings: Vec<String>) -> Value {
    json!({"schema_version": 1, "update": {
        "action": "current",
        "from": version.to_string(),
        "to": version.to_string(),
        "path": path,
        "warnings": warnings,
    }})
}

/// `install` of `update --check`: whether the binary at `path` is replaceable.
pub fn install_block(path: &Path, blocker: Option<Blocker>) -> Value {
    json!({
        "path": path,
        "replaceable": blocker.is_none(),
        "reason": blocker.map(Blocker::reason),
        "fix": blocker.map(|b| b.fix(path)),
    })
}

/// Every service file mailtriage wrote for this user, and whether it runs
/// the binary at `path` (canonical paths compared).
fn services(path: &Path) -> Vec<Value> {
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from);
    let (Some(manager), Some(home)) = (service_files::platform_manager(), home) else {
        return vec![];
    };
    service_files::list(manager, &home)
        .into_iter()
        .map(|file| {
            let same_binary = file
                .executable
                .as_deref()
                .and_then(|exe| fs::canonicalize(exe).ok())
                .is_some_and(|exe| exe == path);
            json!({
                "account": file.account,
                "manager": manager.name(),
                "unit_path": file.unit_path,
                "executable": file.executable,
                "same_binary": same_binary,
            })
        })
        .collect()
}
```

In `src/update/mod.rs`, add `pub mod command;` and `pub mod service_files;` to the module list, keeping it alphabetical.

- [ ] **Step 6: The subcommand (`src/cli.rs`)**

In `src/cli.rs`, replace:

```rust
    setup,
    system_service::{self, Context},
};
```

with:

```rust
    setup,
    system_service::{self, Context},
    update,
};
```

In `src/cli.rs`, replace:

```rust
    /// Run `watch` in the background (launchd on macOS, systemd on Linux).
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
}
```

with:

```rust
    /// Run `watch` in the background (launchd on macOS, systemd on Linux).
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    /// Install the newest stable release from GitHub; needs no config.
    Update(UpdateArg),
}

#[derive(Args)]
struct UpdateArg {
    /// Only report whether an update is available; install nothing.
    #[arg(long)]
    check: bool,
}
```

In `src/cli.rs`, replace:

```rust
        Command::Service { command } => service_command(&cli.config_path()?, command),
    }
```

with:

```rust
        Command::Service { command } => service_command(&cli.config_path()?, command),
        Command::Update(arg) => {
            update::command::run(arg.check, &update::install::EnvHooks).map_err(service_error)
        }
    }
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test --locked --lib -- update::service_files system_service && cargo test --locked --test update_command --test system_service`
Expected: all PASS. The `#[cfg(debug_assertions)]` tests run because `cargo test` builds without `--release`.

- [ ] **Step 8: Document `update`**

Insert this section into `docs/guide.md` directly before the line `## Daily use`, and add `- [Updates](#updates)` to the table of contents after `- [Background service](#background-service)`:

````markdown
## Updates

mailtriage installs new releases of itself from [GitHub Releases](https://github.com/wir-drei-digital/mailtriage/releases). The background service installs a new stable release within about a day and switches to it between passes. `mailtriage update` installs one at once.

### Modes

`updates` in `mailtriage.json` decides what `watch` does:

| Value | `watch` |
| --- | --- |
| `auto` (default) | Checks for a new release about once a day and installs it. |
| `notify` | Checks about once a day and prints an `available` event; installs nothing. |
| `off` | Makes no network call. |

Set it with `mailtriage setup --update --updates notify`, or edit the file. The mode governs only `watch`: `mailtriage update` works in every mode and needs no config.

### `mailtriage update`

```sh
mailtriage update --check --json   # report; changes nothing but the cache
mailtriage update --json           # install the newest stable release
```

Both read the release list of `wir-drei-digital/mailtriage` from the GitHub API, every page, without a token. The candidate is the highest release whose tag is exactly `vX.Y.Z`: drafts, prereleases such as `v0.4.0-rc.1` and other tags are ignored, and GitHub's "Latest" flag is not used. Versions compare by SemVer, so `0.3.0-rc.1 < 0.3.0 < 0.3.1`. A release is installed only when it is newer than the installed binary. mailtriage never downgrades, and a release candidate you installed by hand stays until a higher stable release appears.

`update` replaces the binary that runs it (its path with symlinks resolved). In order, it:

1. checks that this binary may be replaced (see [Binaries mailtriage does not replace](#binaries-mailtriage-does-not-replace));
2. takes the installation lock, `.mailtriage-update.lock` next to the binary, waiting up to 60 seconds for another update;
3. reads the installed version with `<binary> --version`, and stops with `action: current` when the release is not newer;
4. downloads `mailtriage-vX.Y.Z-PLATFORM.tar.gz` and `SHA256SUMS` over HTTPS from `github.com` and GitHub's download hosts only, with at most 10 redirects and 200 MB; `PLATFORM` is `macos-arm64`, `linux-amd64` or `linux-arm64`;
5. checks the archive's SHA-256 against its line in `SHA256SUMS`;
6. unpacks the `mailtriage` executable next to the binary and runs `--version` on it, which must print the release's version. This catches a wrong architecture, a glibc older than the release needs, and macOS refusing to run the binary;
7. checks that the installed binary did not change meanwhile, keeps it as `<binary>.previous`, and moves the new binary into place with a rename. Running processes keep the old file open, so nothing running is disturbed.

When a step before the rename fails, the binary and `<binary>.previous` are unchanged and the temporary files are removed. After the rename the update counts as done; a later problem, such as keeping the backup or recording the update, is a warning. Releases are verified by HTTPS and `SHA256SUMS`, not by signatures. `update` starts and stops nothing: each running `watch` switches to the new binary by itself.

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
| 0 | Updated (also with `warnings`), already current, or `--check` done. |
| 2 | Invalid flags. |
| 3 | GitHub could not be reached or answered with an error, including its rate limit; there is no stable release; the release has no archive for this platform or no `SHA256SUMS` line for it; a checksum mismatch; a bad archive; the new binary did not run; the installed binary changed during the update; the cache directory is unknown (no `HOME`); or, without `--check`, the binary may not be replaced: the message names the reason and its fix. |
| 5 | Another update held the installation lock for 60 seconds (`another update is running`). |

### Binaries mailtriage does not replace

| `install.reason` | When | `install.fix` |
| --- | --- | --- |
| `unsupported_platform` | No release archive is built for this system: only macOS arm64 and Linux amd64 and arm64 with glibc have one. | Build from source, or set `updates` to `off`. |
| `managed_by_homebrew` | The path has a `Cellar` directory followed by `mailtriage`. | `brew upgrade mailtriage` |
| `managed_by_nix` | The path starts with `/nix/store/`. | Update it through nix. |
| `unsafe_permissions` | The binary or its directory is not owned by you, or is writable by group or others. | Install mailtriage into a directory only you own and can write, such as `~/.local/bin`, or set `updates` to `notify`. |
| `not_writable` | You cannot create files in the binary's directory. | Make the directory writable for you, or set `updates` to `notify`. |

A binary installed with `sudo install … /usr/local/bin/mailtriage` belongs to root, so it is `unsafe_permissions`: mailtriage reports new releases for it but does not replace it. For automatic updates, install it as your own user and put `~/.local/bin` on your `PATH`:

```sh
install -d ~/.local/bin
install -m 0755 mailtriage ~/.local/bin/mailtriage
```

Run `mailtriage service install` again after you move the binary.

### Files

| File | Purpose |
| --- | --- |
| `update.json` in `~/Library/Caches/mailtriage` (macOS), or in `$XDG_CACHE_HOME/mailtriage` when `XDG_CACHE_HOME` is an absolute path, else `~/.cache/mailtriage` (Linux) | The last release check and when the next one is due, each config's mode and the last `available` event, and each installed binary's version and last install error. `update.lock` beside it guards it. Deleting it is safe; it is rebuilt. |
| `.mailtriage-update.lock` next to the binary | The installation lock. It is never deleted. |
| `.mailtriage-update-*` next to the binary | Temporary files of an update; the next update removes leftovers. |
| `<binary>.previous` | The binary before the last update. |

Without the cache directory (no `HOME`, and on Linux no absolute `XDG_CACHE_HOME`), `update` exits 3 and `watch` skips its update work.
````

In `docs/development/service-api.md`, add a section after `## \`service\` results`:

```markdown
## `update`

`update::command::run(check_only, &dyn Hooks) -> Result<Value>` is
`mailtriage update [--check]`; it opens no config. Errors are `ServiceError`s:
3 for network, release, archive, smoke-test and replaceability problems, 5
when the installation lock stayed held for 60 s. The JSON results are in the
[guide](guide.md#updates). The code is in `src/update/`: `github` (URL rules,
release list, downloads), `release` (candidate, archive names,
`SHA256SUMS`), `cache` (`update.json`), `schedule`, `check` (one refresh),
`platform` (file identity, replaceability, `--version` probes), `archive`,
`install` (the transaction under the installation lock), `service_files`
(decoding service files) and `command`.
```

- [ ] **Step 9: Full check and commit**

Run: `cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`, then `cargo build && ./target/debug/mailtriage update --help`
Expected: all pass; the help lists `--check` and `--json`. Do not run `update` itself against the real GitHub from the development binary.

```bash
git add src/system_service.rs src/update src/cli.rs tests/update_support/mod.rs tests/update_command.rs docs/guide.md docs/development/service-api.md
git commit -m "Add mailtriage update

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Restarting `watch` onto a replaced binary

**Files:**
- Create: `src/update/events.rs`, `src/update/restart.rs`, `src/update/watch.rs`, `tests/update_restart.rs`
- Modify: `src/update/mod.rs`, `src/cli.rs` (`watch`, `watch_loop`, new `Moment`, `mod tests`), `tests/update_support/mod.rs` (append), `docs/guide.md`

**Interfaces:**
- Consumes: `platform::{installation_path, probe, FileIdentity}`, `install::{Hooks, EnvHooks, NoHooks}`, `version::RUNNING`, `CLI`.
- Produces:
  - `update::events::{error(&str) -> Value, emit(json_mode: bool, &Value), text(&Value) -> String}`.
  - `update::restart::{FIRST_BACKOFF, MAX_BACKOFF, Image { path, identity, replaced_at_start }` (`record() -> Result<Image, String>`), `Failure::{Missing, Probe, Exec}`, `Decision::{Stay, Stopped, Exec(Version), Failed(Failure, String)}`, `Emit = Box<dyn Fn(&Value)>`, `Restarter` (`new(Result<Image, String>, Emit)`, `image(&self) -> Option<&Image>`, `check(&mut self, stopped: &dyn Fn() -> bool, hooks: &dyn Hooks)`, `decide(&mut self, stopped) -> Decision`)}`.
  - `update::watch::WatchUpdates` (`start(json_mode: bool) -> Self`, `before_pass(&mut self, config: &Path, stopped: &dyn Fn() -> bool)`, `while_waiting(&mut self, stopped: &dyn Fn() -> bool)`). Task 6 replaces this file with the full update step behind the same three methods.
  - `cli::watch_loop(stopped, interval_seconds, json_mode, between: impl FnMut(Moment), run_pass)`; `Moment::{BeforePass, Waiting}`.
  - Hook point: `exec` (after the probe and the revalidation, before the `restarting` event and `exec`).
  - Tests: `update_support::{Sandbox::config(name, mode) -> PathBuf, Sandbox::watch(&Server, config) -> Watch, set_updates(path, mode), Watch` (`spawn`, `until`, `event`, `events`, `passes`, `wait_passes`, `stop`, `kill`), `signal(child, name)}`.

`exec` runs no destructors, so the restart check runs only between passes: `watch_loop` calls `between` before `run_pass` and during the wait, when no `Service`, database connection or account lock exists (the pass's `Service` is a temporary dropped at the end of `run_pass`), and the update code holds no lock between its steps. Stdout and stderr are flushed before `exec`. The Rust standard library opens files with `O_CLOEXEC`, so nothing leaks into the new image.

- [ ] **Step 1: Write the failing tests**

Append the `watch` harness to `tests/update_support/mod.rs`. A pass result is a line with `discovered`; `stop` sends SIGTERM through `kill`:

```rust
impl Sandbox {
    /// An offline config (`init`: fake provider, no mail engine) at
    /// `<tmp>/<name>/mailtriage.json` with `updates` set to `mode`.
    pub fn config(&self, name: &str, mode: &str) -> PathBuf {
        let dir = self.root().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mailtriage.json");
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_mailtriage"))
            .args(["init", "--json", "--config"])
            .arg(&path)
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        set_updates(&path, mode);
        path
    }

    /// `watch --account work --interval-seconds 1 --json` for `config`
    /// (given through MAILTRIAGE_CONFIG).
    pub fn watch(&self, server: &Server, config: &Path) -> Watch {
        Watch::spawn(self.command(server).env("MAILTRIAGE_CONFIG", config).args([
            "watch",
            "--account",
            "work",
            "--interval-seconds",
            "1",
            "--json",
        ]))
    }
}

/// Writes `updates` into the config at `path`, keeping everything else.
pub fn set_updates(path: &Path, mode: &str) {
    let mut config: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    config["updates"] = json!(mode);
    std::fs::write(path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
}

/// A running `watch` whose stdout lines are collected as JSON.
pub struct Watch {
    pub child: std::process::Child,
    lines: std::sync::mpsc::Receiver<String>,
    pub seen: Vec<Value>,
}

impl Watch {
    pub fn spawn(command: &mut std::process::Command) -> Self {
        let mut child = command
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, lines) = std::sync::mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            lines,
            seen: Vec::new(),
        }
    }

    /// Collects lines until `done` holds for what was seen; panics after 60 s.
    pub fn until(&mut self, what: &str, done: impl Fn(&[Value]) -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while !done(&self.seen) {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) => self
                    .seen
                    .push(serde_json::from_str(&line).unwrap_or(Value::String(line))),
                Err(_) => panic!("watch never showed {what}; saw {:#?}", self.seen),
            }
        }
    }

    /// Waits for the first event of `kind` and returns it.
    pub fn event(&mut self, kind: &str) -> Value {
        self.until(kind, |seen| {
            seen.iter().any(|v| v["update"]["event"] == kind)
        });
        self.events(kind)[0].clone()
    }

    pub fn events(&self, kind: &str) -> Vec<Value> {
        self.seen
            .iter()
            .filter(|v| v["update"]["event"] == kind)
            .cloned()
            .collect()
    }

    /// Pass results seen so far.
    pub fn passes(&self) -> usize {
        self.seen
            .iter()
            .filter(|v| v.get("discovered").is_some())
            .count()
    }

    pub fn wait_passes(&mut self, count: usize) {
        self.until(&format!("{count} passes"), |seen| {
            seen.iter()
                .filter(|v| v.get("discovered").is_some())
                .count()
                >= count
        });
    }

    /// SIGTERM, then the exit code and the final stop object.
    pub fn stop(mut self) -> (Option<i32>, Value) {
        signal(&self.child, "TERM");
        let status = self.child.wait().unwrap();
        while let Ok(line) = self.lines.recv_timeout(Duration::from_secs(5)) {
            self.seen
                .push(serde_json::from_str(&line).unwrap_or(Value::String(line)));
        }
        (
            status.code(),
            self.seen.last().cloned().unwrap_or(Value::Null),
        )
    }

    /// SIGKILL, as a crash or a killed service would end it.
    pub fn kill(mut self) {
        signal(&self.child, "KILL");
        let _ = self.child.wait();
    }
}

pub fn signal(child: &std::process::Child, name: &str) {
    let status = std::process::Command::new("kill")
        .arg(format!("-{name}"))
        .arg(child.id().to_string())
        .status()
        .unwrap();
    assert!(status.success());
}
```

Create `tests/update_restart.rs`:

```rust
#![cfg(unix)]
//! `watch` re-executes itself onto a replaced binary: the real binary, a
//! script that does not run until `chmod +x`, and a failing `exec`.
mod update_support;
use std::{fs, os::unix::fs::PermissionsExt, thread, time::Duration};
use update_support::{fake_binary, replace_file, run, Sandbox, Server};

const RUNNING: &str = env!("CARGO_PKG_VERSION");

#[test]
fn watch_restarts_onto_a_fresh_copy_and_keeps_its_pid() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    let config = sandbox.config("cfg", "off");
    let mut watch = sandbox.watch(&server, &config);
    watch.wait_passes(2);
    // A fresh copy of the same binary: a new inode at the same path.
    let binary = fs::read(env!("CARGO_BIN_EXE_mailtriage")).unwrap();
    replace_file(&sandbox.bin, &binary, 0o755);
    let restarting = watch.event("restarting");
    assert_eq!(restarting["update"]["pid"], watch.child.id());
    assert_eq!(restarting["update"]["from"], RUNNING);
    assert_eq!(restarting["update"]["to"], RUNNING);
    // The same process keeps passing, with the config from MAILTRIAGE_CONFIG.
    let before = watch.passes();
    watch.wait_passes(before + 2);
    assert!(watch.child.try_wait().unwrap().is_none());
    // Between passes another worker gets the account lock.
    let synced = (0..30).any(|_| {
        let (code, _, _) = run(sandbox
            .command(&server)
            .env("MAILTRIAGE_CONFIG", &config)
            .args(["sync", "--account", "work", "--json"]));
        code == Some(0) || {
            thread::sleep(Duration::from_millis(100));
            false
        }
    });
    assert!(synced, "the account lock stayed held after the restart");
    let (code, last) = watch.stop();
    assert_eq!(code, Some(0));
    assert_eq!(last["watch"]["stopped"], true);
    assert!(server.requests().is_empty());
}

#[test]
fn a_replacement_that_does_not_run_is_reported_once_until_it_changes() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    let config = sandbox.config("cfg", "off");
    let marker = sandbox.root().join("marker");
    let mut watch = sandbox.watch(&server, &config);
    watch.wait_passes(1);
    replace_file(&sandbox.bin, &fake_binary("9.9.9", &marker), 0o644);
    let error = watch.event("error");
    let message = error["update"]["message"].as_str().unwrap();
    assert!(
        message.ends_with("does not run: could not start"),
        "{message}"
    );
    let passes = watch.passes();
    watch.wait_passes(passes + 3);
    assert_eq!(watch.events("error").len(), 1);
    // `chmod +x` changes the file's change time and mode: retried at once.
    fs::set_permissions(&sandbox.bin, fs::Permissions::from_mode(0o755)).unwrap();
    let restarting = watch.event("restarting");
    assert_eq!(restarting["update"]["to"], "9.9.9");
    // The script now runs in watch's place with watch's arguments.
    let status = watch.child.wait().unwrap();
    assert_eq!(status.code(), Some(0));
    let args = fs::read_to_string(&marker).unwrap();
    assert_eq!(
        args.trim(),
        "watch --account work --interval-seconds 1 --json"
    );
}

#[test]
#[cfg(debug_assertions)]
fn a_failed_exec_keeps_the_old_code_running() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    let config = sandbox.config("cfg", "off");
    let hook = sandbox.root().join("hook.sh");
    fs::write(
        &hook,
        format!(
            "#!/bin/sh\n[ \"$1\" = exec ] || exit 0\nrm -f '{}'\n",
            sandbox.bin.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    let mut watch = update_support::Watch::spawn(
        sandbox
            .command(&server)
            .env("MAILTRIAGE_CONFIG", &config)
            .env("MAILTRIAGE_UPDATE_TEST_HOOK", &hook)
            .args([
                "watch",
                "--account",
                "work",
                "--interval-seconds",
                "1",
                "--json",
            ]),
    );
    watch.wait_passes(1);
    replace_file(
        &sandbox.bin,
        &fake_binary("9.9.9", &sandbox.root().join("m")),
        0o755,
    );
    let error = watch.event("error");
    let message = error["update"]["message"].as_str().unwrap();
    assert!(message.starts_with("cannot restart onto "), "{message}");
    let passes = watch.passes();
    watch.wait_passes(passes + 2);
    assert!(watch.child.try_wait().unwrap().is_none());
    let (code, last) = watch.stop();
    assert_eq!(code, Some(0));
    assert_eq!(last["watch"]["stopped"], true);
}
```

Add a unit test to `mod tests` in `src/cli.rs`, before `watch_stops_on_other_errors`, and give the existing `run` helper's `watch_loop` call the new argument (the permitted edit):

In `src/cli.rs`, replace:

```rust
        let result = watch_loop(
            || left.get() == 0,
            0,
            true,
            || {
```

with:

```rust
        let result = watch_loop(
            || left.get() == 0,
            0,
            true,
            |_| {},
            || {
```

In `src/cli.rs`, replace:

```rust
    /// Any other error, a changed account binding included, stops `watch`.
```

with:

```rust
    /// The update hook runs before every pass; it cannot fail or skip one.
    #[test]
    fn the_update_hook_runs_before_each_pass() {
        let moments = std::cell::RefCell::new(Vec::new());
        let left = Cell::new(2);
        let tally = watch_loop(
            || left.get() == 0,
            0,
            true,
            |moment| moments.borrow_mut().push(moment),
            || {
                left.set(left.get() - 1);
                Ok(json!({"partial": false}))
            },
        )
        .unwrap_or_else(|e| panic!("stopped: {} {}", e.code, e.message));
        assert_eq!(*moments.borrow(), [Moment::BeforePass, Moment::BeforePass]);
        assert_eq!(
            tally,
            WatchTally {
                passes: 2,
                partial_passes: 0,
                skipped_passes: 0,
            }
        );
    }

    /// Any other error, a changed account binding included, stops `watch`.
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --locked --bin mailtriage && cargo test --locked --test update_restart`
Expected: compile errors: `Moment` not found, `watch_loop` takes 4 arguments; then `watch` never prints `restarting`.

- [ ] **Step 3: Events (`src/update/events.rs`)**

Create `src/update/events.rs`:

```rust
//! `watch`'s update events: `{"schema_version":1,"update":{"event":…}}`,
//! printed the way `watch` prints passes: one JSON line with `--json`,
//! else one text line.
use serde_json::{json, Value};
use std::io::Write;

/// `{"schema_version":1,"update":{"event":"error","message":…}}`.
pub fn error(message: &str) -> Value {
    json!({"schema_version": 1, "update": {"event": "error", "message": message}})
}

/// Prints `event` on stdout and flushes it.
pub fn emit(json_mode: bool, event: &Value) {
    let line = if json_mode {
        event.to_string()
    } else {
        text(event)
    };
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// The text line of an event.
pub fn text(event: &Value) -> String {
    let u = &event["update"];
    let s = |key: &str| u[key].as_str().unwrap_or("?").to_owned();
    match u["event"].as_str() {
        Some("restarting") => format!(
            "update: restarting onto {} (was {}, pid {})",
            s("to"),
            s("from"),
            u["pid"]
        ),
        Some("available") => {
            let mut line = format!(
                "update: mailtriage {} is available (running {}): {}",
                s("latest"),
                s("current"),
                s("release_url")
            );
            if let Some(fix) = u["install"]["fix"].as_str() {
                line.push_str(&format!("; {fix}"));
            }
            line
        }
        _ => format!("update error: {}", s("message")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_have_one_line_of_text() {
        assert_eq!(text(&error("boom")), "update error: boom");
        let restarting = json!({"schema_version":1,"update":{"event":"restarting","pid":12,"from":"0.2.0","to":"0.3.0"}});
        assert_eq!(
            text(&restarting),
            "update: restarting onto 0.3.0 (was 0.2.0, pid 12)"
        );
        let available = json!({"schema_version":1,"update":{"event":"available","current":"0.2.0","latest":"0.3.0","release_url":"https://x","install":{"reason":"managed_by_homebrew","fix":"run `brew upgrade mailtriage`"}}});
        assert_eq!(
            text(&available),
            "update: mailtriage 0.3.0 is available (running 0.2.0): https://x; run `brew upgrade mailtriage`"
        );
        assert!(!text(&available).contains('\n'));
    }
}
```

- [ ] **Step 4: The restart rule (`src/update/restart.rs`)**

`decide` is the testable part: it probes and revalidates but never `exec`s. A failure is reported once per (identity, kind) and retried after 1 minute, doubling up to 1 hour, or at once for another identity (`chmod +x` changes the mode and the change time). On macOS, `Image::record` also runs `--version` once: another version than the running one means the file was replaced between launch and the recording.

Create `src/update/restart.rs`:

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
    path::PathBuf,
    time::{Duration, Instant},
};

/// The first retry after a failed restart; it doubles up to `MAX_BACKOFF`.
pub const FIRST_BACKOFF: Duration = Duration::from_secs(60);
pub const MAX_BACKOFF: Duration = Duration::from_secs(3600);

/// What `watch` records about itself at start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// The canonical installation path.
    pub path: PathBuf,
    /// The identity of the file this process runs.
    pub identity: FileIdentity,
    /// The file was replaced between launch and the recording: restart
    /// before the first pass.
    pub replaced_at_start: bool,
}

impl Image {
    /// Records the installation path and the image identity. On Linux the
    /// image is `/proc/self/exe`, which follows the loaded file even after
    /// it was replaced. On macOS it is the installation path at start, plus
    /// one `--version` run: another version there means the file was
    /// replaced since launch. `Err` says why the restart rule is off.
    pub fn record() -> Result<Self, String> {
        let path = platform::installation_path()
            .map_err(|e| format!("{e}; restarting onto a replaced binary is off"))?;
        let identity = image_identity(&path).map_err(|e| {
            format!(
                "cannot read {} ({e}); restarting onto a replaced binary is off",
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

#[cfg(target_os = "linux")]
fn image_identity(_path: &std::path::Path) -> std::io::Result<FileIdentity> {
    FileIdentity::read(std::path::Path::new("/proc/self/exe"))
}

#[cfg(not(target_os = "linux"))]
fn image_identity(path: &std::path::Path) -> std::io::Result<FileIdentity> {
    FileIdentity::read(path)
}

/// Why a restart did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Failure {
    Missing,
    Probe,
    Exec,
}

/// What the restart check decided.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// The file is the running image, or a failed restart waits for its retry.
    Stay,
    /// A stop was requested: `watch` exits instead.
    Stopped,
    /// Re-execute the probed file, which printed this version.
    Exec(semver::Version),
    /// The file is missing or does not run.
    Failed(Failure, String),
}

/// Where events go: `events::emit` in `watch`, a collector in tests.
pub type Emit = Box<dyn Fn(&Value)>;

/// The restart rule for one `watch` process.
pub struct Restarter {
    image: Option<Image>,
    emit: Emit,
    /// (identity, kind) pairs already reported, so each is printed once.
    reported: Vec<(Option<FileIdentity>, Failure)>,
    /// The identity whose restart failed, and when to try it again.
    failed: Option<(Option<FileIdentity>, Instant)>,
    backoff: Duration,
}

impl Restarter {
    /// The rule for `image`; without one it is off, and that is reported once.
    pub fn new(image: Result<Image, String>, emit: Emit) -> Self {
        let image = match image {
            Ok(image) => Some(image),
            Err(why) => {
                emit(&events::error(&why));
                None
            }
        };
        Self {
            image,
            emit,
            reported: Vec::new(),
            failed: None,
            backoff: FIRST_BACKOFF,
        }
    }

    pub fn image(&self) -> Option<&Image> {
        self.image.as_ref()
    }

    /// Restarts onto a replaced binary when there is one. Never returns
    /// after a successful `exec`. Call it only between passes: no
    /// `Service`, database connection, account lock or update lock may be
    /// open, because `exec` runs no destructors.
    pub fn check(&mut self, stopped: &dyn Fn() -> bool, hooks: &dyn Hooks) {
        let decision = self.decide(stopped);
        let Some(image) = &self.image else { return };
        let path = image.path.clone();
        match decision {
            Decision::Stay | Decision::Stopped => {}
            Decision::Failed(kind, message) => self.fail(kind, &message),
            Decision::Exec(to) => {
                if let Err(e) = hooks.at("exec") {
                    return self.fail(
                        Failure::Exec,
                        &format!("cannot restart onto {}: {e}", path.display()),
                    );
                }
                (self.emit)(&json!({"schema_version": 1, "update": {
                    "event": "restarting",
                    "pid": std::process::id(),
                    "from": version::RUNNING,
                    "to": to.to_string(),
                }}));
                let _ = std::io::stdout().flush();
                let _ = std::io::stderr().flush();
                let error = exec(&path);
                self.fail(
                    Failure::Exec,
                    &format!("cannot restart onto {}: {error}", path.display()),
                );
            }
        }
    }

    /// The restart decision for the file at the installation path now.
    pub fn decide(&mut self, stopped: &dyn Fn() -> bool) -> Decision {
        let Some(image) = &self.image else {
            return Decision::Stay;
        };
        let current = FileIdentity::read(&image.path).ok();
        if current == Some(image.identity) && !image.replaced_at_start {
            return Decision::Stay;
        }
        if let Some((identity, retry_at)) = self.failed {
            if identity == current && Instant::now() < retry_at {
                return Decision::Stay;
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
        if stopped() {
            return Decision::Stopped;
        }
        Decision::Exec(to)
    }

    /// Reports a failure once per identity and kind and schedules the
    /// retry: 1 min, doubling up to 1 h; at once for another identity.
    fn fail(&mut self, kind: Failure, message: &str) {
        let Some(image) = &self.image else { return };
        let current = FileIdentity::read(&image.path).ok();
        if !self.reported.contains(&(current, kind)) {
            self.reported.push((current, kind));
            (self.emit)(&events::error(message));
        }
        self.backoff = match self.failed {
            Some((identity, _)) if identity == current => (self.backoff * 2).min(MAX_BACKOFF),
            _ => FIRST_BACKOFF,
        };
        self.failed = Some((current, Instant::now() + self.backoff));
    }
}

/// Replaces this process with `path`, keeping `argv` and the environment.
/// Returns only when `exec` fails.
fn exec(path: &std::path::Path) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    let mut args = std::env::args_os();
    let mut command = std::process::Command::new(path);
    if let Some(argv0) = args.next() {
        command.arg0(argv0);
    }
    command.args(args).exec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::install::NoHooks;
    use std::{cell::RefCell, fs, os::unix::fs::PermissionsExt, path::Path, rc::Rc};

    type Seen = Rc<RefCell<Vec<Value>>>;

    fn collector() -> (Emit, Seen) {
        let seen: Seen = Rc::default();
        let sink = Rc::clone(&seen);
        (
            Box::new(move |event| sink.borrow_mut().push(event.clone())),
            seen,
        )
    }

    fn script(path: &Path, version: &str, mode: u32) {
        let tmp = path.with_extension("new");
        fs::write(&tmp, format!("#!/bin/sh\necho 'mailtriage {version}'\n")).unwrap();
        fs::set_permissions(&tmp, fs::Permissions::from_mode(mode)).unwrap();
        fs::rename(&tmp, path).unwrap();
    }

    fn restarter(path: &Path) -> (Restarter, Seen) {
        let image = Image {
            path: path.to_path_buf(),
            identity: FileIdentity::read(path).unwrap(),
            replaced_at_start: false,
        };
        let (emit, seen) = collector();
        (Restarter::new(Ok(image), emit), seen)
    }

    #[test]
    fn an_unchanged_file_stays_and_a_replaced_one_is_probed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mailtriage");
        script(&path, "0.1.0", 0o755);
        let (mut r, _) = restarter(&path);
        assert_eq!(r.decide(&|| false), Decision::Stay);
        script(&path, "9.9.9", 0o755);
        assert_eq!(
            r.decide(&|| false),
            Decision::Exec(semver::Version::new(9, 9, 9))
        );
        assert_eq!(r.decide(&|| true), Decision::Stopped);
    }

    #[test]
    fn a_failed_restart_waits_unless_the_file_changes_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mailtriage");
        script(&path, "0.1.0", 0o755);
        let (mut r, seen) = restarter(&path);
        script(&path, "9.9.9", 0o644);
        assert!(matches!(
            r.decide(&|| false),
            Decision::Failed(Failure::Probe, _)
        ));
        r.check(&|| false, &NoHooks);
        assert_eq!(r.backoff, FIRST_BACKOFF);
        assert_eq!(seen.borrow().len(), 1);
        assert_eq!(seen.borrow()[0]["update"]["event"], "error");
        assert!(seen.borrow()[0]["update"]["message"]
            .as_str()
            .unwrap()
            .ends_with("does not run: could not start"));
        // The same identity waits for its retry.
        assert_eq!(r.decide(&|| false), Decision::Stay);
        // `chmod +x` changes the identity (change time and mode): at once.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            r.decide(&|| false),
            Decision::Exec(semver::Version::new(9, 9, 9))
        );
        fs::remove_file(&path).unwrap();
        assert!(matches!(
            r.decide(&|| false),
            Decision::Failed(Failure::Missing, _)
        ));
    }

    #[test]
    fn retries_back_off_from_a_minute_to_an_hour() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mailtriage");
        script(&path, "0.1.0", 0o755);
        let (mut r, seen) = restarter(&path);
        script(&path, "9.9.9", 0o644);
        let mut waits = Vec::new();
        for _ in 0..8 {
            r.fail(Failure::Probe, "x");
            waits.push(r.backoff.as_secs() / 60);
        }
        assert_eq!(waits, [1, 2, 4, 8, 16, 32, 60, 60]);
        // One event per identity and kind.
        assert_eq!(seen.borrow().len(), 1);
        r.fail(Failure::Exec, "y");
        assert_eq!(seen.borrow().len(), 2);
    }

    #[test]
    fn without_an_image_the_rule_is_off_and_says_so_once() {
        let (emit, seen) = collector();
        let mut r = Restarter::new(Err("no image".into()), emit);
        assert_eq!(r.decide(&|| false), Decision::Stay);
        r.check(&|| false, &NoHooks);
        assert!(r.image().is_none());
        assert_eq!(*seen.borrow(), vec![events::error("no image")]);
    }
}
```

- [ ] **Step 5: The work between passes (`src/update/watch.rs`)**

Create `src/update/watch.rs`:

```rust
//! The update work inside `watch`, between passes: before each pass and
//! every 5 s while waiting, the restart check. Update errors are printed as
//! events and never fail or end a pass.
use super::{
    events,
    install::{EnvHooks, Hooks},
    restart::{Image, Restarter},
};
use std::path::Path;

pub struct WatchUpdates {
    restarter: Restarter,
    hooks: Box<dyn Hooks>,
}

impl WatchUpdates {
    /// Records the installation path and image first, as `watch` must
    /// before anything else.
    pub fn start(json_mode: bool) -> Self {
        let image = Image::record();
        Self {
            restarter: Restarter::new(image, Box::new(move |e| events::emit(json_mode, e))),
            hooks: Box::new(EnvHooks),
        }
    }

    /// The restart check before a pass of the config at `_config`.
    pub fn before_pass(&mut self, _config: &Path, stopped: &dyn Fn() -> bool) {
        self.restarter.check(stopped, &*self.hooks);
    }

    /// The restart check alone, for the wait between passes.
    pub fn while_waiting(&mut self, stopped: &dyn Fn() -> bool) {
        self.restarter.check(stopped, &*self.hooks);
    }
}
```

In `src/update/mod.rs`, add `pub mod events;`, `pub mod restart;` and `pub mod watch;` to the module list, keeping it alphabetical.

- [ ] **Step 6: The hook in `watch` (`src/cli.rs`)**

In `src/cli.rs`, replace:

```rust
fn watch(cli: &Cli, arg: &WatchArg) -> Result<Value, CliError> {
    let path = cli.config_path()?;
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    ctrlc::set_handler(move || {
        flag.store(true, Ordering::SeqCst);
    })
    .map_err(|_| CliError::operational())?;
    let tally = watch_loop(
        || stop.load(Ordering::SeqCst),
        arg.interval_seconds,
        cli.json,
        || Service::open(&path)?.sync(&arg.account, arg.limit),
    )?;
```

with:

```rust
fn watch(cli: &Cli, arg: &WatchArg) -> Result<Value, CliError> {
    // Before anything else: which file this process runs, for the restart rule.
    let mut updates = update::watch::WatchUpdates::start(cli.json);
    let path = cli.config_path()?;
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    ctrlc::set_handler(move || {
        flag.store(true, Ordering::SeqCst);
    })
    .map_err(|_| CliError::operational())?;
    let stopped = || stop.load(Ordering::SeqCst);
    // The update hooks run only between passes, when no `Service` is open.
    let tally = watch_loop(
        stopped,
        arg.interval_seconds,
        cli.json,
        |moment| match moment {
            Moment::BeforePass => updates.before_pass(&path, &stopped),
            Moment::Waiting => updates.while_waiting(&stopped),
        },
        || Service::open(&path)?.sync(&arg.account, arg.limit),
    )?;
```

In `src/cli.rs`, replace:

```rust
/// Runs `run_pass` (a fresh open and one sync) until `stopped`, printing
/// each pass and waiting `interval_seconds` between passes. A pass that
/// failed because `mailtriage.json` or the mail engine's configuration
/// changed mid-pass is skipped: its error object is printed and the next
/// pass reads the current configuration. Any other error, a changed account
/// binding included, ends the loop.
fn watch_loop(
    stopped: impl Fn() -> bool,
    interval_seconds: u64,
    json_mode: bool,
    mut run_pass: impl FnMut() -> anyhow::Result<Value>,
) -> Result<WatchTally, CliError> {
    let mut tally = WatchTally::default();
    while !stopped() {
        match run_pass() {
```

with:

```rust
/// When `watch_loop` calls its `between` hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Moment {
    /// Before each pass.
    BeforePass,
    /// Every 5 s while waiting for the next pass.
    Waiting,
}

/// Runs `run_pass` (a fresh open and one sync) until `stopped`, printing
/// each pass and waiting `interval_seconds` between passes. `between` runs
/// before each pass and every 5 s of the wait; it cannot fail a pass. A
/// pass that failed because `mailtriage.json` or the mail engine's
/// configuration changed mid-pass is skipped: its error object is printed
/// and the next pass reads the current configuration. Any other error, a
/// changed account binding included, ends the loop.
fn watch_loop(
    stopped: impl Fn() -> bool,
    interval_seconds: u64,
    json_mode: bool,
    mut between: impl FnMut(Moment),
    mut run_pass: impl FnMut() -> anyhow::Result<Value>,
) -> Result<WatchTally, CliError> {
    let mut tally = WatchTally::default();
    while !stopped() {
        between(Moment::BeforePass);
        if stopped() {
            break;
        }
        match run_pass() {
```

In `src/cli.rs`, replace:

```rust
        tally.passes += 1;
        for _ in 0..interval_seconds.saturating_mul(5) {
            if stopped() {
                break;
            }
            thread::sleep(Duration::from_millis(200));
        }
```

with:

```rust
        tally.passes += 1;
        for tick in 1..=interval_seconds.saturating_mul(5) {
            if stopped() {
                break;
            }
            thread::sleep(Duration::from_millis(200));
            if tick % 25 == 0 {
                between(Moment::Waiting);
            }
        }
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test --locked --bin mailtriage && cargo test --locked --lib -- update::restart update::events && cargo test --locked --test update_restart`
Expected: all PASS. `watch_restarts_onto_a_fresh_copy_and_keeps_its_pid` re-executes the real binary: the `restarting` event names the child's PID, passes continue on the same pipe, `sync` gets the account lock between passes, and SIGTERM ends it with exit 0.

- [ ] **Step 8: Document the switch**

In `docs/guide.md`, in the list under the `## Background service` table, add after the bullet that starts with `- **Repeating \`install\`**`:

```markdown
- **A replaced binary.** A running service switches to a new binary at the same path by itself, between passes; see [How a running service switches](#how-a-running-service-switches). Moving the binary to another path still needs `service install`.
```

Insert this subsection in the `## Updates` section directly before `### Binaries mailtriage does not replace`:

```markdown
### How a running service switches

`watch` records which file it runs when it starts. Before each pass, and every 5 seconds while it waits, it compares that file with the binary at the same path (device, inode, size, modification and change time, mode). When the binary was replaced, by `mailtriage update`, by another account's service, or by `cargo install` over the same path, `watch`:

1. runs `<binary> --version`, which must print `mailtriage <version>`;
2. checks that the file did not change again meanwhile;
3. prints `{"schema_version":1,"update":{"event":"restarting","pid":1234,"from":"0.2.0","to":"0.3.0"}}`;
4. replaces itself with the new binary, with the same arguments and environment. The process ID stays the same, so launchd and systemd see no change, and the account lock is free while this happens.

It never does this during a pass, and after Ctrl-C or SIGTERM it stops instead. It works in every `updates` mode and needs no `service install`; moving the binary to another path does need `service install`.

When the new binary does not run, or the switch fails, `watch` prints one `{"schema_version":1,"update":{"event":"error","message":"…"}}` per file and kind of failure, keeps running the old code, and tries again after 1 minute, doubling up to 1 hour, or at once when the file changes again (for example after `chmod +x`). On Linux, a process whose binary file was replaced uses its absolute `argv[0]` to find the path; when that does not exist either, `watch` prints one error event and does not switch.

Without `--json`, events are one line of text, for example `update: restarting onto 0.3.0 (was 0.2.0, pid 1234)`.
```

In the same section, step 7 of `update` ends with "each running `watch` switches to the new binary by itself"; link it: replace `` `update` starts and stops nothing: each running `watch` switches to the new binary by itself.`` with `` `update` starts and stops nothing: each running `watch` switches to the new binary by itself (see [How a running service switches](#how-a-running-service-switches)).``

- [ ] **Step 9: Full check and commit**

Run: `cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`
Expected: all pass.

```bash
git add src/update src/cli.rs tests/update_support/mod.rs tests/update_restart.rs docs/guide.md
git commit -m "Restart watch onto a replaced binary

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Checking and installing in the background

**Files:**
- Create: `tests/update_watch.rs`
- Modify: `src/update/watch.rs` (replaced), `docs/guide.md`

**Interfaces:**
- Consumes: Tasks 1 to 5: `config::read_updates_mode`, `UpdateMode`, `Cache`, `ConfigEntry`, `ErrorRecord`, `check::refresh` with `Reservation::Required`, `schedule`, `install::{lock, install, Job, Outcome}`, `platform::{blocker, probe, FileIdentity, Blocker}`, `Restarter`, `Image`, `events`.
- Produces: `WatchUpdates` with the same `start`, `before_pass` and `while_waiting` as Task 5, plus `WatchUpdates::with(image: Result<Image, String>, emit: Rc<dyn Fn(&Value)>, hooks: Box<dyn Hooks>, cache: Option<Cache>, endpoint: Endpoint) -> WatchUpdates` for embedding (the tray's restart rule can reuse `Restarter` the same way). `before_pass` now runs: restart check, update step, and the restart check again after an install.

The update step, per pass:
1. Mode: `config::read_updates_mode(config)`; on success it is stored under the config's key (only when it changed); on failure the stored mode of this config is used, and without one the step ends.
2. `off` ends the step. Otherwise, when `next_check_at` is due, `check::refresh(…, Reservation::Required)`; an error becomes an event.
3. The cached release against the installed version (from `installs[path]` while the file's identity matches, else one `--version` run, recorded).
4. `auto` with a replaceable binary: installs when the release was checked at most 48 hours ago and `installs[path].next_attempt_at` is due; one try of the installation lock; the reservation inside `before_download`; failures recorded with `install_backoff`.
5. Otherwise (`notify`, `auto` without a replaceable or known binary): one `available` event per release and config, with `install` (`reason`, `fix`) when the binary may not be replaced.

- [ ] **Step 1: Write the failing tests**

Create `tests/update_watch.rs`:

```rust
#![cfg(unix)]
//! `watch`'s update step against a loopback server: auto, notify and off,
//! reservations across restarts, stored modes, and contained errors.
mod update_support;
use serde_json::{json, Value};
use std::{fs, path::Path, time::Duration};
use update_support::{
    download_path, fake_binary, release_archive, set_updates, Reply, Sandbox, Server, LIST,
};

const RUNNING: &str = env!("CARGO_PKG_VERSION");

fn original() -> Vec<u8> {
    fs::read(env!("CARGO_BIN_EXE_mailtriage")).unwrap()
}

/// Moves the cache's next check into the past, so the next pass checks.
fn check_now(sandbox: &Sandbox) {
    let mut cache = sandbox.cache();
    cache["next_check_at"] = json!("2000-01-01T00:00:00Z");
    fs::write(sandbox.cache_file(), cache.to_string()).unwrap();
}

#[test]
fn auto_installs_a_newer_release_and_restarts_onto_it() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    let marker = sandbox.root().join("marker");
    server.publish("9.9.9", &release_archive(&fake_binary("9.9.9", &marker)));
    let config = sandbox.config("cfg", "auto");
    let mut watch = sandbox.watch(&server, &config);
    let restarting = watch.event("restarting");
    assert_eq!(restarting["update"]["to"], "9.9.9");
    assert_eq!(restarting["update"]["pid"], watch.child.id());
    assert_eq!(watch.child.wait().unwrap().code(), Some(0));
    assert_eq!(
        fs::read_to_string(&marker).unwrap().trim(),
        "watch --account work --interval-seconds 1 --json"
    );
    assert_eq!(
        fs::read(format!("{}.previous", sandbox.bin.display())).unwrap(),
        original()
    );
    let cache = sandbox.cache();
    let entry = &cache["installs"][sandbox.bin.to_str().unwrap()];
    assert_eq!(entry["version"], "9.9.9");
    assert_eq!(
        (entry["failures"].clone(), entry["next_attempt_at"].clone()),
        (json!(0), Value::Null)
    );
    let config_key = fs::canonicalize(&config).unwrap();
    assert_eq!(
        cache["configs"][config_key.to_str().unwrap()]["mode"],
        "auto"
    );
}

#[test]
fn notify_reports_once_and_changes_nothing() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    server.publish("9.9.9", &release_archive(b"unused"));
    let config = sandbox.config("cfg", "notify");
    let mut watch = sandbox.watch(&server, &config);
    watch.wait_passes(4);
    let available = watch.events("available");
    assert_eq!(available.len(), 1, "{:#?}", watch.seen);
    assert_eq!(
        available[0],
        json!({"schema_version":1,"update":{"event":"available","current":RUNNING,"latest":"9.9.9","release_url":"https://github.com/wir-drei-digital/mailtriage/releases/tag/v9.9.9"}})
    );
    assert_eq!(server.count(LIST), 1);
    assert_eq!(server.count("/download"), 0);
    assert_eq!(fs::read(&sandbox.bin).unwrap(), original());
    let (code, _) = watch.stop();
    assert_eq!(code, Some(0));
}

#[test]
fn off_makes_no_request() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    server.publish("9.9.9", &release_archive(b"unused"));
    let config = sandbox.config("cfg", "off");
    let mut watch = sandbox.watch(&server, &config);
    watch.wait_passes(3);
    watch.stop();
    assert!(server.requests().is_empty());
}

#[test]
fn auto_reports_a_binary_it_may_not_replace() {
    let sandbox = Sandbox::at("opt/homebrew/Cellar/mailtriage/0.1.0/bin");
    let server = Server::start();
    server.publish("9.9.9", &release_archive(b"unused"));
    let config = sandbox.config("cfg", "auto");
    let mut watch = sandbox.watch(&server, &config);
    let available = watch.event("available");
    assert_eq!(
        available["update"]["install"],
        json!({"reason": "managed_by_homebrew", "fix": "run `brew upgrade mailtriage`"})
    );
    watch.wait_passes(2);
    watch.stop();
    assert_eq!(server.count("/download"), 0);
}

#[test]
fn a_failed_check_is_not_retried_after_a_restart() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    server.reply(LIST, Reply::status(500));
    let config = sandbox.config("cfg", "auto");
    let mut watch = sandbox.watch(&server, &config);
    let error = watch.event("error");
    assert_eq!(
        error["update"]["message"],
        "cannot read the release list: GitHub answered 500"
    );
    watch.kill();
    let mut watch = sandbox.watch(&server, &config);
    watch.wait_passes(3);
    watch.stop();
    assert_eq!(server.count(LIST), 1);
    assert_eq!(sandbox.cache()["check_failures"], 1);

    // Killed during the request: the reservation already holds the next one off.
    check_now(&sandbox);
    server.reply(LIST, Reply::status(500).delayed(Duration::from_secs(20)));
    let watch = sandbox.watch(&server, &config);
    server.wait_for(LIST, 2);
    watch.kill();
    let mut watch = sandbox.watch(&server, &config);
    watch.wait_passes(3);
    watch.stop();
    assert_eq!(server.count(LIST), 2);
}

#[test]
fn a_download_killed_midway_is_not_repeated_before_its_time() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    let archive = release_archive(&fake_binary("9.9.9", Path::new("/dev/null")));
    let name = server.publish("9.9.9", &archive);
    let path = download_path("9.9.9", &name);
    server.reply(&path, Reply::ok(archive).delayed(Duration::from_secs(20)));
    let config = sandbox.config("cfg", "auto");
    let watch = sandbox.watch(&server, &config);
    server.wait_for(&path, 1);
    watch.kill();
    let mut watch = sandbox.watch(&server, &config);
    watch.wait_passes(3);
    watch.stop();
    assert_eq!(server.count(&path), 1);
    let entry = &sandbox.cache()["installs"][sandbox.bin.to_str().unwrap()];
    assert!(entry["next_attempt_at"].is_string(), "{entry}");
    assert_eq!(fs::read(&sandbox.bin).unwrap(), original());
}

#[test]
fn a_notify_watcher_does_not_hold_back_an_auto_watcher() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    server.publish(
        "9.9.9",
        &release_archive(&fake_binary("9.9.9", Path::new("/dev/null"))),
    );
    let notify = sandbox.config("a", "notify");
    let auto = sandbox.config("b", "auto");
    let mut watch = sandbox.watch(&server, &notify);
    watch.event("available");
    watch.stop();
    let mut watch = sandbox.watch(&server, &auto);
    let restarting = watch.event("restarting");
    assert_eq!(restarting["update"]["to"], "9.9.9");
    let _ = watch.child.wait();
    // The auto watcher used the release the notify watcher had cached.
    assert_eq!(server.count(LIST), 1);
}

#[test]
fn an_unreadable_config_uses_only_its_own_stored_mode() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    server.publish("9.9.9", &release_archive(b"unused"));
    let config = sandbox.config("cfg", "notify");
    let mut watch = sandbox.watch(&server, &config);
    watch.event("available");
    watch.stop();
    assert_eq!(server.count(LIST), 1);
    // Its updates value turns invalid: the stored notify still checks.
    check_now(&sandbox);
    set_updates(&config, "sometimes");
    let mut watch = sandbox.watch(&server, &config);
    assert_eq!(watch.child.wait().unwrap().code(), Some(2));
    assert_eq!(server.count(LIST), 2);
    // A config with no stored mode makes no request.
    check_now(&sandbox);
    let other = sandbox.config("other", "auto");
    set_updates(&other, "sometimes");
    let mut watch = sandbox.watch(&server, &other);
    assert_eq!(watch.child.wait().unwrap().code(), Some(2));
    assert_eq!(server.count(LIST), 2);
}

#[test]
fn update_errors_never_change_a_pass() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    server.reply(LIST, Reply::status(500));
    let config = sandbox.config("cfg", "notify");
    let mut watch = sandbox.watch(&server, &config);
    watch.event("error");
    watch.wait_passes(3);
    let (code, last) = watch.stop();
    assert_eq!(code, Some(0));
    assert_eq!(last["watch"]["partial_passes"], 0);
    assert_eq!(last["watch"]["skipped_passes"], 0);
    assert_eq!(last["partial"], false);
}

#[test]
fn an_unwritable_cache_stops_network_work_with_one_event() {
    let sandbox = Sandbox::new();
    let server = Server::start();
    server.publish("9.9.9", &release_archive(b"unused"));
    let cache_dir = sandbox.cache_file().parent().unwrap().to_path_buf();
    fs::create_dir_all(cache_dir.parent().unwrap()).unwrap();
    fs::write(&cache_dir, "a file where the cache directory belongs").unwrap();
    let config = sandbox.config("cfg", "auto");
    let mut watch = sandbox.watch(&server, &config);
    watch.wait_passes(3);
    let errors = watch.events("error");
    watch.stop();
    assert!(server.requests().is_empty());
    assert_eq!(errors.len(), 1, "{errors:#?}");
    let message = errors[0]["update"]["message"].as_str().unwrap();
    assert!(
        message.starts_with("cannot write the update cache: "),
        "{message}"
    );
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --locked --test update_watch`
Expected: FAIL after the harness's 60 s wait in the tests that expect an event: Task 5's `watch` prints no `available` or `restarting` event for a newer release. (`off_makes_no_request` passes already: Task 5's `watch` makes no request at all.)

- [ ] **Step 3: The update step**

Replace the whole of `src/update/watch.rs` with:

```rust
//! The update work inside `watch`: before each pass the restart check,
//! then the update step (refresh, install or notify); while waiting
//! between passes, the restart check every 5 s. Update errors are printed
//! as events and never fail or end a pass.
use super::{
    cache::{Cache, ConfigEntry, ErrorRecord},
    check::{self, Reservation},
    events,
    github::{Endpoint, Net},
    install::{self, EnvHooks, Hooks, Job, Outcome},
    platform::{self, Blocker, FileIdentity},
    release::{self, CachedRelease},
    restart::{Image, Restarter},
    schedule, version, CLI,
};
use crate::{config, domain::UpdateMode};
use anyhow::anyhow;
use chrono::{Duration as Age, Utc};
use semver::Version;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

/// The start of every message about a cache that cannot be written; the
/// first such message is printed, later ones are not.
const CACHE_UNWRITABLE: &str = "cannot write the update cache";
/// `auto` installs only from release information at most this old.
const MAX_RELEASE_AGE_HOURS: i64 = 48;

pub struct WatchUpdates {
    restarter: Restarter,
    emit: Rc<dyn Fn(&Value)>,
    hooks: Box<dyn Hooks>,
    /// `None` without `HOME`: no update step.
    cache: Option<Cache>,
    endpoint: Endpoint,
    /// A cache problem was printed.
    cache_reported: bool,
}

impl WatchUpdates {
    /// Records the installation path and image first, as `watch` must
    /// before anything else.
    pub fn start(json_mode: bool) -> Self {
        let image = Image::record();
        let emit: Rc<dyn Fn(&Value)> = Rc::new(move |event| events::emit(json_mode, event));
        Self::with(
            image,
            emit,
            Box::new(EnvHooks),
            Cache::for_user(),
            Endpoint::from_env(),
        )
    }

    pub fn with(
        image: Result<Image, String>,
        emit: Rc<dyn Fn(&Value)>,
        hooks: Box<dyn Hooks>,
        cache: Option<Cache>,
        endpoint: Endpoint,
    ) -> Self {
        let restart_emit = Rc::clone(&emit);
        Self {
            restarter: Restarter::new(image, Box::new(move |event| restart_emit(event))),
            emit,
            hooks,
            cache,
            endpoint,
            cache_reported: false,
        }
    }

    /// The restart check, the update step for the config at `config`, and
    /// the restart check again after an install.
    pub fn before_pass(&mut self, config: &Path, stopped: &dyn Fn() -> bool) {
        self.restarter.check(stopped, &*self.hooks);
        if self.update_step(config) {
            self.restarter.check(stopped, &*self.hooks);
        }
    }

    /// The restart check alone, for the wait between passes.
    pub fn while_waiting(&mut self, stopped: &dyn Fn() -> bool) {
        self.restarter.check(stopped, &*self.hooks);
    }

    /// Returns whether a release was installed.
    fn update_step(&mut self, config: &Path) -> bool {
        let Some(cache) = self.cache.clone() else {
            return false;
        };
        let key = config_key(config);
        let mode = match config::read_updates_mode(config) {
            Ok(mode) => {
                self.store_mode(&cache, &key, mode);
                mode
            }
            // Only this config's own stored mode, never another config's.
            Err(_) => match cache.read().configs.get(&key) {
                Some(entry) => entry.mode,
                None => return false,
            },
        };
        if mode == UpdateMode::Off {
            return false;
        }
        if schedule::due(cache.read().next_check_at.as_deref(), Utc::now()) {
            let refreshed = Net::new(self.endpoint.clone())
                .and_then(|net| check::refresh(&net, &cache, Reservation::Required));
            if let Err(e) = refreshed {
                self.report(e.to_string());
            }
        }
        let file = cache.read();
        let Some(release) = file.release.clone() else {
            return false;
        };
        let Ok(candidate) = Version::parse(&release.version) else {
            return false;
        };
        let path = self.restarter.image().map(|image| image.path.clone());
        let installed = path.as_deref().and_then(|p| installed_version(&cache, p));
        if !version::is_newer(&candidate, &installed.unwrap_or_else(version::running)) {
            return false;
        }
        let blocker = path
            .as_deref()
            .and_then(|p| platform::blocker(p, release::platform()));
        if let (UpdateMode::Auto, Some(path), None) = (mode, &path, blocker) {
            let now = Utc::now();
            let fresh = file
                .checked_at
                .as_deref()
                .and_then(schedule::parse)
                .is_some_and(|at| now - at <= Age::hours(MAX_RELEASE_AGE_HOURS));
            let attempt_due = schedule::due(
                file.installs
                    .get(&key_of(path))
                    .and_then(|e| e.next_attempt_at.as_deref()),
                now,
            );
            return fresh && attempt_due && self.install(&cache, path, &release);
        }
        self.notify(&cache, &key, mode, &release, path.as_deref().zip(blocker));
        false
    }

    /// Stores `mode` for the config `key` when it changed.
    fn store_mode(&mut self, cache: &Cache, key: &str, mode: UpdateMode) {
        if cache.read().configs.get(key).map(|e| e.mode) == Some(mode) {
            return;
        }
        let stored = cache.update(|c| {
            c.configs
                .entry(key.to_owned())
                .and_modify(|e| e.mode = mode)
                .or_insert(ConfigEntry {
                    mode,
                    notified_version: None,
                });
        });
        if let Err(e) = stored {
            self.report(format!("{CACHE_UNWRITABLE}: {e:#}"));
        }
    }

    /// Steps 2 to 10 for the binary at `path`, trying the installation lock
    /// once. Returns whether the release was installed.
    fn install(&mut self, cache: &Cache, path: &Path, release: &CachedRelease) -> bool {
        let key = key_of(path);
        let Some(dir) = path.parent() else {
            return false;
        };
        let lock = match install::lock(dir, Duration::ZERO) {
            Ok(Some(lock)) => lock,
            // Another updater is at work: try again at the next pass.
            Ok(None) => return false,
            Err(e) => {
                self.install_failed(cache, &key, &format!("{e:#}"));
                return false;
            }
        };
        let net = match Net::new(self.endpoint.clone()) {
            Ok(net) => net,
            Err(e) => {
                self.install_failed(cache, &key, &e.to_string());
                return false;
            }
        };
        let running = version::running();
        let job = Job {
            net: &net,
            component: CLI,
            path,
            release,
            fallback: &running,
            cache: Some(cache),
            hooks: &*self.hooks,
        };
        let mut reserve = || {
            cache
                .update(|c| {
                    c.installs.entry(key.clone()).or_default().next_attempt_at =
                        Some(schedule::stamp(schedule::reservation(Utc::now())));
                })
                .map_err(|e| anyhow!("{CACHE_UNWRITABLE}: {e:#}; not installing"))
        };
        let outcome = install::install(&job, &lock, &mut reserve);
        drop(lock);
        match outcome {
            Ok(Outcome::Installed(done)) => {
                for warning in done.warnings {
                    self.report(warning);
                }
                true
            }
            Ok(Outcome::Current { .. }) => false,
            Err(e) => {
                self.install_failed(cache, &key, &format!("{e:#}"));
                false
            }
        }
    }

    /// Records a failed install with its backoff and prints it.
    fn install_failed(&mut self, cache: &Cache, key: &str, message: &str) {
        let now = Utc::now();
        let _ = cache.update(|c| {
            let entry = c.installs.entry(key.to_owned()).or_default();
            entry.failures = entry.failures.saturating_add(1);
            entry.last_error = Some(ErrorRecord {
                at: schedule::stamp(now),
                message: message.to_owned(),
            });
            entry.next_attempt_at = Some(schedule::stamp(schedule::install_backoff(
                now,
                entry.failures,
            )));
        });
        self.report(format!("installing the update failed: {message}"));
    }

    /// The `available` event, once per release and config.
    fn notify(
        &mut self,
        cache: &Cache,
        key: &str,
        mode: UpdateMode,
        release: &CachedRelease,
        not_replaceable: Option<(&Path, Blocker)>,
    ) {
        let notified = cache
            .read()
            .configs
            .get(key)
            .and_then(|e| e.notified_version.clone());
        if notified.as_deref() == Some(release.version.as_str()) {
            return;
        }
        let mut event = json!({"schema_version": 1, "update": {
            "event": "available",
            "current": version::RUNNING,
            "latest": release.version,
            "release_url": release.release_url,
        }});
        if let Some((path, blocker)) = not_replaceable {
            event["update"]["install"] =
                json!({"reason": blocker.reason(), "fix": blocker.fix(path)});
        }
        (self.emit)(&event);
        let stored = cache.update(|c| {
            c.configs
                .entry(key.to_owned())
                .or_insert(ConfigEntry {
                    mode,
                    notified_version: None,
                })
                .notified_version = Some(release.version.clone());
        });
        if let Err(e) = stored {
            self.report(format!("{CACHE_UNWRITABLE}: {e:#}"));
        }
    }

    /// Prints an error event; of the cache problems only the first.
    fn report(&mut self, message: String) {
        if message.starts_with(CACHE_UNWRITABLE) {
            if self.cache_reported {
                return;
            }
            self.cache_reported = true;
        }
        (self.emit)(&events::error(&message));
    }
}

/// The installed version at `path`: `--version` runs only when the file's
/// identity differs from the one recorded with the cached version.
fn installed_version(cache: &Cache, path: &Path) -> Option<Version> {
    let identity = FileIdentity::read(path).ok()?;
    let key = key_of(path);
    if let Some(entry) = cache.read().installs.get(&key) {
        if entry.identity == Some(identity) {
            return entry
                .version
                .as_deref()
                .and_then(|v| Version::parse(v).ok());
        }
    }
    let found = platform::probe(path, CLI).ok();
    let _ = cache.update(|c| {
        let entry = c.installs.entry(key).or_default();
        entry.version = found.as_ref().map(ToString::to_string);
        entry.identity = Some(identity);
    });
    found
}

/// The cache key of a config: its canonical path, else its absolute path.
fn config_key(config: &Path) -> String {
    std::fs::canonicalize(config)
        .or_else(|_| std::path::absolute(config))
        .unwrap_or_else(|_| PathBuf::from(config))
        .to_string_lossy()
        .into_owned()
}

fn key_of(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --locked --test update_watch --test update_restart`
Expected: all PASS.

- [ ] **Step 5: Document the background behaviour**

Insert this subsection in the `## Updates` section of `docs/guide.md` directly before `### How a running service switches`:

```markdown
### In the background

All of this happens in `watch`; no other command checks for updates by itself. Each pass runs, in order, the restart check (see [How a running service switches](#how-a-running-service-switches)), the update step, and then the pass. The update step never fails, ends or changes a pass. Its problems are printed as `{"schema_version":1,"update":{"event":"error","message":"…"}}`, one JSON line with `--json`, else one text line.

The update step:

1. Reads `updates` from the config at every pass. When the file cannot be read or its `updates` is invalid, `watch` uses the mode it last stored for this config; with none, it skips the step.
2. With `off`, it stops there. With `notify` or `auto`, it refreshes the release information when the next check is due, about once a day: 24 hours after a successful check, plus up to an hour. Before the request it records the next check one hour ahead, so a `watch` that crashes or is killed during the request does not ask again sooner. A failed check is retried after 1 hour, doubling up to 24 hours (±10 %), or later when GitHub's `Retry-After` or rate-limit reset says so.
3. With `auto`, when the cached release is newer than the installed binary, was checked at most 48 hours ago, and no earlier attempt waits for its retry, it installs the release as `mailtriage update` does. It tries the installation lock once; when another update holds it, it tries again at the next pass. It records the next attempt one hour ahead before it downloads, and a failed install waits 1 hour, doubling up to 24 hours. After an install, the restart check switches `watch` to the new binary before the pass.
4. With `notify`, or with `auto` when the binary may not be replaced, it prints once per release and config `{"schema_version":1,"update":{"event":"available","current":"0.2.0","latest":"0.3.0","release_url":"…"}}`. For `auto`, the event adds `install` with `reason` and `fix`.

The release information is shared by every `watch` of the same user: a `notify` watcher's check serves an `auto` watcher's install. A cache that cannot be written stops the network work, and `watch` prints one event about it.
```

- [ ] **Step 6: Full check and commit**

Run: `cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`
Expected: all pass.

```bash
git add src/update/watch.rs tests/update_watch.rs docs/guide.md
git commit -m "Check and install updates in watch

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Update state in `service status`, `doctor` and `setup`

**Files:**
- Create: `src/update/report.rs`, `tests/update_status.rs`
- Modify: `src/update/mod.rs`, `src/system_service.rs` (`status_account`), `src/cli.rs` (`Command::Doctor`), `src/setup.rs` (`run` step 9 call, `doctor_step`), `tests/setup.rs` (new test), `tests/cli.rs` and `tests/filing_cli.rs` (`run` helpers, permitted edit), `docs/guide.md`, `docs/agents/index.md`, `docs/development/service-api.md`

**Interfaces:**
- Consumes: `service_files::executable`, `system_service::{is_marked, Manager, Context::unit_path}`, `platform::{installation_path, probe, blocker, Blocker}`, `Cache::for_user`, `version::is_newer`.
- Produces:
  - `update::report::update_block(mode: UpdateMode, unit: Option<(Manager, PathBuf)>) -> Value`: `{mode, executable, installed, latest, available, checked_at, last_error, replaceable, reason}`.
  - `update::report::doctor_block(mode, unit) -> Value`: the same plus `ready`, and `fix` when not ready.
  - `service status`'s object gains `update`; `doctor`'s result gains `update`; setup's `doctor.items` gains `{"check":"update","ready":false,"error":REASON,"fix":FIX}` when `doctor_block` is not ready (setup's `doctor.ready` ignores it).

None of these makes a network call: they read the cache (without creating anything) and run the executable's `--version`.

- [ ] **Step 1: Write the failing tests**

Create `tests/update_status.rs`:

```rust
#![cfg(unix)]
//! The `update` block of `service status` and `doctor`, read from the
//! service file, the executable's `--version` and the cache. No network.
mod common;
mod update_support;
use common::{write_tool, LAUNCHCTL, SYSTEMCTL};
use mailtriage::system_service::{self, Manager, Unit};
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};
use update_support::{cache_dir, run, set_updates, Server};

const RUNNING: &str = env!("CARGO_PKG_VERSION");

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    xdg: PathBuf,
    server: Server,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let (home, xdg, bin) = (root.join("home"), root.join("xdg"), root.join("bin"));
        for d in [&home, &xdg, &bin] {
            fs::create_dir_all(d).unwrap();
        }
        write_tool(&bin, "launchctl", LAUNCHCTL);
        write_tool(&bin, "systemctl", SYSTEMCTL);
        let env = Self {
            _dir: dir,
            root,
            home,
            xdg,
            server: Server::start(),
        };
        let (code, _, _) = run(env.command().args(["init", "--json"]));
        assert_eq!(code, Some(0));
        env
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mailtriage"));
        command
            .current_dir(&self.root)
            .env("HOME", &self.home)
            .env("XDG_CACHE_HOME", &self.xdg)
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("bin").display()),
            )
            .env("MAILTRIAGE_UPDATE_URL", &self.server.base)
            .env_remove("MAILTRIAGE_CONFIG");
        command
    }

    fn status(&self) -> Value {
        let (code, v, stderr) =
            run(self
                .command()
                .args(["service", "status", "--account", "work", "--json"]));
        assert_eq!(code, Some(0), "{v} {stderr}");
        v["service"]["update"].clone()
    }

    fn doctor(&self) -> Value {
        let (code, v, stderr) = run(self
            .command()
            .args(["doctor", "--account", "work", "--json"]));
        assert_eq!(code, Some(0), "{v} {stderr}");
        v
    }

    /// A marked service file for `work` running `exe`.
    fn service_file(&self, exe: &Path) {
        let manager = if cfg!(target_os = "macos") {
            Manager::Launchd
        } else {
            Manager::Systemd
        };
        let unit = Unit {
            account: "work".into(),
            exe: exe.to_path_buf(),
            config: self.root.join("mailtriage.json"),
            interval_seconds: 60,
            limit: 100,
            log_dir: self.root.join("logs"),
            path_env: None,
        };
        let dir = system_service::unit_dir(manager, &self.home);
        fs::create_dir_all(&dir).unwrap();
        let (name, text) = match manager {
            Manager::Launchd => (
                "digital.wirdrei.mailtriage.work.plist".to_owned(),
                system_service::plist(&unit),
            ),
            Manager::Systemd => (
                "mailtriage-work.service".to_owned(),
                system_service::systemd_unit(&unit),
            ),
        };
        fs::write(dir.join(name), text).unwrap();
    }

    /// A script printing `mailtriage VERSION` at `<root>/<relative>/mailtriage`.
    fn executable(&self, relative: &str, version: &str) -> PathBuf {
        let dir = self.root.join(relative);
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        write_tool(
            &dir,
            "mailtriage",
            &format!("#!/bin/sh\necho 'mailtriage {version}'\n"),
        );
        dir.join("mailtriage")
    }

    fn write_cache(&self, cache: &Value) {
        let dir = cache_dir(&self.home, &self.xdg);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("update.json"), cache.to_string()).unwrap();
    }
}

#[test]
fn without_a_service_file_status_describes_the_running_binary() {
    let env = Env::new();
    let u = env.status();
    assert_eq!(u["mode"], "auto");
    assert_eq!(u["executable"], Value::Null);
    assert_eq!(u["installed"], RUNNING);
    // Before the first check.
    assert_eq!(
        (u["latest"].clone(), u["checked_at"].clone()),
        (Value::Null, Value::Null)
    );
    assert_eq!(
        (u["available"].clone(), u["last_error"].clone()),
        (json!(false), Value::Null)
    );
    assert!(u["replaceable"].is_boolean());
    assert!(env.server.requests().is_empty());
}

#[test]
fn status_describes_the_services_executable_and_the_cache() {
    let env = Env::new();
    let exe = env.executable("svc/bin", "0.0.1");
    env.service_file(&exe);
    set_updates(&env.root.join("mailtriage.json"), "notify");
    let release = json!({"version":"9.9.9","release_url":"https://github.com/wir-drei-digital/mailtriage/releases/tag/v9.9.9","published_at":null,"archives":{"mailtriage":null},"sums":null});
    let check_error = json!({"at":"2026-11-03T08:00:00Z","message":"cannot read the release list: GitHub answered 503"});
    let install_error = json!({"at":"2026-11-03T09:00:00Z","message":"checksum mismatch for x"});
    let mut cache = json!({
        "schema_version": 1, "release": release, "checked_at": "2026-11-03T07:00:00Z",
        "next_check_at": null, "check_failures": 1, "last_check_error": check_error,
        "configs": {}, "installs": {exe.to_str().unwrap(): {"version":"0.0.1","at":null,"last_error":install_error,"failures":1,"next_attempt_at":null}}
    });
    env.write_cache(&cache);
    assert_eq!(
        env.status(),
        json!({"mode":"notify","executable":exe,"installed":"0.0.1","latest":"9.9.9","available":true,
               "checked_at":"2026-11-03T07:00:00Z","last_error":install_error,"replaceable":true,"reason":null})
    );
    cache["installs"] = json!({});
    env.write_cache(&cache);
    assert_eq!(env.status()["last_error"], check_error);
    assert!(env.server.requests().is_empty());
}

#[test]
fn doctor_adds_readiness_only_auto_needs() {
    let env = Env::new();
    let exe = env.executable("svc/bin", "0.0.1");
    env.service_file(&exe);
    let d = env.doctor();
    assert_eq!(d["update"]["ready"], true);
    assert!(d["update"].get("fix").is_none());

    let brew = env.executable("brew/Cellar/mailtriage/0.0.1/bin", "0.0.1");
    env.service_file(&brew);
    let d = env.doctor();
    assert_eq!(d["update"]["executable"], brew.to_str().unwrap());
    assert_eq!(
        (
            d["update"]["replaceable"].clone(),
            d["update"]["reason"].clone()
        ),
        (json!(false), json!("managed_by_homebrew"))
    );
    assert_eq!(
        (d["update"]["ready"].clone(), d["update"]["fix"].clone()),
        (json!(false), json!("run `brew upgrade mailtriage`"))
    );
    // The top-level `ready` keeps its meaning.
    assert_eq!(d["ready"], true);

    set_updates(&env.root.join("mailtriage.json"), "notify");
    let d = env.doctor();
    assert_eq!(d["update"]["ready"], true);
    assert_eq!(d["update"]["mode"], "notify");
    assert!(env.server.requests().is_empty());
}
```

Append to `tests/setup.rs`:

```rust
#[test]
fn setup_names_the_update_fix_when_auto_cannot_replace_the_service_binary() {
    let f = Fixture::new();
    write_tool(&f.bin, "launchctl", LAUNCHCTL);
    write_tool(&f.bin, "systemctl", SYSTEMCTL);
    let cellar = f.home.join("brew/Cellar/mailtriage/0.0.1/bin");
    fs::create_dir_all(&cellar).unwrap();
    write_tool(
        &cellar,
        "mailtriage",
        "#!/bin/sh\necho 'mailtriage 0.0.1'\n",
    );
    let unit = mailtriage::system_service::Unit {
        account: "work".into(),
        exe: cellar.join("mailtriage"),
        config: f.config_path(),
        interval_seconds: 60,
        limit: 100,
        log_dir: f.home.join("logs"),
        path_env: None,
    };
    let (manager, name, text) = if cfg!(target_os = "macos") {
        (
            mailtriage::system_service::Manager::Launchd,
            "digital.wirdrei.mailtriage.work.plist",
            mailtriage::system_service::plist(&unit),
        )
    } else {
        (
            mailtriage::system_service::Manager::Systemd,
            "mailtriage-work.service",
            mailtriage::system_service::systemd_unit(&unit),
        )
    };
    let dir = mailtriage::system_service::unit_dir(manager, &f.home);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(name), text).unwrap();

    let (out, v) = f.run_with(&WORK_ENV, "", &[("OPENROUTER_API_KEY", "sk-or-fixture")]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doctor = &v["setup"]["doctor"];
    let item = doctor["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["check"] == "update")
        .cloned()
        .unwrap_or_else(|| panic!("no update item: {doctor}"));
    assert_eq!(
        item,
        json!({"check":"update","ready":false,"error":"managed_by_homebrew","fix":"run `brew upgrade mailtriage`"})
    );
    assert_eq!(doctor["ready"], true, "{doctor}");

    let (out, v) = f.run_with(
        &[&WORK_ENV[..], &["--update", "--updates", "notify"]].concat(),
        "",
        &[("OPENROUTER_API_KEY", "sk-or-fixture")],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let items = v["setup"]["doctor"]["items"].as_array().unwrap();
    assert!(items.iter().all(|i| i["check"] != "update"), "{items:?}");
}
```

`doctor` now reads the update cache and the service directory under `HOME`. Two existing helpers run `doctor` with the real `HOME`; point it at the test directory (permitted edit; the commands keep finding `./mailtriage.json` first):

In `tests/cli.rs`, replace:

```rust
fn run(cwd: &Path, args: &[&str]) -> (Output, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
        .current_dir(cwd)
        .args(args)
        .env_remove("MAILTRIAGE_CONFIG")
```

with:

```rust
fn run(cwd: &Path, args: &[&str]) -> (Output, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
        .current_dir(cwd)
        .args(args)
        .env("HOME", cwd)
        .env("XDG_CACHE_HOME", cwd)
        .env_remove("MAILTRIAGE_CONFIG")
```

In `tests/filing_cli.rs`, replace:

```rust
fn run(cwd: &Path, args: &[&str]) -> (i32, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
        .current_dir(cwd)
        .args(args)
        .env_remove("MAILTRIAGE_CONFIG")
```

with:

```rust
fn run(cwd: &Path, args: &[&str]) -> (i32, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
        .current_dir(cwd)
        .args(args)
        .env("HOME", cwd)
        .env("XDG_CACHE_HOME", cwd)
        .env_remove("MAILTRIAGE_CONFIG")
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --locked --test update_status && cargo test --locked --test setup update`
Expected: FAIL: `service.update` and `doctor`'s `update` are `null`, and setup has no `update` item.

- [ ] **Step 3: The block (`src/update/report.rs`)**

The executable comes from the account's service file when mailtriage wrote it and it decodes; otherwise the block describes the binary that runs the command, with `executable: null`. `last_error` is that binary's `installs` entry's error, else the last check error.

Create `src/update/report.rs`:

```rust
//! The `update` block of `service status` and `doctor`: the service's
//! executable, what it prints for `--version`, and what the cache says.
//! Reads only; no network.
use super::{
    cache::Cache,
    platform::{self, Blocker},
    release, service_files, version, CLI,
};
use crate::{
    domain::UpdateMode,
    system_service::{self, Manager},
};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// The block for the service whose file is `unit` (manager and path), or,
/// without a decodable service file, for the binary running this command.
pub fn update_block(mode: UpdateMode, unit: Option<(Manager, PathBuf)>) -> Value {
    describe(mode, unit).0
}

/// `doctor`'s block: `update_block` plus `ready`, false only when the mode
/// is `auto` and the executable is not replaceable; then also `fix`.
pub fn doctor_block(mode: UpdateMode, unit: Option<(Manager, PathBuf)>) -> Value {
    let (mut block, blocked) = describe(mode, unit);
    let ready = !(mode == UpdateMode::Auto && block["replaceable"] == false);
    block["ready"] = json!(ready);
    if !ready {
        block["fix"] = json!(match blocked {
            Some((path, blocker)) => blocker.fix(&path),
            None => "set \"updates\" to \"notify\"".to_owned(),
        });
    }
    block
}

/// The block, and the binary and why it may not be replaced.
fn describe(
    mode: UpdateMode,
    unit: Option<(Manager, PathBuf)>,
) -> (Value, Option<(PathBuf, Blocker)>) {
    let executable = unit.and_then(|(manager, path)| decoded_executable(manager, &path));
    let target = match &executable {
        Some(exe) => Some(fs::canonicalize(exe).unwrap_or_else(|_| exe.clone())),
        None => platform::installation_path().ok(),
    };
    let installed = target.as_deref().and_then(|p| platform::probe(p, CLI).ok());
    let file = Cache::for_user().map(|c| c.read()).unwrap_or_default();
    let latest = file.release.as_ref().map(|r| r.version.clone());
    let available = match (latest.as_deref().map(semver::Version::parse), &installed) {
        (Some(Ok(latest)), Some(installed)) => version::is_newer(&latest, installed),
        _ => false,
    };
    let last_error = target
        .as_deref()
        .and_then(|p| file.installs.get(p.to_string_lossy().as_ref()))
        .and_then(|e| e.last_error.clone())
        .or(file.last_check_error);
    let blocker = target
        .as_deref()
        .and_then(|p| platform::blocker(p, release::platform()));
    let block = json!({
        "mode": mode.as_str(),
        "executable": executable,
        "installed": installed.map(|v| v.to_string()),
        "latest": latest,
        "available": available,
        "checked_at": file.checked_at,
        "last_error": last_error,
        "replaceable": target.is_some() && blocker.is_none(),
        "reason": blocker.map(Blocker::reason),
    });
    (block, target.zip(blocker))
}

/// The executable of a service file mailtriage wrote; `None` when there is
/// no such file or it cannot be decoded.
fn decoded_executable(manager: Manager, path: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(path).ok()?;
    if !system_service::is_marked(manager, &text) {
        return None;
    }
    service_files::executable(manager, &text)
}
```

In `src/update/mod.rs`, add `pub mod report;` after `pub mod release;`.

- [ ] **Step 4: Wire it in**

`src/system_service.rs`, at the end of `status_account`:

In `src/system_service.rs`, replace:

```rust
    out["last_pass"] = service.store.heartbeat(account)?.unwrap_or(Value::Null);
    Ok(out)
```

with:

```rust
    out["last_pass"] = service.store.heartbeat(account)?.unwrap_or(Value::Null);
    let unit = ctx.map(|c| (c.manager, c.unit_path(account)));
    out["update"] = crate::update::report::update_block(service.config.updates, unit);
    Ok(out)
```

`src/cli.rs`, `doctor`:

In `src/cli.rs`, replace:

```rust
        Command::Doctor(arg) => open(&cli.config_path()?)?
            .doctor(&arg.account)
            .map_err(service_error),
```

with:

```rust
        Command::Doctor(arg) => {
            let mut service = open(&cli.config_path()?)?;
            let mut report = service.doctor(&arg.account).map_err(service_error)?;
            let unit = Context::detect()
                .ok()
                .map(|ctx| (ctx.manager, ctx.unit_path(&arg.account)));
            report["update"] = update::report::doctor_block(service.config.updates, unit);
            Ok(report)
        }
```

`src/setup.rs`, step 9 gets the whole config, and `doctor_step` adds the item:

In `src/setup.rs`, replace:

```rust
    let doctor = doctor_step(p, &path, shown, &name, &cfg.provider, &engine);
```

with:

```rust
    let doctor = doctor_step(p, &path, shown, &name, &cfg, &engine);
```

In `src/setup.rs`, replace:

```rust
    name: &str,
    provider: &ProviderConfig,
    engine: &HimalayaConfig,
) -> Value {
    let mut items = Vec::new();
```

with:

```rust
    name: &str,
    cfg: &AppConfig,
    engine: &HimalayaConfig,
) -> Value {
    let provider = &cfg.provider;
    let mut items = Vec::new();
```

In `src/setup.rs`, replace:

```rust
    p.say("Checks:");
    for item in &items {
```

with:

```rust
    // Like doctor's top-level `ready`, setup's ignores the update item.
    let ready = items.iter().all(|i| i["ready"] == true);
    let unit = Context::detect()
        .ok()
        .map(|ctx| (ctx.manager, ctx.unit_path(name)));
    let update = crate::update::report::doctor_block(cfg.updates, unit);
    if update["ready"] == false {
        items.push(check_item(
            "update",
            false,
            update["reason"].as_str().map(str::to_owned),
            update["fix"].as_str().unwrap_or_default().to_owned(),
        ));
    }
    p.say("Checks:");
    for item in &items {
```

In `src/setup.rs`, replace:

```rust
    json!({"ready": items.iter().all(|i| i["ready"] == true), "items": items})
```

with:

```rust
    json!({"ready": ready, "items": items})
```

`ProviderConfig` stays imported in `src/setup.rs`; other functions use it.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --locked --test update_status --test setup --test system_service --test cli --test filing_cli`
Expected: all PASS. The existing setup tests keep `doctor.ready: true` whether or not the test machine's `target/debug` directory counts as replaceable, because setup's `ready` ignores the `update` item.

- [ ] **Step 6: Document the block**

In `docs/guide.md`, replace:

```markdown
"pid":4242,"running":true,"unit_path":"/Users/alice/Library/LaunchAgents/digital.wirdrei.mailtriage.work.plist"}}
```

with:

```markdown
"pid":4242,"running":true,"unit_path":"/Users/alice/Library/LaunchAgents/digital.wirdrei.mailtriage.work.plist","update":{"mode":"auto","executable":"/Users/alice/.local/bin/mailtriage","installed":"0.2.0","latest":"0.3.0","available":true,"checked_at":"2026-11-03T08:12:40Z","last_error":null,"replaceable":true,"reason":null}}}
```

In `docs/guide.md`, replace:

```markdown
| `last_pass` | The account's latest `sync` or `watch` pass, from the state database: `finished_at`, `partial`, `exit_code` (0, 4 for partial, or the error's exit code) and `mode` (`off`, `dry_run` or `live`). `null` before the first pass. |
```

with:

```markdown
| `last_pass` | The account's latest `sync` or `watch` pass, from the state database: `finished_at`, `partial`, `exit_code` (0, 4 for partial, or the error's exit code) and `mode` (`off`, `dry_run` or `live`). `null` before the first pass. |
| `update` | The binary the service runs, and whether it is current: `mode` (the config's `updates`); `executable`, decoded from the service file (`null` without one, and then `installed` and `replaceable` describe the binary that runs `service status`); `installed`, its `--version` (`null` when it does not run); `latest`, `available` and `checked_at` from the last release check (`null`, `false` and `null` before the first one); `last_error`, the last failed install of that binary, else the last failed check (`{at, message}` or `null`); `replaceable` and `reason` (see [Binaries mailtriage does not replace](#binaries-mailtriage-does-not-replace)). It reads the update cache and makes no network call. |
```

In `docs/guide.md`, replace:

```markdown
| `live_checks_performed` | `false` | Always `false`. |
```

with:

```markdown
| `live_checks_performed` | `false` | Always `false`. |
| `update.ready` | `true` | `update` is the block [`service status`](#service-commands) shows, plus `ready`: `false` only when `updates` is `auto` and the binary may not be replaced, and then `fix` says what to do. The top-level `ready` ignores it. |
```

In `docs/guide.md`, replace:

```markdown
**Step 9, check.** Setup runs `doctor` for the account. It prints each item (`provider`, `key`, `mail`, and `filing` when filing is on) as `ok`, or as `not ready` with the one command that fixes it.
```

with:

```markdown
**Step 9, check.** Setup runs `doctor` for the account. It prints each item (`provider`, `key`, `mail`, `filing` when filing is on, and `update` when `updates` is `auto` but the binary may not be replaced) as `ok`, or as `not ready` with the one command that fixes it. The `update` item does not make `doctor.ready` false, as in `doctor` itself.
```

Insert this subsection in the `## Updates` section directly before `### How a running service switches`:

```markdown
### Status

`mailtriage service status --account NAME` and `mailtriage doctor --account NAME` include an `update` block for the binary the account's service runs (see [Service commands](#service-commands)); `doctor` adds `ready` and `fix`. Both read the cache and make no network call. `mailtriage update --check --json` asks GitHub now.
```

In `docs/agents/index.md`, add a subsection after `### Health checks` (before `## Reading mail`):

```markdown
### Updates

`setup` writes `updates: auto`, so the background service installs every stable release within about a day and switches to it between passes (see [Updates](guide.md#updates)). On a host where provisioning owns the binary, pass `--updates off` (no network calls) or `--updates notify` (report only) to `setup`.

- `mailtriage update --check --json` asks GitHub now and reports `available` and whether the binary may be replaced (`install.replaceable`, `install.reason`, `install.fix`). It exits 0 either way, and 3 when GitHub cannot be reached.
- `service status --json` carries `service.update`: `installed` for the binary the service runs, `latest`, `available`, `last_error` and `replaceable`. It reads the cache and makes no network call.
- `mailtriage update --json` installs the newest release. Exit 5 means another update is running: try again later. Exit 3 names the cause; report it to the user. Do not restart the service afterwards; it switches by itself.
```

In `docs/development/service-api.md`, replace:

```markdown
  loaded, running, pid, last_exit_status, unit_path, log_paths, last_pass}`.
  `last_pass` is `{finished_at, partial, exit_code, mode}` or `null`.
```

with:

```markdown
  loaded, running, pid, last_exit_status, unit_path, log_paths, last_pass,
  update}`. `last_pass` is `{finished_at, partial, exit_code, mode}` or `null`.
  `update` is `update::report::update_block(mode, Option<(Manager, unit
  path)>)`: `{mode, executable, installed, latest, available, checked_at,
  last_error, replaceable, reason}`. `doctor` adds `update::report::doctor_block`
  (the same plus `ready`, and `fix` when not ready), and setup step 9 adds an
  `update` item when that block is not ready.
```

- [ ] **Step 7: Full check and commit**

Run: `cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`
Expected: all pass.

```bash
git add src/update src/system_service.rs src/cli.rs src/setup.rs tests/update_status.rs tests/setup.rs tests/cli.rs tests/filing_cli.rs docs/guide.md docs/agents/index.md docs/development/service-api.md
git commit -m "Show update state in service status, doctor and setup

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Release workflow and rollout documentation

**Files:**
- Create: `.github/scripts/is_highest_release.py`
- Modify: `.github/workflows/release.yml` (the `publish` job), `docs/development/releases.md`, `docs/guide.md` (`## Install`, end of `## Updates`), `docs/development/verification.md`, `README.md`

**Interfaces:**
- Consumes: the commands and outputs of Tasks 1 to 7, and `last_pass.version` from Task 9 (same branch; documented here once for the rollout).
- Produces: `python3 .github/scripts/is_highest_release.py TAG RELEASES_JSON` exits 0 when `TAG` is a `vX.Y.Z` at least as high as every stable release in the `gh release list --json tagName` output, else 1.

Archive names and layout and `SHA256SUMS` stay exactly as they are.

- [ ] **Step 1: Write the decision script and check it**

Create `.github/scripts/is_highest_release.py`:

```python
#!/usr/bin/env python3
"""Whether a stable release tag may be marked Latest on GitHub.

Usage: is_highest_release.py TAG RELEASES_JSON

RELEASES_JSON is the output of
`gh release list --exclude-drafts --exclude-pre-releases --json tagName`.
Exits 0 when TAG is an exact vX.Y.Z tag at least as high as every
published stable release there, else 1. Other tags in the list are ignored,
as the updater ignores them.
"""
import json
import re
import sys

STABLE = re.compile(r'v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)')


def version(tag):
    match = STABLE.fullmatch(tag)
    return tuple(int(part) for part in match.groups()) if match else None


def main(tag, path):
    mine = version(tag)
    if mine is None:
        return 1
    with open(path) as releases:
        published = [version(r['tagName']) for r in json.load(releases)]
    return 0 if all(mine >= other for other in published if other) else 1


if __name__ == '__main__':
    sys.exit(main(sys.argv[1], sys.argv[2]))
```

Run:

```bash
chmod +x .github/scripts/is_highest_release.py
printf '[{"tagName":"v0.3.0"},{"tagName":"v0.10.0"},{"tagName":"v1.0.0-rc.1"},{"tagName":"nightly"}]' > /tmp/releases.json
for tag in v0.10.0 v0.11.0 v0.9.9 v0.3.0 v1.0.0-rc.2 v01.0.0; do
  python3 .github/scripts/is_highest_release.py "$tag" /tmp/releases.json; echo "$tag $?"
done
printf '[]' > /tmp/none.json; python3 .github/scripts/is_highest_release.py v0.1.0 /tmp/none.json; echo "first $?"
```

Expected:

```text
v0.10.0 0
v0.11.0 0
v0.9.9 1
v0.3.0 1
v1.0.0-rc.2 1
v01.0.0 1
first 0
```

- [ ] **Step 2: One publish at a time, Latest only when highest (`.github/workflows/release.yml`)**

The publish job checks out only `.github/scripts` at the release commit, before it downloads the artifacts (a checkout cleans the workspace).

In `.github/workflows/release.yml`, replace:

```yaml
  publish:
    needs: [prepare, binaries]
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    permissions:
      contents: write
    steps:
      - uses: actions/download-artifact@v8.0.1
```

with:

```yaml
  publish:
    needs: [prepare, binaries]
    runs-on: ubuntu-24.04
    timeout-minutes: 10
    # One publish at a time across all tags: a later one waits for the
    # running one and never cancels it.
    concurrency:
      group: release-publish
      cancel-in-progress: false
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@v7.0.1
        with:
          ref: ${{ needs.prepare.outputs.sha }}
          sparse-checkout: .github/scripts
          persist-credentials: false
      - uses: actions/download-artifact@v8.0.1
```

In `.github/workflows/release.yml`, replace:

```yaml
          if [ "$PRERELEASE" = true ]; then
            gh release edit "$RELEASE_TAG" --draft=false --prerelease --latest=false
          else
            gh release edit "$RELEASE_TAG" --draft=false --latest
          fi
```

with:

```yaml
          if [ "$PRERELEASE" = true ]; then
            gh release edit "$RELEASE_TAG" --draft=false --prerelease --latest=false
          else
            # Latest only when no published stable release is higher.
            gh release list --exclude-drafts --exclude-pre-releases --limit 1000 \
              --json tagName > /tmp/published-releases.json
            if python3 .github/scripts/is_highest_release.py "$RELEASE_TAG" /tmp/published-releases.json; then
              gh release edit "$RELEASE_TAG" --draft=false --latest
            else
              gh release edit "$RELEASE_TAG" --draft=false --latest=false
            fi
          fi
```

Run: `ruby -ryaml -e 'YAML.load_file(ARGV[0]); puts "valid"' .github/workflows/release.yml` (or `actionlint .github/workflows/release.yml` where installed)
Expected: `valid`. The release being published is still a draft when the list is read, so `--exclude-drafts` leaves it out.

- [ ] **Step 3: `docs/development/releases.md`**

Insert this section before `## Publish a version`:

```markdown
## Every stable release is a deployment

Installed copies with `updates: auto`, the default, install every published
stable release within about a day, and their background services switch to it
between passes (see [Updates](guide.md#updates)). Before you push a `vX.Y.Z`
tag:

- Try the build on one machine first with a release candidate tag,
  `vX.Y.Z-rc.N`. It is published as a prerelease, which mailtriage never
  installs automatically; install it there by hand from its archive.
- Every database migration must keep the previous release's running processes
  correct: only new tables, and new columns that are nullable or have
  defaults; no renames, no drops, no changed meanings. During an update,
  processes of the old and the new release use one state database at the same
  time. A release whose migration cannot follow this rule must not be
  published as a stable release.
- A bad release is fixed by a newer one; there is no rollback command (see
  [Rolling back by hand](guide.md#rolling-back-by-hand)).
```

In `docs/development/releases.md`, replace:

```markdown
Tags such as `v0.2.0-rc.1` become prereleases and do not replace Latest.
```

with:

```markdown
Tags such as `v0.2.0-rc.1` become prereleases: they are never marked Latest and
never installed automatically. A stable tag is marked Latest only when it is
higher than every published stable release, so publishing a fix for an older
line does not move Latest back. Releases publish one at a time: the publish
jobs of all tags share one concurrency group, and a later one waits for the
running one. GitHub keeps only one waiting run per group, so when you push
several tags at once, a waiting publish can be cancelled by a newer one; rerun
it as in [Retry a failed release](#retry-a-failed-release).
```

In `docs/development/releases.md`, replace:

```markdown
Download artifacts and releases through authenticated GitHub access while the
repository is private. To verify downloads:
```

with:

```markdown
The repository is public, so installed copies read releases without a token.
To verify downloads by hand:
```

- [ ] **Step 4: `docs/guide.md`**

In `## Install`, the repository is public now and installs should be updatable:

In `docs/guide.md`, replace:

```markdown
Each archive holds the `mailtriage` executable, the README and the license. While the repository is private, download with authenticated access, for example with the GitHub CLI:
```

with:

```markdown
Each archive holds the `mailtriage` executable, the README and the license. For example, with the GitHub CLI:
```

In `docs/guide.md`, replace:

```markdown
The macOS executable is unsigned and not notarized. See the [release guide](releases.md) for how releases are made.
```

with:

```markdown
The macOS executable is unsigned and not notarized. See the [release guide](releases.md) for how releases are made. For automatic updates, install the binary into a directory you own instead of with `sudo`, for example `install -m 0755 mailtriage ~/.local/bin/mailtriage`; see [Updates](#updates).
```

Append these subsections at the end of the `## Updates` section, directly before `## Daily use`:

````markdown
### The first release with automatic updates

Copies older than the release that brought automatic updates cannot update themselves. Once:

1. Install the first release with automatic updates by hand, from its archive or with `cargo install`.
2. Run `mailtriage service install --account NAME` once for every account, so each service runs the new binary. The old processes have no restart rule and would keep running the old code.
3. After the next pass, check that `mailtriage service status --account NAME --json` shows the new version in `last_pass.version`.

From then on, mailtriage updates itself.

### When a release breaks `watch`

A release that fails before `watch` reaches its update step cannot repair itself. Install a newer release by hand:

1. Run `mailtriage update`. It needs no config, so it may work when `watch` does not.
2. If it does not run either, download and check the archive yourself, then move the binary into place:

   ```sh
   VERSION=0.3.1 PLATFORM=macos-arm64   # or linux-amd64, linux-arm64
   base="https://github.com/wir-drei-digital/mailtriage/releases/download/v$VERSION"
   curl -fLO "$base/mailtriage-v$VERSION-$PLATFORM.tar.gz"
   curl -fLO "$base/SHA256SUMS"
   shasum -a 256 --check --ignore-missing SHA256SUMS   # Linux: sha256sum --check --ignore-missing SHA256SUMS
   tar -xzf "mailtriage-v$VERSION-$PLATFORM.tar.gz" mailtriage
   mv mailtriage ~/.local/bin/mailtriage   # the path of your installed binary
   ```

   Running services switch to it before their next pass.

### Rolling back by hand

There is no rollback command; a bad release is normally fixed by a newer one. To go back to the binary before the last update:

1. Set `updates` to `off` in every config (`mailtriage setup --update --updates off`, or edit the file), so the services do not install the newer release again.
2. Stop the services: `mailtriage service uninstall --account NAME` for each account.
3. `mv <binary>.previous <binary>`
4. Start them again: `mailtriage service install --account NAME`.

This works only when the newer release did not migrate the state database. An older binary refuses a newer database (`database schema is newer than this binary`); then roll forward to a fixed release instead.
````

- [ ] **Step 5: `README.md` and `docs/development/verification.md`**

In `README.md`, add one line after the "From source" code block, before `## Get started`:

```markdown
mailtriage keeps itself up to date: the background service installs new releases by itself, and `mailtriage update` installs one now. That needs a binary you own, such as `install -m 0755 mailtriage ~/.local/bin/mailtriage` instead of `sudo install` ([Updates](docs/guide.md#updates)).
```

Append to `docs/development/verification.md`:

```markdown
## Automatic updates on a real machine (human check)

The automated tests use a loopback server instead of GitHub and a script instead of a release binary. Check once on macOS arm64 and once on Linux with two consecutive published releases, the older one installed:

1. Install the older release into `~/.local/bin` from its archive, run `mailtriage service install --account work`, and wait for one pass.
2. Run `mailtriage update --check --json`.
3. Run `mailtriage update --json`.
4. Wait one interval, read the service log, and run `mailtriage service status --account work --json`.
5. Run `mailtriage update --json` again.
6. Copy the older release to `/usr/local/bin/mailtriage` with `sudo install -m 0755` and run `/usr/local/bin/mailtriage update --check --json` and `/usr/local/bin/mailtriage update --json`.

| Check | Result | Evidence | Date |
| --- | --- | --- | --- |
| `--check` reports the newer release as `latest`, `available: true` and `install.replaceable: true` | | | |
| `update` reports `action: updated` and lists the service with `same_binary: true`; `~/.local/bin/mailtriage.previous` is the older binary | | | |
| The download followed GitHub's real redirect to its asset host | | | |
| macOS: the new binary runs (no Gatekeeper dialog, not killed); `xattr ~/.local/bin/mailtriage` shows no `com.apple.quarantine` | | | |
| The log shows `{"schema_version":1,"update":{"event":"restarting",…}}` with the service's PID, then passes; `service status` shows the same `pid` and the new version in `last_pass.version` | | | |
| The second `update` reports `action: current` | | | |
| The root-owned copy: `--check` reports `unsafe_permissions` with its fix; `update` exits 3 and leaves the file unchanged | | | |
```

- [ ] **Step 6: Check and commit**

Run: `cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked` (unchanged code, still green), then `cargo build && for c in update "update --check" setup "service status" doctor watch; do ./target/debug/mailtriage $c --help >/dev/null || echo "BROKEN: $c"; done`
Expected: no `BROKEN` line.

```bash
git add .github/scripts/is_highest_release.py .github/workflows/release.yml docs/development/releases.md docs/guide.md docs/development/verification.md README.md
git commit -m "Publish one release at a time and mark Latest only for the highest

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Phase B (after refile is merged into this branch)

The SDD run stops after Task 8 and reports. The controller merges `feature/refile` into `feature/auto-update` (resolving conflicts there) and resumes the run here.

### Task 9: The version that ran each pass (schema v7)

**Files:**
- Modify: `src/store.rs` (`MIGRATIONS`, new `LATEST`, the guard in `Store::open`, `record_heartbeat`, `heartbeat`, new `mod schema_7`), `tests/heartbeat.rs`, `tests/filing_store.rs` (pins), `tests/update_status.rs` (new test), `docs/guide.md`, `docs/agents/index.md`, `docs/development/service-api.md`

**Interfaces:**
- Consumes: refile's complete v6 migration (the sixth entry of `MIGRATIONS`) and its newer-schema guard at 6; Task 7's `service status`.
- Produces: `pass_heartbeats.version TEXT` (nullable); `Store::record_heartbeat` (same signature) writes `env!("CARGO_PKG_VERSION")`; `Store::heartbeat` returns `{finished_at, partial, exit_code, mode, version}` with `version: null` for rows from before v7; so `service status`'s `last_pass` gains `version`. The tray spec's v8 (`pass_heartbeats.reason`) comes after this.

- [ ] **Step 0: Check the starting point**

Run: `grep -n 'MIGRATIONS: \[(u32, &str); 6\]' src/store.rs && grep -n 'database schema is newer' -B2 src/store.rs && cargo test --locked`
Expected: `MIGRATIONS` has six entries, refile's migration 6 last; the guard refuses versions above 6; every test passes after the merge. Stop and report if not: v7 must never be added without v6 (migrations run by `user_version`, so a skipped v6 would never run).

- [ ] **Step 1: Write the failing tests**

Append to `src/store.rs` (a new test module; `MIGRATIONS` and `migrate` are private, so these tests live beside them and build each older schema from the real migrations):

```rust
#[cfg(test)]
mod schema_7 {
    use super::*;

    /// A database migrated only up to `version`, as an older binary left it.
    fn database_at(version: u32) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let db = Connection::open(&path).unwrap();
        for (to, sql) in MIGRATIONS.iter().filter(|(to, _)| *to <= version) {
            migrate(&db, *to, sql).unwrap();
        }
        (dir, path)
    }

    fn user_version(path: &Path) -> u32 {
        Connection::open(path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap()
    }

    /// The heartbeat statement of the release before schema 7.
    const V6_INSERT: &str = "INSERT INTO pass_heartbeats(account,finished_at,partial,exit_code,mode) VALUES(?,?,?,?,?)
             ON CONFLICT(account) DO UPDATE SET finished_at=excluded.finished_at,partial=excluded.partial,
             exit_code=excluded.exit_code,mode=excluded.mode";

    #[test]
    fn every_older_schema_migrates_to_7() {
        for from in [0, 5, 6] {
            let (_dir, path) = database_at(from);
            Store::open(&path).unwrap();
            assert_eq!(user_version(&path), 7, "from {from}");
        }
    }

    #[test]
    fn a_database_at_8_is_refused() {
        let (_dir, path) = database_at(7);
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", 8)
            .unwrap();
        let error = Store::open(&path).err().unwrap();
        assert_eq!(
            error.to_string(),
            "database schema is newer than this binary"
        );
    }

    #[test]
    fn old_rows_read_as_version_null() {
        let (_dir, path) = database_at(6);
        let old = Connection::open(&path).unwrap();
        old.execute(V6_INSERT, params!["work", now(), false, 0, "off"])
            .unwrap();
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store.heartbeat("work").unwrap().unwrap()["version"],
            Value::Null
        );
        store.record_heartbeat("work", false, 0, "off").unwrap();
        assert_eq!(
            store.heartbeat("work").unwrap().unwrap()["version"],
            env!("CARGO_PKG_VERSION")
        );
    }

    /// Rolling updates: a process of the previous release keeps its open
    /// connection and inserts heartbeats after another one migrated.
    #[test]
    fn a_connection_opened_at_6_still_inserts_after_the_migration() {
        let (_dir, path) = database_at(6);
        let old = Connection::open(&path).unwrap();
        old.execute(V6_INSERT, params!["home", now(), false, 0, "off"])
            .unwrap();
        let store = Store::open(&path).unwrap();
        assert_eq!(user_version(&path), 7);
        old.execute(V6_INSERT, params!["work", now(), true, 4, "live"])
            .unwrap();
        let beat = store.heartbeat("work").unwrap().unwrap();
        assert_eq!(
            (beat["exit_code"].clone(), beat["version"].clone()),
            (json!(4), Value::Null)
        );
    }
}
```

In `tests/heartbeat.rs`, `each_pass_records_a_heartbeat` checks the version on both heartbeats:

In `tests/heartbeat.rs`, replace:

```rust
    assert_eq!(beat["mode"], "dry_run");
```

with:

```rust
    assert_eq!(beat["mode"], "dry_run");
    assert_eq!(beat["version"], env!("CARGO_PKG_VERSION"));
```

In `tests/heartbeat.rs`, replace:

```rust
    assert_eq!(beat["exit_code"], 5);
    assert_eq!(beat["partial"], false);
```

with:

```rust
    assert_eq!(beat["exit_code"], 5);
    assert_eq!(beat["partial"], false);
    assert_eq!(beat["version"], env!("CARGO_PKG_VERSION"));
```

Append to `tests/update_status.rs`:

```rust
/// Phase B: the version that ran the last pass.
#[test]
fn status_shows_the_version_of_the_last_pass() {
    let env = Env::new();
    let (code, _, _) = run(env.command().args(["sync", "--account", "work", "--json"]));
    assert_eq!(code, Some(0));
    let (code, v, stderr) =
        run(env
            .command()
            .args(["service", "status", "--account", "work", "--json"]));
    assert_eq!(code, Some(0), "{v} {stderr}");
    assert_eq!(v["service"]["last_pass"]["version"], RUNNING);
}
```

The permitted edits of tests that pin the latest schema: after refile they expect 6, and the newer-schema test sets 7. Find them with `grep -rn 'schema_version().unwrap(), 6\|assert_eq!(version, 6)\|"user_version", 7' tests/` and move them up by one: the latest schema is 7 (`tests/filing_store.rs` lines that assert `schema_version()`, `tests/heartbeat.rs::schema_5_adds_the_heartbeat_table`), and `tests/filing_store.rs::newer_schema_is_rejected` sets `user_version` 8.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --locked --lib schema_7 && cargo test --locked --test heartbeat --test update_status --test filing_store`
Expected: FAIL: the database stays at 6, `version` is missing from heartbeats and `last_pass`.

- [ ] **Step 3: Migration 7 (`src/store.rs`)**

Change the type of `MIGRATIONS` from `[(u32, &str); 6]` to `[(u32, &str); 7]` and add this entry after refile's `(6, …)` entry:

```rust
    // The version that ran each pass; nullable, so processes of the
    // previous release keep inserting heartbeats (spec "Rolling updates").
    (7, "ALTER TABLE pass_heartbeats ADD COLUMN version TEXT;"),
```

After `MIGRATIONS` (before `fn migrate`), add the guard's constant, and in `Store::open` replace the literal of the newer-schema guard (`if version > 6 {`) with it, so the guard follows the last migration from now on. If refile already introduced such a constant, keep refile's and make sure it is derived from `MIGRATIONS`:

```rust
/// The schema this binary migrates to; a newer database is refused.
const LATEST: u32 = MIGRATIONS[MIGRATIONS.len() - 1].0;
```

```rust
        if version > LATEST {
            bail!("database schema is newer than this binary");
        }
```

Replace `record_heartbeat` and `heartbeat` with:

```rust
    /// Records how the account's latest sync pass ended, and the version
    /// of mailtriage that ran it.
    pub fn record_heartbeat(
        &self,
        account: &str,
        partial: bool,
        exit_code: i32,
        mode: &str,
    ) -> Result<()> {
        self.db.execute(
            "INSERT INTO pass_heartbeats(account,finished_at,partial,exit_code,mode,version) VALUES(?,?,?,?,?,?)
             ON CONFLICT(account) DO UPDATE SET finished_at=excluded.finished_at,partial=excluded.partial,
             exit_code=excluded.exit_code,mode=excluded.mode,version=excluded.version",
            params![account, now(), partial, exit_code, mode, env!("CARGO_PKG_VERSION")],
        )?;
        Ok(())
    }
    /// The latest pass: `{finished_at, partial, exit_code, mode, version}`;
    /// `version` is `null` for rows written before schema 7.
    pub fn heartbeat(&self, account: &str) -> Result<Option<Value>> {
        Ok(self
            .db
            .query_row(
                "SELECT finished_at,partial,exit_code,mode,version FROM pass_heartbeats WHERE account=?",
                [account],
                |r| {
                    Ok(json!({
                        "finished_at": r.get::<_, String>(0)?,
                        "partial": r.get::<_, bool>(1)?,
                        "exit_code": r.get::<_, i64>(2)?,
                        "mode": r.get::<_, String>(3)?,
                        "version": r.get::<_, Option<String>>(4)?,
                    }))
                },
            )
            .optional()?)
    }
```

A process of the previous release keeps using its own five-column statement on a connection it opened before the migration; SQLite re-prepares it after the schema change, and its rows read as `version: null` (or keep the version of the row's previous writer for that account, which only a manual `sync` with the new binary can produce while the old service still runs).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --locked --lib schema_7 && cargo test --locked --test heartbeat --test update_status --test filing_store --test system_service`
Expected: all PASS.

- [ ] **Step 5: Document `last_pass.version`**

In `docs/guide.md`, replace:

```markdown
"last_pass":{"exit_code":0,"finished_at":"2026-10-05T08:00:00.000000+00:00","mode":"dry_run","partial":false}
```

with:

```markdown
"last_pass":{"exit_code":0,"finished_at":"2026-10-05T08:00:00.000000+00:00","mode":"dry_run","partial":false,"version":"0.2.0"}
```

In `docs/guide.md`, replace:

```markdown
`exit_code` (0, 4 for partial, or the error's exit code) and `mode` (`off`, `dry_run` or `live`). `null` before the first pass. |
```

with:

```markdown
`exit_code` (0, 4 for partial, or the error's exit code), `mode` (`off`, `dry_run` or `live`) and `version`, the mailtriage version that ran it (`null` for passes recorded before schema 7). `null` before the first pass. |
```

In `docs/agents/index.md`, replace:

```markdown
  - `mode`: the filing mode of that pass (`off`, `dry_run` or `live`).
```

with:

```markdown
  - `mode`: the filing mode of that pass (`off`, `dry_run` or `live`).
  - `version`: the mailtriage version that ran that pass. After an update it shows the new version once the service has switched.
```

Append to `docs/development/service-api.md`:

```markdown
## Heartbeat version (schema v7)

Migration 7 adds the nullable column `pass_heartbeats.version`.
`Store::record_heartbeat` writes the running version (`CARGO_PKG_VERSION`) on
every heartbeat, error heartbeats included, and `Store::heartbeat` returns it
as `version` (`null` for rows written before v7). The newer-schema guard is
`LATEST`, the last migration's version. Like every migration of a stable
release, it is additive, so a process of the previous release keeps
inserting heartbeats on an open connection after another process migrated.
```

- [ ] **Step 6: Full check and commit**

Run: `cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`
Expected: all pass.

```bash
git add src/store.rs tests/heartbeat.rs tests/filing_store.rs tests/update_status.rs docs/guide.md docs/agents/index.md docs/development/service-api.md
git commit -m "Record the version that ran each pass (schema v7)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Human gate

The automated tests never reach GitHub, never run a real release binary and never run `launchctl` or `systemctl`. Before relying on automatic updates, a person runs "Automatic updates on a real machine" from `docs/development/verification.md` (Task 8) once two consecutive stable releases with the updater exist: on macOS arm64 (the real `bsdtar` archive, Gatekeeper, launchd keeping the PID across the switch) and on Linux (systemd, `/proc/self/exe` after the rename). Until then, publish `vX.Y.Z-rc.N` tags to try builds; prereleases are never installed automatically.

