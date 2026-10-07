//! Every sentence the window shows: change summaries, confirmations, plain
//! versions of CLI errors and the refile panel's texts. No internal names.
use crate::cli::{Changes, RefileFolder, RefileMarked, RefilePreview};

/// The confirmation's warning for a change that sorts mail again.
pub const RECLASSIFY_WARNING: &str = "All open mail in this account will be sorted again. This uses your OpenRouter key and takes a few passes.";
pub const NOTHING_TO_MOVE: &str = "No filed mail needs moving.";
pub const DRY_RUN: &str = "Moving filed mail needs filing set to live.";
pub const SERVICE_NOT_RUNNING: &str =
    "The background service is not running. Start it, or run mailtriage sync, to move them.";
pub const EMPTY: &str = "Add categories for the kinds of mail you get.";
pub const CATEGORIES_CHANGED: &str = "These categories were changed somewhere else.";
pub const BUSY_RETRYING: &str = "mailtriage is busy; trying again…";
pub const BUSY: &str = "mailtriage is busy.";
pub const RECHECKING: &str = "The configuration changed while saving. Checking your changes again.";
/// Why the form and Apply are off when nothing is loaded and no load runs
/// (mailtriage missing, not set up, or the first load failed).
pub const NOT_LOADED: &str =
    "The categories could not be loaded; the message at the bottom says why";

/// `n messages`, or `1 message`.
pub fn messages(n: u64) -> String {
    if n == 1 {
        "1 message".into()
    } else {
        format!("{n} messages")
    }
}

/// `a`, `a and b`, `a, b and c`.
pub fn list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The footer's summary, for example
/// "1 added, 1 renamed, mail folder changed for Newsletters". Folder
/// changes are left out while filing is off: no folder is used then.
pub fn change_summary(changes: &Changes, name: &dyn Fn(&str) -> String, filing_on: bool) -> String {
    let mut parts = vec![];
    for (count, word) in [
        (changes.added.len(), "added"),
        (changes.removed.len(), "removed"),
        (changes.renamed.len(), "renamed"),
        (changes.edited.len(), "edited"),
    ] {
        if count > 0 {
            parts.push(format!("{count} {word}"));
        }
    }
    if filing_on && !changes.folders_changed.is_empty() {
        let names: Vec<String> = changes
            .folders_changed
            .iter()
            .map(|c| name(&c.id))
            .collect();
        parts.push(format!("mail folder changed for {}", list(&names)));
    }
    if parts.is_empty() {
        "No changes".into()
    } else {
        parts.join(", ")
    }
}

/// The apply confirmation's lines, one per change.
pub fn change_lines(
    changes: &Changes,
    name: &dyn Fn(&str) -> String,
    filing_on: bool,
) -> Vec<String> {
    let mut lines = vec![];
    lines.extend(changes.added.iter().map(|id| format!("Add {}", name(id))));
    lines.extend(changes.removed.iter().map(|r| {
        if filing_on {
            format!("Remove {} (its mail stays in {})", name(&r.id), r.folder)
        } else {
            format!("Remove {}", name(&r.id))
        }
    }));
    lines.extend(
        changes
            .renamed
            .iter()
            .map(|c| format!("Rename {} to {}", c.from, c.to)),
    );
    if filing_on {
        lines.extend(
            changes
                .folders_changed
                .iter()
                .map(|c| format!("Mail folder for {}: {} → {}", name(&c.id), c.from, c.to)),
        );
    }
    lines.extend(
        changes
            .edited
            .iter()
            .map(|id| format!("Update {}", name(id))),
    );
    lines
}

/// What happens after a successful apply.
pub fn saved(changes: &Changes) -> String {
    if changes.reclassifies {
        "Saved. Open mail will be sorted again over the next passes.".into()
    } else {
        "Saved. New mail uses these categories.".into()
    }
}

/// The question before removing a category.
pub fn remove_question(name: &str, folder: Option<&str>) -> String {
    match folder {
        Some(folder) => format!(
            "Remove {name}? Mail in {folder} stays there until you move it; new mail is sorted into the remaining categories."
        ),
        None => format!("Remove {name}? New mail is sorted into the remaining categories."),
    }
}

