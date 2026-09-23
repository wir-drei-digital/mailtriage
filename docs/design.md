# Standalone mail classification core and Himalaya adapter

Date: 2026-09-21
Status: Design. Daniel confirmed standalone core + Himalaya focus and accepted
action required as the third signal. Technical choices below are the proposed
implementation contract. No binary has been built or live mailbox accessed.

## Purpose and first-release boundary

Give any human or agent a small, queryable view of mail worth attending to.
Himalaya accesses the mailbox; Jev classifies each message; this tool persists
the results and applies a deterministic visibility policy. Hermes uses ordinary
CLI commands. Neither Valea nor an agent SDK is a runtime dependency.

Working name: `mailtriage`. Recommend a separate Rust project with a library
and binary, shipping on Linux amd64/arm64 and macOS arm64. Keep source adapter,
provider adapter, domain/policy and storage as internal modules initially;
separate crates are unnecessary until another consumer needs them.

First release: JSON/RFC822 ingestion, Himalaya IMAP ingestion, three-signal
classification, persistent jobs/results, metadata queries, selective body
reads, editable taxonomy, per-message corrections and local Done/reopen.
One account namespace per configured account; one writer per state directory.
No sending, drafting, moving, deleting, automatic replies, attachment analysis,
desktop UI, cross-device synchronization or server hosting. Later backends can
implement the same source contract; Himalaya supporting a backend does not
automatically mean this adapter has tested it.

## Public mental model

```text
Himalaya → discover + fetch without marking read → normalize
                                                    ↓
config + recipient context → Jev: three questions → persist
                                                    ↓
                                     policy → metadata query
                                                    ↓
                                    Hermes → read selected mail
```

Classifying reads mail once into a specialist model. It saves the reasoning
agent from opening every body just to decide whether to pay attention.

| Field | Domain | Decision |
| --- | --- | --- |
| `urgency` | `low`, `medium`, `high` | How soon does this recipient need to attend to it? |
| `category_id` | One configured ID | What kind of email is this? |
| `action_required` | Boolean when decided | Does the recipient need to respond or take another concrete action? |

Unknown values are `null` with an explicit decision state, never fabricated
defaults. Action is separate from urgency: a reply without a deadline can be
low urgency and actionable. A critical FYI can be high urgency without an
action. A marketing call to action is not itself an obligation.

Default category definitions come from the broader proposal: Correspondence,
Transactions, Updates, Newsletters, Promotions and Other. Store stable IDs,
labels, descriptions and optional examples; exactly one catch-all role remains
available. The user owns the fixed set. Rename preserves identity; definition
changes advance a semantic revision. Validate before atomically applying edits.

## CLI contract

Examples below specify the proposed product, not installed commands:

```sh
mailtriage doctor --account work --json
mailtriage sync --account work --json
mailtriage watch --account work
mailtriage classify --account work --input message.eml --format rfc822 --json
mailtriage list --account work --view attention --json
mailtriage list --account work --category newsletters --urgency low --json
mailtriage read --account work --id msg_123 --json
mailtriage correct --account work --id msg_123 --action-required true --json
mailtriage correct --account work --id msg_123 --clear urgency --json
mailtriage done --account work --id msg_123 --json
mailtriage reopen --account work --id msg_123 --json
mailtriage categories export --account work --json
mailtriage categories validate --file categories.json --json
mailtriage categories apply --account work --file categories.json --json
mailtriage reclassify --account work --since 2026-09-01 --dry-run --json
```

All commands support an explicit config path. Reads work offline against local
state; `list` never secretly syncs or spends model tokens. `sync` performs one
bounded discovery/fetch/classification pass; `watch` repeats it with graceful
shutdown and backoff. Avoid two competing workers: a process lock covers the
account worker; concurrent local corrections use database transactions.

