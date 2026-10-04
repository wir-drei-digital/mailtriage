# IMAP category filing

Date: 2026-10-04
Status: Design approved in conversation; written spec revised after review
round 3 (final Codex round).
Scope: first of two projects. The system tray app is a separate, later spec
that consumes this one.

## Goal

Make mailtriage's classification visible in every mail client by filing mail
into server-side IMAP folders, so the user keeps an empty inbox without a new
mail app. Agents (Claude, Hermes) keep using the CLI; every capability in this
spec is a CLI command with JSON output.

This spec lifts the first-release "never move mail" boundary of
[`docs/design.md`](../../design.md) for accounts that explicitly enable filing.
Everything else in that design (discovery, classification, overrides, Done,
read-only fetch, full-fingerprint identity) remains in force.

## Decisions

| Topic | Decision |
| --- | --- |
| Inbox model | Every classified message moves to its category folder by default. A category may target `INBOX`, which means "leave it in the source folder". |
| Importance | `\Flagged` only, set when the effective decision is action required or high urgency. No custom keywords. |
| Existing mail | New mail only. Backfill over N days or the whole inbox is an explicit command. Every filing action has a dry run. |
| Manual moves in a mail client | Moving into another category's folder is a category correction. Moving back to a source folder pins the message there. Archiving or deleting (leaving all watched folders) marks it done locally. |
| Folder location | Top level, named after the category. An existing folder with that name is adopted when its role can be verified, otherwise after explicit confirmation. |
| Providers | Gmail / Google Workspace, Microsoft 365 / Outlook.com, iCloud, Fastmail / Dovecot. All first-class. |
| Engine | Himalaya v2.1.0 now, behind a `MailEngine` trait so a native IMAP engine can replace it later. |

## Safety invariants

Each invariant has at least one dedicated test.

1. The engine boundary exposes no destructive operation: no delete, expunge,
   flag removal, `\Seen` change, folder delete or folder rename.
2. In filing modes `off` and `dry_run` the engine receives zero write calls
   (create, subscribe, move, store).
3. Every move, revert and flag is journaled as an intent in SQLite before the
   engine call, and every persisted intent state has a defined recovery. A
   crash at any point converges without a duplicate move, a repeated flag, or
   a fabricated correction.
4. Automatic filing moves mail only out of configured source folders. Moves
   from other folders happen only for an explicit request (correction, pin,
   unpin) or to revert an epoch race.
5. Every write runs in one IMAP session that first SELECTs the folder, and the
   engine reports the UIDVALIDITY that session observed. Because UIDVALIDITY
   only increases, a session epoch equal to the epoch in which the batch was
   verified proves the write addressed the verified messages. A different
   session epoch is always detected and handled (see Accepted risks).
6. Filing refuses to write on a server without the MOVE extension. There is no
   COPY + `\Deleted` + EXPUNGE fallback.
7. A message is moved automatically at most once and receives at most one
   automatic flag attempt, ever. An ambiguous flag outcome is never retried, so
   a user's unflag is never overridden.
8. Automatic moves and flags use only a classification produced under the
   current classification generation.
9. Identity is established only by an epoch-bound COPYUID mapping or by the
   full raw-content fingerprint. Message-ID, size and dates only narrow
   candidates.
10. Writes never touch a folder under a safety pause or a message under a
    block or quarantine; both are lifted only by an explicit command.

## Architecture

### MailEngine trait

`src/engine/mod.rs` defines the boundary. The service depends only on this
trait; `src/engine/himalaya.rs` (the current `src/himalaya.rs`, moved and
extended) implements it; `src/engine/fake.rs` is an in-memory implementation
for tests. A later native IMAP engine is a third implementation.

```rust
pub struct EngineCapabilities {
    pub move_supported: bool,
    pub uidplus: bool,                     // COPYUID available
    pub special_use: bool,                 // RFC 6154 advertised
    pub delimiter: Option<char>,
    pub personal_prefix: String,           // first personal namespace, "" if none
}
pub struct FolderInfo {
    pub name: String,                      // native name
    pub attributes: Vec<String>,           // e.g. "\\Noselect", "\\HasChildren"
    pub roles: Option<Vec<String>>,        // special-use roles; None = unknown
    pub subscribed: bool,
}
pub struct SourceEnvelope {                // extended; new fields optional
    pub uid: u64,
    pub subject: String,
    pub from: Vec<Address>,
    pub sent_at: Option<String>,
    pub message_id: Option<String>,
    pub internal_date: Option<String>,     // INTERNALDATE as RFC 3339 UTC
    pub size: Option<u64>,
    pub flags: Vec<String>,
}
pub struct CopyUid { pub target_epoch: u64, pub pairs: Vec<(u64, u64)> } // (source, target)
pub struct WriteOutcome {
    pub selected: bool,                    // a1 SELECT returned OK
    pub session_epoch: Option<u64>,        // UIDVALIDITY seen by that SELECT
    pub completed: bool,                   // a2 returned OK
    pub copyuid: Option<CopyUid>,          // kept even when a2 returned NO
}

pub trait MailEngine {
    fn version(&self) -> Result<String>;
    fn capabilities(&self) -> Result<EngineCapabilities>;
    /// Stable JSON describing server + login identity, never secrets.
    fn binding_identity(&self) -> Result<serde_json::Value>;
    fn list_folders(&self) -> Result<Vec<FolderInfo>>;
    fn create_folder(&self, native: &str) -> Result<()>;
    fn subscribe_folder(&self, native: &str) -> Result<()>;   // idempotent
    fn snapshot(&self, folder: &str) -> Result<MailboxSnapshot>;
    /// Envelopes for UIDs in (after, through].
    fn discover(&self, folder: &str, after: u64, through: u64) -> Result<Vec<SourceEnvelope>>;
    /// Envelopes for exactly these UIDs; absent UIDs are omitted.
    fn envelopes(&self, folder: &str, uids: &[u64]) -> Result<Vec<SourceEnvelope>>;
    /// Raw RFC 5322 bytes; never sets \Seen.
    fn fetch_raw(&self, folder: &str, uid: u64) -> Result<Vec<u8>>;
    /// One session: SELECT folder; UID MOVE uids target.
    fn move_messages(&self, folder: &str, uids: &[u64], target: &str) -> Result<WriteOutcome>;
    /// One session: SELECT folder; UID STORE uids +FLAGS.SILENT (\Flagged).
    fn add_flagged(&self, folder: &str, uids: &[u64]) -> Result<WriteOutcome>;
}
```

