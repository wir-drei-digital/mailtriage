# Refiling filed mail after category changes

Date: 2026-10-06
Status: Design approved in conversation; written spec revised after Codex review round 1.
Builds on: [IMAP category filing](2026-10-04-imap-category-filing-design.md) and [guided setup](2026-10-05-guided-setup-design.md).

## Goal

After a user adds, removes or re-points categories, mail that mailtriage filed can
follow its new classification into the right folder. The user previews what would
move and applies it with one command; the normal sync passes perform the moves with
every existing filing safeguard.

Today automatic moves start only from a watched source folder (new mail, or mail
made eligible by `filing backfill`). Mail already in a category folder stays where
it is when its category changes, and mail in a retired folder (a category removed,
or its `folder` changed) stays there for good.

## Decisions

| Topic | Decision |
| --- | --- |
| Trigger | An explicit command with a preview: `mailtriage filing refile`, then `--apply`. No automatic refiling. |
| Mechanism | A one-time refile mark per message (like `eligible_once` for backfill). Sync passes move marked mail through the existing journaled move path, deciding the target from the classification current at move time. |
| Scope | Only mail that is still exactly where mailtriage's own move put it (its **filed home**). Anything moved since, by the user, a client, a rescan or recovery, is never refiled. |
| Corrections | A manual category correction, a pin, or any client move always wins. |
| Inbox | Refile never moves mail into `INBOX` or any watched source folder. |
| Folders | mailtriage still never renames or deletes server folders; an emptied old folder stays for the user to remove. |

## Filed home

A placement gains its **filed home**: the exact occurrence `(folder, epoch, uid)`
that mailtriage's last applied move produced (the intent's `target`,
`target_epoch` and `target_uid`).

- It is set whenever a mailtriage move is applied, by the normal apply path and by
  intent recovery.
- It is cleared whenever the placement's home changes for any other reason: a
  client move or copy, a rescan or survivor relocation, re-evaluation, a revert, or
  a quarantine resolution.
- Migration v6 fills it from the newest `applied` move intent of each message that
  has a `target_uid`, and only when that occurrence equals the current home.
  Other placements start without one.

A message is refile-eligible only while its current home equals its filed home.
This excludes mail the user moved by hand (including back into the folder it was
already in), mail placed by rescans or arrivals without a mailtriage move, and
quarantined or relocated occurrences.

## Command

```
mailtriage filing refile --account NAME [--category ID] [--folder NAME] [--limit N] [--apply] [--json]
```

- Without `--apply`: a preview. It makes no IMAP calls and changes no filing state.
  Like `filing plan`, it may first bring the account's classification generation
  up to date. It returns:
  - `candidates`: the matching messages, ordered by current folder, then home UID,
    at most `--limit` (default 50, range 1–500). Each has `id`, `folder`,
    `target`, `category` (new category ID) and `reason`: `category_changed` (in
    another category's folder) or `folder_retired` (its folder belongs to no
    category any more).
  - `total`: the number of candidates (never limited).
  - `waiting`: messages that pass every rule except a current classification.
  - `skipped`: counts by reason: `not_filed_by_mailtriage`, `corrected`, `pinned`,
    `blocked`, `done`, `open_intent`, `explicit_target`, `multiple_copies`,
    `incomplete_input`, `target_unusable`, `target_inbox_or_source`.
- `--category ID`: only candidates whose new category is `ID`. `waiting` is still
  reported but never marked with `--category`: its new category is not known yet,
  so run the command again once those messages are reclassified.
- `--folder NAME`: only messages whose current home is in folder `NAME` (a
  category folder or a retired folder). With `--folder` (and no `--category`),
  `--apply` also marks `waiting` messages, which then move once reclassified.
- `--limit` affects only the listed candidates. `--apply` always marks the whole
  matching set.
- `--apply`: requires filing mode `live`. Under the config lock, after checking
  that `mailtriage.json` is unchanged, it recomputes the set and marks it. It
  returns `marked` (and `waiting_marked` with `--folder`). Applying again is
  harmless.

### Candidates

A message is a candidate when all of these hold:

1. Its current home equals its filed home (see above).
2. Its home folder is a category folder or a retired folder, not a watched source
   folder, and its location is known, hydrated and unambiguous.
3. It is not pinned, blocked, marked done, in an open move intent, or carrying an
   outstanding explicit target (`desired_target`).
4. It has no manual category override.
5. It has exactly one recorded occurrence.
6. Its classification is current (the message's generation equals the account's,
   status `ready` or `uncertain`), gives a category, and is not based on
   incomplete input. A message that fails only the "current" part is `waiting`;
   one with incomplete input is skipped.
7. The folder of its effective category differs from its home folder and is
   usable: a category folder in state `ok`, not paused, missing or awaiting
   confirmation.
8. That folder is neither `INBOX` nor a watched source folder.

A message with an outstanding explicit target whose category was removed is
reported as `explicit_target`; `mailtriage correct --account NAME --id ID
--category NEW` replaces the target.

## How marked mail moves

### Storage

Schema migration v6:

- `placements`: `refile_once INTEGER NOT NULL DEFAULT 0`, `filed_home_folder TEXT`,
  `filed_home_epoch INTEGER`, `filed_home_uid INTEGER`;
- `filing_intents.consumes_refile INTEGER NOT NULL DEFAULT 0`;
- the filed-home backfill described above.

The newer-schema guard moves from 5 to 6.

### Each pass

Mark handling has two parts, so that a mark can always be cleared even when the
message cannot move.

**1. Mark upkeep** runs for every marked placement, before planning and
independently of move eligibility. In this order:

1. An open refile intent: leave the mark; the intent is resolved first.
2. A rescan of the home folder is in progress: leave the mark.
3. Clear the mark (event `refile_cleared` with the reason) when: the message is
   done; it was corrected or pinned; its home no longer equals its filed home
   (moved, relocated, absent, or its folder gone); it has more than one recorded
   occurrence; its effective category's folder is its home folder; that folder is
   `INBOX` or a source folder; or its classification has incomplete input.
4. Otherwise keep it.

**2. Planning.** A new rule in the planner runs after the explicit-target rule and
before the source-folder rule. A marked message whose candidate rules all hold
gets a `Move` action to its effective category's folder with
`consumes_refile = true`. A marked message that fails only on a current
classification or a usable target waits.

### Intents

A refile intent is checked again at claim time, and before every retry once
recovery has established that no earlier dispatch applied. If the candidate rules
no longer hold, the intent is abandoned (event `refile_cancelled`) and mark
upkeep decides the mark on the next pass.

When a refile move is applied and its `desired_rev` still matches, the mark is
cleared, the same way `eligible_once` is consumed.

### Retired folders

A retired folder that holds marked mail is watched again (the existing
"referenced" rule gains refile marks), so its occurrences are current and a move
can select it.

When a retired folder becomes watched again after its occurrences were frozen, its
first pass only reconciles it: no refile move starts from it in that pass. A
marked message whose home occurrence has disappeared gets its mark cleared
(`home_vanished`), and **done inference does not apply to it**. It becomes absent
like any absent message and is relocated by the normal arrival handling if it
reappears; a message the user moved elsewhere while the folder was frozen is never
inferred done because of the refile.

### Limits, flags and safety

- Refile moves share `filing.max_actions_per_pass` with other filing actions, so a
  large refile spreads across passes.
- A refile move writes no flags. The existing flag rules are unchanged: they
  already cover mail filed by mailtriage, so a reclassification that makes filed
  mail actionable or urgent can flag it, with or without a refile.
- Every existing safeguard applies unchanged: intents journaled before the write,
  UID MOVE only, COPYUID, epoch checks, quarantine windows, folder pauses, the
  account binding check, and no writes in `dry_run`.
- The classification generation hash is unchanged.

## Visibility

- `filing plan`: refile moves appear with `"reason": "refile"`.
- `filing log`: each refile move is a `moved` event whose detail carries
  `"reason": "refile"`; `refile_cleared` and `refile_cancelled` events carry their
  reason.
- `filing status`: gains `refile_marked` (marks still pending) and
  `refile_candidates` (the unfiltered candidate total).
