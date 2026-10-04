//! Intent recovery (spec "Writes and recovery", step 5 of each pass and after
//! any crash): every open intent converges by observing both ends, never by
//! assuming an outcome. Presence counts only for occurrences whose identity is
//! established (COPYUID or fingerprint) in the folder's current epoch.
use super::apply::{
    dispatch_moves, dispatch_reverts, error_patch, flag_applied, flag_raced, has_flagged,
    matches_meta, modify_placement, pause, paused, race_problem, revert_failed, write_failed,
};
use super::observe::FolderMap;
use super::planner::{CategoryFolder, Locator};
use super::{FilingSummary, Intent, IntentPatch, LocationState, PassContext};
use crate::domain::{FilingMode, SourceEnvelope};
use crate::store::{now, Store};
use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};
use serde_json::json;

/// Walks open intents oldest first, then the reverts (mode not off). Engine
/// writes (retries, reverts) happen only when `map.writes_allowed`.
pub fn recover(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    summary: &mut FilingSummary,
) -> Result<()> {
    if ctx.mode == FilingMode::Off {
        return Ok(());
    }
    for listed in store.intents(ctx.account, true)? {
        let Some(intent) = store.intent(listed.id)? else {
            continue;
        };
        let result = match intent.kind.as_str() {
            "flag" => recover_flag(store, ctx, &intent, summary),
            _ => recover_move(store, ctx, map, &intent, summary),
        };
        if let Err(e) = result {
            write_failed(e, "recovery_failed", &intent.folder, summary)?;
        }
    }
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

/// "In T": the intent's message occurs in `target` in its current epoch
/// `epoch` (when `bounded`, at `uid >= target_uid_next` or `== target_uid`), or
/// an arrival there carries the intent. Prefers the COPYUID target UID.
fn observe_in_target(
    store: &Store,
    ctx: &PassContext,
    intent: &Intent,
    (target, epoch): (&str, u64),
    bounded: bool,
) -> Result<Option<u64>> {
    let bound = intent.target_uid_next.filter(|_| bounded).unwrap_or(0);
    let in_window = |uid: u64| uid >= bound || intent.target_uid == Some(uid);
    let mut uids: Vec<u64> = store
        .occurrences_of(ctx.account, &intent.message_id)?
        .into_iter()
        .filter(|(f, e, u)| f == target && *e == epoch && in_window(*u))
        .map(|(_, _, u)| u)
        .collect();
    uids.extend(
        store
            .arrivals_at(ctx.account, target, epoch, bound)?
            .into_iter()
            .filter(|a| a.intent_id == Some(intent.id))
            .map(|a| a.uid),
    );
    if let Some(uid) = intent.target_uid.filter(|u| uids.contains(u)) {
        return Ok(Some(uid));
    }
    Ok(uids.into_iter().min())
}

/// "T scanned as for lost": T's checkpoint is complete in `target_epoch`,
/// scanned after `dispatched_at`, and no arrival in T at
/// `uid >= target_uid_next` is still `pending` or `unresolved`.
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
    let waiting = store
        .arrivals_at(ctx.account, target, epoch, bound)?
        .iter()
        .any(|a| matches!(a.state.as_str(), "pending" | "unresolved"));
    Ok(!waiting)
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
        _ => {
            store.update_intent(
                intent.id,
                "failed",
                error_patch("flag_not_observed"),
                &ctx.now,
            )?;
            store.record_event(
                ctx.account,
                Some(&intent.message_id),
                Some(&intent.folder),
                "flag_failed",
                json!({"intent_id": intent.id}),
                &ctx.now,
            )
        }
    }
}

/// Both ends of a move intent: the target and its current epoch.
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
        t_now: ctx.engine.snapshot(target)?.uid_validity,
        f_now: ctx.engine.snapshot(&intent.folder)?.uid_validity,
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
        Seen::Mismatch => return Ok(()),
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
        (None, false) => {
            store.update_intent(intent.id, "sent", IntentPatch::default(), &ctx.now)?;
            store.remove_occurrence(ctx.account, &from.folder, from.epoch, from.uid)
        }
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
            Seen::Mismatch | Seen::EpochChanged => return Ok(()),
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

/// Whether a folder's reset rescan in `epoch` is complete.
fn rescanned(store: &Store, ctx: &PassContext, folder: &str, epoch: u64) -> Result<bool> {
    Ok(store
        .folder_record(ctx.account, folder)?
        .is_some_and(|r| r.rescan_epoch == Some(epoch) && r.rescan_complete))
}

