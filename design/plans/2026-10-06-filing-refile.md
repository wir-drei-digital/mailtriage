# Refile Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** After categories are added, removed or re-pointed, `mailtriage filing refile` previews which mailtriage-filed mail would follow its new category, `--apply` marks it, and the normal sync passes move the marked mail with every existing filing safeguard.

**Architecture:** Schema v6 gives each placement a one-time refile mark and a *filed home*: the exact occurrence a COPYUID-proven mailtriage move produced, granted in `recover::mark_move_applied` and dropped by every other change of the home. A new module `src/filing/refile/` holds the candidate rules (`rules`), mark upkeep (`upkeep`), refile intent checks (`intents`), the command (`command`) and retired-folder retention and draining (`retired`). The planner gains a refile rule fed by a `RefileInput` that travels beside the unchanged `PlanInput`; `observe::resolve_folders` keeps retained and draining retired folders watched, with the drain state in `folders.drain_until_uid`.

**Tech Stack:** Rust 2021 (stable 1.92 locally), rusqlite 0.37 with bundled SQLite 3.50 (so `ALTER TABLE … DROP COLUMN` works in test downgrades), serde_json, clap 4. No new crate dependencies.

**Spec:** `design/specs/2026-10-06-filing-refile-design.md`. Read it before every task. Where this plan and the spec disagree, the spec wins, except for the rulings under "Decisions this plan adds". The parallel-development rules in `design/plans/2026-10-06-parallel-development.md` bind this plan.

**Phases:** Refile is merged first and its schema is v6, so all seven tasks are **Phase A**: each is implementable on `feature/refile` from the shared `main` commit alone. There is no Phase B.

## Facts from the code this plan relies on

Checked against `main` at `a192a1b` (with the shared error `reason` foundation `2c15ef6`).

- **Struct literals in existing tests.** `FolderRecord` (`tests/filing_store.rs`), `NewIntent` (`tests/filing_store.rs`, `tests/filing_writes.rs`, `tests/filing_location.rs`), `Action::Move` (`tests/filing_store.rs`, `tests/filing_writes.rs`), and `PlanMessage`, `PlanInput`, `FolderView` (the unit tests in `src/filing/planner.rs`) are built field by field. A new field on any of them breaks an existing test, so this plan adds none. `Placement`, `Intent` and `Plan` are never built literally in tests; they gain fields.
- **Placement writes.** Every multi-field placement write goes through `write_placement` in `src/filing/store.rs` (via `save_placement`, `commit_filing`, `correct_with_writes`, `place_with`). The only other change that ends a home is the epoch reset in `src/store.rs::checkpoint`/`capture_rescan_set`, which deletes the folder's occurrences and leaves placements pointing at the old epoch.
- **COPYUID.** `apply::dispatch_moves` stores `intent.target_uid` only when the COPYUID target epoch equals the claim snapshot's `target_epoch`, and `recover::observe_in_target` prefers that UID. "Proven" therefore means: the applied home equals `(intent.target, intent.target_epoch, intent.target_uid)`.
- **Fake classifier** (`src/provider.rs::fake_decision`): the first keyword wins — `invoice`/`receipt`/`payment` → `transactions`, `newsletter` → `newsletters`, `sale`/`discount` → `promotions`, `update` → `updates`, otherwise `correspondence`; a category that is not configured falls back to the catch-all. Action and urgency depend on the text only. Tests therefore change a message's category by changing the configured categories through `categories apply`, which also changes the generation and requeues open mail.
- **Engine scope.** The Himalaya engine and the fake (`enforce_scope`) refuse every folder call outside the sources plus the last `set_watch_scope` list. An unreferenced retired folder is outside it.
- **Harness.** `tests/common/mod.rs` `Harness::new(mode)`: account `work`, source `INBOX`, each category filed into a folder named after it, `correspondence` → `INBOX`; `h.sync()` runs one pass with limit 100; `h.edit` rewrites the config file.
- **Limits.** `src/cli.rs::parse_limit` rejects values outside 1..=500 with clap's exit 2.

## Decisions this plan adds

Rulings on points the spec leaves open. Each says what it costs if wrong.

1. **Filed-home invariant.** `write_placement` stores the given filed home only while it equals the placement's *known* home; any other write stores NULL. The epoch reset clears filed homes in the reset folder (`capture_rescan_set`). So every home change, absence or ambiguity ends the filed home without per-call-site code. An extra copy (a client copy that leaves the home in place) does **not** clear it; rule 5 skips the message while two occurrences are recorded. Cost: one function.
2. **Proof.** Any mailtriage move (automatic, explicit, refile) grants the filed home when the applied home equals the intent's `(target, target_epoch, target_uid)`. The grant does not depend on `desired_rev`; consuming marks does. Cost: one condition.
3. **Migration v6** also requires `location_state = 'known'`, matching ruling 1. Cost: one SQL clause.
4. **No new fields on test-built types** (see Facts). Refile facts travel in `rules::RefileInput` beside `PlanInput`; `Plan.refile_moves` names the moves that consume a mark (the spec's `consumes_refile` on the action); the drain state is read and written through `Store` methods; `NewIntent` is unchanged (only `claim_move_with` writes `consumes_refile`). Cost: a parallel input struct.
5. **Skip precedence** follows the order of the spec's `skipped` list for rules 1–5: `not_filed_by_mailtriage`, `corrected`, `pinned`, `blocked`, `done`, `open_intent`, `explicit_target`, `multiple_copies`. `retired_frozen` replaces `not_filed_by_mailtriage` for mail in a frozen retired folder. Then rule 6 (`waiting`, `incomplete_input`), then in place (not reported), `target_inbox_or_source`, `target_unusable`. A current classification whose category is configured but missing from the folder map (its folder collides with a source) is `target_inbox_or_source`; an unconfigured one is `target_unusable`. Cost: reporting order only.
6. **Preview universe.** Only placements with a *known* home in a category or retired folder record (not a source) are reported. Absent and ambiguous placements are left out. A missing hydration and a home folder that is gone (record `missing`; in a pass, not in LIST) fold into rule 1 (`not_filed_by_mailtriage`). Cost: counts only.
7. **`--category`** filters `candidates` and `skipped` (by the message's effective category); `waiting` and the folders' `waiting` counts are not filtered. Cost: counts only.
8. **Output shape.** Candidate `folder` and `target` are native names; candidates and waiting are ordered by native folder, then home UID; `folders` by native name; `skipped` always carries all twelve keys. `waiting_marked` is always present (0 unless `--folder` without `--category`). Cost: JSON shape.
9. **`--folder` matching** is exact and case-sensitive over folder records that have a `category_id` and are not sources. An exact native match wins; otherwise the configured names must name exactly one folder; several → exit 2 listing their native names (so passing a native name never loops); none (including sources and never-recorded folders) → exit 2. Cost: one function.
10. **`--apply`** marks only placements not yet marked (so `marked` counts new marks and a repeat marks 0), bumps `desired_rev`, writes one `refile_marked` audit event with the counts, and retries a concurrent change five times before exit 5 (`placements changed concurrently; retry`), like `filing backfill --apply`. With filing `off` the preview is the empty shape. Cost: one event kind.
11. **`categories apply` hint** is a string when the account's filing mode is not `off`, `null` otherwise. Cost: one field.
12. **Mark upkeep** clears with a `desired_rev` bump and event `refile_cleared {reason}`; reasons: `done`, `corrected`, `pinned`, `not_filed_by_mailtriage`, `multiple_copies`, `in_place`, `target_inbox_or_source`, `incomplete_input`. It is skipped in a pass without a folder map (resolution failed). It runs in `dry_run` too (local state only). Blocked mail and mail with an explicit target keep their marks. Only an open *refile* intent defers upkeep (spec step 1). Cost: one function.
13. **Home gates.** A refile move needs the same home as an explicit move: a category folder in state `ok` or a retired folder LIST reports, not paused, in its discovery epoch. A paused or unusable home is not a candidate rule: the preview lists the message and its mark waits. Rule 3 counts open *move* intents only. Cost: one condition.
14. **Intent checks.** The claim-time check runs right after the claim transaction, before dispatch; a failure closes the intent `superseded` with `refile_cancelled`. The retry check runs in `retry_or_supersede` after the revision check and before the attempts cap. Cancel reasons: `source_changed`, `target_changed`, `waiting`, `in_place`, or a skip reason. Cost: two call sites.
15. **Retired-folder state machine** in `folders.drain_until_uid`: NULL = frozen (or never retained), 0 = retained in an earlier pass, N > 0 = draining until UID N in the checkpoint's current epoch. Retention is evaluated each pass (rules 1–5). When retention ends, the folder's `UIDNEXT` is recorded, but only when the snapshot's epoch is the discovery epoch (otherwise the next pass, after discovery recorded the reset, snapshots again). Draining ends when the checkpoint reached N−1, no reset rescan runs, no arrival in the folder is pending and no known home in it lost its occurrence unnoticed — and no open intent, revert, pending arrival or rescan set references the folder. A retired folder that is neither retained, draining nor referenced is **frozen**: drain NULL and the filed homes in it cleared, so a frozen folder holds no refile-eligible mail and is never retained or watched again by this feature. A retired folder LIST no longer reports is frozen at once. A failed drain snapshot keeps the folder watched and reports `drain_snapshot_failed:<native>`. Cost: one module.
16. **Service API.** `Service::filing_refile` (preview) and `Service::filing_refile_apply` take `RefileOptions { category, folder, limit }` (re-exported from `service`). Cost: two methods.

## Global Constraints

- Branch `feature/refile` in worktree `.worktrees/refile`, created from the shared `main` commit. Commit at the end of every task; nothing is pushed or merged by the SDD run. Every commit message ends with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- After every task all three pass: `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`. CI runs them on Linux (amd64, arm64) and macOS.
- No new dependencies; `Cargo.toml` and `Cargo.lock` stay unchanged.
- Schema v6 exactly (migration 6), entirely in this plan. The newer-schema guard moves from 5 to 6. Never renumber.
- Keep shared files easy to merge: new code goes into `src/filing/refile/`; edits to `src/cli.rs`, `src/service.rs`, `src/store.rs`, `docs/guide.md`, `docs/agents/index.md` stay small and additive; docs get new sections, not rewrites.
- Existing tests stay unchanged except the permitted pins: `tests/filing_store.rs` lines 44–45 and 963 (`5` → `6`), line 365 (`user_version` `6` → `7`, the newer-schema guard value), `tests/heartbeat.rs` line 46 (`5` → `6`).
- No new field on `FolderRecord`, `NewIntent`, `Action`, `PlanMessage`, `PlanInput` or `FolderView`.
- The classification generation hash must not change (golden test `generation_hash_is_unchanged_by_filing_and_folder_fields`: `6dfd7bf30e6bf1c0b27ee97af991ecf21dc5ac0400044c2f3adbda7f79d37514`).
- JSON responses keep `schema_version: 1` and the error envelope `{"schema_version":1,"error":{"code","message"[,"reason"]}}`.
- mailtriage never renames or deletes server folders; refile never targets `INBOX` or a source folder, never moves done mail, writes no flag of its own, and writes nothing to the mailbox in `off` or `dry_run`. Every existing safeguard (journaled intents, UID MOVE only, COPYUID, epoch checks, quarantine, pauses, binding check) applies unchanged.
- The preview makes no IMAP calls and changes no filing state (it may bring the generation up to date, as `filing plan` does).
- Tests never use the real `HOME`, keychain, `launchctl`/`systemctl` or caches. The OpenRouter key is never written into any file, fixture or doc recipe.
- Error text never contains mail content or credentials.

## Review Focus

Inputs the spec implies but its test list does not exercise, most likely to bite first. Each has a pinned test in the task named.

1. **A personal-namespace account where a configured folder name is also another folder's native name** (for example `Promotions` recorded before the prefix was known, and `INBOX.Promotions` configured as `Promotions`). `--folder Promotions` must resolve the native match and never answer "pass the native name" for a name that already is one. Test: Task 5 `a_native_name_wins_and_a_shared_configured_name_is_refused`.
2. **A pass whose folder resolution failed** (LIST or CAPABILITY error): the folder map has no capabilities and an empty listing, so every home would look gone. Upkeep must clear no mark. Test: Task 4 `a_pass_without_a_folder_map_clears_no_mark`.
3. **The user deletes the old (retired) folder on the server while it still holds marked mail.** The folder freezes at once, its marks are cleared, and the pass reports no errors and attempts no move from it. Test: Task 7 `a_retired_folder_deleted_on_the_server_freezes_and_clears_its_marks`.
4. **A paused home folder** (after an epoch race) or a paused target: the marked message waits with its mark and moves after `filing retry --folder`. Test: Task 4 `a_paused_target_or_home_keeps_the_mark_until_released`.
5. **The user moves a marked message into another category folder in a mail client before the pass runs.** The client's correction wins: no refile MOVE, mark cleared as `corrected`. Test: Task 4 `a_client_correction_after_marking_wins`.

## File Structure

| Path | Task | Responsibility |
| --- | --- | --- |
| `src/store.rs` | 1, 7 | Migration 6, guard 6, epoch reset clears filed homes (1) and resnapshots a drain (7) |
| `src/filing/types.rs` | 1 | `Placement.refile_once`, filed home fields, `Placement::at_filed_home`; `Intent.consumes_refile` |
| `src/filing/store.rs` | 1, 3, 4, 7 | Columns, filed-home invariant, `claim_move_with` (1); `occurrence_counts`, `frozen_folders` (3); `record_for_planning` (4); drain state and freezing (7) |
| `src/filing/recover.rs` | 2, 4 | Filed-home grant and mark consumption on Move applied (2); retry check (4) |
| `src/filing/refile/mod.rs` (new) | 3–7 | Module root; `plan_pass` (step 9's plan) |
| `src/filing/refile/rules.rs` (new) | 3, 4, 7 | Candidate rules, verdicts, refile facts; `input_for` (4); `retained` (7) |
| `src/filing/planner.rs` | 3 | `plan_with_refile`, the refile rule, `Plan.refile_moves` |
| `src/filing/apply.rs` | 3, 4 | Refile claims (3); claim-time check (4) |
| `src/filing/refile/upkeep.rs` (new) | 4 | Mark upkeep |
| `src/filing/refile/intents.rs` (new) | 4 | Refile intent checks |
| `src/filing/inputs.rs` | 4 | `message_input` |
| `src/filing/refile/command.rs` (new) | 5, 6 | Preview, `--folder` matching, status total, hint (5); apply (6) |
| `src/service.rs` | 3, 5, 6 | Plan step and `filing plan` reason (3); `filing_refile`, status fields, hint (5); `filing_refile_apply` (6) |
| `src/cli.rs` | 6 | `filing refile` |
| `src/filing/refile/retired.rs` (new) | 7 | Retention, draining, freezing |
| `src/filing/observe.rs`, `src/filing/done.rs` | 7 | Watch scope and retirement; done inference waits for draining |
| `tests/refile_store.rs` (new) | 1 | Schema v6 at the store level |
| `tests/refile_support/mod.rs` (new) | 2, 5, 6 | Shared refile test setup |
| `tests/refile_filed_home.rs` (new) | 2 | Filed home through passes |
| `tests/refile_planning.rs` (new) | 3 | Refile moves through passes |
| `tests/refile_marks.rs` (new) | 4 | Mark upkeep and intent checks |
| `tests/refile_preview.rs` (new) | 5 | The preview, status and hint |
| `tests/refile_command.rs` (new) | 6 | Apply and the CLI |
| `tests/refile_retired.rs` (new) | 7 | Retention, draining, freezing |
| `docs/development/service-api.md`, `docs/agents/index.md` | 6 | API and agent usage |
| `docs/guide.md`, the spec | 7 | User guide section, spec status |

---

### Task 1: Schema v6, the stored mark and filed home

**Files:**
- Modify: `src/store.rs` (`MIGRATIONS`, the guard in `Store::open`, `capture_rescan_set`)
- Modify: `src/filing/types.rs` (`Placement`, `Intent`)
- Modify: `src/filing/store.rs` (`PLACEMENT_COLUMNS`, `INTENT_COLUMNS`, `row_placement_at`, `row_intent`, `write_placement`, `records_for_planning`, `claim_move`)
- Modify (permitted pins): `tests/filing_store.rs` (lines 44, 45, 365, 963), `tests/heartbeat.rs` (line 46)
- Create: `tests/refile_store.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - Schema v6: `placements.refile_once INTEGER NOT NULL DEFAULT 0`, `placements.filed_home_folder TEXT`, `placements.filed_home_epoch INTEGER`, `placements.filed_home_uid INTEGER`, `filing_intents.consumes_refile INTEGER NOT NULL DEFAULT 0`, `folders.drain_until_uid INTEGER`.
  - `Placement { …, pub refile_once: bool, pub filed_home_folder: Option<String>, pub filed_home_epoch: Option<u64>, pub filed_home_uid: Option<u64> }` and `Placement::at_filed_home(&self) -> bool`.
  - `Intent { …, pub consumes_refile: bool }`.
  - `Store::claim_move_with(&mut self, account: &str, action: &Action, (target_epoch, target_uid_next): (u64, u64), batch: &str, now: &str, consumes_refile: bool) -> Result<Option<i64>>`; `claim_move` delegates with `false`.

- [ ] **Step 1: Write the failing tests**

Edit the permitted pins: in `tests/filing_store.rs` change `5` to `6` in the two assertions of `migration_reaches_v5_and_is_idempotent` (lines 44–45) and in `assert_v2_mail_kept` (line 963), and change `pragma_update(None, "user_version", 6)` to `7` in `newer_schema_is_rejected` (line 365). In `tests/heartbeat.rs` change `assert_eq!(version, 5);` (line 46) to `6`.

Create `tests/refile_store.rs`:

```rust
//! Schema v6 (refile spec "Storage"): the migration, the stored mark and
//! filed home, and the refile flag of a claimed move.
use mailtriage::{
    domain::{MailboxSnapshot, SourceEnvelope},
    filing::{
        planner::{Action, Locator},
        IntentPatch, LocationState, StageOptions,
    },
    normalize,
    store::Store,
};
use std::collections::BTreeMap;

const NOW: &str = "2026-10-06T12:00:00+00:00";

fn store() -> (tempfile::TempDir, Store) {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("db")).unwrap();
    (d, s)
}

fn snap(epoch: u64, next: u64) -> MailboxSnapshot {
    MailboxSnapshot {
        uid_validity: epoch,
        uid_next: next,
    }
}

/// A fingerprinted, hydrated message at `folder`/`epoch`/`uid`, its placement homed there.
fn placed(s: &mut Store, folder: &str, epoch: u64, uid: u64, mid: &str) -> String {
    if s.checkpoint_state("work", folder).unwrap().is_none() {
        s.checkpoint("work", folder, &snap(epoch, uid + 1)).unwrap();
    }
    let env = SourceEnvelope {
        uid,
        subject: "s".into(),
        message_id: Some(format!("<{mid}@t>")),
        internal_date: Some("2026-10-04T10:00:00+00:00".into()),
        size: Some(100),
        ..Default::default()
    };
    let none = BTreeMap::new();
    let opts = StageOptions {
        record_arrivals: false,
        known_targets: &none,
        rescan_filter: None,
    };
    s.stage_with("work", folder, epoch, uid, &[env], "g1", true, &opts)
        .unwrap();
    let id = s.occurrence_at("work", folder, epoch, uid).unwrap().unwrap();
    let raw = format!("Message-ID: <{mid}@t>\r\nSubject: s\r\n\r\nbody {mid}\r\n");
    s.attach("work", &id, &normalize::rfc822(raw.as_bytes(), 1000).unwrap())
        .unwrap();
    assert!(s.ensure_placement("work", &id, &[folder.to_string()]).unwrap());
    id
}

/// Records the placement's current home as its filed home.
fn file_home(s: &mut Store, id: &str) {
    let mut p = s.placement("work", id).unwrap().unwrap();
    p.filed_home_folder = p.home_folder.clone();
    p.filed_home_epoch = p.home_epoch;
    p.filed_home_uid = p.home_uid;
    assert!(s.save_placement(&p, None).unwrap());
}

/// Turns a fresh v6 database back into v5.
const DOWNGRADE_TO_V5: &str = "ALTER TABLE placements DROP COLUMN refile_once;
ALTER TABLE placements DROP COLUMN filed_home_folder;
ALTER TABLE placements DROP COLUMN filed_home_epoch;
ALTER TABLE placements DROP COLUMN filed_home_uid;
ALTER TABLE filing_intents DROP COLUMN consumes_refile;
ALTER TABLE folders DROP COLUMN drain_until_uid;
PRAGMA user_version=5;";

/// One placement per migration case, with its move intents (ids ascend in
/// insertion order, so the last applied move of a message is its newest).
const V5_ROWS: &str = "INSERT INTO accounts VALUES('work','id','g1');
INSERT INTO folders(account,native,category_id,state) VALUES('work','News','news','ok'),('work','Old','old','retired');
INSERT INTO messages(id,account,envelope,status,observed_at,generation) VALUES
 ('proven','work','{}','ready','2026-10-01T00:00:00+00:00','g1'),
 ('later_failure','work','{}','ready','2026-10-01T00:00:00+00:00','g1'),
 ('no_copyuid','work','{}','ready','2026-10-01T00:00:00+00:00','g1'),
 ('newest_unproven','work','{}','ready','2026-10-01T00:00:00+00:00','g1'),
 ('moved_since','work','{}','ready','2026-10-01T00:00:00+00:00','g1'),
 ('retired','work','{}','ready','2026-10-01T00:00:00+00:00','g1'),
 ('no_uid','work','{}','ready','2026-10-01T00:00:00+00:00','g1'),
 ('no_target_epoch','work','{}','ready','2026-10-01T00:00:00+00:00','g1'),
 ('absent','work','{}','ready','2026-10-01T00:00:00+00:00','g1');
INSERT INTO placements(account,message_id,source_folder,home_folder,home_epoch,home_uid,location_state) VALUES
 ('work','proven','INBOX','News',7,3,'known'),
 ('work','later_failure','INBOX','News',7,10,'known'),
 ('work','no_copyuid','INBOX','News',7,4,'known'),
 ('work','newest_unproven','INBOX','News',7,5,'known'),
 ('work','moved_since','INBOX','News',7,9,'known'),
 ('work','retired','INBOX','Old',8,2,'known'),
 ('work','no_uid','INBOX','News',7,NULL,'known'),
 ('work','no_target_epoch','INBOX','News',7,11,'known'),
 ('work','absent','INBOX','News',7,12,'absent');
INSERT INTO filing_intents(account,message_id,kind,folder,epoch,uid,target,target_epoch,target_uid,state,created_at,updated_at) VALUES
 ('work','proven','move','INBOX',1,1,'News',7,3,'applied','t','t'),
 ('work','proven','flag','News',7,3,NULL,NULL,NULL,'applied','t','t'),
 ('work','later_failure','move','INBOX',1,2,'News',7,10,'applied','t','t'),
 ('work','later_failure','move','News',7,10,'Other',4,NULL,'failed','t','t'),
 ('work','no_copyuid','move','INBOX',1,3,'News',7,NULL,'applied','t','t'),
 ('work','newest_unproven','move','INBOX',1,4,'News',7,5,'applied','t','t'),
 ('work','newest_unproven','move','INBOX',1,4,'News',7,NULL,'applied','t','t'),
 ('work','moved_since','move','INBOX',1,5,'News',7,6,'applied','t','t'),
 ('work','retired','move','INBOX',1,6,'Old',8,2,'applied','t','t'),
 ('work','no_uid','move','INBOX',1,7,'News',7,8,'applied','t','t'),
 ('work','no_target_epoch','move','INBOX',1,8,'News',NULL,11,'applied','t','t'),
 ('work','absent','move','INBOX',1,9,'News',7,12,'applied','t','t');";

#[test]
fn a_fresh_database_is_at_v6() {
    let (_d, s) = store();
    assert_eq!(s.schema_version().unwrap(), 6);
}

#[test]
fn migration_v6_fills_filed_homes_only_from_a_proven_newest_move() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("db");
    drop(Store::open(&path).unwrap());
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(DOWNGRADE_TO_V5).unwrap();
    db.execute_batch(V5_ROWS).unwrap();
    drop(db);
    let s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().unwrap(), 6);
    let filed = |id: &str| {
        let p = s.placement("work", id).unwrap().unwrap();
        (p.filed_home_folder, p.filed_home_epoch, p.filed_home_uid)
    };
    assert_eq!(filed("proven"), (Some("News".to_string()), Some(7), Some(3)));
    assert_eq!(
        filed("later_failure"),
        (Some("News".to_string()), Some(7), Some(10)),
        "the newest applied move counts, not a later failed one"
    );
    for id in [
        "no_copyuid",
        "newest_unproven",
        "moved_since",
        "retired",
        "no_uid",
        "no_target_epoch",
        "absent",
    ] {
        assert_eq!(filed(id), (None, None, None), "{id}");
    }
    assert!(s.placements("work").unwrap().iter().all(|p| !p.refile_once));
    assert!(s
        .intents("work", false)
        .unwrap()
        .iter()
        .all(|i| !i.consumes_refile));
    let drains: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM folders WHERE drain_until_uid IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(drains, 0, "every retired folder starts frozen");
}