Any parsed response is an `Ok(WriteOutcome)`, including NO/BAD completions, so
partial results such as a COPYUID on a failed MOVE are never lost. On timeout
the Himalaya engine still parses the stdout captured before it killed the
process (today `Himalaya::run` discards it) and returns what that output
proves. `Err` means not even the SELECT result was captured; the outcome is
then unknown.

Engine construction is a single factory, `engine::open(&EngineConfig) ->
Result<Box<dyn MailEngine>>`. The service never names `Himalaya` directly. All
folder arguments are native names.

### Himalaya command mapping

All commands keep the current invariants: argument arrays without a shell,
stdin closed, `--backend imap`, timeout and output limits, stderr discarded,
UID mode (never `--seq`), never `--seen`.

| Trait method | Himalaya v2.1.0 command |
| --- | --- |
| `version` | `--version` (unchanged) |
| `capabilities` | `imap raw` with `CAPABILITY` and `NAMESPACE` pipelined; delimiter from LIST |
| `list_folders` | `--json imap list --all` and `--json imap list` (subscribed); when SPECIAL-USE is advertised, also `imap raw` `LIST "" "*" RETURN (SPECIAL-USE)` for roles |
| `create_folder` | `imap create NAME` |
| `subscribe_folder` | `imap subscribe NAME` |
| `snapshot` | `--json imap status FOLDER` (unchanged) |
| `discover` / `envelopes` | `--json imap fetch --mailbox F --envelope --flags --internal-date --size UIDSET` |
| `fetch_raw` | `message read --mailbox F --raw UID` (unchanged; see aliases) |
| `move_messages` | `imap raw` with `a1 SELECT "F"` and `a2 UID MOVE UIDSET "T"` pipelined |
| `add_flagged` | `imap raw` with `a1 SELECT "F"` and `a2 UID STORE UIDSET +FLAGS.SILENT (\Flagged)` pipelined |

- **Raw responses.** A strict line parser reads tagged completions
  (`OK`/`NO`/`BAD`), `[UIDVALIDITY n]` from SELECT, `[COPYUID epoch src dst]`
  from MOVE, CAPABILITY and NAMESPACE data, and `* LIST (attrs) delim name`
  lines. Literal-form names (`{n}`) in LIST are an error for that folder.
  Everything else is ignored.
- **Quoting.** The engine quotes mailbox names as IMAP quoted strings, escaping
  `"` and `\`. Non-ASCII names are supported only if the provider check
  confirms how Himalaya's LIST JSON represents them; otherwise validation
  restricts folder names to printable ASCII.
- **UID sets** are comma-joined and capped at 100 UIDs per call.
- **Configuration binding.** At engine open the Himalaya TOML is read once,
  its account binding and alias table validated, and its bytes hashed. Every
  subprocess spawn re-hashes the file first and refuses with a binding error
  (exit 5) if it changed. The account binding identity is also re-verified
  immediately before each write batch and before each recovery observation.
- **Folder aliases.** `message read --mailbox` resolves Himalaya's
  `[mailbox.alias]` table case-insensitively, while the `imap` commands use
  native names. Discovery, fetch and filing stop for a watched folder whose
  name case-insensitively equals an alias key mapping to a different native
  name; `doctor` and `filing status` report the conflict.

### Module layout

| Path | Responsibility |
| --- | --- |
| `src/engine/mod.rs` | Trait, engine types, factory |
| `src/engine/himalaya.rs` | Himalaya implementation (moved from `src/himalaya.rs`) |
| `src/engine/raw.rs` | Raw IMAP response parser |
| `src/engine/fake.rs` | In-memory mailbox: folders, UIDs, epochs, flags, roles, COPYUID on/off, call log, fault injection, simulated client moves and epoch resets |
| `src/filing/planner.rs` | Pure planner: state in, actions out |
| `src/filing/mod.rs` | Pass orchestration |
| `src/store.rs` | Schema v3, filing persistence, filing-aware merge |
| `src/service.rs` | Calls `filing` within `sync`; filing commands; atomic transitions for `correct` |
| `src/cli.rs` | `filing` subcommands |

`mailtriage::himalaya` stays available as a re-export of
`mailtriage::engine::himalaya` so external code and existing tests compile.

## Configuration

`AppConfig.schema_version` becomes 2. `load` accepts 1 and 2; `save` writes 2.

```json
"accounts": {
  "work": {
    "engine": { "kind": "himalaya", "binary": "...", "config": "...", "account": "...",
                "mailboxes": ["INBOX"], "expected_version": "2.1.0",
                "timeout_seconds": 30, "max_output_bytes": 20000000 },
    "filing": { "mode": "off", "flag": true, "max_actions_per_pass": 200 },
    "categories": [
      { "id": "newsletters", "name": "Newsletters", "folder": "Newsletters",
        "description": "...", "examples": [], "catch_all": false }
    ]
  }
}
```

- `engine` is a tagged enum (`kind`). A legacy `himalaya` object is read as
  `engine: {kind: "himalaya", ...}`. A config containing both is invalid.
- `engine.mailboxes` are the **source folders**. Mail is filed only out of these.
- `filing` is optional; absent means `{mode: "off", flag: true,
  max_actions_per_pass: 200}`. `mode` is `off`, `dry_run` or `live`.
  `max_actions_per_pass` is 1..=1000.
- `categories[].folder` is optional; absent means the category `name`. The
  literal `INBOX` means "stay in the source folder".
- `filing` and `folder` are excluded from the classification generation hash.
  `filing` is not part of `policy`.
- The account binding identity for a Himalaya engine must hash to exactly the
  value the current release produces from the legacy `himalaya` block. A test
  asserts this.

**Folder validation applies only when `filing.mode` is not `off`.** Configs
that never enable filing load exactly as today. `filing enable` validates first
and refuses with the list of categories that need an explicit `folder`. With
filing enabled, `config::validate` requires:

- each effective folder is trimmed, nonempty, at most 200 bytes, and a single
  path segment: no `/`, `.`, `*`, `%`, `"`, `\`, `&`, or control characters;
  printable ASCII unless non-ASCII support was confirmed. `&` is the
  modified UTF-7 shift character; it stays forbidden until the provider check
  confirms how Himalaya encodes mailbox names;
