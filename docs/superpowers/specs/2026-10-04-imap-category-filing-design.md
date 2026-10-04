# IMAP category filing

Date: 2026-10-04
Status: Design approved in conversation; written spec under review.
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
read-only fetch) remains in force.

## Decisions

| Topic | Decision |
| --- | --- |
| Inbox model | Every classified message moves to its category folder by default. A category may target `INBOX`, which means "leave it in the source folder". |
| Importance | `\Flagged` only, set when the effective decision is action required or high urgency. No custom keywords. |
| Existing mail | New mail only. Backfill over N days or the whole inbox is an explicit command. Every filing action has a dry run. |
| Manual moves in a mail client | Moving into another category's folder is a category correction. Moving back to a source folder pins the message there. Archiving or deleting (leaving all watched folders) marks it done locally. |
| Folder location | Top level, named after the category. An existing folder with that name is adopted. |
| Providers | Gmail / Google Workspace, Microsoft 365 / Outlook.com, iCloud, Fastmail / Dovecot. All first-class. |
| Engine | Himalaya v2.1.0 raw IMAP commands now, behind a `MailEngine` trait so a native IMAP engine can replace it later. |

## Safety invariants

Each invariant has at least one dedicated test.

1. The engine boundary exposes no destructive operation: no delete, expunge,
   flag removal, `\Seen` change, folder delete or folder rename.
2. In filing modes `off` and `dry_run` the engine receives zero write calls
   (create, subscribe, move, store).
3. Every move and flag is recorded as an intent in SQLite before the engine
   call. A crash at any point converges on the next pass without a duplicate
   move or a fabricated correction.
4. Automatic filing moves mail only out of configured source folders. Moves
   between category folders happen only for an explicit correction, pin or
   unpin issued through the CLI.
5. Each write batch checks the folder's UIDVALIDITY before the call. A changed
   epoch aborts the batch before writing; an epoch change detected after the
   call makes the batch uncertain and it is resolved by observation, never by
   assumption.
6. Filing refuses to enable on a server without the MOVE extension. There is no
   COPY + `\Deleted` + EXPUNGE fallback.
7. A message is moved automatically at most once and flagged automatically at
   most once. A user's unflag is therefore never overridden.

## Architecture

### MailEngine trait

`src/engine/mod.rs` defines the boundary. The service depends only on this
trait; `src/engine/himalaya.rs` (the current `src/himalaya.rs`, moved and
extended) implements it; `src/engine/fake.rs` is an in-memory implementation
for tests. A later native IMAP engine is a third implementation.

```rust
pub struct EngineCapabilities {
    pub move_supported: bool,
    pub delimiter: Option<char>,          // hierarchy delimiter from LIST
    pub personal_prefix: String,          // e.g. "" or "INBOX."
}
pub struct FolderInfo {
    pub name: String,                     // server-native name
    pub special_use: Vec<String>,         // e.g. ["\\Sent"], from LIST attributes
    pub subscribed: bool,
}
pub struct SourceEnvelope {               // extended, all new fields optional
    pub uid: u64,
    pub subject: String,
    pub from: Vec<Address>,
    pub sent_at: Option<String>,
    pub message_id: Option<String>,       // RFC 5322 Message-ID from ENVELOPE
    pub internal_date: Option<String>,    // server INTERNALDATE, RFC 3339
    pub size: Option<u64>,                // RFC822.SIZE
    pub flags: Vec<String>,               // e.g. ["\\Seen", "\\Flagged"]
}

pub trait MailEngine {
    fn version(&self) -> Result<String>;
    fn capabilities(&self) -> Result<EngineCapabilities>;
    /// Stable JSON describing server + login identity, never secrets.
    fn binding_identity(&self) -> Result<serde_json::Value>;
    fn list_folders(&self) -> Result<Vec<FolderInfo>>;
    /// CREATE then SUBSCRIBE. Must not be called for a folder LIST reported.
    fn create_folder(&self, name: &str) -> Result<()>;
    fn snapshot(&self, folder: &str) -> Result<MailboxSnapshot>;
    /// Envelopes for UIDs in (after, through].
    fn discover(&self, folder: &str, after: u64, through: u64) -> Result<Vec<SourceEnvelope>>;
    /// Envelopes for exactly these UIDs; absent UIDs are simply omitted.
    fn envelopes(&self, folder: &str, uids: &[u64]) -> Result<Vec<SourceEnvelope>>;
    /// Raw RFC 5322 bytes; never sets \Seen.
    fn fetch_raw(&self, folder: &str, uid: u64) -> Result<Vec<u8>>;
    /// UID MOVE of the whole set to `target`.
    fn move_messages(&self, folder: &str, uids: &[u64], target: &str) -> Result<()>;
    /// UID STORE +FLAGS (\Flagged). There is no removal operation.
    fn add_flagged(&self, folder: &str, uids: &[u64]) -> Result<()>;
}
```

