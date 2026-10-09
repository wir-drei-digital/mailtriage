//! Intent recovery (spec "Writes and recovery", step 5 of each pass and after
//! any crash): every open intent converges by observing both ends, never by
//! assuming an outcome. Presence counts only for occurrences whose identity is
//! established (COPYUID or fingerprint) in the folder's current epoch.
use super::apply::{
    close, commit, commit_with_placement, dispatch_moves, dispatch_reverts, error_patch, event,
    flag_applied, flag_raced, has_flagged, matches_meta, paused, race_problem, race_until_uid,
    revert_failed, target_watched, write_failed,
};
use super::arrivals::in_race_window;
use super::observe::FolderMap;
use super::planner::{CategoryFolder, Locator};
use super::{refile, reply};
use super::{
    FilingSummary, FilingWrite, Intent, IntentPatch, LocationState, PassContext, Placement,
};
use crate::domain::{FilingMode, SourceEnvelope};
use crate::store::{now, Store};
use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use std::collections::BTreeMap;

/// Walks open intents oldest first, then the reply queue's `\Seen`
/// attempts (`reply::recover_reads`), then the reverts (mode not off).
/// Engine writes (retries, reverts) happen only when `map.writes_allowed`.
/// Each folder's epoch is read at most once per call (see `Epochs`).
pub fn recover(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    summary: &mut FilingSummary,
) -> Result<()> {
    if ctx.mode == FilingMode::Off {
        return Ok(());
    }
    record_race_bounds(store, ctx)?;
    let mut epochs = Epochs::default();
    for listed in store.intents(ctx.account, true)? {
        let Some(intent) = store.intent(listed.id)? else {
            continue;
        };
        let result = match intent.kind.as_str() {
            "flag" => recover_flag(store, ctx, &intent, summary),
            _ => recover_move(store, ctx, map, &mut epochs, &intent, summary),
        };
        if let Err(e) = result {
            write_failed(e, "recovery_failed", &intent.folder, summary)?;
        }
    }
    reply::recover_reads(store, ctx, summary)?;
    for r in store.reverts(ctx.account, true)? {
        // The session outcome of an interrupted revert is unknown.
        if r.state == "in_flight" {
            revert_failed(store, ctx, &r, "outcome_unknown", summary)?;
        }
    }
    if writes_allowed(ctx, map) {
        dispatch_reverts(store, ctx, summary)?;
    }
    Ok(())
}

/// A raced move whose target `UIDNEXT` could not be observed right after the
/// race gets it now, before any arrival in its target is resolved.
fn record_race_bounds(store: &mut Store, ctx: &PassContext) -> Result<()> {
    let mut verified = false;
    for intent in store.intents(ctx.account, false)? {
        let raced = intent.kind == "move"
            && intent.race_until_uid.is_none()
            && intent
                .error
                .as_deref()
                .is_some_and(|e| e.starts_with("epoch_race"));
        let Some(target) = intent.target.as_deref().filter(|_| raced) else {
            continue;
        };
        if !verified {
            (ctx.verify_binding)()?;
            verified = true;
        }
        if let Some(until) = race_until_uid(ctx, target, &intent) {
            let patch = IntentPatch {
                race_until_uid: Some(until),
                ..Default::default()
            };
            store.update_intent(intent.id, &intent.state, patch, &ctx.now)?;
        }
    }
    Ok(())
}

/// Folder epochs (UIDVALIDITY) read once per recovery call, for the two ends
/// of every move intent. Only the epoch is kept: it is all `Ends` compares,
/// and everything recovery concludes beyond it comes from stored occurrences
/// and checkpoints, which discovery wrote before recovery began. Because
/// UIDVALIDITY only increases, a cached epoch that differs from a recorded
/// one stays different; one that matches can go stale, exactly as a fresh
/// snapshot can right after it returns. Whatever needs the folder as it is
/// now takes its own snapshot: bracketed reads (`observe_in_source`), a
/// retry's target bounds, and race bounds (`race_until_uid`); every write
/// recovery dispatches selects its folder and checks the session epoch.
/// A failed snapshot is not cached, so the next intent asks again.
#[derive(Default)]
struct Epochs(BTreeMap<String, u64>);

