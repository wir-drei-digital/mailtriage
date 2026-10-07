//! The `filing refile` command (refile spec "Command"): which filed mail
//! would follow its new category. It reads stored state only and makes no
//! mailbox calls.
use crate::domain::{AccountConfig, FilingMode};
use crate::filing::observe::{self, sources_of, OfflineEngine};
use crate::filing::refile::rules::{self, Candidate, Skip, Verdict};
use crate::filing::{inputs, mode_str, FilingWrite, FolderRecord, PassContext, Placement};
use crate::service::{err, filing_mode};
use crate::store::{now, Store};
use anyhow::Result;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// `filing refile` filters. `limit` (1..=500) bounds the listed candidates only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefileOptions {
    pub category: Option<String>,
    pub folder: Option<String>,
    pub limit: usize,
}

impl Default for RefileOptions {
    fn default() -> Self {
        Self {
            category: None,
            folder: None,
            limit: 50,
        }
    }
}

/// One matching message: its id, current (native) folder, home UID, and the
/// revision and mark the report read.
struct Row {
    id: String,
    folder: String,
    uid: u64,
    desired_rev: i64,
    marked: bool,
}

/// The matching set, before `limit`.
#[derive(Default)]
struct Report {
    candidates: Vec<(Row, Candidate)>,
    waiting: Vec<Row>,
    skipped: BTreeMap<Skip, usize>,
    /// Native folder -> (candidates, waiting).
    folders: BTreeMap<String, (usize, usize)>,
    /// Native folders of `folders` no configured category files into.
    retired: BTreeSet<String>,
}

/// The preview's JSON (empty with filing `off`).
pub fn preview(
    store: &Store,
    name: &str,
    cfg: &AccountConfig,
    generation: &str,
    opts: &RefileOptions,
) -> Result<Value> {
    let folder = selected_folder(store, name, cfg, opts)?;
    let mode = filing_mode(cfg);
    let found = if mode == FilingMode::Off {
        Report::default()
    } else {
        report(
            store,
            name,
            cfg,
            generation,
            opts.category.as_deref(),
            folder.as_deref(),
        )?
    };
    let configured: BTreeMap<String, Option<String>> = store
        .folder_records(name)?
        .into_iter()
        .map(|r| (r.native, r.configured))
        .collect();
    let candidates: Vec<Value> = found
        .candidates
        .iter()
        .take(opts.limit)
        .map(|(row, c)| {
            json!({"id": row.id, "folder": row.folder, "target": c.target, "category": c.category, "reason": c.reason.as_str()})
        })
        .collect();
    let folders: Vec<Value> = found
        .folders
        .iter()
        .map(|(native, (candidates, waiting))| {
            json!({
                "folder": configured.get(native).cloned().flatten(),
                "native": native,
                "retired": found.retired.contains(native),
                "candidates": candidates,
                "waiting": waiting,
            })
        })
        .collect();
    let skipped: Map<String, Value> = Skip::ALL
        .iter()
        .map(|s| {
            (
                s.as_str().to_string(),
                json!(found.skipped.get(s).copied().unwrap_or(0)),
            )
        })
        .collect();
    Ok(json!({
        "schema_version": 1,
        "account": name,
        "mode": mode_str(mode),
        "candidates": candidates,
        "total": found.candidates.len(),
        "folders": folders,
        "waiting": found.waiting.len(),
        "skipped": skipped,
    }))
}

/// `filing status`'s `refile_candidates`: the unfiltered candidate total.
pub fn candidate_total(
    store: &Store,
    name: &str,
    cfg: &AccountConfig,
    generation: &str,
) -> Result<usize> {
    if filing_mode(cfg) == FilingMode::Off {
        return Ok(0);
    }
    Ok(report(store, name, cfg, generation, None, None)?
        .candidates
        .len())
}

/// `--apply` (filing `live`, checked by the caller under the configuration
/// lock): marks the matching candidates — and with `--folder` but no
/// `--category` the waiting messages too — that are not marked yet, with a
/// new revision each and one `refile_marked` event, in one transaction.
pub fn apply(
    store: &mut Store,
    name: &str,
    cfg: &AccountConfig,
    generation: &str,
    opts: &RefileOptions,
) -> Result<Value> {
    let folder = selected_folder(store, name, cfg, opts)?;
    let with_waiting = folder.is_some() && opts.category.is_none();
    for _ in 0..5 {
        let found = report(
            store,
            name,
            cfg,
            generation,
            opts.category.as_deref(),
            folder.as_deref(),
        )?;
        let mut rows: Vec<(&Row, bool)> = found
            .candidates
            .iter()
            .map(|(row, _)| (row, false))
            .collect();
        if with_waiting {
            rows.extend(found.waiting.iter().map(|row| (row, true)));
        }
        rows.retain(|(row, _)| !row.marked);
        let mut reads: Vec<(Placement, bool)> = Vec::new();
        for (row, waiting) in &rows {
            match store.placement(name, &row.id)? {
                Some(p) if p.desired_rev == row.desired_rev => reads.push((p, *waiting)),
                _ => break,
            }
        }
        if reads.len() != rows.len() {
            continue; // changed since the report: read it again
        }
        let waiting_marked = reads.iter().filter(|(_, waiting)| *waiting).count();
        let marked = reads.len() - waiting_marked;
        let result = json!({"schema_version": 1, "account": name, "marked": marked, "waiting_marked": waiting_marked});
        if reads.is_empty() {
            return Ok(result);
        }
        let changed: Vec<Placement> = reads
            .iter()
            .map(|(p, _)| Placement {
                refile_once: true,
                desired_rev: p.desired_rev + 1,
                ..p.clone()
            })
            .collect();
        let mut writes: Vec<FilingWrite> = reads
            .iter()
            .zip(&changed)
            .map(|((read, _), placement)| FilingWrite::PlacementFrom { placement, read })
            .collect();
        writes.push(FilingWrite::Event {
            message_id: None,
            folder: folder.as_deref(),
            kind: "refile_marked",
            detail: json!({"marked": marked, "waiting_marked": waiting_marked, "category": opts.category, "folder": folder}),
        });
        if store.commit_filing(name, &writes, &now())? {
            return Ok(result);
        }
    }
    Err(err(5, "placements changed concurrently; retry"))
}

