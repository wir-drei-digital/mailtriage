//! Reply queue (`docs/superpowers/specs/2026-10-08-reply-queue-design.md`):
//! held messages stay in their source folder until the server reports
//! `\Answered` or the user marks them done, then move to their category
//! folder unread and enter the read approval list. Flags are stored once at
//! discovery, so each pass reads the flags of held messages again before
//! planning; `\Seen` is added only to messages the user approved. Each
//! `add_seen` session is recorded on its rows (folder and epoch) before it
//! runs, so an epoch race, detected or suspected, pauses the folder as a
//! flag race does.
use super::apply::{commit, event, matches_meta, paused, race_problem, write_failed};
use super::observe::FolderMap;
use super::planner::{self, PlanInput, PlanMessage};
use super::{
    inputs, is_config_changed, FilingSummary, FilingWrite, Intent, LocationState, PassContext,
};
use crate::domain::FilingMode;
use crate::store::{now, Store};
use anyhow::Result;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

/// UIDs per `envelopes` call.
const BATCH: usize = 100;

/// An approved message in its home: UID, message id.
type ReadMember = (u64, String);

/// Reads the flags of every held, not yet answered message again and stores
/// the ones that changed, in the store and in `input`. A folder whose
/// UIDVALIDITY changed meanwhile is skipped: its UIDs may name other mail.
/// So is a folder a client-side alias resolves elsewhere, and every folder
/// while the engine configuration is unreadable: neither is read.
pub fn refresh_flags(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    input: &mut PlanInput,
    summary: &mut FilingSummary,
) -> Result<()> {
    if !input.reply_queue || input.mode == FilingMode::Off || map.reads_blocked {
        return Ok(());
    }
    let mut groups: BTreeMap<(String, u64), Vec<(u64, usize)>> = BTreeMap::new();
    for (i, m) in input.messages.iter().enumerate() {
        let home = waiting_home(input, m);
        if let Some(home) = home.filter(|h| !map.alias_conflicts.contains(&h.folder)) {
            groups
                .entry((home.folder.clone(), home.epoch))
                .or_default()
                .push((home.uid, i));
        }
    }
    for ((folder, epoch), members) in groups {
        for chunk in members.chunks(BATCH) {
            match refresh_batch(store, ctx, input, (&folder, epoch), chunk) {
                Ok(n) => summary.replies_checked += n,
                Err(e) if is_config_changed(&e) => return Err(e),
                Err(_) => {
                    summary.errors += 1;
                    summary
                        .problems
                        .push(format!("reply_check_failed:{folder}"));
                }
            }
        }
    }
    Ok(())
}

/// The home of a held message that is still waiting, when the planner may
/// act on it: known, hydrated, unblocked, unpinned, in an unpaused source
/// folder in its discovery epoch.
fn waiting_home<'m>(input: &PlanInput, m: &'m PlanMessage) -> Option<&'m planner::Locator> {
    let home = m.home.as_ref()?;
    let view = input.folders.get(&home.folder)?;
    let usable = m.location_known
        && m.hydrated
        && !m.blocked
        && !m.pinned
        && m.desired_target.is_none()
        && !m.open_move_intent
        && view.is_source
        && !view.paused
        && view.epoch == Some(home.epoch);
    (usable && planner::holds(input, m) && !planner::reply_done(m)).then_some(home)
}

/// Recovery, before a move intent is retried: whether the planner would now
/// hold its message instead (queue on, no explicit request, held, and
/// neither answered nor done), as for a reply exit reopened before its
/// retry. Flags are the stored ones; a reply seen later exits again.
pub fn holds_again(
    store: &Store,
    ctx: &PassContext,
    map: &FolderMap,
    intent: &Intent,
) -> Result<bool> {
    if !ctx.cfg.filing.reply_queue {
        return Ok(false);
    }
    let input = inputs::message_input(store, ctx, map, &intent.message_id)?;
    Ok(input.messages.first().is_some_and(|m| {
        m.desired_target.is_none() && planner::holds(&input, m) && !planner::reply_done(m)
    }))
}