impl Epochs {
    fn of(&mut self, ctx: &PassContext, folder: &str) -> Result<u64> {
        if let Some(epoch) = self.0.get(folder) {
            return Ok(*epoch);
        }
        let epoch = ctx.engine.snapshot(folder)?.uid_validity;
        self.0.insert(folder.to_owned(), epoch);
        Ok(epoch)
    }
}

fn writes_allowed(ctx: &PassContext, map: &FolderMap) -> bool {
    ctx.mode == FilingMode::Live && map.writes_allowed
}

/// What one bracketed `envelopes` read shows at a recorded locator.
enum Seen {
    /// The UID holds the message (same Message-ID and size).
    Present(SourceEnvelope),
    /// The UID is gone.
    Absent,
    /// The UID shows another Message-ID or size than stored: undecidable, so
    /// nothing is concluded from it.
    Mismatch,
    /// The folder is no longer in the recorded epoch.
    EpochChanged,
}

/// "In F": `envelopes(F, [uid])` bracketed by snapshots that both report `epoch`.
fn observe_in_source(
    store: &Store,
    ctx: &PassContext,
    message_id: &str,
    at: &Locator,
) -> Result<Seen> {
    (ctx.verify_binding)()?;
    if ctx.engine.snapshot(&at.folder)?.uid_validity != at.epoch {
        return Ok(Seen::EpochChanged);
    }
    let envelopes = ctx.engine.envelopes(&at.folder, &[at.uid])?;
    if ctx.engine.snapshot(&at.folder)?.uid_validity != at.epoch {
        return Ok(Seen::EpochChanged);
    }
    let Some(env) = envelopes.into_iter().find(|e| e.uid == at.uid) else {
        return Ok(Seen::Absent);
    };
    if !matches_meta(store, ctx.account, message_id, &env)? {
        return Ok(Seen::Mismatch);
    }
    Ok(Seen::Present(env))
}

/// "In T": a live occurrence of the intent's message in `target` in its
/// current epoch `epoch`; when `bounded`, at `uid >= target_uid_next` or at the
/// COPYUID `target_uid`. Prefers the COPYUID target UID.
fn observe_in_target(
    store: &Store,
    ctx: &PassContext,
    intent: &Intent,
    (target, epoch): (&str, u64),
    bounded: bool,
) -> Result<Option<u64>> {
    let bound = intent.target_uid_next.filter(|_| bounded).unwrap_or(0);
    let uids: Vec<u64> = store
        .occurrences_of(ctx.account, &intent.message_id)?
        .into_iter()
        .filter(|(f, e, u)| {
            f == target && *e == epoch && (*u >= bound || intent.target_uid == Some(*u))
        })
        .map(|(_, _, u)| u)
        .collect();
    if let Some(uid) = intent.target_uid.filter(|u| uids.contains(u)) {
        return Ok(Some(uid));
    }
    Ok(uids.into_iter().min())
}

/// "T scanned as for lost": T's checkpoint is complete in `target_epoch`,
/// scanned after `dispatched_at`, its watch in that epoch started below
/// `target_uid_next` (so the scanned range covers the move), and no arrival
/// in T at `uid >= target_uid_next` that could be the moved message is still
/// `pending` or `unresolved`. An arrival cannot be it when its known
/// Message-ID differs from the moved message's (identical bytes share it), or
/// when its occurrence is already gone from T.
fn target_scanned(store: &Store, ctx: &PassContext, intent: &Intent, target: &str) -> Result<bool> {
    let (Some(epoch), Some(bound)) = (intent.target_epoch, intent.target_uid_next) else {
        return Ok(false);
    };
    let Some((at_epoch, _, complete, scanned_at)) = store.checkpoint_state(ctx.account, target)?
    else {
        return Ok(false);
    };
    let dispatched = intent
        .dispatched_at
        .as_deref()
        .unwrap_or(&intent.created_at);
    if at_epoch != epoch || !complete || !later(scanned_at.as_deref(), dispatched) {
        return Ok(false);
    }
    // A first watch at or above `target_uid_next` never sees the move.
    if store.watch_floor(ctx.account, target, epoch)? >= bound {
        return Ok(false);
    }
    let ours = store
        .message_meta(ctx.account, &intent.message_id)?
        .and_then(|m| m.rfc_message_id);
    for a in store.arrivals_at(ctx.account, target, epoch, bound)? {
        if !matches!(a.state.as_str(), "pending" | "unresolved") {
            continue;
        }
        let other = matches!((&a.rfc_message_id, &ours), (Some(x), Some(y)) if x != y);
        let gone = store
            .occurrence_at(ctx.account, target, epoch, a.uid)?
            .is_none();
        if !other && !gone {
            return Ok(false);
        }
    }
    Ok(true)
}

