//! Claiming and applying planned flags and moves (spec "Writes and
//! recovery"): session-bound batches, outcome mapping, epoch races and their
//! reverts. Every write is journaled as an intent (or revert) before the
//! engine call.
use super::observe::FolderMap;
use super::planner::{Action, Locator, Plan};
use super::{
    is_config_changed, rfc_message_id, FilingSummary, IntentPatch, PassContext, Placement, Revert,
};
use crate::domain::{FilingMode, SourceEnvelope};
use crate::engine::WriteOutcome;
use crate::store::{now, Store};
use anyhow::{bail, Result};
use serde_json::json;
use uuid::Uuid;

/// UIDs per write call (spec "Himalaya command mapping").
const BATCH: usize = 100;

/// Step 9 in `live`: clears satisfied explicit requests, then claims and
/// applies flags (per folder and epoch) and moves (per folder, epoch and
/// target) in batches of at most 100. Nothing happens unless writes are allowed.
pub fn apply(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    plan: &Plan,
    summary: &mut FilingSummary,
) -> Result<()> {
    if ctx.mode != FilingMode::Live || !map.writes_allowed {
        return Ok(());
    }
    clear_requests(store, ctx, plan)?;
    let flags = group(plan, |a| match a {
        Action::Flag { at, .. } => Some((at.folder.clone(), at.epoch, String::new())),
        Action::Move { .. } => None,
    });
    for ((folder, epoch, _), actions) in &flags {
        for chunk in actions.chunks(BATCH) {
            if let Err(e) = flag_batch(store, ctx, folder, *epoch, chunk, summary) {
                write_failed(e, "flag_batch_failed", folder, summary)?;
            }
        }
    }
    let moves = group(plan, |a| match a {
        Action::Move { from, to, .. } => Some((from.folder.clone(), from.epoch, to.clone())),
        Action::Flag { .. } => None,
    });
    for ((folder, epoch, to), actions) in &moves {
        for chunk in actions.chunks(BATCH) {
            if let Err(e) = move_batch(store, ctx, (folder, *epoch, to), chunk, summary) {
                write_failed(e, "move_batch_failed", folder, summary)?;
            }
        }
    }
    Ok(())
}

type BatchKey = (String, u64, String);

/// Actions grouped by `key`, groups and members in plan order.
fn group(plan: &Plan, key: impl Fn(&Action) -> Option<BatchKey>) -> Vec<(BatchKey, Vec<&Action>)> {
    let mut groups: Vec<(BatchKey, Vec<&Action>)> = Vec::new();
    for action in &plan.actions {
        let Some(k) = key(action) else { continue };
        match groups.iter_mut().find(|(g, _)| *g == k) {
            Some((_, members)) => members.push(action),
            None => groups.push((k, vec![action])),
        }
    }
    groups
}

/// Satisfied explicit requests: `desired_target` cleared, `desired_rev` bumped,
/// only while the revision the planner saw is still current.
fn clear_requests(store: &mut Store, ctx: &PassContext, plan: &Plan) -> Result<()> {
    for (id, rev) in &plan.cleared_requests {
        let Some(mut p) = store.placement(ctx.account, id)? else {
            continue;
        };
        if p.desired_rev != *rev {
            continue;
        }
        p.desired_target = None;
        p.desired_rev += 1;
        store.save_placement(&p, Some(*rev))?;
    }
    Ok(())
}

fn locator(action: &Action) -> &Locator {
    match action {
        Action::Move { from, .. } => from,
        Action::Flag { at, .. } => at,
    }
}

/// A failed batch or write is a filing error; a mail engine configuration
/// change aborts the pass instead.
pub(crate) fn write_failed(
    e: anyhow::Error,
    code: &str,
    folder: &str,
    summary: &mut FilingSummary,
) -> Result<()> {
    if is_config_changed(&e) {
        return Err(e);
    }
    summary.errors += 1;
    summary.problems.push(format!("{code}:{folder}"));
    Ok(())
}

pub(crate) fn paused(store: &Store, account: &str, folder: &str) -> Result<bool> {
    Ok(store
        .folder_record(account, folder)?
        .is_some_and(|r| r.pause_reason.is_some()))
}

