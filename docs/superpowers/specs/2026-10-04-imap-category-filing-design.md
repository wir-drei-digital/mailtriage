# IMAP category filing

Date: 2026-10-04
Status: Design approved in conversation; written spec revised after review
round 1.
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
| Folder location | Top level, named after the category. An existing folder with that name is adopted. |
| Providers | Gmail / Google Workspace, Microsoft 365 / Outlook.com, iCloud, Fastmail / Dovecot. All first-class. |
| Engine | Himalaya v2.1.0 now, behind a `MailEngine` trait so a native IMAP engine can replace it later. |

## Safety invariants

Each invariant has at least one dedicated test.

1. The engine boundary exposes no destructive operation: no delete, expunge,
   flag removal, `\Seen` change, folder delete or folder rename.
2. In filing modes `off` and `dry_run` the engine receives zero write calls
   (create, subscribe, move, store).
3. Every move and flag is recorded as an intent in SQLite before the engine
   call. A crash at any point converges on the next pass without a duplicate
   move, a repeated flag, or a fabricated correction.
4. Automatic filing moves mail only out of configured source folders. Moves
   from category folders happen only for an explicit correction, pin or unpin.
5. Every write runs in one IMAP session that first SELECTs the folder; the
   engine reports the UIDVALIDITY that session observed. Immediately before
   each write batch, the service re-reads the batch's envelopes and drops any
   UID whose identity no longer matches. A write whose session saw a different
   epoch than expected is a detected epoch race: a move is reverted using
   COPYUID when the server provides it; otherwise the folder pauses and the
   affected arrivals are quarantined for review.
6. Filing refuses to enable on a server without the MOVE extension. There is no
   COPY + `\Deleted` + EXPUNGE fallback.
7. A message is moved automatically at most once and receives at most one
   automatic flag attempt, ever. An ambiguous flag outcome is never retried, so
   a user's unflag is never overridden.
8. Automatic moves and flags use only a classification produced under the
   current classification generation.

## Architecture

### MailEngine trait

`src/engine/mod.rs` defines the boundary. The service depends only on this
trait; `src/engine/himalaya.rs` (the current `src/himalaya.rs`, moved and
extended) implements it; `src/engine/fake.rs` is an in-memory implementation
for tests. A later native IMAP engine is a third implementation.

```rust
pub struct EngineCapabilities {
    pub move_supported: bool,
    pub uidplus: bool,                    // COPYUID available
    pub delimiter: Option<char>,
    pub personal_prefix: String,          // first personal namespace, e.g. "" or "INBOX."
}
pub struct FolderInfo {
    pub name: String,                     // native name, as LIST returns it
    pub attributes: Vec<String>,          // as LIST returns them, e.g. "\\Sent", "\\Noselect"
    pub subscribed: bool,                 // present in LSUB
}
pub struct SourceEnvelope {               // extended; new fields optional
    pub uid: u64,
    pub subject: String,
    pub from: Vec<Address>,
    pub sent_at: Option<String>,
    pub message_id: Option<String>,       // ENVELOPE message-id
    pub internal_date: Option<String>,    // INTERNALDATE as RFC 3339 UTC
    pub size: Option<u64>,                // RFC822.SIZE
    pub flags: Vec<String>,
}
pub struct MoveReport {
    pub session_epoch: u64,               // UIDVALIDITY seen by the SELECT in the move session
    pub copyuid: Option<CopyUid>,         // from the tagged/untagged COPYUID response
}
pub struct CopyUid { pub target_epoch: u64, pub pairs: Vec<(u64, u64)> } // (source uid, target uid)
pub struct StoreReport { pub session_epoch: u64 }

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
    /// SELECT folder, then UID MOVE uids target, in one session.
    fn move_messages(&self, folder: &str, uids: &[u64], target: &str) -> Result<MoveReport>;
    /// SELECT folder, then UID STORE uids +FLAGS.SILENT (\Flagged), in one session.
    fn add_flagged(&self, folder: &str, uids: &[u64]) -> Result<StoreReport>;
}
```

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
| `capabilities` | `imap raw` with `CAPABILITY` and `NAMESPACE` pipelined; LIST delimiter |
| `list_folders` | `--json imap list --all` (names, delimiter, attributes) and `--json imap list` (subscribed set) |
| `create_folder` | `imap create NAME` |
| `subscribe_folder` | `imap subscribe NAME` |
| `snapshot` | `--json imap status FOLDER` (unchanged) |
| `discover` / `envelopes` | `--json imap fetch --mailbox F --envelope --flags --internal-date --size UIDSET` |
| `fetch_raw` | `message read --mailbox F --raw UID` (unchanged; see aliases below) |
| `move_messages` | `imap raw` with `a1 SELECT "F"` and `a2 UID MOVE UIDSET "T"` pipelined |
| `add_flagged` | `imap raw` with `a1 SELECT "F"` and `a2 UID STORE UIDSET +FLAGS.SILENT (\Flagged)` pipelined |