#[test]
fn the_filed_home_is_stored_only_while_it_is_the_known_home() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    let id = placed(&mut s, "News", 3, 7, "a");
    let fresh = s.placement("work", &id).unwrap().unwrap();
    assert!(!fresh.refile_once);
    assert_eq!(fresh.filed_home_folder, None);
    assert!(!fresh.at_filed_home());
    let mut p = fresh.clone();
    p.refile_once = true;
    p.filed_home_folder = Some("News".into());
    p.filed_home_epoch = Some(3);
    p.filed_home_uid = Some(7);
    assert!(s.save_placement(&p, None).unwrap());
    let stored = s.placement("work", &id).unwrap().unwrap();
    assert_eq!(stored, p);
    assert!(stored.at_filed_home());

    let mut elsewhere = stored.clone();
    elsewhere.home_uid = Some(8);
    assert!(s.save_placement(&elsewhere, None).unwrap());
    let stored = s.placement("work", &id).unwrap().unwrap();
    assert_eq!(
        (
            stored.filed_home_folder.as_deref(),
            stored.filed_home_epoch,
            stored.filed_home_uid
        ),
        (None, None, None),
        "another home never carries the filed home along"
    );
    assert!(stored.refile_once, "the mark is written as given");

    let mut absent = p.clone();
    absent.location_state = LocationState::Absent;
    assert!(s.save_placement(&absent, None).unwrap());
    assert_eq!(
        s.placement("work", &id).unwrap().unwrap().filed_home_folder,
        None,
        "only a known home is a filed home"
    );
}

#[test]
fn an_epoch_reset_clears_the_filed_homes_in_that_folder_only() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    let a = placed(&mut s, "News", 3, 7, "a");
    let b = placed(&mut s, "Other", 5, 2, "b");
    file_home(&mut s, &a);
    file_home(&mut s, &b);
    s.checkpoint("work", "News", &snap(4, 9)).unwrap();
    assert_eq!(s.placement("work", &a).unwrap().unwrap().filed_home_folder, None);
    assert_eq!(
        s.placement("work", &b)
            .unwrap()
            .unwrap()
            .filed_home_folder
            .as_deref(),
        Some("Other")
    );
}

#[test]
fn a_refile_claim_records_that_it_consumes_the_mark() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    let id = placed(&mut s, "Other", 5, 1, "a");
    let action = Action::Move {
        message_id: id.clone(),
        from: Locator {
            folder: "Other".into(),
            epoch: 5,
            uid: 1,
        },
        to: "Updates".into(),
        desired_rev: 0,
        consumes_eligible: false,
    };
    let refile = s
        .claim_move_with("work", &action, (9, 1), "b1", NOW, true)
        .unwrap()
        .unwrap();
    let intent = s.intent(refile).unwrap().unwrap();
    assert!(intent.consumes_refile);
    assert_eq!((intent.target_epoch, intent.target_uid_next), (Some(9), Some(1)));
    s.update_intent(refile, "superseded", IntentPatch::default(), NOW)
        .unwrap();
    let plain = s
        .claim_move("work", &action, 9, 1, "b2", NOW)
        .unwrap()
        .unwrap();
    assert!(!s.intent(plain).unwrap().unwrap().consumes_refile);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --locked --test refile_store`
Expected: compile errors — no field `refile_once` / `filed_home_folder` on `Placement`, no field `consumes_refile` on `Intent`, no method `at_filed_home`, no method `claim_move_with`.

- [ ] **Step 3: Migration 6 and the guard (`src/store.rs`)**

Change `const MIGRATIONS: [(u32, &str); 5]` to `[(u32, &str); 6]` and append after the `5` entry:

```rust
    (
        6,
        "ALTER TABLE placements ADD COLUMN refile_once INTEGER NOT NULL DEFAULT 0;
ALTER TABLE placements ADD COLUMN filed_home_folder TEXT;
ALTER TABLE placements ADD COLUMN filed_home_epoch INTEGER;
ALTER TABLE placements ADD COLUMN filed_home_uid INTEGER;
ALTER TABLE filing_intents ADD COLUMN consumes_refile INTEGER NOT NULL DEFAULT 0;
ALTER TABLE folders ADD COLUMN drain_until_uid INTEGER;
UPDATE placements SET filed_home_folder=home_folder,filed_home_epoch=home_epoch,filed_home_uid=home_uid
 WHERE location_state='known' AND home_folder IS NOT NULL AND home_epoch IS NOT NULL AND home_uid IS NOT NULL
 AND NOT EXISTS(SELECT 1 FROM folders f WHERE f.account=placements.account AND f.native=placements.home_folder AND f.state='retired')
 AND EXISTS(SELECT 1 FROM filing_intents i WHERE i.id=(SELECT MAX(n.id) FROM filing_intents n
   WHERE n.account=placements.account AND n.message_id=placements.message_id AND n.kind='move' AND n.state='applied')
  AND i.target_uid IS NOT NULL AND i.target=placements.home_folder AND i.target_epoch=placements.home_epoch AND i.target_uid=placements.home_uid);",
    ),