/// A CLI error in plain words; `name` maps a category ID to its name.
pub fn plain_error(message: &str, name: &dyn Fn(&str) -> String) -> String {
    if message.starts_with("invalid categories") {
        return "Every category needs a name, a description and its own ID, and exactly one must be the default category.".into();
    }
    if let Some(ids) = message.strip_prefix("categories need a valid folder: ") {
        let names: Vec<String> = ids.split(", ").map(name).collect();
        return format!(
            "Choose another mail folder for {}. Each category needs its own folder. A folder name uses only English letters without accents, digits, spaces and simple punctuation; it has at most 200 characters, does not start with - or a space, does not end with a space, is not called Inbox, and does not contain / . * % \" \\ or &.",
            list(&names)
        );
    }
    if message.starts_with("category has manual corrections") {
        return "You corrected some mail to a category you removed. Correct that mail to another category first.".into();
    }
    message.to_owned()
}

/// A retired folder's row of the refile panel.
pub fn retired_row(folder: &RefileFolder) -> String {
    let name = folder.name();
    match (folder.candidates, folder.waiting) {
        (0, w) => format!(
            "{name} is no longer used: {} may move once they are sorted again.",
            messages(w)
        ),
        (c, 0) => format!("{name} is no longer used: {} can move now.", messages(c)),
        (c, w) => format!(
            "{name} is no longer used: {} can move now; {w} more may move once they are sorted again.",
            messages(c)
        ),
    }
}

/// The panel's account-wide row; `None` when nothing can move.
pub fn all_row(preview: &RefilePreview) -> Option<String> {
    let total = preview.total;
    if total == 0 {
        return None;
    }
    let retired: u64 = preview
        .folders
        .iter()
        .filter(|f| f.retired)
        .map(|f| f.candidates)
        .sum();
    let (verb, pronoun) = if total == 1 {
        ("is", "its")
    } else {
        ("are", "their")
    };
    let tail = match retired {
        0 => String::new(),
        r if r == total => " (all in folders no longer used)".into(),
        r => format!(" ({r} of them in folders no longer used)"),
    };
    Some(format!(
        "{} {verb} in a folder that no longer matches {pronoun} category{tail}.",
        messages(total)
    ))
}

/// Messages still being sorted outside retired folders.
pub fn waiting_row(preview: &RefilePreview) -> Option<String> {
    let in_retired: u64 = preview
        .folders
        .iter()
        .filter(|f| f.retired)
        .map(|f| f.waiting)
        .sum();
    let waiting = preview.waiting.saturating_sub(in_retired);
    (waiting > 0).then(|| {
        format!(
            "{} {} still being sorted again; some may need moving afterwards. Open this panel again later.",
            messages(waiting),
            if waiting == 1 { "is" } else { "are" }
        )
    })
}

/// The "Not moved" counts in words, for example "corrected by you: 3".
pub fn skipped_rows(preview: &RefilePreview) -> Vec<String> {
    preview
        .skipped
        .iter()
        .filter(|(_, n)| **n > 0)
        .map(|(reason, n)| {
            let words = match reason.as_str() {
                "not_filed_by_mailtriage" => "not filed by mailtriage",
                "corrected" => "corrected by you",
                "pinned" => "kept in place by you",
                "blocked" => "held back after a problem",
                "done" => "marked done",
                "open_intent" => "already being moved",
                "explicit_target" => "waiting for a move you asked for",
                "multiple_copies" => "in more than one folder",
                "incomplete_input" => "not fully read",
                "retired_frozen" => "in a folder no longer checked",
                "target_unusable" => "their new folder is not ready",
                "target_inbox_or_source" => "would go back to the inbox",
                other => return format!("{}: {n}", other.replace('_', " ")),
            };
            format!("{words}: {n}")
        })
        .collect()
}