- **Raw responses.** A strict line parser reads only the tagged completions
  (`a1`/`a2` `OK`/`NO`/`BAD`), `[UIDVALIDITY n]` from the SELECT, and
  `[COPYUID epoch src-set dst-set]` from the MOVE. Any other content is
  ignored. A non-`OK` `a1` means nothing was written; a non-`OK` `a2` is an
  error. Missing `UIDVALIDITY` is an error.
- **Quoting.** The engine quotes mailbox names as IMAP quoted strings,
  escaping `"` and `\`. Names outside printable ASCII are encoded as modified
  UTF-7 only if the provider check confirms Himalaya's LIST JSON returns
  decoded names; otherwise validation restricts folder names to printable
  ASCII.
- **UID sets** are comma-joined and capped at 100 UIDs per call.
- **Folder aliases.** `message read --mailbox` resolves Himalaya's
  `[mailbox.alias]` table case-insensitively, while the `imap` commands use
  native names. The engine reads that account's alias table from the Himalaya
  TOML and fails `doctor`, discovery and filing when any watched folder name
  case-insensitively equals an alias key that maps to a different native name.
- **Special-use.** Himalaya's LIST is a plain LIST without
  `RETURN (SPECIAL-USE)`, so attributes are used when the server sends them and
  a name denylist applies regardless (see Folders).

### Module layout

| Path | Responsibility |
| --- | --- |
| `src/engine/mod.rs` | Trait, engine types, factory |
| `src/engine/himalaya.rs` | Himalaya implementation (moved from `src/himalaya.rs`), raw response parser |
| `src/engine/fake.rs` | In-memory mailbox: folders, UIDs, epochs, flags, COPYUID on/off, call log, fault injection, simulated user moves and epoch resets |
| `src/filing/planner.rs` | Pure planner: state in, actions out |
| `src/filing/mod.rs` | Pass orchestration: folder resolution, arrival resolution, intent application, placement re-evaluation, done inference |
| `src/store.rs` | Schema v3 and filing persistence, filing-aware merge |
| `src/service.rs` | Calls `filing` within `sync`; new filing commands; atomic placement transitions for `correct` |
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
- `filing` and `folder` are excluded from the classification generation hash,
  so changing them never reclassifies mail. `filing` is not part of `policy`.
- The account binding identity for a Himalaya engine must hash to exactly the
  value the current release produces from the legacy `himalaya` block, so
  existing state directories keep working. A test asserts this.

**Folder validation applies only when `filing.mode` is not `off`.** Configs
that never enable filing load exactly as today, whatever their category names.
`filing enable` validates first and refuses with the list of categories that
need an explicit `folder`. With filing enabled, `config::validate` requires:

- each effective folder is trimmed, nonempty, at most 200 bytes, and a single
  path segment: no `/`, `.`, `*`, `%`, `"`, `\`, or control characters;
  printable ASCII unless non-ASCII support was confirmed (see Himalaya
  mapping);
- every category whose folder is not `INBOX` has a unique folder, compared
  case-insensitively; any case variant of `inbox` other than the literal
  `INBOX` is rejected.