/// Spec "Epoch race", suspected (no session outcome captured): F pauses with
/// `epoch_race_suspected` and the intent awaits the rescans.
fn suspect_race(
    store: &mut Store,
    ctx: &PassContext,
    intent: &Intent,
    summary: &mut FilingSummary,
) -> Result<()> {
    pause(store, ctx.account, &intent.folder, "epoch_race_suspected")?;
    store.update_intent(
        intent.id,
        "awaiting_rescan",
        error_patch("epoch_race_suspected"),
        &ctx.now,
    )?;
    store.record_event(
        ctx.account,
        Some(&intent.message_id),
        Some(&intent.folder),
        "epoch_race",
        json!({"intent_id": intent.id, "kind": "move", "error": "epoch_race_suspected"}),
        &ctx.now,
    )?;
    race_problem(&intent.folder, summary);
    Ok(())
}

/// In both F and T: the move left a duplicate copy; blocked until released.
fn mark_duplicate(store: &mut Store, ctx: &PassContext, intent: &Intent) -> Result<()> {
    store.update_intent(intent.id, "failed", error_patch("duplicate_copy"), &ctx.now)?;
    modify_placement(store, ctx.account, &intent.message_id, |p| {
        p.blocked_reason = Some("duplicate_copy".into())
    })?;
    store.record_event(
        ctx.account,
        Some(&intent.message_id),
        intent.target.as_deref(),
        "duplicate_copy",
        json!({"intent_id": intent.id}),
        &ctx.now,
    )
}

/// In neither end once T settled: the message left both; it is absent.
fn mark_lost(store: &mut Store, ctx: &PassContext, intent: &Intent) -> Result<()> {
    store.update_intent(intent.id, "lost", IntentPatch::default(), &ctx.now)?;
    modify_placement(store, ctx.account, &intent.message_id, |p| {
        p.location_state = LocationState::Absent;
        p.absent_since = Some(ctx.now.clone());
    })
}

/// In F only with T settled: retried (re-claimed with job backoff) while the
/// intent's `desired_rev` is current, `failed` with `move_failed` once
/// `attempts` reach `max_attempts`, `superseded` when the request moved on
/// (a new revision, a block, or a target no longer filed into). The retry
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
    if intent.attempts >= ctx.max_attempts {
        store.update_intent(intent.id, "failed", error_patch("move_failed"), &ctx.now)?;
        modify_placement(store, ctx.account, &intent.message_id, |p| {
            p.blocked_reason = Some("move_failed".into())
        })?;
        return store.record_event(
            ctx.account,
            Some(&intent.message_id),
            Some(&intent.folder),
            "move_failed",
            json!({"intent_id": intent.id, "attempts": intent.attempts}),
            &ctx.now,
        );
    }
    // Without a resolved folder map (resolution failed) nothing is decided.
    if map.caps.is_none() {
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
    if !matches!(
        observe_in_source(store, ctx, &intent.message_id, &from)?,
        Seen::Present(_)
    ) {
        return Ok(());
    }
    let snapshot = ctx.engine.snapshot(&target)?;
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

/// Spec "Placement transitions", Move applied: home becomes the target
/// occurrence, `filed_by = mailtriage`; the desired fields are cleared (with a
/// revision bump) only while the intent's revision is current, so a newer
/// request is never consumed. Arrivals of the intent or at the new home
/// resolve as `own_move`.
pub(crate) fn mark_move_applied(
    store: &mut Store,
    ctx: &PassContext,
    intent: &Intent,
    home: (String, u64, u64),
    _summary: &mut FilingSummary,
) -> Result<()> {
    let (folder, epoch, uid) = home;
    modify_placement(store, ctx.account, &intent.message_id, |p| {
        p.home_folder = Some(folder.clone());
        p.home_epoch = Some(epoch);
        p.home_uid = Some(uid);
        p.location_state = LocationState::Known;
        p.absent_since = None;
        p.filed_at = Some(ctx.now.clone());
        p.filed_by = Some("mailtriage".into());
        if intent.desired_rev == p.desired_rev {
            let clear_target = p.desired_target.take().is_some();
            let clear_eligible = intent.consumes_eligible && p.eligible_once;
            if clear_eligible {
                p.eligible_once = false;
            }
            if clear_target || clear_eligible {
                p.desired_rev += 1;
            }
        }
    })?;
    store.update_intent(intent.id, "applied", IntentPatch::default(), &ctx.now)?;
    store.record_event(
        ctx.account,
        Some(&intent.message_id),
        Some(&folder),
        "moved",
        json!({"intent_id": intent.id, "from": intent.folder}),
        &ctx.now,
    )?;
    for a in store.arrivals_at(ctx.account, &folder, epoch, 0)? {
        let ours =
            a.intent_id == Some(intent.id) || (a.uid == uid && a.message_id == intent.message_id);
        if a.state == "pending" && ours {
            store.resolve_arrival(a.id, "resolved", Some("own_move"), &ctx.now)?;
        }
    }
    Ok(())
}
