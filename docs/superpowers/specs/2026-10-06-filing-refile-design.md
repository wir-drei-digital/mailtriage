# Refiling filed mail after category changes

Date: 2026-10-06
Status: Design approved in conversation; written spec awaiting review.
Builds on: [IMAP category filing](2026-10-04-imap-category-filing-design.md) and [guided setup](2026-10-05-guided-setup-design.md).

## Goal

After a user adds, removes or re-points categories, mail that mailtriage already
filed can follow its new classification into the right folder. The user previews
what would move and applies it with one command; the normal sync passes perform
the moves with every existing filing safeguard.

Today automatic moves start only from a watched source folder (new mail, or mail
made eligible by `filing backfill`). Mail already in a category folder stays where
it is when its category changes, and mail in a retired folder (a category removed,
or its `folder` changed) stays there for good.

## Decisions

| Topic | Decision |
| --- | --- |
| Trigger | An explicit command with a preview: `mailtriage filing refile`, then `--apply`. No automatic refiling. |
| Mechanism | A one-time refile mark per message (like `eligible_once` for backfill). Sync passes move marked mail through the existing journaled move path, deciding the target from the classification current at move time. |
| Corrections | A manual category correction, a pin, or a client move always wins: such mail is never refiled. |
| Inbox | Refile never moves mail into `INBOX` or any watched source folder. |
| Folders | mailtriage still never renames or deletes server folders; an emptied old folder stays for the user to remove. |

## Command

```
mailtriage filing refile --account NAME [--category ID] [--folder NAME] [--limit N] [--apply] [--json]
```

- Without `--apply`: a read-only preview. No IMAP calls and no state changes. It
  returns the candidates (at most `--limit`, default 50, range 1–500; counts are
  always complete), each with `id`, `folder` (current), `target` (folder),
  `category` (new category ID) and `reason`:
  - `category_changed`: the message is in another category's folder;
  - `folder_retired`: its folder no longer belongs to any category.
  It also returns `waiting` (marked or matching mail whose classification is not
  current yet) and `skipped` counts by reason (`corrected`, `pinned`, `blocked`,
  `done`, `open_intent`, `target_unusable`, `target_inbox_or_source`).
- `--category ID`: only candidates whose new category is `ID`.
- `--folder NAME`: only candidates currently in folder `NAME`.
- `--apply`: requires filing mode `live`. Under the config lock, and after checking
  that `mailtriage.json` is unchanged, it recomputes the same set and sets the
  refile mark on each candidate (and on matching `waiting` messages, so they move
  once reclassified). It returns `marked`. Applying again is harmless.

### Candidates

A message is a candidate when all of these hold:

1. mailtriage tracks it (it has a placement).
2. Its home folder is a category folder or a retired folder, not a watched source
   folder.
3. It is not pinned, blocked, marked done, or in an open move intent.
4. It has no manual category override (`correct --category`, or a recorded client
   correction): the correction wins.
5. Its classification is current (the message's generation equals the account's
   and its status is `ready` or `uncertain`) and gives a category. Otherwise it
   counts as `waiting`.
6. The folder of its effective category differs from its home folder and is
   usable: a category folder in state `ok`, not paused, missing or awaiting
   confirmation.
7. That folder is neither `INBOX` nor a watched source folder.

## How marked mail moves

### Storage

Schema migration v6:

- `placements.refile_once INTEGER NOT NULL DEFAULT 0`;
- `filing_intents.consumes_refile INTEGER NOT NULL DEFAULT 0`.

The newer-schema guard moves from 5 to 6.

### Planner

A new rule runs after the explicit-target rule (`desired_target` from `correct`)
and before the source-folder rule. For a message with `refile_once`, each pass
re-checks the candidate rules against the classification current at that moment:

- **Move:** the home is a category folder in state `ok`, or a retired folder that
  LIST still reports, and rules 3–7 hold. A `Move` action with
  `consumes_refile = true` targets the effective category's folder.
- **Keep the mark (wait):** the classification is not current yet, the target
  folder is paused, missing or awaiting confirmation, or a move intent is open.
- **Clear the mark without moving:** the message is already in its category's
  folder; it was corrected, pinned or marked done since; the target is `INBOX` or
  a source folder; or its home folder is gone.

When a refile move is applied and its `desired_rev` still matches, the mark is
cleared, the same way `eligible_once` is consumed. A mark cleared without a move
records an event (`refile_cleared`, detail with the reason).

### Watching

A retired folder that holds marked mail stays watched (the existing "referenced"
rule: open intents, reverts, pending arrivals, rescan sets, and now refile marks),
so its occurrences stay current and a move can select it.

### Limits, flags and safety

- Refile moves share `filing.max_actions_per_pass` with other filing actions, so a
  large refile spreads across passes.
- Refile never adds or removes `\Flagged`.
- Every existing safeguard applies unchanged: intents journaled before the write,
  UID MOVE only, COPYUID, epoch checks, quarantine windows, folder pauses, the
  account binding check, and no writes in `dry_run`.
- The classification generation hash is unchanged (refile state is not part of it).

## Visibility

- `filing plan`: refile moves appear with `"reason": "refile"`.
- `filing log`: each refile move is a `moved` event whose detail carries
  `"reason": "refile"`; cleared marks are `refile_cleared` events.
- `filing status`: gains `refile_marked` (marks still pending) and
  `refile_candidates` (the unfiltered preview count).
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
  `filing refile --folder OLD --apply` moves the old folder's mail across. The
  emptied old folder stays on the server.
- **Remove a category:** `categories apply` retires its folder and requeues open
  mail for classification; `filing refile --folder OLD --apply` moves its mail
  into the remaining categories' folders.

## Errors and exit codes

| Code | Cases |
| --- | --- |
| 0 | Preview or apply completed. |
| 2 | `--apply` while filing is not `live`; unknown `--category`; `--folder` that is neither a category folder nor a retired folder; `--limit` outside 1–500; unknown account. |
| 3 | State database unavailable. |
| 5 | `mailtriage.json` changed while `--apply` ran. |

## Testing

- **Planner unit tests:** each candidate rule; each outcome (move, wait, clear);
  the rule order against `desired_target` and the source-folder rule;
  `consumes_refile` on the action.
- **Store:** migration v6 from v5 and from an empty database; the newer-schema
  guard at 6; marks set, consumed by an applied move with a matching
  `desired_rev`, and left when it does not match.
- **Service tests with the fake engine:**
  - a category change: preview lists the message, `--apply` marks it, a pass
    moves it, the mark is cleared and the log shows the refile reason;
  - a folder re-point: the old folder is retired, its mail moves to the new
    folder, the retired folder stays watched until the marks are gone;
  - a correction, a pin and done mail are skipped and never moved;
  - mail waiting for reclassification keeps its mark and moves after it;
  - a paused target keeps the mark; an `INBOX` target is skipped;
  - applying twice marks once; moves spread over passes by
    `max_actions_per_pass`;
  - `dry_run`: `--apply` exits 2 and nothing is marked.
- **CLI test:** preview and apply JSON shapes and the exit codes above.
- Existing tests unchanged except the newer-schema guard value (5 → 6) and
  assertions that pin the latest schema version.

## Docs

`docs/guide.md` (Filing into folders: a "Refiling after category changes" section
with the workflows above), `docs/hermes.md` (agent usage), `docs/service-api.md`
(command and JSON), and this spec's status.

## Out of scope

Automatic refiling; renaming or deleting server folders; refiling into `INBOX` or a
source folder; refiling done mail; changing flags during a refile.