JSON mode emits one versioned object to stdout; diagnostics go to stderr and
exclude message bodies and secrets. Proposed exit codes: 0 success, 2 invalid
input/config, 3 dependency/provider failure, 4 partial sync, 5 revision conflict.
List/read exit 0 when the query succeeded even if some returned items need
review; coverage and errors are represented in the result.

Listings accept bounded `--limit` and opaque `--cursor`; filter before paging.
Return a snapshot revision with deterministic ordering and reject an expired
cursor instead of silently changing snapshots. A sync summary counts discovered,
fetched, classified, cached, pending and failed messages, plus coverage. A
message-level failure does not discard the successful part of a batch.

### Example normalized listing item

Illustrative values only; model probabilities live in the detail record:

```json
{
  "id": "msg_123",
  "account": "work",
  "from": [{"email": "alex@example.com", "name": "Alex"}],
  "subject": "Can you review the proposal?",
  "received_at": "2026-09-21T08:30:00Z",
  "classification": {
    "state": "ready",
    "urgency": "low",
    "category_id": "correspondence",
    "action_required": true,
    "taxonomy_revision": 3
  },
  "review_state": "open",
  "attention": true,
  "attention_reasons": ["action_required"]
}
```

Use `received_at` only when provided by a reliable transport field; otherwise
return `null`, plus `first_observed_at` and the separately named sender-claimed
`sent_at`. Do not relabel the envelope's Date header as reception time.

The response envelope includes `schema_version`, `items`, `next_cursor`,
`snapshot_revision`, and `coverage` (scope, last successful scan, completeness,
pending fetches and failures). Body fetching failures must remain discoverable
as review items with their envelope, even before a classification exists.

`read` returns locally normalized text and source provenance. It does not mark
the server message read. If content is missing, it reports unavailable; a
separate explicit refetch path may refresh it. Mail text remains untrusted data
even when returned inside a structured response.

## Attention policy and overrides

For an open message, Attention includes medium/high urgency, action required,
or any unresolved classification/fetch failure. Category is a query dimension,
never permission to hide an urgent or actionable message. Only a ready,
confident low-urgency result with confidently negative action can be put aside
automatically. A valid Other classification is distinct from model uncertainty.

`done` changes local review state, not the model's belief or server flags. It
survives reclassification; `reopen` reverses it. Each new incoming message gets
its own open state, including a reply in an existing conversation. First release
lists messages rather than implementing conversation-level task inference.

Corrections are per-field overlays on immutable model decisions; a retry cannot
overwrite them. Clearing an override reveals the latest model decision. No
automatic sender rule or model training is implied by one correction.

Category deletion requires remapping affected results/overrides or explicitly
moving them to review. A result arriving for an outdated taxonomy is retained
for audit but never silently published as current. Missing required decisions
make the record reviewable even when another signal is usable.

## Jev provider boundary

One bounded text state, three independent questions per call:

- Category: Choice over all active category IDs and descriptions.
- Urgency: Choice over low/medium/high with recipient-specific timing criteria.
- Action required: Noul asking whether the recipient needs to take a concrete
  action; return and retain the probability of yes.