/// The result of a move.
pub fn moved(result: &RefileMarked) -> String {
    let marked = format!("Marked {}", messages(result.marked));
    if result.waiting_marked > 0 {
        format!(
            "{marked}; {} more will move if their new category calls for it.",
            result.waiting_marked
        )
    } else {
        format!("{marked}.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Change, Removed};
    use std::collections::BTreeMap;

    fn name(id: &str) -> String {
        match id {
            "news" => "Newsletters".into(),
            "travel" => "Travel".into(),
            "promo" => "Promotions".into(),
            "work" => "Work".into(),
            other => other.into(),
        }
    }

    fn changes() -> Changes {
        Changes {
            added: vec!["travel".into()],
            removed: vec![Removed {
                id: "promo".into(),
                folder: "Promotions".into(),
            }],
            renamed: vec![Change {
                id: "news".into(),
                from: "News".into(),
                to: "Newsletters".into(),
            }],
            folders_changed: vec![Change {
                id: "news".into(),
                from: "News".into(),
                to: "Newsletters".into(),
            }],
            edited: vec!["work".into()],
            reclassifies: true,
        }
    }

    #[test]
    fn summaries_name_each_kind_of_change() {
        assert_eq!(
            change_summary(&changes(), &name, true),
            "1 added, 1 removed, 1 renamed, 1 edited, mail folder changed for Newsletters"
        );
        assert_eq!(
            change_summary(&changes(), &name, false),
            "1 added, 1 removed, 1 renamed, 1 edited"
        );
        assert_eq!(
            change_summary(&Changes::default(), &name, true),
            "No changes"
        );
        assert_eq!(
            change_lines(&changes(), &name, true),
            [
                "Add Travel",
                "Remove Promotions (its mail stays in Promotions)",
                "Rename News to Newsletters",
                "Mail folder for Newsletters: News → Newsletters",
                "Update Work"
            ]
        );
        assert_eq!(list(&["a".into(), "b".into(), "c".into()]), "a, b and c");
    }

    #[test]
    fn cli_errors_become_plain_words() {
        assert!(plain_error(
            "invalid categories; require unique IDs, descriptions and exactly one catch-all",
            &name
        )
        .starts_with("Every category needs"));
        assert_eq!(
            plain_error("categories need a valid folder: news, promo", &name),
            "Choose another mail folder for Newsletters and Promotions. Each category needs its own folder. A folder name uses only English letters without accents, digits, spaces and simple punctuation; it has at most 200 characters, does not start with - or a space, does not end with a space, is not called Inbox, and does not contain / . * % \" \\ or &."
        );
        assert_eq!(plain_error("something else", &name), "something else");
    }

    fn folder(name: &str, retired: bool, candidates: u64, waiting: u64) -> RefileFolder {
        RefileFolder {
            folder: Some(name.into()),
            native: format!("INBOX.{name}"),
            retired,
            candidates,
            waiting,
        }
    }

    #[test]
    fn refile_texts() {
        assert_eq!(
            retired_row(&folder("Promotions", true, 30, 12)),
            "Promotions is no longer used: 30 messages can move now; 12 more may move once they are sorted again."
        );
        assert_eq!(
            retired_row(&folder("Promotions", true, 1, 0)),
            "Promotions is no longer used: 1 message can move now."
        );
        assert_eq!(
            retired_row(&folder("Promotions", true, 0, 4)),
            "Promotions is no longer used: 4 messages may move once they are sorted again."
        );
        let preview = RefilePreview {
            total: 38,
            folders: vec![
                folder("News", false, 8, 0),
                folder("Promotions", true, 30, 12),
            ],
            waiting: 15,
            skipped: BTreeMap::from([("corrected".into(), 3), ("odd_reason".into(), 2)]),
        };
        assert_eq!(
            all_row(&preview).unwrap(),
            "38 messages are in a folder that no longer matches their category (30 of them in folders no longer used)."
        );
        assert_eq!(
            waiting_row(&preview).unwrap(),
            "3 messages are still being sorted again; some may need moving afterwards. Open this panel again later."
        );
        assert_eq!(
            skipped_rows(&preview),
            ["corrected by you: 3", "odd reason: 2"]
        );
        // Only waiting messages: nothing to move yet, but the waiting line.
        let waiting_only = RefilePreview {
            total: 0,
            folders: vec![],
            waiting: 1,
            skipped: BTreeMap::new(),
        };
        assert_eq!(all_row(&waiting_only), None);
        assert_eq!(
            waiting_row(&waiting_only).unwrap(),
            "1 message is still being sorted again; some may need moving afterwards. Open this panel again later."
        );
        assert_eq!(
            moved(&RefileMarked {
                marked: 30,
                waiting_marked: 12
            }),
            "Marked 30 messages; 12 more will move if their new category calls for it."
        );
        assert_eq!(
            moved(&RefileMarked {
                marked: 1,
                waiting_marked: 0
            }),
            "Marked 1 message."
        );
    }
}
