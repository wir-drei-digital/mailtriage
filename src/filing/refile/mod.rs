//! Refiling filed mail after category changes (refile spec,
//! `docs/superpowers/specs/2026-10-06-filing-refile-design.md`).
pub mod rules;

use super::observe::FolderMap;
use super::planner::{self, Plan};
use super::{inputs, PassContext};
use crate::store::Store;
use anyhow::Result;

/// Step 9's plan: the planner input and every placement's refile facts;
/// marked messages get the refile rule.
pub fn plan_pass(store: &Store, ctx: &PassContext, map: &FolderMap, preview: bool) -> Result<Plan> {
    let input = inputs::plan_input(store, ctx, map, preview)?;
    let gone = rules::gone(store, ctx.account, &map.listed)?;
    let refile = rules::input(store, ctx.account, ctx.cfg, &gone)?;
    Ok(planner::plan_with_refile(&input, &refile))
}
