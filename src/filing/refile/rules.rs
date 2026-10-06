//! Refile spec "Candidates": which filed messages may follow a category
//! change, and why the others stay where they are. Pure rules over the
//! planner input, plus per-placement facts read from the store.
use crate::domain::AccountConfig;
use crate::filing::planner::{target_usable, CategoryFolder, PlanInput, PlanMessage};
use crate::filing::{MessageMeta, Placement};
use crate::store::{Record, Store};
use anyhow::Result;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Why a message in a category or retired folder stays where it is (the
/// `skipped` keys), in the order of the spec's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Skip {
    NotFiledByMailtriage,
    Corrected,
    Pinned,
    Blocked,
    Done,
    OpenIntent,
    ExplicitTarget,
    MultipleCopies,
    IncompleteInput,
    RetiredFrozen,
    TargetUnusable,
    TargetInboxOrSource,
}

impl Skip {
    pub const ALL: [Skip; 12] = [
        Skip::NotFiledByMailtriage,
        Skip::Corrected,
        Skip::Pinned,
        Skip::Blocked,
        Skip::Done,
        Skip::OpenIntent,
        Skip::ExplicitTarget,
        Skip::MultipleCopies,
        Skip::IncompleteInput,
        Skip::RetiredFrozen,
        Skip::TargetUnusable,
        Skip::TargetInboxOrSource,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Skip::NotFiledByMailtriage => "not_filed_by_mailtriage",
            Skip::Corrected => "corrected",
            Skip::Pinned => "pinned",
            Skip::Blocked => "blocked",
            Skip::Done => "done",
            Skip::OpenIntent => "open_intent",
            Skip::ExplicitTarget => "explicit_target",
            Skip::MultipleCopies => "multiple_copies",
            Skip::IncompleteInput => "incomplete_input",
            Skip::RetiredFrozen => "retired_frozen",
            Skip::TargetUnusable => "target_unusable",
            Skip::TargetInboxOrSource => "target_inbox_or_source",
        }
    }
}

/// Why a candidate moves: its folder belongs to another category, or to none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    CategoryChanged,
    FolderRetired,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::CategoryChanged => "category_changed",
            Reason::FolderRetired => "folder_retired",
        }
    }
}

/// A candidate's new folder (native) and category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub target: String,
    pub category: String,
    pub reason: Reason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Not refile's business: no known home, or a home in a source folder or
    /// in a folder without a record.
    OutOfScope,
    /// Rules 1–5 hold; its new category is decided once it is classified again.
    Waiting,
    /// Its current classification files it where it is.
    InPlace,
    Skipped(Skip),
    Candidate(Candidate),
}

/// What the store says about one placement beyond the planner input.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefileFacts {
    /// `refile_once`.
    pub marked: bool,
    /// Rules 1–2: a known, hydrated home that is the filed home, in a folder
    /// that still exists.
    pub at_filed_home: bool,
    /// The recorded home folder, known or not.
    pub home_folder: Option<String>,
    pub pinned: bool,
    pub blocked: bool,
    /// Review state `done`.
    pub done: bool,
    pub open_move_intent: bool,
    /// The open move intent is a refile intent.
    pub open_refile_intent: bool,
    /// An outstanding `desired_target`.
    pub explicit_target: bool,
    /// A manual category override.
    pub corrected: bool,
    pub single_occurrence: bool,
}

/// Refile facts by message id, beside the planner input.
#[derive(Debug, Clone, Default)]
pub struct RefileInput {
    pub facts: BTreeMap<String, RefileFacts>,
    /// Retired folders that are neither retained nor draining.
    pub frozen: BTreeSet<String>,
    /// The account's configured category ids.
    pub categories: BTreeSet<String>,
}