- every category whose folder is not `INBOX` has a unique folder, compared
  case-insensitively; any case variant of `inbox` other than the literal
  `INBOX` is rejected.

Collisions with source folders are checked online on resolved native names.

## State (SQLite schema v3)

Migration from v2 runs in one transaction. It adds columns and tables only;
placements for existing messages are created by the bootstrap (below).

- `messages` gains `rfc_message_id TEXT`, `size INTEGER` and
  `internal_date TEXT`, with an index on `(account, rfc_message_id)`. Every
  writer of `envelope` (including `attach`) merges fields instead of replacing
  transport metadata.
- `filing_state(account PK, mode, enabled_at, bootstrap_done INTEGER,
  last_pass TEXT)`.
- `placements(account, message_id PK, ...)`, one row per message with
  established identity and at least one occurrence:
  - observed: `home_folder`, `home_epoch`, `home_uid`, `location_state`
    (`known`, `ambiguous`, `absent`), `absent_since`;
  - desired: `desired_target` (category id, `@source`, or null for automatic),
    `pinned`, `eligible_once`, `desired_rev` (incremented by every desired
    change);
  - history: `source_folder`, `filed_at`, `filed_by` (`mailtriage`/`user`),
    `flag_attempted_at`, `flagged_at`, `done_inferred`;
  - safety: `blocked_reason` (`move_failed`, `duplicate_copy`, `quarantined`,
    `merge_conflict`).
- `folders(account, native PK, configured, category_id, origin, state,
  role_verified INTEGER, confirmed INTEGER, subscribed INTEGER, pause_reason,
  epoch, watch_from_uid, checked_at, error)`. `origin`: `created`/`adopted`.
  `state` (observed): `ok`, `missing`, `special_use`, `noselect`,
  `needs_confirmation`, `retired`, `error`. `pause_reason` (safety, explicit
  release only): null, `epoch_race` or `epoch_race_suspected`. `rescan_epoch`
  and `rescan_complete` record the epoch of the latest reset rescan and whether
  it finished.
- `arrivals(id PK, account, folder, epoch, uid, message_id, rfc_message_id,
  state, kind, intent_id, created_at, resolved_at)`. `state`: `pending`,
  `resolved`, `vanished` (the occurrence disappeared before its identity was
  established), `unresolved` (terminal fetch failure while the occurrence still
  exists), `dismissed` (an unresolved arrival the user explicitly dismissed).
  `kind` on resolution: `own_move`, `user_move`, `user_pin`, `extra`, `new`,
  `rescan`, `reverted`, `quarantined`.
- `rescan_sets(account, folder, epoch, message_id)`: messages to look for when
  a folder's epoch resets (see Folders).
- `filing_intents(id PK, account, message_id, kind, folder, epoch, uid, target,
  target_epoch, target_uid_next, target_uid, desired_rev, consumes_eligible
  INTEGER, batch, state, attempts, next_after, dispatched_at, created_at,
  updated_at, error)`. `kind`: `move` or `flag`. `state`: `in_flight`,
  `sent`, `uncertain`, `awaiting_rescan`, `applied`, `lost`, `failed`,
  `superseded`.
- `filing_reverts(id PK, account, parent_intent, folder, folder_epoch, uid,
  target, target_epoch, state, target_uid, created_at, updated_at, error)`:
  compensation for an epoch race. `folder`/`folder_epoch`/`uid` is the
  COPYUID destination locator of a message the race moved; `target` is the
  original source folder and `target_epoch` the session epoch the race ran in.
  It references no message identity.
- `filing_events(id PK, account, message_id, folder, at, kind, detail)`:
  append-only audit log. Kinds: `moved`, `flagged`, `client_correction`,
  `pinned`, `unpinned`, `relocated`, `archived_done`, `reopened`,
  `move_failed`, `flag_failed`, `duplicate_copy`, `epoch_race`,
  `epoch_race_reverted`, `folder_created`, `folder_adopted`,
  `folder_subscribed`, `folder_missing`, `folder_special_use`,
  `folder_needs_confirmation`, `location_ambiguous`, `arrival_unresolved`,
  `merge_conflict`, `released`.

