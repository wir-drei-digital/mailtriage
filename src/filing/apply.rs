//! Claiming and applying planned flags and moves (spec "Writes and
//! recovery"): session-bound batches, outcome mapping, epoch races and their
//! reverts. Every write is journaled as an intent (or revert) before the
//! engine call.
use super::observe::FolderMap;
use super::planner::{Action, Locator, Plan};
use super::refile;
use super::{
    is_config_changed, rfc_message_id, FilingSummary, FilingWrite, Intent, IntentPatch,
    PassContext, Placement, Revert,
};
use crate::domain::{FilingMode, SourceEnvelope};
use crate::engine::WriteOutcome;
use crate::store::{now, Store};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use uuid::Uuid;

/// UIDs per write call (spec "Himalaya command mapping").
const BATCH: usize = 100;

/// Step 9 in `live`: clears satisfied explicit requests, consumes the flag
/// attempt of already-flagged messages, then claims and applies flags (per
/// folder and epoch) and moves (per folder, epoch, target, and whether they
/// are reply exits, whose claims enter the read approval list) in batches of
/// at most 100. Nothing happens unless writes are allowed. Returns the
/// messages whose UID batch verification dropped (absent, or another
/// Message-ID or size), for placement re-evaluation.
pub fn apply(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    plan: &Plan,
    summary: &mut FilingSummary,
) -> Result<Vec<String>> {
    let mut dropped = Vec::new();
    if ctx.mode != FilingMode::Live || !map.writes_allowed {
        return Ok(dropped);
    }
    clear_requests(store, ctx, plan)?;
    for id in &plan.satisfied_flags {
        consume_flag_attempt(store, ctx, id)?;
    }
    let flags = group(plan, |a| match a {
        Action::Flag { at, .. } => Some((at.folder.clone(), at.epoch, String::new(), false)),
        Action::Move { .. } => None,
    });
    for ((folder, epoch, _, _), actions) in &flags {
        for chunk in actions.chunks(BATCH) {
            let batch = flag_batch(store, ctx, (folder, *epoch), chunk, &mut dropped, summary);
            if let Err(e) = batch {
                write_failed(e, "flag_batch_failed", folder, summary)?;
            }
        }
    }
    let moves = group(plan, |a| match a {
        Action::Move {
            message_id,
            from,
            to,
            ..
        } => Some((
            from.folder.clone(),
            from.epoch,
            to.clone(),
            plan.reply_exits.contains(message_id),
        )),
        Action::Flag { .. } => None,
    });
    for ((folder, epoch, to, reply_exit), actions) in &moves {
        for chunk in actions.chunks(BATCH) {
            let batch = move_batch(
                store,
                ctx,
                map,
                (folder, *epoch, to, *reply_exit),
                chunk,
                &plan.refile_moves,
                &mut dropped,
                summary,
            );
            if let Err(e) = batch {
                write_failed(e, "move_batch_failed", folder, summary)?;
            }
        }
    }
    Ok(dropped)
}

/// Folder, epoch, move target ("" for flags), reply exit.
type BatchKey = (String, u64, String, bool);

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
        let Some(read) = store.placement(ctx.account, id)? else {
            continue;
        };
        if read.desired_rev != *rev {
            continue;
        }
        let mut p = read.clone();
        p.desired_target = None;
        p.desired_rev += 1;
        let write = FilingWrite::PlacementFrom {
            placement: &p,
            read: &read,
        };
        store.commit_filing(ctx.account, &[write], &ctx.now)?;
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

/// Commits writes that change no placement.
pub(crate) fn commit(
    store: &mut Store,
    ctx: &PassContext,
    writes: &[FilingWrite<'_>],
) -> Result<()> {
    if !store.commit_filing(ctx.account, writes, &ctx.now)? {
        bail!("filing commit refused");
    }
    Ok(())
}

/// Re-reads the message's placement, lets `f` change it and commits it
/// together with `extra` in one transaction while its `desired_rev` (and,
/// when `f` changed it, its `blocked_reason`) is unchanged, re-reading after
/// a concurrent change. `f` may bump `desired_rev` itself.
pub(crate) fn commit_with_placement(
    store: &mut Store,
    ctx: &PassContext,
    message_id: &str,
    mut f: impl FnMut(&mut Placement),
    extra: &[FilingWrite<'_>],
) -> Result<()> {
    for _ in 0..5 {
        let Some(read) = store.placement(ctx.account, message_id)? else {
            bail!("message has no placement");
        };
        let mut p = read.clone();
        f(&mut p);
        let mut writes = vec![FilingWrite::PlacementFrom {
            placement: &p,
            read: &read,
        }];
        writes.extend(extra.iter().cloned());
        if store.commit_filing(ctx.account, &writes, &ctx.now)? {
            return Ok(());
        }
    }
    bail!("placement kept changing concurrently")
}

pub(crate) fn event<'a>(
    message_id: Option<&'a str>,
    folder: Option<&'a str>,
    kind: &'a str,
    detail: Value,
) -> FilingWrite<'a> {
    FilingWrite::Event {
        message_id,
        folder,
        kind,
        detail,
    }
}