Collisions with source folders are checked online on resolved native names
(see Folders), because a personal namespace prefix can make distinct
configured strings name the same mailbox.

## State (SQLite schema v3)

Migration from v2 runs in one transaction, like the existing migrations.

- `messages` gains `match_key TEXT` and `internal_date TEXT`, with an index on
  `(account, match_key)`. The extended envelope is kept in `envelope` JSON;
  every writer of `envelope` (including `attach`) merges fields instead of
  replacing transport metadata.
- `filing_state(account PK, mode TEXT, enabled_at TEXT, last_pass TEXT)`.
- `placements(account, message_id PK, source_folder TEXT, home_folder TEXT,
  home_epoch INTEGER, home_uid INTEGER, filed_at TEXT, filed_by TEXT,
  pinned INTEGER, eligible_once INTEGER, requested_target TEXT,
  request_rev INTEGER, flag_attempted_at TEXT, flagged_at TEXT,
  blocked_reason TEXT, absent_since TEXT, done_inferred INTEGER,
  location_state TEXT)`.
  - `source_folder`: the source folder the message was first discovered in
    (the first configured source folder if it was first seen elsewhere).
  - `home_*`: where the message is expected to live, retained even after the
    occurrence row is removed.
  - `requested_target`: an explicit target, either a category id or `@source`;
    resolved to a native folder at plan time, so a category's folder change
    retargets pending requests.
  - `request_rev`: incremented by every placement transition.
  - `location_state`: `known`, `ambiguous` or `absent`.
- `folders(account, native PK, configured TEXT, category_id TEXT,
  origin TEXT, state TEXT, subscribed INTEGER, watch_from_uid INTEGER,
  epoch INTEGER, rescan_below_uid INTEGER, checked_at TEXT, error TEXT)`.
  `origin` is `created` or `adopted`; `state` is `ok`, `missing`,
  `special_use`, `noselect`, `retired`, `paused` or `error`.
- `arrivals(id PK, account, folder, epoch, uid, message_id, candidate_id,
  intent_id, state TEXT, kind TEXT, created_at, resolved_at)`: written in the
  same transaction as the discovered occurrence. `state` is `pending` or
  `resolved`; `kind` is `own_move`, `user_move`, `pinned`, `extra`,
  `unknown_new`, `rescan` or `quarantined`.
- `filing_intents(id PK, account, message_id, kind TEXT, folder TEXT,
  epoch INTEGER, uid INTEGER, target TEXT, target_epoch INTEGER,
  target_uid_next INTEGER, target_uid INTEGER, request_rev INTEGER,
  batch TEXT, state TEXT, attempts INTEGER, next_after TEXT, created_at,
  updated_at, error TEXT)`. `kind` is `move` or `flag`. `target_epoch` and
  `target_uid_next` are the target's snapshot taken before dispatch;
  `target_uid` comes from COPYUID when available.
- `filing_events(id PK, account, message_id, at, kind TEXT, detail TEXT)`:
  append-only audit log. Kinds: `moved`, `flagged`, `client_correction`,
  `pinned`, `unpinned`, `relocated`, `archived_done`, `reopened`,
  `move_failed`, `flag_failed`, `duplicate_copy`, `epoch_race`,
  `epoch_race_reverted`, `folder_created`, `folder_adopted`,
  `folder_subscribed`, `folder_missing`, `folder_special_use`,
  `location_ambiguous`.

## Identity

`match_key` = SHA-256 of `message_id` (trimmed, case-sensitive), `size`, and
`internal_date` normalized to UTC RFC 3339 with seconds. It is null when any
part is missing. It selects candidates; it is not proof of identity:

| Situation | Identity established by |
| --- | --- |
| Arrival of our own move, COPYUID available | the COPYUID pair (exact) |
| Arrival of our own move, no COPYUID | exactly one candidate, it has a `sent` or `uncertain` move intent to this folder, same `target_epoch`, and `uid >= target_uid_next` |
| Rescan of a category folder after its epoch reset | exactly one candidate whose `home_folder` is this folder |
| Anything else (user moves, server rules, ambiguous keys) | full raw-content fingerprint, as today |