## Identity and placements

`rfc_message_id` is the ENVELOPE Message-ID, trimmed and case-sensitive; null
when missing. Identical raw content always has the same Message-ID, so it is a
sound filter for "could this be the same message" (null matches null). Size and
INTERNALDATE are not used as filters, because a client may assign a new
INTERNALDATE when it transfers a message. Before a write, an occurrence is
re-verified by comparing its Message-ID and size with the stored values. None
of these establish identity.

**Every** newly discovered occurrence in a watched folder is committed together
with a `pending` arrival row, in the discovery transaction. Its identity is
established in one of two ways:

1. **COPYUID.** The occurrence is the target of a recorded COPYUID pair from a
   move whose session epoch equalled the source epoch and whose target epoch
   equals this folder's epoch: it joins that intent's message.
2. **Fingerprint.** Otherwise it becomes a provisional message (as today),
   is fetched, and `Store::attach` decides by full raw fingerprint.

Moves on servers without UIDPLUS therefore re-download each moved message once.

**A placement is created** when a message with an established fingerprint has
an occurrence and no placement yet: on first fetch of a newly discovered
message, when a merge gives an offline-ingested (`classify`) record its first
occurrence, and by the bootstrap. Provisional messages have no placement, so
filing commands on them fail with "identity not yet established; retry after
sync" (exit 5).

**Filing-aware merge.** When `attach` merges a provisional message into an
existing one, in one transaction it moves occurrences and arrivals to the
canonical message, merges envelope metadata, and resolves the provisional's
pending arrival against the canonical placement. A provisional message cannot
own intents or placement state. If the provisional message carries local edits
(overrides or Done), the existing refusal stands; the arrival becomes
`unresolved` with event `merge_conflict`, the canonical placement gets
`blocked_reason = merge_conflict`, and both stay listed in `filing status`
until the user clears the provisional edits and runs `filing retry --arrival`.

**Bootstrap.** On the first pass with mode not `off` (and until
`bootstrap_done`), placements are created for existing source-managed messages
with a fingerprint and occurrences: home = the occurrence in the first
configured source folder by configuration order, then lowest UID; if none is in
a source folder, the lowest (folder, UID). `source_folder` = that source folder,
else the first configured source folder. Envelope metadata (internal date,
size, flags, Message-ID) is hydrated with `envelopes()` in batches of 100 per
folder per pass. A placement is not eligible for automatic moves or flags
until hydrated; lacking an internal date it is never "new".

## Filing mode and "new mail"

`filing.mode` in the config is authoritative. Each pass compares it with
`filing_state.mode`:

- `off` → `dry_run` or `live`: set `enabled_at` to now and restart the
  bootstrap, so messages fetched while filing was off get placements.
- `dry_run` ↔ `live`: keep `enabled_at`.
- anything → `off`: keep the old value; the next enable sets a new one.

A message is **new** when its `internal_date` ≥ `enabled_at`.

## Sync pass order

`sync` (and each `watch` pass) runs under the existing account lock:

1. **Engine check.** Version; with mode not `off`, capabilities, alias check
   and configuration binding once per pass. Without MOVE support, writes are
   skipped and reported; discovery and classification continue.
2. **Folder resolution** (mode not `off`; see Folders).
3. **Discovery** of source folders, and with mode not `off` of category folders
   in state `ok` or `needs_confirmation`, plus retired or paused folders still
   referenced by open intents, pending arrivals or rescan sets. Every new
   occurrence gets a `pending` arrival.
4. **Reconciliation.** Existing bounded windows for every watched folder.
5. **Intent resolution** (mode not `off`), before any arrival inference.
6. **Fetch and classify** queued messages; merges and new placements happen
   here.
7. **Arrival resolution and placement re-evaluation** (mode not `off`).
8. **Bootstrap and hydration** batches (mode not `off`, until done).
9. **Plan and apply.** The pure planner runs (mode not `off`); in `live`,
   actions are claimed and applied (flags first, then moves, batched per
   (folder, target)), capped at `max_actions_per_pass`.
10. **Done inference** (mode not `off`).
11. **Summary.** `sync` output gains `filing: {mode, planned, moved, flagged,
    reverted, client_corrections, pinned, archived_done, quarantined,
    unresolved, errors}`. Any filing error makes the pass partial (exit 4).

With mode `off`, steps 2, 5, 7, 8, 9 and 10 are skipped and only source folders
are watched; category folder checkpoints resume when filing is enabled again.

## Planner

`plan(input) -> Plan` is a pure function in `src/filing/planner.rs`.
`Plan { folders_to_create: Vec<String>, actions: Vec<Action> }`;
`Action` is `Move { message_id, from: Locator, to: String, desired_rev,
consumes_eligible }` or `Flag { message_id, at: Locator }`. `filing plan` and
`dry_run` passes call the same function as `live`. In a preview, a folder that
would be created counts as resolvable.

**Effective decision** = the model decision with per-field overrides applied,
as `Service::item` does today. The `review_mode` reason does not block filing.
`review_state` (Done) does not affect filing.

**Current classification** = the message's `generation` equals the account's
current generation and its status is `ready` or `uncertain`.

**Desired folder.** `desired_target` resolves to a category's folder, or to
`source_folder` for `@source` and for a category whose folder is `INBOX`. With
no `desired_target`, an unpinned message whose home is a source folder desires
its effective category's folder when the automatic rules below hold; otherwise
it desires its home.

