//! Explicit placement transitions (spec "Placement transitions", "Resolving
//! ambiguity") and the arrival and folder commands that lift a safety state.
//! Every placement change is one transaction guarded by `desired_rev`.
use super::{FilingWrite, LocationState, Placement};
use crate::domain::AccountConfig;
use crate::service::err;
use crate::store::Store;
use anyhow::Result;
use serde_json::{json, Value};

const IDENTITY_PENDING: &str = "identity not yet established; retry after sync";
const NO_PLACEMENT: &str = "message has no mailbox placement";
/// Blocks `filing retry --id` lifts.
const RETRYABLE: [&str; 3] = ["move_failed", "duplicate_copy", "quarantined"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transition {
    /// `correct` set or cleared `category_id`; the new effective category.
    CategoryChanged {
        effective_category: Option<String>,
    },
    Pin,
    Unpin,
    Retry,
}

/// Applies an explicit transition (spec "Placement transitions"), resolving an ambiguous location first.
pub fn apply_transition(
    store: &mut Store,
    account: &str,
    message_id: &str,
    t: Transition,
    sources: &[String],
    now: &str,
) -> Result<Placement> {
    for _ in 0..5 {
        let (old, new) = plan_transition(store, account, message_id, &t, sources)?;
        let mut writes = vec![FilingWrite::Placement {
            placement: &new,
            expected_rev: old.desired_rev,
        }];
        if let Some((kind, detail)) = transition_event(&t, &old) {
            writes.push(FilingWrite::Event {
                message_id: Some(message_id),
                folder: new.home_folder.as_deref(),
                kind,
                detail,
            });
        }
        if store.commit_filing(account, &writes, now)? {
            return Ok(new);
        }
    }
    Err(err(5, "placement changed concurrently; retry"))
}

/// The stored placement and the placement after `t`, with `desired_rev`
/// bumped by one. Fails with exit 5 for a message whose identity is not
/// established and exit 2 for one without a placement.
pub(crate) fn plan_transition(
    store: &Store,
    account: &str,
    message_id: &str,
    t: &Transition,
    sources: &[String],
) -> Result<(Placement, Placement)> {
    let meta = store
        .message_meta(account, message_id)?
        .ok_or_else(|| err(2, "message not found in account"))?;
    if !meta.fingerprinted {
        return Err(err(5, IDENTITY_PENDING));
    }
    let old = store
        .placement(account, message_id)?
        .ok_or_else(|| err(2, NO_PLACEMENT))?;
    let mut p = old.clone();
    if p.location_state == LocationState::Ambiguous {
        let desired = desired_native(store, account, &p, t)?;
        select_home(store, account, &mut p, desired.as_deref(), sources)?;
    }
    match t {
        Transition::CategoryChanged { effective_category } => {
            p.desired_target = effective_category.clone();
            p.pinned = false;
            if p.blocked_reason.as_deref() == Some("move_failed") {
                p.blocked_reason = None;
            }
        }
        Transition::Pin => {
            p.pinned = true;
            let in_source = p.location_state == LocationState::Known
                && p.home_folder.as_ref().is_some_and(|h| sources.contains(h));
            let settled = in_source && !open_move(store, account, message_id)?;
            p.desired_target = (!settled).then(|| "@source".to_string());
        }
        Transition::Unpin => {
            p.pinned = false;
            p.desired_target = None;
            p.eligible_once = true;
        }
        Transition::Retry => {
            if p.blocked_reason
                .as_deref()
                .is_some_and(|b| RETRYABLE.contains(&b))
            {
                p.blocked_reason = None;
            }
            p.eligible_once = true;
        }
    }
    p.desired_rev += 1;
    Ok((old, p))
}

fn open_move(store: &Store, account: &str, message_id: &str) -> Result<bool> {
    Ok(store
        .intents(account, true)?
        .iter()
        .any(|i| i.message_id == message_id && i.kind == "move"))
}

fn transition_event(t: &Transition, old: &Placement) -> Option<(&'static str, Value)> {
    match t {
        Transition::CategoryChanged { .. } => None,
        Transition::Pin => Some(("pinned", json!({"via": "command"}))),
        Transition::Unpin => Some(("unpinned", json!({"via": "command"}))),
        Transition::Retry => Some(("released", json!({"cleared": old.blocked_reason}))),
    }
}

/// The folder a transition newly desires, for resolving ambiguity: the
/// category's folder (its recorded native name; the source folder for an
/// `INBOX` target or a category without a folder record) or the source
/// folder for a pin.
fn desired_native(
    store: &Store,
    account: &str,
    p: &Placement,
    t: &Transition,
) -> Result<Option<String>> {
    Ok(match t {
        Transition::CategoryChanged {
            effective_category: Some(c),
        } => Some(category_native(store, account, c)?.unwrap_or_else(|| p.source_folder.clone())),
        Transition::Pin => Some(p.source_folder.clone()),
        _ => None,
    })
}

/// The recorded native folder of a category (not retired), if any.
pub(crate) fn category_native(
    store: &Store,
    account: &str,
    category: &str,
) -> Result<Option<String>> {
    Ok(store
        .folder_records(account)?
        .into_iter()
        .find(|r| r.category_id.as_deref() == Some(category) && r.state != "retired")
        .map(|r| r.native))
}

/// Spec "Resolving ambiguity": the occurrence in the newly desired folder,
/// else one in a source folder (configuration order, lowest UID), else the
/// lowest (folder, UID). The placement becomes `known`; other occurrences
/// remain extras. Without any occurrence nothing changes.
fn select_home(
    store: &Store,
    account: &str,
    p: &mut Placement,
    desired: Option<&str>,
    sources: &[String],
) -> Result<()> {
    // Ordered by (folder, UID).
    let occurrences = store.occurrences_of(account, &p.message_id)?;
    let rank = |f: &str| sources.iter().position(|s| s == f).unwrap_or(usize::MAX);
    let chosen = occurrences
        .iter()
        .find(|(f, _, _)| Some(f.as_str()) == desired)
        .or_else(|| {
            occurrences
                .iter()
                .filter(|(f, _, _)| rank(f) != usize::MAX)
                .min_by_key(|(f, _, u)| (rank(f), *u))
        })
        .or_else(|| occurrences.first());
    if let Some((folder, epoch, uid)) = chosen {
        p.home_folder = Some(folder.clone());
        p.home_epoch = Some(*epoch);
        p.home_uid = Some(*uid);
        p.location_state = LocationState::Known;
        p.absent_since = None;
    }
    Ok(())
}

/// The folder a placement's `desired_target` resolves to: the source folder
/// for `@source` and for a category filed to `INBOX`, else the category's
/// recorded native folder (its configured folder before it was ever
/// resolved). `None` for an unknown category.
pub(crate) fn desired_folder(
    store: &Store,
    cfg: &AccountConfig,
    p: &Placement,
    target: &str,
) -> Result<Option<String>> {
    if target == "@source" {
        return Ok(Some(p.source_folder.clone()));
    }
    let Some(category) = cfg.categories.iter().find(|c| c.id == target) else {
        return Ok(None);
    };
    if category.effective_folder() == "INBOX" {
        return Ok(Some(p.source_folder.clone()));
    }
    Ok(Some(
        category_native(store, &p.account, target)?
            .unwrap_or_else(|| category.effective_folder().to_string()),
    ))
}

/// `filing retry --folder`: clears the folder's safety pause (event
/// `released`) and schedules a rescan of it. Idempotent.
pub fn release_folder(store: &mut Store, account: &str, native: &str, now: &str) -> Result<()> {
    if store.folder_record(account, native)?.is_none() {
        return Err(err(2, "unknown folder; run sync first"));
    }
    store.release_pause(account, native, now)?;
    Ok(())
}

/// `filing retry --arrival`: requeues an unresolved arrival's fetch and lifts
/// the merge-conflict block it caused (a repeated conflict blocks again).
pub fn retry_arrival(
    store: &mut Store,
    account: &str,
    arrival_id: i64,
    generation: &str,
    now: &str,
) -> Result<()> {
    if !store.reopen_arrival(account, arrival_id, generation, now)? {
        return Err(err(2, "arrival is not unresolved"));
    }
    Ok(())
}

/// `filing dismiss --arrival`: a reviewed unresolved arrival becomes
/// `dismissed`, lifting its done-inference barrier.
pub fn dismiss_arrival(store: &mut Store, account: &str, arrival_id: i64, now: &str) -> Result<()> {
    if !store.dismiss_arrival_row(account, arrival_id, now)? {
        return Err(err(2, "arrival is not unresolved"));
    }
    Ok(())
}

/// `filing adopt --folder`: confirms an existing folder whose role could not
/// be verified; the next pass re-resolves it (spec "Folders" rule 3).
pub fn adopt_folder(store: &mut Store, account: &str, native: &str, _now: &str) -> Result<()> {
    let Some(mut rec) = store.folder_record(account, native)? else {
        return Err(err(2, "unknown folder; run sync first"));
    };
    if !rec.confirmed {
        rec.confirmed = true;
        store.save_folder(&rec)?;
    }
    Ok(())
}