Every write batch re-reads its envelopes first; a UID whose `match_key` (or,
when null, subject + sender + size) differs from the stored values is dropped
from the batch and its placement is re-evaluated.

## Filing mode and "new mail"

`filing.mode` in the config is authoritative. Each pass compares it with
`filing_state.mode`:

- `off` → `dry_run` or `live`: set `enabled_at` to now.
- `dry_run` ↔ `live`: keep `enabled_at`, so mail that arrived during a dry run
  is filed when going live.
- anything → `off`: keep the old value; the next enable sets a new one.

A message is **new** when its `internal_date` ≥ `enabled_at`. A message with no
internal date is never new; only backfill, a correction, or an unpin makes it
eligible.

## Sync pass order

`sync` (and therefore each `watch` pass) runs, under the existing account lock:

1. **Engine check.** Version; with mode not `off`, capabilities and the alias
   check once per pass. Missing MOVE support in `live` stops filing writes for
   the pass and is reported; discovery and classification continue.
2. **Folder resolution** (mode not `off`; see Folders).
3. **Discovery.** Source folders as today; with mode not `off`, also category
   folders in state `ok`, and retired folders still referenced by open intents
   or pending arrivals. Each new occurrence is committed together with an
   `arrivals` row when it is a candidate match, a COPYUID target, or lands in a
   non-source folder. Identity rules above decide whether the occurrence joins
   an existing message or creates a provisional one.
4. **Reconciliation.** Existing bounded windows for every watched folder. When
   a removed occurrence is a placement's home, the placement is re-evaluated.
5. **Arrival and intent resolution** (mode not `off`).
6. **Fetch and classify** queued messages, then resolve arrivals whose identity
   the fingerprint merge just established.
7. **Plan** (mode not `off`). The pure planner computes actions.
8. **Apply** (mode `live`). Flags first, in the message's current folder; then
   moves, batched per (source folder, target folder). Total actions are capped
   at `max_actions_per_pass`; the remainder waits.
9. **Done inference** (mode not `off`).
10. **Summary.** `sync` output gains `filing: {mode, planned, moved, flagged,
    client_corrections, pinned, archived_done, quarantined, errors}`. Any
    filing error makes the pass partial (exit 4).

With mode `off`, steps 2, 5, 7, 8 and 9 are skipped and only source folders are
watched; category folder checkpoints resume where they stopped when filing is
enabled again.

## Planner

`plan(input) -> Plan` is a pure function in `src/filing/planner.rs`.
`Plan { folders_to_create: Vec<String>, actions: Vec<Action> }`;
`Action` is `Move { message_id, from: Locator, to: String, request_rev }` or
`Flag { message_id, at: Locator }`. `filing plan` and `dry_run` passes call the
same function as `live`. In a preview, a folder that would be created counts as
resolvable, so the preview shows the moves a live pass would make.

**Effective decision** = the model decision with per-field overrides applied,
as `Service::item` does today. The `review_mode` reason does not block filing:
filing has its own dry run. `review_state` (Done) does not affect filing.

**Current classification** = the message's `generation` equals the account's
current generation and its status is `ready` or `uncertain`.

**Every move requires:** mode is not `off`; `location_state = known`; the home
folder is a source folder, a category folder in state `ok`, or (for explicit
moves only) a retired folder that LIST still reports; a known home UID in the
current epoch; no open move intent; `blocked_reason` empty; and a resolved
target that differs from home and is resolvable.

**Explicit moves** take precedence. When `requested_target` is set, it resolves
to that category's folder, or to `source_folder` for `@source` or for a
category whose folder is `INBOX`. If that equals the home folder, the request
is cleared (rev-checked) without an action; otherwise the message gets a
`Move` from a source, category or retired folder.

**Automatic moves.** Otherwise a message gets a `Move` to its effective
category's folder when all hold:

- its home folder is a source folder and it is not pinned;
- its classification is current and the effective category is set (model
  confident, or corrected by the user); a category set by an override needs no
  current classification;