**Every action requires:** mode not `off`; a placement with
`location_state = known`, hydrated, not blocked; no open intent of the same
kind; a known home UID in the current epoch; home folder and target folder
without `pause_reason`; target folder in state `ok` (or would-create in a
preview).

**Moves.** A message gets a `Move` when its desired folder differs from its
home and:

- for an explicit `desired_target`: the home folder is a source folder, a
  category folder in state `ok`, or a retired folder that LIST still reports;
- for an automatic move: the home folder is a source folder; the message is
  not pinned; its classification is current and the effective category is set
  (a category from an override needs no current classification); the decision
  is not based on incomplete input unless the category came from an override;
  the category's folder is not `INBOX`; and either it is new with `filed_at`
  empty, or `eligible_once` is set (`consumes_eligible = true`).

**Flags.** A message gets a `Flag` when `filing.flag` is true,
`flag_attempted_at` is empty, its current envelope lacks `\Flagged`, and the
effective decision has `action_required == true` or `urgency == high`, where
each field comes from an override or a current classification. Like moves,
flags only apply to mail in scope: new mail, mail with `eligible_once`, or mail
already filed by mailtriage. Backlog mail is never flagged until it is
backfilled.

**Ordering and caps.** Oldest internal date first, flags before moves for the
same message, truncated at `max_actions_per_pass`.

## Placement transitions

Each transition is one SQLite transaction. Changes to the desired fields bump
`desired_rev`. Commands that change overrides do so with the existing
compare-and-swap in that same transaction.

| Trigger | Change |
| --- | --- |
| `correct` sets or clears `category_id` (filing not `off`) | `desired_target` = new effective category id; `pinned = 0`; clears `move_failed` block |
| `correct` of other fields | none |
| `filing pin` | `pinned = 1`; `desired_target = @source` (a no-op only if home is a source folder and no move intent is open) |
| `filing unpin` | `pinned = 0`; `desired_target` cleared; `eligible_once = 1` |
| `filing retry --id` | clears `move_failed`, `duplicate_copy` or `quarantined`; `eligible_once = 1` |
| Move applied | observed home updated; `filed_at`, `filed_by = mailtriage`; `desired_target` cleared only if the intent's `desired_rev` equals the current one; `eligible_once` cleared only if the intent has `consumes_eligible` and `desired_rev` still matches |
| Client move into category C | override `category_id = C` unless already C; home updated; `filed_by = user`; `pinned = 0`; `desired_target` cleared |
| Client move into a source folder | home updated; `pinned = 1`; `desired_target` cleared |
| Move fails terminally / duplicate copy / quarantine | `blocked_reason` set |

**Claiming.** In `live`, each action is claimed by one transaction that
re-reads the placement, checks the action's `desired_rev` equals the current
one and no blocking condition appeared, and writes the intent `in_flight`. A
stale action is dropped for this pass.

## Writes and recovery

### Moves

For each batch (folder F, verified epoch E, target T):

1. Re-verify the binding; re-read the batch's envelopes in F with `envelopes()`
   bracketed by `snapshot(F)` before and after. Drop UIDs that are absent or
   whose Message-ID or size differs from the stored values (their placements
   are re-evaluated); abort the batch if F's epoch is not E.
2. `snapshot(T)` → `target_epoch`, `target_uid_next`.
3. Claim the actions (intents `in_flight`, `dispatched_at = now`).
4. `move_messages(F, uids, T)`:
   - `selected` with `session_epoch == E`: record COPYUID pairs if any. If
     `completed`: intents → `sent`, source occurrences removed. If not
     completed: intents → `uncertain` (a partial MOVE may have happened).
   - `selected` with `session_epoch != E`: **epoch race** (see below).
   - not `selected`: nothing was written; intents → `uncertain`, resolved as
     "still in F".
   - `Err`: intents → `uncertain`.

**Intent recovery** (step 5 of each pass, and after any crash). Presence in a
folder is established only by an occurrence whose identity is established
(COPYUID or fingerprint) in that folder's current epoch.

| State | Recovery |
| --- | --- |
| `in_flight` | Treated as `uncertain`. |
| `uncertain`, F or T epoch now differs from the one recorded | **Suspected epoch race** (see Epoch race). Then `awaiting_rescan`. |
| `uncertain`, epochs unchanged | Re-verify binding, observe both ends. In T (COPYUID, or an identity-established arrival at `uid >= target_uid_next`) and not in F → `applied`. In both → `failed`, `blocked_reason = duplicate_copy`. In F only, with T scanned as for `lost` → if the intent's `desired_rev` still matches, retry (re-claimed, job backoff, up to `policy.max_attempts`, then `failed`, `blocked_reason = move_failed`); otherwise `superseded`. In neither → `sent`. |
| `sent` | Waits for its arrival. Becomes `lost` once T has been scanned, in `target_epoch`, through the `UIDNEXT - 1` of a snapshot taken after `dispatched_at`, and every arrival in T at `uid >= target_uid_next` is `resolved` or `vanished`; an `unresolved` arrival in that range keeps it `sent` until retried or dismissed. A lost move sets `location_state = absent`. |
| `sent`, T epoch changed | `awaiting_rescan`. |
| `awaiting_rescan` | Waits until every folder whose epoch changed (F, T, or both) has a complete reset rescan. Then old-epoch UIDs and bounds are discarded and the outcome is decided by identity-established occurrences in the current epochs only: in T only → `applied`; in both → `failed`, `duplicate_copy`; in F only → retry if `desired_rev` matches, else `superseded`; in neither → `lost`. |

