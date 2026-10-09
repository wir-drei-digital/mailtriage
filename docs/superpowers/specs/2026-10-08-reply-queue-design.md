# Reply queue

Date: 2026-10-08
Status: Implemented on branch `feature/reply-queue`. Open questions were
decided by Michael on 2026-10-08. The same day he asked for the read state
to wait for his approval ("Read approval"); the implementation needs no new
intent kind, and only the read approval list adds a table (SQLite v9; v10
adds its attempt columns).
Builds on: [IMAP category filing](2026-10-04-imap-category-filing-design.md),
[Filing refile](2026-10-06-filing-refile-design.md)

## Goal

The inbox should show only the mail that still needs an answer. Mail that
needs a reply stays where the user works. Everything else goes straight to its
category folder. Once the user has answered, mailtriage moves the message to
its category folder, unread, and lists it for the user's approval; only an
approved message is marked read. The user then sees at a glance what is still
open, in any mail client, phone included.

## Decisions

| Topic | Decision |
| --- | --- |
| Queue location | The **source folder (INBOX) is the queue**. A message whose effective decision is `action_required` is held there instead of being filed. A separate queue folder is the alternative in [Option B](#option-b-a-separate-queue-folder). |
| Entry | New mail with a current classification (or an override) and `action_required = true` is **held**: no move. |
| Flag | A held message gets the `\Flagged` attempt only for high urgency; the inbox itself shows what needs action. Mail the queue does not hold (old, backfilled or filed mail) is flagged as without the queue. A reply exit consumes the message's flag attempt, so answered mail is not flagged after it leaves, even once the queue is off. |
| Everything else | Unchanged: moved once to its category folder. |
| Reply signal | The IMAP `\Answered` flag on the held message. Mail clients set it when the user replies from that client. |
| Exit | A held message observed with `\Answered` is moved to its category folder at once, **unread**, and enters the read approval list. |
| Read approval | `mailtriage filing replies` lists that mail; `--approve` (all, or each `--id`) lets the next pass add `\Seen`. |
| Manual exits | `mailtriage done --id` on a held message triggers the same exit. A client move into a category folder still counts as a correction plus exit. Archiving still means done. |
| Read state | mailtriage **adds** `\Seen` only to answered mail the user approved. It still never removes `\Seen` and never removes a flag. |
| Off switch | `filing.reply_queue: false` (the default) keeps today's behaviour exactly. |
| Invoices | A payment sets no `\Answered`; a paid invoice leaves the queue with `done`. |
| Existing mail | Only new mail is held. Mail in INBOX before filing was enabled is left alone, and backfilled mail files as before. |
| Hermes | mailtriage replaces the Hermes `mail-triage` job for `michael@` (see below). |

Why the inbox and not a separate folder:

- **Fewer moves.** A held message is moved once, at the exit. That keeps
  invariant 7 ("moved automatically at most once") as it is. Invariant 4 also
  holds, because the exit leaves a source folder.
- **No new folder role.** The inbox is already watched, so recovery, done
  inference and arrivals need no new role.
- **Existing habits stay intact.** The Hermes `mail-triage` job reads only
  `INBOX`, and the team already treats a move to `INBOX/Done` as "handled".
  Both keep working.

## Behaviour

### Config

```json
"filing": { "mode": "live", "flag": true, "max_actions_per_pass": 200, "reply_queue": true }
```

`reply_queue` defaults to `false` and is written only when true. A config
that turns it on is written as schema 4; every other config stays schema 3.
Binaries without the queue accept only schemas 1 to 3, so they refuse a
config that uses it instead of rewriting it without `reply_queue`, and keep
reading every config that does not. Turn it on with
`mailtriage filing enable --account NAME --mode live --reply-queue on`
(`off` turns it off; without the flag the configured value is kept).

### Each pass

1. **Read replies.** Before planning, `filing::reply::refresh_flags` reads
   the flags of every held, not yet answered message again (one `envelopes`
   call per source folder and epoch, 100 UIDs each) and stores the ones that
   changed. Flags are otherwise stored only at discovery and hydration. A
   folder whose UIDVALIDITY changed meanwhile is skipped. A failure counts
   as `reply_check_failed:<folder>`.
2. **Plan.** In `move_action`, a message in a source folder, unpinned and
   without an explicit request, is held when `planner::holds` is true: the
   queue is on, the message is new (unfiled, internal date at or after
   `enabled_at`), and its effective `action_required` is true from an
   override or a current classification. A held message gets no move unless
   it is answered (`\Answered`) or done (`review_state` `done`). Then its
   move to the category folder is a **reply exit**, listed in
   `Plan.reply_exits`. Held messages are listed in `Plan.awaiting_reply`.
3. **Apply.** Reply exits form their own batches (the batch key gains a
   reply-exit bit) and move like every move: same intent, journal,
   verification, race handling and recovery. A reply exit enters the read
   approval list in the transaction that claims its move
   (`Store::claim_move_with` with `MoveClaim::ReplyExit`), once per
   message, so a crash cannot separate the two.
4. **Read.** After the moves, `filing::reply::apply_reads` adds `\Seen` to
   every approved message that is not read yet, in its current home: known,
   unblocked, not being moved, its folder unpaused and in its discovery
   epoch. Right before the write it reads the UIDs' envelopes and keeps only
   those that still show the stored Message-ID and size, as batch
   verification does (`apply::matches_meta`); any other is not written and
   the pass reports `read_mismatch:<folder>`. A message that already
   carries `\Seen` is recorded without a write. The write is
   `MailEngine::add_seen`: `a1 SELECT folder; a2 UID STORE uids
   +FLAGS.SILENT (\Seen)`. Before it, the rows record the session's folder
   and epoch (`attempt_folder`, `attempt_epoch`); a known outcome clears
   them. The outcome maps like a flag's:
   - `selected`, `session_epoch` equal to the verified epoch, `completed`:
     `applied_at` is set.
   - `selected` in another epoch: an epoch race. The folder pauses
     (`epoch_race`), each message gets event `epoch_race` with kind `seen`,
     and the pass reports `epoch_race:<folder>`, in one transaction. Another
     message may now carry `\Seen`; it is never removed. The rows stay
     approved and are read once the folder is released.
   - not selected, or incomplete in the verified epoch: the rows wait for
     the next pass (`read_incomplete:<folder>`).
   - an error (`read_failed:<folder>`) or a crash: the outcome is unknown
     and the attempt stays; the row is not written again until it is
     settled. Intent recovery (step 5 of each pass) compares the folder's
     epoch with it: another epoch is a suspected race, handled as above
     with error `epoch_race_suspected`; the same epoch clears the attempt
     and the row is read again. Adding `\Seen` twice is harmless.

A message whose decision changes to "no action" while held files like any
new mail and does not enter the list. A held message whose category targets `INBOX`
stays where it is.

### Manual paths

- **`done --id`** on a held message is a reply exit on the next pass and
  enters the list like an answered one. This covers replies sent from
  elsewhere (Hermes, `icm-pim mail reply`, a phone app that does not set
  `\Answered`), paid invoices, and "no answer needed". A `reopen` before
  the exit's move has run keeps the message held: recovery checks a move
  intent before each retry (`filing::reply::holds_again`) and supersedes
  it (error `held`) when the planner would hold the message now, as the
  refile rules recheck a refile intent.
- **A client move into a category folder** is a correction, as today. It
  does not enter the list.
- **A client move into a non-watched folder** (archive, `INBOX/Done`) is done
  inference, as today.
- **`filing pin --id`** keeps a held message in the inbox, even after
  `\Answered`.

### Read approval

```sh
mailtriage filing replies --account work            # the list
mailtriage filing replies --account work --approve  # approve all
mailtriage filing replies --account work --approve --id ID --id ID
```

The list holds every reply exit whose `\Seen` is not added yet: `id`,
`subject`, `from`, `folder` (its known home; it shows the source folder until
the next pass confirms the move), `answered` (`false` for a `done` exit),
`requested_at` and `approved_at`. `waiting` counts the unapproved rows,
`approved_pending` the approved ones the next live pass reads, and
`approved` names the ids this call approved. An `--id` that is not waiting
fails with exit code 2 and approves nothing; `--id` needs `--approve`.

### Visibility

- `sync` (and the stored last pass): `filing.awaiting_reply`,
  `filing.reply_exits`, `filing.replies_checked` and `filing.reads_applied`,
  each only when not 0.
- `filing status`: `reply_queue`, `awaiting_reply`, `awaiting_reply_ids`,
  `read_waiting` and `read_approved_pending`.
- `filing plan`: a reply exit's move carries `"reason": "reply_exit"`.
- `filing enable`: the result carries `reply_queue`.

## Safety invariants (amendments)

- **Invariant 1:** "no `\Seen` change" becomes "never removes `\Seen`; adds
  it only to answered mail the user approved". The engine trait has exactly
  one new method, `add_seen`. tests/engine_contract.rs pins its exact text,
  and `assert_no_forbidden` now accepts `Seen` only in a STORE without a
  MOVE.
- **Invariant 3:** a reply exit is an ordinary move intent. `\Seen` writes
  are not intents: each is tracked on its `read_approvals` row, which
  records the session's folder and epoch before the engine call, so a lost
  outcome followed by an epoch change is detected as a suspected race, as
  for a flag.
- **Invariant 4:** unchanged. A reply exit leaves a source folder.
- **Invariant 7:** unchanged. A held message is moved once, at the exit.

The other invariants are untouched.

## Implementation

| Area | Change |
| --- | --- |
| Config | `FilingConfig.reply_queue`; `config::written_schema` (3, or 4 with the queue); `SCHEMA_VERSION` 4 is the newest readable. |
| CLI | `filing enable --reply-queue on\|off`; `Service::filing_enable_with`. |
| Planner | `PlanInput.reply_queue`, `PlanMessage.done`, `holds`, `reply_done`, `MoveDecision::{Held, ReplyExit}`, `Plan.{reply_exits, awaiting_reply}`; the flag rule drops "action required" for held messages. |
| Flags | `filing/reply.rs` (`refresh_flags`), called by `refile::plan_pass` before the planner; reuses `Store::hydrate`. |
| Engine | `MailEngine::add_seen` (Himalaya, fake with `FakeOp::Seen`, offline). |
| Apply | Reply-exit batches; a reply exit's claim (`MoveClaim::ReplyExit`) also inserts its read approval row and sets `flag_attempted_at`. |
| Read | `filing::reply::apply_reads`, after `apply` in `plan_and_apply`; `filing::reply::recover_reads` in `recover`, for attempts whose outcome was lost. |
| Recovery | `filing::reply::holds_again` before a move retry: an intent whose message is held again is superseded. |
| CLI | `filing replies [--approve [--id ID]...]`; `Service::filing_replies`. |
| State | Holding is recomputed every pass from placement, effective decision, flags and review state. SQLite v9 adds `read_approvals(account, message_id, requested_at, approved_at, applied_at)`; a new table, so processes of the previous release are unaffected, but a binary before v9 refuses the database. SQLite v10 adds the nullable `read_approvals.attempt_folder` and `attempt_epoch`. |

## Option B: a separate queue folder

`filing.reply_folder: "Antworten"` would move action mail into its own folder
and from there, after `\Answered`, to the category. This is closer to "a
folder of mail to answer", but costs considerably more:

- a new watched folder role (watch list, engine scope, `folder_views`,
  `still_a_target`/`writable`, validation, arrival `Kind`);
- a second automatic move per message, which breaks invariant 7 and needs a
  refile-like rule with its own consume marker;
- a move out of a non-source folder, which breaks invariant 4.

Recommendation: build the inbox queue first. If the inbox alone is not
enough, Option B can follow as an extension.

## Interaction with Hermes `mail-triage`

w3d/08_workflows/mail-triage/CONTEXT.md reads only `INBOX` of
`michael@wirdrei.digital`, and its rule is that the mailbox is not changed.
With live filing:

- The Hermes digest would see only held mail. Newsletters, system mail and
  finance mail would already be filed, so its counts per category would drop
  out.
- Infomaniak needs the provider check from docs/verification.md before
  `live`.

Decided on 2026-10-08: mailtriage replaces the Hermes job for `michael@`.
The Hermes job also reconciles the daily AKB balance, sends the Telegram
digest and writes tasks; those parts need a new home (for example a Hermes
job that reads `mailtriage list --json`) before the job is switched off.

## Testing

- Planner unit tests (planner.rs): hold, answered exit, done exit, flag held
  mail only for high urgency and other mail as before, only new, current
  or override, unpinned, decision change files without `\Seen`, inbox
  category stays.
- tests/reply_queue.rs (fake engine, real passes): answered mail is filed
  unread and read only after approval, while unanswered mail waits quietly;
  approval of single ids and refusal of unknown ones; done files unread for
  approval; mail without action is unchanged and unread; dry run previews
  `reply_exit` with no writes; a lost move response converges with one move;
  a failed read write is retried on the next pass; a `\Seen` session in
  another epoch pauses the folder; a lost `\Seen` outcome is a suspected
  race after an epoch change and converges without one; backfilled mail
  that needs action files and is flagged as before; answered mail is not
  flagged after its exit, even with the queue turned off; a done exit
  reopened before its retry stays held and is not moved again.
- tests/engine_contract.rs: the exact `add_seen` text.
- tests/config_v2.rs: the queue alone makes a config schema 4.
- Dovecot e2e (flat and prefix), step 6: held unread and unflagged in INBOX,
  `\Answered`, filed to Transactions unread, listed, approved, then `\Seen`
  with `\Answered` kept.
- Still open: the provider check on Infomaniak. On 2026-10-08, 54 of the 329 INBOX messages of
  `michael@wirdrei.digital` carried `\Answered`, so the clients in use set
  it; which ones (webmail, the Infomaniak Mail app) and whether replies via
  Himalaya or icm-pim set it is still to check.

## Docs

The following docs need updates:

- README ("never changes the read state");
- guide.md:24, the "What is moved and flagged" and "Safety rules" sections,
  and a new "Reply queue" section;
- service-api.md (new fields, `filing replies`, schemas v9 and v10);
- verification.md (new rows);
- the filing spec's invariants list (a pointer to this spec).

## Decided questions

1. `\Flagged`: with the queue on, only high urgency flags a held message.
2. Payment: a paid invoice leaves the queue with `done`.
3. Existing mail: only new mail is held.
4. Hermes: mailtriage replaces the Hermes job for `michael@`.
5. Sent-folder detection stays out of scope for now.
6. Read state: answered mail is filed at once but stays unread until the
   user approves it with `filing replies --approve` (all, or single ids).

## Out of scope

- Detecting replies by reading the Sent folder or `In-Reply-To`.
- Removing `\Flagged` or `\Seen`.
- Thread-level filing (all messages of a conversation together).
