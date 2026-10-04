//! Pure filing planner (spec "Planner"): local state in, actions out.

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