`superseded` and `lost` close the intent without a block. `applied` follows the
"Move applied" transition, so a superseding request is never consumed by an
older intent. Arrivals at the target that belong to an open intent are resolved
only as that intent's outcome; client-move inference never runs for a message
with an open move intent or for an arrival quarantined or explained by a
revert.

### Epoch race

A write ran, or may have run, in a different epoch from the one its batch was
verified in, so it may have acted on other messages.

- **Detected move race with COPYUID.** For each reported pair, journal a
  `filing_reverts` row (destination locator T/`copyuid.target_epoch`/target
  UID, back to F in the race's session epoch, parent intent), then mark the
  pending arrivals at those T UIDs `quarantined` pending the revert. Each
  revert is dispatched like a move but verified differently: `snapshot(T)` must
  still report `copyuid.target_epoch`, the revert session's epoch must equal
  it, and the UID must still be present. Nothing is compared with any
  message's stored metadata, because the moved message's identity is unknown.
  The revert's own COPYUID locates the returned messages in F; arrivals there
  resolve as `reverted`. A completed revert closes its quarantined arrival
  (kind `reverted`, occurrence removed) and changes no placement. Event
  `epoch_race_reverted`. A revert is never itself reverted: a race or error
  during a revert falls through to the next case.
- **Detected move race without COPYUID, failed revert, or suspected race.** F
  gets `pause_reason = epoch_race` (`epoch_race_suspected` when no session
  outcome was captured). Every arrival in T at `uid >= target_uid_next` in
  `target_epoch` that no intent explains is resolved `quarantined`, and
  placements created from those arrivals get `blocked_reason = quarantined`.
  Quarantine is written in step 5, before arrival resolution in step 7, so it
  is never mistaken for a client move. Event `epoch_race`.
- **Flag race, detected or suspected.** F gets `pause_reason = epoch_race`;
  event `epoch_race` (another message may now be flagged; flags are never
  removed). The flag intent becomes `failed`.

The race batch's original move intents become `awaiting_rescan`.

### Flags

1. Re-verify binding and re-read the envelope as for moves; skip if it already
   has `\Flagged`.
2. Claim: in one transaction write the flag intent and set
   `flag_attempted_at`. This consumes the only automatic flag attempt.
3. `add_flagged(F, [uid])`, batched per folder:
   - `selected`, `session_epoch == E`, `completed` → `applied`, `flagged_at`,
     event `flagged`;
   - epoch race → as above;
   - anything else → `uncertain`. Next pass: if F's epoch changed, suspected
     flag race as above; otherwise re-read flags: `\Flagged` present →
     `applied`; absent → `failed`, event `flag_failed`. Never retried.

A crash between claim and dispatch leaves the message unflagged; that is the
price of never overriding a user's unflag.

## Arrivals and location

### Arrival resolution

Step 7 resolves `pending` arrivals whose identity is established, that are not
quarantined or explained by a revert, and whose message has no open intent:

| Arrival | Home UID still present (`envelopes` on `home_folder`/`home_epoch`/`home_uid`) | Result |
| --- | --- | --- |
| Category C's folder | no | `user_move`: client move into category C |
| A source folder | no | `user_pin`: client move into a source folder |
| Any watched folder | yes | `extra`: occurrence recorded, no change |
| Category C's folder, message had no placement | n/a | `new`: placement with home here, `filed_by = user`, client-correction override to C, never moved automatically |
| Source folder, message had no placement | n/a | `new`: ordinary new mail |
| Below a folder's reset-time `UIDNEXT` during its rescan | n/a | `rescan`: location update only (home re-established, or the placement re-evaluated); never a correction or pin |

An arrival whose occurrence disappears before its identity is established
becomes `vanished`. One whose provisional message fails to fetch terminally
while the occurrence still exists becomes `unresolved` (event
`arrival_unresolved`). `filing retry --arrival ID` requeues the fetch;
`filing dismiss --arrival ID` marks it `dismissed` after the user has reviewed
it. Both are listed in `filing status`.

### Placement re-evaluation

When a placement's home occurrence disappears (reconciliation, or a write-batch
check), and its message has no open intent:

- exactly one surviving occurrence in a watched folder → the matching client
  move transition (category folder → client move into that category; source
  folder → client move into a source folder, which pins); a surviving
  occurrence in the same category records only `relocated`. During a rescan,
  or for a `rescan` arrival, only the location is updated;
- several surviving occurrences → `location_state = ambiguous`, event
  `location_ambiguous`;
- none → `location_state = absent`, `absent_since = now`.

**Resolving ambiguity.** `filing pin`, `filing unpin`, `filing retry --id` and
`correct --category` on an ambiguous placement first select a home
deterministically: the occurrence in the newly desired folder if present, else
an occurrence in a source folder (configuration order, lowest UID), else the
lowest (folder, UID). The placement becomes `known`; other occurrences remain
extras.

### Done inference

An `absent` placement is marked done when all hold, checked and applied in one
transaction with `UPDATE ... WHERE review_state = 'open'` (an explicitly done
message is never touched and keeps `done_inferred = 0`):

- it is still absent from every watched, retired, paused or missing folder;
- every watched folder's checkpoint is complete in an epoch unchanged since
  `absent_since`, and no rescan is in progress;
- no `pending` or `unresolved` arrival and no quarantined, unreverted arrival
  exists in the account (an unidentified occurrence could be this message);
- no folder has a `pause_reason`;
- it has no open intent.