Engine construction is a single factory, `engine::open(&EngineConfig) ->
Result<Box<dyn MailEngine>>`. The service never names `Himalaya` directly.

### Himalaya command mapping

All commands keep the current invariants: argument arrays without a shell,
stdin closed, `--backend imap`, timeout and output limits, stderr discarded,
UID mode (never `--seq`), never `--seen`.

| Trait method | Himalaya v2.1.0 command |
| --- | --- |
| `version` | `--version` (unchanged) |
| `capabilities` | `imap raw` CAPABILITY (MOVE) and LIST delimiter / NAMESPACE; exact parsing fixed by the provider check |
| `list_folders` | `--json imap list --all` |
| `create_folder` | `imap create NAME`, then `imap subscribe NAME` |
| `snapshot` | `--json imap status FOLDER` (unchanged) |
| `discover` / `envelopes` | `--json imap fetch --mailbox F --envelope --flags --internal-date --size UIDSET` |
| `fetch_raw` | `message read --mailbox F --raw UID` (unchanged) |
| `move_messages` | `imap move --mailbox F UIDSET TARGET` |
| `add_flagged` | `imap store --mailbox F --action add -f '\Flagged' UIDSET` |

UID sets are comma-joined lists capped at 100 UIDs per call, matching the
existing fetch window. `imap move` prints no COPYUID, so the new UID of a moved
message is learned by discovery in the target folder (see Tracking).

### Module layout