fn time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// Whether `a` is strictly later than `b`; unparseable times are not.
fn later(a: Option<&str>, b: &str) -> bool {
    matches!((a.and_then(time), time(b)), (Some(a), Some(b)) if a > b)
}

/// Spec "Flags" step 3 for an `uncertain` (or interrupted `in_flight`) flag:
/// an epoch change is a suspected flag race; otherwise `\Flagged` present is
/// `applied` and absent is `failed`. Never retried.
fn recover_flag(
    store: &mut Store,
    ctx: &PassContext,
    intent: &Intent,
    summary: &mut FilingSummary,
) -> Result<()> {
    let at = Locator {
        folder: intent.folder.clone(),
        epoch: intent.epoch,
        uid: intent.uid,
    };
    match observe_in_source(store, ctx, &intent.message_id, &at)? {
        Seen::EpochChanged => flag_raced(
            store,
            ctx,
            intent.id,
            &intent.message_id,
            &intent.folder,
            "epoch_race_suspected",
            summary,
        ),
        Seen::Present(env) if has_flagged(&env.flags) => {
            flag_applied(store, ctx, intent.id, &intent.message_id, &intent.folder)
        }
        _ => commit(
            store,
            ctx,
            &[
                close(intent.id, "failed", Some("flag_not_observed")),
                event(
                    Some(&intent.message_id),
                    Some(&intent.folder),
                    "flag_failed",
                    json!({"intent_id": intent.id}),
                ),
            ],
        ),
    }
}

/// Both ends of a move intent: the target and the epochs both ends are in
/// this recovery call.
struct Ends<'i> {
    target: &'i str,
    target_epoch: u64,
    t_now: u64,
    f_now: u64,
}

fn recover_move(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    epochs: &mut Epochs,
    intent: &Intent,
    summary: &mut FilingSummary,
) -> Result<()> {
    let target = intent
        .target
        .as_deref()
        .ok_or_else(|| anyhow!("move intent without target"))?;
    let target_epoch = intent
        .target_epoch
        .ok_or_else(|| anyhow!("move intent without target epoch"))?;
    (ctx.verify_binding)()?;
    let ends = Ends {
        target,
        target_epoch,
        t_now: epochs.of(ctx, target)?,
        f_now: epochs.of(ctx, &intent.folder)?,
    };
    match intent.state.as_str() {
        "sent" => recover_sent(store, ctx, intent, &ends, summary),
        "awaiting_rescan" => recover_awaiting(store, ctx, map, intent, &ends, summary),
        // `in_flight` is treated as `uncertain`.
        _ => recover_uncertain(store, ctx, map, intent, &ends, summary),
    }
}

/// `sent`: waits for its arrival in T; `lost` once T has been scanned as for
/// lost; `awaiting_rescan` when T's epoch changed.
fn recover_sent(
    store: &mut Store,
    ctx: &PassContext,
    intent: &Intent,
    ends: &Ends,
    summary: &mut FilingSummary,
) -> Result<()> {
    if ends.t_now != ends.target_epoch {
        return store.update_intent(
            intent.id,
            "awaiting_rescan",
            IntentPatch::default(),
            &ctx.now,
        );
    }
    if let Some(uid) = observe_in_target(store, ctx, intent, (ends.target, ends.t_now), true)? {
        return mark_move_applied(
            store,
            ctx,
            intent,
            (ends.target.to_string(), ends.t_now, uid),
            summary,
        );
    }
    if target_scanned(store, ctx, intent, ends.target)? {
        return mark_lost(store, ctx, intent);
    }
    Ok(())
}