On success: `review_state = done`, `done_inferred = 1`, event `archived_done`.
If the message reappears, `location_state` returns to `known`, and when
`done_inferred = 1` it is reopened (`review_state = open`, `done_inferred = 0`,
event `reopened`). `done` and `reopen` commands set `done_inferred = 0`.

## Folders

Each pass with mode not `off`:

1. LIST and LSUB once; with SPECIAL-USE advertised, also the extended LIST for
   roles.
2. For each category whose folder is not `INBOX`, the native name is
   `personal_prefix + folder`. A native name equal to a source folder's native
   name is rejected (category paused, reported).
3. Present in LIST:
   - `\Noselect` or `\NonExistent` → `noselect`;
   - any role (`\Sent`, `\Trash`, `\Drafts`, `\Junk`, `\Archive`, `\All`,
     `\Flagged`) → `special_use`;
   - roles known and empty (`role_verified = 1`) → `ok`, adopted;
   - roles unknown (no SPECIAL-USE support) → `needs_confirmation` unless
     `confirmed = 1`, event `folder_needs_confirmation`; `filing adopt
     --folder NAME` sets `confirmed = 1`. A name on the denylist (`Sent`,
     `Sent Items`, `Sent Messages`, `Trash`, `Deleted Items`,
     `Deleted Messages`, `Bin`, `Drafts`, `Junk`, `Junk E-mail`, `Spam`,
     `Archive`, `All Mail`, `Starred`, `Important`, or under `[Gmail]` /
     `[Google Mail]`, case-insensitive) is `special_use` regardless.
4. Absent from LIST and never recorded → `create_folder` in `live` (state `ok`,
   origin `created`, `role_verified = 1`); a preview lists it in
   `folders_to_create`.
5. Absent but previously recorded → `missing`, event `folder_missing`; never
   recreated automatically. If it reappears it is re-resolved by rule 3.
6. Every `ok` category folder not in LSUB → `subscribe_folder` in `live`;
   `subscribed` is tracked separately, so a crash between create and subscribe
   completes next pass.
7. A recorded folder no longer referenced by any category → `retired`. It stays
   watched while open intents, reverts, pending arrivals or rescan sets
   reference it; afterwards its occurrences are frozen and still count as
   present for done inference.

`pause_reason` is independent of the observed state: rule 3 never clears it.
`filing retry --folder NAME` clears it (event `released`) and schedules a
rescan of that folder.

**First watch.** A category folder's checkpoint starts at its current
`UIDNEXT - 1`; pre-existing content is never ingested.

**Epoch reset of any watched folder.** Before the existing code deletes its
occurrences, the message ids of those occurrences, of placements whose home is
the folder, of open intents targeting it and of pending arrivals in it are
written to `rescan_sets`; `rescan_epoch` is set and `rescan_complete` cleared.
The folder is rescanned from UID 1. For a category folder, an envelope below
the reset-time `UIDNEXT` becomes a provisional message only if its
`rfc_message_id` equals that of a rescan-set member (null matches null); other
pre-existing content is not ingested. Source folders are rescanned in full, as
today. Identity is then established by fingerprint (arrival kind `rescan`).
`rescan_complete` is set when the scan reaches the reset-time `UIDNEXT - 1`
and every provisional message it created has resolved, vanished or become
unresolved. Rescan-set members not found are then re-evaluated.

## Commands

All take `--account` and `--json`, keep the existing response envelope and exit
codes, and are safe to retry.

| Command | Effect |
| --- | --- |
| `filing status` | Mode, `enabled_at`, bootstrap progress, MOVE/UIDPLUS/SPECIAL-USE support, folders (origin, state, pause, subscription), paused categories, intents by state, counts of blocked, quarantined, ambiguous, unresolved and eligible-but-unfiled messages, alias conflicts, last pass summary. Read-only; no engine calls. |
| `filing enable --mode dry-run\|live` | Validates folders, then writes `filing.mode` under the exclusive config lock. Idempotent. |
| `filing disable` | Writes `filing.mode = off`. Takes effect at the next pass. |
| `filing plan [--limit N]` | Read-only planner output: `folders_to_create` and actions. |
| `filing backfill (--days N \| --all) [--apply]` | Lists source-folder placements that would become eligible; `--apply` (requires `live`) sets `eligible_once` on them. `--days` uses the hydrated internal date. |
| `filing pin --id ID` / `filing unpin --id ID` | Placement transitions. |
| `filing retry --id ID \| --folder NAME \| --arrival ID` | Clears a message block or quarantine, releases a folder's safety pause, or requeues an unresolved arrival's fetch. |
| `filing dismiss --arrival ID` | Marks a reviewed unresolved arrival `dismissed`, lifting its done-inference barrier. |
| `filing adopt --folder NAME` | Confirms adoption of an existing folder whose role could not be verified. |
| `filing log [--id ID] [--limit N]` | Recent `filing_events`, newest first. |

Changes to existing commands:

- `list` / `read` items gain `placement: {folder, location_state, filed_by,
  pinned, flagged, blocked_reason, pending_action}` (null without a placement).
- `correct` performs the placement transition atomically with the override.
  With filing `off` it changes only the override, as today.
- `done` / `reopen` set `done_inferred = 0`.
- `doctor` gains `filing: {mode, move_supported, uidplus, special_use,
  personal_prefix, alias_conflicts, folders, problems}` when mode is not `off`;
  read-only engine calls only.
- `categories validate` / `apply` enforce the folder rules when filing is on.
- `init` writes the new config shape with `filing.mode = off`.

## Error handling

