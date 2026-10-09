//! Pure filing planner (spec "Planner"): local state in, actions out.
use super::refile::rules::{self, RefileInput, Verdict};
use crate::domain::{FilingMode, Urgency};
use chrono::{DateTime, Utc};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Locator {
    pub folder: String,
    pub epoch: u64,
    pub uid: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    Move {
        message_id: String,
        from: Locator,
        to: String,
        desired_rev: i64,
        consumes_eligible: bool,
    },
    Flag {
        message_id: String,
        at: Locator,
    },
}
impl Action {
    pub fn message_id(&self) -> &str {
        match self {
            Action::Move { message_id, .. } | Action::Flag { message_id, .. } => message_id,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Plan {
    pub folders_to_create: Vec<String>,
    pub actions: Vec<Action>,
    /// Satisfied explicit requests: (message id, desired_rev) to clear.
    pub cleared_requests: Vec<(String, i64)>,
    /// Flag-eligible messages that already carry `\Flagged`: their one flag
    /// attempt is consumed without an engine call.
    pub satisfied_flags: Vec<String>,
    /// Refile spec: messages whose `Move` consumes their refile mark.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub refile_moves: BTreeSet<String>,
    /// Reply queue spec: messages whose `Move` is a reply exit: moved
    /// unread, entering the read approval list.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub reply_exits: BTreeSet<String>,
    /// Reply queue spec: messages held in their source folder until they
    /// are answered or marked done.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub awaiting_reply: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderUse {
    Ok,
    WouldCreate,
    Unusable,
}

#[derive(Debug, Clone)]
pub struct FolderView {
    pub usable: FolderUse,
    pub paused: bool,
    pub is_source: bool,
    pub is_category: bool,
    pub retired_listed: bool,
    /// The epoch of the folder's discovery checkpoint; `None` until a
    /// discovery established one (a failed first scan does not).
    pub epoch: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CategoryFolder {
    Inbox,
    Native(String),
}

#[derive(Debug, Clone, Default)]
pub struct Effective {
    pub category_id: Option<String>,
    pub category_from_override: bool,
    pub urgency: Option<Urgency>,
    pub urgency_from_override: bool,
    pub action_required: Option<bool>,
    pub action_from_override: bool,
    /// Generation is current and status is ready or uncertain.
    pub current: bool,
    pub input_incomplete: bool,
}

#[derive(Debug, Clone)]
pub struct PlanMessage {
    pub message_id: String,
    pub source_folder: String,
    pub home: Option<Locator>,
    pub location_known: bool,
    pub hydrated: bool,
    pub internal_date: Option<DateTime<Utc>>,
    pub flags: Vec<String>,
    pub desired_target: Option<String>,
    pub pinned: bool,
    pub eligible_once: bool,
    pub desired_rev: i64,
    pub filed_at: Option<String>,
    pub flag_attempted: bool,
    pub blocked: bool,
    pub open_move_intent: bool,
    pub open_flag_intent: bool,
    /// The user marked the message done (`review_state` `done`).
    pub done: bool,
    pub effective: Effective,
}

#[derive(Debug, Clone)]
pub struct PlanInput {
    pub mode: FilingMode,
    /// `filing plan` and dry_run passes: folders that would be created count as usable.
    pub preview: bool,
    pub flag_enabled: bool,
    /// Reply queue spec: `filing.reply_queue`.
    pub reply_queue: bool,
    pub max_actions: usize,
    pub enabled_at: Option<DateTime<Utc>>,
    pub categories: BTreeMap<String, CategoryFolder>,
    pub folders: BTreeMap<String, FolderView>,
    pub messages: Vec<PlanMessage>,
}

enum MoveDecision {
    Move(Action),
    /// A move that consumes the message's refile mark.
    Refile(Action),
    /// A held message leaves its source folder, unread, for read approval.
    ReplyExit(Action),
    /// A held message stays in its source folder.
    Held,
    Clear,
    Nothing,
}

enum FlagDecision {
    Flag(Action),
    /// Everything holds, but the message is already flagged.
    Satisfied,
    Nothing,
}

pub fn plan(input: &PlanInput) -> Plan {
    plan_with_refile(input, &RefileInput::default())
}

/// `plan` plus the refile rule (refile spec "Each pass", Planning): a marked
/// message whose candidate rules hold moves to its effective category's
/// folder, after the explicit-target rule and before the source-folder rule.
pub fn plan_with_refile(input: &PlanInput, refile: &RefileInput) -> Plan {
    let mut out = Plan::default();
    if input.mode == FilingMode::Off {
        return out;
    }
    if input.preview {
        out.folders_to_create = input
            .folders
            .iter()
            .filter(|(_, f)| f.is_category && f.usable == FolderUse::WouldCreate)
            .map(|(name, _)| name.clone())
            .collect();
    }
    let mut per_message = Vec::new();
    for m in &input.messages {
        let Some((home, home_view)) = eligible_home(input, m) else {
            continue;
        };
        let mut actions = Vec::new();
        match flag_action(input, m, home, home_view) {
            FlagDecision::Flag(flag) => actions.push(flag),
            FlagDecision::Satisfied => out.satisfied_flags.push(m.message_id.clone()),
            FlagDecision::Nothing => {}
        }
        match move_action(input, m, home, home_view, refile) {
            MoveDecision::Move(a) => actions.push(a),
            MoveDecision::Refile(a) => {
                out.refile_moves.insert(m.message_id.clone());
                actions.push(a);
            }
            MoveDecision::ReplyExit(a) => {
                out.reply_exits.insert(m.message_id.clone());
                actions.push(a);
            }
            MoveDecision::Held => out.awaiting_reply.push(m.message_id.clone()),
            MoveDecision::Clear => out
                .cleared_requests
                .push((m.message_id.clone(), m.desired_rev)),
            MoveDecision::Nothing => {}
        }
        if !actions.is_empty() {
            per_message.push((m.internal_date, m.message_id.as_str(), actions));
        }
    }
    per_message.sort_by(|a, b| {
        let by_date = match (a.0, b.0) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        };
        by_date.then_with(|| a.1.cmp(b.1))
    });
    out.actions = per_message
        .into_iter()
        .flat_map(|(_, _, a)| a)
        .take(input.max_actions)
        .collect();
    let kept: BTreeSet<&str> = out
        .actions
        .iter()
        .filter(|a| matches!(a, Action::Move { .. }))
        .map(Action::message_id)
        .collect();
    out.refile_moves.retain(|id| kept.contains(id.as_str()));
    out.reply_exits.retain(|id| kept.contains(id.as_str()));
    out
}

/// The message's home and its folder view, when any action may touch it.
fn eligible_home<'i, 'm>(
    input: &'i PlanInput,
    m: &'m PlanMessage,
) -> Option<(&'m Locator, &'i FolderView)> {
    if !m.location_known || !m.hydrated || m.blocked {
        return None;
    }
    let home = m.home.as_ref()?;
    let view = input.folders.get(&home.folder)?;
    if view.paused || view.epoch != Some(home.epoch) {
        return None;
    }
    Some((home, view))
}

fn is_new(input: &PlanInput, m: &PlanMessage) -> bool {
    m.filed_at.is_none()
        && matches!((m.internal_date, input.enabled_at), (Some(d), Some(on)) if d >= on)
}

/// Reply queue spec "Entry": with the queue on, new mail whose effective
/// decision is action required (from an override or a current
/// classification) is held in its source folder. Backfilled mail is not.
pub(crate) fn holds(input: &PlanInput, m: &PlanMessage) -> bool {
    let e = &m.effective;
    input.reply_queue
        && is_new(input, m)
        && e.action_required == Some(true)
        && (e.action_from_override || e.current)
}

/// Reply queue spec "Exit": the server reports `\Answered`, or the user
/// marked the message done.
pub(crate) fn reply_done(m: &PlanMessage) -> bool {
    m.done || m.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Answered"))
}

fn flag_action(
    input: &PlanInput,
    m: &PlanMessage,
    home: &Locator,
    home_view: &FolderView,
) -> FlagDecision {
    if !input.flag_enabled || m.flag_attempted || m.open_flag_intent {
        return FlagDecision::Nothing;
    }
    // Flags only touch a source folder or a category folder in state `ok`.
    if !(home_view.is_source || (home_view.is_category && home_view.usable == FolderUse::Ok)) {
        return FlagDecision::Nothing;
    }
    if !(is_new(input, m) || m.eligible_once || m.filed_at.is_some()) {
        return FlagDecision::Nothing;
    }
    let e = &m.effective;
    // A held message's source folder already shows that it needs action;
    // only high urgency flags it. Mail the queue does not hold is flagged
    // as without it.
    let action = e.action_required == Some(true)
        && (e.action_from_override || e.current)
        && !holds(input, m);
    let urgent = e.urgency == Some(Urgency::High) && (e.urgency_from_override || e.current);
    if !(action || urgent) {
        return FlagDecision::Nothing;
    }
    if m.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Flagged")) {
        return FlagDecision::Satisfied;
    }
    FlagDecision::Flag(Action::Flag {
        message_id: m.message_id.clone(),
        at: home.clone(),
    })
}

fn resolve_target(input: &PlanInput, m: &PlanMessage, target: &str) -> Option<String> {
    if target == "@source" {
        return Some(m.source_folder.clone());
    }
    match input.categories.get(target)? {
        CategoryFolder::Inbox => Some(m.source_folder.clone()),
        CategoryFolder::Native(name) => Some(name.clone()),
    }
}

/// A target takes moves only once its discovery is established (`epoch`):
/// a move into a folder without a checkpoint could land below its first
/// watch and never be discovered. A preview also counts a folder it would
/// create.
pub(crate) fn target_usable(input: &PlanInput, folder: &str) -> bool {
    match input.folders.get(folder) {
        Some(v) if !v.paused => match v.usable {
            FolderUse::WouldCreate => input.preview,
            _ if v.epoch.is_none() => false,
            FolderUse::Ok => true,
            FolderUse::Unusable => v.is_source,
        },
        _ => false,
    }
}

fn move_action(
    input: &PlanInput,
    m: &PlanMessage,
    home: &Locator,
    home_view: &FolderView,
    refile: &RefileInput,
) -> MoveDecision {
    if m.open_move_intent {
        return MoveDecision::Nothing;
    }
    if let Some(target) = &m.desired_target {
        let Some(to) = resolve_target(input, m, target) else {
            return MoveDecision::Nothing;
        };
        if to == home.folder {
            return MoveDecision::Clear;
        }
        let home_ok = home_view.is_source
            || (home_view.is_category && home_view.usable == FolderUse::Ok)
            || home_view.retired_listed;
        if !home_ok || !target_usable(input, &to) {
            return MoveDecision::Nothing;
        }
        return MoveDecision::Move(Action::Move {
            message_id: m.message_id.clone(),
            from: home.clone(),
            to,
            desired_rev: m.desired_rev,
            consumes_eligible: false,
        });
    }
    // Refile: a marked message outside the source folders, with the home an
    // explicit move needs; it waits while any candidate rule fails.
    let marked = refile.facts.get(&m.message_id).is_some_and(|f| f.marked);
    if marked && !home_view.is_source {
        let home_ok = (home_view.is_category && home_view.usable == FolderUse::Ok)
            || home_view.retired_listed;
        return match rules::verdict(input, m, refile) {
            Verdict::Candidate(c) if home_ok => MoveDecision::Refile(Action::Move {
                message_id: m.message_id.clone(),
                from: home.clone(),
                to: c.target,
                desired_rev: m.desired_rev,
                consumes_eligible: false,
            }),
            _ => MoveDecision::Nothing,
        };
    }
    if !home_view.is_source || m.pinned {
        return MoveDecision::Nothing;
    }
    let held = holds(input, m);
    if held && !reply_done(m) {
        return MoveDecision::Held;
    }
    let e = &m.effective;
    let Some(category) = &e.category_id else {
        return MoveDecision::Nothing;
    };
    if !e.category_from_override && (!e.current || e.input_incomplete) {
        return MoveDecision::Nothing;
    }
    let Some(CategoryFolder::Native(to)) = input.categories.get(category) else {
        return MoveDecision::Nothing;
    };
    if !(is_new(input, m) || m.eligible_once) || *to == home.folder || !target_usable(input, to) {
        return MoveDecision::Nothing;
    }
    let action = Action::Move {
        message_id: m.message_id.clone(),
        from: home.clone(),
        to: to.clone(),
        desired_rev: m.desired_rev,
        consumes_eligible: m.eligible_once,
    };
    if held {
        MoveDecision::ReplyExit(action)
    } else {
        MoveDecision::Move(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 4, h, 0, 0).unwrap()
    }
    fn folders() -> BTreeMap<String, FolderView> {
        let mut f = BTreeMap::new();
        f.insert(
            "INBOX".into(),
            FolderView {
                usable: FolderUse::Ok,
                paused: false,
                is_source: true,
                is_category: false,
                retired_listed: false,
                epoch: Some(1),
            },
        );
        f.insert(
            "Newsletters".into(),
            FolderView {
                usable: FolderUse::Ok,
                paused: false,
                is_source: false,
                is_category: true,
                retired_listed: false,
                epoch: Some(2),
            },
        );
        f.insert(
            "Transactions".into(),
            FolderView {
                usable: FolderUse::Ok,
                paused: false,
                is_source: false,
                is_category: true,
                retired_listed: false,
                epoch: Some(3),
            },
        );
        f
    }
    fn input(messages: Vec<PlanMessage>) -> PlanInput {
        PlanInput {
            mode: FilingMode::Live,
            preview: false,
            flag_enabled: true,
            reply_queue: false,
            max_actions: 200,
            enabled_at: Some(t(8)),
            categories: BTreeMap::from([
                (
                    "newsletters".to_string(),
                    CategoryFolder::Native("Newsletters".into()),
                ),
                (
                    "transactions".to_string(),
                    CategoryFolder::Native("Transactions".into()),
                ),
                ("correspondence".to_string(), CategoryFolder::Inbox),
            ]),
            folders: folders(),
            messages,
        }
    }
    fn msg(id: &str, category: &str) -> PlanMessage {
        PlanMessage {
            message_id: id.into(),
            source_folder: "INBOX".into(),
            home: Some(Locator {
                folder: "INBOX".into(),
                epoch: 1,
                uid: 10,
            }),
            location_known: true,
            hydrated: true,
            internal_date: Some(t(9)),
            flags: vec![],
            desired_target: None,
            pinned: false,
            eligible_once: false,
            desired_rev: 0,
            filed_at: None,
            flag_attempted: false,
            blocked: false,
            open_move_intent: false,
            open_flag_intent: false,
            done: false,
            effective: Effective {
                category_id: Some(category.into()),
                current: true,
                urgency: Some(Urgency::Low),
                action_required: Some(false),
                ..Default::default()
            },
        }
    }
    fn moves(p: &Plan) -> Vec<(String, String)> {
        p.actions
            .iter()
            .filter_map(|a| match a {
                Action::Move { message_id, to, .. } => Some((message_id.clone(), to.clone())),
                _ => None,
            })
            .collect()
    }
    fn flags(p: &Plan) -> Vec<String> {
        p.actions
            .iter()
            .filter_map(|a| match a {
                Action::Flag { message_id, .. } => Some(message_id.clone()),
                _ => None,
            })
            .collect()
    }

    /// Reply queue spec: a message that needs action, in the queue.
    fn needs_reply(id: &str, category: &str) -> PlanMessage {
        let mut m = msg(id, category);
        m.effective.action_required = Some(true);
        m
    }
    fn queued(messages: Vec<PlanMessage>) -> PlanInput {
        let mut i = input(messages);
        i.reply_queue = true;
        i
    }

    #[test]
    fn reply_queue_holds_new_mail_that_needs_action() {
        let p = plan(&queued(vec![
            needs_reply("ask", "transactions"),
            msg("fyi", "newsletters"),
        ]));
        assert_eq!(moves(&p), vec![("fyi".into(), "Newsletters".into())]);
        assert_eq!(p.awaiting_reply, vec!["ask".to_string()]);
        assert!(p.reply_exits.is_empty());
        // Without the queue the same message moves (and is flagged) as before.
        let p = plan(&input(vec![needs_reply("ask", "transactions")]));
        assert_eq!(moves(&p), vec![("ask".into(), "Transactions".into())]);
        assert_eq!(flags(&p), vec!["ask".to_string()]);
        assert!(p.awaiting_reply.is_empty());
    }

    #[test]
    fn an_answered_held_message_leaves_with_a_reply_exit() {
        let mut answered = needs_reply("ask", "transactions");
        answered.flags = vec!["\\Seen".into(), "\\Answered".into()];
        let p = plan(&queued(vec![answered]));
        assert_eq!(moves(&p), vec![("ask".into(), "Transactions".into())]);
        assert_eq!(p.reply_exits, BTreeSet::from(["ask".to_string()]));
        assert!(p.awaiting_reply.is_empty());
    }

    #[test]
    fn marking_a_held_message_done_is_a_reply_exit() {
        let mut done = needs_reply("bill", "transactions");
        done.done = true;
        let p = plan(&queued(vec![done]));
        assert_eq!(moves(&p), vec![("bill".into(), "Transactions".into())]);
        assert!(p.reply_exits.contains("bill"));
    }

    #[test]
    fn the_reply_queue_flags_held_mail_only_for_high_urgency() {
        let mut urgent = needs_reply("urgent", "transactions");
        urgent.effective.urgency = Some(Urgency::High);
        let p = plan(&queued(vec![needs_reply("ask", "transactions"), urgent]));
        assert_eq!(flags(&p), vec!["urgent".to_string()]);
    }

    #[test]
    fn the_reply_queue_flags_mail_it_does_not_hold_as_before() {
        let mut backfill = needs_reply("backfill", "transactions");
        backfill.internal_date = Some(t(7));
        backfill.eligible_once = true;
        let mut filed = needs_reply("filed", "transactions");
        filed.home = Some(Locator {
            folder: "Transactions".into(),
            epoch: 3,
            uid: 4,
        });
        filed.filed_at = Some("2026-10-04T10:00:00+00:00".into());
        let held = needs_reply("held", "transactions");
        let messages = vec![backfill, filed, held];
        let p = plan(&queued(messages.clone()));
        assert_eq!(flags(&p), vec!["backfill".to_string(), "filed".into()]);
        assert_eq!(p.awaiting_reply, vec!["held".to_string()]);
        let p = plan(&input(messages));
        assert_eq!(flags(&p).len(), 3, "without the queue all three");
    }

    #[test]
    fn the_reply_queue_holds_only_new_current_unpinned_mail() {
        let mut old = needs_reply("old", "transactions");
        old.internal_date = Some(t(7));
        let mut backfill = needs_reply("backfill", "transactions");
        backfill.internal_date = Some(t(7));
        backfill.eligible_once = true;
        let mut stale = needs_reply("stale", "transactions");
        stale.effective.current = false;
        let mut pinned = needs_reply("pinned", "transactions");
        pinned.pinned = true;
        pinned.flags = vec!["\\Answered".into()];
        let mut overridden = msg("override", "transactions");
        overridden.effective.current = false;
        overridden.effective.category_from_override = true;
        overridden.effective.action_required = Some(true);
        overridden.effective.action_from_override = true;
        let p = plan(&queued(vec![old, backfill, stale, pinned, overridden]));
        // Old mail is left alone, backfilled mail files as before, a stale
        // classification and a pin decide nothing, an override holds.
        assert_eq!(moves(&p), vec![("backfill".into(), "Transactions".into())]);
        assert!(p.reply_exits.is_empty());
        assert_eq!(p.awaiting_reply, vec!["override".to_string()]);
    }

    #[test]
    fn a_held_message_whose_decision_changes_files_without_seen() {
        let p = plan(&queued(vec![msg("ask", "transactions")]));
        assert_eq!(moves(&p), vec![("ask".into(), "Transactions".into())]);
        assert!(p.reply_exits.is_empty());
    }

    #[test]
    fn an_answered_held_message_in_an_inbox_category_stays() {
        let mut answered = needs_reply("ask", "correspondence");
        answered.flags = vec!["\\Answered".into()];
        let p = plan(&queued(vec![answered]));
        assert!(moves(&p).is_empty());
        assert!(p.reply_exits.is_empty());
    }

    #[test]
    fn new_mail_moves_to_category_folder() {
        let p = plan(&input(vec![msg("a", "newsletters")]));
        assert_eq!(moves(&p), vec![("a".into(), "Newsletters".into())]);
    }

    #[test]
    fn off_mode_plans_nothing() {
        let mut i = input(vec![msg("a", "newsletters")]);
        i.mode = FilingMode::Off;
        assert!(plan(&i).actions.is_empty());
    }

    #[test]
    fn inbox_category_stays_and_old_mail_is_not_new() {
        let mut old = msg("old", "newsletters");
        old.internal_date = Some(t(7));
        let mut undated = msg("undated", "newsletters");
        undated.internal_date = None;
        let p = plan(&input(vec![msg("c", "correspondence"), old, undated]));
        assert!(moves(&p).is_empty());
    }

    #[test]
    fn eligible_once_files_old_mail_and_is_consumed() {
        let mut old = msg("old", "newsletters");
        old.internal_date = Some(t(7));
        old.eligible_once = true;
        let p = plan(&input(vec![old]));
        assert!(matches!(
            &p.actions[0],
            Action::Move {
                consumes_eligible: true,
                ..
            }
        ));
    }

    #[test]
    fn already_filed_pinned_blocked_intent_or_unhydrated_do_not_move() {
        let mut filed = msg("filed", "newsletters");
        filed.filed_at = Some("x".into());
        let mut pinned = msg("pinned", "newsletters");
        pinned.pinned = true;
        let mut blocked = msg("blocked", "newsletters");
        blocked.blocked = true;
        let mut busy = msg("busy", "newsletters");
        busy.open_move_intent = true;
        let mut raw = msg("raw", "newsletters");
        raw.hydrated = false;
        let mut lost = msg("lost", "newsletters");
        lost.location_known = false;
        let p = plan(&input(vec![filed, pinned, blocked, busy, raw, lost]));
        assert!(moves(&p).is_empty());
    }

    #[test]
    fn stale_generation_and_incomplete_input_block_automatic_moves_unless_overridden() {
        let mut stale = msg("stale", "newsletters");
        stale.effective.current = false;
        let mut partial = msg("partial", "newsletters");
        partial.effective.input_incomplete = true;
        let mut corrected = msg("corrected", "transactions");
        corrected.effective.current = false;
        corrected.effective.input_incomplete = true;
        corrected.effective.category_from_override = true;
        let p = plan(&input(vec![stale, partial, corrected]));
        assert_eq!(moves(&p), vec![("corrected".into(), "Transactions".into())]);
    }

    #[test]
    fn explicit_request_moves_from_category_folder_and_clears_when_satisfied() {
        let mut m = msg("m", "newsletters");
        m.home = Some(Locator {
            folder: "Newsletters".into(),
            epoch: 2,
            uid: 5,
        });
        m.filed_at = Some("x".into());
        m.desired_target = Some("transactions".into());
        m.desired_rev = 3;
        let mut back = msg("back", "newsletters");
        back.home = Some(Locator {
            folder: "Newsletters".into(),
            epoch: 2,
            uid: 6,
        });
        back.desired_target = Some("@source".into());
        let mut done = msg("done", "newsletters");
        done.home = Some(Locator {
            folder: "Transactions".into(),
            epoch: 3,
            uid: 7,
        });
        done.desired_target = Some("transactions".into());
        done.desired_rev = 4;
        let p = plan(&input(vec![m, back, done]));
        assert_eq!(
            moves(&p),
            vec![
                ("back".into(), "INBOX".into()),
                ("m".into(), "Transactions".into())
            ]
        );
        assert!(p.actions.iter().any(|a| matches!(a, Action::Move { message_id, desired_rev: 3, consumes_eligible: false, .. } if message_id == "m")));
        assert_eq!(p.cleared_requests, vec![("done".to_string(), 4)]);
    }

    #[test]
    fn paused_or_unusable_folders_block_actions() {
        let mut i = input(vec![msg("a", "newsletters"), msg("b", "transactions")]);
        i.folders.get_mut("Newsletters").unwrap().paused = true;
        i.folders.get_mut("Transactions").unwrap().usable = FolderUse::Unusable;
        assert!(moves(&plan(&i)).is_empty());
        let mut i = input(vec![msg("a", "newsletters")]);
        i.folders.get_mut("INBOX").unwrap().paused = true;
        assert!(plan(&i).actions.is_empty());
    }

    #[test]
    fn epoch_mismatch_blocks_actions() {
        let mut m = msg("a", "newsletters");
        m.home.as_mut().unwrap().epoch = 99;
        assert!(plan(&input(vec![m])).actions.is_empty());
    }

    #[test]
    fn preview_counts_would_create_folders() {
        let mut i = input(vec![msg("a", "newsletters")]);
        i.folders.get_mut("Newsletters").unwrap().usable = FolderUse::WouldCreate;
        assert!(moves(&plan(&i)).is_empty());
        i.preview = true;
        let p = plan(&i);
        assert_eq!(moves(&p).len(), 1);
        assert_eq!(p.folders_to_create, vec!["Newsletters".to_string()]);
    }

    #[test]
    fn flags_for_action_or_high_urgency_once() {
        let mut act = msg("act", "correspondence");
        act.effective.action_required = Some(true);
        let mut hot = msg("hot", "newsletters");
        hot.effective.urgency = Some(Urgency::High);
        let mut tried = msg("tried", "correspondence");
        tried.effective.action_required = Some(true);
        tried.flag_attempted = true;
        let mut has = msg("has", "correspondence");
        has.effective.action_required = Some(true);
        has.flags = vec!["\\Flagged".into()];
        let mut stale = msg("stale", "correspondence");
        stale.effective.action_required = Some(true);
        stale.effective.current = false;
        let mut forced = msg("forced", "correspondence");
        forced.effective.action_required = Some(true);
        forced.effective.current = false;
        forced.effective.action_from_override = true;
        let p = plan(&input(vec![act, hot, tried, has, stale, forced]));
        assert_eq!(
            flags(&p),
            vec!["act".to_string(), "forced".into(), "hot".into()]
        );
        assert_eq!(p.satisfied_flags, vec!["has".to_string()]);
        let mut i = input(vec![msg("x", "correspondence")]);
        i.messages[0].effective.action_required = Some(true);
        i.flag_enabled = false;
        assert!(flags(&plan(&i)).is_empty());
    }

    #[test]
    fn backlog_is_not_flagged_until_in_scope() {
        let mut old = msg("old", "correspondence");
        old.internal_date = Some(t(7));
        old.effective.action_required = Some(true);
        let mut backfilled = old.clone();
        backfilled.message_id = "backfilled".into();
        backfilled.eligible_once = true;
        let mut filed = old.clone();
        filed.message_id = "filed".into();
        filed.filed_at = Some("x".into());
        let p = plan(&input(vec![old, backfilled, filed]));
        assert_eq!(flags(&p), vec!["backfilled".to_string(), "filed".into()]);
    }

    #[test]
    fn ordering_flags_first_oldest_first_and_cap() {
        let mut a = msg("a", "transactions");
        a.internal_date = Some(t(11));
        a.effective.action_required = Some(true);
        let mut b = msg("b", "newsletters");
        b.internal_date = Some(t(10));
        let mut i = input(vec![a, b]);
        let p = plan(&i);
        let order: Vec<_> = p
            .actions
            .iter()
            .map(|x| match x {
                Action::Move { message_id, .. } => format!("m:{message_id}"),
                Action::Flag { message_id, .. } => format!("f:{message_id}"),
            })
            .collect();
        assert_eq!(order, vec!["m:b", "f:a", "m:a"]);
        i.max_actions = 2;
        assert_eq!(plan(&i).actions.len(), 2);
    }

    /// A non-source folder view: a category folder, or a retired one.
    fn view(usable: FolderUse, is_category: bool, retired_listed: bool, epoch: u64) -> FolderView {
        FolderView {
            usable,
            paused: false,
            is_source: false,
            is_category,
            retired_listed,
            epoch: Some(epoch),
        }
    }
    fn homed(mut m: PlanMessage, folder: &str, epoch: u64) -> PlanMessage {
        m.home = Some(Locator {
            folder: folder.into(),
            epoch,
            uid: 1,
        });
        m
    }

    #[test]
    fn flags_require_a_source_or_ok_category_home() {
        let mut i = input(vec![]);
        i.folders
            .insert("Broken".into(), view(FolderUse::Unusable, true, false, 4));
        // Retired (not a category): no flag even though LIST reports it and `usable` says Ok.
        i.folders
            .insert("Old".into(), view(FolderUse::Ok, false, true, 5));
        for (id, folder, epoch) in [
            ("ok", "Newsletters", 2),
            ("broken", "Broken", 4),
            ("old", "Old", 5),
        ] {
            let mut m = homed(msg(id, "correspondence"), folder, epoch);
            m.filed_at = Some("x".into());
            m.effective.action_required = Some(true);
            i.messages.push(m);
        }
        assert_eq!(flags(&plan(&i)), vec!["ok".to_string()]);
    }

    #[test]
    fn automatic_moves_only_leave_source_folders() {
        let new = homed(msg("new", "transactions"), "Newsletters", 2);
        let mut backfill = homed(msg("backfill", "transactions"), "Newsletters", 2);
        backfill.internal_date = Some(t(7));
        backfill.eligible_once = true;
        let p = plan(&input(vec![new, backfill]));
        assert!(p.actions.is_empty());
        assert!(p.cleared_requests.is_empty());
    }

    #[test]
    fn explicit_request_home_gate() {
        let mut i = input(vec![]);
        i.folders
            .insert("Broken".into(), view(FolderUse::Unusable, true, false, 4));
        // Retired folders: only `retired_listed` admits them, whatever `usable` says.
        i.folders
            .insert("Listed".into(), view(FolderUse::Unusable, false, true, 5));
        i.folders
            .insert("Unlisted".into(), view(FolderUse::Ok, false, false, 6));
        for (id, folder, epoch) in [
            ("broken", "Broken", 4),
            ("listed", "Listed", 5),
            ("unlisted", "Unlisted", 6),
        ] {
            let mut m = homed(msg(id, "newsletters"), folder, epoch);
            m.filed_at = Some("x".into());
            m.desired_target = Some("transactions".into());
            i.messages.push(m);
        }
        let p = plan(&i);
        assert_eq!(moves(&p), vec![("listed".into(), "Transactions".into())]);
        assert!(p.cleared_requests.is_empty());
    }

    #[test]
    fn open_move_intent_blocks_explicit_move_and_clear() {
        let mut busy_move = msg("busy_move", "newsletters");
        busy_move.pinned = true;
        busy_move.desired_target = Some("transactions".into());
        busy_move.open_move_intent = true;
        let mut busy_clear = homed(msg("busy_clear", "newsletters"), "Transactions", 3);
        busy_clear.pinned = true;
        busy_clear.desired_target = Some("transactions".into());
        busy_clear.open_move_intent = true;
        let p = plan(&input(vec![busy_move, busy_clear]));
        assert!(moves(&p).is_empty());
        assert!(p.cleared_requests.is_empty());
    }

    #[test]
    fn explicit_inbox_category_resolves_to_source_folder() {
        let mut i = input(vec![]);
        i.folders.insert(
            "Lists".into(),
            FolderView {
                usable: FolderUse::Ok,
                paused: false,
                is_source: true,
                is_category: false,
                retired_listed: false,
                epoch: Some(7),
            },
        );
        let mut back = homed(msg("back", "newsletters"), "Newsletters", 2);
        back.filed_at = Some("x".into());
        back.desired_target = Some("correspondence".into());
        back.desired_rev = 5;
        let mut back_lists = back.clone();
        back_lists.message_id = "back_lists".into();
        back_lists.source_folder = "Lists".into();
        let mut stay = msg("stay", "newsletters");
        stay.desired_target = Some("correspondence".into());
        stay.desired_rev = 6;
        i.messages = vec![back, back_lists, stay];
        let p = plan(&i);
        assert_eq!(
            moves(&p),
            vec![
                ("back".into(), "INBOX".into()),
                ("back_lists".into(), "Lists".into())
            ]
        );
        assert_eq!(p.cleared_requests, vec![("stay".to_string(), 6)]);
    }

    #[test]
    fn explicit_request_overrides_automatic_from_source_home() {
        let mut m = msg("m", "newsletters");
        m.desired_target = Some("transactions".into());
        m.desired_rev = 2;
        let p = plan(&input(vec![m]));
        assert_eq!(moves(&p), vec![("m".into(), "Transactions".into())]);
        assert!(matches!(
            &p.actions[0],
            Action::Move {
                desired_rev: 2,
                consumes_eligible: false,
                ..
            }
        ));
    }

    /// Final review C1(a): a target whose discovery is not established in its
    /// current epoch (`epoch` None) takes no move, automatic or explicit; a
    /// preview still counts a folder it would create.
    #[test]
    fn a_target_without_established_discovery_takes_no_move() {
        let mut i = input(vec![msg("a", "newsletters"), msg("b", "transactions")]);
        i.folders.get_mut("Newsletters").unwrap().epoch = None;
        assert_eq!(moves(&plan(&i)), vec![("b".into(), "Transactions".into())]);
        let mut i = input(vec![]);
        i.folders.insert(
            "Lists".into(),
            FolderView {
                usable: FolderUse::Ok,
                paused: false,
                is_source: true,
                is_category: false,
                retired_listed: false,
                epoch: None,
            },
        );
        let mut back = homed(msg("back", "newsletters"), "Newsletters", 2);
        back.source_folder = "Lists".into();
        back.desired_target = Some("@source".into());
        i.messages.push(back);
        assert!(moves(&plan(&i)).is_empty(), "a source target needs it too");
        let mut i = input(vec![msg("a", "newsletters")]);
        let news = i.folders.get_mut("Newsletters").unwrap();
        news.usable = FolderUse::WouldCreate;
        news.epoch = None;
        i.preview = true;
        assert_eq!(moves(&plan(&i)), vec![("a".into(), "Newsletters".into())]);
    }

    #[test]
    fn folders_to_create_are_listed_only_in_preview() {
        let mut i = input(vec![msg("a", "newsletters")]);
        i.folders.get_mut("Newsletters").unwrap().usable = FolderUse::WouldCreate;
        assert!(plan(&i).folders_to_create.is_empty());
        i.mode = FilingMode::DryRun;
        assert!(plan(&i).folders_to_create.is_empty());
        i.preview = true;
        assert_eq!(plan(&i).folders_to_create, vec!["Newsletters".to_string()]);
    }

    /// Refile spec "Candidates" and the planner's refile rule.
    mod refile {
        use super::{flags, homed, input, moves, msg, t, view};
        use crate::filing::planner::{
            plan, plan_with_refile, Action, FolderUse, PlanInput, PlanMessage,
        };
        use crate::filing::refile::rules::{
            verdict, Candidate, Reason, RefileFacts, RefileInput, Skip, Verdict,
        };
        use std::collections::BTreeSet;

        /// Filed by mailtriage into Newsletters; `category` is its current classification.
        fn filed(id: &str, category: &str) -> PlanMessage {
            let mut m = homed(msg(id, category), "Newsletters", 2);
            m.filed_at = Some("x".into());
            m
        }

        /// Marked, at its filed home, every placement rule holding.
        fn facts(m: &PlanMessage) -> RefileFacts {
            RefileFacts {
                marked: true,
                at_filed_home: true,
                home_folder: m.home.as_ref().map(|h| h.folder.clone()),
                single_occurrence: true,
                ..Default::default()
            }
        }

        /// `updates` is configured but missing from `input()`'s folder map,
        /// as a category whose folder collides with a source is.
        fn refile(entries: &[(&PlanMessage, RefileFacts)]) -> RefileInput {
            RefileInput {
                facts: entries
                    .iter()
                    .map(|(m, f)| (m.message_id.clone(), f.clone()))
                    .collect(),
                frozen: BTreeSet::new(),
                categories: [
                    "correspondence",
                    "transactions",
                    "newsletters",
                    "updates",
                    "other",
                ]
                .map(String::from)
                .into_iter()
                .collect(),
            }
        }

        fn plan_of(i: &PlanInput, r: &RefileInput) -> Vec<(String, String)> {
            moves(&plan_with_refile(i, r))
        }

        #[test]
        fn a_marked_message_moves_to_its_new_category_folder() {
            let m = filed("m", "transactions");
            let p = plan_with_refile(&input(vec![m.clone()]), &refile(&[(&m, facts(&m))]));
            assert_eq!(moves(&p), vec![("m".into(), "Transactions".into())]);
            assert_eq!(p.refile_moves, BTreeSet::from(["m".to_string()]));
            assert!(matches!(
                &p.actions[0],
                Action::Move {
                    consumes_eligible: false,
                    desired_rev: 0,
                    ..
                }
            ));
            let mut unmarked = facts(&m);
            unmarked.marked = false;
            assert!(
                plan_of(&input(vec![m.clone()]), &refile(&[(&m, unmarked)])).is_empty(),
                "only marked mail is refiled"
            );
            assert!(
                plan(&input(vec![m])).actions.is_empty(),
                "`plan` has no refile facts"
            );
        }

        #[test]
        fn every_placement_rule_keeps_a_marked_message_where_it_is() {
            type Change = fn(&mut PlanMessage, &mut RefileFacts);
            let cases: [(&str, Change); 7] = [
                ("not at its filed home", |_, f| f.at_filed_home = false),
                ("pinned", |m, f| {
                    m.pinned = true;
                    f.pinned = true;
                }),
                ("blocked", |m, f| {
                    m.blocked = true;
                    f.blocked = true;
                }),
                ("done", |_, f| f.done = true),
                ("open move intent", |m, f| {
                    m.open_move_intent = true;
                    f.open_move_intent = true;
                }),
                ("corrected", |m, f| {
                    m.effective.category_from_override = true;
                    f.corrected = true;
                }),
                ("two copies", |_, f| f.single_occurrence = false),
            ];
            for (name, change) in cases {
                let mut m = filed("m", "transactions");
                let mut f = facts(&m);
                change(&mut m, &mut f);
                assert!(
                    plan_of(&input(vec![m.clone()]), &refile(&[(&m, f)])).is_empty(),
                    "{name}"
                );
            }
        }

        #[test]
        fn an_explicit_target_and_the_source_rule_come_first() {
            let mut m = filed("m", "transactions");
            m.desired_target = Some("transactions".into());
            m.desired_rev = 4;
            let mut f = facts(&m);
            f.explicit_target = true;
            let p = plan_with_refile(&input(vec![m.clone()]), &refile(&[(&m, f)]));
            assert_eq!(moves(&p), vec![("m".into(), "Transactions".into())]);
            assert!(
                p.refile_moves.is_empty(),
                "the explicit request's move consumes no mark"
            );
            let s = msg("s", "newsletters");
            let p = plan_with_refile(&input(vec![s.clone()]), &refile(&[(&s, facts(&s))]));
            assert_eq!(moves(&p), vec![("s".into(), "Newsletters".into())]);
            assert!(
                p.refile_moves.is_empty(),
                "a source folder's mail follows the source rule"
            );
        }

        #[test]
        fn a_marked_message_waits_for_a_current_classification_and_a_usable_target() {
            let mut stale = filed("stale", "transactions");
            stale.effective.current = false;
            let paused = filed("paused", "transactions");
            let r = refile(&[(&stale, facts(&stale)), (&paused, facts(&paused))]);
            let mut i = input(vec![stale.clone(), paused.clone()]);
            i.folders.get_mut("Transactions").unwrap().paused = true;
            assert!(plan_of(&i, &r).is_empty());
            assert_eq!(verdict(&i, &stale, &r), Verdict::Waiting);
            assert_eq!(
                verdict(&i, &paused, &r),
                Verdict::Skipped(Skip::TargetUnusable)
            );
            let mut i = input(vec![paused.clone()]);
            i.folders.get_mut("Transactions").unwrap().epoch = None;
            assert!(
                plan_of(&i, &r).is_empty(),
                "a target without established discovery"
            );
        }

        #[test]
        fn incomplete_input_never_moves() {
            let mut m = filed("m", "transactions");
            m.effective.input_incomplete = true;
            let r = refile(&[(&m, facts(&m))]);
            let i = input(vec![m.clone()]);
            assert!(plan_of(&i, &r).is_empty());
            assert_eq!(verdict(&i, &m, &r), Verdict::Skipped(Skip::IncompleteInput));
        }

        #[test]
        fn inbox_and_source_folders_are_never_targets() {
            let inbox = filed("inbox", "correspondence");
            let collides = filed("collides", "updates");
            let unknown = filed("unknown", "removed");
            let r = refile(&[
                (&inbox, facts(&inbox)),
                (&collides, facts(&collides)),
                (&unknown, facts(&unknown)),
            ]);
            let i = input(vec![inbox.clone(), collides.clone(), unknown.clone()]);
            assert!(plan_of(&i, &r).is_empty());
            assert_eq!(
                verdict(&i, &inbox, &r),
                Verdict::Skipped(Skip::TargetInboxOrSource)
            );
            assert_eq!(
                verdict(&i, &collides, &r),
                Verdict::Skipped(Skip::TargetInboxOrSource)
            );
            assert_eq!(
                verdict(&i, &unknown, &r),
                Verdict::Skipped(Skip::TargetUnusable)
            );
        }

        #[test]
        fn verdicts_name_the_target_and_why_it_moves() {
            let here = filed("here", "newsletters");
            let changed = filed("changed", "transactions");
            let mut i = input(vec![]);
            i.folders
                .insert("Old".into(), view(FolderUse::Unusable, false, true, 5));
            i.folders
                .insert("Gone".into(), view(FolderUse::Unusable, false, false, 6));
            let mut retired = homed(msg("retired", "transactions"), "Old", 5);
            retired.filed_at = Some("x".into());
            let mut unlisted = homed(msg("unlisted", "transactions"), "Gone", 6);
            unlisted.filed_at = Some("x".into());
            let source = msg("source", "transactions");
            let r = refile(&[
                (&here, facts(&here)),
                (&changed, facts(&changed)),
                (&retired, facts(&retired)),
                (&unlisted, facts(&unlisted)),
                (&source, facts(&source)),
            ]);
            let to_transactions = |reason| {
                Verdict::Candidate(Candidate {
                    target: "Transactions".into(),
                    category: "transactions".into(),
                    reason,
                })
            };
            assert_eq!(verdict(&i, &here, &r), Verdict::InPlace);
            assert_eq!(
                verdict(&i, &changed, &r),
                to_transactions(Reason::CategoryChanged)
            );
            assert_eq!(
                verdict(&i, &retired, &r),
                to_transactions(Reason::FolderRetired)
            );
            assert_eq!(
                verdict(&i, &unlisted, &r),
                to_transactions(Reason::FolderRetired)
            );
            assert_eq!(verdict(&i, &source, &r), Verdict::OutOfScope);
            i.messages = vec![retired, unlisted];
            assert_eq!(
                plan_of(&i, &r),
                vec![("retired".into(), "Transactions".into())],
                "out of a retired folder only while LIST reports it"
            );
        }

        #[test]
        fn the_refile_move_writes_no_flag_and_the_flag_rule_is_unchanged() {
            let mut actionable = filed("act", "transactions");
            actionable.effective.action_required = Some(true);
            let quiet = filed("quiet", "transactions");
            let r = refile(&[(&actionable, facts(&actionable)), (&quiet, facts(&quiet))]);
            let p = plan_with_refile(&input(vec![actionable, quiet]), &r);
            assert_eq!(
                flags(&p),
                vec!["act".to_string()],
                "the existing rule flags filed mail"
            );
            assert!(
                matches!(&p.actions[0], Action::Flag { at, .. } if at.folder == "Newsletters"),
                "where it is, before the move"
            );
            assert_eq!(moves(&p).len(), 2);
        }

        #[test]
        fn refile_moves_share_the_cap() {
            let mut a = filed("a", "transactions");
            a.internal_date = Some(t(11));
            let mut b = filed("b", "transactions");
            b.internal_date = Some(t(10));
            let r = refile(&[(&a, facts(&a)), (&b, facts(&b))]);
            let mut i = input(vec![a, b]);
            i.max_actions = 1;
            let p = plan_with_refile(&i, &r);
            assert_eq!(moves(&p), vec![("b".into(), "Transactions".into())]);
            assert_eq!(
                p.refile_moves,
                BTreeSet::from(["b".to_string()]),
                "only moves that made the cut"
            );
        }
    }
}
