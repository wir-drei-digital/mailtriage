//! Observation steps of a filing pass (spec "Sync pass order", "Folders",
//! "Identity and placements"): folder resolution and the watch list, the
//! discovery options of watched folders, rescan completion, bootstrap and
//! hydration; and, outside a pass, the offline folder map of `filing plan` /
//! `filing status` and `doctor`'s read-only folder report.
use super::refile::retired::{self, RetiredRefs};
use super::{
    is_config_changed, planner::CategoryFolder, FilingSummary, FolderRecord, HydrationBatch,
    Intent, PassContext, RescanFilter,
};
use crate::domain::{AccountConfig, FilingMode, MailboxSnapshot, SourceEnvelope};
use crate::engine::{EngineCapabilities, FolderInfo, MailEngine, WriteOutcome};
use crate::store::Store;
use anyhow::Result;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

/// UIDs per `envelopes` call when hydrating, per folder and pass.
const HYDRATE_BATCH: usize = 100;

/// Names that are `special_use` whatever roles the server reports (spec
/// "Folders" rule 3), lowercase; children of `[Gmail]` / `[Google Mail]` too.
const DENYLIST: [&str; 15] = [
    "sent",
    "sent items",
    "sent messages",
    "trash",
    "deleted items",
    "deleted messages",
    "bin",
    "drafts",
    "junk",
    "junk e-mail",
    "spam",
    "archive",
    "all mail",
    "starred",
    "important",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchRole {
    Source,
    Category,
    Retired,
}

#[derive(Debug, Clone)]
pub struct WatchSpec {
    pub folder: String,
    pub role: WatchRole,
}

#[derive(Debug, Clone, Default)]
pub struct FolderMap {
    pub caps: Option<EngineCapabilities>,
    pub sources: Vec<String>,
    pub categories: BTreeMap<String, CategoryFolder>,
    pub watch: Vec<WatchSpec>,
    /// Names LIST reported this pass.
    pub listed: BTreeSet<String>,
    /// dry_run: folders a live pass would create.
    pub would_create: Vec<String>,
    /// Live, MOVE supported, no alias conflict.
    pub writes_allowed: bool,
    /// Folders a client-side alias resolves elsewhere: neither discovered nor
    /// fetched from while the conflict lasts.
    pub alias_conflicts: BTreeSet<String>,
}

impl FolderMap {
    /// Mode off: only the configured sources are watched.
    pub fn sources_only(cfg: &AccountConfig) -> Self {
        let mut map = Self {
            sources: sources_of(cfg),
            ..Default::default()
        };
        for source in map.sources.clone() {
            map.watch(source, WatchRole::Source);
        }
        map
    }

    /// The native folder of a category filed outside `INBOX`.
    pub fn native_for(&self, category_id: &str) -> Option<&str> {
        match self.categories.get(category_id)? {
            CategoryFolder::Native(native) => Some(native),
            CategoryFolder::Inbox => None,
        }
    }

    fn watch(&mut self, folder: String, role: WatchRole) {
        self.watch.push(WatchSpec { folder, role });
    }
}

/// A category filed outside `INBOX`.
struct Wanted {
    id: String,
    configured: String,
    native: String,
}

/// Spec "Folders" rules 1–7, with mode not off. The watch scope is set before
/// any other folder-specific engine call.
pub fn resolve_folders(
    store: &mut Store,
    ctx: &PassContext,
    summary: &mut FilingSummary,
) -> Result<FolderMap> {
    let caps = ctx.engine.capabilities()?;
    summary.capabilities = Some(caps.clone());
    let mut map = FolderMap {
        sources: sources_of(ctx.cfg),
        writes_allowed: ctx.mode == FilingMode::Live && caps.move_supported,
        ..Default::default()
    };
    if ctx.mode == FilingMode::Live && !caps.move_supported {
        summary.problems.push("move_unsupported".into());
    }
    let wanted = category_natives(ctx.cfg, &caps, &mut map, summary);
    let natives: Vec<String> = wanted.iter().map(|w| w.native.clone()).collect();
    let retiring: Vec<FolderRecord> = store
        .folder_records(ctx.account)?
        .into_iter()
        .filter(|r| {
            r.category_id.is_some()
                && !natives.contains(&r.native)
                && !map.sources.contains(&r.native)
        })
        .collect();
    let referenced = referenced_folders(store, ctx.account)?;
    let refs = if retiring.is_empty() {
        RetiredRefs::default()
    } else {
        RetiredRefs::load(store, ctx)?
    };
    let mut scope: BTreeSet<String> = map.sources.iter().chain(&natives).cloned().collect();
    scope.extend(
        retiring
            .iter()
            .filter(|r| referenced.contains(&r.native) || refs.holds(&r.native))
            .map(|r| r.native.clone()),
    );
    ctx.engine
        .set_watch_scope(&scope.into_iter().collect::<Vec<_>>());
    let checked: Vec<String> = map.sources.iter().chain(&natives).cloned().collect();
    let conflicts: BTreeSet<String> = ctx.engine.alias_conflicts(&checked)?.into_iter().collect();
    for folder in &conflicts {
        summary.problems.push(format!("alias_conflict:{folder}"));
        map.writes_allowed = false;
    }
    map.alias_conflicts = conflicts.clone();
    let folders = ctx.engine.list_folders()?;
    map.listed = folders.iter().map(|f| f.name.clone()).collect();
    map.caps = Some(caps);
    let listed: BTreeMap<&str, &FolderInfo> =
        folders.iter().map(|f| (f.name.as_str(), f)).collect();
    resolve_sources(store, ctx, &mut map, &listed, &conflicts)?;
    for w in &wanted {
        let info = listed.get(w.native.as_str()).copied();
        let state = resolve_category(store, ctx, &mut map, summary, info, w)?;
        if matches!(state.as_deref(), Some("ok" | "needs_confirmation"))
            && !conflicts.contains(&w.native)
        {
            map.watch(w.native.clone(), WatchRole::Category);
        }
    }
    retire(store, ctx, &mut map, retiring, &referenced, &refs, summary)?;
    Ok(map)
}

/// The folder map from stored folder records and the configuration only (no
/// engine calls), for `filing plan` and `filing status`. Native names use the
/// personal prefix of the last pass (`""` before any); a category without a
/// usable record (never recorded, or a create that failed) would be created
/// by a live pass, as rule 4 decides, unless its name is denylisted.
/// `listed` holds the recorded folders not known to be missing, `caps` and
/// `alias_conflicts` come from the last pass, nothing is watched and writes
/// are never allowed.
pub fn offline_map(store: &Store, account: &str, cfg: &AccountConfig) -> Result<FolderMap> {
    let last_pass = store.filing_state(account)?.last_pass;
    let caps: Option<EngineCapabilities> = last_pass
        .as_ref()
        .and_then(|p| p.get("capabilities"))
        .and_then(|c| serde_json::from_value(c.clone()).ok());
    let prefix = caps
        .as_ref()
        .map_or(String::new(), |c| c.personal_prefix.clone());
    let records: BTreeMap<String, FolderRecord> = store
        .folder_records(account)?
        .into_iter()
        .map(|r| (r.native.clone(), r))
        .collect();
    let mut map = FolderMap {
        caps,
        sources: sources_of(cfg),
        ..Default::default()
    };
    for category in &cfg.categories {
        let folder = category.effective_folder();
        if folder == "INBOX" {
            map.categories
                .insert(category.id.clone(), CategoryFolder::Inbox);
            continue;
        }
        let native = native_name(&prefix, folder);
        if map.sources.iter().any(|s| same_folder(s, &native)) {
            continue;
        }
        let creatable = records
            .get(&native)
            .is_none_or(|r| r.state == "error" && r.origin.is_none());
        if creatable && !denylisted(&native, &prefix) {
            map.would_create.push(native.clone());
        }
        map.categories
            .insert(category.id.clone(), CategoryFolder::Native(native));
    }
    map.listed = records
        .values()
        .filter(|r| r.state != "missing")
        .map(|r| r.native.clone())
        .collect();
    map.alias_conflicts = last_pass
        .as_ref()
        .and_then(|p| p.get("problems"))
        .and_then(|p| p.as_array())
        .into_iter()
        .flatten()
        .filter_map(|p| p.as_str()?.strip_prefix("alias_conflict:"))
        .map(str::to_string)
        .collect();
    Ok(map)
}

/// The engine of an offline preview: every call fails, so a `filing plan`
/// never reaches the mailbox.
pub struct OfflineEngine;

impl OfflineEngine {
    fn refuse<T>() -> Result<T> {
        anyhow::bail!("offline preview makes no engine calls")
    }
}

impl MailEngine for OfflineEngine {
    fn version(&self) -> Result<String> {
        Self::refuse()
    }
    fn binding_identity(&self) -> Result<serde_json::Value> {
        Self::refuse()
    }
    fn snapshot(&self, _: &str) -> Result<MailboxSnapshot> {
        Self::refuse()
    }
    fn discover(&self, _: &str, _: u64, _: u64) -> Result<Vec<SourceEnvelope>> {
        Self::refuse()
    }
    fn fetch_raw(&self, _: &str, _: u64) -> Result<Vec<u8>> {
        Self::refuse()
    }
    fn capabilities(&self) -> Result<EngineCapabilities> {
        Self::refuse()
    }
    fn list_folders(&self) -> Result<Vec<FolderInfo>> {
        Self::refuse()
    }
    fn create_folder(&self, _: &str) -> Result<()> {
        Self::refuse()
    }
    fn subscribe_folder(&self, _: &str) -> Result<()> {
        Self::refuse()
    }
    fn envelopes(&self, _: &str, _: &[u64]) -> Result<Vec<SourceEnvelope>> {
        Self::refuse()
    }
    fn move_messages(&self, _: &str, _: &[u64], _: &str) -> Result<WriteOutcome> {
        Self::refuse()
    }
    fn add_flagged(&self, _: &str, _: &[u64]) -> Result<WriteOutcome> {
        Self::refuse()
    }
}

/// `doctor`'s filing block (without `mode`), from read-only engine calls
/// only: capabilities, the alias check of the sources and category folders,
/// and LIST. Problems are codes: `move_unsupported`,
/// `folder_collides_with_source:<category>` and `alias_conflict:<folder>`.
pub fn engine_report(engine: &dyn MailEngine, cfg: &AccountConfig) -> Result<serde_json::Value> {
    let caps = engine.capabilities()?;
    let sources = sources_of(cfg);
    let mut problems = Vec::new();
    if !caps.move_supported {
        problems.push("move_unsupported".to_string());
    }
    let mut checked = sources.clone();
    for category in &cfg.categories {
        let folder = category.effective_folder();
        if folder == "INBOX" {
            continue;
        }
        let native = native_name(&caps.personal_prefix, folder);
        if sources.iter().any(|s| same_folder(s, &native)) {
            problems.push(format!("folder_collides_with_source:{}", category.id));
        } else if !checked.contains(&native) {
            checked.push(native);
        }
    }
    let conflicts = engine.alias_conflicts(&checked)?;
    problems.extend(conflicts.iter().map(|f| format!("alias_conflict:{f}")));
    let folders: Vec<_> = engine
        .list_folders()?
        .into_iter()
        .map(|f| json!({"name": f.name, "roles": f.roles, "subscribed": f.subscribed}))
        .collect();
    Ok(json!({
        "move_supported": caps.move_supported,
        "uidplus": caps.uidplus,
        "special_use": caps.special_use,
        "personal_prefix": caps.personal_prefix,
        "alias_conflicts": conflicts,
        "folders": folders,
        "problems": problems,
    }))
}

pub(crate) fn sources_of(cfg: &AccountConfig) -> Vec<String> {
    cfg.engine_config()
        .map(|e| e.mailboxes().to_vec())
        .unwrap_or_default()
}

/// Rule 2: the native name of every category. `INBOX` targets map to
/// `Inbox`; a native name equal to a source is reported and left out, so
/// nothing is ever filed into a source as if it were a category folder.
fn category_natives(
    cfg: &AccountConfig,
    caps: &EngineCapabilities,
    map: &mut FolderMap,
    summary: &mut FilingSummary,
) -> Vec<Wanted> {
    let mut wanted = Vec::new();
    for category in &cfg.categories {
        let folder = category.effective_folder();
        if folder == "INBOX" {
            map.categories
                .insert(category.id.clone(), CategoryFolder::Inbox);
            continue;
        }
        let native = native_name(&caps.personal_prefix, folder);
        if map.sources.iter().any(|s| same_folder(s, &native)) {
            summary
                .problems
                .push(format!("folder_collides_with_source:{}", category.id));
            continue;
        }
        map.categories
            .insert(category.id.clone(), CategoryFolder::Native(native.clone()));
        wanted.push(Wanted {
            id: category.id.clone(),
            configured: folder.to_string(),
            native,
        });
    }
    wanted
}

/// Rule 2: `personal_prefix + folder`, unless the folder already carries it.
fn native_name(prefix: &str, folder: &str) -> String {
    if prefix.is_empty() || folder.starts_with(prefix) {
        folder.to_string()
    } else {
        format!("{prefix}{folder}")
    }
}

/// Native names are compared exactly, except `INBOX`, which IMAP treats case-insensitively.
fn same_folder(a: &str, b: &str) -> bool {
    a == b || (a.eq_ignore_ascii_case("INBOX") && b.eq_ignore_ascii_case("INBOX"))
}

/// Folders an open intent or revert, a pending arrival or a rescan set names.
fn referenced_folders(store: &Store, account: &str) -> Result<BTreeSet<String>> {
    let mut out = store.rescan_folders(account)?;
    for intent in store.intents(account, true)? {
        out.insert(intent.folder);
        out.extend(intent.target);
    }
    for revert in store.reverts(account, true)? {
        out.insert(revert.folder);
        out.insert(revert.target);
    }
    out.extend(
        store
            .arrivals(account, Some("pending"))?
            .into_iter()
            .map(|a| a.folder),
    );
    Ok(out)
}

/// Every source folder: recorded (`ok` when listed, else `missing`) and
/// watched unless an alias conflict stops it.
fn resolve_sources(
    store: &mut Store,
    ctx: &PassContext,
    map: &mut FolderMap,
    listed: &BTreeMap<&str, &FolderInfo>,
    conflicts: &BTreeSet<String>,
) -> Result<()> {
    for source in map.sources.clone() {
        let info = listed
            .iter()
            .find(|(name, _)| same_folder(name, &source))
            .map(|(_, f)| *f);
        let mut rec = store
            .folder_record(ctx.account, &source)?
            .unwrap_or_else(|| blank_record(ctx.account, &source));
        rec.configured = Some(source.clone());
        rec.category_id = None;
        rec.origin = Some("source".into());
        rec.state = if info.is_some() { "ok" } else { "missing" }.into();
        rec.subscribed = info.is_some_and(|f| f.subscribed);
        rec.checked_at = Some(ctx.now.clone());
        rec.error = None;
        store.save_folder(&rec)?;
        if !conflicts.contains(&source) {
            map.watch(source, WatchRole::Source);
        }
    }
    Ok(())
}

/// Rules 3–6 for one category folder. Returns its state; `None` when it has
/// no record (never seen, and not created this pass).
fn resolve_category(
    store: &mut Store,
    ctx: &PassContext,
    map: &mut FolderMap,
    summary: &mut FilingSummary,
    info: Option<&FolderInfo>,
    w: &Wanted,
) -> Result<Option<String>> {
    let prev = store.folder_record(ctx.account, &w.native)?;
    let prev_state = prev.as_ref().map(|p| p.state.clone());
    let mut rec = prev
        .clone()
        .unwrap_or_else(|| blank_record(ctx.account, &w.native));
    rec.category_id = Some(w.id.clone());
    rec.configured = Some(w.configured.clone());
    rec.checked_at = Some(ctx.now.clone());
    let mut events = Vec::new();
    match info {
        Some(f) => {
            rec.error = None;
            rec.subscribed = f.subscribed;
            classify_listed(&mut rec, f, map, &mut events);
        }
        // Never created (it would turn `special_use` by rule 3 next pass) and
        // never `missing`: it stays `special_use` while absent.
        None if denylisted(&w.native, prefix_of(map)) => {
            rec.error = None;
            rec.state = "special_use".into();
        }
        // Rule 4: never recorded, or a create that failed earlier.
        None if prev
            .as_ref()
            .is_none_or(|p| p.state == "error" && p.origin.is_none()) =>
        {
            if ctx.mode == FilingMode::Live && map.writes_allowed {
                create(ctx, &mut rec, summary, &mut events)?;
            } else {
                if ctx.mode == FilingMode::DryRun {
                    map.would_create.push(w.native.clone());
                }
                if prev.is_none() {
                    return Ok(None);
                }
            }
        }
        // Rule 5: never recreated automatically.
        None => {
            rec.error = None;
            rec.state = "missing".into();
        }
    }
    if rec.state == "ok" {
        subscribe(ctx, map, &mut rec, summary, &mut events)?;
    }
    if prev_state.as_deref() != Some(rec.state.as_str()) {
        match rec.state.as_str() {
            "special_use" => events.push("folder_special_use"),
            "needs_confirmation" => events.push("folder_needs_confirmation"),
            "missing" => events.push("folder_missing"),
            _ => {}
        }
    }
    store.save_folder(&rec)?;
    for kind in events {
        let detail = json!({"category_id": w.id});
        store.record_event(ctx.account, None, Some(&w.native), kind, detail, &ctx.now)?;
    }
    Ok(Some(rec.state))
}

/// Rule 3 for a listed folder. Never touches `pause_reason`. Any reported
/// role makes it `special_use` (the engines report only RFC 6154 roles). With
/// roles unknown, a folder verified earlier (created by mailtriage, or seen
/// with known empty roles) or confirmed by the user stays `ok`.
fn classify_listed(
    rec: &mut FolderRecord,
    f: &FolderInfo,
    map: &FolderMap,
    events: &mut Vec<&'static str>,
) {
    let has = |attr: &str| f.attributes.iter().any(|a| a.eq_ignore_ascii_case(attr));
    let prefix = prefix_of(map);
    let state = if has("\\Noselect") || has("\\NonExistent") {
        "noselect"
    } else if f.roles.as_ref().is_some_and(|r| !r.is_empty()) || denylisted(&f.name, prefix) {
        rec.role_verified = false;
        "special_use"
    } else if f.roles.is_some() {
        rec.role_verified = true;
        "ok"
    } else if rec.confirmed || rec.role_verified {
        "ok"
    } else {
        "needs_confirmation"
    };
    rec.state = state.into();
    if state == "ok" && rec.origin.is_none() {
        rec.origin = Some("adopted".into());
        events.push("folder_adopted");
    }
}

fn prefix_of(map: &FolderMap) -> &str {
    map.caps.as_ref().map_or("", |c| c.personal_prefix.as_str())
}

fn denylisted(native: &str, prefix: &str) -> bool {
    let name = native
        .strip_prefix(prefix)
        .unwrap_or(native)
        .to_ascii_lowercase();
    let native = native.to_ascii_lowercase();
    DENYLIST.contains(&name.as_str())
        || ["[gmail]", "[google mail]"]
            .iter()
            .any(|p| name.starts_with(p) || native.starts_with(p))
}

/// Rule 4 in a live pass that may write.
fn create(
    ctx: &PassContext,
    rec: &mut FolderRecord,
    summary: &mut FilingSummary,
    events: &mut Vec<&'static str>,
) -> Result<()> {
    match ctx.engine.create_folder(&rec.native) {
        Ok(()) => {
            rec.state = "ok".into();
            rec.origin = Some("created".into());
            rec.role_verified = true;
            rec.subscribed = false;
            rec.error = None;
            events.push("folder_created");
            summary.folders_created += 1;
            Ok(())
        }
        Err(e) => write_failed(e, rec, "create_failed", summary),
    }
}

/// Rule 6: an `ok` folder outside LSUB is subscribed in a live pass that may write.
fn subscribe(
    ctx: &PassContext,
    map: &FolderMap,
    rec: &mut FolderRecord,
    summary: &mut FilingSummary,
    events: &mut Vec<&'static str>,
) -> Result<()> {
    if rec.subscribed || ctx.mode != FilingMode::Live || !map.writes_allowed {
        return Ok(());
    }
    match ctx.engine.subscribe_folder(&rec.native) {
        Ok(()) => {
            rec.subscribed = true;
            events.push("folder_subscribed");
            Ok(())
        }
        Err(e) => write_failed(e, rec, "subscribe_failed", summary),
    }
}

/// A failed folder write leaves the folder in `error` (retried next pass);
/// a mail engine configuration change aborts the pass instead.
fn write_failed(
    e: anyhow::Error,
    rec: &mut FolderRecord,
    code: &str,
    summary: &mut FilingSummary,
) -> Result<()> {
    if is_config_changed(&e) {
        return Err(e);
    }
    rec.state = "error".into();
    rec.error = Some(code.into());
    summary.errors += 1;
    summary
        .problems
        .push(format!("folder_{code}:{}", rec.native));
    Ok(())
}

/// Rule 7: a recorded category folder no category uses any more is
/// `retired`; it stays watched while listed and still referenced, or while
/// refile retention or draining keeps it (refile spec "Retired folders").
fn retire(
    store: &mut Store,
    ctx: &PassContext,
    map: &mut FolderMap,
    retiring: Vec<FolderRecord>,
    referenced: &BTreeSet<String>,
    refs: &RetiredRefs,
    summary: &mut FilingSummary,
) -> Result<()> {
    for mut rec in retiring {
        if rec.state != "retired" {
            rec.state = "retired".into();
            rec.checked_at = Some(ctx.now.clone());
            store.save_folder(&rec)?;
        }
        let listed = map.listed.contains(&rec.native);
        let referenced = referenced.contains(&rec.native);
        let kept = retired::step(store, ctx, refs, &rec.native, listed, referenced, summary)?;
        if listed && (referenced || kept) {
            map.watch(rec.native, WatchRole::Retired);
        }
    }
    Ok(())
}

fn blank_record(account: &str, native: &str) -> FolderRecord {
    FolderRecord {
        account: account.into(),
        native: native.into(),
        configured: None,
        category_id: None,
        origin: None,
        state: String::new(),
        role_verified: false,
        confirmed: false,
        subscribed: false,
        pause_reason: None,
        epoch: None,
        watch_from_uid: None,
        rescan_epoch: None,
        rescan_below_uid: None,
        rescan_complete: true,
        checked_at: None,
        error: None,
    }
}

/// UIDs of `folder` in `epoch` known from COPYUID (spec "Identity and
/// placements"): target UID → (message id, intent id). A COPYUID pair proves
/// identity whatever state its intent is in now.
pub fn known_targets(intents: &[Intent], folder: &str, epoch: u64) -> BTreeMap<u64, (String, i64)> {
    intents
        .iter()
        .filter(|i| i.target.as_deref() == Some(folder) && i.target_epoch == Some(epoch))
        .filter_map(|i| Some((i.target_uid?, (i.message_id.clone(), i.id))))
        .collect()
}

/// The rescan filter of a category or retired folder whose rescan in `epoch`
/// is in progress; sources are rescanned in full.
pub fn rescan_filter(
    store: &Store,
    account: &str,
    spec: &WatchSpec,
    epoch: u64,
) -> Result<Option<RescanFilter>> {
    if spec.role == WatchRole::Source {
        return Ok(None);
    }
    let Some(rec) = store.folder_record(account, &spec.folder)? else {
        return Ok(None);
    };
    let (false, Some(rescan_epoch), Some(below_uid)) =
        (rec.rescan_complete, rec.rescan_epoch, rec.rescan_below_uid)
    else {
        return Ok(None);
    };
    if rescan_epoch != epoch {
        return Ok(None);
    }
    let rfc_ids = store
        .rescan_members(account, &spec.folder, epoch)?
        .into_iter()
        .map(|(_, rfc)| rfc)
        .collect();
    Ok(Some(RescanFilter { below_uid, rfc_ids }))
}

/// Marks each unfinished rescan complete once its scan reached the reset-time
/// `UIDNEXT - 1` and nothing it found still waits for content.
pub fn update_rescan_completion(
    store: &mut Store,
    ctx: &PassContext,
    _map: &FolderMap,
) -> Result<()> {
    for mut rec in store.folder_records(ctx.account)? {
        let (false, Some(epoch), Some(below_uid)) =
            (rec.rescan_complete, rec.rescan_epoch, rec.rescan_below_uid)
        else {
            continue;
        };
        if store.rescan_finished(ctx.account, &rec.native, epoch, below_uid)? {
            rec.rescan_complete = true;
            store.save_folder(&rec)?;
        }
    }
    Ok(())
}

/// Bootstrap (once per enable) and one hydration batch per watched home folder.
pub fn bootstrap_and_hydrate(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    summary: &mut FilingSummary,
) -> Result<()> {
    if !store.filing_state(ctx.account)?.bootstrap_done {
        store.bootstrap_placements(ctx.account, &map.sources)?;
        store.set_bootstrap_done(ctx.account, true)?;
    }
    let watched: BTreeSet<&str> = map.watch.iter().map(|w| w.folder.as_str()).collect();
    for batch in store.unhydrated_homes(ctx.account, HYDRATE_BATCH)? {
        if !watched.contains(batch.folder.as_str()) {
            continue;
        }
        match hydrate_batch(store, ctx, &batch) {
            Ok(n) => summary.hydrated += n,
            Err(e) if is_config_changed(&e) => return Err(e),
            Err(_) => {
                summary.errors += 1;
                summary
                    .problems
                    .push(format!("hydrate_failed:{}", batch.folder));
            }
        }
    }
    Ok(())
}

/// One `envelopes` call. Stored only when the folder is still in the batch's
/// epoch afterwards: UIDVALIDITY only grows, so the UIDs named the same
/// messages throughout.
fn hydrate_batch(store: &mut Store, ctx: &PassContext, batch: &HydrationBatch) -> Result<usize> {
    let uids: Vec<u64> = batch.members.iter().map(|(uid, _)| *uid).collect();
    let envelopes = ctx.engine.envelopes(&batch.folder, &uids)?;
    if ctx.engine.snapshot(&batch.folder)?.uid_validity != batch.epoch {
        return Ok(0);
    }
    let mut hydrated = 0;
    for env in &envelopes {
        if let Some((_, id)) = batch.members.iter().find(|(uid, _)| *uid == env.uid) {
            store.hydrate(ctx.account, id, env)?;
            hydrated += 1;
        }
    }
    Ok(hydrated)
}