| Failure | Behaviour |
| --- | --- |
| MOVE or STORE error / partial | Intents `uncertain`; resolved by observation. |
| Epoch race | Reverted with COPYUID, else folder paused and arrivals quarantined. |
| Folder creation or subscription error | Folder `error`; category pauses; retried next pass. |
| Alias conflict | That folder's discovery and fetch stop; filing writes stop. |
| Himalaya config changed during a pass | Engine refuses further calls; pass ends with exit 5. |
| Authentication, throttling, network | Pass partial (exit 4); `watch` continues on its interval. |
| mailtriage config changed during a pass | Existing `require_unchanged` check before claiming writes. |

Error text never includes message bodies, subjects or credentials.

## Accepted risks

- **Verification-to-write window.** The Himalaya engine verifies a batch in one
  subprocess and writes in another, so a mailbox recreated in the milliseconds
  between them can be written to in its new epoch. Invariant 5 guarantees
  detection when the write's response is captured, and a write whose outcome
  was lost is treated as a suspected race whenever the folder's epoch changed;
  moves are reverted with COPYUID where available, otherwise the folder pauses
  and arrivals are quarantined; a mistaken flag stays (flags are never removed)
  and is reported. A native IMAP engine closes the window by
  checking SELECT's UIDVALIDITY before sending the write.
- **Re-download without UIDPLUS.** Identity of moved mail is established by
  fingerprint, costing one extra download per moved message.
- **Configuration change race.** The Himalaya TOML is re-hashed before every
  spawn; a change within microseconds of a spawn is not detected.

## Testing

- **Planner**: table-driven unit tests for every rule above, including explicit
  versus automatic precedence, pin with an open intent, `eligible_once`
  consumption, blocks, pauses, quarantine, `INBOX` targets, retired and
  preview folders, incomplete input, review mode, stale generation,
  unhydrated placements, ordering and the cap.
- **Raw parser**: SELECT/MOVE/STORE/CAPABILITY/NAMESPACE/LIST responses, with
  and without COPYUID, NO/BAD at each tag, partial COPYUID with NO, missing
  UIDVALIDITY, literals, untagged noise.
- **Service with `FakeEngine`**: every row of the transition, intent-recovery,
  arrival and re-evaluation tables; zero write calls in `off`/`dry_run`;
  crash injection before, during and after each write; epoch race with and
  without COPYUID, during a revert, suspected after a lost response, and for
  flags; reverts never touching placements; duplicate copy; stale claim
  dropped after a concurrent `correct` or `pin`; an uncertain intent superseded
  by a newer request closes without retry; newer correction survives an older
  intent; flag attempted once; done inference blocked by pending, unresolved
  and quarantined arrivals and by paused folders, never applied to explicit
  done, reversed on reappearance; target and source epoch resets during
  `sent` and `uncertain` moves resolved by `awaiting_rescan`; a client
  transfer that changes INTERNALDATE still found by rescan; timeout with
  partial output; bootstrap and
  hydration on an existing database; offline-ingested record gaining an
  occurrence; merge conflict; folder confirmation, pause release, missing and
  retired folders; adopted content not ingested; backfill in capped batches.
- **Config**: legacy configs load and save as `engine`; legacy category names
  load with filing off; binding identity hash unchanged; folder validation
  with filing on; generation hash unchanged by `filing` and `folder`;
  Himalaya TOML change detection.
- **Himalaya contract**: the fake-binary tests assert exact argument arrays and
  raw command text for every operation, quoting, UID mode, and that
  `expunge`, `delete`, `rename`, `-FLAGS`, `--action remove`, `--action set`,
  `--seq` and `--seen` never occur.
- **Dovecot end to end**: a Linux CI job runs the real Himalaya v2.1.0 release
  binary (pinned by checksum) against a Dovecot container with seeded mail, in
  two namespace layouts (no prefix with `/`, and `INBOX.` prefix with `.`), and
  exercises enable → live filing → client move → pin → archive → epoch reset.
- **Live provider check** (below).

## Live provider check

The first implementation task, before filing logic is built on the engine. It
needs one throwaway test account per provider, configured in Himalaya by the
user; no credentials enter this repository. For Gmail, Microsoft 365, iCloud
and Fastmail or a Dovecot host, record in `docs/verification.md`:

- `imap raw` output for pipelined `CAPABILITY`/`NAMESPACE`, `SELECT` +
  `UID MOVE` (UIDVALIDITY, COPYUID), `SELECT` + `UID STORE`, and
  `LIST "" "*" RETURN (SPECIAL-USE)`;
- `imap list --all --json` and `imap list --json` shapes, delimiter,
  attributes returned, and the representation of a non-ASCII folder name;
- the alias table location in the Himalaya TOML;
- create and subscribe for an ASCII and a non-ASCII name;
- INTERNALDATE, size and Message-ID before and after a move;
- `\Flagged` in the provider's web and mobile clients;
- `\Seen` unchanged by fetch, move and store;
- Gmail: label semantics of MOVE from INBOX, archive, and a message carrying
  two category labels;
- login rate: one `watch` with seven watched folders at a 60-second interval
  for 30 minutes without throttling; if throttled, whether pipelined STATUS via
  `imap raw` resolves it.

Outcome per provider: go, go with noted differences, or no-go. A no-go blocks
enabling `live` for that provider until resolved and is documented in the
README.

## Out of scope

System tray app and worker heartbeat (next project); native IMAP engine, IDLE
and connection reuse; provider-specific APIs; COPY + EXPUNGE fallback; renaming
or deleting server folders; custom IMAP keywords; `\Seen` changes;
conversation-level filing; Windows builds.