/// Recorded category and retired folders `listed` does not report: their
/// mail went with them.
pub fn gone(store: &Store, account: &str, listed: &BTreeSet<String>) -> Result<BTreeSet<String>> {
    Ok(store
        .folder_records(account)?
        .into_iter()
        .filter(|r| r.category_id.is_some() && !listed.contains(&r.native))
        .map(|r| r.native)
        .collect())
}

/// The refile facts of every placement of the account.
pub fn input(
    store: &Store,
    account: &str,
    cfg: &AccountConfig,
    gone: &BTreeSet<String>,
) -> Result<RefileInput> {
    let rows = store.records_for_planning(account)?;
    let counts = store.occurrence_counts(account)?;
    build(store, account, cfg, gone, rows, &counts, None)
}

/// Facts of `rows`; the open move intent `exclude` does not count.
fn build(
    store: &Store,
    account: &str,
    cfg: &AccountConfig,
    gone: &BTreeSet<String>,
    rows: Vec<(Record, Placement, MessageMeta)>,
    counts: &BTreeMap<String, usize>,
    exclude: Option<i64>,
) -> Result<RefileInput> {
    // Message id -> whether one of its open move intents is a refile intent.
    let mut open: BTreeMap<String, bool> = BTreeMap::new();
    for i in store.intents(account, true)? {
        if i.kind == "move" && Some(i.id) != exclude {
            *open.entry(i.message_id).or_default() |= i.consumes_refile;
        }
    }
    let facts = rows
        .into_iter()
        .map(|(record, p, meta)| {
            let home_gone = p.home_folder.as_ref().is_some_and(|h| gone.contains(h));
            let f = RefileFacts {
                marked: p.refile_once,
                at_filed_home: p.at_filed_home() && meta.size.is_some() && !home_gone,
                home_folder: p.home_folder.clone(),
                pinned: p.pinned,
                blocked: p.blocked_reason.is_some(),
                done: record.review_state == "done",
                open_move_intent: open.contains_key(&p.message_id),
                open_refile_intent: open.get(&p.message_id) == Some(&true),
                explicit_target: p.desired_target.is_some(),
                corrected: record
                    .overrides
                    .get("category_id")
                    .and_then(Value::as_str)
                    .is_some(),
                single_occurrence: counts.get(&p.message_id) == Some(&1),
            };
            (p.message_id, f)
        })
        .collect();
    Ok(RefileInput {
        facts,
        frozen: store.frozen_folders(account)?,
        categories: cfg.categories.iter().map(|c| c.id.clone()).collect(),
    })
}

/// Candidate rules 1–5 in the order of the spec's `skipped` list; mail not
/// at its filed home in a frozen retired folder is `retired_frozen`.
pub fn placement_skip(f: &RefileFacts, frozen: &BTreeSet<String>) -> Option<Skip> {
    if !f.at_filed_home {
        let in_frozen = f.home_folder.as_ref().is_some_and(|h| frozen.contains(h));
        return Some(if in_frozen {
            Skip::RetiredFrozen
        } else {
            Skip::NotFiledByMailtriage
        });
    }
    [
        (f.corrected, Skip::Corrected),
        (f.pinned, Skip::Pinned),
        (f.blocked, Skip::Blocked),
        (f.done, Skip::Done),
        (f.open_move_intent, Skip::OpenIntent),
        (f.explicit_target, Skip::ExplicitTarget),
        (!f.single_occurrence, Skip::MultipleCopies),
    ]
    .into_iter()
    .find(|(failed, _)| *failed)
    .map(|(_, skip)| skip)
}