- the decision is not based on incomplete input (`input_incomplete`) unless the
  category was corrected by the user;
- the category's folder is not `INBOX`;
- either (a) it is new and `filed_at` is empty, or (b) `eligible_once` is set.

**Flags.** A message gets a `Flag` when `filing.flag` is true, mode is not
`off`, `flag_attempted_at` is empty, no flag intent exists, its current envelope
lacks `\Flagged`, and the effective decision has `action_required == true` or
`urgency == high`, where each field comes from an override or a current
classification. Flag eligibility does not depend on "new".

**Ordering and caps.** Actions are ordered by internal date, oldest first,
flags before moves for the same message, and truncated at
`max_actions_per_pass`.

## Placement transitions

Each transition is one SQLite transaction that bumps `request_rev`. Commands
that change overrides do so with the existing compare-and-swap in that same
transaction.

| Trigger | Change |
| --- | --- |
| `correct` sets or clears `category_id` (filing not `off`) | `requested_target` = new effective category id; `pinned = 0`; `blocked_reason` cleared |
| `correct` of other fields | none |
| `filing pin` | `pinned = 1`; `requested_target = @source` if home is not a source folder, else cleared; `blocked_reason` cleared |
| `filing unpin` | `pinned = 0`; `eligible_once = 1`; `requested_target` cleared; `blocked_reason` cleared |
| `filing retry` | `blocked_reason` cleared; `eligible_once = 1` |
| Move intent applied | home updated; `filed_at`, `filed_by = mailtriage`; `eligible_once` cleared; `requested_target` cleared only if the intent's `request_rev` equals the current one |
| Client correction (user move into category C) | override `category_id = C` unless already C; home updated; `filed_by = user`; `pinned = 0`; `requested_target` cleared |
| User move into a source folder | home updated; `pinned = 1`; `requested_target` cleared |
| Move fails terminally, or a duplicate copy is found | `blocked_reason` set |

The planner never plans for a blocked message. Blocks are cleared only by an
explicit transition above.

## Writes and recovery

### Moves

For each batch (folder F at epoch E, target T):

1. Re-read the batch's envelopes in F and drop mismatches (see Identity).
2. Snapshot T: `target_epoch`, `target_uid_next`.
3. Write the intents `in_flight` with locators, target snapshot and
   `request_rev`.
4. Call `move_messages`.
   - Success and `session_epoch == E`: intents → `sent`, source occurrences
     removed, COPYUID target UIDs stored. Arrivals at those UIDs resolve as
     `own_move` → `applied`.
   - Success but `session_epoch != E` (epoch race): with COPYUID, move the
     reported target UIDs back to F's new epoch the same way, event
     `epoch_race_reverted`; without COPYUID, pause F, event `epoch_race`, and
     quarantine every arrival in T at `uid >= target_uid_next` in
     `target_epoch` that no intent explains (ingested, no correction, listed in
     `filing status`). The batch's intents become `uncertain`.
   - Error: intents → `uncertain`.
5. Resolving `uncertain` on a later pass observes both endpoints:
   - found in T (by the identity rules) and absent from F → `applied`;
   - found in T and still in F → `failed`, `blocked_reason = duplicate_copy`,
     event `duplicate_copy` (a partial MOVE may leave a copy; mailtriage never
     deletes);
   - still in F, not in T, and T is scanned through its current `UIDNEXT - 1`
     in `target_epoch` → retry with the job backoff schedule up to
     `policy.max_attempts`, then `failed` with `blocked_reason = move_failed`;
   - absent from both → `sent`.
   - If T's epoch changed since `target_epoch`, UID comparisons are invalid:
     the intent waits until T is rescanned and then decides by identity rules.
6. A `sent` intent becomes `lost` when T is scanned through the `UIDNEXT - 1`
   observed after the move in `target_epoch`, every provisional message in that
   range has been fetched or has failed terminally, and no arrival matched.
   The placement's `location_state` becomes `absent`.

### Flags