```

In `Store::open` change `if version > 5 {` to `if version > 6 {`.

In `capture_rescan_set`, after the `for sql in [...] { ... }` loop, add:

```rust
    // Refile spec "Filed home": a UIDVALIDITY change ends every filed home in
    // the folder; rediscovering the message never restores one.
    tx.execute(
        "UPDATE placements SET filed_home_folder=NULL,filed_home_epoch=NULL,filed_home_uid=NULL WHERE account=?1 AND filed_home_folder=?2",
        params![account, mailbox],
    )?;
```

- [ ] **Step 4: The new fields (`src/filing/types.rs`)**

At the end of `pub struct Placement` (after `blocked_reason`):

```rust
    /// Refile spec: set by `filing refile --apply`, consumed by the refile
    /// move, cleared by mark upkeep.
    pub refile_once: bool,
    /// Refile spec "Filed home": the occurrence a COPYUID-proven mailtriage
    /// move produced. Stored only while it is the known home.
    pub filed_home_folder: Option<String>,
    pub filed_home_epoch: Option<u64>,
    pub filed_home_uid: Option<u64>,
```

After the struct:

```rust
impl Placement {
    /// Refile spec "Filed home": the location is known and the home is
    /// exactly the occurrence a proven mailtriage move produced.
    pub fn at_filed_home(&self) -> bool {
        self.location_state == LocationState::Known
            && self.filed_home_folder.is_some()
            && self.home_folder == self.filed_home_folder
            && self.home_epoch.is_some()
            && self.home_epoch == self.filed_home_epoch
            && self.home_uid.is_some()
            && self.home_uid == self.filed_home_uid
    }
}
```

At the end of `pub struct Intent` (after `race_until_uid`):

```rust
    /// A refile move: applying it with a current `desired_rev` clears the
    /// placement's refile mark.
    pub consumes_refile: bool,
```

- [ ] **Step 5: Store columns, the invariant and refile claims (`src/filing/store.rs`)**

Append `,refile_once,filed_home_folder,filed_home_epoch,filed_home_uid` to `PLACEMENT_COLUMNS` and `,consumes_refile` to `INTENT_COLUMNS`.

In `row_placement_at`, after `blocked_reason: r.get(at + 17)?,`:

```rust
        refile_once: r.get(at + 18)?,
        filed_home_folder: r.get(at + 19)?,
        filed_home_epoch: r.get(at + 20)?,
        filed_home_uid: r.get(at + 21)?,
```

In `row_intent`, after `race_until_uid: r.get(21)?,`: `consumes_refile: r.get(22)?,`.

In `records_for_planning`, the message columns now follow 22 placement columns: change `r.get(29)?`, `r.get(30)?`, `r.get(31)?`, `r.get(32)?`, `r.get(33)?` to `r.get(33)?`, `r.get(34)?`, `r.get(35)?`, `r.get(36)?`, `r.get(37)?` respectively.

Replace `write_placement` with:

```rust
/// `save_placement` inside a caller's transaction; does not bump the
/// revision. `done_inferred` is never written here: only done inference,
/// reopening and explicit review change it. Refile spec "Filed home": the
/// filed home is stored only while it is the known home, so every other
/// change of the home clears it.
fn write_placement(
    tx: &Connection,
    p: &Placement,
    expected_rev: Option<i64>,
    block: BlockWrite<'_>,
) -> Result<bool> {
    let (write_block, read_block) = match block {
        BlockWrite::Given => (true, None),
        BlockWrite::Changed(read) => {
            let changed = p.blocked_reason.as_deref() != read;
            (changed, changed.then_some(read))
        }
    };
    let filed = p.at_filed_home();
    let n = tx.execute(
        "UPDATE placements SET source_folder=?3,home_folder=?4,home_epoch=?5,home_uid=?6,location_state=?7,absent_since=?8,desired_target=?9,pinned=?10,eligible_once=?11,desired_rev=?12,filed_at=?13,filed_by=?14,flag_attempted_at=?15,flagged_at=?16,blocked_reason=CASE WHEN ?17 THEN ?18 ELSE blocked_reason END,refile_once=?22,filed_home_folder=?23,filed_home_epoch=?24,filed_home_uid=?25
 WHERE account=?1 AND message_id=?2 AND (?19 IS NULL OR desired_rev=?19) AND (?20=0 OR blocked_reason IS ?21)",
        params![
            p.account,
            p.message_id,
            p.source_folder,
            p.home_folder,
            p.home_epoch,
            p.home_uid,
            p.location_state.as_str(),
            p.absent_since,
            p.desired_target,
            p.pinned,
            p.eligible_once,
            p.desired_rev,
            p.filed_at,
            p.filed_by,
            p.flag_attempted_at,
            p.flagged_at,
            write_block,
            p.blocked_reason,
            expected_rev,
            read_block.is_some(),
            read_block.flatten(),
            p.refile_once,
            p.filed_home_folder.as_deref().filter(|_| filed),
            p.filed_home_epoch.filter(|_| filed),
            p.filed_home_uid.filter(|_| filed)
        ],
    )?;
    Ok(n == 1)
}
```

Replace `claim_move` with a delegating `claim_move` and `claim_move_with`:

```rust
    /// Claims a move in one transaction: refused (`None`) when the placement's
    /// `desired_rev` moved on, it is blocked, or a move intent is already open.
    pub fn claim_move(
        &mut self,
        account: &str,
        action: &Action,
        target_epoch: u64,
        target_uid_next: u64,
        batch: &str,
        now: &str,
    ) -> Result<Option<i64>> {
        self.claim_move_with(
            account,
            action,
            (target_epoch, target_uid_next),
            batch,
            now,
            false,
        )
    }

    /// `claim_move` with the target snapshot as `(epoch, UIDNEXT)`; the
    /// intent records whether it consumes a refile mark (refile spec).
    pub fn claim_move_with(
        &mut self,
        account: &str,
        action: &Action,
        (target_epoch, target_uid_next): (u64, u64),
        batch: &str,
        now: &str,
        consumes_refile: bool,
    ) -> Result<Option<i64>> {
        let Action::Move {
            message_id,
            from,
            to,
            desired_rev,
            consumes_eligible,
        } = action
        else {
            bail!("claim_move needs a Move")
        };
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ok: bool = tx
            .query_row(
                &format!(
                    "SELECT desired_rev=? AND blocked_reason IS NULL AND NOT EXISTS(SELECT 1 FROM filing_intents
           WHERE message_id=placements.message_id AND kind='move' AND state IN {})
         FROM placements WHERE account=? AND message_id=?",
                    open_states_sql()
                ),
                params![desired_rev, account, message_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false);
        if !ok {
            return Ok(None);
        }
        tx.execute("INSERT INTO filing_intents(account,message_id,kind,folder,epoch,uid,target,target_epoch,target_uid_next,desired_rev,consumes_eligible,consumes_refile,batch,state,dispatched_at,created_at,updated_at)
                VALUES(?,?,'move',?,?,?,?,?,?,?,?,?,?,'in_flight',?,?,?)",
            params![account, message_id, from.folder, from.epoch, from.uid, to, target_epoch, target_uid_next, desired_rev, consumes_eligible, consumes_refile, batch, now, now, now])?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(Some(id))
    }
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --locked --test refile_store && cargo test --locked --test filing_store && cargo test --locked --test heartbeat`
Expected: all PASS (`refile_store`: 5 passed).

- [ ] **Step 7: Full check and commit**

Run: `cargo fmt && cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`
Expected: clean, every test passes.

```bash
git add src/store.rs src/filing/types.rs src/filing/store.rs tests/filing_store.rs tests/heartbeat.rs tests/refile_store.rs
git commit -m "Store the refile mark and the filed home (schema v6)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Proven moves grant the filed home; applied refile moves consume the mark

**Files:**
- Modify: `src/filing/recover.rs` (`mark_move_applied`, new `apply_move`, new unit tests)
- Create: `tests/refile_support/mod.rs`, `tests/refile_filed_home.rs`

**Interfaces:**
- Consumes (Task 1): `Placement.refile_once`, `Placement.filed_home_*`, `Placement::at_filed_home`, `Intent.consumes_refile`.
- Produces:
  - Move applied grants `filed_home = (target, target_epoch, target_uid)` exactly when the applied home equals it; otherwise the filed home is `None`.
  - Move applied with `intent.consumes_refile` and a current `desired_rev` clears `refile_once` and bumps `desired_rev`.
  - Event `moved` detail gains `"reason": "refile"` for a refile intent.
  - Test support (used by Tasks 3–7): `tests/refile_support/mod.rs` with `updates_category`, `correspondence_category`, `without`, `add_category`, `remove_category`, `set_catch_all`, `add_updates`, `deliver_filed`, `filed_in_other_now_updates`, `id_of`, `located`, `placement`, `home`, `filed_home`, `mark`, `events`, `moves_to`, `pause`, `review_state`, `scoped`.

- [ ] **Step 1: Write the failing tests**

Append to `src/filing/recover.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::apply_move;
    use crate::filing::{Intent, LocationState, Placement};

    /// Marked, at its filed home in Other.
    fn placement() -> Placement {
        Placement {
            account: "work".into(),
            message_id: "m".into(),
            source_folder: "INBOX".into(),
            home_folder: Some("Other".into()),
            home_epoch: Some(4),
            home_uid: Some(9),
            location_state: LocationState::Known,
            absent_since: None,
            desired_target: None,
            pinned: false,
            eligible_once: false,
            desired_rev: 3,
            filed_at: Some("t0".into()),
            filed_by: Some("mailtriage".into()),
            flag_attempted_at: None,
            flagged_at: None,
            done_inferred: false,
            blocked_reason: None,
            refile_once: true,
            filed_home_folder: Some("Other".into()),
            filed_home_epoch: Some(4),
            filed_home_uid: Some(9),
        }
    }

    /// A refile move into Updates claimed in epoch 7.
    fn intent(target_uid: Option<u64>, desired_rev: i64) -> Intent {
        Intent {
            id: 1,
            account: "work".into(),
            message_id: "m".into(),
            kind: "move".into(),
            folder: "Other".into(),
            epoch: 4,
            uid: 9,
            target: Some("Updates".into()),
            target_epoch: Some(7),
            target_uid_next: Some(20),
            target_uid,
            desired_rev,
            consumes_eligible: false,
            batch: None,
            state: "sent".into(),
            attempts: 0,
            next_after: None,
            dispatched_at: None,
            created_at: "t".into(),
            updated_at: "t".into(),
            error: None,
            race_until_uid: None,
            consumes_refile: true,
        }
    }

    fn at(epoch: u64, uid: u64) -> (String, u64, u64) {
        ("Updates".into(), epoch, uid)
    }

    #[test]
    fn a_copyuid_proven_refile_move_grants_the_filed_home_and_consumes_the_mark() {
        let mut p = placement();
        apply_move(&mut p, &intent(Some(21), 3), &at(7, 21), "t1");
        assert_eq!(
            (p.home_folder.as_deref(), p.home_epoch, p.home_uid),
            (Some("Updates"), Some(7), Some(21))
        );
        assert_eq!(
            (
                p.filed_home_folder.as_deref(),
                p.filed_home_epoch,
                p.filed_home_uid
            ),
            (Some("Updates"), Some(7), Some(21))
        );
        assert!(!p.refile_once);
        assert_eq!(p.desired_rev, 4);
        assert_eq!(
            (p.filed_at.as_deref(), p.filed_by.as_deref()),
            (Some("t1"), Some("mailtriage"))
        );
    }

    #[test]
    fn without_a_matching_copyuid_there_is_no_filed_home() {
        // No COPYUID; another UID found by fingerprint; another target epoch.
        for (target_uid, home) in [(None, at(7, 21)), (Some(22), at(7, 21)), (Some(21), at(8, 21))] {
            let mut p = placement();
            apply_move(&mut p, &intent(target_uid, 3), &home, "t1");
            assert_eq!(p.filed_home_folder, None, "{target_uid:?} {home:?}");
            assert!(!p.refile_once, "the mark is consumed either way");
        }
    }

    #[test]
    fn a_stale_revision_keeps_the_mark_but_not_the_proof() {
        let mut p = placement();
        apply_move(&mut p, &intent(Some(21), 2), &at(7, 21), "t1");
        assert!(p.refile_once);
        assert_eq!(p.desired_rev, 3);
        assert_eq!(
            p.filed_home_uid,
            Some(21),
            "the filed home follows the proof, not the revision"
        );
    }

    #[test]
    fn a_move_that_is_no_refile_keeps_the_mark() {
        let mut i = intent(Some(21), 3);
        i.consumes_refile = false;
        let mut p = placement();
        apply_move(&mut p, &i, &at(7, 21), "t1");
        assert!(p.refile_once);
        assert_eq!(p.desired_rev, 3);
    }
}
```

Create `tests/refile_support/mod.rs`:

```rust
#![allow(dead_code)]
//! Setup shared by the refile tests: mail that mailtriage filed and whose
//! category then changes, and readers for placements, events and calls.
use super::common::{mail, Harness};
use mailtriage::{domain::Category, filing::Placement};
use serde_json::Value;

/// The fake classifier files mail mentioning "update" into `updates`, or
/// into the catch-all `other` while `updates` is not configured.
pub fn updates_category() -> Category {
    Category {
        id: "updates".into(),
        name: "Updates".into(),
        description: "Status and service updates".into(),
        examples: vec![],
        catch_all: false,
        folder: Some("Updates".into()),
    }
}

/// Mail without a keyword; it stays in INBOX.
pub fn correspondence_category() -> Category {
    Category {
        id: "correspondence".into(),
        name: "Correspondence".into(),
        description: "Direct conversations with people".into(),
        examples: vec![],
        catch_all: false,
        folder: Some("INBOX".into()),
    }
}

/// Leaves categories out of the configuration file; call before the first pass.
pub fn without(h: &Harness, ids: &[&str]) {
    h.edit(|c| {
        c.accounts
            .get_mut("work")
            .unwrap()
            .categories
            .retain(|cat| !ids.contains(&cat.id.as_str()))
    });
}

/// `categories apply` with `category` added: open mail is classified again.
pub fn add_category(h: &Harness, category: Category) -> Value {
    let mut categories = h.service().config.accounts["work"].categories.clone();
    categories.push(category);
    h.service().apply_categories("work", categories).unwrap()
}

/// `categories apply` without category `id`.
pub fn remove_category(h: &Harness, id: &str) -> Value {
    let categories = h.service().config.accounts["work"]
        .categories
        .iter()
        .filter(|c| c.id != id)
        .cloned()
        .collect();
    h.service().apply_categories("work", categories).unwrap()
}

/// `categories apply` with `id` as the only catch-all: mail whose keyword
/// category is not configured follows it.
pub fn set_catch_all(h: &Harness, id: &str) {
    let mut categories = h.service().config.accounts["work"].categories.clone();
    for c in &mut categories {
        c.catch_all = c.id == id;
    }
    h.service().apply_categories("work", categories).unwrap();
}

/// Adds `updates` back and runs the pass that classifies open mail again.
pub fn add_updates(h: &Harness) {
    add_category(h, updates_category());
    h.sync();
}

/// Delivers `mid` into INBOX and runs the two passes that file it.
pub fn deliver_filed(h: &Harness, mid: &str, subject: &str, body: &str) -> String {
    h.fake.deliver("INBOX", &mail(mid, subject, body));
    h.sync();
    h.sync();
    id_of(h, mid)
}

/// "System update" mail (`u`) filed into Other while `updates` was left
/// out, then `updates` added back: the message now belongs in Updates.
pub fn filed_in_other_now_updates(h: &Harness) -> String {
    without(h, &["updates"]);
    h.sync();
    let id = deliver_filed(h, "u", "System update", "Version 2 is out");
    add_updates(h);
    id
}

pub fn id_of(h: &Harness, mid: &str) -> String {
    let rfc = format!("<{mid}@test>");
    h.service()
        .store
        .records("work")
        .unwrap()
        .into_iter()
        .find(|r| r.envelope["message_id"] == rfc.as_str())
        .unwrap()
        .id
}

/// Where the server holds `mid`: (folder, UID) of its first copy.
pub fn located(h: &Harness, mid: &str) -> (String, u64) {
    h.fake.locate(&format!("<{mid}@test>"))[0].clone()
}

pub fn placement(h: &Harness, id: &str) -> Placement {
    h.service().store.placement("work", id).unwrap().unwrap()
}

pub fn home(p: &Placement) -> Option<(String, u64, u64)> {
    Some((p.home_folder.clone()?, p.home_epoch?, p.home_uid?))
}

pub fn filed_home(p: &Placement) -> Option<(String, u64, u64)> {
    Some((p.filed_home_folder.clone()?, p.filed_home_epoch?, p.filed_home_uid?))
}

/// Sets the refile mark the way `filing refile --apply` does.
pub fn mark(h: &Harness, id: &str) {
    let mut s = h.service();
    let mut p = s.store.placement("work", id).unwrap().unwrap();
    let rev = p.desired_rev;
    p.refile_once = true;
    p.desired_rev += 1;
    assert!(s.store.save_placement(&p, Some(rev)).unwrap());
}

/// Filing events of `kind`, newest first.
pub fn events(h: &Harness, kind: &str) -> Vec<Value> {
    h.service()
        .store
        .events("work", None, 500)
        .unwrap()
        .into_iter()
        .filter(|e| e["kind"] == kind)
        .collect()
}

/// MOVE calls into `target`.
pub fn moves_to(h: &Harness, target: &str) -> usize {
    let suffix = format!(" -> {target}");
    h.fake
        .calls()
        .iter()
        .filter(|c| c.starts_with("move ") && c.ends_with(&suffix))
        .count()
}

/// A safety pause on a folder, as an epoch race leaves it.
pub fn pause(h: &Harness, native: &str) {
    let mut s = h.service();
    let mut rec = s.store.folder_record("work", native).unwrap().unwrap();
    rec.pause_reason = Some("epoch_race".into());
    s.store.save_folder(&rec).unwrap();
}

pub fn review_state(h: &Harness, id: &str) -> String {
    h.service().read("work", id).unwrap()["item"]["review_state"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Whether the latest pass put `folder` in the engine's watch scope.
pub fn scoped(h: &Harness, folder: &str) -> bool {
    h.fake
        .calls()
        .iter()
        .rev()
        .find_map(|c| c.strip_prefix("scope ").map(str::to_string))
        .is_some_and(|scope| scope.split(',').any(|f| f == folder))
}
```

Create `tests/refile_filed_home.rs`:

```rust
//! Refile spec "Filed home": which mailtriage moves grant it, and what ends it.
mod common;
mod refile_support;
use common::{mail, Harness};
use mailtriage::{
    domain::FilingMode::Live,
    engine::fake::{FakeOp, Fault},
    filing::LocationState,
    service::RetryTarget,
};
use refile_support::{deliver_filed, filed_home, home, id_of, located, pause, placement};

#[test]
fn a_move_proven_by_copyuid_grants_the_filed_home() {
    let h = Harness::new(Live);
    h.sync();
    let id = deliver_filed(&h, "n", "Weekly newsletter", "Our newsletter");
    let p = placement(&h, &id);
    let (folder, uid) = located(&h, "n");
    assert_eq!(folder, "Newsletters");
    assert_eq!(home(&p), Some((folder.clone(), h.fake.epoch(&folder), uid)));
    assert_eq!(filed_home(&p), home(&p));
    assert!(p.at_filed_home());
}

#[test]
fn moves_without_a_matching_copyuid_grant_none() {
    // No UIDPLUS: the moved copy is identified by fingerprint.
    let h = Harness::new(Live);
    h.fake.set_capabilities(true, false, true);
    h.sync();
    let id = deliver_filed(&h, "n", "Weekly newsletter", "Our newsletter");
    h.sync();
    let p = placement(&h, &id);
    assert_eq!(
        (p.home_folder.as_deref(), p.filed_by.as_deref()),
        (Some("Newsletters"), Some("mailtriage"))
    );
    assert_eq!(filed_home(&p), None);

    // A lost response: recovery applies the move by observation.
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::ErrorAfter);
    for _ in 0..4 {
        h.sync();
    }
    let p = placement(&h, &id_of(&h, "n"));
    assert_eq!(p.home_folder.as_deref(), Some("Newsletters"));
    assert_eq!(filed_home(&p), None);
}

#[test]
fn a_move_applied_after_its_targets_epoch_changed_grants_none() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("n", "Weekly newsletter", "Our newsletter"));
    h.sync(); // moved; the intent waits for its arrival
    h.fake.reset_epoch("Newsletters");
    for _ in 0..5 {
        h.sync();
    }
    let s = h.service();
    let intent = s
        .store
        .intents("work", false)
        .unwrap()
        .into_iter()
        .find(|i| i.kind == "move")
        .unwrap();
    assert_eq!(intent.state, "applied");
    let p = s.store.placement("work", &intent.message_id).unwrap().unwrap();
    assert_eq!(p.home_epoch, Some(h.fake.epoch("Newsletters")));
    assert_eq!(filed_home(&p), None);
}

#[test]
fn a_client_move_or_relocation_ends_the_filed_home() {
    let h = Harness::new(Live);
    h.sync();
    let a = deliver_filed(&h, "a", "Weekly newsletter", "Our newsletter");
    let b = deliver_filed(&h, "b", "Second newsletter", "More newsletter news");
    let (folder, uid) = located(&h, "a");
    h.fake.client_move(&folder, uid, "Updates");
    let (folder, uid) = located(&h, "b");
    h.fake.client_move(&folder, uid, &folder); // back into the folder it was in
    h.sync();
    h.sync();
    let a = placement(&h, &a);
    assert_eq!((a.home_folder.as_deref(), filed_home(&a)), (Some("Updates"), None));
    let b = placement(&h, &b);
    assert_eq!(b.home_folder.as_deref(), Some("Newsletters"));
    assert_eq!(b.home_uid, Some(located(&h, "b").1));
    assert_eq!(filed_home(&b), None);
}

#[test]
fn an_epoch_reset_ends_it_and_rediscovery_never_restores_it() {
    let h = Harness::new(Live);
    h.sync();
    let id = deliver_filed(&h, "n", "Weekly newsletter", "Our newsletter");
    h.fake.reset_epoch("Newsletters");
    for _ in 0..4 {
        h.sync();
    }
    let p = placement(&h, &id);
    assert_eq!(
        (p.home_folder.as_deref(), p.home_epoch),
        (Some("Newsletters"), Some(h.fake.epoch("Newsletters")))
    );
    assert_eq!(filed_home(&p), None);
}

#[test]
fn a_rescan_in_the_same_epoch_keeps_it() {
    let h = Harness::new(Live);
    h.sync();
    let id = deliver_filed(&h, "n", "Weekly newsletter", "Our newsletter");
    let before = filed_home(&placement(&h, &id));
    assert!(before.is_some());
    pause(&h, "Newsletters");
    h.service()
        .filing_retry("work", RetryTarget::Folder("Newsletters".into()))
        .unwrap();
    for _ in 0..3 {
        h.sync();
    }
    let rec = h
        .service()
        .store
        .folder_record("work", "Newsletters")
        .unwrap()
        .unwrap();
    assert!(rec.rescan_complete);
    assert_eq!(filed_home(&placement(&h, &id)), before);
}

#[test]
fn mail_that_left_every_folder_loses_it() {
    let h = Harness::new(Live);
    h.sync();
    let id = deliver_filed(&h, "n", "Weekly newsletter", "Our newsletter");
    let (folder, uid) = located(&h, "n");
    h.fake.client_delete(&folder, uid);
    h.sync();
    h.sync();
    let p = placement(&h, &id);
    assert_eq!(p.location_state, LocationState::Absent);
    assert_eq!(filed_home(&p), None);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --locked --lib recover::tests && cargo test --locked --test refile_filed_home`
Expected: `recover::tests` fails to compile (`apply_move` not found). `refile_filed_home` compiles (the fields exist since Task 1) and `a_move_proven_by_copyuid_grants_the_filed_home` fails: the filed home is `None`.

- [ ] **Step 3: Move applied (`src/filing/recover.rs`)**

Add `Placement` to the `use super::{…}` list. Replace `mark_move_applied` with:

```rust
/// Spec "Placement transitions", Move applied: the placement changes as
/// `apply_move` says; in the same transaction the intent is `applied`, event
/// `moved` is recorded (with `"reason": "refile"` for a refile move), and
/// arrivals of the intent or at the new home resolve as `own_move`.
pub(crate) fn mark_move_applied(
    store: &mut Store,
    ctx: &PassContext,
    intent: &Intent,
    home: (String, u64, u64),
    _summary: &mut FilingSummary,
) -> Result<()> {
    let (folder, epoch, uid) = &home;
    let arrivals: Vec<i64> = store
        .arrivals_at(ctx.account, folder, *epoch, 0)?
        .into_iter()
        .filter(|a| a.state == "pending")
        .filter(|a| {
            a.intent_id == Some(intent.id) || (a.uid == *uid && a.message_id == intent.message_id)
        })
        .map(|a| a.id)
        .collect();
    let mut writes = vec![close(intent.id, "applied", None)];
    let mut detail = json!({"intent_id": intent.id, "from": intent.folder});
    if intent.consumes_refile {
        detail["reason"] = json!("refile");
    }
    writes.push(event(Some(&intent.message_id), Some(folder), "moved", detail));
    writes.extend(arrivals.iter().map(|id| FilingWrite::Arrival {
        id: *id,
        state: "resolved",
        kind: Some("own_move"),
    }));
    commit_with_placement(
        store,
        ctx,
        &intent.message_id,
        |p| apply_move(p, intent, &home, &ctx.now),
        &writes,
    )
}

/// Move applied, on the placement alone: the home becomes the target
/// occurrence, `filed_by = mailtriage`; the desired fields the intent
/// consumes (`desired_target`, and `eligible_once` or `refile_once` when
/// the intent consumes them) are cleared with a revision bump only while the
/// intent's revision is current, so a newer request is never consumed.
/// Refile spec "Filed home": the new home becomes the filed home only when it
/// is the COPYUID destination this intent recorded, in its target epoch.
fn apply_move(p: &mut Placement, intent: &Intent, home: &(String, u64, u64), now: &str) {
    let (folder, epoch, uid) = home;
    p.home_folder = Some(folder.clone());
    p.home_epoch = Some(*epoch);
    p.home_uid = Some(*uid);
    p.location_state = LocationState::Known;
    p.absent_since = None;
    p.filed_at = Some(now.to_string());
    p.filed_by = Some("mailtriage".into());
    let proven = intent.target.as_deref() == Some(folder.as_str())
        && intent.target_epoch == Some(*epoch)
        && intent.target_uid == Some(*uid);
    p.filed_home_folder = proven.then(|| folder.clone());
    p.filed_home_epoch = proven.then_some(*epoch);
    p.filed_home_uid = proven.then_some(*uid);
    if intent.desired_rev == p.desired_rev {
        let clear_target = p.desired_target.take().is_some();
        let clear_eligible = intent.consumes_eligible && p.eligible_once;
        if clear_eligible {
            p.eligible_once = false;
        }
        let clear_refile = intent.consumes_refile && p.refile_once;
        if clear_refile {
            p.refile_once = false;
        }
        if clear_target || clear_eligible || clear_refile {
            p.desired_rev += 1;
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --locked --lib recover::tests && cargo test --locked --test refile_filed_home && cargo test --locked --test filing_writes --test filing_location`
Expected: all PASS; the existing filing tests are unchanged.

- [ ] **Step 5: Full check and commit**

Run: `cargo fmt && cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`

```bash
git add src/filing/recover.rs tests/refile_support/mod.rs tests/refile_filed_home.rs
git commit -m "Grant the filed home on COPYUID-proven moves and consume refile marks

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Candidate rules, the planner's refile rule and refile claims

**Files:**
- Create: `src/filing/refile/mod.rs`, `src/filing/refile/rules.rs`, `tests/refile_planning.rs`
- Modify: `src/filing/mod.rs` (`pub mod refile;`)
- Modify: `src/filing/planner.rs` (imports, `Plan.refile_moves`, `MoveDecision`, `plan`, new `plan_with_refile`, `move_action`, `target_usable` → `pub(crate)`, new nested test module)
- Modify: `src/filing/apply.rs` (`apply`, `move_batch`)
- Modify: `src/filing/store.rs` (`occurrence_counts`, `frozen_folders`; import `BTreeMap`)
- Modify: `src/service.rs` (`filing` import list, `plan_and_apply`, `filing_plan`, new `plan_action`)

**Interfaces:**
- Consumes (Tasks 1–2): `Placement::at_filed_home`, `Placement.refile_once`, `Intent.consumes_refile`, `Store::claim_move_with`; test support `filed_in_other_now_updates`, `mark`, `events`, `moves_to`, `located`, `placement`, `home`, `filed_home`.
- Produces:
  - `filing::refile::rules::Skip` (12 variants, `Skip::ALL`, `fn as_str(self) -> &'static str`), `Reason { CategoryChanged, FolderRetired }` with `as_str`, `Candidate { target: String, category: String, reason: Reason }`, `Verdict { OutOfScope, Waiting, InPlace, Skipped(Skip), Candidate(Candidate) }`.
  - `RefileFacts { marked, at_filed_home, home_folder: Option<String>, pinned, blocked, done, open_move_intent, open_refile_intent, explicit_target, corrected, single_occurrence }` (all `pub`, `Default`), `RefileInput { facts: BTreeMap<String, RefileFacts>, frozen: BTreeSet<String>, categories: BTreeSet<String> }` (`Default`).
  - `rules::gone(store: &Store, account: &str, listed: &BTreeSet<String>) -> Result<BTreeSet<String>>`.
  - `rules::input(store: &Store, account: &str, cfg: &AccountConfig, gone: &BTreeSet<String>) -> Result<RefileInput>`.
  - `rules::placement_skip(f: &RefileFacts, frozen: &BTreeSet<String>) -> Option<Skip>`.
  - `rules::verdict(input: &PlanInput, m: &PlanMessage, refile: &RefileInput) -> Verdict`.
  - `rules::in_category_folder(input: &PlanInput, folder: &str) -> bool`.
  - `planner::plan_with_refile(input: &PlanInput, refile: &RefileInput) -> Plan`; `Plan.refile_moves: BTreeSet<String>`; `planner::target_usable` is `pub(crate)`.
  - `filing::refile::plan_pass(store: &Store, ctx: &PassContext, map: &FolderMap, preview: bool) -> Result<Plan>` (Task 4 changes `store` to `&mut Store`).
  - `Store::occurrence_counts(&self, account: &str) -> Result<BTreeMap<String, usize>>`, `Store::frozen_folders(&self, account: &str) -> Result<BTreeSet<String>>`.
  - `filing plan` actions of refile moves carry `"reason": "refile"`.

- [ ] **Step 1: Write the failing tests**

Create `tests/refile_planning.rs`:

```rust
//! Refile spec "Each pass" (Planning): marked mail moves in the next pass.
mod common;
mod refile_support;
use common::Harness;
use mailtriage::domain::FilingMode::{DryRun, Live};
use refile_support::{
    events, filed_home, filed_in_other_now_updates, home, located, mark, moves_to, placement,
};

#[test]
fn a_marked_message_is_refiled_by_the_next_pass() {
    let h = Harness::new(Live);
    let id = filed_in_other_now_updates(&h);
    mark(&h, &id);
    let plan = h.service().filing_plan("work", 50).unwrap();
    assert_eq!(plan["total"], 1);
    let action = &plan["actions"][0];
    assert_eq!(action["message_id"], id.as_str());
    assert_eq!(action["to"], "Updates");
    assert_eq!(action["reason"], "refile");
    h.sync();
    h.sync();
    assert_eq!(located(&h, "u").0, "Updates");
    let p = placement(&h, &id);
    assert!(!p.refile_once, "consumed by its own move");
    assert_eq!(p.home_folder.as_deref(), Some("Updates"));
    assert_eq!(filed_home(&p), home(&p), "a refile move is proven like any other");
    assert_eq!(events(&h, "moved")[0]["detail"]["reason"], "refile");
    let intent = h.service().store.intents("work", false).unwrap().pop().unwrap();
    assert!(intent.consumes_refile);
    assert_eq!(intent.state, "applied");
    h.sync();
    assert_eq!(moves_to(&h, "Updates"), 1, "moved once");
    assert_eq!(h.service().filing_plan("work", 50).unwrap()["total"], 0);
}

#[test]
fn unmarked_mail_is_not_refiled() {
    let h = Harness::new(Live);
    filed_in_other_now_updates(&h);
    h.sync();
    h.sync();
    assert_eq!(located(&h, "u").0, "Other");
    assert_eq!(moves_to(&h, "Updates"), 0);
}

#[test]
fn a_dry_run_pass_plans_the_refile_and_writes_nothing() {
    let h = Harness::new(Live);
    let id = filed_in_other_now_updates(&h);
    mark(&h, &id);
    h.set_mode(DryRun);
    let writes = h.fake.write_calls();
    let out = h.sync();
    assert_eq!(out["filing"]["planned"], 1);
    assert_eq!(h.fake.write_calls(), writes);
    assert!(placement(&h, &id).refile_once, "the mark waits for live");
}
```

Append a nested module at the end of `mod tests` in `src/filing/planner.rs` (inside its closing brace):

```rust
    /// Refile spec "Candidates" and the planner's refile rule.
    mod refile {
        use super::{flags, homed, input, moves, msg, t, view};
        use crate::filing::planner::{
            plan, plan_with_refile, Action, FolderUse, PlanInput, PlanMessage,
        };
        use crate::filing::refile::rules::{
            verdict, Candidate, Reason, RefileFacts, RefileInput, Skip, Verdict,
        };
        use std::collections::BTreeSet;

        /// Filed by mailtriage into Newsletters; `category` is its current classification.
        fn filed(id: &str, category: &str) -> PlanMessage {
            let mut m = homed(msg(id, category), "Newsletters", 2);
            m.filed_at = Some("x".into());
            m
        }

        /// Marked, at its filed home, every placement rule holding.
        fn facts(m: &PlanMessage) -> RefileFacts {
            RefileFacts {
                marked: true,
                at_filed_home: true,
                home_folder: m.home.as_ref().map(|h| h.folder.clone()),
                single_occurrence: true,
                ..Default::default()
            }
        }

        /// `updates` is configured but missing from `input()`'s folder map,
        /// as a category whose folder collides with a source is.
        fn refile(entries: &[(&PlanMessage, RefileFacts)]) -> RefileInput {
            RefileInput {
                facts: entries
                    .iter()
                    .map(|(m, f)| (m.message_id.clone(), f.clone()))
                    .collect(),
                frozen: BTreeSet::new(),
                categories: ["correspondence", "transactions", "newsletters", "updates", "other"]
                    .map(String::from)
                    .into_iter()
                    .collect(),
            }
        }

        fn plan_of(i: &PlanInput, r: &RefileInput) -> Vec<(String, String)> {
            moves(&plan_with_refile(i, r))
        }

        #[test]
        fn a_marked_message_moves_to_its_new_category_folder() {
            let m = filed("m", "transactions");
            let p = plan_with_refile(&input(vec![m.clone()]), &refile(&[(&m, facts(&m))]));
            assert_eq!(moves(&p), vec![("m".into(), "Transactions".into())]);
            assert_eq!(p.refile_moves, BTreeSet::from(["m".to_string()]));
            assert!(matches!(
                &p.actions[0],
                Action::Move {
                    consumes_eligible: false,
                    desired_rev: 0,
                    ..
                }
            ));
            let mut unmarked = facts(&m);
            unmarked.marked = false;
            assert!(
                plan_of(&input(vec![m.clone()]), &refile(&[(&m, unmarked)])).is_empty(),
                "only marked mail is refiled"
            );
            assert!(plan(&input(vec![m])).actions.is_empty(), "`plan` has no refile facts");
        }

        #[test]
        fn every_placement_rule_keeps_a_marked_message_where_it_is() {
            type Change = fn(&mut PlanMessage, &mut RefileFacts);
            let cases: [(&str, Change); 7] = [
                ("not at its filed home", |_, f| f.at_filed_home = false),
                ("pinned", |m, f| {
                    m.pinned = true;
                    f.pinned = true;
                }),
                ("blocked", |m, f| {
                    m.blocked = true;
                    f.blocked = true;
                }),
                ("done", |_, f| f.done = true),
                ("open move intent", |m, f| {
                    m.open_move_intent = true;
                    f.open_move_intent = true;
                }),
                ("corrected", |m, f| {
                    m.effective.category_from_override = true;
                    f.corrected = true;
                }),
                ("two copies", |_, f| f.single_occurrence = false),
            ];
            for (name, change) in cases {
                let mut m = filed("m", "transactions");
                let mut f = facts(&m);
                change(&mut m, &mut f);
                assert!(plan_of(&input(vec![m.clone()]), &refile(&[(&m, f)])).is_empty(), "{name}");
            }
        }

        #[test]
        fn an_explicit_target_and_the_source_rule_come_first() {
            let mut m = filed("m", "transactions");
            m.desired_target = Some("transactions".into());
            m.desired_rev = 4;
            let mut f = facts(&m);
            f.explicit_target = true;
            let p = plan_with_refile(&input(vec![m.clone()]), &refile(&[(&m, f)]));
            assert_eq!(moves(&p), vec![("m".into(), "Transactions".into())]);
            assert!(p.refile_moves.is_empty(), "the explicit request's move consumes no mark");
            let s = msg("s", "newsletters");
            let p = plan_with_refile(&input(vec![s.clone()]), &refile(&[(&s, facts(&s))]));
            assert_eq!(moves(&p), vec![("s".into(), "Newsletters".into())]);
            assert!(p.refile_moves.is_empty(), "a source folder's mail follows the source rule");
        }

        #[test]
        fn a_marked_message_waits_for_a_current_classification_and_a_usable_target() {
            let mut stale = filed("stale", "transactions");
            stale.effective.current = false;
            let paused = filed("paused", "transactions");
            let r = refile(&[(&stale, facts(&stale)), (&paused, facts(&paused))]);
            let mut i = input(vec![stale.clone(), paused.clone()]);
            i.folders.get_mut("Transactions").unwrap().paused = true;
            assert!(plan_of(&i, &r).is_empty());
            assert_eq!(verdict(&i, &stale, &r), Verdict::Waiting);
            assert_eq!(verdict(&i, &paused, &r), Verdict::Skipped(Skip::TargetUnusable));
            let mut i = input(vec![paused.clone()]);
            i.folders.get_mut("Transactions").unwrap().epoch = None;
            assert!(plan_of(&i, &r).is_empty(), "a target without established discovery");
        }

        #[test]
        fn incomplete_input_never_moves() {
            let mut m = filed("m", "transactions");
            m.effective.input_incomplete = true;
            let r = refile(&[(&m, facts(&m))]);
            let i = input(vec![m.clone()]);
            assert!(plan_of(&i, &r).is_empty());
            assert_eq!(verdict(&i, &m, &r), Verdict::Skipped(Skip::IncompleteInput));
        }

        #[test]
        fn inbox_and_source_folders_are_never_targets() {
            let inbox = filed("inbox", "correspondence");
            let collides = filed("collides", "updates");
            let unknown = filed("unknown", "removed");
            let r = refile(&[
                (&inbox, facts(&inbox)),
                (&collides, facts(&collides)),
                (&unknown, facts(&unknown)),
            ]);
            let i = input(vec![inbox.clone(), collides.clone(), unknown.clone()]);
            assert!(plan_of(&i, &r).is_empty());
            assert_eq!(verdict(&i, &inbox, &r), Verdict::Skipped(Skip::TargetInboxOrSource));
            assert_eq!(verdict(&i, &collides, &r), Verdict::Skipped(Skip::TargetInboxOrSource));
            assert_eq!(verdict(&i, &unknown, &r), Verdict::Skipped(Skip::TargetUnusable));
        }

        #[test]
        fn verdicts_name_the_target_and_why_it_moves() {
            let here = filed("here", "newsletters");
            let changed = filed("changed", "transactions");
            let mut i = input(vec![]);
            i.folders.insert("Old".into(), view(FolderUse::Unusable, false, true, 5));
            i.folders.insert("Gone".into(), view(FolderUse::Unusable, false, false, 6));
            let mut retired = homed(msg("retired", "transactions"), "Old", 5);
            retired.filed_at = Some("x".into());
            let mut unlisted = homed(msg("unlisted", "transactions"), "Gone", 6);
            unlisted.filed_at = Some("x".into());
            let source = msg("source", "transactions");
            let r = refile(&[
                (&here, facts(&here)),
                (&changed, facts(&changed)),
                (&retired, facts(&retired)),
                (&unlisted, facts(&unlisted)),
                (&source, facts(&source)),
            ]);
            let to_transactions = |reason| {
                Verdict::Candidate(Candidate {
                    target: "Transactions".into(),
                    category: "transactions".into(),
                    reason,
                })
            };
            assert_eq!(verdict(&i, &here, &r), Verdict::InPlace);
            assert_eq!(verdict(&i, &changed, &r), to_transactions(Reason::CategoryChanged));
            assert_eq!(verdict(&i, &retired, &r), to_transactions(Reason::FolderRetired));
            assert_eq!(verdict(&i, &unlisted, &r), to_transactions(Reason::FolderRetired));
            assert_eq!(verdict(&i, &source, &r), Verdict::OutOfScope);
            i.messages = vec![retired, unlisted];
            assert_eq!(
                plan_of(&i, &r),
                vec![("retired".into(), "Transactions".into())],
                "out of a retired folder only while LIST reports it"
            );
        }

        #[test]
        fn the_refile_move_writes_no_flag_and_the_flag_rule_is_unchanged() {
            let mut actionable = filed("act", "transactions");
            actionable.effective.action_required = Some(true);
            let quiet = filed("quiet", "transactions");
            let r = refile(&[(&actionable, facts(&actionable)), (&quiet, facts(&quiet))]);
            let p = plan_with_refile(&input(vec![actionable, quiet]), &r);
            assert_eq!(flags(&p), vec!["act".to_string()], "the existing rule flags filed mail");
            assert!(
                matches!(&p.actions[0], Action::Flag { at, .. } if at.folder == "Newsletters"),
                "where it is, before the move"
            );
            assert_eq!(moves(&p).len(), 2);
        }

        #[test]
        fn refile_moves_share_the_cap() {
            let mut a = filed("a", "transactions");
            a.internal_date = Some(t(11));
            let mut b = filed("b", "transactions");
            b.internal_date = Some(t(10));
            let r = refile(&[(&a, facts(&a)), (&b, facts(&b))]);
            let mut i = input(vec![a, b]);
            i.max_actions = 1;
            let p = plan_with_refile(&i, &r);
            assert_eq!(moves(&p), vec![("b".into(), "Transactions".into())]);
            assert_eq!(p.refile_moves, BTreeSet::from(["b".to_string()]), "only moves that made the cut");
        }
    }
```

Create `src/filing/refile/rules.rs` with only its test module for now (Step 3 adds the code above it):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn ok() -> RefileFacts {
        RefileFacts {
            marked: true,
            at_filed_home: true,
            home_folder: Some("News".into()),
            single_occurrence: true,
            ..Default::default()
        }
    }

    #[test]
    fn rules_one_to_five_report_the_first_failure_in_list_order() {
        let none = BTreeSet::new();
        assert_eq!(placement_skip(&ok(), &none), None);
        let mut f = ok();
        f.single_occurrence = false;
        assert_eq!(placement_skip(&f, &none), Some(Skip::MultipleCopies));
        f.explicit_target = true;
        assert_eq!(placement_skip(&f, &none), Some(Skip::ExplicitTarget));
        f.open_move_intent = true;
        assert_eq!(placement_skip(&f, &none), Some(Skip::OpenIntent));
        f.done = true;
        assert_eq!(placement_skip(&f, &none), Some(Skip::Done));
        f.blocked = true;
        assert_eq!(placement_skip(&f, &none), Some(Skip::Blocked));
        f.pinned = true;
        assert_eq!(placement_skip(&f, &none), Some(Skip::Pinned));
        f.corrected = true;
        assert_eq!(placement_skip(&f, &none), Some(Skip::Corrected));
        f.at_filed_home = false;
        assert_eq!(placement_skip(&f, &none), Some(Skip::NotFiledByMailtriage));
        let frozen = BTreeSet::from(["News".to_string()]);
        assert_eq!(placement_skip(&f, &frozen), Some(Skip::RetiredFrozen));
        assert_eq!(placement_skip(&ok(), &frozen), None, "mail at its filed home is never frozen out");
    }

    #[test]
    fn skip_names_are_the_specs_keys() {
        assert_eq!(
            Skip::ALL.map(Skip::as_str),
            [
                "not_filed_by_mailtriage",
                "corrected",
                "pinned",
                "blocked",
                "done",
                "open_intent",
                "explicit_target",
                "multiple_copies",
                "incomplete_input",
                "retired_frozen",
                "target_unusable",
                "target_inbox_or_source",
            ]
        );
        assert_eq!(Reason::CategoryChanged.as_str(), "category_changed");
        assert_eq!(Reason::FolderRetired.as_str(), "folder_retired");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --locked --lib refile && cargo test --locked --test refile_planning`
Expected: compile errors — module `refile` not found, `plan_with_refile` not found, no field `refile_moves`.

- [ ] **Step 3: The rules (`src/filing/refile/rules.rs`, `src/filing/refile/mod.rs`, `src/filing/mod.rs`)**

In `src/filing/mod.rs` add `pub mod refile;` after `pub mod recover;`.

Create `src/filing/refile/mod.rs`:

```rust
//! Refiling filed mail after category changes (refile spec,
//! `design/specs/2026-10-06-filing-refile-design.md`).
pub mod rules;

use super::observe::FolderMap;
use super::planner::{self, Plan};
use super::{inputs, PassContext};
use crate::store::Store;
use anyhow::Result;

/// Step 9's plan: the planner input and every placement's refile facts;
/// marked messages get the refile rule.
pub fn plan_pass(store: &Store, ctx: &PassContext, map: &FolderMap, preview: bool) -> Result<Plan> {
    let input = inputs::plan_input(store, ctx, map, preview)?;
    let gone = rules::gone(store, ctx.account, &map.listed)?;
    let refile = rules::input(store, ctx.account, ctx.cfg, &gone)?;
    Ok(planner::plan_with_refile(&input, &refile))
}
```

Put this above the test module in `src/filing/refile/rules.rs`:

```rust
//! Refile spec "Candidates": which filed messages may follow a category
//! change, and why the others stay where they are. Pure rules over the
//! planner input, plus per-placement facts read from the store.
use crate::domain::AccountConfig;
use crate::filing::planner::{target_usable, CategoryFolder, PlanInput, PlanMessage};
use crate::filing::{MessageMeta, Placement};
use crate::store::{Record, Store};
use anyhow::Result;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Why a message in a category or retired folder stays where it is (the
/// `skipped` keys), in the order of the spec's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Skip {
    NotFiledByMailtriage,
    Corrected,
    Pinned,
    Blocked,
    Done,
    OpenIntent,
    ExplicitTarget,
    MultipleCopies,
    IncompleteInput,
    RetiredFrozen,
    TargetUnusable,
    TargetInboxOrSource,
}

impl Skip {
    pub const ALL: [Skip; 12] = [
        Skip::NotFiledByMailtriage,
        Skip::Corrected,
        Skip::Pinned,
        Skip::Blocked,
        Skip::Done,
        Skip::OpenIntent,
        Skip::ExplicitTarget,
        Skip::MultipleCopies,
        Skip::IncompleteInput,
        Skip::RetiredFrozen,
        Skip::TargetUnusable,
        Skip::TargetInboxOrSource,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Skip::NotFiledByMailtriage => "not_filed_by_mailtriage",
            Skip::Corrected => "corrected",
            Skip::Pinned => "pinned",
            Skip::Blocked => "blocked",
            Skip::Done => "done",
            Skip::OpenIntent => "open_intent",
            Skip::ExplicitTarget => "explicit_target",
            Skip::MultipleCopies => "multiple_copies",
            Skip::IncompleteInput => "incomplete_input",
            Skip::RetiredFrozen => "retired_frozen",
            Skip::TargetUnusable => "target_unusable",
            Skip::TargetInboxOrSource => "target_inbox_or_source",
        }
    }
}

/// Why a candidate moves: its folder belongs to another category, or to none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    CategoryChanged,
    FolderRetired,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::CategoryChanged => "category_changed",
            Reason::FolderRetired => "folder_retired",
        }
    }
}

