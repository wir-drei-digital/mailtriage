//! Arrival resolution and placement re-evaluation (spec "Arrivals and
//! location", step 7 of a pass): observed occurrences become client
//! corrections, pins, relocations and location states. Inference never runs
//! for a message with an open move intent (intent recovery decides its
//! outcome) or for an arrival that is quarantined or explained by a revert.
use super::apply::{commit, commit_with_placement, event, write_failed};
use super::inputs::effective;
use super::observe::FolderMap;
use super::planner::CategoryFolder;
use super::{
    is_config_changed, Arrival, FilingSummary, FilingWrite, FolderRecord, Intent, LocationState,
    PassContext, Placement, Revert, OPEN_INTENT_STATES,
};
use crate::store::Store;
use anyhow::{anyhow, bail, Result};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

/// UIDs per `envelopes` call.
const BATCH: usize = 100;

/// A message occurrence: (folder, epoch, UID).
type Loc = (String, u64, u64);

/// What a watched folder means to location inference.
enum Kind {
    Category(String),
    Source,
    /// A retired folder: neither a correction nor a pin.
    Other,
}

fn kind_of(map: &FolderMap, folder: &str) -> Kind {
    if map.sources.iter().any(|s| s == folder) {
        return Kind::Source;
    }
    map.categories
        .iter()
        .find(|(_, c)| matches!(c, CategoryFolder::Native(n) if n == folder))
        .map_or(Kind::Other, |(id, _)| Kind::Category(id.clone()))
}

fn home_of(p: &Placement) -> Option<Loc> {
    Some((p.home_folder.clone()?, p.home_epoch?, p.home_uid?))
}

fn set_home(p: &mut Placement, at: &Loc) {
    p.home_folder = Some(at.0.clone());
    p.home_epoch = Some(at.1);
    p.home_uid = Some(at.2);
    p.location_state = LocationState::Known;
    p.absent_since = None;
}

/// A failed item counts as a filing error with a code-only problem; a mail
/// engine configuration change aborts the pass.
pub(crate) fn step_error(e: anyhow::Error, code: &str, summary: &mut FilingSummary) -> Result<()> {
    if is_config_changed(&e) {
        return Err(e);
    }
    summary.errors += 1;
    if !summary.problems.iter().any(|p| p == code) {
        summary.problems.push(code.into());
    }
    Ok(())
}

/// Pass-level state location inference consults; arrival resolution does
/// not change any of it.
struct Facts {
    open_moves: BTreeSet<String>,
    moves: Vec<Intent>,
    reverts: Vec<Revert>,
    folders: BTreeMap<String, FolderRecord>,
}

impl Facts {
    fn load(store: &Store, account: &str) -> Result<Self> {
        let moves: Vec<Intent> = store
            .intents(account, false)?
            .into_iter()
            .filter(|i| i.kind == "move")
            .collect();
        let open_moves = moves
            .iter()
            .filter(|i| OPEN_INTENT_STATES.contains(&i.state.as_str()))
            .map(|i| i.message_id.clone())
            .collect();
        let folders = store
            .folder_records(account)?
            .into_iter()
            .map(|r| (r.native.clone(), r))
            .collect();
        Ok(Self {
            open_moves,
            moves,
            reverts: store.reverts(account, false)?,
            folders,
        })
    }

    fn open_revert_at(&self, at: &Loc) -> bool {
        self.reverts.iter().any(|r| {
            matches!(r.state.as_str(), "pending" | "in_flight")
                && (r.folder.as_str(), r.folder_epoch, r.uid) == (at.0.as_str(), at.1, at.2)
        })
    }

    /// The locator an applied revert returned a raced message to.
    fn applied_revert_to(&self, at: &Loc) -> bool {
        self.reverts.iter().any(|r| {
            r.state == "applied"
                && (r.target.as_str(), r.target_epoch, r.target_uid)
                    == (at.0.as_str(), at.1, Some(at.2))
        })
    }