/// `uncertain` (and `in_flight`): a changed epoch at either end is a suspected
/// race; otherwise both ends are observed (spec intent-recovery table).
fn recover_uncertain(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    intent: &Intent,
    ends: &Ends,
    summary: &mut FilingSummary,
) -> Result<()> {
    if ends.f_now != intent.epoch || ends.t_now != ends.target_epoch {
        return suspect_race(store, ctx, intent, summary);
    }
    let from = Locator {
        folder: intent.folder.clone(),
        epoch: intent.epoch,
        uid: intent.uid,
    };
    let in_t = observe_in_target(store, ctx, intent, (ends.target, ends.t_now), true)?;
    let in_f = match observe_in_source(store, ctx, &intent.message_id, &from)? {
        Seen::EpochChanged => return suspect_race(store, ctx, intent, summary),
        Seen::Mismatch => return mismatch_wait(&intent.folder, summary),
        Seen::Present(_) => true,
        Seen::Absent => false,
    };
    match (in_t, in_f) {
        (Some(uid), false) => mark_move_applied(
            store,
            ctx,
            intent,
            (ends.target.to_string(), ends.t_now, uid),
            summary,
        ),
        (Some(_), true) => mark_duplicate(store, ctx, intent),
        (None, true) if target_scanned(store, ctx, intent, ends.target)? => {
            retry_or_supersede(store, ctx, map, intent, from, summary)
        }
        (None, true) => Ok(()),
        (None, false) => commit(
            store,
            ctx,
            &[
                FilingWrite::RemoveOccurrence {
                    folder: &from.folder,
                    epoch: from.epoch,
                    uid: from.uid,
                },
                close(intent.id, "sent", None),
            ],
        ),
    }
}

/// `awaiting_rescan`: waits until every folder whose epoch changed has a
/// complete reset rescan, then decides from identity-established occurrences
/// in the current epochs (bounds of a changed epoch are discarded).
fn recover_awaiting(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    intent: &Intent,
    ends: &Ends,
    summary: &mut FilingSummary,
) -> Result<()> {
    let f_changed = ends.f_now != intent.epoch;
    let t_changed = ends.t_now != ends.target_epoch;
    if (f_changed && !rescanned(store, ctx, &intent.folder, ends.f_now)?)
        || (t_changed && !rescanned(store, ctx, ends.target, ends.t_now)?)
    {
        return Ok(());
    }
    let in_t = observe_in_target(store, ctx, intent, (ends.target, ends.t_now), !t_changed)?;
    let in_f = if f_changed {
        store
            .occurrences_of(ctx.account, &intent.message_id)?
            .into_iter()
            .filter(|(f, e, _)| *f == intent.folder && *e == ends.f_now)
            .map(|(_, _, u)| u)
            .min()
    } else {
        let at = Locator {
            folder: intent.folder.clone(),
            epoch: intent.epoch,
            uid: intent.uid,
        };
        match observe_in_source(store, ctx, &intent.message_id, &at)? {
            Seen::Present(_) => Some(intent.uid),
            Seen::Absent => None,
            Seen::Mismatch => return mismatch_wait(&intent.folder, summary),
            Seen::EpochChanged => return Ok(()),
        }
    };
    match (in_t, in_f) {
        (Some(uid), None) => mark_move_applied(
            store,
            ctx,
            intent,
            (ends.target.to_string(), ends.t_now, uid),
            summary,
        ),
        (Some(_), Some(_)) => mark_duplicate(store, ctx, intent),
        // "Not in T" needs T settled: rescanned (checked above) or scanned as for lost.
        (None, _) if !t_changed && !target_scanned(store, ctx, intent, ends.target)? => Ok(()),
        (None, Some(uid)) => {
            let from = Locator {
                folder: intent.folder.clone(),
                epoch: ends.f_now,
                uid,
            };
            retry_or_supersede(store, ctx, map, intent, from, summary)
        }
        (None, None) => mark_lost(store, ctx, intent),
    }
}

/// A recorded source UID shows another Message-ID or size than stored:
/// nothing is concluded from it (the intent waits), and each pass that waits
/// on it reports the code-only problem `source_mismatch:<folder>` (once per
/// folder and pass).
fn mismatch_wait(folder: &str, summary: &mut FilingSummary) -> Result<()> {
    let code = format!("source_mismatch:{folder}");
    if !summary.problems.contains(&code) {
        summary.problems.push(code);
    }
    Ok(())
}

/// Whether a folder's reset rescan in `epoch` is complete.
fn rescanned(store: &Store, ctx: &PassContext, folder: &str, epoch: u64) -> Result<bool> {
    Ok(store
        .folder_record(ctx.account, folder)?
        .is_some_and(|r| r.rescan_epoch == Some(epoch) && r.rescan_complete))
}