/// A candidate's new folder (native) and category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub target: String,
    pub category: String,
    pub reason: Reason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Not refile's business: no known home, or a home in a source folder or
    /// in a folder without a record.
    OutOfScope,
    /// Rules 1–5 hold; its new category is decided once it is classified again.
    Waiting,
    /// Its current classification files it where it is.
    InPlace,
    Skipped(Skip),
    Candidate(Candidate),
}

/// What the store says about one placement beyond the planner input.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefileFacts {
    /// `refile_once`.
    pub marked: bool,
    /// Rules 1–2: a known, hydrated home that is the filed home, in a folder
    /// that still exists.
    pub at_filed_home: bool,
    /// The recorded home folder, known or not.
    pub home_folder: Option<String>,
    pub pinned: bool,
    pub blocked: bool,
    /// Review state `done`.
    pub done: bool,
    pub open_move_intent: bool,
    /// The open move intent is a refile intent.
    pub open_refile_intent: bool,
    /// An outstanding `desired_target`.
    pub explicit_target: bool,
    /// A manual category override.
    pub corrected: bool,
    pub single_occurrence: bool,
}

/// Refile facts by message id, beside the planner input.
#[derive(Debug, Clone, Default)]
pub struct RefileInput {
    pub facts: BTreeMap<String, RefileFacts>,
    /// Retired folders that are neither retained nor draining.
    pub frozen: BTreeSet<String>,
    /// The account's configured category ids.
    pub categories: BTreeSet<String>,
}

/// Recorded category and retired folders `listed` does not report: their
/// mail went with them.
pub fn gone(store: &Store, account: &str, listed: &BTreeSet<String>) -> Result<BTreeSet<String>> {
    Ok(store
        .folder_records(account)?
        .into_iter()
        .filter(|r| r.category_id.is_some() && !listed.contains(&r.native))
        .map(|r| r.native)
        .collect())
}

/// The refile facts of every placement of the account.
pub fn input(
    store: &Store,
    account: &str,
    cfg: &AccountConfig,
    gone: &BTreeSet<String>,
) -> Result<RefileInput> {
    let rows = store.records_for_planning(account)?;
    let counts = store.occurrence_counts(account)?;
    build(store, account, cfg, gone, rows, &counts, None)
}

/// Facts of `rows`; the open move intent `exclude` does not count.
fn build(
    store: &Store,
    account: &str,
    cfg: &AccountConfig,
    gone: &BTreeSet<String>,
    rows: Vec<(Record, Placement, MessageMeta)>,
    counts: &BTreeMap<String, usize>,
    exclude: Option<i64>,
) -> Result<RefileInput> {
    // Message id -> whether one of its open move intents is a refile intent.
    let mut open: BTreeMap<String, bool> = BTreeMap::new();
    for i in store.intents(account, true)? {
        if i.kind == "move" && Some(i.id) != exclude {
            *open.entry(i.message_id).or_default() |= i.consumes_refile;
        }
    }
    let facts = rows
        .into_iter()
        .map(|(record, p, meta)| {
            let home_gone = p.home_folder.as_ref().is_some_and(|h| gone.contains(h));
            let f = RefileFacts {
                marked: p.refile_once,
                at_filed_home: p.at_filed_home() && meta.size.is_some() && !home_gone,
                home_folder: p.home_folder.clone(),
                pinned: p.pinned,
                blocked: p.blocked_reason.is_some(),
                done: record.review_state == "done",
                open_move_intent: open.contains_key(&p.message_id),
                open_refile_intent: open.get(&p.message_id) == Some(&true),
                explicit_target: p.desired_target.is_some(),
                corrected: record
                    .overrides
                    .get("category_id")
                    .and_then(Value::as_str)
                    .is_some(),
                single_occurrence: counts.get(&p.message_id) == Some(&1),
            };
            (p.message_id, f)
        })
        .collect();
    Ok(RefileInput {
        facts,
        frozen: store.frozen_folders(account)?,
        categories: cfg.categories.iter().map(|c| c.id.clone()).collect(),
    })
}

/// Candidate rules 1–5 in the order of the spec's `skipped` list; mail not
/// at its filed home in a frozen retired folder is `retired_frozen`.
pub fn placement_skip(f: &RefileFacts, frozen: &BTreeSet<String>) -> Option<Skip> {
    if !f.at_filed_home {
        let in_frozen = f.home_folder.as_ref().is_some_and(|h| frozen.contains(h));
        return Some(if in_frozen {
            Skip::RetiredFrozen
        } else {
            Skip::NotFiledByMailtriage
        });
    }
    [
        (f.corrected, Skip::Corrected),
        (f.pinned, Skip::Pinned),
        (f.blocked, Skip::Blocked),
        (f.done, Skip::Done),
        (f.open_move_intent, Skip::OpenIntent),
        (f.explicit_target, Skip::ExplicitTarget),
        (!f.single_occurrence, Skip::MultipleCopies),
    ]
    .into_iter()
    .find(|(failed, _)| *failed)
    .map(|(_, skip)| skip)
}

/// Candidate rules 1–8 for one planned message.
pub fn verdict(input: &PlanInput, m: &PlanMessage, refile: &RefileInput) -> Verdict {
    let (Some(f), Some(home)) = (refile.facts.get(&m.message_id), m.home.as_ref()) else {
        return Verdict::OutOfScope;
    };
    if input.folders.get(&home.folder).is_none_or(|v| v.is_source) {
        return Verdict::OutOfScope;
    }
    if let Some(skip) = placement_skip(f, &refile.frozen) {
        return Verdict::Skipped(skip);
    }
    let e = &m.effective;
    if !e.current {
        return Verdict::Waiting;
    }
    if e.input_incomplete {
        return Verdict::Skipped(Skip::IncompleteInput);
    }
    let Some(category) = e.category_id.clone() else {
        return Verdict::Skipped(Skip::TargetUnusable);
    };
    let to = match input.categories.get(&category) {
        Some(CategoryFolder::Native(native)) => native.clone(),
        Some(CategoryFolder::Inbox) => return Verdict::Skipped(Skip::TargetInboxOrSource),
        // A configured category the folder map leaves out files into a source.
        None if refile.categories.contains(&category) => {
            return Verdict::Skipped(Skip::TargetInboxOrSource)
        }
        None => return Verdict::Skipped(Skip::TargetUnusable),
    };
    if to == home.folder {
        return Verdict::InPlace;
    }
    if input.folders.get(&to).is_some_and(|v| v.is_source) {
        return Verdict::Skipped(Skip::TargetInboxOrSource);
    }
    if !target_usable(input, &to) {
        return Verdict::Skipped(Skip::TargetUnusable);
    }
    let reason = if in_category_folder(input, &home.folder) {
        Reason::CategoryChanged
    } else {
        Reason::FolderRetired
    };
    Verdict::Candidate(Candidate {
        target: to,
        category,
        reason,
    })
}

/// Whether some configured category files into native `folder`.
pub fn in_category_folder(input: &PlanInput, folder: &str) -> bool {
    input
        .categories
        .values()
        .any(|c| matches!(c, CategoryFolder::Native(n) if n == folder))
}
```

Add to `impl Store` in `src/filing/store.rs` (change the import to `use std::collections::{BTreeMap, BTreeSet};`):

```rust
    /// Recorded occurrences per message.
    pub fn occurrence_counts(&self, account: &str) -> Result<BTreeMap<String, usize>> {
        let mut st = self.db.prepare(
            "SELECT message_id, COUNT(*) FROM occurrences WHERE account=? GROUP BY message_id",
        )?;
        let rows = st
            .query_map([account], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as usize)))?
            .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
        Ok(rows)
    }

    /// Refile spec "Retired folders": retired folders neither retained nor
    /// draining (`drain_until_uid` NULL).
    pub fn frozen_folders(&self, account: &str) -> Result<BTreeSet<String>> {
        let mut st = self.db.prepare(
            "SELECT native FROM folders WHERE account=? AND state='retired' AND drain_until_uid IS NULL",
        )?;
        let rows = st
            .query_map([account], |r| r.get(0))?
            .collect::<rusqlite::Result<BTreeSet<_>>>()?;
        Ok(rows)
    }
```

- [ ] **Step 4: The planner rule (`src/filing/planner.rs`)**

Change the imports to:

```rust
use super::refile::rules::{self, RefileInput, Verdict};
use crate::domain::{FilingMode, Urgency};
use chrono::{DateTime, Utc};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
};
```

Add to `pub struct Plan` (after `satisfied_flags`):

```rust
    /// Refile spec: messages whose `Move` consumes their refile mark.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub refile_moves: BTreeSet<String>,
```

Change `enum MoveDecision` to:

```rust
enum MoveDecision {
    Move(Action),
    /// A move that consumes the message's refile mark.
    Refile(Action),
    Clear,
    Nothing,
}
```

Replace `pub fn plan` with:

```rust
pub fn plan(input: &PlanInput) -> Plan {
    plan_with_refile(input, &RefileInput::default())
}

/// `plan` plus the refile rule (refile spec "Each pass", Planning): a marked
/// message whose candidate rules hold moves to its effective category's
/// folder, after the explicit-target rule and before the source-folder rule.
pub fn plan_with_refile(input: &PlanInput, refile: &RefileInput) -> Plan {
    let mut out = Plan::default();
    if input.mode == FilingMode::Off {
        return out;
    }
    if input.preview {
        out.folders_to_create = input
            .folders
            .iter()
            .filter(|(_, f)| f.is_category && f.usable == FolderUse::WouldCreate)
            .map(|(name, _)| name.clone())
            .collect();
    }
    let mut per_message = Vec::new();
    for m in &input.messages {
        let Some((home, home_view)) = eligible_home(input, m) else {
            continue;
        };
        let mut actions = Vec::new();
        match flag_action(input, m, home, home_view) {
            FlagDecision::Flag(flag) => actions.push(flag),
            FlagDecision::Satisfied => out.satisfied_flags.push(m.message_id.clone()),
            FlagDecision::Nothing => {}
        }
        match move_action(input, m, home, home_view, refile) {
            MoveDecision::Move(a) => actions.push(a),
            MoveDecision::Refile(a) => {
                out.refile_moves.insert(m.message_id.clone());
                actions.push(a);
            }
            MoveDecision::Clear => out
                .cleared_requests
                .push((m.message_id.clone(), m.desired_rev)),
            MoveDecision::Nothing => {}
        }
        if !actions.is_empty() {
            per_message.push((m.internal_date, m.message_id.as_str(), actions));
        }
    }
    per_message.sort_by(|a, b| {
        let by_date = match (a.0, b.0) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        };
        by_date.then_with(|| a.1.cmp(b.1))
    });
    out.actions = per_message
        .into_iter()
        .flat_map(|(_, _, a)| a)
        .take(input.max_actions)
        .collect();
    let kept: BTreeSet<&str> = out
        .actions
        .iter()
        .filter(|a| matches!(a, Action::Move { .. }))
        .map(Action::message_id)
        .collect();
    out.refile_moves.retain(|id| kept.contains(id.as_str()));
    out
}
```

Change `fn target_usable` to `pub(crate) fn target_usable`.

Replace `fn move_action` with:

```rust
fn move_action(
    input: &PlanInput,
    m: &PlanMessage,
    home: &Locator,
    home_view: &FolderView,
    refile: &RefileInput,
) -> MoveDecision {
    if m.open_move_intent {
        return MoveDecision::Nothing;
    }
    if let Some(target) = &m.desired_target {
        let Some(to) = resolve_target(input, m, target) else {
            return MoveDecision::Nothing;
        };
        if to == home.folder {
            return MoveDecision::Clear;
        }
        let home_ok = home_view.is_source
            || (home_view.is_category && home_view.usable == FolderUse::Ok)
            || home_view.retired_listed;
        if !home_ok || !target_usable(input, &to) {
            return MoveDecision::Nothing;
        }
        return MoveDecision::Move(Action::Move {
            message_id: m.message_id.clone(),
            from: home.clone(),
            to,
            desired_rev: m.desired_rev,
            consumes_eligible: false,
        });
    }
    // Refile: a marked message outside the source folders, with the home an
    // explicit move needs; it waits while any candidate rule fails.
    let marked = refile.facts.get(&m.message_id).is_some_and(|f| f.marked);
    if marked && !home_view.is_source {
        let home_ok = (home_view.is_category && home_view.usable == FolderUse::Ok)
            || home_view.retired_listed;
        return match rules::verdict(input, m, refile) {
            Verdict::Candidate(c) if home_ok => MoveDecision::Refile(Action::Move {
                message_id: m.message_id.clone(),
                from: home.clone(),
                to: c.target,
                desired_rev: m.desired_rev,
                consumes_eligible: false,
            }),
            _ => MoveDecision::Nothing,
        };
    }
    if !home_view.is_source || m.pinned {
        return MoveDecision::Nothing;
    }
    let e = &m.effective;
    let Some(category) = &e.category_id else {
        return MoveDecision::Nothing;
    };
    if !e.category_from_override && (!e.current || e.input_incomplete) {
        return MoveDecision::Nothing;
    }
    let Some(CategoryFolder::Native(to)) = input.categories.get(category) else {
        return MoveDecision::Nothing;
    };
    if !(is_new(input, m) || m.eligible_once) || *to == home.folder || !target_usable(input, to) {
        return MoveDecision::Nothing;
    }
    MoveDecision::Move(Action::Move {
        message_id: m.message_id.clone(),
        from: home.clone(),
        to: to.clone(),
        desired_rev: m.desired_rev,
        consumes_eligible: m.eligible_once,
    })
}
```

- [ ] **Step 5: Refile claims (`src/filing/apply.rs`)**

Add `use std::collections::BTreeSet;`. In `apply`, change the move call to
`move_batch(store, ctx, (folder, *epoch, to), chunk, &plan.refile_moves, &mut dropped, summary)`.
Change `move_batch`'s signature and claim loop:

```rust
/// One move batch: verify, snapshot the target, claim (a refile move as
/// one), dispatch.
fn move_batch(
    store: &mut Store,
    ctx: &PassContext,
    (folder, epoch, to): (&str, u64, &str),
    actions: &[&Action],
    refile: &BTreeSet<String>,
    dropped: &mut Vec<String>,
    summary: &mut FilingSummary,
) -> Result<()> {
```

and replace its `for action in verified.kept { … }` loop with:

```rust
    for action in verified.kept {
        let consumes_refile = refile.contains(action.message_id());
        let target_snapshot = (target.uid_validity, target.uid_next);
        let claim = store.claim_move_with(
            ctx.account,
            action,
            target_snapshot,
            &batch,
            &at,
            consumes_refile,
        )?;
        if let Some(id) = claim {
            claimed.push((id, locator(action).uid));
        }
    }
```

- [ ] **Step 6: The pass and `filing plan` (`src/service.rs`)**

Add `refile,` to the `filing::{…}` import list. In `plan_and_apply`, replace the two lines building `plan` with:

```rust
            let plan = filing::refile::plan_pass(store, ctx, map, preview)?;
```

In `filing_plan`, replace `planner::plan(&inputs::plan_input(&self.store, &ctx, &map, true)?)` with:

```rust
            let input = inputs::plan_input(&self.store, &ctx, &map, true)?;
            let gone = refile::rules::gone(&self.store, name, &map.listed)?;
            let facts = refile::rules::input(&self.store, name, &account, &gone)?;
            planner::plan_with_refile(&input, &facts)
```

and replace `let shown = &plan.actions[..plan.actions.len().min(limit)];` with:

```rust
        let shown: Vec<Value> = plan.actions[..plan.actions.len().min(limit)]
            .iter()
            .map(|a| plan_action(&plan, a))
            .collect();
```

Add the free function next to `open_moves`:

```rust
/// A `filing plan` action; a refile move carries `"reason": "refile"`.
fn plan_action(plan: &Plan, action: &planner::Action) -> Value {
    let mut value = json!(action);
    if matches!(action, planner::Action::Move { .. })
        && plan.refile_moves.contains(action.message_id())
    {
        value["reason"] = json!("refile");
    }
    value
}
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test --locked --lib planner && cargo test --locked --lib refile && cargo test --locked --test refile_planning && cargo test --locked --test filing_writes --test filing_cli`
Expected: all PASS; the existing planner and filing tests are unchanged.

- [ ] **Step 8: Full check and commit**

Run: `cargo fmt && cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`

```bash
git add src/filing/mod.rs src/filing/refile/mod.rs src/filing/refile/rules.rs src/filing/planner.rs src/filing/apply.rs src/filing/store.rs src/service.rs tests/refile_planning.rs
git commit -m "Plan refile moves for marked mail whose candidate rules hold

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Mark upkeep and refile intent checks

**Files:**
- Create: `src/filing/refile/upkeep.rs`, `src/filing/refile/intents.rs`, `tests/refile_marks.rs`
- Modify: `src/filing/refile/mod.rs` (modules, `plan_pass` runs upkeep)
- Modify: `src/filing/refile/rules.rs` (`input_for`)
- Modify: `src/filing/inputs.rs` (`plan_input` split, new `message_input`)
- Modify: `src/filing/store.rs` (`records_for_planning` split, new `record_for_planning`)
- Modify: `src/filing/apply.rs` (`move_batch` takes the map and checks refile claims)
- Modify: `src/filing/recover.rs` (`retry_or_supersede` checks refile intents)

**Interfaces:**
- Consumes (Task 3): `rules::{RefileInput, RefileFacts, Verdict, gone, input, verdict}`, `Plan.refile_moves`, `plan_pass`; test support from Task 2.
- Produces:
  - `refile::upkeep::run(store: &mut Store, ctx: &PassContext, map: &FolderMap, input: &PlanInput, refile: &mut RefileInput) -> Result<()>`.
  - `refile::intents::recheck(store: &Store, ctx: &PassContext, map: &FolderMap, intent: &Intent, source: &Locator) -> Result<Option<&'static str>>`; `refile::intents::cancel(store: &mut Store, ctx: &PassContext, intent: &Intent, reason: &str) -> Result<()>`.
  - `rules::input_for(store: &Store, account: &str, cfg: &AccountConfig, gone: &BTreeSet<String>, id: &str, exclude: i64) -> Result<RefileInput>`.
  - `inputs::message_input(store: &Store, ctx: &PassContext, map: &FolderMap, id: &str) -> Result<PlanInput>`.
  - `Store::record_for_planning(&self, account: &str, id: &str) -> Result<Option<(Record, Placement, MessageMeta)>>`.
  - `plan_pass(store: &mut Store, …)`; events `refile_cleared {reason}`, `refile_cancelled {intent_id, reason}`.

- [ ] **Step 1: Write the failing tests**

Create `src/filing/refile/upkeep.rs` with only its test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::{clear_reason, Target};
    use crate::filing::planner::Effective;
    use crate::filing::refile::rules::RefileFacts;

    fn marked() -> RefileFacts {
        RefileFacts {
            marked: true,
            at_filed_home: true,
            home_folder: Some("Other".into()),
            single_occurrence: true,
            ..Default::default()
        }
    }

    fn current() -> Effective {
        Effective {
            category_id: Some("updates".into()),
            current: true,
            ..Default::default()
        }
    }

    #[test]
    fn placement_reasons_clear_in_the_stated_order() {
        let mut f = marked();
        f.single_occurrence = false;
        assert_eq!(clear_reason(&f, &current(), Target::Elsewhere, false), Some("multiple_copies"));
        f.at_filed_home = false;
        assert_eq!(
            clear_reason(&f, &current(), Target::Elsewhere, false),
            Some("not_filed_by_mailtriage")
        );
        f.pinned = true;
        assert_eq!(clear_reason(&f, &current(), Target::Elsewhere, false), Some("pinned"));
        f.corrected = true;
        assert_eq!(clear_reason(&f, &current(), Target::Elsewhere, false), Some("corrected"));
        f.done = true;
        assert_eq!(clear_reason(&f, &current(), Target::Elsewhere, false), Some("done"));
    }

    #[test]
    fn an_open_refile_intent_or_a_running_rescan_keeps_the_mark() {
        let mut f = marked();
        f.done = true;
        f.open_move_intent = true;
        f.open_refile_intent = true;
        assert_eq!(clear_reason(&f, &current(), Target::InPlace, false), None);
        f.open_refile_intent = false;
        assert_eq!(clear_reason(&f, &current(), Target::InPlace, true), None);
        assert_eq!(
            clear_reason(&f, &current(), Target::InPlace, false),
            Some("done"),
            "another open move intent does not defer upkeep"
        );
    }

    #[test]
    fn only_a_current_classification_clears_by_where_it_files_the_message() {
        let f = marked();
        let mut stale = current();
        stale.current = false;
        stale.input_incomplete = true;
        for target in [Target::InPlace, Target::InboxOrSource, Target::Elsewhere] {
            assert_eq!(clear_reason(&f, &stale, target, false), None, "{target:?}");
        }
        assert_eq!(clear_reason(&f, &current(), Target::InPlace, false), Some("in_place"));
        assert_eq!(
            clear_reason(&f, &current(), Target::InboxOrSource, false),
            Some("target_inbox_or_source")
        );
        let mut partial = current();
        partial.input_incomplete = true;
        assert_eq!(clear_reason(&f, &partial, Target::Elsewhere, false), Some("incomplete_input"));
        assert_eq!(clear_reason(&f, &current(), Target::Elsewhere, false), None);
        assert_eq!(clear_reason(&f, &current(), Target::Unknown, false), None);
    }
}
```

Create `tests/refile_marks.rs`:

```rust
//! Refile spec "Each pass" (mark upkeep) and "Intents".
mod common;
mod refile_support;
use common::{mail, Harness};
use mailtriage::{
    domain::FilingMode::Live,
    engine::fake::{FakeOp, Fault},
    filing::{
        apply, inputs, observe, planner,
        refile::{rules, upkeep},
        FilingSummary, PassContext,
    },
    service::RetryTarget,
};
use refile_support::{
    add_category, correspondence_category, deliver_filed, events, filed_in_other_now_updates,
    located, mark, moves_to, pause, placement, review_state, set_catch_all, updates_category,
    without,
};
use serde_json::json;

#[test]
fn upkeep_clears_marks_with_their_reason() {
    type Act = fn(&Harness, &str);
    let cases: [(&str, Act); 5] = [
        ("done", |h, id| {
            h.service().review("work", id, true).unwrap();
        }),
        ("pinned", |h, id| {
            h.service().filing_pin("work", id).unwrap();
        }),
        ("corrected", |h, id| {
            h.service()
                .correct("work", id, json!({"category_id": "transactions"}), None)
                .unwrap();
        }),
        ("multiple_copies", |h, _| {
            let (folder, uid) = located(h, "u");
            h.fake.client_copy(&folder, uid, "Transactions");
        }),
        ("not_filed_by_mailtriage", |h, _| h.fake.reset_epoch("Other")),
    ];
    for (reason, act) in cases {
        let h = Harness::new(Live);
        let id = filed_in_other_now_updates(&h);
        mark(&h, &id);
        act(&h, &id);
        for _ in 0..4 {
            h.sync();
        }
        assert!(!placement(&h, &id).refile_once, "{reason}");
        assert_eq!(events(&h, "refile_cleared")[0]["detail"]["reason"], reason);
        assert_eq!(moves_to(&h, "Updates"), 0, "{reason}");
    }
}

#[test]
fn absent_then_done_clears_the_mark() {
    let h = Harness::new(Live);
    let id = filed_in_other_now_updates(&h);
    let (folder, uid) = located(&h, "u");
    h.fake.client_delete(&folder, uid);
    mark(&h, &id);
    for _ in 0..3 {
        h.sync();
    }
    assert!(!placement(&h, &id).refile_once);
    assert_eq!(
        events(&h, "refile_cleared")[0]["detail"]["reason"],
        "not_filed_by_mailtriage"
    );
    assert_eq!(moves_to(&h, "Updates"), 0);
    assert_eq!(review_state(&h, &id), "done", "done inference still runs");
}

