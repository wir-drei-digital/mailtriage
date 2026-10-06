//! Refile spec "Retired folders": retention and draining keep a retired
//! folder watched while it holds refile-eligible mail, or while mail moved
//! into it may still be undiscovered; afterwards it freezes for good.
use crate::filing::refile::rules;
use crate::filing::{is_config_changed, FilingSummary, PassContext};
use crate::store::Store;
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};

/// This pass's retention and drain state, read before the watch scope is set.
#[derive(Debug, Clone, Default)]
pub struct RetiredRefs {
    /// Home folders of placements that pass candidate rules 1–5.
    pub retained: BTreeSet<String>,
    /// `folders.drain_until_uid` where set.
    pub drains: BTreeMap<String, u64>,
}

impl RetiredRefs {
    pub fn load(store: &Store, ctx: &PassContext) -> Result<Self> {
        let refile = rules::input(store, ctx.account, ctx.cfg, &BTreeSet::new())?;
        Ok(Self {
            retained: rules::retained(&refile),
            drains: store.drain_states(ctx.account)?,
        })
    }

    /// Whether the engine scope must include retired folder `native`.
    pub fn holds(&self, native: &str) -> bool {
        self.retained.contains(native) || self.drains.contains_key(native)
    }
}

/// One retired folder this pass. Returns whether retention or draining
/// keeps it watched. A folder neither retained, draining nor `referenced` by
/// an open intent, revert, pending arrival or rescan set is frozen; so is
/// one LIST no longer reports.
pub fn step(
    store: &mut Store,
    ctx: &PassContext,
    refs: &RetiredRefs,
    native: &str,
    listed: bool,
    referenced: bool,
    summary: &mut FilingSummary,
) -> Result<bool> {
    if !listed {
        store.freeze_retired(ctx.account, native)?;
        return Ok(false);
    }
    let drain = refs.drains.get(native).copied();
    if refs.retained.contains(native) {
        if drain != Some(0) {
            store.set_drain_until_uid(ctx.account, native, Some(0))?;
        }
        return Ok(true);
    }
    match drain {
        // Retention ended since the last pass: record the folder's UIDNEXT.
        Some(0) => match ctx.engine.snapshot(native) {
            Ok(s) if store.discovery_epoch(ctx.account, native)? == Some(s.uid_validity) => {
                store.set_drain_until_uid(ctx.account, native, Some(s.uid_next.max(1)))?;
                Ok(true)
            }
            // Another epoch: discovery records the reset first; snapshot again next pass.
            Ok(_) => Ok(true),
            Err(e) if is_config_changed(&e) => Err(e),
            Err(_) => {
                summary.errors += 1;
                summary
                    .problems
                    .push(format!("drain_snapshot_failed:{native}"));
                Ok(true)
            }
        },
        Some(_) if !referenced && store.drain_finished(ctx.account, native)? => {
            store.freeze_retired(ctx.account, native)?;
            Ok(false)
        }
        Some(_) => Ok(true),
        None => {
            if !referenced {
                store.freeze_retired(ctx.account, native)?;
            }
            Ok(false)
        }
    }
}
