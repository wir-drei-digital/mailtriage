//! What `categories apply` would write for an account: the digest that
//! identifies its categories and filing state, apply's folder normalization,
//! and the changes a category file makes.
use crate::domain::{AccountConfig, Category, FilingMode};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// The digest input. Field order is part of the `v1` encoding.
#[derive(Serialize)]
struct DigestInput<'a> {
    filing_on: bool,
    categories: &'a [Category],
}

/// Whether the account's configured filing mode is not `off`. With filing
/// on, `apply` keeps the folder of an existing category without `folder`.
pub fn filing_on(account: &AccountConfig) -> bool {
    account.filing.mode != FilingMode::Off
}

/// `v1:` and the lowercase hex SHA-256 of the typed values, so key order and
/// `"folder": null` versus no `folder` in an input file do not matter, while
/// category order does.
pub fn digest(filing_on: bool, categories: &[Category]) -> String {
    let bytes = serde_json::to_vec(&DigestInput {
        filing_on,
        categories,
    })
    .expect("categories always serialize");
    format!("v1:{:x}", Sha256::digest(bytes))
}

/// The digest of an account's current categories and filing state.
pub fn account_digest(account: &AccountConfig) -> String {
    digest(filing_on(account), &account.categories)
}

/// `apply`'s normalization: with filing on, a category without `folder`
/// keeps the folder its ID had before (a rename does not move its folder),
/// and a new one uses its name.
pub fn normalized(previous: &AccountConfig, mut categories: Vec<Category>) -> Vec<Category> {
    if filing_on(previous) {
        for c in categories.iter_mut().filter(|c| c.folder.is_none()) {
            let folder = match previous.categories.iter().find(|p| p.id == c.id) {
                Some(before) => before.effective_folder().to_string(),
                None => c.name.clone(),
            };
            c.folder = Some(folder);
        }
    }
    categories
}

/// A removed category and the folder its mail was filed to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Removed {
    pub id: String,
    pub folder: String,
}

/// A value of a category that changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change {
    pub id: String,
    pub from: String,
    pub to: String,
}

/// `categories validate --account`'s `changes`. Folder entries hold
/// effective folder names, for display only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Changes {
    pub added: Vec<String>,
    pub removed: Vec<Removed>,
    pub renamed: Vec<Change>,
    pub folders_changed: Vec<Change>,
    pub edited: Vec<String>,
    pub reclassifies: bool,
}

/// What applying `next` (already normalized) changes in `previous`. Added,
/// renamed, re-pointed and edited categories are listed in `next`'s order,
/// removed ones in `previous`'s. `reclassifies` is exactly `apply`'s rule
/// for advancing `taxonomy_revision`.
pub fn changes(previous: &[Category], next: &[Category]) -> Changes {
    let before = |id: &str| previous.iter().find(|p| p.id == id);
    let mut changes = Changes {
        reclassifies: crate::service::category_semantics(previous)
            != crate::service::category_semantics(next),
        ..Changes::default()
    };
    for c in next {
        let Some(p) = before(&c.id) else {
            changes.added.push(c.id.clone());
            continue;
        };
        if p.name != c.name {
            changes.renamed.push(Change {
                id: c.id.clone(),
                from: p.name.clone(),
                to: c.name.clone(),
            });
        }
        if p.effective_folder() != c.effective_folder() {
            changes.folders_changed.push(Change {
                id: c.id.clone(),
                from: p.effective_folder().to_string(),
                to: c.effective_folder().to_string(),
            });
        }
        if p.description != c.description || p.examples != c.examples || p.catch_all != c.catch_all
        {
            changes.edited.push(c.id.clone());
        }
    }
    for p in previous {
        if !next.iter().any(|c| c.id == p.id) {
            changes.removed.push(Removed {
                id: p.id.clone(),
                folder: p.effective_folder().to_string(),
            });
        }
    }
    changes
}
