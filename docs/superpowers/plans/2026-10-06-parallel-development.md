# Parallel development: refile, automatic updates, tray

Date: 2026-10-06

Three features are implemented at the same time, each on its own branch in its own
git worktree, all created from the same `main` commit (after their plans and the
shared foundation below are on `main`):

| Branch | Worktree | Spec | Plan |
| --- | --- | --- | --- |
| `feature/refile` | `.worktrees/refile` | [refile](../specs/2026-10-06-filing-refile-design.md) | `2026-10-06-filing-refile.md` |
| `feature/auto-update` | `.worktrees/auto-update` | [automatic updates](../specs/2026-10-06-auto-update-design.md) | `2026-10-06-auto-update.md` |
| `feature/tray` | `.worktrees/tray` | [tray app](../specs/2026-10-06-tray-design.md) | `2026-10-06-tray.md` |

Merge order into `main`: **refile, then auto-update, then tray.** The controller
merges; a later branch merges `main` after the earlier ones land and resolves
conflicts there.

## Rules every plan follows

1. **Phase A and Phase B.** Phase A tasks are implementable on the branch from the
   shared `main` commit alone. Tasks that need another feature's code are
   **Phase B**, come last in the plan, and are headed
   `Phase B (after <feature> is merged into this branch)`. The SDD run executes
   Phase A, then stops and reports. The controller merges the prerequisite branches
   into this branch and resumes the run for Phase B.
2. **Schema numbers are never renumbered.**
   - Refile: v6, in Phase A.
   - Auto-update: `pass_heartbeats.version` is v7 and needs v6, so the migration
     and every test that names a schema number are Phase B (after refile).
   - Tray: `pass_heartbeats.reason` is v8, Phase B (after refile and auto-update).
3. **Keep shared files easy to merge.**
   - New code goes into new modules: for example `src/update/` for the updater;
     the tray's new service control in its own functions or module, not interleaved
     with existing ones.
   - Edits to shared files (`src/cli.rs`, `src/service.rs`, `src/system_service.rs`,
     `src/store.rs`, `docs/guide.md`, `docs/hermes.md`, `README.md`,
     `.github/workflows/*`, `Cargo.toml`) stay small and additive.
   - Docs get new sections rather than rewrites of existing ones.
4. **Cross-feature outputs come from fixtures until Phase B.**
   - The tray's refile panel reads `filing refile` JSON exactly as the refile spec
     defines it (including `folders`).
   - The tray's health reads `update`, `last_pass.version` and `last_pass.reason`
     exactly as the auto-update and tray specs define them.
   - Until Phase B, tray tests use fixture JSON from a fake `mailtriage` script, so
     no refile or updater code is needed.
5. **Tray Phase B covers:**
   - the updater extension for the `mailtriage-tray` component (`release.archives`,
     exit code 4, tray install, skip and retry);
   - the heartbeat `reason` column (v8);
   - the release-workflow changes that touch the same jobs as auto-update's.

## Shared foundation (already on `main` when the branches start)

The machine-readable error reason, so all three branches use one mechanism:

- `ErrorKind` (src/service.rs) maps to a stable `reason` string, and `ServiceError`
  carries it.
  - Existing kinds and errors gain reasons: `config_changed` (the existing
    `ConfigChanged`, and the engine `ConfigChanged` error in `service_error`);
    `config_busy` (`configuration is being edited`); `account_busy`
    (`an account worker is already running`); `binding_conflict` (the account-binding
    error).
  - New errors pick a kind with `err_kind(code, kind, message)`.
- `CliError` gains `reason: Option<&'static str>`; the JSON error object prints
  `"reason"` only when set. Text output, codes and messages are unchanged.
- Features add their own kinds (`categories_changed`, `service_config_mismatch`,
  `service_config_unknown`, …) as new enum variants on their branches.

## Global constraints for all three plans

- Tests never run setup against the real `HOME`, never run the real `launchctl` or
  `systemctl`, never touch the real keychain, and never write into the real cache or
  `~/.cargo/bin`.
- The OpenRouter key is never written into any file, test fixture or doc recipe; it
  comes only from key stores, `api_key_command` or the environment.
- CI gates: `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`
  (the tray branch moves these to `--workspace`), `cargo test --locked`.
- Every commit message ends with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Nothing is pushed and nothing is merged by the SDD runs; the controller merges.