#[test]
fn a_stale_classification_keeps_the_mark_until_the_current_one_decides() {
    let h = Harness::new(Live);
    without(&h, &["updates"]);
    h.sync();
    for i in 0..3 {
        h.fake.deliver(
            "INBOX",
            &mail(&format!("f{i}"), &format!("Lunch {i}"), "See you at noon"),
        );
    }
    h.sync();
    let id = deliver_filed(&h, "u", "System update", "Version 2 is out");
    // Open mail is queued again; u comes after the three others.
    add_category(&h, updates_category());
    // Its stale classification still says `other`: its own folder.
    mark(&h, &id);
    for pass in 0..3 {
        h.service().sync("work", 1).unwrap();
        assert!(placement(&h, &id).refile_once, "pass {pass}");
        assert!(events(&h, "refile_cleared").is_empty(), "pass {pass}");
    }
    h.service().sync("work", 1).unwrap(); // u is classified again: `updates`
    h.sync();
    assert_eq!(located(&h, "u").0, "Updates");
    assert!(!placement(&h, &id).refile_once);
}

#[test]
fn a_client_correction_after_marking_wins() {
    let h = Harness::new(Live);
    let id = filed_in_other_now_updates(&h);
    mark(&h, &id);
    let (folder, uid) = located(&h, "u");
    h.fake.client_move(&folder, uid, "Transactions");
    h.sync();
    h.sync();
    assert_eq!(located(&h, "u").0, "Transactions");
    assert_eq!(moves_to(&h, "Updates"), 0);
    assert!(!placement(&h, &id).refile_once);
    assert_eq!(events(&h, "refile_cleared")[0]["detail"]["reason"], "corrected");
}

#[test]
fn an_inbox_target_clears_the_mark() {
    let h = Harness::new(Live);
    without(&h, &["correspondence"]);
    h.sync();
    let id = deliver_filed(&h, "l", "Lunch", "See you at noon");
    assert_eq!(located(&h, "l").0, "Other");
    add_category(&h, correspondence_category());
    h.sync();
    mark(&h, &id);
    h.sync();
    h.sync();
    assert_eq!(located(&h, "l").0, "Other", "never refiled into INBOX");
    assert_eq!(moves_to(&h, "INBOX"), 0);
    assert_eq!(
        events(&h, "refile_cleared")[0]["detail"]["reason"],
        "target_inbox_or_source"
    );
}

#[test]
fn a_paused_target_or_home_keeps_the_mark_until_released() {
    for paused in ["Updates", "Other"] {
        let h = Harness::new(Live);
        let id = filed_in_other_now_updates(&h);
        pause(&h, paused);
        mark(&h, &id);
        h.sync();
        h.sync();
        assert!(placement(&h, &id).refile_once, "{paused}");
        assert_eq!(moves_to(&h, "Updates"), 0, "{paused}");
        h.service()
            .filing_retry("work", RetryTarget::Folder(paused.into()))
            .unwrap();
        for _ in 0..3 {
            h.sync();
        }
        assert_eq!(located(&h, "u").0, "Updates", "{paused}");
        assert!(!placement(&h, &id).refile_once, "{paused}");
    }
}

#[test]
fn done_between_planning_and_claim_supersedes_the_refile_intent() {
    let h = Harness::new(Live);
    let id = filed_in_other_now_updates(&h);
    mark(&h, &id);
    let mut s = h.service();
    let cfg = s.config.accounts["work"].clone();
    let generation = s.store.records("work").unwrap()[0].generation.clone();
    let verify = || -> anyhow::Result<()> { Ok(()) };
    let ctx = PassContext {
        account: "work",
        cfg: &cfg,
        engine: &h.fake,
        mode: Live,
        generation: &generation,
        now: mailtriage::store::now(),
        max_attempts: 5,
        verify_binding: &verify,
    };
    let mut summary = FilingSummary::default();
    let map = observe::resolve_folders(&mut s.store, &ctx, &mut summary).unwrap();
    let input = inputs::plan_input(&s.store, &ctx, &map, false).unwrap();
    let gone = rules::gone(&s.store, "work", &map.listed).unwrap();
    let refile = rules::input(&s.store, "work", &cfg, &gone).unwrap();
    let plan = planner::plan_with_refile(&input, &refile);
    assert!(plan.refile_moves.contains(&id));
    s.store.review("work", &id, true).unwrap();
    apply::apply(&mut s.store, &ctx, &map, &plan, &mut summary).unwrap();
    assert_eq!(moves_to(&h, "Updates"), 0, "never dispatched");
    let intent = s.store.intents("work", false).unwrap().pop().unwrap();
    assert_eq!((intent.state.as_str(), intent.consumes_refile), ("superseded", true));
    assert_eq!(events(&h, "refile_cancelled")[0]["detail"]["reason"], "done");
    drop(s);
    h.sync();
    assert!(!placement(&h, &id).refile_once, "upkeep clears the mark of done mail");
    assert_eq!(moves_to(&h, "Updates"), 0);
}

#[test]
fn a_category_change_after_a_failed_dispatch_supersedes_and_retargets() {
    let h = Harness::new(Live);
    without(&h, &["correspondence"]);
    h.sync();
    let id = deliver_filed(&h, "l", "Lunch", "See you at noon"); // the catch-all: Other
    set_catch_all(&h, "updates");
    h.sync(); // now `updates`
    mark(&h, &id);
    h.fake.inject(FakeOp::Move, Fault::ErrorBefore);
    h.sync(); // the dispatch to Updates fails
    assert_eq!(moves_to(&h, "Updates"), 1);
    assert_eq!(located(&h, "l").0, "Other");
    set_catch_all(&h, "promotions");
    h.sync();
    h.sync();
    assert_eq!(located(&h, "l").0, "Promotions");
    assert_eq!(moves_to(&h, "Updates"), 1, "the intent to Updates is never retried");
    let refiles: Vec<(String, Option<String>)> = h
        .service()
        .store
        .intents("work", false)
        .unwrap()
        .into_iter()
        .filter(|i| i.consumes_refile)
        .map(|i| (i.state, i.target))
        .collect();
    assert_eq!(
        refiles,
        vec![
            ("superseded".to_string(), Some("Updates".to_string())),
            ("applied".to_string(), Some("Promotions".to_string())),
        ]
    );
    assert_eq!(events(&h, "refile_cancelled")[0]["detail"]["reason"], "waiting");
    assert!(!placement(&h, &id).refile_once);
}

#[test]
fn a_changed_source_occurrence_supersedes_the_intent() {
    let h = Harness::new(Live);
    let id = filed_in_other_now_updates(&h);
    mark(&h, &id);
    h.fake.inject(FakeOp::Move, Fault::ErrorBefore);
    h.sync(); // the dispatch fails; the intent is uncertain
    h.fake.reset_epoch("Other"); // the source occurrence changes
    for _ in 0..6 {
        h.sync();
    }
    assert_eq!(moves_to(&h, "Updates"), 1, "never retried");
    let s = h.service();
    let intent = s
        .store
        .intents("work", false)
        .unwrap()
        .into_iter()
        .find(|i| i.consumes_refile)
        .unwrap();
    assert_eq!(intent.message_id, id);
    assert_eq!(intent.state, "superseded");
    assert_eq!(
        events(&h, "refile_cancelled")[0]["detail"]["reason"],
        "source_changed"
    );
}