pub(crate) fn close<'a>(id: i64, state: &'a str, error: Option<&str>) -> FilingWrite<'a> {
    FilingWrite::Intent {
        id,
        state,
        patch: IntentPatch {
            error: error.map(str::to_string),
            ..Default::default()
        },
    }
}

/// Spec "Flags" step 1: a message that already carries `\Flagged` consumes its
/// one flag attempt without an engine call, so a later unflag is never
/// overridden.
fn consume_flag_attempt(store: &mut Store, ctx: &PassContext, message_id: &str) -> Result<()> {
    commit_with_placement(
        store,
        ctx,
        message_id,
        |p| {
            p.flag_attempted_at.get_or_insert_with(now);
        },
        &[],
    )
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

/// What batch verification keeps of a batch.
struct Verified<'a> {
    /// Present with the stored Message-ID and size (and, for flags, unflagged).
    kept: Vec<&'a Action>,
    /// For flags: present but already `\Flagged`.
    flagged: Vec<&'a Action>,
}

/// Spec "Moves" step 1 and "Flags" step 1: re-verifies the binding and re-reads
/// the batch with `envelopes()` bracketed by `snapshot()`. `None` aborts the
/// batch (the folder is not in `epoch`). Messages whose UID is absent or shows
/// another Message-ID or size are appended to `dropped`.
fn verify_batch<'a>(
    store: &Store,
    ctx: &PassContext,
    (folder, epoch): (&str, u64),
    actions: &[&'a Action],
    dropped: &mut Vec<String>,
) -> Result<Option<Verified<'a>>> {
    (ctx.verify_binding)()?;
    let uids: Vec<u64> = actions.iter().map(|a| locator(a).uid).collect();
    let before = ctx.engine.snapshot(folder)?;
    let envelopes = ctx.engine.envelopes(folder, &uids)?;
    let after = ctx.engine.snapshot(folder)?;
    if before.uid_validity != epoch || after.uid_validity != epoch {
        return Ok(None);
    }
    let mut out = Verified {
        kept: Vec::new(),
        flagged: Vec::new(),
    };
    for action in actions {
        let env = envelopes.iter().find(|e| e.uid == locator(action).uid);
        let env = match env {
            Some(e) if matches_meta(store, ctx.account, action.message_id(), e)? => e,
            _ => {
                dropped.push(action.message_id().to_string());
                continue;
            }
        };
        if matches!(action, Action::Flag { .. }) && has_flagged(&env.flags) {
            out.flagged.push(*action);
        } else {
            out.kept.push(*action);
        }
    }
    Ok(Some(out))
}

fn new_batch() -> String {
    Uuid::new_v4().simple().to_string()
}