/// Spec "Epoch race", suspected (no session outcome captured): F pauses with
/// `epoch_race_suspected` and the intent awaits the rescans, in one transaction.
fn suspect_race(
    store: &mut Store,
    ctx: &PassContext,
    intent: &Intent,
    summary: &mut FilingSummary,
) -> Result<()> {
    let reason = "epoch_race_suspected";
    let detail = json!({"intent_id": intent.id, "kind": "move", "error": reason});
    let race_until = intent
        .target
        .as_deref()
        .and_then(|t| race_until_uid(ctx, t, intent));
    commit(
        store,
        ctx,
        &[
            FilingWrite::Pause {
                folder: &intent.folder,
                reason,
            },
            FilingWrite::Intent {
                id: intent.id,
                state: "awaiting_rescan",
                patch: IntentPatch {
                    error: Some(reason.into()),
                    race_until_uid: race_until,
                    ..Default::default()
                },
            },
            event(
                Some(&intent.message_id),
                Some(&intent.folder),
                "epoch_race",
                detail,
            ),
        ],
    )?;
    race_problem(&intent.folder, summary);
    Ok(())
}

/// In both F and T: the move left a duplicate copy. The block and the failed
/// intent are one transaction, so the message is never unblocked and re-moved.
fn mark_duplicate(store: &mut Store, ctx: &PassContext, intent: &Intent) -> Result<()> {
    block(
        store,
        ctx,
        intent,
        "duplicate_copy",
        "duplicate_copy",
        intent.target.as_deref(),
    )
}

/// Fails a move intent and blocks its placement, with an event, atomically.
fn block(
    store: &mut Store,
    ctx: &PassContext,
    intent: &Intent,
    reason: &str,
    kind: &str,
    folder: Option<&str>,
) -> Result<()> {
    let detail = json!({"intent_id": intent.id, "attempts": intent.attempts});
    commit_with_placement(
        store,
        ctx,
        &intent.message_id,
        |p| p.blocked_reason = Some(reason.into()),
        &[
            close(intent.id, "failed", Some(reason)),
            event(Some(&intent.message_id), folder, kind, detail),
        ],
    )
}

/// In neither end once T settled: the message left both; it is absent. The
/// placement, the intent and event `lost` change together.
fn mark_lost(store: &mut Store, ctx: &PassContext, intent: &Intent) -> Result<()> {
    let detail = json!({"intent_id": intent.id, "from": intent.folder});
    commit_with_placement(
        store,
        ctx,
        &intent.message_id,
        |p| {
            p.location_state = LocationState::Absent;
            p.absent_since = Some(ctx.now.clone());
        },
        &[
            close(intent.id, "lost", None),
            event(
                Some(&intent.message_id),
                intent.target.as_deref(),
                "lost",
                detail,
            ),
        ],
    )
}