#[test]
fn a_pass_without_a_folder_map_clears_no_mark() {
    let h = Harness::new(Live);
    let id = filed_in_other_now_updates(&h);
    h.service().review("work", &id, true).unwrap(); // a reason to clear
    mark(&h, &id);
    let mut s = h.service();
    let cfg = s.config.accounts["work"].clone();
    let generation = s.store.records("work").unwrap()[0].generation.clone();
    let verify = || -> anyhow::Result<()> { Ok(()) };
    let ctx = PassContext {
        account: "work",
        cfg: &cfg,
        engine: &h.fake,
        mode: Live,
        generation: &generation,
        now: mailtriage::store::now(),
        max_attempts: 5,
        verify_binding: &verify,
    };
    let map = observe::FolderMap::sources_only(&cfg); // resolution failed
    let input = inputs::plan_input(&s.store, &ctx, &map, false).unwrap();
    let gone = rules::gone(&s.store, "work", &map.listed).unwrap();
    let mut refile = rules::input(&s.store, "work", &cfg, &gone).unwrap();
    upkeep::run(&mut s.store, &ctx, &map, &input, &mut refile).unwrap();
    assert!(s.store.placement("work", &id).unwrap().unwrap().refile_once);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --locked --lib upkeep && cargo test --locked --test refile_marks`
Expected: compile errors — module `upkeep` not declared, `clear_reason`/`Target`/`run` not found.

- [ ] **Step 3: Single-message planning input (`src/filing/store.rs`, `src/filing/inputs.rs`)**

In `src/filing/store.rs` replace `records_for_planning` with:

```rust
    /// Every placement with its message (without content) and transport
    /// metadata, in message id order.
    pub fn records_for_planning(
        &self,
        account: &str,
    ) -> Result<Vec<(Record, Placement, MessageMeta)>> {
        self.planning_rows(account, None)
    }

    /// `records_for_planning` for one message.
    pub fn record_for_planning(
        &self,
        account: &str,
        id: &str,
    ) -> Result<Option<(Record, Placement, MessageMeta)>> {
        Ok(self.planning_rows(account, Some(id))?.pop())
    }

    fn planning_rows(
        &self,
        account: &str,
        id: Option<&str>,
    ) -> Result<Vec<(Record, Placement, MessageMeta)>> {
        let placement: Vec<String> = PLACEMENT_COLUMNS
            .split(',')
            .map(|c| format!("p.{c}"))
            .collect();
        let mut st = self.db.prepare(&format!(
            "SELECT m.id,m.account,NULL,m.envelope,m.status,m.classification,m.overrides,m.review_state,m.observed_at,m.error,m.generation,{},
 m.rfc_message_id,m.size,m.internal_date,m.fingerprint IS NOT NULL,m.source_managed
 FROM placements p JOIN messages m ON m.id=p.message_id WHERE p.account=?1 AND (?2 IS NULL OR p.message_id=?2) ORDER BY p.message_id",
            placement.join(",")
        ))?;
        let rows = st
            .query_map(params![account, id], |r| {
                let record = row_record(r)?;
                let placement = row_placement_at(r, 11)?;
                let meta = MessageMeta {
                    rfc_message_id: r.get(33)?,
                    size: r.get(34)?,
                    internal_date: r.get(35)?,
                    flags: envelope_flags(&record.envelope),
                    fingerprinted: r.get(36)?,
                    source_managed: r.get(37)?,
                };
                Ok((record, placement, meta))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
```

In `src/filing/inputs.rs` replace `plan_input` with:

```rust
/// Everything the pure planner needs, read from the store and this pass's
/// folder map. Categories come from `map.categories`, which leaves out a
/// category whose folder collides with a source folder.
pub fn plan_input(
    store: &Store,
    ctx: &PassContext,
    map: &FolderMap,
    preview: bool,
) -> Result<PlanInput> {
    let rows = store.records_for_planning(ctx.account)?;
    input_of(store, ctx, map, preview, rows)
}

/// `plan_input` with only the placement of `id` (refile intent checks).
pub fn message_input(
    store: &Store,
    ctx: &PassContext,
    map: &FolderMap,
    id: &str,
) -> Result<PlanInput> {
    let rows = store.record_for_planning(ctx.account, id)?.into_iter().collect();
    input_of(store, ctx, map, false, rows)
}

fn input_of(
    store: &Store,
    ctx: &PassContext,
    map: &FolderMap,
    preview: bool,
    rows: Vec<(Record, Placement, MessageMeta)>,
) -> Result<PlanInput> {
    let open: BTreeSet<(String, String)> = store
        .intents(ctx.account, true)?
        .into_iter()
        .map(|i| (i.message_id, i.kind))
        .collect();
    let has_open = |id: &str, kind: &str| open.contains(&(id.to_string(), kind.to_string()));
    let messages = rows
        .into_iter()
        .map(|(record, p, meta)| {
            let mut m = plan_message(&p, &meta, effective(&record, ctx.generation));
            m.open_move_intent = has_open(&p.message_id, "move");
            m.open_flag_intent = has_open(&p.message_id, "flag");
            m
        })
        .collect();
    Ok(PlanInput {
        mode: ctx.mode,
        preview,
        flag_enabled: ctx.cfg.filing.flag,
        max_actions: ctx.cfg.filing.max_actions_per_pass,
        enabled_at: enabled_second(store.filing_state(ctx.account)?.enabled_at.as_deref()),
        categories: map.categories.clone(),
        folders: folder_views(store, ctx, map)?,
        messages,
    })
}
```

In `src/filing/refile/rules.rs` add after `input`:

```rust
/// `input` for the placement of `id` only; its open move intent `exclude`
/// (the refile intent being checked) does not count.
pub fn input_for(
    store: &Store,
    account: &str,
    cfg: &AccountConfig,
    gone: &BTreeSet<String>,
    id: &str,
    exclude: i64,
) -> Result<RefileInput> {
    let rows: Vec<_> = store.record_for_planning(account, id)?.into_iter().collect();
    let counts = BTreeMap::from([(id.to_string(), store.occurrences_of(account, id)?.len())]);
    build(store, account, cfg, gone, rows, &counts, Some(exclude))
}
```

- [ ] **Step 4: Mark upkeep (`src/filing/refile/upkeep.rs`, `src/filing/refile/mod.rs`)**

Put this above the test module in `upkeep.rs`:

```rust
//! Refile spec "Each pass", mark upkeep: before planning, every marked
//! placement keeps its mark, or loses it (event `refile_cleared` with the
//! reason) once it can no longer move.
use crate::filing::apply::{commit_with_placement, event};
use crate::filing::observe::FolderMap;
use crate::filing::planner::{CategoryFolder, Effective, PlanInput, PlanMessage};
use crate::filing::refile::rules::{RefileFacts, RefileInput};
use crate::filing::PassContext;
use crate::store::Store;
use anyhow::Result;
use serde_json::json;
use std::collections::BTreeSet;

/// Clears the marks steps 3–4 decide, in input order, and updates `refile`
/// so planning sees them cleared. A pass without a folder map (resolution
/// failed) clears nothing: every home would look gone.
pub fn run(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    input: &PlanInput,
    refile: &mut RefileInput,
) -> Result<()> {
    if map.caps.is_none() {
        return Ok(());
    }
    let rescanning: BTreeSet<String> = store
        .folder_records(ctx.account)?
        .into_iter()
        .filter(|r| r.rescan_epoch.is_some() && !r.rescan_complete)
        .map(|r| r.native)
        .collect();
    let mut cleared = Vec::new();
    for m in &input.messages {
        let Some(f) = refile.facts.get(&m.message_id).filter(|f| f.marked) else {
            continue;
        };
        let home_rescanning = f.home_folder.as_ref().is_some_and(|h| rescanning.contains(h));
        let target = target_of(input, refile, m, f);
        let Some(reason) = clear_reason(f, &m.effective, target, home_rescanning) else {
            continue;
        };
        clear(store, ctx, &m.message_id, f.home_folder.as_deref(), reason)?;
        cleared.push(m.message_id.clone());
    }
    for id in cleared {
        if let Some(f) = refile.facts.get_mut(&id) {
            f.marked = false;
        }
    }
    Ok(())
}

/// Where the effective category files the message, from its recorded home.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    InPlace,
    InboxOrSource,
    Elsewhere,
    Unknown,
}

fn target_of(input: &PlanInput, refile: &RefileInput, m: &PlanMessage, f: &RefileFacts) -> Target {
    let Some(category) = m.effective.category_id.as_deref() else {
        return Target::Unknown;
    };
    match input.categories.get(category) {
        Some(CategoryFolder::Inbox) => Target::InboxOrSource,
        Some(CategoryFolder::Native(native)) if f.home_folder.as_deref() == Some(native.as_str()) => {
            Target::InPlace
        }
        Some(CategoryFolder::Native(native))
            if input.folders.get(native).is_some_and(|v| v.is_source) =>
        {
            Target::InboxOrSource
        }
        Some(CategoryFolder::Native(_)) => Target::Elsewhere,
        // A configured category the folder map leaves out files into a source.
        None if refile.categories.contains(category) => Target::InboxOrSource,
        None => Target::Unknown,
    }
}

/// Upkeep steps 1–5 for one marked placement; `None` keeps the mark.
fn clear_reason(
    f: &RefileFacts,
    e: &Effective,
    target: Target,
    home_rescanning: bool,
) -> Option<&'static str> {
    // 1–2: an open refile intent is resolved first; a rescan may find the home.
    if f.open_refile_intent || home_rescanning {
        return None;
    }
    // 3: reasons that do not depend on the classification.
    if f.done {
        return Some("done");
    }
    if f.corrected {
        return Some("corrected");
    }
    if f.pinned {
        return Some("pinned");
    }
    if !f.at_filed_home {
        return Some("not_filed_by_mailtriage");
    }
    if !f.single_occurrence {
        return Some("multiple_copies");
    }
    // 4: a stale classification never clears a mark.
    if !e.current {
        return None;
    }
    match target {
        Target::InPlace => Some("in_place"),
        Target::InboxOrSource => Some("target_inbox_or_source"),
        _ if e.input_incomplete => Some("incomplete_input"),
        _ => None,
    }
}

/// Clears the mark (a new revision) with event `refile_cleared`, in one transaction.
fn clear(
    store: &mut Store,
    ctx: &PassContext,
    id: &str,
    folder: Option<&str>,
    reason: &str,
) -> Result<()> {
    let writes = [event(Some(id), folder, "refile_cleared", json!({"reason": reason}))];
    commit_with_placement(
        store,
        ctx,
        id,
        |p| {
            if p.refile_once {
                p.refile_once = false;
                p.desired_rev += 1;
            }
        },
        &writes,
    )
}
```

In `src/filing/refile/mod.rs` add `pub mod intents;` and `pub mod upkeep;` after `pub mod rules;`, and replace `plan_pass` with:

```rust
/// Step 9's plan: the planner input and every placement's refile facts,
/// mark upkeep, then the planner; marked messages get the refile rule.
pub fn plan_pass(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    preview: bool,
) -> Result<Plan> {
    let input = inputs::plan_input(store, ctx, map, preview)?;
    let gone = rules::gone(store, ctx.account, &map.listed)?;
    let mut refile = rules::input(store, ctx.account, ctx.cfg, &gone)?;
    upkeep::run(store, ctx, map, &input, &mut refile)?;
    Ok(planner::plan_with_refile(&input, &refile))
}
```

- [ ] **Step 5: Intent checks (`src/filing/refile/intents.rs`, `src/filing/apply.rs`, `src/filing/recover.rs`)**

Create `src/filing/refile/intents.rs`:

```rust
//! Refile spec "Intents": a refile intent is checked again at claim time and
//! before every retry once recovery established that no earlier dispatch
//! applied; a failed check closes it `superseded` (event `refile_cancelled`).
use crate::filing::apply::{close, commit, event};
use crate::filing::observe::FolderMap;
use crate::filing::planner::Locator;
use crate::filing::refile::rules::{self, Verdict};
use crate::filing::{inputs, Intent, PassContext};
use crate::store::Store;
use anyhow::Result;
use serde_json::json;

/// Why `intent` may no longer move its message from `source` (the validated
/// source occurrence), or `None` while every candidate rule holds (the
/// intent itself not counting as an open intent), the source is the
/// journaled one and the current classification still files the message
/// into the intent's target.
pub fn recheck(
    store: &Store,
    ctx: &PassContext,
    map: &FolderMap,
    intent: &Intent,
    source: &Locator,
) -> Result<Option<&'static str>> {
    let journaled = (intent.folder.as_str(), intent.epoch, intent.uid);
    if journaled != (source.folder.as_str(), source.epoch, source.uid) {
        return Ok(Some("source_changed"));
    }
    let input = inputs::message_input(store, ctx, map, &intent.message_id)?;
    let gone = rules::gone(store, ctx.account, &map.listed)?;
    let refile = rules::input_for(store, ctx.account, ctx.cfg, &gone, &intent.message_id, intent.id)?;
    let Some(m) = input.messages.first() else {
        return Ok(Some("not_filed_by_mailtriage"));
    };
    if m.home.as_ref() != Some(source) {
        return Ok(Some("source_changed"));
    }
    Ok(match rules::verdict(&input, m, &refile) {
        Verdict::Candidate(c) if intent.target.as_deref() == Some(c.target.as_str()) => None,
        Verdict::Candidate(_) => Some("target_changed"),
        Verdict::Waiting => Some("waiting"),
        Verdict::InPlace => Some("in_place"),
        Verdict::Skipped(skip) => Some(skip.as_str()),
        Verdict::OutOfScope => Some("not_filed_by_mailtriage"),
    })
}

/// Closes `intent` as `superseded` with event `refile_cancelled`, in one
/// transaction; the next pass's planning may create a new one.
pub fn cancel(store: &mut Store, ctx: &PassContext, intent: &Intent, reason: &str) -> Result<()> {
    let detail = json!({"intent_id": intent.id, "reason": reason});
    commit(
        store,
        ctx,
        &[
            close(intent.id, "superseded", None),
            event(
                Some(&intent.message_id),
                Some(&intent.folder),
                "refile_cancelled",
                detail,
            ),
        ],
    )
}
```

In `src/filing/apply.rs`: add `use super::refile;`. Change the move call in `apply` to
`move_batch(store, ctx, map, (folder, *epoch, to), chunk, &plan.refile_moves, &mut dropped, summary)`.
Change `move_batch` to take the map and check refile claims:

```rust
/// One move batch: verify, snapshot the target, claim (a refile move as
/// one, checked again right after its claim), dispatch.
#[allow(clippy::too_many_arguments)] // One move batch with its pass context.
fn move_batch(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    (folder, epoch, to): (&str, u64, &str),
    actions: &[&Action],
    refile: &BTreeSet<String>,
    dropped: &mut Vec<String>,
    summary: &mut FilingSummary,
) -> Result<()> {
```

with its claim loop:

```rust
    for action in verified.kept {
        let consumes_refile = refile.contains(action.message_id());
        let target_snapshot = (target.uid_validity, target.uid_next);
        let claim = store.claim_move_with(
            ctx.account,
            action,
            target_snapshot,
            &batch,
            &at,
            consumes_refile,
        )?;
        let Some(id) = claim else {
            continue;
        };
        if consumes_refile && refile_cancelled(store, ctx, map, id, locator(action))? {
            continue;
        }
        claimed.push((id, locator(action).uid));
    }
```

and add after `move_batch`:

```rust
/// Refile spec "Intents", at claim time: a refile intent that fails its
/// check is superseded before dispatch.
fn refile_cancelled(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    id: i64,
    from: &Locator,
) -> Result<bool> {
    let Some(intent) = store.intent(id)? else {
        return Ok(false);
    };
    match refile::intents::recheck(store, ctx, map, &intent, from)? {
        Some(reason) => {
            refile::intents::cancel(store, ctx, &intent, reason)?;
            Ok(true)
        }
        None => Ok(false),
    }
}
```

In `src/filing/recover.rs` add `use super::refile;` and, in `retry_or_supersede`, directly after the block that returns `superseded` for a moved-on revision or a block, insert:

```rust
    // Refile spec "Intents": before every retry a refile intent is checked again.
    if intent.consumes_refile && map.caps.is_some() {
        if let Some(reason) = refile::intents::recheck(store, ctx, map, intent, &from)? {
            return refile::intents::cancel(store, ctx, intent, reason);
        }
    }
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --locked --lib upkeep && cargo test --locked --test refile_marks && cargo test --locked --test refile_planning --test filing_writes --test filing_location`
Expected: all PASS.

- [ ] **Step 7: Full check and commit**

Run: `cargo fmt && cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`

```bash
git add src/filing/refile/mod.rs src/filing/refile/upkeep.rs src/filing/refile/intents.rs src/filing/refile/rules.rs src/filing/inputs.rs src/filing/store.rs src/filing/apply.rs src/filing/recover.rs tests/refile_marks.rs
git commit -m "Keep refile marks only while they can move, and recheck refile intents

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: The `filing refile` preview, status counts and the categories hint

**Files:**
- Create: `src/filing/refile/command.rs`, `tests/refile_preview.rs`
- Modify: `src/filing/refile/mod.rs` (`pub mod command;`)
- Modify: `src/service.rs` (re-export `RefileOptions`, `filing` import, new `filing_refile`, `filing_status` fields, `apply_categories` hint)
- Modify: `tests/refile_support/mod.rs` (preview helpers)

**Interfaces:**
- Consumes (Tasks 3–4): `rules::{gone, input, verdict, in_category_folder, Skip, Candidate, Verdict}`, `inputs::plan_input`, `observe::{offline_map, OfflineEngine}`; test support.
- Produces:
  - `refile::command::RefileOptions { pub category: Option<String>, pub folder: Option<String>, pub limit: usize }` (`Default`: limit 50), re-exported as `mailtriage::service::RefileOptions`.
  - `refile::command::preview(store: &Store, name: &str, cfg: &AccountConfig, generation: &str, opts: &RefileOptions) -> Result<Value>`.
  - `refile::command::candidate_total(store: &Store, name: &str, cfg: &AccountConfig, generation: &str) -> Result<usize>`.
  - `refile::command::hint(name: &str, cfg: &AccountConfig) -> Value`.
  - `refile::command::match_folder(records: &[FolderRecord], sources: &[String], name: &str) -> Result<String>`.
  - `Service::filing_refile(&mut self, name: &str, opts: RefileOptions) -> Result<Value>`.
  - `filing status` gains `refile_marked` and `refile_candidates`; `categories apply` gains `hint`.
  - Test support: `opts`, `preview`, `code`.

- [ ] **Step 1: Write the failing tests**

Add to `tests/refile_support/mod.rs` (extend the `use mailtriage::{…}` line to `use mailtriage::{domain::Category, filing::Placement, service::{RefileOptions, ServiceError}};`):

```rust
pub fn opts(category: Option<&str>, folder: Option<&str>) -> RefileOptions {
    RefileOptions {
        category: category.map(str::to_string),
        folder: folder.map(str::to_string),
        limit: 50,
    }
}

/// `filing refile` without `--apply`.
pub fn preview(h: &Harness, category: Option<&str>, folder: Option<&str>) -> Value {
    h.service().filing_refile("work", opts(category, folder)).unwrap()
}

/// The exit code of a refused service call.
pub fn code(e: &anyhow::Error) -> Option<i32> {
    e.downcast_ref::<ServiceError>().map(|s| s.code)
}
```

Create `tests/refile_preview.rs`:

```rust
//! Refile spec "Command" (the preview) and "Visibility".
mod common;
mod refile_support;
use common::{mail, Harness};
use mailtriage::{
    domain::FilingMode::{Live, Off},
    engine::fake::{FakeOp, Fault},
    filing::{transitions, FolderRecord},
};
use refile_support::{
    add_category, add_updates, code, correspondence_category, deliver_filed, filed_in_other_now_updates,
    id_of, located, mark, opts, placement, preview, remove_category, updates_category, without,
};
use serde_json::{json, Value};

/// Every engine call but the binding check (the account binding reads the
/// engine's identity; nothing else may reach the mailbox).
fn mailbox_calls(h: &Harness) -> usize {
    h.fake.calls().iter().filter(|c| *c != "binding_identity").count()
}

fn retired_record(native: &str, configured: &str) -> FolderRecord {
    FolderRecord {
        account: "work".into(),
        native: native.into(),
        configured: Some(configured.into()),
        category_id: Some("deals".into()),
        origin: Some("created".into()),
        state: "retired".into(),
        role_verified: true,
        confirmed: false,
        subscribed: true,
        pause_reason: None,
        epoch: None,
        watch_from_uid: None,
        rescan_epoch: None,
        rescan_below_uid: None,
        rescan_complete: true,
        checked_at: None,
        error: None,
    }
}

#[test]
fn the_preview_lists_a_changed_category_with_its_target_and_folder() {
    let h = Harness::new(Live);
    let id = filed_in_other_now_updates(&h);
    let calls = mailbox_calls(&h);
    let v = preview(&h, None, None);
    assert_eq!(mailbox_calls(&h), calls, "no mailbox calls");
    assert_eq!(
        v["candidates"],
        json!([{"id": id, "folder": "Other", "target": "Updates", "category": "updates", "reason": "category_changed"}])
    );
    assert_eq!((v["total"].clone(), v["waiting"].clone()), (json!(1), json!(0)));
    assert_eq!(
        v["folders"],
        json!([{"folder": "Other", "native": "Other", "retired": false, "candidates": 1, "waiting": 0}])
    );
    let skipped = v["skipped"].as_object().unwrap();
    assert_eq!(skipped.len(), 12);
    assert!(skipped.values().all(|n| *n == 0));
    assert_eq!(v["mode"], "live");
    assert!(!placement(&h, &id).refile_once, "a preview marks nothing");
}

#[test]
fn mail_not_yet_classified_again_is_waiting_in_its_folder() {
    let h = Harness::new(Live);
    without(&h, &["updates"]);
    h.sync();
    deliver_filed(&h, "u", "System update", "Version 2 is out");
    add_category(&h, updates_category()); // stale until the next pass
    let v = preview(&h, None, None);
    assert_eq!((v["total"].clone(), v["waiting"].clone()), (json!(0), json!(1)));
    assert_eq!(
        v["folders"],
        json!([{"folder": "Other", "native": "Other", "retired": false, "candidates": 0, "waiting": 1}])
    );
    assert_eq!(preview(&h, Some("updates"), None)["waiting"], 1, "--category still reports it");
}

#[test]
fn folder_names_match_configured_and_native_names() {
    let h = Harness::new(Live);
    h.fake.set_prefix("INBOX.", '.');
    let id = filed_in_other_now_updates(&h);
    assert_eq!(located(&h, "u").0, "INBOX.Other");
    let by_name = preview(&h, None, Some("Other"));
    assert_eq!(by_name, preview(&h, None, Some("INBOX.Other")));
    assert_eq!(
        by_name["candidates"],
        json!([{"id": id, "folder": "INBOX.Other", "target": "INBOX.Updates", "category": "updates", "reason": "category_changed"}])
    );
    assert_eq!(
        by_name["folders"],
        json!([{"folder": "Other", "native": "INBOX.Other", "retired": false, "candidates": 1, "waiting": 0}])
    );
    assert_eq!(preview(&h, None, Some("INBOX.Updates"))["total"], 0, "another folder");
    for bad in ["INBOX", "Nope", "inbox.other"] {
        let e = h.service().filing_refile("work", opts(None, Some(bad))).unwrap_err();
        assert_eq!(code(&e), Some(2), "{bad}");
    }
}

#[test]
fn a_retired_folder_keeps_its_configured_name() {
    let h = Harness::new(Live);
    h.sync();
    let id = deliver_filed(&h, "n", "Weekly newsletter", "Our newsletter");
    remove_category(&h, "newsletters");
    h.sync(); // Newsletters is retired; n is classified again into `other`
    let v = preview(&h, None, Some("Newsletters"));
    assert_eq!(
        v["candidates"],
        json!([{"id": id, "folder": "Newsletters", "target": "Other", "category": "other", "reason": "folder_retired"}])
    );
    assert_eq!(
        v["folders"],
        json!([{"folder": "Newsletters", "native": "Newsletters", "retired": true, "candidates": 1, "waiting": 0}])
    );
}

#[test]
fn a_native_name_wins_and_a_shared_configured_name_is_refused() {
    let h = Harness::new(Live);
    h.sync();
    let mut s = h.service();
    for native in ["INBOX.Deals", "Archive.Deals"] {
        s.store.save_folder(&retired_record(native, "Deals")).unwrap();
    }
    // Recorded under the prefix that came later, configured as `Promotions`.
    s.store
        .save_folder(&retired_record("INBOX.Promotions", "Promotions"))
        .unwrap();
    let e = s.filing_refile("work", opts(None, Some("Deals"))).unwrap_err();
    assert_eq!(code(&e), Some(2));
    assert!(e.to_string().contains("Archive.Deals, INBOX.Deals"), "{e}");
    assert_eq!(s.filing_refile("work", opts(None, Some("INBOX.Deals"))).unwrap()["total"], 0);
    // `Promotions` is a native name (the category's own folder): no question asked.
    assert!(s.filing_refile("work", opts(None, Some("Promotions"))).is_ok());
}

#[test]
fn mail_mailtriage_did_not_put_there_is_never_a_candidate() {
    // Moved back into its folder by the user, and delivered straight into it.
    let h = Harness::new(Live);
    without(&h, &["updates"]);
    h.sync();
    deliver_filed(&h, "a", "System update", "Version 2 is out");
    let (folder, uid) = located(&h, "a");
    h.fake.client_move(&folder, uid, &folder);
    h.fake
        .deliver("Other", &mail("b", "Server update", "Version 3 is out"));
    h.sync();
    h.sync();
    add_updates(&h);
    let v = preview(&h, None, None);
    assert_eq!(v["total"], 0);
    assert_eq!(v["skipped"]["not_filed_by_mailtriage"], 2);

    // Placed by a rescan after an epoch reset.
    let h = Harness::new(Live);
    without(&h, &["updates"]);
    h.sync();
    deliver_filed(&h, "c", "System update", "Version 2 is out");
    h.fake.reset_epoch("Other");
    for _ in 0..3 {
        h.sync();
    }
    add_updates(&h);
    let v = preview(&h, None, None);
    assert_eq!((v["total"].clone(), v["skipped"]["not_filed_by_mailtriage"].clone()), (json!(0), json!(1)));

    // Moved without COPYUID, so applied by recovery.
    let h = Harness::new(Live);
    h.fake.set_capabilities(true, false, true);
    without(&h, &["updates"]);
    h.sync();
    let id = deliver_filed(&h, "d", "System update", "Version 2 is out");
    h.sync();
    add_updates(&h);
    assert_eq!(placement(&h, &id).filed_by.as_deref(), Some("mailtriage"));
    let v = preview(&h, None, None);
    assert_eq!((v["total"].clone(), v["skipped"]["not_filed_by_mailtriage"].clone()), (json!(0), json!(1)));
}

#[test]
fn a_quarantined_copy_that_became_the_home_is_not_a_candidate() {
    let h = Harness::new(Live);
    h.sync();
    h.fake
        .deliver("INBOX", &mail("m", "Another newsletter", "Our newsletter"));
    h.fake.inject(FakeOp::Move, Fault::ErrorBefore);
    h.sync();
    // x lands in Newsletters while INBOX is recreated: a suspected race.
    h.fake
        .deliver("Newsletters", &mail("x", "Invoice", "Payment due"));
    h.fake.reset_epoch("INBOX");
    h.service().sync("work", 1).unwrap();
    let mut s = h.service();
    transitions::release_folder(&mut s.store, "work", "INBOX", &mailtriage::store::now()).unwrap();
    for _ in 0..3 {
        h.sync();
    }
    let x = id_of(&h, "x");
    assert_eq!(placement(&h, &x).blocked_reason.as_deref(), Some("quarantined"));
    h.service().filing_retry("work", mailtriage::service::RetryTarget::Message(x.clone())).unwrap();
    // x, classified `transactions`, sits in Newsletters: it would be a candidate.
    let v = preview(&h, None, None);
    assert_eq!((v["total"].clone(), v["skipped"]["not_filed_by_mailtriage"].clone()), (json!(0), json!(1)));
}

#[test]
fn an_outstanding_request_for_a_removed_category_is_explicit_target() {
    let h = Harness::new(Live);
    h.sync();
    let id = deliver_filed(&h, "n", "Weekly newsletter", "Our newsletter");
    let mut s = h.service();
    s.correct("work", &id, json!({"category_id": "promotions"}), None).unwrap();
    // Clearing the correction leaves a request for the model's `newsletters`.
    s.correct("work", &id, json!({}), Some("category_id")).unwrap();
    drop(s);
    remove_category(&h, "newsletters");
    let v = preview(&h, None, None);
    assert_eq!(v["total"], 0);
    assert_eq!(v["skipped"]["explicit_target"], 1);
}

#[test]
fn corrected_pinned_done_and_inbox_bound_mail_is_skipped() {
    let h = Harness::new(Live);
    without(&h, &["updates", "correspondence"]);
    h.sync();
    let corrected = deliver_filed(&h, "u1", "System update 1", "Version 2 is out");
    let pinned = deliver_filed(&h, "u2", "System update 2", "Version 2 is out");
    let done = deliver_filed(&h, "u3", "System update 3", "Version 2 is out");
    deliver_filed(&h, "l", "Lunch", "See you at noon");
    add_category(&h, correspondence_category());
    add_updates(&h);
    let mut s = h.service();
    s.correct("work", &corrected, json!({"category_id": "transactions"}), None)
        .unwrap();
    s.filing_pin("work", &pinned).unwrap();
    s.review("work", &done, true).unwrap();
    drop(s);
    let v = preview(&h, None, None);
    assert_eq!(v["total"], 0);
    for key in ["corrected", "pinned", "done", "target_inbox_or_source"] {
        assert_eq!(v["skipped"][key], 1, "{key}");
    }
    let only_updates = preview(&h, Some("updates"), None);
    assert_eq!(
        (only_updates["skipped"]["pinned"].clone(), only_updates["skipped"]["target_inbox_or_source"].clone()),
        (json!(1), json!(0)),
        "--category counts the skips of that category only"
    );
}

#[test]
fn status_counts_marks_and_candidates() {
    let h = Harness::new(Live);
    let id = filed_in_other_now_updates(&h);
    let st = h.service().filing_status("work").unwrap();
    assert_eq!((st["refile_marked"].clone(), st["refile_candidates"].clone()), (json!(0), json!(1)));
    mark(&h, &id);
    assert_eq!(h.service().filing_status("work").unwrap()["refile_marked"], 1);
}

#[test]
fn categories_apply_hints_at_refile_while_filing_is_on() {
    let h = Harness::new(Live);
    h.sync();
    let categories = h.service().config.accounts["work"].categories.clone();
    let out = h.service().apply_categories("work", categories.clone()).unwrap();
    assert_eq!(
        out["hint"],
        "Open mail is classified again over the next sync passes. Once they have run, `mailtriage filing refile --account work` shows which filed mail would move."
    );
    let h = Harness::new(Off);
    let out = h.service().apply_categories("work", categories).unwrap();
    assert_eq!(out["hint"], Value::Null);
}

#[test]
fn bad_arguments_exit_2() {
    let h = Harness::new(Live);
    h.sync();
    let mut s = h.service();
    let cases = [
        ("work", opts(Some("nope"), None)),
        ("work", opts(None, Some("INBOX"))),
        ("work", with_limit(0)),
        ("work", with_limit(501)),
        ("nope", opts(None, None)),
    ];
    for (account, o) in cases {
        let e = s.filing_refile(account, o.clone()).unwrap_err();
        assert_eq!(code(&e), Some(2), "{account} {o:?}");
    }
}

fn with_limit(limit: usize) -> mailtriage::service::RefileOptions {
    mailtriage::service::RefileOptions {
        limit,
        ..opts(None, None)
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --locked --test refile_preview`
Expected: compile errors — `RefileOptions` not found in `service`, no method `filing_refile`.

- [ ] **Step 3: The command module (`src/filing/refile/command.rs`, `src/filing/refile/mod.rs`)**

Add `pub mod command;` to `src/filing/refile/mod.rs` (before `pub mod intents;`). Create `src/filing/refile/command.rs`:

```rust
//! The `filing refile` command (refile spec "Command"): which filed mail
//! would follow its new category. It reads stored state only and makes no
//! mailbox calls.
use crate::domain::{AccountConfig, FilingMode};
use crate::filing::observe::{self, OfflineEngine};
use crate::filing::refile::rules::{self, Candidate, Skip, Verdict};
use crate::filing::{inputs, mode_str, FolderRecord, PassContext};
use crate::service::err;
use crate::store::{now, Store};
use anyhow::Result;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// `filing refile` filters. `limit` (1..=500) bounds the listed candidates only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefileOptions {
    pub category: Option<String>,
    pub folder: Option<String>,
    pub limit: usize,
}

impl Default for RefileOptions {
    fn default() -> Self {
        Self {
            category: None,
            folder: None,
            limit: 50,
        }
    }
}

/// One matching message: its id, current (native) folder, home UID, and the
/// revision and mark the report read.
struct Row {
    id: String,
    folder: String,
    uid: u64,
    desired_rev: i64,
    marked: bool,
}

/// The matching set, before `limit`.
#[derive(Default)]
struct Report {
    candidates: Vec<(Row, Candidate)>,
    waiting: Vec<Row>,
    skipped: BTreeMap<Skip, usize>,
    /// Native folder -> (candidates, waiting).
    folders: BTreeMap<String, (usize, usize)>,
    /// Native folders of `folders` no configured category files into.
    retired: BTreeSet<String>,
}

/// The preview's JSON (empty with filing `off`).
pub fn preview(
    store: &Store,
    name: &str,
    cfg: &AccountConfig,
    generation: &str,
    opts: &RefileOptions,
) -> Result<Value> {
    let folder = selected_folder(store, name, cfg, opts)?;
    let mode = filing_mode(cfg);
    let found = if mode == FilingMode::Off {
        Report::default()
    } else {
        report(store, name, cfg, generation, opts.category.as_deref(), folder.as_deref())?
    };
    let configured: BTreeMap<String, Option<String>> = store
        .folder_records(name)?
        .into_iter()
        .map(|r| (r.native, r.configured))
        .collect();
    let candidates: Vec<Value> = found
        .candidates
        .iter()
        .take(opts.limit)
        .map(|(row, c)| {
            json!({"id": row.id, "folder": row.folder, "target": c.target, "category": c.category, "reason": c.reason.as_str()})
        })
        .collect();
    let folders: Vec<Value> = found
        .folders
        .iter()
        .map(|(native, (candidates, waiting))| {
            json!({
                "folder": configured.get(native).cloned().flatten(),
                "native": native,
                "retired": found.retired.contains(native),
                "candidates": candidates,
                "waiting": waiting,
            })
        })
        .collect();
    let skipped: Map<String, Value> = Skip::ALL
        .iter()
        .map(|s| (s.as_str().to_string(), json!(found.skipped.get(s).copied().unwrap_or(0))))
        .collect();
    Ok(json!({
        "schema_version": 1,
        "account": name,
        "mode": mode_str(mode),
        "candidates": candidates,
        "total": found.candidates.len(),
        "folders": folders,
        "waiting": found.waiting.len(),
        "skipped": skipped,
    }))
}

/// `filing status`'s `refile_candidates`: the unfiltered candidate total.
pub fn candidate_total(
    store: &Store,
    name: &str,
    cfg: &AccountConfig,
    generation: &str,
) -> Result<usize> {
    if filing_mode(cfg) == FilingMode::Off {
        return Ok(0);
    }
    Ok(report(store, name, cfg, generation, None, None)?.candidates.len())
}

/// `categories apply`'s `hint`; `null` with filing `off`.
pub fn hint(name: &str, cfg: &AccountConfig) -> Value {
    if filing_mode(cfg) == FilingMode::Off {
        return Value::Null;
    }
    json!(format!(
        "Open mail is classified again over the next sync passes. Once they have run, `mailtriage filing refile --account {name}` shows which filed mail would move."
    ))
}

/// `--folder NAME`: a category or retired folder's native name wins;
/// otherwise its configured name, which must name exactly one folder.
pub fn match_folder(records: &[FolderRecord], sources: &[String], name: &str) -> Result<String> {
    let known: Vec<&FolderRecord> = records
        .iter()
        .filter(|r| r.category_id.is_some() && !sources.contains(&r.native))
        .collect();
    if let Some(r) = known.iter().find(|r| r.native == name) {
        return Ok(r.native.clone());
    }
    let named: Vec<&str> = known
        .iter()
        .filter(|r| r.configured.as_deref() == Some(name))
        .map(|r| r.native.as_str())
        .collect();
    match named.as_slice() {
        [native] => Ok(native.to_string()),
        [] => Err(err(2, "unknown folder: not a category or retired folder")),
        several => Err(err(
            2,
            format!(
                "folder name matches several folders; pass the native name: {}",
                several.join(", ")
            ),
        )),
    }
}

/// The checks of `--limit`, `--category` and `--folder`; the native folder
/// `--folder` names.
fn selected_folder(
    store: &Store,
    name: &str,
    cfg: &AccountConfig,
    opts: &RefileOptions,
) -> Result<Option<String>> {
    if !(1..=500).contains(&opts.limit) {
        return Err(err(2, "refile limit must be 1..=500"));
    }
    if let Some(category) = &opts.category {
        if !cfg.categories.iter().any(|c| &c.id == category) {
            return Err(err(2, "unknown category"));
        }
    }
    match &opts.folder {
        None => Ok(None),
        Some(folder) => {
            let records = store.folder_records(name)?;
            Ok(Some(match_folder(&records, &sources_of(cfg), folder)?))
        }
    }
}

/// Every placement's verdict over the offline folder map, filtered.
fn report(
    store: &Store,
    name: &str,
    cfg: &AccountConfig,
    generation: &str,
    category: Option<&str>,
    folder: Option<&str>,
) -> Result<Report> {
    let map = observe::offline_map(store, name, cfg)?;
    let no_binding_check = || -> Result<()> { Ok(()) };
    let ctx = PassContext {
        account: name,
        cfg,
        engine: &OfflineEngine,
        mode: filing_mode(cfg),
        generation,
        now: now(),
        // The planner input does not use it.
        max_attempts: 0,
        verify_binding: &no_binding_check,
    };
    let input = inputs::plan_input(store, &ctx, &map, true)?;
    let gone = rules::gone(store, name, &map.listed)?;
    let refile = rules::input(store, name, cfg, &gone)?;
    let mut out = Report::default();
    for m in &input.messages {
        let Some(home) = &m.home else {
            continue;
        };
        if folder.is_some_and(|f| f != home.folder) {
            continue;
        }
        let row = Row {
            id: m.message_id.clone(),
            folder: home.folder.clone(),
            uid: home.uid,
            desired_rev: m.desired_rev,
            marked: refile.facts.get(&m.message_id).is_some_and(|f| f.marked),
        };
        match rules::verdict(&input, m, &refile) {
            Verdict::OutOfScope | Verdict::InPlace => {}
            Verdict::Waiting => {
                out.folders.entry(row.folder.clone()).or_default().1 += 1;
                out.waiting.push(row);
            }
            Verdict::Candidate(c) => {
                if category.is_none_or(|k| k == c.category) {
                    out.folders.entry(row.folder.clone()).or_default().0 += 1;
                    out.candidates.push((row, c));
                }
            }
            Verdict::Skipped(skip) => {
                if category.is_none_or(|k| m.effective.category_id.as_deref() == Some(k)) {
                    *out.skipped.entry(skip).or_default() += 1;
                }
            }
        }
    }
    out.retired = out
        .folders
        .keys()
        .filter(|f| !rules::in_category_folder(&input, f))
        .cloned()
        .collect();
    out.candidates
        .sort_by(|a, b| (&a.0.folder, a.0.uid).cmp(&(&b.0.folder, b.0.uid)));
    out.waiting
        .sort_by(|a, b| (&a.folder, a.uid).cmp(&(&b.folder, b.uid)));
    Ok(out)
}

/// The filing mode a pass would run: the configured one, `off` without an engine.
fn filing_mode(cfg: &AccountConfig) -> FilingMode {
    match cfg.engine_config() {
        Some(_) => cfg.filing.mode,
        None => FilingMode::Off,
    }
}

fn sources_of(cfg: &AccountConfig) -> Vec<String> {
    cfg.engine_config()
        .map(|e| e.mailboxes().to_vec())
        .unwrap_or_default()
}
```

Task 6 reads `Row.desired_rev` and `Row.marked`; until then the compiler warns that they are never read. Add `#[allow(dead_code)] // Read by `apply` (next task).` above `struct Row` in this task and remove it in Task 6.

- [ ] **Step 4: Service (`src/service.rs`)**

After `pub enum RetryTarget { … }` add:

```rust
/// `filing refile` filters (refile spec "Command").
pub use crate::filing::refile::command::RefileOptions;
```

Add the method after `filing_backfill`:

```rust
    /// `filing refile` without `--apply` (refile spec "Command"): which
    /// filed mail would follow its new category. Read-only; it may bring the
    /// classification generation up to date, as `filing plan` does.
    pub fn filing_refile(&mut self, name: &str, opts: RefileOptions) -> Result<Value> {
        let (account, generation) = self.ensure(name)?;
        refile::command::preview(&self.store, name, &account, &generation, &opts)
    }
```

In `filing_status`: change `let (account, _) = self.ensure(name)?;` to `let (account, generation) = self.ensure(name)?;`; add `let mut refile_marked = 0;` before the `for (_, p, meta) in self.store.records_for_planning(name)? {` loop and, as the loop's first statement, `if p.refile_once { refile_marked += 1; }`; after the loop add `let refile_candidates = refile::command::candidate_total(&self.store, name, &account, &generation)?;`; and add to the returned object, after `"stale_requests": …,`:

```rust
            "refile_marked": refile_marked,
            "refile_candidates": refile_candidates,
```

In `apply_categories`, replace the final `self.categories(name)` with:

```rust
        let mut out = self.categories(name)?;
        out["hint"] = refile::command::hint(name, &self.account(name)?);
        Ok(out)
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --locked --test refile_preview && cargo test --locked --test filing_cli`
Expected: all PASS; `status_reports_counts_and_stale_requests_without_engine_calls` still sees no mailbox calls.

- [ ] **Step 6: Full check and commit**

Run: `cargo fmt && cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`

```bash
git add src/filing/refile/mod.rs src/filing/refile/command.rs src/service.rs tests/refile_support/mod.rs tests/refile_preview.rs
git commit -m "Preview refiling, count marks in filing status and hint after categories apply

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: `filing refile --apply`, the CLI and the agent and API docs

**Files:**
- Modify: `src/filing/refile/command.rs` (`apply`; drop the `dead_code` allow on `Row`)
- Modify: `src/service.rs` (`filing_refile_apply`)
- Modify: `src/cli.rs` (`FilingCommand::Refile`, `RefileArg`, dispatch, import)
- Modify: `tests/refile_support/mod.rs` (`apply` helper)
- Create: `tests/refile_command.rs`
- Modify: `docs/development/service-api.md` (new section), `docs/agents/index.md` (new subsection)

**Interfaces:**
- Consumes (Task 5): `RefileOptions`, `command::{preview, report, selected_folder}` (private to the module), `Service::filing_refile`; `FilingWrite::{PlacementFrom, Event}`; `Store::commit_filing`.
- Produces:
  - `refile::command::apply(store: &mut Store, name: &str, cfg: &AccountConfig, generation: &str, opts: &RefileOptions) -> Result<Value>` → `{"schema_version":1,"account":NAME,"marked":N,"waiting_marked":W}`.
  - `Service::filing_refile_apply(&mut self, name: &str, opts: RefileOptions) -> Result<Value>`.
  - CLI: `mailtriage filing refile --account NAME [--category ID] [--folder NAME] [--limit N] [--apply] [--json]`.
  - Event `refile_marked {marked, waiting_marked, category, folder}`.

- [ ] **Step 1: Write the failing tests**

Add to `tests/refile_support/mod.rs`:

```rust
/// `filing refile --apply`.
pub fn apply(h: &Harness, category: Option<&str>, folder: Option<&str>) -> Value {
    h.service()
        .filing_refile_apply("work", opts(category, folder))
        .unwrap()
}
```

Create `tests/refile_command.rs`:

```rust
//! Refile spec "Command" (`--apply`), "Errors and exit codes" and the CLI.
mod common;
mod refile_support;
use common::{mail, Harness};
use mailtriage::domain::FilingMode::{DryRun, Live};
use refile_support::{
    add_updates, apply, code, deliver_filed, events, filed_in_other_now_updates, id_of, located,
    moves_to, opts, placement, preview, without,
};
use serde_json::{json, Value};
use std::{path::Path, process::Command};

#[test]
fn apply_marks_and_the_next_passes_move_the_mail() {
    let h = Harness::new(Live);
    let id = filed_in_other_now_updates(&h);
    assert_eq!(
        apply(&h, None, None),
        json!({"schema_version": 1, "account": "work", "marked": 1, "waiting_marked": 0})
    );
    assert!(placement(&h, &id).refile_once);
    assert_eq!(events(&h, "refile_marked")[0]["detail"]["marked"], 1);
    h.sync();
    h.sync();
    assert_eq!(located(&h, "u").0, "Updates");
    assert!(!placement(&h, &id).refile_once);
    assert_eq!(events(&h, "moved")[0]["detail"]["reason"], "refile");
    assert_eq!(preview(&h, None, None)["total"], 0, "in place now");
}

#[test]
fn the_limit_shortens_the_list_and_apply_marks_the_whole_set_once() {
    let h = Harness::new(Live);
    h.edit(|c| c.accounts.get_mut("work").unwrap().filing.max_actions_per_pass = 1);
    without(&h, &["updates"]);
    h.sync();
    for i in 0..3 {
        h.fake.deliver(
            "INBOX",
            &mail(&format!("u{i}"), &format!("System update {i}"), "Version 2 is out"),
        );
    }
    for _ in 0..5 {
        h.sync(); // one move per pass
    }
    assert!((0..3).all(|i| located(&h, &format!("u{i}")).0 == "Other"));
    add_updates(&h);
    let mut o = opts(None, None);
    o.limit = 1;
    let v = h.service().filing_refile("work", o).unwrap();
    assert_eq!((v["candidates"].as_array().unwrap().len(), v["total"].clone()), (1, json!(3)));
    assert_eq!(apply(&h, None, None)["marked"], 3, "--apply marks the whole set");
    assert_eq!(apply(&h, None, None)["marked"], 0, "applying again marks nothing new");
    let mut moved = vec![];
    for _ in 0..3 {
        h.sync();
        moved.push(moves_to(&h, "Updates"));
    }
    assert_eq!(moved, vec![1, 2, 3], "max_actions_per_pass spreads the moves");
}

#[test]
fn category_never_marks_waiting_mail_and_folder_does() {
    let h = Harness::new(Live);
    without(&h, &["updates", "promotions"]);
    h.sync();
    let u = deliver_filed(&h, "u", "System update", "Version 2 is out"); // will be `updates`
    let s = deliver_filed(&h, "s", "Big sale", "Everything must go"); // stays `other`
    assert_eq!((located(&h, "u").0, located(&h, "s").0), ("Other".into(), "Other".into()));
    refile_support::add_category(&h, refile_support::updates_category()); // both wait
    assert_eq!(preview(&h, None, Some("Other"))["waiting"], 2);
    assert_eq!(
        apply(&h, Some("updates"), None),
        json!({"schema_version": 1, "account": "work", "marked": 0, "waiting_marked": 0})
    );
    assert_eq!(
        apply(&h, None, Some("Other")),
        json!({"schema_version": 1, "account": "work", "marked": 0, "waiting_marked": 2})
    );
    h.sync();
    h.sync();
    assert_eq!(located(&h, "u").0, "Updates", "moved once classified");
    assert_eq!(located(&h, "s").0, "Other");
    assert!(!placement(&h, &u).refile_once);
    assert!(!placement(&h, &s).refile_once, "classified into its own folder: cleared");
    let cleared: Vec<Value> = events(&h, "refile_cleared")
        .into_iter()
        .filter(|e| e["message_id"] == s.as_str())
        .collect();
    assert_eq!(cleared[0]["detail"]["reason"], "in_place");
    assert_eq!(moves_to(&h, "Updates"), 1);
}

#[test]
fn copies_are_skipped_and_never_moved() {
    let h = Harness::new(Live);
    without(&h, &["updates"]);
    h.sync();
    deliver_filed(&h, "a", "System update", "Version 2 is out");
    deliver_filed(&h, "b", "Server update", "Version 3 is out");
    let (folder, uid) = located(&h, "a");
    h.fake.client_copy(&folder, uid, "Transactions"); // copies in two category folders
    add_updates(&h);
    let (folder, uid) = located(&h, "b");
    h.fake.client_copy(&folder, uid, "Updates"); // a copy already in the target
    h.sync();
    let v = preview(&h, None, None);
    assert_eq!((v["total"].clone(), v["skipped"]["multiple_copies"].clone()), (json!(0), json!(2)));
    assert_eq!(apply(&h, None, None)["marked"], 0);
    h.sync();
    h.sync();
    assert_eq!(moves_to(&h, "Updates"), 0);
}

#[test]
fn apply_needs_live_and_marks_nothing_otherwise() {
    let h = Harness::new(Live);
    let id = filed_in_other_now_updates(&h);
    h.set_mode(DryRun);
    let e = h.service().filing_refile_apply("work", opts(None, None)).unwrap_err();
    assert_eq!(code(&e), Some(2));
    assert_eq!(e.to_string(), "refile --apply requires filing mode live");
    assert!(!placement(&h, &id).refile_once);
    assert_eq!(preview(&h, None, None)["total"], 1, "the preview works in dry_run");
}

#[test]
fn a_configuration_change_during_apply_exits_5() {
    let h = Harness::new(Live);
    let id = filed_in_other_now_updates(&h);
    let mut s = h.service();
    h.edit(|c| c.accounts.get_mut("work").unwrap().brief = "changed".into());
    let e = s.filing_refile_apply("work", opts(None, None)).unwrap_err();
    assert_eq!(code(&e), Some(5));
    let kind = e.downcast_ref::<mailtriage::service::ServiceError>().unwrap().kind;
    assert_eq!(kind.reason(), Some("config_changed"));
    assert!(!placement(&h, &id).refile_once);
}

#[test]
fn filed_mail_made_actionable_is_flagged_and_the_refile_writes_no_flag() {
    let h = Harness::new(Live);
    let id = filed_in_other_now_updates(&h);
    // The fake classifier's action decision depends on the text only, so the
    // decision changes through a correction of `action_required`.
    h.service()
        .correct("work", &id, json!({"action_required": true}), None)
        .unwrap();
    apply(&h, None, None);
    h.sync();
    h.sync();
    let flags: Vec<String> = h
        .fake
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("flag "))
        .collect();
    assert_eq!(flags.len(), 1, "{flags:?}");
    assert!(flags[0].starts_with("flag Other "), "flagged where it was, by the flag rule");
    let (folder, uid) = located(&h, "u");
    assert_eq!(folder, "Updates");
    assert!(h.fake.flags(&folder, uid).contains(&"\\Flagged".to_string()));
    assert_eq!(id_of(&h, "u"), id);
}

fn run(cwd: &Path, args: &[&str]) -> (i32, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_mailtriage"))
        .current_dir(cwd)
        .args(args)
        .env_remove("MAILTRIAGE_CONFIG")
        .output()
        .unwrap();
    let v = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    (out.status.code().unwrap(), v)
}

/// `init` plus a Himalaya engine that is never started: the refile commands
/// make no mailbox calls.
fn offline_account(d: &Path) {
    assert_eq!(run(d, &["init", "--json"]).0, 0);
    std::fs::write(d.join("h.toml"), "[accounts.work]\nimap.server='imaps://x.test'\n").unwrap();
    let path = d.join("mailtriage.json");
    let mut c: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    c["accounts"]["work"]["engine"] = json!({"kind":"himalaya","binary":"/nonexistent/himalaya","config":"h.toml","account":"work","mailboxes":["INBOX"],"expected_version":"2.1.0","timeout_seconds":5,"max_output_bytes":100000});
    std::fs::write(&path, c.to_string()).unwrap();
}

fn no_skips() -> Value {
    let keys = [
        "not_filed_by_mailtriage",
        "corrected",
        "pinned",
        "blocked",
        "done",
        "open_intent",
        "explicit_target",
        "multiple_copies",
        "incomplete_input",
        "retired_frozen",
        "target_unusable",
        "target_inbox_or_source",
    ];
    Value::Object(keys.iter().map(|k| (k.to_string(), json!(0))).collect())
}

#[test]
fn cli_refile_prints_the_preview_and_apply_shapes_and_exit_codes() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    offline_account(d);
    let enable = |mode| run(d, &["filing", "enable", "--account", "work", "--mode", mode, "--json"]).0;
    assert_eq!(enable("dry-run"), 0);
    let (code, v) = run(d, &["filing", "refile", "--account", "work", "--json"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(
        v,
        json!({"schema_version": 1, "account": "work", "mode": "dry_run", "candidates": [], "total": 0, "folders": [], "waiting": 0, "skipped": no_skips()})
    );
    let (code, v) = run(d, &["filing", "refile", "--account", "work", "--apply", "--json"]);
    assert_eq!((code, v["error"]["message"].clone()), (2, json!("refile --apply requires filing mode live")));
    for extra in [
        &["--category", "nope"][..],
        &["--folder", "Nope"][..],
        &["--folder", "INBOX"][..],
        &["--limit", "0"][..],
        &["--limit", "501"][..],
    ] {
        let mut args = vec!["filing", "refile", "--account", "work", "--json"];
        args.extend_from_slice(extra);
        assert_eq!(run(d, &args).0, 2, "{extra:?}");
    }
    assert_eq!(run(d, &["filing", "refile", "--account", "nope", "--json"]).0, 2);
    assert_eq!(enable("live"), 0);
    let (code, v) = run(d, &["filing", "refile", "--account", "work", "--apply", "--json"]);
    assert_eq!(
        (code, v),
        (0, json!({"schema_version": 1, "account": "work", "marked": 0, "waiting_marked": 0}))
    );
}

#[test]
fn cli_refile_reports_an_unavailable_state_database_with_exit_3() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    offline_account(d);
    std::fs::create_dir_all(d.join(".state").join("mailtriage.sqlite")).unwrap();
    assert_eq!(run(d, &["filing", "refile", "--account", "work", "--json"]).0, 3);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --locked --test refile_command`
Expected: compile error — no method `filing_refile_apply`.

- [ ] **Step 3: Apply (`src/filing/refile/command.rs`)**

Remove the `#[allow(dead_code)]` line above `struct Row`. Add `FilingWrite` and `Placement` to the `use crate::filing::{…}` line. Add after `candidate_total`:

```rust
/// `--apply` (filing `live`, checked by the caller under the configuration
/// lock): marks the matching candidates — and with `--folder` but no
/// `--category` the waiting messages too — that are not marked yet, with a
/// new revision each and one `refile_marked` event, in one transaction.
pub fn apply(
    store: &mut Store,
    name: &str,
    cfg: &AccountConfig,
    generation: &str,
    opts: &RefileOptions,
) -> Result<Value> {
    let folder = selected_folder(store, name, cfg, opts)?;
    let with_waiting = folder.is_some() && opts.category.is_none();
    for _ in 0..5 {
        let found = report(store, name, cfg, generation, opts.category.as_deref(), folder.as_deref())?;
        let mut rows: Vec<(&Row, bool)> = found.candidates.iter().map(|(row, _)| (row, false)).collect();
        if with_waiting {
            rows.extend(found.waiting.iter().map(|row| (row, true)));
        }
        rows.retain(|(row, _)| !row.marked);
        let mut reads: Vec<(Placement, bool)> = Vec::new();
        for (row, waiting) in &rows {
            match store.placement(name, &row.id)? {
                Some(p) if p.desired_rev == row.desired_rev => reads.push((p, *waiting)),
                _ => break,
            }
        }
        if reads.len() != rows.len() {
            continue; // changed since the report: read it again
        }
        let waiting_marked = reads.iter().filter(|(_, waiting)| *waiting).count();
        let marked = reads.len() - waiting_marked;
        let result = json!({"schema_version": 1, "account": name, "marked": marked, "waiting_marked": waiting_marked});
        if reads.is_empty() {
            return Ok(result);
        }
        let changed: Vec<Placement> = reads
            .iter()
            .map(|(p, _)| Placement {
                refile_once: true,
                desired_rev: p.desired_rev + 1,
                ..p.clone()
            })
            .collect();
        let mut writes: Vec<FilingWrite> = reads
            .iter()
            .zip(&changed)
            .map(|((read, _), placement)| FilingWrite::PlacementFrom { placement, read })
            .collect();
        writes.push(FilingWrite::Event {
            message_id: None,
            folder: folder.as_deref(),
            kind: "refile_marked",
            detail: json!({"marked": marked, "waiting_marked": waiting_marked, "category": opts.category, "folder": folder}),
        });
        if store.commit_filing(name, &writes, &now())? {
            return Ok(result);
        }
    }
    Err(err(5, "placements changed concurrently; retry"))
}
```

- [ ] **Step 4: Service and CLI (`src/service.rs`, `src/cli.rs`)**

In `src/service.rs`, after `filing_refile`:

```rust
    /// `filing refile --apply` (refile spec "Command"): with filing `live`,
    /// under the configuration lock and with `mailtriage.json` unchanged,
    /// marks the matching set; the next passes move it.
    pub fn filing_refile_apply(&mut self, name: &str, opts: RefileOptions) -> Result<Value> {
        let _config_lock = self.shared_config_lock()?;
        self.require_unchanged()?;
        let (account, generation) = self.ensure(name)?;
        if filing_mode(&account) != FilingMode::Live {
            return Err(err(2, "refile --apply requires filing mode live"));
        }
        refile::command::apply(&mut self.store, name, &account, &generation, &opts)
    }
```

In `src/cli.rs`: add `RefileOptions` to the `service::{…}` import list. Add to `enum FilingCommand` after `Backfill(BackfillArg),`:

```rust
    /// Move filed mail whose category changed into its new folder.
    Refile(RefileArg),
```

Add after `struct BackfillArg`:

```rust
#[derive(Args)]
struct RefileArg {
    #[arg(long)]
    account: String,
    /// Only messages whose new category is ID.
    #[arg(long)]
    category: Option<String>,
    /// Only messages in this folder (its server name, or its configured name).
    #[arg(long)]
    folder: Option<String>,
    /// Candidates to list (the whole set is always counted and marked).
    #[arg(long, default_value_t = 50, value_parser = parse_limit)]
    limit: usize,
    /// Mark the matching mail; the next passes move it (requires filing mode live).
    #[arg(long)]
    apply: bool,
}
```

In `fn filing`, add after the `FilingCommand::Backfill(arg) => { … }` arm:

```rust
        FilingCommand::Refile(arg) => {
            let opts = RefileOptions {
                category: arg.category.clone(),
                folder: arg.folder.clone(),
                limit: arg.limit,
            };
            if arg.apply {
                service.filing_refile_apply(&arg.account, opts)
            } else {
                service.filing_refile(&arg.account, opts)
            }
        }
```

- [ ] **Step 5: Agent and API docs**

Append to `docs/development/service-api.md`:

````markdown
## `filing refile` (schema v6)

```rust
// mailtriage::service::RefileOptions (= filing::refile::command::RefileOptions)
pub struct RefileOptions {
  pub category: Option<String>, // only candidates whose new category is this id
  pub folder: Option<String>,   // a folder's native name, or its configured name
  pub limit: usize,             // 1..=500, default 50; lists candidates only
}
impl Service {
  // Preview: no locks, no engine calls; may bring the generation up to date.
  pub fn filing_refile(&mut self, account: &str, opts: RefileOptions) -> Result<Value>;
  // Filing `live` only; shared configuration lock; `mailtriage.json` must be unchanged.
  pub fn filing_refile_apply(&mut self, account: &str, opts: RefileOptions) -> Result<Value>;
}
```

Preview result (filing `off`: the same shape, empty):

```json
{"schema_version":1,"account":"work","mode":"live",
 "candidates":[{"id":"msg_…","folder":"INBOX.Other","target":"INBOX.Updates","category":"updates","reason":"category_changed"}],
 "total":1,
 "folders":[{"folder":"Other","native":"INBOX.Other","retired":false,"candidates":1,"waiting":0}],
 "waiting":0,
 "skipped":{"not_filed_by_mailtriage":0,"corrected":0,"pinned":0,"blocked":0,"done":0,"open_intent":0,"explicit_target":0,"multiple_copies":0,"incomplete_input":0,"retired_frozen":0,"target_unusable":0,"target_inbox_or_source":0}}
```

`reason` is `category_changed` or `folder_retired`. Candidates and waiting messages are ordered by native folder, then UID; `folders` by native name. `--category` filters `candidates` and `skipped`, not `waiting`.

Apply result: `{"schema_version":1,"account":"work","marked":N,"waiting_marked":W}`. `marked` counts candidates newly marked, `waiting_marked` waiting messages newly marked (only with `folder` and no `category`). Repeating it marks 0.

Errors: 2 for `--apply` outside `live` (`refile --apply requires filing mode live`), an unknown category, a folder that names no category or retired folder (`unknown folder: not a category or retired folder`) or several (`folder name matches several folders; pass the native name: A, B`), a limit outside 1..=500, an unknown account; 3 when the state database is unavailable; 5 when `mailtriage.json` changed (`reason: config_changed`) or the placements kept changing (`placements changed concurrently; retry`).

Schema v6 (migration 6): `placements.refile_once`, `placements.filed_home_folder`/`filed_home_epoch`/`filed_home_uid` (the occurrence a COPYUID-proven mailtriage move produced; kept only while it is the known home), `filing_intents.consumes_refile`, `folders.drain_until_uid` (NULL: frozen; 0: retained in the last pass; N: draining until UID N). The newer-schema guard is 6.

Also: `filing status` gains `refile_marked` and `refile_candidates`; `categories apply` gains `hint` (null with filing `off`); `filing plan` refile moves carry `"reason":"refile"`; events `refile_marked`, `refile_cleared {reason}`, `refile_cancelled {intent_id, reason}`, and `moved` with `"reason":"refile"`.
````

Append to the end of the "## Filing into folders" section of `docs/agents/index.md` (after its last paragraph, before the next `##` heading if any):

````markdown
### Refiling after category changes

After the user adds, removes or re-points categories, mail that mailtriage already filed stays in its old folder until it is refiled. `filing refile` previews which filed mail would follow its new category. It reads local state only and is safe to repeat.

```sh
/opt/mailtriage/mailtriage filing refile --account work --json
/opt/mailtriage/mailtriage filing refile --account work --folder INBOX.Promotions --json
/opt/mailtriage/mailtriage filing refile --account work --folder INBOX.Promotions --apply --json
```

- `categories apply` returns a `hint`; run the preview once the next passes have classified open mail again.
- Report `total`, the `folders` entries (`retired: true`: no category uses the folder any more) and `waiting` (mail still being classified again) to the user.
- Run `--apply` only when the user asked to move the mail. It needs filing `live` (exit 2 otherwise) and marks the whole matching set; the following passes move it. Repeating it is harmless: `marked` and `waiting_marked` count only new marks.
- Pass a folder's `native` name to `--folder`. A configured name that two folders share exits 2 and lists their native names.
- `skipped` explains what stays. Mail the user corrected, pinned, marked done or moved (`not_filed_by_mailtriage`) is never refiled; do not try to move it. `explicit_target`: a request for a removed category is pending; `correct --category NEW` replaces it. `retired_frozen`: the mail sits in a retired folder mailtriage no longer watches.
- `filing status` reports `refile_marked` and `refile_candidates`; `filing log` shows `refile_marked`, `refile_cleared` and `refile_cancelled` events, and `moved` events with `"reason": "refile"`.
````

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --locked --test refile_command && cargo test --locked --test refile_preview && cargo test --locked --test filing_cli`
Expected: all PASS.

- [ ] **Step 7: Full check and commit**

Run: `cargo fmt && cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`, then `cargo build && ./target/debug/mailtriage filing refile --help` (prints the five flags).

```bash
git add src/filing/refile/command.rs src/service.rs src/cli.rs tests/refile_support/mod.rs tests/refile_command.rs docs/development/service-api.md docs/agents/index.md
git commit -m "Add filing refile --apply and the CLI command

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Retired folders: retention, draining and freezing; the guide

**Files:**
- Create: `src/filing/refile/retired.rs`, `tests/refile_retired.rs`
- Modify: `src/filing/refile/mod.rs` (`pub mod retired;`)
- Modify: `src/filing/refile/rules.rs` (`retained`, one unit test)
- Modify: `src/filing/store.rs` (`drain_states`, `set_drain_until_uid`, `drain_finished`, `freeze_retired`)
- Modify: `src/store.rs` (`capture_rescan_set` resnapshots a drain)
- Modify: `src/filing/observe.rs` (`resolve_folders` scope, `retire`)
- Modify: `src/filing/done.rs` (`settled`)
- Modify: `docs/guide.md` (new subsection), `design/specs/2026-10-06-filing-refile-design.md` (status line)

**Interfaces:**
- Consumes (Tasks 3, 5, 6): `rules::{input, placement_skip, RefileInput}`, `Store::frozen_folders`, `Store::discovery_epoch`; test support `apply`, `preview`, `scoped`, `remove_category`.
- Produces:
  - `rules::retained(refile: &RefileInput) -> BTreeSet<String>`.
  - `refile::retired::RetiredRefs { retained: BTreeSet<String>, drains: BTreeMap<String, u64> }` with `load(store: &Store, ctx: &PassContext) -> Result<Self>` and `holds(&self, native: &str) -> bool`.
  - `refile::retired::step(store: &mut Store, ctx: &PassContext, refs: &RetiredRefs, native: &str, listed: bool, referenced: bool, summary: &mut FilingSummary) -> Result<bool>`.
  - `Store::drain_states(&self, account: &str) -> Result<BTreeMap<String, u64>>`, `Store::set_drain_until_uid(&mut self, account: &str, native: &str, until: Option<u64>) -> Result<()>`, `Store::drain_finished(&self, account: &str, native: &str) -> Result<bool>`, `Store::freeze_retired(&mut self, account: &str, native: &str) -> Result<()>`.

- [ ] **Step 1: Write the failing tests**

Add to the test module of `src/filing/refile/rules.rs`:

```rust
    #[test]
    fn only_mail_that_passes_rules_one_to_five_retains_its_folder() {
        let fact = |home: &str| RefileFacts {
            at_filed_home: true,
            home_folder: Some(home.into()),
            single_occurrence: true,
            ..Default::default()
        };
        let mut refile = RefileInput::default();
        refile.facts.insert("kept".into(), fact("Kept"));
        let mut done = fact("Done");
        done.done = true;
        let mut corrected = fact("Corrected");
        corrected.corrected = true;
        let mut pinned = fact("Pinned");
        pinned.pinned = true;
        let mut moved = fact("Moved");
        moved.at_filed_home = false;
        for (id, f) in [("done", done), ("corrected", corrected), ("pinned", pinned), ("moved", moved)] {
            refile.facts.insert(id.into(), f);
        }
        assert_eq!(retained(&refile), BTreeSet::from(["Kept".to_string()]));
    }
```

Create `tests/refile_retired.rs`:

```rust
//! Refile spec "Retired folders": retention, draining and freezing.
mod common;
mod refile_support;
use common::{mail, Harness};
use mailtriage::{
    domain::{FilingMode::Live, MailboxSnapshot, SourceEnvelope},
    filing::{FolderRecord, LocationState, StageOptions},
    store::Store,
};
use refile_support::{
    apply, deliver_filed, events, filed_home, home, located, placement, preview, remove_category,
    review_state, scoped,
};
use serde_json::json;
use std::collections::BTreeMap;

fn snapshot_calls(h: &Harness, folder: &str) -> usize {
    let call = format!("snapshot {folder}");
    h.fake.calls().iter().filter(|c| **c == call).count()
}

#[test]
fn a_repointed_folder_stays_watched_until_its_mail_moved_then_freezes() {
    let h = Harness::new(Live);
    h.sync();
    let id = deliver_filed(&h, "n", "Weekly newsletter", "Our newsletter");
    let mut categories = h.service().config.accounts["work"].categories.clone();
    categories
        .iter_mut()
        .find(|c| c.id == "newsletters")
        .unwrap()
        .folder = Some("News".into());
    let out = h.service().apply_categories("work", categories).unwrap();
    assert!(out["hint"].is_string());
    h.sync(); // News is created; Newsletters is retired and kept by n
    assert!(scoped(&h, "Newsletters"));
    let v = preview(&h, None, Some("Newsletters"));
    assert_eq!(
        v["candidates"],
        json!([{"id": id, "folder": "Newsletters", "target": "News", "category": "newsletters", "reason": "folder_retired"}])
    );
    assert_eq!(apply(&h, None, Some("Newsletters"))["marked"], 1);
    for _ in 0..4 {
        h.sync();
    }
    assert_eq!(located(&h, "n").0, "News");
    let p = placement(&h, &id);
    assert_eq!(filed_home(&p), home(&p));
    assert!(!scoped(&h, "Newsletters"), "drained and frozen");
    assert!(!h.service().store.drain_states("work").unwrap().contains_key("Newsletters"));
    assert!(h.fake.uids("Newsletters").is_empty(), "the emptied folder stays on the server");
}

#[test]
fn removing_a_category_moves_its_waiting_mail_once_classified() {
    let h = Harness::new(Live);
    h.sync();
    let id = deliver_filed(&h, "n", "Weekly newsletter", "Our newsletter");
    remove_category(&h, "newsletters");
    let v = preview(&h, None, Some("Newsletters"));
    assert_eq!((v["total"].clone(), v["waiting"].clone()), (json!(0), json!(1)));
    assert_eq!(
        v["folders"],
        json!([{"folder": "Newsletters", "native": "Newsletters", "retired": true, "candidates": 0, "waiting": 1}])
    );
    assert_eq!(
        apply(&h, None, Some("Newsletters")),
        json!({"schema_version": 1, "account": "work", "marked": 0, "waiting_marked": 1})
    );
    for _ in 0..5 {
        h.sync();
    }
    assert_eq!(located(&h, "n").0, "Other");
    assert!(!placement(&h, &id).refile_once);
    assert!(!scoped(&h, "Newsletters"));
}

#[test]
fn a_draining_folder_is_watched_until_mail_moved_into_it_is_found() {
    let h = Harness::new(Live);
    h.sync();
    deliver_filed(&h, "a", "Weekly newsletter", "Our newsletter");
    let b = deliver_filed(&h, "b", "System update", "Version 2 is out");
    let fillers: Vec<u64> = (0..3)
        .map(|i| {
            h.fake.deliver(
                "INBOX",
                &mail(&format!("f{i}"), &format!("Lunch {i}"), "See you at noon"),
            )
        })
        .collect();
    h.sync();
    remove_category(&h, "newsletters");
    h.sync(); // Newsletters is retired and kept by a, classified again into `other`
    assert!(scoped(&h, "Newsletters"));
    assert_eq!(apply(&h, None, Some("Newsletters"))["marked"], 1);
    // Three messages, then b, go into Newsletters: b lies beyond one pass's
    // discovery window.
    for uid in fillers {
        h.fake.client_move("INBOX", uid, "Newsletters");
    }
    let (folder, uid) = located(&h, "b");
    h.fake.client_move(&folder, uid, "Newsletters");
    for pass in 0..40 {
        h.service().sync("work", 1).unwrap();
        assert_eq!(review_state(&h, &b), "open", "pass {pass}: b is never inferred done");
        if !scoped(&h, "Newsletters") {
            break;
        }
    }
    assert!(!scoped(&h, "Newsletters"), "frozen once drained");
    let p = placement(&h, &b);
    assert_eq!(
        (p.home_folder.as_deref(), p.location_state),
        (Some("Newsletters"), LocationState::Known)
    );
    assert_eq!(located(&h, "a").0, "Other");
}

#[test]
fn done_mail_alone_does_not_keep_a_retired_folder_watched() {
    let h = Harness::new(Live);
    h.sync();
    let id = deliver_filed(&h, "d", "Weekly newsletter", "Our newsletter");
    h.service().review("work", &id, true).unwrap();
    remove_category(&h, "newsletters");
    let before = snapshot_calls(&h, "Newsletters");
    h.sync();
    h.sync();
    assert_eq!(snapshot_calls(&h, "Newsletters"), before, "frozen at once");
    assert_eq!(filed_home(&placement(&h, &id)), None, "a frozen folder holds no filed home");
    h.service().review("work", &id, false).unwrap();
    h.sync();
    h.sync();
    assert_eq!(snapshot_calls(&h, "Newsletters"), before, "never retained again");
    assert_eq!(preview(&h, None, Some("Newsletters"))["skipped"]["retired_frozen"], 1);
}

#[test]
fn a_folder_frozen_before_v6_is_reported_and_never_watched_again() {
    let h = Harness::new(Live);
    h.sync();
    let id = deliver_filed(&h, "n", "Weekly newsletter", "Our newsletter");
    {
        // As migration v6 leaves mail in a folder retired before it: no filed home.
        let mut s = h.service();
        let mut p = s.store.placement("work", &id).unwrap().unwrap();
        p.filed_home_folder = None;
        assert!(s.store.save_placement(&p, None).unwrap());
    }
    remove_category(&h, "newsletters");
    let before = snapshot_calls(&h, "Newsletters");
    for _ in 0..3 {
        h.sync();
    }
    let v = preview(&h, None, Some("Newsletters"));
    assert_eq!((v["total"].clone(), v["skipped"]["retired_frozen"].clone()), (json!(0), json!(1)));
    assert_eq!(
        apply(&h, None, Some("Newsletters")),
        json!({"schema_version": 1, "account": "work", "marked": 0, "waiting_marked": 0})
    );
    h.sync();
    h.sync();
    assert_eq!(located(&h, "n").0, "Newsletters");
    assert_eq!(snapshot_calls(&h, "Newsletters"), before, "never watched again");
}

#[test]
fn a_retired_folder_deleted_on_the_server_freezes_and_clears_its_marks() {
    let h = Harness::new(Live);
    h.sync();
    let id = deliver_filed(&h, "n", "Weekly newsletter", "Our newsletter");
    remove_category(&h, "newsletters");
    h.sync();
    assert_eq!(apply(&h, None, Some("Newsletters"))["marked"], 1);
    h.fake.remove_folder("Newsletters");
    let out = h.sync();
    assert_eq!(out["filing"]["errors"], 0, "{out}");
    assert!(!placement(&h, &id).refile_once);
    assert_eq!(
        events(&h, "refile_cleared")[0]["detail"]["reason"],
        "not_filed_by_mailtriage"
    );
    assert!(!h.fake.calls().iter().any(|c| c.starts_with("move Newsletters")));
    h.sync();
    assert!(!scoped(&h, "Newsletters"));
}

fn store() -> (tempfile::TempDir, Store) {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(&d.path().join("db")).unwrap();
    (d, s)
}

fn snap(epoch: u64, next: u64) -> MailboxSnapshot {
    MailboxSnapshot {
        uid_validity: epoch,
        uid_next: next,
    }
}

fn retired(native: &str) -> FolderRecord {
    FolderRecord {
        account: "work".into(),
        native: native.into(),
        configured: Some(native.into()),
        category_id: Some("old".into()),
        origin: Some("created".into()),
        state: "retired".into(),
        role_verified: true,
        confirmed: false,
        subscribed: true,
        pause_reason: None,
        epoch: None,
        watch_from_uid: None,
        rescan_epoch: None,
        rescan_below_uid: None,
        rescan_complete: true,
        checked_at: None,
        error: None,
    }
}

#[test]
fn draining_ends_once_discovery_passed_the_snapshot_and_nothing_waits() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    s.save_folder(&retired("Old")).unwrap();
    s.checkpoint_start_at("work", "Old", &snap(3, 8), 7).unwrap();
    s.set_drain_until_uid("work", "Old", Some(10)).unwrap();
    assert!(!s.drain_finished("work", "Old").unwrap(), "discovery is at UID 7");
    let none = BTreeMap::new();
    let opts = StageOptions {
        record_arrivals: true,
        known_targets: &none,
        rescan_filter: None,
    };
    let env = SourceEnvelope {
        uid: 9,
        subject: "s".into(),
        message_id: Some("<b@t>".into()),
        size: Some(10),
        ..Default::default()
    };
    s.stage_with("work", "Old", 3, 9, &[env], "g1", true, &opts).unwrap();
    assert!(!s.drain_finished("work", "Old").unwrap(), "an arrival it found is pending");
    let arrival = s.arrivals("work", Some("pending")).unwrap().remove(0);
    s.resolve_arrival(arrival.id, "resolved", Some("extra"), "2026-10-06T12:00:00+00:00")
        .unwrap();
    assert!(s.drain_finished("work", "Old").unwrap());
    assert_eq!(s.drain_states("work").unwrap(), BTreeMap::from([("Old".to_string(), 10)]));
    s.freeze_retired("work", "Old").unwrap();
    assert!(s.drain_states("work").unwrap().is_empty());
}

#[test]
fn an_epoch_reset_while_draining_records_a_new_snapshot() {
    let (_d, mut s) = store();
    s.ensure_account("work", "id", "g1").unwrap();
    for native in ["Draining", "Kept"] {
        s.save_folder(&retired(native)).unwrap();
        s.checkpoint_start_at("work", native, &snap(3, 8), 7).unwrap();
    }
    s.set_drain_until_uid("work", "Draining", Some(8)).unwrap();
    s.set_drain_until_uid("work", "Kept", Some(0)).unwrap();
    s.checkpoint("work", "Draining", &snap(4, 12)).unwrap();
    s.checkpoint("work", "Kept", &snap(5, 20)).unwrap();
    assert_eq!(
        s.drain_states("work").unwrap(),
        BTreeMap::from([("Draining".to_string(), 12), ("Kept".to_string(), 0)])
    );
    assert!(!s.drain_finished("work", "Draining").unwrap(), "the reset rescan runs first");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --locked --lib rules && cargo test --locked --test refile_retired`
Expected: compile errors — `retained`, `drain_states`, `set_drain_until_uid`, `drain_finished`, `freeze_retired` not found.

- [ ] **Step 3: Store (`src/filing/store.rs`, `src/store.rs`)**

Add to `impl Store` in `src/filing/store.rs`:

```rust
    /// Refile spec "Retired folders": `drain_until_uid` where it is set
    /// (0: retained in an earlier pass; N: draining until UID N).
    pub fn drain_states(&self, account: &str) -> Result<BTreeMap<String, u64>> {
        let mut st = self.db.prepare(
            "SELECT native, drain_until_uid FROM folders WHERE account=? AND drain_until_uid IS NOT NULL",
        )?;
        let rows = st
            .query_map([account], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
        Ok(rows)
    }

    pub fn set_drain_until_uid(
        &mut self,
        account: &str,
        native: &str,
        until: Option<u64>,
    ) -> Result<()> {
        self.db.execute(
            "UPDATE folders SET drain_until_uid=? WHERE account=? AND native=?",
            params![until, account, native],
        )?;
        Ok(())
    }

    /// Draining is finished once discovery passed the snapshot in the
    /// checkpoint's epoch (a reset records a new snapshot), no reset rescan
    /// runs, no arrival in the folder is pending, and no known home in it
    /// lost its occurrence unnoticed.
    pub fn drain_finished(&self, account: &str, native: &str) -> Result<bool> {
        Ok(self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM folders f JOIN checkpoints c ON c.account=f.account AND c.mailbox=f.native
  WHERE f.account=?1 AND f.native=?2 AND f.drain_until_uid>0 AND c.last_uid>=f.drain_until_uid-1
  AND NOT (f.rescan_complete=0 AND f.rescan_epoch IS NOT NULL))
 AND NOT EXISTS(SELECT 1 FROM arrivals WHERE account=?1 AND folder=?2 AND state='pending')
 AND NOT EXISTS(SELECT 1 FROM placements p JOIN checkpoints c ON c.account=p.account AND c.mailbox=p.home_folder AND c.epoch=p.home_epoch
  WHERE p.account=?1 AND p.home_folder=?2 AND p.location_state='known'
  AND NOT EXISTS(SELECT 1 FROM occurrences o WHERE o.account=p.account AND o.mailbox=p.home_folder AND o.epoch=p.home_epoch AND o.uid=p.home_uid AND o.message_id=p.message_id))",
            params![account, native],
            |r| r.get(0),
        )?)
    }

    /// Freezes a retired folder for refile: no drain state and no filed
    /// home in it, so it is never retained again; in one transaction.
    pub fn freeze_retired(&mut self, account: &str, native: &str) -> Result<()> {
        let tx = self.db.transaction()?;
        let changed = tx.execute(
            "UPDATE folders SET drain_until_uid=NULL WHERE account=?1 AND native=?2 AND drain_until_uid IS NOT NULL",
            params![account, native],
        )? + tx.execute(
            "UPDATE placements SET filed_home_folder=NULL,filed_home_epoch=NULL,filed_home_uid=NULL WHERE account=?1 AND filed_home_folder=?2",
            params![account, native],
        )?;
        if changed > 0 {
            bump(&tx)?;
        }
        tx.commit()?;
        Ok(())
    }
```

In `src/store.rs` `capture_rescan_set`, change the folders `UPDATE` to:

```rust
    // Refile spec "Draining": an epoch change during draining records a new
    // snapshot (the reset-time UIDNEXT).
    tx.execute(
        "UPDATE folders SET rescan_epoch=?3,rescan_below_uid=?4,rescan_complete=0,epoch=?3,watch_from_uid=NULL,drain_until_uid=CASE WHEN drain_until_uid>0 THEN ?4 ELSE drain_until_uid END WHERE account=?1 AND native=?2",
        params![account, mailbox, new, snapshot.uid_next],
    )?;
```

- [ ] **Step 4: Retention and draining (`src/filing/refile/rules.rs`, `src/filing/refile/retired.rs`, `src/filing/refile/mod.rs`)**

In `rules.rs` add after `placement_skip`:

```rust
/// Refile spec "Retired folders", retention: the home folders of
/// placements that pass candidate rules 1–5.
pub fn retained(refile: &RefileInput) -> BTreeSet<String> {
    let none = BTreeSet::new();
    refile
        .facts
        .values()
        .filter(|f| placement_skip(f, &none).is_none())
        .filter_map(|f| f.home_folder.clone())
        .collect()
}
```

Add `pub mod retired;` to `src/filing/refile/mod.rs` (after `pub mod intents;`). Create `src/filing/refile/retired.rs`:

```rust
//! Refile spec "Retired folders": retention and draining keep a retired
//! folder watched while it holds refile-eligible mail, or while mail moved
//! into it may still be undiscovered; afterwards it freezes for good.
use crate::filing::refile::rules;
use crate::filing::{is_config_changed, FilingSummary, PassContext};
use crate::store::Store;
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};

/// This pass's retention and drain state, read before the watch scope is set.
#[derive(Debug, Clone, Default)]
pub struct RetiredRefs {
    /// Home folders of placements that pass candidate rules 1–5.
    pub retained: BTreeSet<String>,
    /// `folders.drain_until_uid` where set.
    pub drains: BTreeMap<String, u64>,
}

impl RetiredRefs {
    pub fn load(store: &Store, ctx: &PassContext) -> Result<Self> {
        let refile = rules::input(store, ctx.account, ctx.cfg, &BTreeSet::new())?;
        Ok(Self {
            retained: rules::retained(&refile),
            drains: store.drain_states(ctx.account)?,
        })
    }

    /// Whether the engine scope must include retired folder `native`.
    pub fn holds(&self, native: &str) -> bool {
        self.retained.contains(native) || self.drains.contains_key(native)
    }
}

/// One retired folder this pass. Returns whether retention or draining
/// keeps it watched. A folder neither retained, draining nor `referenced` by
/// an open intent, revert, pending arrival or rescan set is frozen; so is
/// one LIST no longer reports.
pub fn step(
    store: &mut Store,
    ctx: &PassContext,
    refs: &RetiredRefs,
    native: &str,
    listed: bool,
    referenced: bool,
    summary: &mut FilingSummary,
) -> Result<bool> {
    if !listed {
        store.freeze_retired(ctx.account, native)?;
        return Ok(false);
    }
    let drain = refs.drains.get(native).copied();
    if refs.retained.contains(native) {
        if drain != Some(0) {
            store.set_drain_until_uid(ctx.account, native, Some(0))?;
        }
        return Ok(true);
    }
    match drain {
        // Retention ended since the last pass: record the folder's UIDNEXT.
        Some(0) => match ctx.engine.snapshot(native) {
            Ok(s) if store.discovery_epoch(ctx.account, native)? == Some(s.uid_validity) => {
                store.set_drain_until_uid(ctx.account, native, Some(s.uid_next.max(1)))?;
                Ok(true)
            }
            // Another epoch: discovery records the reset first; snapshot again next pass.
            Ok(_) => Ok(true),
            Err(e) if is_config_changed(&e) => Err(e),
            Err(_) => {
                summary.errors += 1;
                summary.problems.push(format!("drain_snapshot_failed:{native}"));
                Ok(true)
            }
        },
        Some(_) if !referenced && store.drain_finished(ctx.account, native)? => {
            store.freeze_retired(ctx.account, native)?;
            Ok(false)
        }
        Some(_) => Ok(true),
        None => {
            if !referenced {
                store.freeze_retired(ctx.account, native)?;
            }
            Ok(false)
        }
    }
}
```

- [ ] **Step 5: Watching and done inference (`src/filing/observe.rs`, `src/filing/done.rs`)**

In `observe.rs` add `use super::refile::retired::{self, RetiredRefs};`. In `resolve_folders`, replace the block from `let referenced = …` through the `scope.extend(…);` statement with:

```rust
    let referenced = referenced_folders(store, ctx.account)?;
    let refs = RetiredRefs::load(store, ctx)?;
    let mut scope: BTreeSet<String> = map.sources.iter().chain(&natives).cloned().collect();
    scope.extend(
        retiring
            .iter()
            .filter(|r| referenced.contains(&r.native) || refs.holds(&r.native))
            .map(|r| r.native.clone()),
    );
```

and change the last call to `retire(store, ctx, &mut map, retiring, &referenced, &refs, summary)?;`. Replace `fn retire` with:

```rust
/// Rule 7: a recorded category folder no category uses any more is
/// `retired`; it stays watched while listed and still referenced, or while
/// refile retention or draining keeps it (refile spec "Retired folders").
fn retire(
    store: &mut Store,
    ctx: &PassContext,
    map: &mut FolderMap,
    retiring: Vec<FolderRecord>,
    referenced: &BTreeSet<String>,
    refs: &RetiredRefs,
    summary: &mut FilingSummary,
) -> Result<()> {
    for mut rec in retiring {
        if rec.state != "retired" {
            rec.state = "retired".into();
            rec.checked_at = Some(ctx.now.clone());
            store.save_folder(&rec)?;
        }
        let listed = map.listed.contains(&rec.native);
        let referenced = referenced.contains(&rec.native);
        let kept = retired::step(store, ctx, refs, &rec.native, listed, referenced, summary)?;
        if listed && (referenced || kept) {
            map.watch(rec.native, WatchRole::Retired);
        }
    }
    Ok(())
}
```

In `done.rs`, in `settled`, after the `if folders.iter().any(|r| r.pause_reason.is_some()) { … }` check add `let drains = store.drain_states(ctx.account)?;`, and inside the `for w in &map.watch` loop, after the existing `if !complete || rescanning { return Ok(false); }`, add:

```rust
        // Refile spec "Draining": a draining folder counts as still being scanned.
        let draining = drains.get(&w.folder).is_some_and(|until| *until > 0);
        if draining && !store.drain_finished(ctx.account, &w.folder)? {
            return Ok(false);
        }
```

- [ ] **Step 6: The guide and the spec status**

In `design/specs/2026-10-06-filing-refile-design.md` change the `Status:` line to `Status: Implemented (plan: design/plans/2026-10-06-filing-refile.md). Design approved in conversation; written spec revised after three Codex review rounds.`

Insert a new subsection in `docs/guide.md` directly before `### Provider check`:

````markdown
### Refiling after category changes

Automatic filing moves mail only out of the source folders. When you add, remove or re-point categories, the mail that mailtriage already filed stays where it is until you refile it. `filing refile` shows which filed mail would follow its new category, and `--apply` lets the next passes move it:

```sh
mailtriage filing refile --account work --json
mailtriage filing refile --account work --category updates --json
mailtriage filing refile --account work --folder Promotions --apply --json
```

Refiling moves only mail that is still exactly where a mailtriage move put it, as the server confirmed when the move ran. Mail you moved (even back into the same folder), corrected, pinned or marked done stays where it is, and nothing is ever refiled into `INBOX` or another source folder. Mail filed before this version without a server-reported destination (COPYUID) cannot be refiled.

| Change | What to do |
| --- | --- |
| Add a category | `categories apply`, let a few passes classify open mail again, then preview with `filing refile --category NEW` and move with `--apply`. |
| Rename a category (`name` only) | Nothing moves; its folder stays. |
| Re-point its `folder` | `categories apply`. The next live pass creates the new folder and retires the old one. `filing refile --folder OLD --apply` moves the old folder's mail, including mail still being classified again. |
| Remove a category | `categories apply` retires its folder and classifies its open mail again. `filing refile --folder OLD --apply` moves that mail into the remaining categories' folders once it is classified. |

With filing on, `categories apply` returns a `hint` naming the command. The preview makes no mailbox calls and reports:

| Field | Content |
| --- | --- |
| `candidates` | Messages that would move, by folder and UID, at most `--limit` (1 to 500, default 50): `id`, `folder` and `target` (server folder names), `category` (the new category) and `reason` (`category_changed`, or `folder_retired` when no category uses the folder any more). |
| `total` | All candidates, without the limit. |
| `folders` | One entry per folder holding candidates or waiting mail: `folder` (the configured name, `null` when unknown), `native` (the server name; pass it to `--folder`), `retired`, `candidates`, `waiting`. |
| `waiting` | Messages still to be classified again; whether they move is decided then. |
| `skipped` | What stays, counted by reason (below). |

`--folder NAME` takes a folder's server name or its configured name; a configured name two folders share exits 2 and lists their server names. `--category ID` limits the candidates to one new category; waiting mail is reported but not marked, so run the command again once it is classified. With `--folder` and no `--category`, `--apply` also marks the waiting mail, which then moves once classified. `--apply` needs `live`, marks the whole matching set whatever `--limit` says, and is safe to repeat: `marked` and `waiting_marked` count only new marks. The moves happen over the following passes, at most `filing.max_actions_per_pass` per pass, with every safeguard of other moves; a refile adds no flag.

| `skipped` reason | Meaning |
| --- | --- |
| `not_filed_by_mailtriage` | The message is not where a mailtriage move put it: you moved it, a rescan or recovery placed it, or it was filed without COPYUID. |
| `corrected`, `pinned`, `done` | Your correction, pin or Done wins. |
| `blocked`, `open_intent` | A block or an unfinished move; see `filing status`. |
| `explicit_target` | A request to move it into a category that no longer exists is pending; `correct --account NAME --id ID --category NEW` replaces it. |
| `multiple_copies` | It is in more than one folder. |
| `incomplete_input` | It was classified from incomplete content. |
| `retired_frozen` | It is in a retired folder mailtriage no longer watches. |
| `target_unusable` | Its new folder is paused, missing or awaiting `filing adopt`. A marked message waits for it. |
| `target_inbox_or_source` | Its new category keeps mail in `INBOX` or a source folder. |

A retired folder stays watched while it holds mail that can still be refiled and, after that mail has moved, until mailtriage has scanned everything moved into it before then. After that it is no longer watched and the mail in it is not refiled. mailtriage never renames or deletes it; delete the emptied folder in your mail client when you like.

A mark is dropped, with a `refile_cleared` event naming the reason, when the message is corrected, pinned, marked done, moved, copied, or classified into its own folder, into `INBOX` or a source folder, or from incomplete content. `filing status` reports `refile_marked` and `refile_candidates`. `filing log` shows `refile_marked`, `refile_cleared`, `refile_cancelled` and `moved` events with `"reason": "refile"`.

Exit codes: 2 for `--apply` outside `live`, an unknown `--category`, a `--folder` that names no category or retired folder (or two of them), a `--limit` outside 1 to 500, or an unknown account; 3 when the state database is unavailable; 5 when `mailtriage.json` changed during `--apply` or the placements kept changing.
````

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test --locked --lib rules && cargo test --locked --test refile_retired && cargo test --locked --test filing_observe --test filing_location --test refile_preview --test refile_command`
Expected: all PASS; `renamed_category_retires_its_old_folder_and_stops_watching_it` still passes (an empty retired folder freezes at once).

- [ ] **Step 8: Full check and commit**

Run: `cargo fmt && cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked`, then `cargo build && ./target/debug/mailtriage filing refile --help >/dev/null && echo ok`.

```bash
git add src/filing/refile/mod.rs src/filing/refile/rules.rs src/filing/refile/retired.rs src/filing/store.rs src/store.rs src/filing/observe.rs src/filing/done.rs tests/refile_retired.rs docs/guide.md design/specs/2026-10-06-filing-refile-design.md
git commit -m "Keep retired folders watched while refile needs them, then drain and freeze

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Spec coverage

| Spec section | Task |
| --- | --- |
| Filed home: proof, no proof, invalidation | 1 (invariant, epoch reset), 2 (grant, tests) |
| Filed home: migration v6 | 1 |
| Command: preview fields, `--category`, `--folder`, `--limit` | 5 |
| Command: `--apply`, `marked`, `waiting_marked` | 6 |
| Candidates, rules 1–8, `explicit_target` | 3 (rules), 5 (reported) |
| Storage (schema v6, guard 6) | 1 |
| Each pass: mark upkeep | 4 |
| Each pass: planning rule | 3 |
| Intents (claim and retry checks, mark consumed) | 2 (consumption), 4 (checks) |
| Retired folders: retention, draining, frozen | 7 (`retired_frozen` reported from 3/5) |
| Limits, flags and safety | 3 (cap, no flag), 6 (spread over passes, flag test) |
| Visibility: `filing plan`, `filing log`, `filing status`, `categories apply` | 3, 2/4/6, 5, 5 |
| Workflows | 3/6 (add a category), 7 (re-point, remove); a rename keeps its folder through `categories apply` (existing test `rename_keeps_folder_after_enable`), so nothing becomes a candidate |
| Errors and exit codes | 5, 6 |
| Testing list | 1–7 (each spec test has a named test) |
| Docs (guide, hermes, service-api, status) | 6, 7 |