/// Candidate rules 1–8 for one planned message.
pub fn verdict(input: &PlanInput, m: &PlanMessage, refile: &RefileInput) -> Verdict {
    let (Some(f), Some(home)) = (refile.facts.get(&m.message_id), m.home.as_ref()) else {
        return Verdict::OutOfScope;
    };
    if input.folders.get(&home.folder).is_none_or(|v| v.is_source) {
        return Verdict::OutOfScope;
    }
    if let Some(skip) = placement_skip(f, &refile.frozen) {
        return Verdict::Skipped(skip);
    }
    let e = &m.effective;
    if !e.current {
        return Verdict::Waiting;
    }
    if e.input_incomplete {
        return Verdict::Skipped(Skip::IncompleteInput);
    }
    let Some(category) = e.category_id.clone() else {
        return Verdict::Skipped(Skip::TargetUnusable);
    };
    let to = match input.categories.get(&category) {
        Some(CategoryFolder::Native(native)) => native.clone(),
        Some(CategoryFolder::Inbox) => return Verdict::Skipped(Skip::TargetInboxOrSource),
        // A configured category the folder map leaves out files into a source.
        None if refile.categories.contains(&category) => {
            return Verdict::Skipped(Skip::TargetInboxOrSource)
        }
        None => return Verdict::Skipped(Skip::TargetUnusable),
    };
    if to == home.folder {
        return Verdict::InPlace;
    }
    if input.folders.get(&to).is_some_and(|v| v.is_source) {
        return Verdict::Skipped(Skip::TargetInboxOrSource);
    }
    if !target_usable(input, &to) {
        return Verdict::Skipped(Skip::TargetUnusable);
    }
    let reason = if in_category_folder(input, &home.folder) {
        Reason::CategoryChanged
    } else {
        Reason::FolderRetired
    };
    Verdict::Candidate(Candidate {
        target: to,
        category,
        reason,
    })
}

/// Whether some configured category files into native `folder`.
pub fn in_category_folder(input: &PlanInput, folder: &str) -> bool {
    input
        .categories
        .values()
        .any(|c| matches!(c, CategoryFolder::Native(n) if n == folder))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok() -> RefileFacts {
        RefileFacts {
            marked: true,
            at_filed_home: true,
            home_folder: Some("News".into()),
            single_occurrence: true,
            ..Default::default()
        }
    }

    #[test]
    fn rules_one_to_five_report_the_first_failure_in_list_order() {
        let none = BTreeSet::new();
        assert_eq!(placement_skip(&ok(), &none), None);
        let mut f = ok();
        f.single_occurrence = false;
        assert_eq!(placement_skip(&f, &none), Some(Skip::MultipleCopies));
        f.explicit_target = true;
        assert_eq!(placement_skip(&f, &none), Some(Skip::ExplicitTarget));
        f.open_move_intent = true;
        assert_eq!(placement_skip(&f, &none), Some(Skip::OpenIntent));
        f.done = true;
        assert_eq!(placement_skip(&f, &none), Some(Skip::Done));
        f.blocked = true;
        assert_eq!(placement_skip(&f, &none), Some(Skip::Blocked));
        f.pinned = true;
        assert_eq!(placement_skip(&f, &none), Some(Skip::Pinned));
        f.corrected = true;
        assert_eq!(placement_skip(&f, &none), Some(Skip::Corrected));
        f.at_filed_home = false;
        assert_eq!(placement_skip(&f, &none), Some(Skip::NotFiledByMailtriage));
        let frozen = BTreeSet::from(["News".to_string()]);
        assert_eq!(placement_skip(&f, &frozen), Some(Skip::RetiredFrozen));
        assert_eq!(
            placement_skip(&ok(), &frozen),
            None,
            "mail at its filed home is never frozen out"
        );
    }

    #[test]
    fn skip_names_are_the_specs_keys() {
        assert_eq!(
            Skip::ALL.map(Skip::as_str),
            [
                "not_filed_by_mailtriage",
                "corrected",
                "pinned",
                "blocked",
                "done",
                "open_intent",
                "explicit_target",
                "multiple_copies",
                "incomplete_input",
                "retired_frozen",
                "target_unusable",
                "target_inbox_or_source",
            ]
        );
        assert_eq!(Reason::CategoryChanged.as_str(), "category_changed");
        assert_eq!(Reason::FolderRetired.as_str(), "folder_retired");
    }
}