/// Sets a safety pause; an existing pause keeps its reason.
pub(crate) fn pause(store: &mut Store, account: &str, folder: &str, reason: &str) -> Result<()> {
    let Some(mut rec) = store.folder_record(account, folder)? else {
        bail!("no folder record to pause");
    };
    if rec.pause_reason.is_none() {
        rec.pause_reason = Some(reason.into());
        store.save_folder(&rec)?;
    }
    Ok(())
}

/// Re-reads a placement, lets `f` change it and writes it back only while its
/// `desired_rev` is unchanged, re-reading after a concurrent change. `f` may
/// bump `desired_rev` itself.
pub(crate) fn modify_placement(
    store: &mut Store,
    account: &str,
    id: &str,
    mut f: impl FnMut(&mut Placement),
) -> Result<()> {
    for _ in 0..5 {
        let Some(mut p) = store.placement(account, id)? else {
            bail!("message has no placement");
        };
        let rev = p.desired_rev;
        f(&mut p);
        if store.save_placement(&p, Some(rev))? {
            return Ok(());
        }
    }
    bail!("placement kept changing concurrently")
}

pub(crate) fn has_flagged(flags: &[String]) -> bool {
    flags.iter().any(|f| f.eq_ignore_ascii_case("\\Flagged"))
}

/// Whether an envelope still shows the stored Message-ID and size of a message.
pub(crate) fn matches_meta(
    store: &Store,
    account: &str,
    id: &str,
    env: &SourceEnvelope,
) -> Result<bool> {
    let Some(meta) = store.message_meta(account, id)? else {
        return Ok(false);
    };
    Ok(rfc_message_id(env) == meta.rfc_message_id && env.size == meta.size)
}

/// Spec "Moves" step 1 and "Flags" step 1: re-verifies the binding and re-reads
/// the batch with `envelopes()` bracketed by `snapshot()`. `None` aborts the
/// batch (the folder is not in `epoch`); otherwise the actions whose UID is
/// present with the stored Message-ID and size and, for flags, lacks
/// `\Flagged`. A message found already flagged gets its stored envelope
/// refreshed, so it is not planned again.
fn verify_batch<'a>(
    store: &mut Store,
    ctx: &PassContext,
    (folder, epoch): (&str, u64),
    actions: &[&'a Action],
    for_flags: bool,
) -> Result<Option<Vec<&'a Action>>> {
    (ctx.verify_binding)()?;
    let uids: Vec<u64> = actions.iter().map(|a| locator(a).uid).collect();
    let before = ctx.engine.snapshot(folder)?;
    let envelopes = ctx.engine.envelopes(folder, &uids)?;
    let after = ctx.engine.snapshot(folder)?;
    if before.uid_validity != epoch || after.uid_validity != epoch {
        return Ok(None);
    }
    let mut kept = Vec::new();
    for action in actions {
        let Some(env) = envelopes.iter().find(|e| e.uid == locator(action).uid) else {
            continue;
        };
        if !matches_meta(store, ctx.account, action.message_id(), env)? {
            continue;
        }
        if for_flags && has_flagged(&env.flags) {
            store.hydrate(ctx.account, action.message_id(), env)?;
            continue;
        }
        kept.push(*action);
    }
    Ok(Some(kept))
}

fn new_batch() -> String {
    Uuid::new_v4().simple().to_string()
}

/// One flag batch: verify, claim (consuming the only flag attempt), one
/// `add_flagged` session, then map the outcome (spec "Flags" step 3).
fn flag_batch(
    store: &mut Store,
    ctx: &PassContext,
    folder: &str,
    epoch: u64,
    actions: &[&Action],
    summary: &mut FilingSummary,
) -> Result<()> {
    if paused(store, ctx.account, folder)? {
        return Ok(());
    }
    let Some(kept) = verify_batch(store, ctx, (folder, epoch), actions, true)? else {
        return Ok(());
    };
    let (batch, at) = (new_batch(), now());
    let mut claimed = Vec::new();
    for action in kept {
        if let Some(id) = store.claim_flag(ctx.account, action, &batch, &at)? {
            claimed.push((id, locator(action).uid, action.message_id().to_string()));
        }
    }
    if claimed.is_empty() {
        return Ok(());
    }
    let uids: Vec<u64> = claimed.iter().map(|c| c.1).collect();
    let result = ctx.engine.add_flagged(folder, &uids);
    let outcome = result.as_ref().ok();
    for (id, _, message_id) in &claimed {
        flag_outcome(
            store,
            ctx,
            folder,
            epoch,
            (*id, message_id),
            outcome,
            summary,
        )?;
    }
    result.map(drop)
}