1. Re-read the envelope; skip if it already has `\Flagged`.
2. In one transaction: write the flag intent and set `flag_attempted_at`. This
   consumes the message's only automatic flag attempt.
3. Call `add_flagged`.
   - Success and `session_epoch == E` → `applied`, `flagged_at`, event
     `flagged`.
   - Success but epoch race → `failed`, event `epoch_race`, folder paused for
     review (another message may now be flagged).
   - Error → `uncertain`. On the next pass, re-read flags: `\Flagged` present →
     `applied`; absent → `failed`, event `flag_failed`. Never retried.

A crash between steps 2 and 3 leaves the message unflagged; that is the cost of
never overriding a user's unflag.

## Arrivals and location

### Arrivals

| Arrival | Home UID still present | Result |
| --- | --- | --- |
| Our move (identity per rules) | n/a | `own_move`: intent applied |
| Category C's folder, identity established | no | `user_move`: client-correction transition |
| A source folder, identity established | no | `pinned`: user-move-into-source transition |
| Any folder, identity established | yes | `extra`: occurrence recorded, no change |
| Category folder, fingerprint matches no known message | n/a | `unknown_new`: after classification, home = this folder, `filed_by = user`, client-correction override to C. Never moved automatically |
| Rescan after epoch reset of a category folder | n/a | `rescan` (see Folders) |

"Home UID still present" is checked with one batched `envelopes` call per home
folder using `home_epoch`/`home_uid`, which survive reconciliation. Arrivals
whose identity needs a fingerprint stay `pending` until the provisional message
is fetched; a provisional message linked to a pending arrival gets no override
until then, so the fingerprint merge can always run.

The fingerprint merge (`Store::attach`) becomes filing-aware: in one
transaction it moves occurrences, arrivals and intents from the provisional to
the canonical message, merges envelope metadata, drops the provisional
placement, and resolves the pending arrival against the canonical placement.

### Placement re-evaluation

When the home occurrence disappears (reconciliation or a write-batch check):

- one surviving occurrence in a watched folder → home moves there; if it is a
  category folder whose category differs, apply the client-correction
  transition, otherwise event `relocated`;
- several surviving category-folder occurrences → `location_state = ambiguous`,
  event `location_ambiguous`, no automatic action until one remains or the user
  issues an explicit transition;
- none → `location_state = absent`, `absent_since = now`.

Home and `location_state` always follow confirmed observations; only redundant
override writes are suppressed.

### Done inference

A placement with `location_state = absent` is marked done locally
(`review_state = done`, `done_inferred = 1`, event `archived_done`) on a later
pass when all hold:

- the message is source-managed (it was discovered from a watched folder);
  messages ingested with `classify` have no placement and are never inferred;
- it still has no occurrence in any watched, retired or missing folder;
- every watched folder's checkpoint is complete in an epoch unchanged since
  `absent_since`;
- no pending arrival exists in the account, no provisional message discovered
  after `absent_since` is still awaiting a non-terminal fetch, and the message
  has no `in_flight`, `sent` or `uncertain` intent.

If the message later reappears, `location_state` returns to `known`; if
`done_inferred = 1`, it is reopened (`review_state = open`, `done_inferred = 0`,
event `reopened`). An explicit `done` sets `done_inferred = 0` and is never
reversed by observation.

## Folders

Each pass with mode not `off`:

1. LIST and LSUB once.
2. For each category whose folder is not `INBOX`, the native name is
   `personal_prefix + folder` (the first personal namespace reported by
   NAMESPACE, else empty). It is rejected if it equals a source folder's native
   name, and recorded in `folders` with the configured name.
3. Present in LIST:
   - with `\Noselect` or `\NonExistent` → state `noselect`;
   - with a special-use attribute (`\Sent`, `\Trash`, `\Drafts`, `\Junk`,
     `\Archive`, `\All`, `\Flagged`), or a name in the denylist (`Sent`,
     `Sent Items`, `Sent Messages`, `Trash`, `Deleted Items`,
     `Deleted Messages`, `Bin`, `Drafts`, `Junk`, `Junk E-mail`, `Spam`,
     `Archive`, `All Mail`, `Starred`, `Important`, and anything under
     `[Gmail]` or `[Google Mail]`, compared case-insensitively) → state
     `special_use`;
   - otherwise state `ok`; recorded as `adopted` unless already recorded.