| Path | Responsibility |
| --- | --- |
| `src/engine/mod.rs` | Trait, engine types, factory |
| `src/engine/himalaya.rs` | Himalaya implementation (moved from `src/himalaya.rs`) |
| `src/engine/fake.rs` | In-memory mailbox: folders, UIDs, epochs, flags, call log, fault injection, simulated user moves |
| `src/filing/planner.rs` | Pure planner: state in, actions out |
| `src/filing/mod.rs` | Pass orchestration: folder resolution, arrival resolution, intent application, done inference |
| `src/store.rs` | Schema v3 and filing persistence |
| `src/service.rs` | Calls `filing` within `sync`; new filing commands |
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
  max_actions_per_pass: 200}`. `mode` is one of `off`, `dry_run`, `live`.
  `max_actions_per_pass` is 1..=1000.
- `categories[].folder` is optional; absent means the category `name`. The
  literal `INBOX` means "stay in the source folder".
- `filing` and `folder` are excluded from the classification generation hash,
  so changing them never reclassifies mail. `filing` is not part of `policy`.
- The account binding identity for a Himalaya engine must hash to exactly the
  value the current release produces from the legacy `himalaya` block, so
  existing state directories keep working. A test asserts this.

Folder validation (offline, in `config::validate`):

- Folder names are trimmed, nonempty, at most 200 bytes, single path segments:
  no `/`, `.`, `*`, `%`, control characters or leading/trailing whitespace.
  Non-ASCII is allowed only if the provider check confirms Himalaya encodes
  modified UTF-7; otherwise validation restricts names to printable ASCII.
- Every category whose folder is not `INBOX` has a unique folder,
  compared case-insensitively. Any case variant of `inbox` other than the
  literal `INBOX` is rejected.
- A folder may not equal a source folder unless it is `INBOX`.

Online validation (doctor, enable, each live pass): a category folder that
LIST reports with a special-use attribute (`\Sent`, `\Trash`, `\Drafts`,
`\Junk`, `\Archive`, `\All`, `\Flagged`) pauses that category and is reported.

## State (SQLite schema v3)

Migration from v2 runs in one transaction, like the existing migrations.

- `messages` gains `match_key TEXT` (SHA-256 of normalized Message-ID, size and
  internal date when all three are present), `internal_date TEXT`, and an index
  on `(account, match_key)`. The extended envelope is stored in the existing
  `envelope` JSON.
- `occurrences` is unchanged. A message may have several.
- `filing_state(account PK, mode TEXT, enabled_at TEXT, last_pass TEXT)`:
  `last_pass` is the JSON summary of the last pass.
- `placements(account, message_id PK, source_folder TEXT, home_folder TEXT,
  filed_at TEXT, filed_by TEXT, pinned INTEGER, eligible_once INTEGER,
  requested_folder TEXT, flagged_at TEXT, absent_since TEXT)`.
  `source_folder` is the source folder the message was first discovered in
  (the first configured source folder if it was first seen elsewhere).
  `home_folder` is where the message is expected to live now. `filed_by` is
  `mailtriage` or `user`. `eligible_once` permits one automatic filing out of a
  source folder (set by backfill and unpin). `requested_folder` is an explicit
  target set by a CLI correction or pin.
- `folders(account, folder PK, category_id TEXT, origin TEXT, state TEXT,
  watch_from_uid INTEGER, epoch INTEGER, checked_at TEXT, error TEXT)`.
  `origin` is `created` or `adopted`; `state` is `ok`, `missing`,
  `special_use`, `retired` or `error`.
- `filing_intents(id PK, account, message_id, kind TEXT, folder TEXT,
  epoch INTEGER, uid INTEGER, target TEXT, target_uid_next INTEGER, batch TEXT,
  state TEXT, attempts INTEGER, next_after TEXT, created_at, updated_at,
  error TEXT)`. `kind` is `move` or `flag`.
- `filing_events(id PK, account, message_id, at, kind TEXT, detail TEXT)`:
  append-only audit log. Kinds: `moved`, `flagged`, `client_correction`,
  `pinned`, `unpinned`, `archived_done`, `move_failed`, `flag_failed`,
  `folder_created`, `folder_adopted`, `folder_missing`, `folder_special_use`.

## Filing mode and "new mail"

`filing.mode` in the config is authoritative. Each pass compares it with
`filing_state.mode`:

- `off` → `dry_run` or `live`: set `enabled_at` to now.
- `dry_run` ↔ `live`: keep `enabled_at`, so mail that arrived during a dry run
  is filed when going live.
- anything → `off`: keep the old value; the next enable sets a new one.

A message is **new** when its `internal_date` ≥ `enabled_at`. A message with no
internal date is never new; only backfill, a correction, or an unpin makes it
eligible. This prevents a first sync from treating the whole backlog as new.

## Sync pass order

`sync` (and therefore each `watch` pass) runs, under the existing account lock:

1. **Engine check.** Version, then capabilities once per pass when mode is not
   `off`. Missing MOVE support with mode `live` stops filing for the pass and is
   reported; discovery and classification continue.
2. **Folder resolution** (mode not `off`). LIST once. For each category folder
   other than `INBOX`: present → record `adopted` (unless already recorded);
   absent and never recorded → `create_folder` in `live`, reported as
   "would create" in `dry_run`; absent but previously recorded → `missing`
   (never recreated automatically). Special-use → `special_use`. A folder no
   longer referenced by any category becomes `retired`.
3. **Discovery.** Source folders as today. Category folders with state `ok`
   are watched too; when a folder is first watched its checkpoint starts at the
   current `UIDNEXT - 1`, so pre-existing content is never ingested. Each new
   envelope is matched by `match_key` to a known message of the account:
   match → add the occurrence to that message and record it as an arrival;
   no match → create a provisional message as today.
4. **Reconciliation.** Existing bounded windows, for every watched folder.
5. **Arrival and intent resolution** (mode not `off`; see Tracking).
6. **Fetch and classify** queued messages (unchanged), then resolve arrivals
   that fingerprint deduplication revealed.
7. **Plan** (mode not `off`). The pure planner computes actions.
8. **Apply** (mode `live` only). Flags first, in the message's current folder
   by its known UID; then moves, batched per (source folder, target folder).
   Total actions are capped at `max_actions_per_pass`; the remainder waits for
   the next pass.
9. **Done inference** (mode not `off`; see Tracking).

With mode `off`, steps 2, 5, 7, 8 and 9 are skipped and category folders are
not watched; their checkpoints resume where they stopped when filing is
enabled again.
10. **Summary.** `sync` output gains `filing: {mode, planned, moved, flagged,
    client_corrections, pinned, archived_done, errors}`. Any filing error makes
    the pass partial (exit 4).

## Planner

`plan(input) -> Vec<Action>` is a pure function in `src/filing/planner.rs`.
`Action` is `Move { message_id, from: Locator, to: String }` or
`Flag { message_id, at: Locator }`. `filing plan` and `dry_run` passes call the
same function as `live`; only application differs.

Effective decision = the model decision with per-field overrides applied, as
`Service::item` does today. The `review_mode` reason does not block filing:
filing has its own dry run. `review_state` (Done) does not affect filing.

Every move requires: mode is not `off`; the message's `home_folder` is a source
folder or a category folder in state `ok`; it has an occurrence there with a
known UID in the current epoch; it has no open move intent; and the target
differs from `home_folder` and, if a category folder, is in state `ok`.

**Explicit moves** take precedence. When `requested_folder` is set, the
message gets a `Move` to it, from a source or category folder. A CLI category
correction sets `requested_folder` to the corrected category's folder, or to
`source_folder` when that category targets `INBOX`. `filing pin` sets it to
`source_folder`. It is cleared when the move is applied.

**Automatic moves.** Otherwise a message gets a `Move` to its effective
category's folder when all of the following hold:

- its `home_folder` is a source folder and it is not pinned;
- its classification state is `ready` or `uncertain` and the effective category
  is set (model confident, or corrected by the user);
- the classification is not based on incomplete input (`input_incomplete`),
  unless the category was corrected by the user;
- the category's folder is not `INBOX`;
- either (a) it is new and has never been filed, or (b) `eligible_once` is set.

**Flag rules.** A message gets a `Flag` when `filing.flag` is true, mode is not
`off`, it has never been flagged by mailtriage (`flagged_at` empty), its
current envelope lacks `\Flagged`, and its effective decision has
`action_required == true` or `urgency == high`. Flag eligibility does not depend
on "new": a message is flagged when it qualifies, whether or not it moves.

**Ordering and caps.** Actions are ordered by message internal date, oldest
first, flags before moves for the same message, and truncated at
`max_actions_per_pass`.

## Tracking and corrections

### Move intents

```text
in_flight ──engine ok──> sent ──arrival in target──> applied
    │                      └──target fully scanned, no arrival──> lost
    └──engine error / crash──> uncertain
