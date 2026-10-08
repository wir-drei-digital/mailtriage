//! Reply queue (`docs/superpowers/specs/2026-10-08-reply-queue-design.md`):
//! held messages stay in their source folder until the server reports
//! `\Answered` or the user marks them done. Flags are stored once at
//! discovery, so each pass reads the flags of held messages again before
//! planning.
use super::planner::{self, PlanInput, PlanMessage};
use super::{is_config_changed, FilingSummary, PassContext};
use crate::domain::FilingMode;
use crate::store::Store;
use anyhow::Result;
use std::collections::BTreeMap;

/// UIDs per `envelopes` call.
const BATCH: usize = 100;

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