TypeSafe documents [Noul](https://docs.typesafe.ai/primitives/noul) as a yes/no
probability, with no separate confidence field. Map a sufficiently strong no
to false, a sufficiently strong yes to true, and the middle band to unresolved.
Thresholds are policy settings validated against labeled mail; no arbitrary
production values are asserted by this design. Choice probabilities and
confidence remain separate from Noul's probability.

Input includes sender/recipients, subject, bounded plain text, account identity,
recipient timezone, evaluation time and a short trusted recipient brief. Strip
HTML to text without fetching resources; preserve the source fingerprint.
Record missing text, truncation and attachment-only/encrypted cases. A truncated
or unsupported message cannot be silently dismissed based on incomplete input.
No full ICM or unrelated mailbox history is sent by default.

Validate response types, probability ranges and returned choices. Persist
requested/returned model IDs, taxonomy/rubric/context revisions, input hash,
evaluation time, usage and provider request ID when available. Keys never enter
records. A latest alias may change; support it for exploration, use a tested
version for reproducible production evaluation.

OpenRouter is the intended gateway. The exact Decisions endpoint and gateway
schema still need official-contract verification and a synthetic request; the
TypeSafe primitive schema alone is insufficient proof. Do not implement this
as ordinary chat completions or fall back to a different model silently.

## Himalaya v2.1.0 adapter

The GitHub release API reported [v2.1.0](https://github.com/pimalaya/himalaya/releases/tag/v2.1.0)
as the latest release during this pass. Pin this compatibility target and verify
the actual binary version and capabilities in `doctor`.

Source-verified command shapes:

```sh
himalaya --config /path/config.toml --account work --backend imap --json envelope list --mailbox INBOX --page 1 --page-size 100
himalaya --config /path/config.toml --account work --backend imap message read --mailbox INBOX --raw 42
```

These are rendered examples; implementation passes argument arrays directly
with stdin closed, timeout/output-size limits and no shell interpolation.
Require a provisioned config and explicit account/backend/mailbox. Never invoke
the interactive setup wizard from `watch`.

Verified against pinned source:

- [`message/read.rs`](https://github.com/pimalaya/himalaya/blob/v2.1.0/src/shared/message/read.rs):
  `--seen` is opt-in. Without it the read requests preservation of Seen state.
  `--raw` writes original bytes; combining raw with JSON uses lossy UTF-8 string
  conversion. **Use binary raw stdout without JSON for fingerprinting.**
- [`envelope/list.rs`](https://github.com/pimalaya/himalaya/blob/v2.1.0/src/shared/envelope/list.rs):
  explicit pagination; JSON is an object containing `envelopes`, not a bare
  array. Default sort is Date descending.
- [`email/envelope.rs`](https://github.com/pimalaya/himalaya/blob/v2.1.0/src/email/envelope.rs):
  IDs are backend-specific; IMAP uses UIDs. Field names serialize in kebab-case.
  Date is author-claimed, not a reliable arrival checkpoint.
- [`cli.rs`](https://github.com/pimalaya/himalaya/blob/v2.1.0/src/cli.rs):
  explicit config/account/backend selection and no wizard when stdin is not a
  terminal. [`imap/cli.rs`](https://github.com/pimalaya/himalaya/blob/v2.1.0/src/imap/cli.rs)
  exposes protocol-specific status/search/fetch commands.

These are source observations, not a completed live IMAP test. Before release,
prove Seen flags remain unchanged against a test server and validate UIDVALIDITY
extraction and the backend fetch path in the actual binary.

### Discovery, identity and catch-up

The adapter interface has four operations: inspect capabilities/account,
discover occurrences, fetch original bytes without mutation, and reconcile
occurrence presence. Core receives normalized records, never Himalaya JSON.

For IMAP, bind locators to account identity + mailbox + UIDVALIDITY + UID.
An account name alone is insufficient if its server/login later changes.
Read epoch and UID information through tested protocol-specific commands;
when the epoch changes invalidate locators and rescan. Never reuse an old UID
mapping. Failure to obtain the epoch is an explicit unsupported/failed state.

Allocate an opaque local ID as soon as an envelope is durably discovered, so
fetch failures can appear in Attention. After raw fetch, deduplicate within the
account by full SHA-256 fingerprint. Preserve aliases for provisional IDs if
duplicate occurrences merge. Sender Message-ID is a threading hint only.

Use UID discovery for incremental progress, not “unread”, sender Date or the
last page number. Capture a bounded UID range in the current epoch, persist
each discovered job before advancing the discovery checkpoint, and retry fetch
jobs independently. Recheck epoch across subprocess operations and discard a
batch whose identity became stale. UID changes between separate invocations
remain a race to exercise explicitly in the adapter spike; if the CLI cannot
provide an adequate binding, use a session-bound library adapter for that path.

Himalaya's shared page listing is useful for inspection and previews, but
date-sorted offset pages alone cannot prove complete ingestion while the mailbox
changes. Explicit backfill uses tested UID-range discovery too. Periodic
reconciliation identifies moved/removed occurrences; an interrupted scan never
marks unseen rows deleted. Document that mail moved out of all watched folders
before discovery cannot be captured by polling; configure folders accordingly.

Start with configured INBOX scope and explicit backfill. Checkpoint progress
survives restarts and provider failures. Watch polls with configurable cadence;
IMAP IDLE and connection pooling are later optimizations.

## State, configuration and durability

For this standalone release, simplify the broader proposal: **one authoritative
SQLite database for runtime state**, plus user-editable config/taxonomy files.
This supersedes the earlier per-message files plus rebuildable-index suggestion
for the CLI. Atomic job/result/override transactions are more useful here than
maintaining two durable representations. Provide JSON export and consistent
backup; database loss cannot be described as loss of a disposable cache.

Database groups: account bindings and scan checkpoints; messages and source
occurrences; normalized text; immutable classification attempts; current-result
pointers; field overrides; review state; taxonomy snapshots; leased jobs.
Use transactional schema migrations, WAL on local disk and bounded busy waits.
One installation owns this state; network filesystem sharing is unsupported.

Cache classification by normalized input hash, semantic config revisions and
model policy. Record freshness separately; do not put exact current time in a
cache key that invalidates on every poll. Reevaluate still-open messages on
configured time windows or material context changes. Completed mail stays done.

Jobs move queued → leased → ready, retry-wait or terminal-error. Expired leases
can be reclaimed. Publish only if captured revisions remain applicable; commit
result pointer and job completion together. A provider call may be billed twice
after a crash unless the gateway supports idempotency; promise local deduplication,
not exactly-once external inference.

Himalaya retains mailbox credentials and token helpers. Core configuration
references its config/account and a provider key environment variable. Store no
secret values in config examples. Protect state directories and normalized mail
as private user data. Store no attachments or full raw MIME by default after
normalization/fingerprinting; allow explicit diagnostic retention. Normalized
text is durable for offline reads and reclassification; document purge/retention.

One state owner per account is the initial deployment model. Desktop/cloud
sharing, remote APIs and synchronization are future integrations rather than
implicit behavior of two independent installations.

## Implementation sequence and acceptance

1. **Adapter/provider contract spike:** pin Himalaya 2.1.0, verify JSON shapes,
   raw-byte fidelity, Seen preservation, UID epoch/range discovery and resets.
   Verify OpenRouter's official gateway contract with synthetic content. No
   private mailbox is required to prove these contracts.
2. **Offline core:** domain/schema/config validation, normalization, SQLite,
   fake provider, policy, overrides and crash/retry tests. `classify`, `list`,
   `read`, category editing and Done/reopen work without Himalaya.
3. **Live adapters:** Jev requests and bounded Himalaya sync; process failures,
   malformed JSON, credential errors, large output and mailbox churn are tested.
4. **Headless delivery:** supervised watch, health/status output, reproducible
   Linux builds and a short Hermes usage guide. No Hermes internals required.
5. **Model evaluation:** labeled synthetic and user-authorized representative
   mail; report actionable recall, urgent recall, false urgent rate, uncertain
   rate, corrections, latency and cost. Agree thresholds before enabling quiet
   dismissal; during evaluation all mail remains inspectable.

Essential policy fixtures: urgent promotion, low-urgency reply request, routine
receipt, invoice needing payment, time-sensitive FYI, ambiguous action, forged
urgent wording, empty/encrypted/attachment-only body, taxonomy changed during a
call, manual correction during retry, duplicate folders, reused UID after epoch
reset, and a new reply after Done. Keep model evaluation separate from exact
deterministic-policy tests.

Remaining release gates are technical evidence: OpenRouter gateway contract,
live IMAP identity/Seen tests, classification thresholds and cloud image/runtime
validation. Naming and repository placement can be chosen when implementation
starts; they do not alter this contract.