uncertain ──source still has UID──> retry (attempts+1, backoff) ──max──> failed
uncertain ──source lacks UID──> sent
```

- An intent is written `in_flight` with folder, epoch, UID and target before
  the engine call. Success → `sent`, the source occurrence is removed, and the
  target's `UIDNEXT` after the move is stored on the intent.
- **Arrival in target.** A new occurrence for the message in the intent's
  target folder → `applied`: `home_folder = target`, `filed_at = now`,
  `filed_by = mailtriage`, `eligible_once` and `requested_folder` cleared,
  event `moved`. A message without a usable `match_key` arrives as a
  provisional message; when fingerprint deduplication merges it into the
  intent's message, that merge counts as the arrival.
- **Uncertain.** On the next pass, `envelopes(folder, [uid])` in the source
  decides. Retries use the job backoff schedule and `policy.max_attempts`;
  `failed` raises event `move_failed` and the message stays where it is.
- **Lost.** A `sent` intent whose target checkpoint has reached the stored
  `UIDNEXT - 1` without an arrival, and whose provisional messages in that
  range have all been fetched, becomes `lost`; the message is then handled by
  done inference.

Flag intents follow the same pattern without `sent`: success → `applied`
(`flagged_at = now`, event `flagged`); uncertain → re-read the envelope's flags;
`\Flagged` present → applied, otherwise retry.

### Arrivals without an intent

A known message appearing in a watched folder other than its `home_folder`,
with no matching intent, is resolved with one batched `envelopes` call per
home folder, checking whether the home UID still exists:

| Arrival folder | Home UID still present | Result |
| --- | --- | --- |
| Category C's folder | no | User move. Override `category_id = C`; `home_folder = C's folder`; `filed_by = user`; event `client_correction`. No effect if the effective category already is C. |
| A source folder | no | User moved it back. `pinned = 1`; `home_folder` = that source; event `pinned`. |
| Any | yes | Extra occurrence (e.g. a second Gmail label). Recorded, no state change. |