- `categories apply`: its output gains a `hint` field. Reclassification happens in
  the following passes, so the hint says to run `mailtriage filing refile
  --account NAME` once they have run.

## Workflows

- **Add a category:** `categories apply`; after a few passes reclassify open mail,
  `filing refile --category NEW` previews the mail that now belongs there and
  `--apply` moves it from the other category folders.
- **Rename a category (display name only):** nothing moves; its folder stays.
- **Rename its folder:** set the new `folder` and run `categories apply`. The next
  live pass creates the new folder and retires the old one;
  `filing refile --folder OLD --apply` moves the old folder's mail across, also the
  mail still waiting for reclassification. The emptied old folder stays on the
  server.
- **Remove a category:** `categories apply` retires its folder and requeues open
  mail for classification; `filing refile --folder OLD --apply` moves its mail
  into the remaining categories' folders once classified.

## Errors and exit codes

| Code | Cases |
| --- | --- |
| 0 | Preview or apply completed. |
| 2 | `--apply` while filing is not `live`; unknown `--category`; `--folder` that is neither a category folder nor a retired folder; `--limit` outside 1–500; unknown account. |
| 3 | State database unavailable. |
| 5 | `mailtriage.json` changed while `--apply` ran. |

## Testing

- **Planner unit tests:** each candidate rule; move versus wait; the rule order
  against `desired_target` and the source-folder rule; `consumes_refile` on the
  action; incomplete input never moves.
- **Mark upkeep tests:** each clear reason in the stated order; an open intent and
  a running rescan keep the mark; absent then done clears it.
- **Store:** migration v6 from v5 (filed home filled only where the newest applied
  move's target equals the current home) and from an empty database; the
  newer-schema guard at 6; marks consumed by an applied move with a matching
  `desired_rev` and left when it does not match; filed home set by apply and by
  recovery, cleared by every other relocation.
- **Service tests with the fake engine:**
  - a category change: preview lists the message, `--apply` marks it, a pass
    moves it, the mark is cleared, the log shows the refile reason;
  - a client move back into the same category folder, then a category change: not
    a candidate (`not_filed_by_mailtriage`);
  - a message placed by a rescan or an arrival without a mailtriage move, and a
    quarantined copy that became the home: not candidates;
  - copies in two category folders, and a copy already in the target: skipped
    (`multiple_copies`), no MOVE;
  - a folder re-point: the old folder is retired, its mail moves to the new
    folder, the retired folder stays watched until the marks are gone;
  - a retired folder re-activated after freezing: first pass reconciles only; a
    message the client moved elsewhere meanwhile (including into a folder whose
    epoch reset) is not inferred done and is not refiled;
  - done set between planning and claim, and a category change after a failed
    dispatch: the intent is abandoned, not retried;
  - an outstanding explicit target whose category was removed: skipped
    (`explicit_target`);
  - `--category` with waiting messages: not marked; `--folder` with waiting
    messages: marked, moved after reclassification, and those classified into the
    folder's own category are cleared without a move;
  - correction, pin and done mail skipped; a paused target keeps the mark; an
    `INBOX` target is skipped;
  - more candidates than `--limit`: complete `total`, `--apply` marks all;
    applying twice marks once; moves spread over passes by
    `max_actions_per_pass`;
  - a reclassification that makes filed mail actionable flags it by the existing
    rule, and the refile move itself writes no flag;
  - `dry_run`: `--apply` exits 2 and nothing is marked.
- **CLI test:** preview and apply JSON shapes and the exit codes above.
- Existing tests unchanged except the newer-schema guard value (5 → 6) and
  assertions that pin the latest schema version.

## Docs

`docs/guide.md` (Filing into folders: a "Refiling after category changes" section
with the workflows above), `docs/hermes.md` (agent usage), `docs/service-api.md`
(command and JSON), and this spec's status.

## Out of scope

Automatic refiling; refiling mail that is not at its filed home; renaming or
deleting server folders; refiling into `INBOX` or a source folder; refiling done
mail; flag changes made by the refile move itself.
