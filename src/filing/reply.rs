//! Reply queue (`docs/superpowers/specs/2026-10-08-reply-queue-design.md`):
//! held messages stay in their source folder until the server reports
//! `\Answered` or the user marks them done, then move to their category
//! folder unread and enter the read approval list. Flags are stored once at
//! discovery, so each pass reads the flags of held messages again before
//! planning; `\Seen` is added only to messages the user approved.
use super::apply::{paused, write_failed};
use super::observe::FolderMap;
use super::planner::{self, PlanInput, PlanMessage};
use super::{is_config_changed, rfc_message_id, FilingSummary, LocationState, PassContext};
use crate::domain::FilingMode;
use crate::store::{now, Store};
use anyhow::Result;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// UIDs per `envelopes` call.
const BATCH: usize = 100;

/// An approved message in its home: UID, message id, stored Message-ID.
type ReadMember = (u64, String, Option<String>);

/// Reads the flags of every held, not yet answered message again and stores
/// the ones that changed, in the store and in `input`. A folder whose
/// UIDVALIDITY changed meanwhile is skipped: its UIDs may name other mail.
pub fn refresh_flags(
    store: &mut Store,
    ctx: &PassContext,
    input: &mut PlanInput,
    summary: &mut FilingSummary,
) -> Result<()> {
    if !input.reply_queue || input.mode == FilingMode::Off {
        return Ok(());
    }
    let mut groups: BTreeMap<(String, u64), Vec<(u64, usize)>> = BTreeMap::new();
    for (i, m) in input.messages.iter().enumerate() {
        if let Some(home) = waiting_home(input, m) {
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
/// unknown, blocked, being moved or in a paused folder or another epoch; it
/// is retried on a later pass. Each UID is checked to still name the
/// message (its Message-ID) right before the write; a message that already
/// carries `\Seen` needs no write.
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
        if row.approved_at.is_none() || moving.contains(&row.message_id) {
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
        let rfc = store.record(ctx.account, &row.message_id)?.and_then(|r| {
            r.envelope
                .get("message_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
        groups
            .entry((folder.clone(), epoch))
            .or_default()
            .push((uid, row.message_id, rfc));
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

/// One verified `add_seen` session for approved messages in `folder`.
fn read_batch(
    store: &mut Store,
    ctx: &PassContext,
    (folder, epoch): (&str, u64),
    members: &[ReadMember],
    summary: &mut FilingSummary,
) -> Result<()> {
    (ctx.verify_binding)()?;
    let uids: Vec<u64> = members.iter().map(|(uid, _, _)| *uid).collect();
    let envelopes = ctx.engine.envelopes(folder, &uids)?;
    if ctx.engine.snapshot(folder)?.uid_validity != epoch {
        return Ok(());
    }
    let at = now();
    let mut write = Vec::new();
    for (uid, id, rfc) in members {
        let Some(env) = envelopes.iter().find(|e| e.uid == *uid) else {
            continue;
        };
        if rfc.is_some() && rfc_message_id(env) != *rfc {
            continue;
        }
        if env.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Seen")) {
            store.mark_read_applied(ctx.account, id, &at)?;
            summary.reads_applied += 1;
        } else {
            write.push((*uid, id));
        }
    }
    if write.is_empty() {
        return Ok(());
    }
    let uids: Vec<u64> = write.iter().map(|(uid, _)| *uid).collect();
    let outcome = ctx.engine.add_seen(folder, &uids)?;
    if !(outcome.selected && outcome.completed && outcome.session_epoch == Some(epoch)) {
        summary.errors += 1;
        summary.problems.push(format!("read_incomplete:{folder}"));
        return Ok(());
    }
    for (_, id) in write {
        store.mark_read_applied(ctx.account, id, &at)?;
        summary.reads_applied += 1;
    }
    Ok(())
}