fn flag_outcome(
    store: &mut Store,
    ctx: &PassContext,
    folder: &str,
    epoch: u64,
    (id, message_id): (i64, &str),
    outcome: Option<&WriteOutcome>,
    summary: &mut FilingSummary,
) -> Result<()> {
    match outcome {
        Some(o) if o.selected && o.session_epoch == Some(epoch) && o.completed => {
            flag_applied(store, ctx, id, message_id, folder)?;
            summary.flagged += 1;
            Ok(())
        }
        Some(o) if o.selected && o.session_epoch != Some(epoch) => {
            flag_raced(store, ctx, id, message_id, folder, "epoch_race", summary)
        }
        _ => {
            let code = if outcome.is_some() {
                "incomplete"
            } else {
                "dispatch_error"
            };
            store.update_intent(id, "uncertain", error_patch(code), &ctx.now)
        }
    }
}

pub(crate) fn flag_applied(
    store: &mut Store,
    ctx: &PassContext,
    id: i64,
    message_id: &str,
    folder: &str,
) -> Result<()> {
    store.update_intent(id, "applied", IntentPatch::default(), &ctx.now)?;
    modify_placement(store, ctx.account, message_id, |p| {
        p.flagged_at = Some(ctx.now.clone())
    })?;
    store.record_event(
        ctx.account,
        Some(message_id),
        Some(folder),
        "flagged",
        json!({"intent_id": id}),
        &ctx.now,
    )
}

/// Spec "Epoch race", flag race (detected or suspected): the folder pauses,
/// the flag intent fails; flags are never removed.
pub(crate) fn flag_raced(
    store: &mut Store,
    ctx: &PassContext,
    id: i64,
    message_id: &str,
    folder: &str,
    error: &str,
    summary: &mut FilingSummary,
) -> Result<()> {
    store.update_intent(id, "failed", error_patch(error), &ctx.now)?;
    pause(store, ctx.account, folder, "epoch_race")?;
    store.record_event(
        ctx.account,
        Some(message_id),
        Some(folder),
        "epoch_race",
        json!({"intent_id": id, "kind": "flag", "error": error}),
        &ctx.now,
    )?;
    race_problem(folder, summary);
    Ok(())
}

pub(crate) fn error_patch(code: &str) -> IntentPatch {
    IntentPatch {
        error: Some(code.into()),
        ..Default::default()
    }
}

/// Counts an epoch race as a filing error, reporting each folder once.
pub(crate) fn race_problem(folder: &str, summary: &mut FilingSummary) {
    summary.errors += 1;
    let code = format!("epoch_race:{folder}");
    if !summary.problems.contains(&code) {
        summary.problems.push(code);
    }
}

/// One move batch: verify, snapshot the target, claim, dispatch.
fn move_batch(
    store: &mut Store,
    ctx: &PassContext,
    (folder, epoch, to): (&str, u64, &str),
    actions: &[&Action],
    summary: &mut FilingSummary,
) -> Result<()> {
    if paused(store, ctx.account, folder)? || paused(store, ctx.account, to)? {
        return Ok(());
    }
    let Some(kept) = verify_batch(store, ctx, (folder, epoch), actions, false)? else {
        return Ok(());
    };
    if kept.is_empty() {
        return Ok(());
    }
    let target = ctx.engine.snapshot(to)?;
    let (batch, at) = (new_batch(), now());
    let mut claimed = Vec::new();
    for action in kept {
        let id = store.claim_move(
            ctx.account,
            action,
            target.uid_validity,
            target.uid_next,
            &batch,
            &at,
        )?;
        if let Some(id) = id {
            claimed.push((id, locator(action).uid));
        }
    }
    if claimed.is_empty() {
        return Ok(());
    }
    dispatch_moves(store, ctx, folder, epoch, to, &claimed, summary)
}