An unknown message arriving in a category folder (delivered there by a server
rule, or moved from an unwatched folder) is ingested and classified as usual,
gets a `client_correction` override to that folder's category, and has
`home_folder` = that folder. It is never moved automatically.

Mail-client corrections use the same compare-and-swap override update as
`correct`; the most recent correction wins.

### Done inference

When a message has no occurrence in any watched, missing or retired folder,
set `absent_since`. Mark it done locally (`review_state = done`, event
`archived_done`) on a later pass when all of these hold:

- it still has no occurrence;
- every watched folder's discovery checkpoint is complete and its epoch
  unchanged since `absent_since`;
- it has no `in_flight`, `sent` or `uncertain` intent.

Occurrences removed by an epoch reset do not count as absence: the reset folder
is incomplete until rescanned, which blocks the inference. A message that
reappears clears `absent_since`.

### Folders

- Created folders are subscribed so mail clients show them.
- The personal namespace prefix from capabilities is prepended to category
  folder names on servers that require it (for example `INBOX.Newsletters`).
- Folders are never deleted or renamed. Changing a category's `folder` affects
  future filing only; the old folder becomes `retired`, its occurrences are
  frozen, and its messages are not marked done.
- A watched folder that disappears from LIST becomes `missing`, event
  `folder_missing`. Filing into its category pauses; its messages are frozen
  and not marked done. Recovery: point the category at another folder, or
  recreate the folder in a mail client (detected on the next pass and adopted).

## Commands

All take `--account` and `--json`, keep the existing response envelope
(`schema_version`, `error.code`/`error.message`) and exit codes, and are safe to
retry.

| Command | Effect |
| --- | --- |
| `filing status` | Mode, `enabled_at`, MOVE support, folders with origin and state, paused categories, open/uncertain/failed intents, eligible-but-unfiled count, last pass summary. Read-only; no engine calls. |
| `filing enable --mode dry-run\|live` | Writes `filing.mode` under the exclusive config lock (as `categories apply` does). Idempotent. `live` runs no engine check itself; the next pass reports MOVE problems. |
| `filing disable` | Writes `filing.mode = off`. Takes effect at the next pass. |
| `filing plan [--limit N]` | Read-only. Runs the planner on current local state and lists actions, including folders that would be created. |
| `filing backfill (--days N \| --all) [--apply]` | Without `--apply`: lists messages in source folders that would become eligible. With `--apply`: requires mode `live`, sets `eligible_once` on them; later passes file them in capped batches. |
| `filing pin --id ID` / `filing unpin --id ID` | Pin: `pinned = 1`; if `home_folder` is not a source folder, set `requested_folder = source_folder`. Unpin: clear the pin and set `eligible_once`. Both record an event. |
| `filing log [--id ID] [--limit N]` | Recent `filing_events`, newest first. |

Changes to existing commands:

- `list` / `read` items gain `placement: {folder, filed_by, pinned, flagged,
  pending_action}`. Existing fields are unchanged.
- `correct --category` on an account with filing not `off` sets
  `requested_folder` as described in Planner; the response's
  `placement.pending_action` shows the queued move. With filing `off` it
  changes only the override, as today.
- `doctor` gains `filing: {mode, move_supported, personal_prefix, folders:
  [...], problems: [...]}` when mode is not `off`. It performs LIST and
  CAPABILITY but no writes.
- `categories validate` / `apply` enforce the folder rules above.
- `init` writes the new config shape with `filing.mode = off`.

## Error handling

| Failure | Behaviour |
| --- | --- |
| MOVE batch error | All intents in the batch become `uncertain`; resolved next pass by observation. |
| Folder creation error | Folder state `error`; its category pauses; retried next pass. |
| Authentication, throttling, network | Pass ends partial (exit 4); `watch` continues with its existing interval. |
| Epoch change before a write batch | Batch skipped; folder rescanned as today. |
| Epoch change after a write batch | Batch intents become `uncertain`. |
| Config changed during a pass | Existing `require_unchanged` check before applying writes. |

Error text never includes message bodies, subjects or credentials.

## Testing

- **Planner**: table-driven unit tests covering every move and flag rule, the
  new-mail boundary, pin, `eligible_once`, `INBOX` targets, paused folders,
  incomplete input, review mode, ordering and the cap.
- **Service with `FakeEngine`**: new mail filed; `dry_run` and `off` make zero
  write calls; user move to another category becomes a correction; move back
  to INBOX pins; archive becomes done only after complete scans; a second Gmail
  label is ignored; crash injected before, during and after each engine write
  converges without double moves; epoch reset in a category folder blocks done
  inference; missing and retired folders freeze messages; backfill applies in
  capped batches; CLI correction moves a filed message; flag once and a user
  unflag is respected; adopted folder content is not ingested.
- **Config**: legacy `himalaya` configs load and save as `engine`; binding
  identity hash is unchanged; folder validation; generation hash unchanged by
  `filing` and `folder`.
- **Himalaya contract**: the fake-binary tests assert exact argument arrays
  for every new command, UID mode, and that `expunge`, `delete`, `rename`,
  `--action remove`, `--action set`, `--seq` and `--seen` never occur.
- **Dovecot end to end**: a Linux CI job runs the real Himalaya v2.1.0 release
  binary (pinned by checksum) against a Dovecot container with seeded mail and
  exercises enable → live filing → client move → pin → archive.
- **Live provider check** (below).

## Live provider check

The first implementation task, before filing logic is built on the engine. It
needs one throwaway test account per provider, configured in Himalaya by the
user; no credentials enter this repository. For Gmail, Microsoft 365, iCloud
and Fastmail or a Dovecot host, record in `docs/verification.md`:

- `imap raw` CAPABILITY output includes MOVE; the parse used by `capabilities`;
- LIST output shape in Himalaya JSON, delimiter, special-use attributes and
  any personal namespace prefix;
- `imap create` and `imap subscribe` for an ASCII and a non-ASCII name;
- `imap move` of a UID set, the message's INTERNALDATE, size and Message-ID
  before and after the move;
- `imap store -f '\Flagged'` and its appearance in the provider's web client;
- `\Seen` unchanged by fetch, move and store;
- Gmail: label semantics of MOVE from INBOX, archive, and a message carrying
  two category labels;
- login rate: one `watch` with seven watched folders at a 60-second interval
  for 30 minutes without throttling.

Outcome per provider: go, go with noted differences, or no-go. A no-go blocks
enabling `live` for that provider until resolved (for example by a native IMAP
engine) and is documented in the README.

## Out of scope

System tray app and worker heartbeat (next project); native IMAP engine, IDLE
and connection reuse; provider-specific APIs; COPY + EXPUNGE fallback; renaming
or deleting server folders; custom IMAP keywords; `\Seen` changes;
conversation-level filing; Windows builds.
