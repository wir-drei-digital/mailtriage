//! Refile spec "Each pass", mark upkeep: before planning, every marked
//! placement keeps its mark, or loses it (event `refile_cleared` with the
//! reason) once it can no longer move.
use crate::filing::apply::{commit_with_placement, event};
use crate::filing::observe::FolderMap;
use crate::filing::planner::{CategoryFolder, Effective, PlanInput, PlanMessage};
use crate::filing::refile::rules::{RefileFacts, RefileInput};
use crate::filing::PassContext;
use crate::store::Store;
use anyhow::Result;
use serde_json::json;
use std::collections::BTreeSet;

/// Clears the marks steps 3–4 decide, in input order, and updates `refile`
/// so planning sees them cleared. A pass without a folder map (resolution
/// failed) clears nothing: every home would look gone.
pub fn run(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    input: &PlanInput,
    refile: &mut RefileInput,
) -> Result<()> {
    if map.caps.is_none() {
        return Ok(());
    }
    let rescanning: BTreeSet<String> = store
        .folder_records(ctx.account)?
        .into_iter()
        .filter(|r| r.rescan_epoch.is_some() && !r.rescan_complete)
        .map(|r| r.native)
        .collect();
    let mut cleared = Vec::new();
    for m in &input.messages {
        let Some(f) = refile.facts.get(&m.message_id).filter(|f| f.marked) else {
            continue;
        };
        let home_rescanning = f
            .home_folder
            .as_ref()
            .is_some_and(|h| rescanning.contains(h));
        let target = target_of(input, refile, m, f);
        let Some(reason) = clear_reason(f, &m.effective, target, home_rescanning) else {
            continue;
        };
        clear(store, ctx, &m.message_id, f.home_folder.as_deref(), reason)?;
        cleared.push(m.message_id.clone());
    }
    for id in cleared {
        if let Some(f) = refile.facts.get_mut(&id) {
            f.marked = false;
        }
    }
    Ok(())
}

/// Where the effective category files the message, from its recorded home.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    InPlace,
    InboxOrSource,
    Elsewhere,
    Unknown,
}

fn target_of(input: &PlanInput, refile: &RefileInput, m: &PlanMessage, f: &RefileFacts) -> Target {
    let Some(category) = m.effective.category_id.as_deref() else {
        return Target::Unknown;
    };
    match input.categories.get(category) {
        Some(CategoryFolder::Inbox) => Target::InboxOrSource,
        Some(CategoryFolder::Native(native))
            if f.home_folder.as_deref() == Some(native.as_str()) =>
        {
            Target::InPlace
        }
        Some(CategoryFolder::Native(native))
            if input.folders.get(native).is_some_and(|v| v.is_source) =>
        {
            Target::InboxOrSource
        }
        Some(CategoryFolder::Native(_)) => Target::Elsewhere,
        // A configured category the folder map leaves out files into a source.
        None if refile.categories.contains(category) => Target::InboxOrSource,
        None => Target::Unknown,
    }
}

/// Upkeep steps 1–5 for one marked placement; `None` keeps the mark.
fn clear_reason(
    f: &RefileFacts,
    e: &Effective,
    target: Target,
    home_rescanning: bool,
) -> Option<&'static str> {
    // 1–2: an open refile intent is resolved first; a rescan may find the home.
    if f.open_refile_intent || home_rescanning {
        return None;
    }
    // 3: reasons that do not depend on the classification.
    if f.done {
        return Some("done");
    }
    if f.corrected {
        return Some("corrected");
    }
    if f.pinned {
        return Some("pinned");
    }
    if !f.at_filed_home {
        return Some("not_filed_by_mailtriage");
    }
    if !f.single_occurrence {
        return Some("multiple_copies");
    }
    // 4: a stale classification never clears a mark.
    if !e.current {
        return None;
    }
    match target {
        Target::InPlace => Some("in_place"),
        Target::InboxOrSource => Some("target_inbox_or_source"),
        _ if e.input_incomplete => Some("incomplete_input"),
        _ => None,
    }
}

/// Clears the mark (a new revision) with event `refile_cleared`, in one transaction.
fn clear(
    store: &mut Store,
    ctx: &PassContext,
    id: &str,
    folder: Option<&str>,
    reason: &str,
) -> Result<()> {
    let writes = [event(
        Some(id),
        folder,
        "refile_cleared",
        json!({"reason": reason}),
    )];
    commit_with_placement(
        store,
        ctx,
        id,
        |p| {
            if p.refile_once {
                p.refile_once = false;
                p.desired_rev += 1;
            }
        },
        &writes,
    )
}

#[cfg(test)]
mod tests {
    use super::{clear_reason, Target};
    use crate::filing::planner::Effective;
    use crate::filing::refile::rules::RefileFacts;

    fn marked() -> RefileFacts {
        RefileFacts {
            marked: true,
            at_filed_home: true,
            home_folder: Some("Other".into()),
            single_occurrence: true,
            ..Default::default()
        }
    }

    fn current() -> Effective {
        Effective {
            category_id: Some("updates".into()),
            current: true,
            ..Default::default()
        }
    }

    #[test]
    fn placement_reasons_clear_in_the_stated_order() {
        let mut f = marked();
        f.single_occurrence = false;
        assert_eq!(
            clear_reason(&f, &current(), Target::Elsewhere, false),
            Some("multiple_copies")
        );
        f.at_filed_home = false;
        assert_eq!(
            clear_reason(&f, &current(), Target::Elsewhere, false),
            Some("not_filed_by_mailtriage")
        );
        f.pinned = true;
        assert_eq!(
            clear_reason(&f, &current(), Target::Elsewhere, false),
            Some("pinned")
        );
        f.corrected = true;
        assert_eq!(
            clear_reason(&f, &current(), Target::Elsewhere, false),
            Some("corrected")
        );
        f.done = true;
        assert_eq!(
            clear_reason(&f, &current(), Target::Elsewhere, false),
            Some("done")
        );
    }

    #[test]
    fn an_open_refile_intent_or_a_running_rescan_keeps_the_mark() {
        let mut f = marked();
        f.done = true;
        f.open_move_intent = true;
        f.open_refile_intent = true;
        assert_eq!(clear_reason(&f, &current(), Target::InPlace, false), None);
        f.open_refile_intent = false;
        assert_eq!(clear_reason(&f, &current(), Target::InPlace, true), None);
        assert_eq!(
            clear_reason(&f, &current(), Target::InPlace, false),
            Some("done"),
            "another open move intent does not defer upkeep"
        );
    }

    #[test]
    fn only_a_current_classification_clears_by_where_it_files_the_message() {
        let f = marked();
        let mut stale = current();
        stale.current = false;
        stale.input_incomplete = true;
        for target in [Target::InPlace, Target::InboxOrSource, Target::Elsewhere] {
            assert_eq!(clear_reason(&f, &stale, target, false), None, "{target:?}");
        }
        assert_eq!(
            clear_reason(&f, &current(), Target::InPlace, false),
            Some("in_place")
        );
        assert_eq!(
            clear_reason(&f, &current(), Target::InboxOrSource, false),
            Some("target_inbox_or_source")
        );
        let mut partial = current();
        partial.input_incomplete = true;
        assert_eq!(
            clear_reason(&f, &partial, Target::Elsewhere, false),
            Some("incomplete_input")
        );
        assert_eq!(clear_reason(&f, &current(), Target::Elsewhere, false), None);
        assert_eq!(clear_reason(&f, &current(), Target::Unknown, false), None);
    }
}