/// One flag batch: verify, claim (consuming the only flag attempt), one
/// `add_flagged` session, then map the outcome (spec "Flags" step 3).
fn flag_batch(
    store: &mut Store,
    ctx: &PassContext,
    (folder, epoch): (&str, u64),
    actions: &[&Action],
    dropped: &mut Vec<String>,
    summary: &mut FilingSummary,
) -> Result<()> {
    if paused(store, ctx.account, folder)? {
        return Ok(());
    }
    let Some(verified) = verify_batch(store, ctx, (folder, epoch), actions, dropped)? else {
        return Ok(());
    };
    for action in verified.flagged {
        consume_flag_attempt(store, ctx, action.message_id())?;
    }
    let (batch, at) = (new_batch(), now());
    let mut claimed = Vec::new();
    for action in verified.kept {
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

/// The flag is on the message: placement `flagged_at`, intent `applied` and
/// event `flagged`, in one transaction.
pub(crate) fn flag_applied(
    store: &mut Store,
    ctx: &PassContext,
    id: i64,
    message_id: &str,
    folder: &str,
) -> Result<()> {
    let detail = json!({"intent_id": id});
    commit_with_placement(
        store,
        ctx,
        message_id,
        |p| p.flagged_at = Some(ctx.now.clone()),
        &[
            close(id, "applied", None),
            event(Some(message_id), Some(folder), "flagged", detail),
        ],
    )
}

/// Spec "Epoch race", flag race (detected or suspected): the folder pauses
/// and the flag intent fails, in one transaction; flags are never removed.
pub(crate) fn flag_raced(
    store: &mut Store,
    ctx: &PassContext,
    id: i64,
    message_id: &str,
    folder: &str,
    error: &str,
    summary: &mut FilingSummary,
) -> Result<()> {
    let detail = json!({"intent_id": id, "kind": "flag", "error": error});
    commit(
        store,
        ctx,
        &[
            FilingWrite::Pause {
                folder,
                reason: "epoch_race",
            },
            close(id, "failed", Some(error)),
            event(Some(message_id), Some(folder), "epoch_race", detail),
        ],
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

/// One move batch: verify, snapshot the target, claim (a refile move as
/// one, checked again right after its claim), dispatch. A claimed reply exit
/// enters the read approval list (reply queue spec).
#[allow(clippy::too_many_arguments)] // One move batch with its pass context.
fn move_batch(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    (folder, epoch, to, reply_exit): (&str, u64, &str, bool),
    actions: &[&Action],
    refile: &BTreeSet<String>,
    dropped: &mut Vec<String>,
    summary: &mut FilingSummary,
) -> Result<()> {
    if paused(store, ctx.account, folder)? || paused(store, ctx.account, to)? {
        return Ok(());
    }
    let Some(verified) = verify_batch(store, ctx, (folder, epoch), actions, dropped)? else {
        return Ok(());
    };
    if verified.kept.is_empty() {
        return Ok(());
    }
    let target = ctx.engine.snapshot(to)?;
    if !target_watched(store, ctx, to, target.uid_validity)? {
        return Ok(());
    }
    let (batch, at) = (new_batch(), now());
    let mut claimed = Vec::new();
    for action in verified.kept {
        let consumes_refile = refile.contains(action.message_id());
        let target_snapshot = (target.uid_validity, target.uid_next);
        let claim = store.claim_move_with(
            ctx.account,
            action,
            target_snapshot,
            &batch,
            &at,
            consumes_refile,
        )?;
        let Some(id) = claim else {
            continue;
        };
        if consumes_refile && refile_cancelled(store, ctx, map, id, locator(action))? {
            continue;
        }
        if reply_exit {
            store.request_read_approval(ctx.account, action.message_id(), &at)?;
        }
        claimed.push((id, locator(action).uid));
    }
    if claimed.is_empty() {
        return Ok(());
    }
    dispatch_moves(store, ctx, folder, epoch, to, &claimed, summary)?;
    if reply_exit {
        summary.reply_exits += claimed.len();
    }
    Ok(())
}

/// Refile spec "Intents", at claim time: a refile intent that fails its
/// check is superseded before dispatch.
fn refile_cancelled(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    id: i64,
    from: &Locator,
) -> Result<bool> {
    let Some(intent) = store.intent(id)? else {
        return Ok(false);
    };
    match refile::intents::recheck(store, ctx, map, &intent, from)? {
        Some(reason) => {
            refile::intents::cancel(store, ctx, &intent, reason)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Whether a move into `target` may be claimed: its discovery checkpoint is
/// established in `epoch`, the epoch the target is in now, so the move's
/// arrival (at or above the UIDNEXT just observed) lies inside the watched
/// range and is discovered.
pub(crate) fn target_watched(
    store: &Store,
    ctx: &PassContext,
    target: &str,
    epoch: u64,
) -> Result<bool> {
    Ok(store.discovery_epoch(ctx.account, target)? == Some(epoch))
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
        let (state, removed) = if outcome.completed {
            ("sent", Some((folder, epoch, *uid)))
        } else {
            ("uncertain", None)
        };
        let mut writes: Vec<FilingWrite<'_>> = removed
            .map(|(folder, epoch, uid)| FilingWrite::RemoveOccurrence { folder, epoch, uid })
            .into_iter()
            .collect();
        writes.push(FilingWrite::Intent {
            id: *id,
            state,
            patch,
        });
        commit(store, ctx, &writes)?;
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
/// COPYUID every moved message is journaled for a revert, in the same
/// transaction that sets the intents `awaiting_rescan`, and the reverts are
/// dispatched; without it the source folder pauses in that transaction
/// (`epoch_race_suspected` when the session reported no epoch).
fn move_raced(
    store: &mut Store,
    ctx: &PassContext,
    folder: &str,
    target: &str,
    claimed: &[(i64, u64)],
    outcome: &WriteOutcome,
    summary: &mut FilingSummary,
) -> Result<()> {
    // The quarantine window closes at T's UIDNEXT right after the race; a
    // failed snapshot leaves it to the next recovery observation.
    let race_until = race_bound(ctx, target, claimed, store)?;
    let raced = |id: i64, error: &'static str| FilingWrite::Intent {
        id,
        state: "awaiting_rescan",
        patch: IntentPatch {
            error: Some(error.into()),
            race_until_uid: race_until,
            ..Default::default()
        },
    };
    let copyuid = outcome.copyuid.as_ref().filter(|c| !c.pairs.is_empty());
    if let (Some(copyuid), Some(session)) = (copyuid, outcome.session_epoch) {
        let at = now();
        let reverts: Vec<Revert> = copyuid
            .pairs
            .iter()
            .map(|(_, dst)| Revert {
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
            })
            .collect();
        let mut writes: Vec<FilingWrite<'_>> = reverts.iter().map(FilingWrite::NewRevert).collect();
        writes.extend(claimed.iter().map(|(id, _)| raced(*id, "epoch_race")));
        commit(store, ctx, &writes)?;
        return dispatch_reverts(store, ctx, summary);
    }
    let reason = if outcome.session_epoch.is_some() {
        "epoch_race"
    } else {
        "epoch_race_suspected"
    };
    let mut messages = Vec::new();
    for (id, _) in claimed {
        if let Some(intent) = store.intent(*id)? {
            messages.push((*id, intent.message_id));
        }
    }
    let mut writes = vec![FilingWrite::Pause { folder, reason }];
    for (id, message_id) in &messages {
        writes.push(raced(*id, reason));
        let detail = json!({"intent_id": id, "kind": "move", "error": reason});
        writes.push(event(Some(message_id), Some(folder), "epoch_race", detail));
    }
    commit(store, ctx, &writes)?;
    race_problem(folder, summary);
    Ok(())
}

/// T's `UIDNEXT` right after a race, when T is still in the claimed
/// intents' target epoch (an epoch change means nothing more can arrive in
/// it); `None` when the snapshot fails.
fn race_bound(
    ctx: &PassContext,
    target: &str,
    claimed: &[(i64, u64)],
    store: &Store,
) -> Result<Option<u64>> {
    let Some(intent) = claimed
        .first()
        .map(|c| store.intent(c.0))
        .transpose()?
        .flatten()
    else {
        return Ok(None);
    };
    Ok(race_until_uid(ctx, target, &intent))
}

/// T's `UIDNEXT` now for a raced move intent: the end of its quarantine
/// window. When T left `target_epoch`, nothing more can arrive in that
/// epoch, so the window ends at `target_uid_next`. `None` when the
/// snapshot fails (the window stays open-ended until it is recorded).
pub(crate) fn race_until_uid(ctx: &PassContext, target: &str, intent: &Intent) -> Option<u64> {
    let snapshot = ctx.engine.snapshot(target).ok()?;
    if Some(snapshot.uid_validity) == intent.target_epoch {
        Some(snapshot.uid_next)
    } else {
        Some(intent.target_uid_next.unwrap_or(0))
    }
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
    revert_applied(store, ctx, r, target_uid)?;
    summary.reverted += 1;
    Ok(())
}

/// A completed revert, in one transaction: the revert `applied` with its own
/// COPYUID target UID, the raced message's arrival in the destination closed
/// (kind `reverted`) and that occurrence removed, event `epoch_race_reverted`.
/// It changes no placement.
fn revert_applied(
    store: &mut Store,
    ctx: &PassContext,
    r: &Revert,
    target_uid: Option<u64>,
) -> Result<()> {
    let arrivals: Vec<i64> = store
        .arrivals_at(ctx.account, &r.folder, r.folder_epoch, r.uid)?
        .into_iter()
        .filter(|a| a.uid == r.uid)
        .filter(|a| a.state == "pending" || a.kind.as_deref() == Some("quarantined"))
        .map(|a| a.id)
        .collect();
    let mut writes = vec![FilingWrite::Revert {
        id: r.id,
        state: "applied",
        target_uid,
        error: None,
    }];
    writes.extend(arrivals.iter().map(|id| FilingWrite::Arrival {
        id: *id,
        state: "resolved",
        kind: Some("reverted"),
    }));
    writes.push(FilingWrite::RemoveOccurrence {
        folder: &r.folder,
        epoch: r.folder_epoch,
        uid: r.uid,
    });
    let detail = json!({"revert_id": r.id, "parent_intent": r.parent_intent, "from": r.folder});
    writes.push(event(None, Some(&r.target), "epoch_race_reverted", detail));
    commit(store, ctx, &writes)
}

/// Spec "Epoch race": a failed revert pauses its target folder (the original
/// source) with event `epoch_race`, in the transaction that fails it.
pub(crate) fn revert_failed(
    store: &mut Store,
    ctx: &PassContext,
    r: &Revert,
    code: &str,
    summary: &mut FilingSummary,
) -> Result<()> {
    let detail = json!({"revert_id": r.id, "parent_intent": r.parent_intent, "error": code});
    commit(
        store,
        ctx,
        &[
            FilingWrite::Pause {
                folder: &r.target,
                reason: "epoch_race",
            },
            FilingWrite::Revert {
                id: r.id,
                state: "failed",
                target_uid: None,
                error: Some(code),
            },
            event(None, Some(&r.target), "epoch_race", detail),
        ],
    )?;
    race_problem(&r.target, summary);
    Ok(())
}