/// `categories apply`'s `hint`; `null` with filing `off`.
pub fn hint(name: &str, cfg: &AccountConfig) -> Value {
    if filing_mode(cfg) == FilingMode::Off {
        return Value::Null;
    }
    json!(format!(
        "Open mail is classified again over the next sync passes. Once they have run, `mailtriage filing refile --account {name}` shows which filed mail would move."
    ))
}

/// `--folder NAME`: a category or retired folder's native name wins;
/// otherwise its configured name, which must name exactly one folder.
pub fn match_folder(records: &[FolderRecord], sources: &[String], name: &str) -> Result<String> {
    let known: Vec<&FolderRecord> = records
        .iter()
        .filter(|r| r.category_id.is_some() && !sources.contains(&r.native))
        .collect();
    if let Some(r) = known.iter().find(|r| r.native == name) {
        return Ok(r.native.clone());
    }
    let named: Vec<&str> = known
        .iter()
        .filter(|r| r.configured.as_deref() == Some(name))
        .map(|r| r.native.as_str())
        .collect();
    match named.as_slice() {
        [native] => Ok(native.to_string()),
        [] => Err(err(2, "unknown folder: not a category or retired folder")),
        several => Err(err(
            2,
            format!(
                "folder name matches several folders; pass the native name: {}",
                several.join(", ")
            ),
        )),
    }
}

/// The checks of `--limit`, `--category` and `--folder`; the native folder
/// `--folder` names.
fn selected_folder(
    store: &Store,
    name: &str,
    cfg: &AccountConfig,
    opts: &RefileOptions,
) -> Result<Option<String>> {
    if !(1..=500).contains(&opts.limit) {
        return Err(err(2, "refile limit must be 1..=500"));
    }
    if let Some(category) = &opts.category {
        if !cfg.categories.iter().any(|c| &c.id == category) {
            return Err(err(2, "unknown category"));
        }
    }
    match &opts.folder {
        None => Ok(None),
        Some(folder) => {
            let records = store.folder_records(name)?;
            Ok(Some(match_folder(&records, &sources_of(cfg), folder)?))
        }
    }
}

/// Every placement's verdict over the offline folder map, filtered.
fn report(
    store: &Store,
    name: &str,
    cfg: &AccountConfig,
    generation: &str,
    category: Option<&str>,
    folder: Option<&str>,
) -> Result<Report> {
    let map = observe::offline_map(store, name, cfg)?;
    let no_binding_check = || -> Result<()> { Ok(()) };
    let ctx = PassContext {
        account: name,
        cfg,
        engine: &OfflineEngine,
        mode: filing_mode(cfg),
        generation,
        now: now(),
        // The planner input does not use it.
        max_attempts: 0,
        verify_binding: &no_binding_check,
    };
    // Not a planner preview: spec rule 7 wants a target in state `ok`, so a
    // folder a live pass has yet to create (or failed to) is `target_unusable`,
    // as it is for a pass's refile rule.
    let input = inputs::plan_input(store, &ctx, &map, false)?;
    let gone = rules::gone(store, name, &map.listed)?;
    let refile = rules::input(store, name, cfg, &gone)?;
    let mut out = Report::default();
    for m in &input.messages {
        let Some(home) = &m.home else {
            continue;
        };
        if folder.is_some_and(|f| f != home.folder) {
            continue;
        }
        let row = Row {
            id: m.message_id.clone(),
            folder: home.folder.clone(),
            uid: home.uid,
            desired_rev: m.desired_rev,
            marked: refile.facts.get(&m.message_id).is_some_and(|f| f.marked),
        };
        match rules::verdict(&input, m, &refile) {
            Verdict::OutOfScope | Verdict::InPlace => {}
            Verdict::Waiting => {
                out.folders.entry(row.folder.clone()).or_default().1 += 1;
                out.waiting.push(row);
            }
            Verdict::Candidate(c) => {
                if category.is_none_or(|k| k == c.category) {
                    out.folders.entry(row.folder.clone()).or_default().0 += 1;
                    out.candidates.push((row, c));
                }
            }
            Verdict::Skipped(skip) => {
                if category.is_none_or(|k| m.effective.category_id.as_deref() == Some(k)) {
                    *out.skipped.entry(skip).or_default() += 1;
                }
            }
        }
    }
    out.retired = out
        .folders
        .keys()
        .filter(|f| !rules::in_category_folder(&input, f))
        .cloned()
        .collect();
    out.candidates
        .sort_by(|a, b| (&a.0.folder, a.0.uid).cmp(&(&b.0.folder, b.0.uid)));
    out.waiting
        .sort_by(|a, b| (&a.folder, a.uid).cmp(&(&b.folder, b.uid)));
    Ok(out)
}