/// One `move_messages` session for claimed intents `(intent id, UID)` verified
/// in `folder`/`epoch`, mapped per spec "Moves" step 4. Every engine or
/// outcome failure leaves the intents `uncertain` for recovery.
pub(crate) fn dispatch_moves(
    store: &mut Store,
    ctx: &PassContext,
    folder: &str,
    epoch: u64,
    target: &str,
    claimed: &[(i64, u64)],
    summary: &mut FilingSummary,
) -> Result<()> {
    let uids: Vec<u64> = claimed.iter().map(|c| c.1).collect();
    let outcome = match ctx.engine.move_messages(folder, &uids, target) {
        Ok(o) => o,
        Err(e) => {
            set_all(store, ctx, claimed, "uncertain", "dispatch_error")?;
            return write_failed(e, "move_failed", folder, summary);
        }
    };
    if !outcome.selected {
        set_all(store, ctx, claimed, "uncertain", "not_selected")?;
        summary.errors += 1;
        summary.problems.push(format!("move_not_selected:{folder}"));
        return Ok(());
    }
    if outcome.session_epoch != Some(epoch) {
        return move_raced(store, ctx, folder, target, claimed, &outcome, summary);
    }
    for (id, uid) in claimed {
        let Some(intent) = store.intent(*id)? else {
            bail!("unknown filing intent");
        };
        let target_uid = outcome
            .copyuid
            .as_ref()
            .filter(|c| Some(c.target_epoch) == intent.target_epoch)
            .and_then(|c| c.pairs.iter().find(|(src, _)| src == uid))
            .map(|(_, dst)| *dst);
        let patch = IntentPatch {
            target_uid,
            error: (!outcome.completed).then(|| "incomplete".to_string()),
            ..Default::default()
        };
        if outcome.completed {
            store.update_intent(*id, "sent", patch, &ctx.now)?;
            store.remove_occurrence(ctx.account, folder, epoch, *uid)?;
        } else {
            store.update_intent(*id, "uncertain", patch, &ctx.now)?;
        }
    }
    if outcome.completed {
        summary.moved += claimed.len();
    } else {
        summary.errors += 1;
        summary.problems.push(format!("move_incomplete:{folder}"));
    }
    Ok(())
}

fn set_all(
    store: &mut Store,
    ctx: &PassContext,
    claimed: &[(i64, u64)],
    state: &str,
    code: &str,
) -> Result<()> {
    for (id, _) in claimed {
        store.update_intent(*id, state, error_patch(code), &ctx.now)?;
    }
    Ok(())
}

/// Spec "Epoch race" for a move session that ran in another epoch: with
/// COPYUID every moved message is journaled for a revert and the reverts are
/// dispatched; without it the source folder pauses. The intents await rescans.
fn move_raced(
    store: &mut Store,
    ctx: &PassContext,
    folder: &str,
    target: &str,
    claimed: &[(i64, u64)],
    outcome: &WriteOutcome,
    summary: &mut FilingSummary,
) -> Result<()> {
    set_all(store, ctx, claimed, "awaiting_rescan", "epoch_race")?;
    let copyuid = outcome.copyuid.as_ref().filter(|c| !c.pairs.is_empty());
    if let (Some(copyuid), Some(session)) = (copyuid, outcome.session_epoch) {
        let at = now();
        for (_, dst) in &copyuid.pairs {
            store.insert_revert(&Revert {
                id: 0,
                account: ctx.account.into(),
                parent_intent: claimed[0].0,
                folder: target.into(),
                folder_epoch: copyuid.target_epoch,
                uid: *dst,
                target: folder.into(),
                target_epoch: session,
                state: "pending".into(),
                target_uid: None,
                created_at: at.clone(),
                updated_at: at.clone(),
                error: None,
            })?;
        }
        return dispatch_reverts(store, ctx, summary);
    }
    pause(store, ctx.account, folder, "epoch_race")?;
    for (id, _) in claimed {
        let Some(intent) = store.intent(*id)? else {
            continue;
        };
        store.record_event(
            ctx.account,
            Some(&intent.message_id),
            Some(folder),
            "epoch_race",
            json!({"intent_id": id, "kind": "move", "error": "epoch_race"}),
            &ctx.now,
        )?;
    }
    race_problem(folder, summary);
    Ok(())
}