/// One `envelopes` call; returns how many held messages it read.
fn refresh_batch(
    store: &mut Store,
    ctx: &PassContext,
    input: &mut PlanInput,
    (folder, epoch): (&str, u64),
    members: &[(u64, usize)],
) -> Result<usize> {
    let uids: Vec<u64> = members.iter().map(|(uid, _)| *uid).collect();
    let envelopes = ctx.engine.envelopes(folder, &uids)?;
    if ctx.engine.snapshot(folder)?.uid_validity != epoch {
        return Ok(0);
    }
    let mut read = 0;
    for env in &envelopes {
        let Some((_, i)) = members.iter().find(|(uid, _)| *uid == env.uid) else {
            continue;
        };
        read += 1;
        let m = &mut input.messages[*i];
        if env.flags != m.flags {
            store.hydrate(ctx.account, &m.message_id, env)?;
            m.flags = env.flags.clone();
        }
    }
    Ok(read)
}

/// Adds `\Seen` to every approved message of the read approval list, in
/// its current home (live, writes allowed). A message moves on while
/// unknown, blocked, being moved, in a paused folder or another epoch, or
/// while an earlier session's outcome waits for `recover_reads`; it is
/// retried on a later pass. Each UID is checked to still name the
/// message (its stored Message-ID and size, as batch verification checks)
/// right before the write; a message that already carries `\Seen` needs no
/// write. It runs whatever `filing.reply_queue` says, so reads approved
/// before the queue was turned off are still applied.
pub fn apply_reads(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    summary: &mut FilingSummary,
) -> Result<()> {
    if ctx.mode != FilingMode::Live || !map.writes_allowed {
        return Ok(());
    }
    let moving: BTreeSet<String> = store
        .intents(ctx.account, true)?
        .into_iter()
        .filter(|i| i.kind == "move")
        .map(|i| i.message_id)
        .collect();
    let mut groups: BTreeMap<(String, u64), Vec<ReadMember>> = BTreeMap::new();
    for row in store.read_approvals(ctx.account, true)? {
        let unsettled = row.attempt_folder.is_some();
        if row.approved_at.is_none() || unsettled || moving.contains(&row.message_id) {
            continue;
        }
        let Some(p) = store.placement(ctx.account, &row.message_id)? else {
            continue;
        };
        let (Some(folder), Some(epoch), Some(uid)) = (&p.home_folder, p.home_epoch, p.home_uid)
        else {
            continue;
        };
        if p.location_state != LocationState::Known
            || p.blocked_reason.is_some()
            || paused(store, ctx.account, folder)?
            || store.discovery_epoch(ctx.account, folder)? != Some(epoch)
        {
            continue;
        }
        groups
            .entry((folder.clone(), epoch))
            .or_default()
            .push((uid, row.message_id));
    }
    for ((folder, epoch), members) in groups {
        for chunk in members.chunks(BATCH) {
            if let Err(e) = read_batch(store, ctx, (&folder, epoch), chunk, summary) {
                write_failed(e, "read_failed", &folder, summary)?;
            }
        }
    }
    Ok(())
}

/// One verified `add_seen` session for approved messages in `folder`. A UID
/// that shows another Message-ID or size is not written and reported as
/// `read_mismatch:<folder>` (once per folder and pass). The session's
/// folder and epoch are recorded on the rows it writes before it runs and
/// cleared once its outcome is known; a lost outcome keeps them for
/// `recover_reads`.
fn read_batch(
    store: &mut Store,
    ctx: &PassContext,
    (folder, epoch): (&str, u64),
    members: &[ReadMember],
    summary: &mut FilingSummary,
) -> Result<()> {
    (ctx.verify_binding)()?;
    let uids: Vec<u64> = members.iter().map(|(uid, _)| *uid).collect();
    let envelopes = ctx.engine.envelopes(folder, &uids)?;
    if ctx.engine.snapshot(folder)?.uid_validity != epoch {
        return Ok(());
    }
    let at = now();
    let mut write = Vec::new();
    for (uid, id) in members {
        let Some(env) = envelopes.iter().find(|e| e.uid == *uid) else {
            continue;
        };
        if !matches_meta(store, ctx.account, id, env)? {
            let code = format!("read_mismatch:{folder}");
            if !summary.problems.contains(&code) {
                summary.problems.push(code);
            }
            continue;
        }
        if env.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Seen")) {
            store.mark_read_applied(ctx.account, id, &at)?;
            summary.reads_applied += 1;
        } else {
            write.push((*uid, id.as_str()));
        }
    }
    if write.is_empty() {
        return Ok(());
    }
    let ids: Vec<&str> = write.iter().map(|(_, id)| *id).collect();
    commit(store, ctx, &attempts(&ids, Some((folder, epoch))))?;
    let uids: Vec<u64> = write.iter().map(|(uid, _)| *uid).collect();
    let outcome = match ctx.engine.add_seen(folder, &uids) {
        Ok(o) => o,
        // The engine refused before running anything.
        Err(e) if is_config_changed(&e) => {
            commit(store, ctx, &attempts(&ids, None))?;
            return Err(e);
        }
        Err(e) => return Err(e),
    };
    if outcome.selected && outcome.session_epoch != Some(epoch) {
        return seen_raced(store, ctx, (folder, epoch), &ids, "epoch_race", summary);
    }
    if !(outcome.selected && outcome.completed) {
        // Nothing ran, or it ran in the verified epoch: retried later.
        commit(store, ctx, &attempts(&ids, None))?;
        summary.errors += 1;
        summary.problems.push(format!("read_incomplete:{folder}"));
        return Ok(());
    }
    for id in ids {
        store.mark_read_applied(ctx.account, id, &at)?;
        summary.reads_applied += 1;
    }
    Ok(())
}