4. Absent from LIST and never recorded → `create_folder` in `live` (state
   `ok`, origin `created`); a preview reports it in `folders_to_create`.
5. Absent from LIST but previously recorded → `missing`, event
   `folder_missing`; never recreated automatically. Recreating it in a mail
   client makes it `ok` again (adopted) on the next pass.
6. Every `ok` category folder not in LSUB → `subscribe_folder` in `live`;
   `subscribed` is tracked separately from creation, so a crash between create
   and subscribe is completed on the next pass.
7. A recorded folder no longer referenced by any category → `retired`. It stays
   watched while open intents or pending arrivals reference it; afterwards its
   occurrences are frozen (kept, not rescanned) and still count as present for
   done inference.

Filing into a category whose folder is not `ok` pauses; the pause and reason
are listed in `filing status`. Folders are never deleted or renamed.

**First watch.** A category folder's checkpoint starts at its current
`UIDNEXT - 1`, so pre-existing content is never ingested.

**Epoch reset of a category folder.** Its occurrences are invalidated as today,
and it is rescanned from UID 1. Below the `UIDNEXT` observed at the reset
(`rescan_below_uid`), envelopes only re-attach to known messages whose home is
this folder (`rescan` arrivals); unmatched content is not ingested. Above it,
discovery proceeds normally.

## Commands

All take `--account` and `--json`, keep the existing response envelope
(`schema_version`, `error.code`/`error.message`) and exit codes, and are safe to
retry.

| Command | Effect |
| --- | --- |
| `filing status` | Mode, `enabled_at`, MOVE/UIDPLUS support, folders with origin and state, paused categories, open/uncertain/failed intents, blocked, ambiguous, quarantined and eligible-but-unfiled counts, last pass summary. Read-only; no engine calls. |
| `filing enable --mode dry-run\|live` | Validates folders, then writes `filing.mode` under the exclusive config lock (as `categories apply` does). Idempotent. |
| `filing disable` | Writes `filing.mode = off`. Takes effect at the next pass. |
| `filing plan [--limit N]` | Read-only. Runs the planner on local state and lists `folders_to_create` and actions. |
| `filing backfill (--days N \| --all) [--apply]` | Without `--apply`: lists messages in source folders that would become eligible. With `--apply`: requires mode `live`, sets `eligible_once`; later passes file them in capped batches. |
| `filing pin --id ID` / `filing unpin --id ID` | Placement transitions as above. |
| `filing retry --id ID` | Clears a block. |
| `filing log [--id ID] [--limit N]` | Recent `filing_events`, newest first. |

Changes to existing commands:

- `list` / `read` items gain `placement: {folder, location_state, filed_by,
  pinned, flagged, blocked_reason, pending_action}`. Existing fields are
  unchanged.
- `correct` performs the placement transition atomically with the override;
  `placement.pending_action` shows the queued move. With filing `off` it changes
  only the override, as today.
- `done` / `reopen` set `done_inferred = 0`.
- `doctor` gains `filing: {mode, move_supported, uidplus, personal_prefix,
  alias_conflicts, folders, problems}` when mode is not `off`. It performs
  LIST, LSUB, CAPABILITY and NAMESPACE but no writes.
- `categories validate` / `apply` enforce the folder rules when filing is on.
- `init` writes the new config shape with `filing.mode = off`.

## Error handling

| Failure | Behaviour |
| --- | --- |
| MOVE or STORE error | Intents `uncertain`; resolved by observation as above. |
| Epoch race | Reverted with COPYUID, else folder paused and arrivals quarantined. |
| Folder creation or subscription error | Folder state `error`; category pauses; retried next pass. |
| Alias conflict | Discovery of the conflicting folder and all filing writes stop; reported by `doctor` and `filing status`. |
| Authentication, throttling, network | Pass ends partial (exit 4); `watch` continues with its existing interval. |
| Config changed during a pass | Existing `require_unchanged` check before applying writes. |

