# Reply queue

Date: 2026-10-08
Status: Implemented on branch `feature/reply-queue`. Open questions were
decided by Michael on 2026-10-08; the implementation is simpler than the
first draft (no new database state, no new intent kind), see "Implementation".
Builds on: [IMAP category filing](2026-10-04-imap-category-filing-design.md),
[Filing refile](2026-10-06-filing-refile-design.md)

## Goal

The inbox should show only the mail that still needs an answer. Mail that
needs a reply stays where the user works. Everything else goes straight to its
category folder. Once the user has answered, mailtriage moves the message to
its category folder and marks it read. The user then sees at a glance what is
still open, in any mail client, phone included.

## Decisions

| Topic | Decision |
| --- | --- |
| Queue location | The **source folder (INBOX) is the queue**. A message whose effective decision is `action_required` is held there instead of being filed. A separate queue folder is the alternative in [Option B](#option-b-a-separate-queue-folder). |
| Entry | New mail with a current classification (or an override) and `action_required = true` is **held**: no move. |
| Flag | With the queue on, only high urgency gets the `\Flagged` attempt; the inbox itself shows what needs action. |
| Everything else | Unchanged: moved once to its category folder. |
| Reply signal | The IMAP `\Answered` flag on the held message. Mail clients set it when the user replies from that client. |
| Exit | A held message observed with `\Answered` gets `\Seen` and is moved to its category folder. |
| Manual exits | `mailtriage done --id` on a held message triggers the same exit. A client move into a category folder still counts as a correction plus exit. Archiving still means done. |
| Read state | mailtriage **adds** `\Seen` only at the reply exit. It still never removes `\Seen` and never removes a flag. |
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
   reply-exit bit) and go through `MailEngine::move_messages_seen`:
   `a1 SELECT folder; s1 UID STORE uids +FLAGS.SILENT (\Seen); a2 UID MOVE
   uids target` in one session. The STORE runs while the UIDs are still
   valid, and `a2` stays the MOVE that the outcome mapping reads. The
   intent, journal, verification, race handling and recovery are those of
   every move.
4. **Retries.** A reply exit that is retried by recovery moves with
   `move_messages`, without `\Seen`: the read state is added at most once,
   in the first attempt's session.

A message whose decision changes to "no action" while held files like any
new mail, without `\Seen`. A held message whose category targets `INBOX`
stays where it is.

### Manual paths

- **`done --id`** on a held message is a reply exit on the next pass. This
  covers replies sent from elsewhere (Hermes, `icm-pim mail reply`, a phone
  app that does not set `\Answered`), paid invoices, and "no answer needed".
- **A client move into a category folder** is a correction, as today. No
  `\Seen` is added.
- **A client move into a non-watched folder** (archive, `INBOX/Done`) is done
  inference, as today.
- **`filing pin --id`** keeps a held message in the inbox, even after
  `\Answered`.

### Visibility

- `sync` (and the stored last pass): `filing.awaiting_reply`,
  `filing.reply_exits` and `filing.replies_checked`, each only when not 0.
- `filing status`: `reply_queue`, `awaiting_reply` and `awaiting_reply_ids`.
- `filing plan`: a reply exit's move carries `"reason": "reply_exit"`.
- `filing enable`: the result carries `reply_queue`.

## Safety invariants (amendments)

- **Invariant 1:** "no `\Seen` change" becomes "never removes `\Seen`; adds
  it only in the first attempt of a reply exit, in the MOVE's session". The
  engine trait has exactly one new method, `move_messages_seen`.
  tests/engine_contract.rs pins its exact text, and `assert_no_forbidden`
  now accepts `Seen` only in that form.
- **Invariant 3:** unchanged. A reply exit is an ordinary move intent.
- **Invariant 4:** unchanged. A reply exit leaves a source folder.
- **Invariant 7:** unchanged. A held message is moved once, at the exit.

The other invariants are untouched.

## Implementation

| Area | Change |
| --- | --- |
| Config | `FilingConfig.reply_queue`; `config::written_schema` (3, or 4 with the queue); `SCHEMA_VERSION` 4 is the newest readable. |
| CLI | `filing enable --reply-queue on\|off`; `Service::filing_enable_with`. |
| Planner | `PlanInput.reply_queue`, `PlanMessage.done`, `holds`, `reply_done`, `MoveDecision::{Held, ReplyExit}`, `Plan.{reply_exits, awaiting_reply}`; the flag rule drops "action required" with the queue on. |
| Flags | `filing/reply.rs` (`refresh_flags`), called by `refile::plan_pass` before the planner; reuses `Store::hydrate`. |
| Engine | `MailEngine::move_messages_seen` (Himalaya, fake, offline). |
| Apply | Reply-exit batches; `dispatch_moves` takes the reply-exit bit; recovery retries pass `false`. |
| State | No migration: holding is recomputed every pass from placement, effective decision, flags and review state. |

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

- Planner unit tests (planner.rs): hold, answered exit, done exit, flag only
  high, only new, current or override, unpinned, decision change files
  without `\Seen`, inbox category stays.
- tests/reply_queue.rs (fake engine, real passes): answered mail leaves read
  and filed while unanswered mail waits quietly; done files read; mail
  without action is unchanged and unread; dry run previews `reply_exit` with
  no writes; a lost response converges with one write; a failed exit is
  retried without `\Seen`.
- tests/engine_contract.rs: the exact `move_messages_seen` text.
- tests/config_v2.rs: the queue alone makes a config schema 4.
- Still open: the Dovecot end-to-end run and the provider check on
  Infomaniak. On 2026-10-08, 54 of the 329 INBOX messages of
  `michael@wirdrei.digital` carried `\Answered`, so the clients in use set
  it; which ones (webmail, the Infomaniak Mail app) and whether replies via
  Himalaya or icm-pim set it is still to check.

## Docs

The following docs need updates:

- README ("never changes the read state");
- guide.md:24, the "What is moved and flagged" and "Safety rules" sections,
  and a new "Reply queue" section;
- service-api.md (new fields and events, schema v9);
- verification.md (new rows);
- the filing spec's invariants list (a pointer to this spec).

## Decided questions

1. `\Flagged`: only high urgency is flagged with the queue on.
2. Payment: a paid invoice leaves the queue with `done`.
3. Existing mail: only new mail is held.
4. Hermes: mailtriage replaces the Hermes job for `michael@`.
5. Sent-folder detection stays out of scope for now.

## Out of scope

- Detecting replies by reading the Sent folder or `In-Reply-To`.
- Removing `\Flagged` or `\Seen`.
- Thread-level filing (all messages of a conversation together).