    /// Spec "Epoch race": an arrival in a raced move's target, in its target
    /// epoch at `uid >= target_uid_next`, that no intent's COPYUID explains.
    /// The window stays open while the raced source is paused (until the
    /// user releases it).
    fn quarantined(&self, a: &Arrival) -> bool {
        if a.intent_id.is_some() {
            return false;
        }
        let targets = |i: &Intent| {
            i.target.as_deref() == Some(a.folder.as_str()) && i.target_epoch == Some(a.epoch)
        };
        let explained = self
            .moves
            .iter()
            .any(|i| targets(i) && i.target_uid == Some(a.uid));
        let raced = |i: &Intent| {
            i.error
                .as_deref()
                .is_some_and(|e| e.starts_with("epoch_race"))
                && self
                    .folders
                    .get(&i.folder)
                    .is_some_and(|r| r.pause_reason.is_some())
                && i.target_uid_next.is_some_and(|n| a.uid >= n)
        };
        !explained && self.moves.iter().any(|i| targets(i) && raced(i))
    }

    /// Below the reset-time `UIDNEXT` of the folder's latest rescan epoch.
    fn in_rescan(&self, at: &Loc) -> bool {
        self.folders.get(&at.0).is_some_and(|r| {
            r.rescan_epoch == Some(at.1) && r.rescan_below_uid.is_some_and(|below| at.2 < below)
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Home {
    Present,
    Absent,
    /// The folder is no longer in the home's epoch: wait for its rescan.
    Unknown,
}

/// Server-side presence of home locators: one `envelopes` read per folder
/// epoch and batch, bracketed by snapshots that must both report the epoch.
/// A home in an older epoch is unknown until the folder's reset rescan is
/// complete, and absent afterwards (the rescan did not re-establish it).
struct HomeChecks {
    known: BTreeMap<Loc, Home>,
    verified: bool,
    /// Folder -> epoch of its complete reset rescan.
    rescanned: BTreeMap<String, u64>,
}

impl HomeChecks {
    fn new(facts: &Facts) -> Self {
        let rescanned = facts
            .folders
            .values()
            .filter(|r| r.rescan_complete)
            .filter_map(|r| Some((r.native.clone(), r.rescan_epoch?)))
            .collect();
        Self {
            known: BTreeMap::new(),
            verified: false,
            rescanned,
        }
    }

    fn check(
        &mut self,
        ctx: &PassContext,
        map: &FolderMap,
        locs: &[Loc],
        summary: &mut FilingSummary,
    ) -> Result<()> {
        let mut groups: BTreeMap<(String, u64), BTreeSet<u64>> = BTreeMap::new();
        for loc in locs.iter().filter(|l| !self.known.contains_key(*l)) {
            groups
                .entry((loc.0.clone(), loc.1))
                .or_default()
                .insert(loc.2);
        }
        for ((folder, epoch), uids) in groups {
            let uids: Vec<u64> = uids.into_iter().collect();
            let seen = match self.observe(ctx, map, &folder, epoch, &uids) {
                Ok(seen) => seen,
                Err(e) => {
                    write_failed(e, "home_check_failed", &folder, summary)?;
                    vec![Home::Unknown; uids.len()]
                }
            };
            for (uid, home) in uids.into_iter().zip(seen) {
                self.known.insert((folder.clone(), epoch, uid), home);
            }
        }
        Ok(())
    }

    fn observe(
        &mut self,
        ctx: &PassContext,
        map: &FolderMap,
        folder: &str,
        epoch: u64,
        uids: &[u64],
    ) -> Result<Vec<Home>> {
        if !map.watch.iter().any(|w| w.folder == folder) {
            // Occurrences in an unwatched folder are frozen: present while LIST reports it.
            let home = if map.listed.contains(folder) {
                Home::Present
            } else {
                Home::Absent
            };
            return Ok(vec![home; uids.len()]);
        }
        if !self.verified {
            (ctx.verify_binding)()?;
            self.verified = true;
        }
        let unknown = vec![Home::Unknown; uids.len()];
        let current = ctx.engine.snapshot(folder)?.uid_validity;
        if current != epoch {
            let gone = epoch < current && self.rescanned.get(folder) == Some(&current);
            return Ok(if gone {
                vec![Home::Absent; uids.len()]
            } else {
                unknown
            });
        }
        let mut present = BTreeSet::new();
        for chunk in uids.chunks(BATCH) {
            present.extend(ctx.engine.envelopes(folder, chunk)?.iter().map(|e| e.uid));
        }
        if ctx.engine.snapshot(folder)?.uid_validity != epoch {
            return Ok(unknown);
        }
        Ok(uids
            .iter()
            .map(|u| {
                if present.contains(u) {
                    Home::Present
                } else {
                    Home::Absent
                }
            })
            .collect())
    }

    fn get(
        &mut self,
        ctx: &PassContext,
        map: &FolderMap,
        loc: &Loc,
        summary: &mut FilingSummary,
    ) -> Result<Home> {
        self.check(ctx, map, std::slice::from_ref(loc), summary)?;
        Ok(self.known.get(loc).copied().unwrap_or(Home::Unknown))
    }

    /// A home just set to an occurrence this pass recorded.
    fn assume_present(&mut self, loc: &Loc) {
        self.known.insert(loc.clone(), Home::Present);
    }
}

/// Step 7 (mode not off): resolves `pending` arrivals oldest first, with the
/// homes of placed messages checked in batches first.
pub fn resolve_arrivals(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    summary: &mut FilingSummary,
) -> Result<()> {
    if map.caps.is_none() {
        return Ok(());
    }
    let pending = store.arrivals(ctx.account, Some("pending"))?;
    if pending.is_empty() {
        return Ok(());
    }
    let facts = Facts::load(store, ctx.account)?;
    let mut homes = HomeChecks::new(&facts);
    let wanted = homes_to_check(store, ctx, &facts, &pending)?;
    homes.check(ctx, map, &wanted, summary)?;
    for a in &pending {
        if let Err(e) = resolve_one(store, ctx, map, &facts, &mut homes, a, summary) {
            step_error(e, "arrival_failed", summary)?;
        }
    }
    Ok(())
}

/// The homes of known placements whose arrival may need them.
fn homes_to_check(
    store: &Store,
    ctx: &PassContext,
    facts: &Facts,
    pending: &[Arrival],
) -> Result<Vec<Loc>> {
    let mut out = Vec::new();
    for a in pending {
        if facts.open_moves.contains(&a.message_id) {
            continue;
        }
        let Some(p) = store.placement(ctx.account, &a.message_id)? else {
            continue;
        };
        let home = home_of(&p).filter(|_| p.location_state == LocationState::Known);
        if let Some(home) = home.filter(|h| *h != (a.folder.clone(), a.epoch, a.uid)) {
            out.push(home);
        }
    }
    Ok(out)
}

/// One arrival, in the order of the brief's resolution list.
fn resolve_one(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    facts: &Facts,
    homes: &mut HomeChecks,
    a: &Arrival,
    summary: &mut FilingSummary,
) -> Result<()> {
    let at: Loc = (a.folder.clone(), a.epoch, a.uid);
    let fingerprinted = store
        .message_meta(ctx.account, &a.message_id)?
        .is_some_and(|m| m.fingerprinted);
    if !fingerprinted {
        return unidentified(store, ctx, a, summary);
    }
    if facts.open_moves.contains(&a.message_id) || facts.open_revert_at(&at) {
        return Ok(());
    }
    if facts.applied_revert_to(&at) {
        resolve(store, ctx, a, "reverted")?;
        return reevaluate_one(
            store,
            ctx,
            map,
            facts,
            &a.message_id,
            Some(&at),
            true,
            summary,
        );
    }
    if facts.quarantined(a) {
        return quarantine(store, ctx, map, a, &at, summary);
    }
    let placement = store.placement(ctx.account, &a.message_id)?;
    if a.intent_id.is_some() || placement.as_ref().and_then(home_of).as_ref() == Some(&at) {
        // A placement homed here and never filed was created for this
        // arrival (at fetch, or before an interrupted resolution): new mail.
        if a.intent_id.is_none() && placement.as_ref().is_some_and(|p| p.filed_by.is_none()) {
            return new_arrival(store, ctx, map, a, &at, summary);
        }
        return resolve(store, ctx, a, "own_move");
    }
    if facts.in_rescan(&at) {
        if placement.is_none() {
            store.ensure_placement(ctx.account, &a.message_id, &map.sources)?;
        }
        resolve(store, ctx, a, "rescan")?;
        return reevaluate_one(
            store,
            ctx,
            map,
            facts,
            &a.message_id,
            Some(&at),
            true,
            summary,
        );
    }
    let occurring = store.occurrence_at(ctx.account, &a.folder, a.epoch, a.uid)?;
    if occurring.as_deref() != Some(a.message_id.as_str()) {
        // Gone again before it could be inferred from: nothing to conclude.
        return resolve(store, ctx, a, "extra");
    }
    match placement {
        None => new_arrival(store, ctx, map, a, &at, summary),
        Some(p) => placed_arrival(store, ctx, map, homes, &p, a, &at, summary),
    }
}

fn resolve(store: &mut Store, ctx: &PassContext, a: &Arrival, kind: &str) -> Result<()> {
    store.resolve_arrival(a.id, "resolved", Some(kind), &ctx.now)
}

fn arrival_writes(arrival: Option<(i64, &str)>) -> Vec<FilingWrite<'_>> {
    arrival
        .map(|(id, kind)| FilingWrite::Arrival {
            id,
            state: "resolved",
            kind: Some(kind),
        })
        .into_iter()
        .collect()
}

/// A provisional message: `vanished` once its occurrence is gone,
/// `unresolved` once its fetch failed terminally, else still pending.
fn unidentified(
    store: &mut Store,
    ctx: &PassContext,
    a: &Arrival,
    summary: &mut FilingSummary,
) -> Result<()> {
    let occurring = store.occurrence_at(ctx.account, &a.folder, a.epoch, a.uid)?;
    if occurring.as_deref() != Some(a.message_id.as_str()) {
        return store.resolve_arrival(a.id, "vanished", None, &ctx.now);
    }
    if store.job_state(&a.message_id)?.as_deref() != Some("terminal") {
        return Ok(());
    }
    let writes = [
        FilingWrite::Arrival {
            id: a.id,
            state: "unresolved",
            kind: None,
        },
        event(
            Some(&a.message_id),
            Some(&a.folder),
            "arrival_unresolved",
            json!({"arrival_id": a.id}),
        ),
    ];
    commit(store, ctx, &writes)?;
    summary.unresolved += 1;
    Ok(())
}

/// A quarantined arrival: the placement it created (or that is homed at it)
/// is blocked; an existing placement elsewhere is left untouched.
fn quarantine(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    a: &Arrival,
    at: &Loc,
    summary: &mut FilingSummary,
) -> Result<()> {
    let created = store.ensure_placement(ctx.account, &a.message_id, &map.sources)?;
    let homed_here = store
        .placement(ctx.account, &a.message_id)?
        .is_some_and(|p| home_of(&p).as_ref() == Some(at));
    let writes = arrival_writes(Some((a.id, "quarantined")));
    if created || homed_here {
        let block = |p: &mut Placement| p.blocked_reason = Some("quarantined".into());
        commit_with_placement(store, ctx, &a.message_id, block, &writes)?;
    } else {
        commit(store, ctx, &writes)?;
    }
    summary.quarantined += 1;
    Ok(())
}

/// A message without a placement: placed here. In category C's folder it is
/// filed by the user (a client correction to C); elsewhere it is new mail.
fn new_arrival(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    a: &Arrival,
    at: &Loc,
    summary: &mut FilingSummary,
) -> Result<()> {
    store.ensure_placement(ctx.account, &a.message_id, &map.sources)?;
    match kind_of(map, &a.folder) {
        Kind::Category(c) => client_move(
            store,
            ctx,
            &a.message_id,
            at,
            &c,
            Some((a.id, "new")),
            summary,
        ),
        _ => resolve(store, ctx, a, "new"),
    }
}

/// A placed message: an extra occurrence while its home is present (or its
/// location ambiguous, or its home folder's rescan re-found it), else moved
/// here by the user.
#[allow(clippy::too_many_arguments)] // One arrival with its pass context.
fn placed_arrival(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    homes: &mut HomeChecks,
    p: &Placement,
    a: &Arrival,
    at: &Loc,
    summary: &mut FilingSummary,
) -> Result<()> {
    let refound = match home_of(p) {
        Some((folder, epoch, _)) => store
            .occurrences_of(ctx.account, &p.message_id)?
            .iter()
            .any(|(f, e, _)| *f == folder && *e != epoch),
        None => false,
    };
    match (p.location_state, home_of(p)) {
        (LocationState::Ambiguous, _) => return resolve(store, ctx, a, "extra"),
        (LocationState::Known, Some(_)) if refound => return resolve(store, ctx, a, "extra"),
        (LocationState::Known, Some(home)) => match homes.get(ctx, map, &home, summary)? {
            Home::Present => return resolve(store, ctx, a, "extra"),
            Home::Unknown => return Ok(()),
            Home::Absent => {}
        },
        _ => {}
    }
    match kind_of(map, &a.folder) {
        Kind::Category(c) => client_move(
            store,
            ctx,
            &a.message_id,
            at,
            &c,
            Some((a.id, "user_move")),
            summary,
        )?,
        Kind::Source => pin_here(
            store,
            ctx,
            &a.message_id,
            at,
            Some((a.id, "user_pin")),
            summary,
        )?,
        Kind::Other => relocate(store, ctx, &a.message_id, at, None, Some((a.id, "extra")))?,
    }
    homes.assume_present(at);
    Ok(())
}

/// Spec "Placement transitions", client move into category C: override
/// `category_id = C` unless the override already is C; home here,
/// `filed_by = user`, unpinned, the explicit request cleared, a new
/// revision, event `client_correction`. The override compare-and-swap, the
/// placement, the arrival and the event are one transaction.
fn client_move(
    store: &mut Store,
    ctx: &PassContext,
    id: &str,
    at: &Loc,
    category: &str,
    arrival: Option<(i64, &str)>,
    summary: &mut FilingSummary,
) -> Result<()> {
    for _ in 0..5 {
        let record = store
            .record(ctx.account, id)?
            .ok_or_else(|| anyhow!("unknown message"))?;
        let Some(mut p) = store.placement(ctx.account, id)? else {
            bail!("message has no placement");
        };
        let inferred = p.done_inferred;
        let previous = effective(&record, ctx.generation).category_id;
        let mut overrides = record.overrides.clone();
        if let Some(o) = overrides.as_object_mut() {
            if o.get("category_id").and_then(|v| v.as_str()) != Some(category) {
                o.insert("category_id".into(), json!(category));
            }
        }
        set_home(&mut p, at);
        p.filed_by = Some("user".into());
        p.pinned = false;
        p.desired_target = None;
        p.desired_rev += 1;
        let mut writes = arrival_writes(arrival);
        let detail = json!({"category_id": category, "previous": previous});
        writes.push(event(Some(id), Some(&at.0), "client_correction", detail));
        let expected = &record.overrides;
        if store.correct_with_writes(
            ctx.account,
            id,
            expected,
            &overrides,
            Some(&p),
            &writes,
            &ctx.now,
        )? {
            summary.client_corrections += 1;
            return reopen(store, ctx, id, inferred);
        }
    }
    bail!("placement kept changing concurrently")
}

/// Spec "Placement transitions", client move into a source folder: home
/// here, pinned, the explicit request cleared, a new revision, event `pinned`.
fn pin_here(
    store: &mut Store,
    ctx: &PassContext,
    id: &str,
    at: &Loc,
    arrival: Option<(i64, &str)>,
    summary: &mut FilingSummary,
) -> Result<()> {
    let mut writes = arrival_writes(arrival);
    writes.push(event(
        Some(id),
        Some(&at.0),
        "pinned",
        json!({"via": "client"}),
    ));
    let mut inferred = false;
    let pin = |p: &mut Placement| {
        inferred = p.done_inferred;
        set_home(p, at);
        p.pinned = true;
        p.desired_target = None;
        p.desired_rev += 1;
    };
    commit_with_placement(store, ctx, id, pin, &writes)?;
    summary.pinned += 1;
    reopen(store, ctx, id, inferred)
}

/// A location-only update: home here, `known`, with an optional event.
fn relocate(
    store: &mut Store,
    ctx: &PassContext,
    id: &str,
    at: &Loc,
    event_kind: Option<&str>,
    arrival: Option<(i64, &str)>,
) -> Result<()> {
    let mut writes = arrival_writes(arrival);
    if let Some(kind) = event_kind {
        writes.push(event(Some(id), Some(&at.0), kind, json!({})));
    }
    let mut inferred = false;
    let relocate = |p: &mut Placement| {
        inferred = p.done_inferred;
        set_home(p, at);
    };
    commit_with_placement(store, ctx, id, relocate, &writes)?;
    reopen(store, ctx, id, inferred)
}

/// A message whose Done was inferred reappeared: it is reopened (event
/// `reopened`) and requeued when its classification is stale.
fn reopen(store: &mut Store, ctx: &PassContext, id: &str, inferred: bool) -> Result<()> {
    if !inferred || !store.reopen_inferred(ctx.account, id)? {
        return Ok(());
    }
    store.record_event(ctx.account, Some(id), None, "reopened", json!({}), &ctx.now)?;
    if let Some(r) = store.record(ctx.account, id)? {
        if r.generation != ctx.generation {
            store.requeue(ctx.account, std::slice::from_ref(&r.id), ctx.generation)?;
        }
    }
    Ok(())
}

/// Spec "Placement re-evaluation" for messages whose home occurrence may have
/// disappeared (reconciliation, write-batch verification, rescan
/// completion). Messages with an open move intent are skipped. A home still
/// recorded locally is checked on the server first; one found gone is
/// removed, then the placement is re-evaluated.
pub fn reevaluate(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    message_ids: &[String],
    summary: &mut FilingSummary,
) -> Result<()> {
    if map.caps.is_none() || message_ids.is_empty() {
        return Ok(());
    }
    let facts = Facts::load(store, ctx.account)?;
    let mut ids = BTreeSet::new();
    for id in message_ids {
        // A merged provisional's id resolves to its canonical message.
        if let Some(r) = store.record(ctx.account, id)? {
            if !facts.open_moves.contains(&r.id) {
                ids.insert(r.id);
            }
        }
    }
    let recorded = recorded_homes(store, ctx, &ids)?;
    let mut homes = HomeChecks::new(&facts);
    let locs: Vec<Loc> = recorded.values().cloned().collect();
    homes.check(ctx, map, &locs, summary)?;
    for id in &ids {
        let home = recorded
            .get(id)
            .map(|loc| (loc, homes.known.get(loc).copied()));
        if let Err(e) = reevaluate_checked(store, ctx, map, &facts, id, home, summary) {
            step_error(e, "reevaluation_failed", summary)?;
        }
    }
    Ok(())
}

/// Known placements whose home occurrence is still recorded locally.
fn recorded_homes(
    store: &Store,
    ctx: &PassContext,
    ids: &BTreeSet<String>,
) -> Result<BTreeMap<String, Loc>> {
    let mut out = BTreeMap::new();
    for id in ids {
        let Some(p) = store.placement(ctx.account, id)? else {
            continue;
        };
        let Some(home) = home_of(&p).filter(|_| p.location_state == LocationState::Known) else {
            continue;
        };
        if store
            .occurrence_at(ctx.account, &home.0, home.1, home.2)?
            .as_deref()
            == Some(id.as_str())
        {
            out.insert(id.clone(), home);
        }
    }
    Ok(out)
}

fn reevaluate_checked(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    facts: &Facts,
    id: &str,
    home: Option<(&Loc, Option<Home>)>,
    summary: &mut FilingSummary,
) -> Result<()> {
    if let Some((loc, seen)) = home {
        if seen != Some(Home::Absent) {
            return Ok(());
        }
        store.remove_occurrence(ctx.account, &loc.0, loc.1, loc.2)?;
    }
    reevaluate_one(store, ctx, map, facts, id, None, false, summary)
}

/// Re-evaluates one placement from its recorded occurrences. `hint` is the
/// occurrence a rescan or revert arrival re-established in the home's
/// folder; `location_only` never infers a correction or pin.
#[allow(clippy::too_many_arguments)] // One placement with its pass context.
fn reevaluate_one(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    facts: &Facts,
    id: &str,
    hint: Option<&Loc>,
    location_only: bool,
    summary: &mut FilingSummary,
) -> Result<()> {
    let Some(p) = store.placement(ctx.account, id)? else {
        return Ok(());
    };
    let occurrences = store.occurrences_of(ctx.account, id)?;
    let home = home_of(&p);
    if p.location_state == LocationState::Known
        && home.as_ref().is_some_and(|h| occurrences.contains(h))
    {
        return Ok(());
    }
    if let Some(h) =
        hint.filter(|h| occurrences.contains(h) && p.home_folder.as_deref() == Some(h.0.as_str()))
    {
        return relocate(store, ctx, id, h, None, None);
    }
    let watched: BTreeSet<&str> = map.watch.iter().map(|w| w.folder.as_str()).collect();
    let survivors: Vec<&Loc> = occurrences
        .iter()
        .filter(|(f, _, _)| watched.contains(f.as_str()))
        .collect();
    match survivors.as_slice() {
        [] => set_absent(store, ctx, &p),
        [one] => survivor(store, ctx, map, facts, id, one, location_only, summary),
        many => set_ambiguous(store, ctx, &p, many.len()),
    }
}

/// Exactly one surviving occurrence: the matching client move transition,
/// `relocated` within the same category, or a location update during a
/// rescan, for a quarantined or reverted occurrence, or in a retired folder.
/// An occurrence whose own arrival is still pending is left to it.
#[allow(clippy::too_many_arguments)] // One placement with its pass context.
fn survivor(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    facts: &Facts,
    id: &str,
    s: &Loc,
    location_only: bool,
    summary: &mut FilingSummary,
) -> Result<()> {
    let arrival = store.arrival_at(ctx.account, &s.0, s.1, s.2)?;
    if arrival.as_ref().is_some_and(|a| a.state == "pending") {
        return Ok(());
    }
    let settled = arrival
        .as_ref()
        .and_then(|a| a.kind.as_deref())
        .is_some_and(|k| k == "quarantined" || k == "reverted");
    if location_only || settled || facts.in_rescan(s) {
        return relocate(store, ctx, id, s, None, None);
    }
    match kind_of(map, &s.0) {
        Kind::Category(c) => {
            let record = store
                .record(ctx.account, id)?
                .ok_or_else(|| anyhow!("unknown message"))?;
            if effective(&record, ctx.generation).category_id.as_deref() == Some(c.as_str()) {
                relocate(store, ctx, id, s, Some("relocated"), None)
            } else {
                client_move(store, ctx, id, s, &c, None, summary)
            }
        }
        Kind::Source => pin_here(store, ctx, id, s, None, summary),
        Kind::Other => relocate(store, ctx, id, s, None, None),
    }
}

/// No surviving occurrence: `absent` since now (an earlier absence keeps its time).
fn set_absent(store: &mut Store, ctx: &PassContext, p: &Placement) -> Result<()> {
    if p.location_state == LocationState::Absent {
        return Ok(());
    }
    let absent = |p: &mut Placement| {
        if p.location_state != LocationState::Absent {
            p.location_state = LocationState::Absent;
            p.absent_since = Some(ctx.now.clone());
        }
    };
    commit_with_placement(store, ctx, &p.message_id, absent, &[])
}

/// Several surviving occurrences: `ambiguous`, event `location_ambiguous`.
/// A message whose Done was inferred reappeared, so it is reopened.
fn set_ambiguous(store: &mut Store, ctx: &PassContext, p: &Placement, n: usize) -> Result<()> {
    if p.location_state == LocationState::Ambiguous {
        return Ok(());
    }
    let id = p.message_id.as_str();
    let detail = json!({"occurrences": n});
    let writes = [event(Some(id), None, "location_ambiguous", detail)];
    let mut inferred = false;
    let ambiguous = |p: &mut Placement| {
        inferred = p.done_inferred;
        p.location_state = LocationState::Ambiguous;
        p.absent_since = None;
    };
    commit_with_placement(store, ctx, id, ambiguous, &writes)?;
    reopen(store, ctx, id, inferred)
}

/// Spec "Epoch reset of any watched folder": once a folder's rescan is
/// complete, its rescan-set members not found in it are re-evaluated and its
/// rescan sets (of that epoch and earlier ones) are pruned, so a referenced
/// retired folder stops being watched.
pub fn complete_rescans(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    summary: &mut FilingSummary,
) -> Result<()> {
    if map.caps.is_none() {
        return Ok(());
    }
    for rec in store.folder_records(ctx.account)? {
        let (true, Some(epoch)) = (rec.rescan_complete, rec.rescan_epoch) else {
            continue;
        };
        let members = store.rescan_set_ids(ctx.account, &rec.native, epoch)?;
        if members.is_empty() {
            continue;
        }
        let mut missing = Vec::new();
        for id in members {
            let found = store
                .occurrences_of(ctx.account, &id)?
                .iter()
                .any(|(f, e, _)| *f == rec.native && *e == epoch);
            if !found {
                missing.push(id);
            }
        }
        reevaluate(store, ctx, map, &missing, summary)?;
        store.delete_rescan_sets(ctx.account, &rec.native, epoch)?;
    }
    Ok(())
}

/// Spec "Filing-aware merge": `attach` refused to merge `provisional` into the
/// message holding `fingerprint` because the provisional carries local edits.
/// Its pending arrivals become `unresolved` (event `merge_conflict`) and the
/// canonical placement, unless already blocked, gets `merge_conflict`, in
/// one transaction. Without a pending arrival (filing off) nothing changes.
pub fn merge_conflict(
    store: &mut Store,
    account: &str,
    provisional: &str,
    fingerprint: &str,
    now: &str,
) -> Result<()> {
    let arrivals: Vec<Arrival> = store
        .arrivals(account, Some("pending"))?
        .into_iter()
        .filter(|a| a.message_id == provisional)
        .collect();
    if arrivals.is_empty() {
        return Ok(());
    }
    let canonical = store.canonical_for_fingerprint(account, fingerprint)?;
    let subject = canonical.as_deref().unwrap_or(provisional);
    for _ in 0..5 {
        let placement = match &canonical {
            Some(id) => store.placement(account, id)?,
            None => None,
        };
        let blocked = placement
            .filter(|p| p.blocked_reason.is_none())
            .map(|mut p| {
                p.blocked_reason = Some("merge_conflict".into());
                p
            });
        let mut writes = Vec::new();
        if let Some(p) = &blocked {
            writes.push(FilingWrite::Placement {
                placement: p,
                expected_rev: p.desired_rev,
            });
        }
        for a in &arrivals {
            writes.push(FilingWrite::Arrival {
                id: a.id,
                state: "unresolved",
                kind: None,
            });
            let detail = json!({"arrival_id": a.id, "provisional": provisional});
            writes.push(event(
                Some(subject),
                Some(&a.folder),
                "merge_conflict",
                detail,
            ));
        }
        if store.commit_filing(account, &writes, now)? {
            return Ok(());
        }
    }
    bail!("placement kept changing concurrently")
}