Error text never includes message bodies, subjects or credentials.

## Testing

- **Planner**: table-driven unit tests for every move and flag rule, explicit
  versus automatic precedence, the new-mail boundary, pin, `eligible_once`,
  blocks, `INBOX` targets, paused and retired folders, preview folders,
  incomplete input, review mode, stale generation, ordering and the cap.
- **Raw parser**: SELECT/MOVE/STORE responses with and without COPYUID,
  NO/BAD at each tag, missing UIDVALIDITY, untagged noise.
- **Service with `FakeEngine`**: new mail filed; `dry_run` and `off` make zero
  write calls; user move becomes a correction only after fingerprint identity;
  move back pins; archive becomes done only after the done conditions;
  reappearance reopens an inferred done but not an explicit one; a second Gmail
  label is ignored; copy-then-delete in a client relocates; several surviving
  occurrences become ambiguous; crash injected before, during and after each
  write converges; epoch race with and without COPYUID; duplicate copy blocks;
  terminal move failure blocks until `retry`; a newer correction survives an
  older intent's completion; stale-generation classifications are not acted
  on; flag attempted once and never retried; retired folder with an in-flight
  move resolves; missing folder freezes; epoch-reset rescan re-attaches without
  ingesting; backfill applies in capped batches; adopted folder content is not
  ingested; filing-aware merge preserves metadata and moves references.
- **Config**: legacy `himalaya` configs load and save as `engine`; legacy
  category names load with filing off; binding identity hash unchanged; folder
  validation with filing on; generation hash unchanged by `filing` and
  `folder`.
- **Himalaya contract**: the fake-binary tests assert exact argument arrays and
  raw command text for every operation, UID mode, quoting, and that
  `expunge`, `delete`, `rename`, `-FLAGS`, `--action remove`, `--action set`,
  `--seq` and `--seen` never occur.
- **Dovecot end to end**: a Linux CI job runs the real Himalaya v2.1.0 release
  binary (pinned by checksum) against a Dovecot container with seeded mail, in
  two namespace layouts (no prefix with `/`, and `INBOX.` prefix with `.`), and
  exercises enable → live filing → client move → pin → archive.
- **Live provider check** (below).

## Live provider check

The first implementation task, before filing logic is built on the engine. It
needs one throwaway test account per provider, configured in Himalaya by the
user; no credentials enter this repository. For Gmail, Microsoft 365, iCloud
and Fastmail or a Dovecot host, record in `docs/verification.md`:

- `imap raw` output for pipelined `CAPABILITY`/`NAMESPACE` and
  `SELECT` + `UID MOVE` (UIDVALIDITY and COPYUID presence) and
  `SELECT` + `UID STORE`;
- `imap list --all --json` and `imap list --json` shapes, delimiter,
  attributes actually returned, name encoding for a non-ASCII folder;
- the alias table location in the Himalaya TOML;
- create and subscribe for an ASCII and a non-ASCII name;
- INTERNALDATE, size and Message-ID before and after a move;
- `\Flagged` in the provider's web and mobile clients;
- `\Seen` unchanged by fetch, move and store;
- Gmail: label semantics of MOVE from INBOX, archive, and a message carrying
  two category labels;
- login rate: one `watch` with seven watched folders at a 60-second interval
  for 30 minutes without throttling; if throttled, record whether pipelined
  STATUS via `imap raw` resolves it.

Outcome per provider: go, go with noted differences, or no-go. A no-go blocks
enabling `live` for that provider until resolved (for example by a native IMAP
engine) and is documented in the README.

## Out of scope

System tray app and worker heartbeat (next project); native IMAP engine, IDLE
and connection reuse; provider-specific APIs; COPY + EXPUNGE fallback; renaming
or deleting server folders; custom IMAP keywords; `\Seen` changes;
conversation-level filing; Windows builds.
