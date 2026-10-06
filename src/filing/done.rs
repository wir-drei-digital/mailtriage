//! Done inference (spec "Done inference", step 10 of a pass): a message that
//! left every watched folder is marked done once nothing could still hide
//! it. An explicit review state is never touched.
use super::observe::FolderMap;
use super::{FilingSummary, FilingWrite, PassContext, Placement};
use crate::store::Store;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde_json::json;
use std::collections::BTreeSet;

fn time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// Marks done every `absent` placement of an `open` message whose absence
/// was recorded in an earlier pass (`absent_since` strictly before this pass's `now`), that has
/// no occurrence anywhere (frozen ones in retired, paused or missing folders
/// count as present) and no open intent, while the account is settled.
pub fn infer_done(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    summary: &mut FilingSummary,
) -> Result<()> {
    if map.caps.is_none() {
        return Ok(());
    }
    let Some(now) = time(&ctx.now) else {
        return Ok(());
    };
    let earlier = |p: &Placement| {
        p.absent_since
            .as_deref()
            .and_then(time)
            .is_some_and(|t| t < now)
    };
    let candidates: Vec<Placement> = store
        .open_absent_placements(ctx.account)?
        .into_iter()
        .filter(earlier)
        .collect();
    if candidates.is_empty() || !settled(store, ctx, map)? {
        return Ok(());
    }
    let open: BTreeSet<String> = store
        .intents(ctx.account, true)?
        .into_iter()
        .map(|i| i.message_id)
        .collect();
    for p in candidates {
        let id = p.message_id.as_str();
        if open.contains(id) || !store.occurrences_of(ctx.account, id)?.is_empty() {
            continue;
        }
        // Only an `open` message changes; an explicit Done keeps done_inferred = 0.
        if store.infer_done_row(ctx.account, id)? {
            let detail = json!({"absent_since": p.absent_since});
            store.record_event(
                ctx.account,
                Some(id),
                None,
                "archived_done",
                detail,
                &ctx.now,
            )?;
            summary.archived_done += 1;
        }
    }
    Ok(())
}

/// Each pass: a known placement whose Done was inferred but which is still
/// done (its reopen was interrupted) is reopened, event `reopened`.
pub fn reopen_reappeared(store: &mut Store, ctx: &PassContext) -> Result<()> {
    for id in store.known_but_inferred_done(ctx.account)? {
        let reopen = FilingWrite::ReopenInferred {
            message_id: &id,
            generation: ctx.generation,
        };
        store.commit_filing(ctx.account, &[reopen], &ctx.now)?;
    }
    Ok(())
}

/// Every watched folder's checkpoint complete with no rescan in progress (a
/// reset rescan looks for every absent message, so a complete one in the
/// current epoch would have found it), no arrival that could hide an
/// unidentified occurrence (pending or unresolved; quarantined ones have
/// established identity), and no folder paused.
fn settled(store: &Store, ctx: &PassContext, map: &FolderMap) -> Result<bool> {
    let folders = store.folder_records(ctx.account)?;
    if folders.iter().any(|r| r.pause_reason.is_some()) {
        return Ok(false);
    }
    let drains = store.drain_states(ctx.account)?;
    for w in &map.watch {
        let Some((epoch, _, complete, _)) = store.checkpoint_state(ctx.account, &w.folder)? else {
            return Ok(false);
        };
        let rescanning = folders
            .iter()
            .any(|r| r.native == w.folder && r.rescan_epoch == Some(epoch) && !r.rescan_complete);
        if !complete || rescanning {
            return Ok(false);
        }
        // Refile spec "Draining": a draining folder counts as still being scanned.
        let draining = drains.get(&w.folder).is_some_and(|until| *until > 0);
        if draining && !store.drain_finished(ctx.account, &w.folder)? {
            return Ok(false);
        }
    }
    Ok(!store.arrivals_unsettled(ctx.account)?)
}