/// The `\Seen` attempt of each of `ids`: set to the session's folder and
/// epoch, or cleared.
fn attempts<'a>(ids: &[&'a str], attempt: Option<(&'a str, u64)>) -> Vec<FilingWrite<'a>> {
    ids.iter()
        .map(|id| FilingWrite::ReadAttempt {
            message_id: id,
            attempt,
        })
        .collect()
}

/// Filing spec "Epoch race", applied to `\Seen` as to a flag (detected or
/// suspected): the folder pauses with event `epoch_race` (kind `seen`) per
/// message and the attempts are cleared, in one transaction. Another
/// message may now carry `\Seen`; it is never removed. The rows stay
/// approved, for the message's home once the folder is released.
fn seen_raced(
    store: &mut Store,
    ctx: &PassContext,
    (folder, epoch): (&str, u64),
    ids: &[&str],
    error: &str,
    summary: &mut FilingSummary,
) -> Result<()> {
    let mut writes = vec![FilingWrite::Pause {
        folder,
        reason: "epoch_race",
    }];
    writes.extend(attempts(ids, None));
    for id in ids {
        let detail = json!({"kind": "seen", "error": error, "epoch": epoch});
        writes.push(event(Some(id), Some(folder), "epoch_race", detail));
    }
    commit(store, ctx, &writes)?;
    race_problem(folder, summary);
    Ok(())
}

/// Recovery (step 5) for `\Seen` sessions whose outcome was lost (an error
/// or a crash): a folder no longer in the attempt's epoch is a suspected
/// race (`seen_raced`); otherwise the session ran in the verified epoch, the
/// attempt is cleared and `apply_reads` takes the row up again (adding
/// `\Seen` twice is harmless). Each folder is read once per call.
pub fn recover_reads(
    store: &mut Store,
    ctx: &PassContext,
    summary: &mut FilingSummary,
) -> Result<()> {
    let mut open: BTreeMap<(String, u64), Vec<String>> = BTreeMap::new();
    for row in store.read_approvals(ctx.account, true)? {
        if let (Some(folder), Some(epoch)) = (row.attempt_folder, row.attempt_epoch) {
            open.entry((folder, epoch))
                .or_default()
                .push(row.message_id);
        }
    }
    if open.is_empty() {
        return Ok(());
    }
    (ctx.verify_binding)()?;
    let mut epochs: BTreeMap<String, u64> = BTreeMap::new();
    for ((folder, epoch), ids) in open {
        let now_epoch = match epochs.get(&folder) {
            Some(e) => *e,
            None => match ctx.engine.snapshot(&folder) {
                Ok(s) => *epochs.entry(folder.clone()).or_insert(s.uid_validity),
                Err(e) => {
                    write_failed(e, "recovery_failed", &folder, summary)?;
                    continue;
                }
            },
        };
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        if now_epoch == epoch {
            commit(store, ctx, &attempts(&ids, None))?;
        } else {
            let race = (folder.as_str(), epoch);
            seen_raced(store, ctx, race, &ids, "epoch_race_suspected", summary)?;
        }
    }
    Ok(())
}
