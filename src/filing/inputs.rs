//! Planner input built from the store (spec "Planner"): effective decisions,
//! folder views and one planned message per placement.
use super::observe::FolderMap;
use super::planner::{Effective, FolderUse, FolderView, Locator, PlanInput, PlanMessage};
use super::{LocationState, MessageMeta, PassContext, Placement};
use crate::domain::Urgency;
use crate::store::{Record, Store};
use anyhow::Result;
use chrono::{DateTime, Timelike, Utc};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// The effective decision as `Service::item` computes it: the model decision
/// with per-field overrides applied.
pub fn effective(record: &Record, generation: &str) -> Effective {
    let c = record.classification.as_ref();
    let field = |key: &str| c.and_then(|c| c.get(key)).filter(|v| !v.is_null());
    let mut e = Effective {
        category_id: field("category_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
        urgency: field("urgency").and_then(urgency_of),
        action_required: field("action_required").and_then(Value::as_bool),
        current: c.is_some()
            && record.generation == generation
            && matches!(record.status.as_str(), "ready" | "uncertain"),
        input_incomplete: field("reasons")
            .and_then(Value::as_array)
            .is_some_and(|r| r.iter().any(|x| x == "input_incomplete")),
        ..Default::default()
    };
    if let Some(v) = record.overrides.get("category_id").and_then(Value::as_str) {
        e.category_id = Some(v.to_owned());
        e.category_from_override = true;
    }
    if let Some(v) = record.overrides.get("urgency").and_then(urgency_of) {
        e.urgency = Some(v);
        e.urgency_from_override = true;
    }
    if let Some(v) = record
        .overrides
        .get("action_required")
        .and_then(Value::as_bool)
    {
        e.action_required = Some(v);
        e.action_from_override = true;
    }
    e
}

fn urgency_of(v: &Value) -> Option<Urgency> {
    serde_json::from_value(v.clone()).ok()
}

/// Everything the pure planner needs, read from the store and this pass's
/// folder map. Categories come from `map.categories`, which leaves out a
/// category whose folder collides with a source folder.
pub fn plan_input(
    store: &Store,
    ctx: &PassContext,
    map: &FolderMap,
    preview: bool,
) -> Result<PlanInput> {
    let open: BTreeSet<(String, String)> = store
        .intents(ctx.account, true)?
        .into_iter()
        .map(|i| (i.message_id, i.kind))
        .collect();
    let has_open = |id: &str, kind: &str| open.contains(&(id.to_string(), kind.to_string()));
    let messages = store
        .records_for_planning(ctx.account)?
        .into_iter()
        .map(|(record, p, meta)| {
            let mut m = plan_message(&p, &meta, effective(&record, ctx.generation));
            m.open_move_intent = has_open(&p.message_id, "move");
            m.open_flag_intent = has_open(&p.message_id, "flag");
            m
        })
        .collect();
    Ok(PlanInput {
        mode: ctx.mode,
        preview,
        flag_enabled: ctx.cfg.filing.flag,
        max_actions: ctx.cfg.filing.max_actions_per_pass,
        // INTERNALDATE has whole-second resolution, so `enabled_at` is compared
        // at that resolution: mail delivered in the second filing was enabled is new.
        enabled_at: store
            .filing_state(ctx.account)?
            .enabled_at
            .as_deref()
            .and_then(parse_time)
            .and_then(|t| t.with_nanosecond(0)),
        categories: map.categories.clone(),
        folders: folder_views(store, ctx, map)?,
        messages,
    })
}

fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

fn plan_message(p: &Placement, meta: &MessageMeta, effective: Effective) -> PlanMessage {
    let known = p.location_state == LocationState::Known;
    let home = match (known, &p.home_folder, p.home_epoch, p.home_uid) {
        (true, Some(folder), Some(epoch), Some(uid)) => Some(Locator {
            folder: folder.clone(),
            epoch,
            uid,
        }),
        _ => None,
    };
    PlanMessage {
        message_id: p.message_id.clone(),
        source_folder: p.source_folder.clone(),
        home,
        location_known: known,
        hydrated: meta.size.is_some(),
        internal_date: meta.internal_date.as_deref().and_then(parse_time),
        flags: meta.flags.clone(),
        desired_target: p.desired_target.clone(),
        pinned: p.pinned,
        eligible_once: p.eligible_once,
        desired_rev: p.desired_rev,
        filed_at: p.filed_at.clone(),
        flag_attempted: p.flag_attempted_at.is_some(),
        blocked: p.blocked_reason.is_some(),
        open_move_intent: false,
        open_flag_intent: false,
        effective,
    }
}

/// Sources are usable; a category folder is usable only in state `ok`; a
/// retired folder is no category and only `retired_listed` admits it as the
/// home of an explicit move; a folder a preview would create is `WouldCreate`.
fn folder_views(
    store: &Store,
    ctx: &PassContext,
    map: &FolderMap,
) -> Result<BTreeMap<String, FolderView>> {
    let epoch_of = |folder: &str| -> Result<Option<u64>> {
        Ok(store.checkpoint_state(ctx.account, folder)?.map(|c| c.0))
    };
    let records: BTreeMap<String, _> = store
        .folder_records(ctx.account)?
        .into_iter()
        .map(|r| (r.native.clone(), r))
        .collect();
    let sources: BTreeSet<&String> = map.sources.iter().collect();
    let mut views = BTreeMap::new();
    for source in &map.sources {
        let view = FolderView {
            usable: FolderUse::Ok,
            paused: records
                .get(source)
                .is_some_and(|r| r.pause_reason.is_some()),
            is_source: true,
            is_category: false,
            retired_listed: false,
            epoch: epoch_of(source)?,
        };
        views.insert(source.clone(), view);
    }
    for (native, r) in &records {
        if sources.contains(native) || r.category_id.is_none() {
            continue;
        }
        let retired = r.state == "retired";
        let view = FolderView {
            usable: if r.state == "ok" {
                FolderUse::Ok
            } else {
                FolderUse::Unusable
            },
            paused: r.pause_reason.is_some(),
            is_source: false,
            is_category: !retired,
            retired_listed: retired && map.listed.contains(native),
            epoch: epoch_of(native)?,
        };
        views.insert(native.clone(), view);
    }
    for native in &map.would_create {
        views.insert(
            native.clone(),
            FolderView {
                usable: FolderUse::WouldCreate,
                paused: false,
                is_source: false,
                is_category: true,
                retired_listed: false,
                epoch: None,
            },
        );
    }
    Ok(views)
}
