//! Refile spec "Intents": a refile intent is checked again at claim time and
//! before every retry once recovery established that no earlier dispatch
//! applied; a failed check closes it `superseded` (event `refile_cancelled`).
use crate::filing::apply::{close, commit, event};
use crate::filing::observe::FolderMap;
use crate::filing::planner::Locator;
use crate::filing::refile::rules::{self, Verdict};
use crate::filing::{inputs, Intent, PassContext};
use crate::store::Store;
use anyhow::Result;
use serde_json::json;

/// Why `intent` may no longer move its message from `source` (the validated
/// source occurrence), or `None` while every candidate rule holds (the
/// intent itself not counting as an open intent), the source is the
/// journaled one and the current classification still files the message
/// into the intent's target.
pub fn recheck(
    store: &Store,
    ctx: &PassContext,
    map: &FolderMap,
    intent: &Intent,
    source: &Locator,
) -> Result<Option<&'static str>> {
    let journaled = (intent.folder.as_str(), intent.epoch, intent.uid);
    if journaled != (source.folder.as_str(), source.epoch, source.uid) {
        return Ok(Some("source_changed"));
    }
    let input = inputs::message_input(store, ctx, map, &intent.message_id)?;
    let gone = rules::gone(store, ctx.account, &map.listed)?;
    let refile = rules::input_for(
        store,
        ctx.account,
        ctx.cfg,
        &gone,
        &intent.message_id,
        intent.id,
    )?;
    let Some(m) = input.messages.first() else {
        return Ok(Some("not_filed_by_mailtriage"));
    };
    if m.home.as_ref() != Some(source) {
        return Ok(Some("source_changed"));
    }
    Ok(match rules::verdict(&input, m, &refile) {
        Verdict::Candidate(c) if intent.target.as_deref() == Some(c.target.as_str()) => None,
        Verdict::Candidate(_) => Some("target_changed"),
        Verdict::Waiting => Some("waiting"),
        Verdict::InPlace => Some("in_place"),
        Verdict::Skipped(skip) => Some(skip.as_str()),
        Verdict::OutOfScope => Some("not_filed_by_mailtriage"),
    })
}

/// Closes `intent` as `superseded` with event `refile_cancelled`, in one
/// transaction; the next pass's planning may create a new one.
pub fn cancel(store: &mut Store, ctx: &PassContext, intent: &Intent, reason: &str) -> Result<()> {
    let detail = json!({"intent_id": intent.id, "reason": reason});
    commit(
        store,
        ctx,
        &[
            close(intent.id, "superseded", None),
            event(
                Some(&intent.message_id),
                Some(&intent.folder),
                "refile_cancelled",
                detail,
            ),
        ],
    )
}