/// Dispatches every `pending` revert (spec "Epoch race"): the destination
/// folder must still be in the COPYUID epoch with the UID present, and the
/// revert session must run in that epoch. A failed or raced revert pauses its
/// target (the original source); a revert is never itself reverted.
pub(crate) fn dispatch_reverts(
    store: &mut Store,
    ctx: &PassContext,
    summary: &mut FilingSummary,
) -> Result<()> {
    for r in store.reverts(ctx.account, true)? {
        if r.state != "pending" {
            continue;
        }
        if let Err(e) = revert_one(store, ctx, &r, summary) {
            write_failed(e, "revert_failed", &r.target, summary)?;
        }
    }
    Ok(())
}

fn revert_one(
    store: &mut Store,
    ctx: &PassContext,
    r: &Revert,
    summary: &mut FilingSummary,
) -> Result<()> {
    if paused(store, ctx.account, &r.folder)? || paused(store, ctx.account, &r.target)? {
        return revert_failed(store, ctx, r, "paused", summary);
    }
    (ctx.verify_binding)()?;
    let in_epoch = ctx.engine.snapshot(&r.folder)?.uid_validity == r.folder_epoch;
    let present = in_epoch
        && ctx
            .engine
            .envelopes(&r.folder, &[r.uid])?
            .iter()
            .any(|e| e.uid == r.uid);
    if !present {
        return revert_failed(store, ctx, r, "revert_unverified", summary);
    }
    store.update_revert(r.id, "in_flight", None, None, &now())?;
    let outcome = match ctx.engine.move_messages(&r.folder, &[r.uid], &r.target) {
        Ok(o) => o,
        // The engine refused before running anything: still pending.
        Err(e) if is_config_changed(&e) => {
            store.update_revert(r.id, "pending", None, None, &ctx.now)?;
            return Err(e);
        }
        Err(_) => return revert_failed(store, ctx, r, "dispatch_error", summary),
    };
    if !(outcome.selected && outcome.session_epoch == Some(r.folder_epoch) && outcome.completed) {
        return revert_failed(store, ctx, r, "revert_incomplete", summary);
    }
    let target_uid = outcome
        .copyuid
        .as_ref()
        .filter(|c| c.target_epoch == r.target_epoch)
        .and_then(|c| c.pairs.iter().find(|(src, _)| *src == r.uid))
        .map(|(_, dst)| *dst);
    store.update_revert(r.id, "applied", target_uid, None, &ctx.now)?;
    close_reverted_arrival(store, ctx, r)?;
    summary.reverted += 1;
    store.record_event(
        ctx.account,
        None,
        Some(&r.target),
        "epoch_race_reverted",
        json!({"revert_id": r.id, "parent_intent": r.parent_intent, "from": r.folder}),
        &ctx.now,
    )
}

/// A completed revert closes the raced message's arrival in the destination
/// (kind `reverted`) and removes that occurrence; it changes no placement.
fn close_reverted_arrival(store: &mut Store, ctx: &PassContext, r: &Revert) -> Result<()> {
    for a in store.arrivals_at(ctx.account, &r.folder, r.folder_epoch, r.uid)? {
        let open = a.state == "pending" || a.kind.as_deref() == Some("quarantined");
        if a.uid == r.uid && open {
            store.resolve_arrival(a.id, "resolved", Some("reverted"), &ctx.now)?;
        }
    }
    store.remove_occurrence(ctx.account, &r.folder, r.folder_epoch, r.uid)
}

/// Spec "Epoch race": a failed revert pauses its target folder (the original
/// source) with event `epoch_race`.
pub(crate) fn revert_failed(
    store: &mut Store,
    ctx: &PassContext,
    r: &Revert,
    code: &str,
    summary: &mut FilingSummary,
) -> Result<()> {
    store.update_revert(r.id, "failed", None, Some(code), &ctx.now)?;
    pause(store, ctx.account, &r.target, "epoch_race")?;
    store.record_event(
        ctx.account,
        None,
        Some(&r.target),
        "epoch_race",
        json!({"revert_id": r.id, "parent_intent": r.parent_intent, "error": code}),
        &ctx.now,
    )?;
    race_problem(&r.target, summary);
    Ok(())
}