/// In F only with T settled: retried (re-claimed with job backoff) while the
/// intent's `desired_rev` is current, `failed` with `move_failed` once
/// `attempts` reach `max_attempts`, `superseded` when the request moved on
/// (a new revision, a block, a target no longer filed into, or a message
/// the reply queue holds again, error `held`). The retry
/// waits (no state change) for `next_after`, for writes to be allowed, and for
/// both folders to be writable.
fn retry_or_supersede(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    intent: &Intent,
    from: Locator,
    summary: &mut FilingSummary,
) -> Result<()> {
    let target = intent.target.clone().unwrap_or_default();
    let p = store
        .placement(ctx.account, &intent.message_id)?
        .ok_or_else(|| anyhow!("message has no placement"))?;
    if p.desired_rev != intent.desired_rev || p.blocked_reason.is_some() {
        return store.update_intent(intent.id, "superseded", IntentPatch::default(), &ctx.now);
    }
    // Refile spec "Intents": before every retry a refile intent is checked again.
    if intent.consumes_refile && map.caps.is_some() {
        if let Some(reason) = refile::intents::recheck(store, ctx, map, intent, &from)? {
            return refile::intents::cancel(store, ctx, intent, reason);
        }
    }
    // Reply queue spec: a reply exit whose message is held again (reopened)
    // is not retried.
    if map.caps.is_some() && reply::holds_again(store, ctx, map, intent)? {
        return store.update_intent(intent.id, "superseded", error_patch("held"), &ctx.now);
    }
    if intent.attempts >= ctx.max_attempts {
        return block(
            store,
            ctx,
            intent,
            "move_failed",
            "move_failed",
            Some(&intent.folder),
        );
    }
    // Without a resolved folder map (resolution failed) nothing is decided.
    if map.caps.is_none() {
        return Ok(());
    }
    // A retry rewrites the target bounds that define a race's quarantine
    // window, so it waits until every arrival in that window is settled.
    if race_window_unsettled(store, ctx, intent)? {
        return Ok(());
    }
    if !still_a_target(map, &target) {
        return store.update_intent(intent.id, "superseded", IntentPatch::default(), &ctx.now);
    }
    let backoff_open = intent
        .next_after
        .as_deref()
        .and_then(time)
        .is_none_or(|t| t <= Utc::now());
    if !writes_allowed(ctx, map)
        || !backoff_open
        || !writable(store, ctx, map, &from.folder, &target)?
    {
        return Ok(());
    }
    match observe_in_source(store, ctx, &intent.message_id, &from)? {
        Seen::Present(_) => {}
        Seen::Mismatch => return mismatch_wait(&from.folder, summary),
        Seen::Absent | Seen::EpochChanged => return Ok(()),
    }
    // Fresh, not from `Epochs`: `target_uid_next` bounds where the retried
    // move arrives and the race window (spec "Moves" step 2).
    let snapshot = ctx.engine.snapshot(&target)?;
    if !target_watched(store, ctx, &target, snapshot.uid_validity)? {
        return Ok(());
    }
    let attempts = intent.attempts + 1;
    let delay = 30_i64.saturating_mul(2_i64.pow(attempts.min(7)));
    let next = Intent {
        epoch: from.epoch,
        uid: from.uid,
        target_epoch: Some(snapshot.uid_validity),
        target_uid_next: Some(snapshot.uid_next),
        attempts,
        next_after: Some((Utc::now() + Duration::seconds(delay)).to_rfc3339()),
        ..intent.clone()
    };
    if !store.reclaim_move(&next, &now())? {
        return store.update_intent(intent.id, "superseded", IntentPatch::default(), &ctx.now);
    }
    dispatch_moves(
        store,
        ctx,
        &from.folder,
        from.epoch,
        &target,
        &[(intent.id, from.uid)],
        summary,
    )
}

/// A `pending` or `unresolved` arrival inside the intent's quarantine window.
fn race_window_unsettled(store: &Store, ctx: &PassContext, intent: &Intent) -> Result<bool> {
    let (Some(target), Some(epoch), Some(from)) = (
        intent.target.as_deref(),
        intent.target_epoch,
        intent.target_uid_next,
    ) else {
        return Ok(false);
    };
    Ok(store
        .arrivals_at(ctx.account, target, epoch, from)?
        .iter()
        .any(|a| {
            matches!(a.state.as_str(), "pending" | "unresolved")
                && in_race_window(intent, &a.folder, a.epoch, a.uid)
        }))
}

/// A retry only files into a folder that is still a source or the folder of
/// a configured category.
fn still_a_target(map: &FolderMap, target: &str) -> bool {
    map.sources.iter().any(|s| s == target)
        || map
            .categories
            .values()
            .any(|c| matches!(c, CategoryFolder::Native(n) if n == target))
}

/// Neither end paused, and the target a source or a category folder in state `ok`.
fn writable(
    store: &Store,
    ctx: &PassContext,
    map: &FolderMap,
    folder: &str,
    target: &str,
) -> Result<bool> {
    if paused(store, ctx.account, folder)? || paused(store, ctx.account, target)? {
        return Ok(false);
    }
    Ok(map.sources.iter().any(|s| s == target)
        || store
            .folder_record(ctx.account, target)?
            .is_some_and(|r| r.state == "ok"))
}

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
    writes.push(event(
        Some(&intent.message_id),
        Some(folder),
        "moved",
        detail,
    ));
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
        for (target_uid, home) in [
            (None, at(7, 21)),
            (Some(22), at(7, 21)),
            (Some(21), at(8, 21)),
        ] {
            let mut p = placement();
            apply_move(&mut p, &intent(target_uid, 3), &home, "t1");
            assert_eq!(p.filed_home_folder, None, "{target_uid:?} {home:?}");
            assert!(!p.refile_once, "the mark is consumed either way");
        }
    }

    #[test]
    fn a_stale_revision_keeps_the_mark_and_still_grants_the_proof() {
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
