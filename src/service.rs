use crate::{
    config,
    domain::*,
    engine::{self, MailEngine},
    filing::{
        self, arrivals, inputs,
        observe::{self, FolderMap, OfflineEngine, WatchRole, WatchSpec},
        planner::{self, Plan},
        transitions::{self, Transition},
        FilingSummary, FilingWrite, FolderRecord, Intent, LocationState, PassContext, Placement,
        StageOptions,
    },
    normalize, policy, provider,
    secrets::{self, KeyCache},
    store::{now, Record, Store},
};
use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};
use fs2::FileExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    rc::Rc,
};

/// What a `ServiceError` is about, for callers that react to a class of
/// error rather than to its message text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ErrorKind {
    #[default]
    Other,
    /// `mailtriage.json` or the mail engine's configuration (the Himalaya
    /// TOML) changed while the command ran; a rerun reads the current one.
    ConfigChanged,
    /// Another command holds the configuration lock to edit `mailtriage.json`.
    ConfigBusy,
    /// Another worker for the account is running.
    AccountBusy,
    /// The account's stored binding no longer matches its configuration.
    BindingConflict,
}

impl ErrorKind {
    /// The stable, machine-readable `reason` the JSON error object carries;
    /// `None` for an error without a kind.
    pub fn reason(self) -> Option<&'static str> {
        match self {
            Self::Other => None,
            Self::ConfigChanged => Some("config_changed"),
            Self::ConfigBusy => Some("config_busy"),
            Self::AccountBusy => Some("account_busy"),
            Self::BindingConflict => Some("binding_conflict"),
        }
    }
}

#[derive(Debug)]
pub struct ServiceError {
    pub code: i32,
    pub message: String,
    pub kind: ErrorKind,
}
impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for ServiceError {}
pub(crate) fn err(code: i32, message: impl Into<String>) -> anyhow::Error {
    err_kind(code, ErrorKind::Other, message)
}
/// An error of `kind`, whose `reason` the JSON error object names.
pub(crate) fn err_kind(code: i32, kind: ErrorKind, message: impl Into<String>) -> anyhow::Error {
    ServiceError {
        code,
        message: message.into(),
        kind,
    }
    .into()
}
/// A configuration change while the command ran: a conflict (exit 5).
fn config_err(message: impl Into<String>) -> anyhow::Error {
    err_kind(5, ErrorKind::ConfigChanged, message)
}

/// The process exit code the CLI reports for `error`.
pub fn exit_code(error: &anyhow::Error) -> i32 {
    if let Some(e) = error.downcast_ref::<ServiceError>() {
        return e.code;
    }
    if error.downcast_ref::<engine::ConfigChanged>().is_some() {
        return 5;
    }
    3
}

/// Whether `error` means `mailtriage.json` or the mail engine's configuration
/// changed while the command ran. Rerunning with the current configuration
/// is the remedy, so `watch` skips such a pass and continues; a changed
/// account binding is not such an error.
pub fn is_config_change(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<ServiceError>()
        .is_some_and(|e| e.kind == ErrorKind::ConfigChanged)
        || filing::is_config_changed(error)
}

#[derive(Debug, Clone)]
pub struct ListOptions {
    pub view: String,
    pub category: Option<String>,
    pub urgency: Option<String>,
    pub action_required: Option<bool>,
    pub limit: usize,
    pub cursor: Option<String>,
}
impl Default for ListOptions {
    fn default() -> Self {
        Self {
            view: "attention".into(),
            category: None,
            urgency: None,
            action_required: None,
            limit: 50,
            cursor: None,
        }
    }
}
/// `filing backfill` scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backfill {
    /// Mail whose hydrated internal date lies within the last N days (1..=3650).
    Days(u32),
    All,
}

/// What `filing retry` lifts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryTarget {
    /// A message's `move_failed`, `duplicate_copy` or `quarantined` block.
    Message(String),
    /// A folder's safety pause.
    Folder(String),
    /// An unresolved arrival: its fetch is requeued.
    Arrival(i64),
}

pub struct Service {
    pub config: AppConfig,
    path: PathBuf,
    config_bytes_hash: String,
    pub store: Store,
    engine_override: Option<Rc<dyn MailEngine>>,
    key: KeyCache,
}
impl Service {
    pub fn open(path: &Path) -> Result<Self> {
        let path = fs::canonicalize(path).map_err(|_| {
            err(
                2,
                "configuration not found; run `mailtriage setup` or pass --config",
            )
        })?;
        let bytes = fs::read(&path)?;
        let mut cfg = config::load(&path).map_err(|_| {
            err(
                2,
                "invalid configuration; check required fields, categories and provider settings",
            )
        })?;
        resolve_paths(&mut cfg, &path);
        fs::create_dir_all(&cfg.state_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&cfg.state_dir, fs::Permissions::from_mode(0o700))?;
        }
        let store = Store::open(&cfg.state_dir.join("mailtriage.sqlite"))?;
        Ok(Self {
            config: cfg,
            path,
            config_bytes_hash: hash(&bytes),
            store,
            engine_override: None,
            key: KeyCache::default(),
        })
    }
    /// Like `open`, but every account with an engine config uses `engine`.
    pub fn open_with_engine(path: &Path, engine: Rc<dyn MailEngine>) -> Result<Self> {
        let mut service = Self::open(path)?;
        service.engine_override = Some(engine);
        Ok(service)
    }
    fn engine(&self, account: &AccountConfig) -> Result<Option<Rc<dyn MailEngine>>> {
        match (&self.engine_override, account.engine_config()) {
            (_, None) => Ok(None),
            (Some(engine), Some(_)) => Ok(Some(Rc::clone(engine))),
            (None, Some(cfg)) => Ok(Some(engine::open(&cfg)?)),
        }
    }
    fn account(&self, name: &str) -> Result<AccountConfig> {
        self.config
            .accounts
            .get(name)
            .cloned()
            .ok_or_else(|| err(2, "unknown account"))
    }
    fn ensure(&mut self, name: &str) -> Result<(AccountConfig, String)> {
        let account = self.account(name)?;
        let generation = generation(&self.config, &account)?;
        let identity = binding_identity(&account, self.engine_override.as_deref())?;
        self.store.ensure_account(name,&identity,&generation).map_err(|_|err_kind(5,ErrorKind::BindingConflict,"account binding changed or state unavailable; verify config and use a new namespace for a different mailbox"))?;
        let cutoff =
            Utc::now() - Duration::hours(self.config.policy.freshness_hours.min(87600) as i64);
        let mut expired = vec![];
        for row in self.store.metadata_records(name)? {
            if row.review_state == "open" && matches!(row.status.as_str(), "ready" | "uncertain") {
                if let Some(at) = row
                    .classification
                    .as_ref()
                    .and_then(|c| c["classified_at"].as_str())
                {
                    if chrono::DateTime::parse_from_rfc3339(at)
                        .map(|d| d < cutoff)
                        .unwrap_or(true)
                    {
                        expired.push(row.id);
                    }
                }
            }
        }
        self.store.requeue(name, &expired, &generation)?;
        Ok((account, generation))
    }
    fn lock(&self, name: &str) -> Result<File> {
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(
                self.config
                    .state_dir
                    .join(format!("worker-{}.lock", hash(name.as_bytes()))),
            )?;
        f.try_lock_exclusive().map_err(|_| {
            err_kind(
                5,
                ErrorKind::AccountBusy,
                "an account worker is already running",
            )
        })?;
        Ok(f)
    }
    fn stored_identity(&self, name: &str) -> Result<String> {
        Ok(self
            .store
            .db
            .query_row("SELECT identity FROM accounts WHERE name=?", [name], |r| {
                r.get(0)
            })?)
    }
    fn verify_source_binding(&self, name: &str, account: &AccountConfig) -> Result<()> {
        check_binding(
            account,
            self.engine_override.as_deref(),
            &self.stored_identity(name)?,
        )
    }
    /// `verify_source_binding` built from owned clones, so a filing step can
    /// call it while the store is mutably borrowed.
    fn binding_verifier(
        &self,
        name: &str,
        account: &AccountConfig,
    ) -> Result<impl Fn() -> Result<()>> {
        let expected = self.stored_identity(name)?;
        let account = account.clone();
        let engine = self.engine_override.clone();
        Ok(move || check_binding(&account, engine.as_deref(), &expected))
    }
    fn unchanged(&self) -> Result<bool> {
        Ok(hash(&fs::read(&self.path)?) == self.config_bytes_hash)
    }
    fn require_unchanged(&self) -> Result<()> {
        if !self.unchanged()? {
            return Err(config_err(
                "configuration changed during command; retry with current configuration",
            ));
        }
        Ok(())
    }
    pub fn doctor(&mut self, name: &str) -> Result<Value> {
        let (account, _) = self.ensure(name)?;
        let provider_valid = provider::validate_configuration(&self.config.provider).is_ok();
        let provider_cfg = &self.config.provider;
        let (key_source, key_error) = if provider_cfg.kind == "fake" {
            (Value::Null, None)
        } else {
            (
                json!(secrets::key_source(provider_cfg)),
                self.key.get(provider_cfg).err().map(|e| e.to_string()),
            )
        };
        let key_present = key_error.is_none();
        let transport = match self.engine(&account).map(|e| e.map(|e| e.version())) {
            Ok(None) => json!({"configured":false,"ready":true}),
            Ok(Some(Ok(v))) => json!({"configured":true,"ready":true,"version":v}),
            _ => {
                json!({"configured":true,"ready":false,"error":"Himalaya version/config check failed"})
            }
        };
        let mut out = json!({"schema_version":1,"account":name,"ready":provider_valid&&key_present&&transport["ready"]==true,"provider":{"kind":self.config.provider.kind,"model":self.config.provider.model,"configuration_valid":provider_valid,"key_source":key_source,"key_present":key_present},"transport":transport,"review_mode":self.config.policy.review_mode,"state_dir":self.config.state_dir,"live_checks_performed":false,"coverage":self.coverage(name)?});
        if let Some(e) = key_error {
            out["provider"]["key_error"] = json!(e);
        }
        if let Some(filing) = self.doctor_filing(&account) {
            out["filing"] = filing;
        }
        Ok(out)
    }
    /// `doctor`'s filing block with filing on and an engine configured, from
    /// read-only engine calls; a failed check is reported, never raised.
    fn doctor_filing(&self, account: &AccountConfig) -> Option<Value> {
        if account.filing.mode == FilingMode::Off {
            return None;
        }
        let report = match self.engine(account) {
            Ok(None) => return None,
            Ok(Some(engine)) => observe::engine_report(engine.as_ref(), account),
            Err(e) => Err(e),
        };
        let mut report = report.unwrap_or_else(|_| {
            json!({"move_supported":null,"uidplus":null,"special_use":null,"personal_prefix":null,"alias_conflicts":[],"folders":[],"problems":["engine_check_failed"]})
        });
        report["mode"] = json!(filing::mode_str(account.filing.mode));
        Some(report)
    }
    pub fn classify(&mut self, name: &str, bytes: &[u8], format: &str) -> Result<Value> {
        let _lock = self.lock(name)?;
        let (account, generation) = self.ensure(name)?;
        let message = match format {
            "rfc822" => normalize::rfc822(bytes, self.config.policy.max_body_chars),
            "json" => normalize::json(bytes, self.config.policy.max_body_chars),
            _ => return Err(err(2, "format must be rfc822 or json")),
        }
        .map_err(|_| err(2, "invalid message input"))?;
        let id = self.store.ingest(name, &message, &generation)?;
        let skipped = if self.store.leasable(&id, &generation)? {
            self.classification_skipped()
        } else {
            None
        };
        let outcome = match &skipped {
            Some(_) => "skipped".to_owned(),
            None => self.process_one(name, &account, &generation, &id, None)?,
        };
        let row = self.required(name, &id)?;
        let mut out = json!({"schema_version":1,"item":self.item(&row)?,"outcome":outcome,"partial":outcome=="failed"});
        mark_skipped(&mut out, skipped);
        Ok(out)
    }
    /// Why classification cannot run in this command: the OpenRouter key is
    /// unavailable. Resolved once per `Service` through the key cache, before
    /// any job is leased, so an unavailable key consumes no attempts and its
    /// mail stays queued. The reason is a fixed key-error string. Callers ask
    /// only when a job is eligible to lease, so an idle pass never runs the
    /// key command.
    fn classification_skipped(&self) -> Option<String> {
        let provider = &self.config.provider;
        if provider.kind != "openrouter" {
            return None;
        }
        self.key.get(provider).err().map(|e| e.to_string())
    }
    fn process_one(
        &mut self,
        name: &str,
        account: &AccountConfig,
        generation: &str,
        id: &str,
        source: Option<&dyn MailEngine>,
    ) -> Result<String> {
        let lease_seconds = self.config.provider.timeout_seconds
            + account
                .engine_config()
                .map(|e| 3 * e.timeout_seconds())
                .unwrap_or(0)
            + 60;
        if !self.store.lease(id, generation, lease_seconds)? {
            return Ok("cached".into());
        }
        let mut row = self.required(name, id)?;
        if row.normalized.is_none() {
            let fetch_result = (|| -> Result<NormalizedMessage> {
                self.verify_source_binding(name, account)?;
                let h = source.ok_or_else(|| anyhow!("source adapter unavailable"))?;
                let (mailbox, epoch, uid) = self
                    .store
                    .locator(name, id)?
                    .ok_or_else(|| anyhow!("source locator unavailable"))?;
                if h.snapshot(&mailbox)?.uid_validity != epoch {
                    return Err(anyhow!("mailbox epoch changed"));
                }
                let raw = h.fetch_raw(&mailbox, uid)?;
                if h.snapshot(&mailbox)?.uid_validity != epoch {
                    return Err(anyhow!("mailbox epoch changed"));
                }
                self.verify_source_binding(name, account)?;
                normalize::rfc822(&raw, self.config.policy.max_body_chars)
            })();
            match fetch_result {
                Ok(msg) => match self.store.attach(name, id, &msg) {
                    Ok(canonical) => {
                        if canonical != id {
                            return Ok("cached".into());
                        }
                        row = self.required(name, id)?;
                    }
                    Err(_) => {
                        self.store.fail(
                            id,
                            generation,
                            "duplicate message with conflicting local state requires review",
                            self.config.policy.max_attempts,
                        )?;
                        arrivals::merge_conflict(
                            &mut self.store,
                            name,
                            id,
                            &msg.raw_sha256,
                            &now(),
                        )?;
                        return Ok("failed".into());
                    }
                },
                Err(_) => {
                    self.store.fail(
                        id,
                        generation,
                        "message fetch failed; source or epoch needs review",
                        self.config.policy.max_attempts,
                    )?;
                    return Ok("failed".into());
                }
            }
        }
        let mut message = row
            .normalized
            .clone()
            .ok_or_else(|| anyhow!("normalized content unavailable"))?;
        if message.body.chars().count() > self.config.policy.max_body_chars {
            message.body = message
                .body
                .chars()
                .take(self.config.policy.max_body_chars)
                .collect();
            message.incomplete = true;
            message
                .warnings
                .push("body_truncated_by_current_policy".into());
        }
        match provider::classify_with_key(
            &self.config.provider,
            account,
            &message,
            &self.config.policy,
            &self.key,
        ) {
            Ok(result) => {
                if !self.unchanged()? {
                    self.store.fail(
                        id,
                        generation,
                        "configuration changed during classification; retry required",
                        self.config.policy.max_attempts,
                    )?;
                    return Err(config_err(
                        "configuration changed during classification; result was not published",
                    ));
                }
                if self
                    .store
                    .finish(id, generation, &serde_json::to_value(result)?)?
                {
                    Ok("classified".into())
                } else {
                    Err(err(
                        5,
                        "classification revision changed; result was not published",
                    ))
                }
            }
            Err(_) => {
                self.store.fail(
                    id,
                    generation,
                    "classification provider failed; check doctor and retry",
                    self.config.policy.max_attempts,
                )?;
                Ok("failed".into())
            }
        }
    }
    /// One pass in the spec's "Sync pass order". A pass that holds the
    /// account lock and names a configured account records a heartbeat: how
    /// it ended (0, 4 partial, or the error's exit code) and its mode.
    pub fn sync(&mut self, name: &str, limit: usize) -> Result<Value> {
        if !(1..=1000).contains(&limit) {
            return Err(err(2, "sync limit must be 1..=1000"));
        }
        let _lock = self.lock(name)?;
        let result = self.sync_locked(name, limit);
        if let Some(account) = self.config.accounts.get(name) {
            let mode = if account.engine_config().is_some() {
                account.filing.mode
            } else {
                FilingMode::Off
            };
            let (partial, code) = match &result {
                Ok(value) => {
                    let partial = value.get("partial").and_then(Value::as_bool) == Some(true);
                    (partial, if partial { 4 } else { 0 })
                }
                Err(e) => (false, exit_code(e)),
            };
            // A heartbeat that cannot be written never hides the pass result.
            let _ = self
                .store
                .record_heartbeat(name, partial, code, filing::mode_str(mode));
        }
        result
    }
    /// The pass itself, under the account lock; the steps Tasks 7 and 8 add
    /// are marked where they belong.
    fn sync_locked(&mut self, name: &str, limit: usize) -> Result<Value> {
        let (account, generation) = self.ensure(name)?;
        // 1. Engine check and filing mode.
        let engine = self.engine(&account)?;
        let mode = engine
            .as_ref()
            .map_or(FilingMode::Off, |_| account.filing.mode);
        let now = now();
        self.store.sync_filing_mode(name, mode, &now)?;
        if let Some(h) = &engine {
            h.version().map_err(abort_on_config_change)?;
        }
        let verify_binding = self.binding_verifier(name, &account)?;
        // `Some` only with filing on.
        let ctx = engine
            .as_deref()
            .filter(|_| mode != FilingMode::Off)
            .map(|h| PassContext {
                account: name,
                cfg: &account,
                engine: h,
                mode,
                generation: &generation,
                now: now.clone(),
                max_attempts: self.config.policy.max_attempts,
                verify_binding: &verify_binding,
            });
        let filing = ctx.as_ref();
        let mut summary = FilingSummary {
            mode: filing::mode_str(mode).into(),
            ..Default::default()
        };
        // 2. Folder resolution.
        let map = resolve_or_sources(&mut self.store, filing, &account, &mut summary)?;
        // 3–4. Discovery and reconciliation of every watched folder; the
        // messages whose occurrence reconciliation removed are re-evaluated.
        let mut removed = Vec::new();
        let (discovered, scan_errors) = match engine.as_deref() {
            Some(h) => {
                let scan = Scan {
                    name,
                    account: &account,
                    generation: &generation,
                    engine: h,
                    mode,
                    limit: limit as u64,
                };
                self.discover_watched(&scan, &map, &mut removed)?
            }
            None => (0, 0),
        };
        // 5. Intent recovery, before any arrival inference.
        if let Some(ctx) = filing {
            self.recover_intents(ctx, &map, &mut summary)?;
        }
        // 6. Fetch and classify, skipped while the key is unavailable.
        let (done, skipped) = self.fetch_and_classify(
            name,
            &account,
            &generation,
            engine.as_deref(),
            filing.map(|_| &map),
            limit,
        )?;
        // 7–10. Arrivals and re-evaluation, bootstrap and hydration, plan and
        // apply, done inference.
        if let Some(ctx) = filing {
            self.locate_and_file(ctx, &map, &removed, &mut summary)?;
        }
        // 11. Summary.
        let mut out = self.sync_response(
            name,
            discovered,
            scan_errors,
            &done,
            filing.map(|_| &summary),
        )?;
        mark_skipped(&mut out, skipped);
        Ok(out)
    }
    /// Steps 7–10 with filing on: arrival resolution, then re-evaluation of
    /// what reconciliation removed; rescan completion (re-evaluating rescan
    /// members not found), bootstrap and hydration; plan and apply, then
    /// re-evaluation of what batch verification dropped; done inference last.
    fn locate_and_file(
        &mut self,
        ctx: &PassContext,
        map: &FolderMap,
        removed: &[String],
        summary: &mut FilingSummary,
    ) -> Result<()> {
        if let Err(e) = arrivals::resolve_arrivals(&mut self.store, ctx, map, summary) {
            step_failed(e, "arrivals_failed", summary)?;
        }
        // Also homes whose occurrence vanished in a pass that then aborted.
        let mut lost = removed.to_vec();
        lost.extend(self.store.orphaned_homes(ctx.account)?);
        reevaluate_step(&mut self.store, ctx, map, &lost, summary)?;
        observe_placements(&mut self.store, ctx, map, summary)?;
        let dropped = self.plan_and_apply(ctx, map, summary)?;
        reevaluate_step(&mut self.store, ctx, map, &dropped, summary)?;
        if let Err(e) = filing::done::reopen_reappeared(&mut self.store, ctx) {
            step_failed(e, "reopen_failed", summary)?;
        }
        if let Err(e) = filing::done::infer_done(&mut self.store, ctx, map, summary) {
            step_failed(e, "done_inference_failed", summary)?;
        }
        Ok(())
    }
    /// Step 5: intent recovery. Before it may write (retries, reverts) the
    /// configuration must be unchanged.
    fn recover_intents(
        &mut self,
        ctx: &PassContext,
        map: &FolderMap,
        summary: &mut FilingSummary,
    ) -> Result<()> {
        if map.writes_allowed {
            self.require_unchanged()?;
        }
        if let Err(e) = filing::recover::recover(&mut self.store, ctx, map, summary) {
            step_failed(e, "recovery_failed", summary)?;
        }
        Ok(())
    }
    /// Step 9: the pure planner over the stored state (a preview in
    /// `dry_run`); in `live`, with the configuration unchanged, its actions are
    /// claimed and applied. Returns the messages batch verification dropped.
    fn plan_and_apply(
        &mut self,
        ctx: &PassContext,
        map: &FolderMap,
        summary: &mut FilingSummary,
    ) -> Result<Vec<String>> {
        if ctx.mode == FilingMode::Live && map.writes_allowed {
            self.require_unchanged()?;
        }
        let store = &mut self.store;
        let result = (|| {
            let preview = ctx.mode == FilingMode::DryRun;
            let plan =
                filing::planner::plan(&filing::inputs::plan_input(store, ctx, map, preview)?);
            summary.planned = plan.actions.len();
            filing::apply::apply(store, ctx, map, &plan, summary)
        })();
        match result {
            Ok(dropped) => Ok(dropped),
            Err(e) => {
                step_failed(e, "apply_failed", summary)?;
                Ok(vec![])
            }
        }
    }
    /// The sync JSON; with filing on it gains `filing`, is stored as the last
    /// pass, and any filing error makes it partial.
    fn sync_response(
        &mut self,
        name: &str,
        discovered: usize,
        scan_errors: usize,
        done: &Processed,
        summary: Option<&FilingSummary>,
    ) -> Result<Value> {
        let coverage = self.coverage(name)?;
        let mut out = json!({"schema_version":1,"account":name,"discovered":discovered,"fetched":done.fetched,"classified":done.classified,"cached":done.cached,"pending":coverage["pending_jobs"],"failed":done.failed,"scan_errors":scan_errors,"partial":done.failed+scan_errors>0,"coverage":coverage});
        if let Some(summary) = summary {
            let filing_json = serde_json::to_value(summary)?;
            self.store.set_last_pass(name, &filing_json)?;
            out["filing"] = filing_json;
            if summary.errors > 0 {
                out["partial"] = json!(true);
            }
        }
        Ok(out)
    }
    /// Steps 3–4 over every watched folder. A failing folder is a scan error;
    /// a mail engine configuration change aborts the pass.
    fn discover_watched(
        &mut self,
        scan: &Scan,
        map: &FolderMap,
        removed: &mut Vec<String>,
    ) -> Result<(usize, usize)> {
        let intents = self.store.intents(scan.name, false)?;
        let (mut discovered, mut scan_errors) = (0, 0);
        for spec in &map.watch {
            match self.discover_folder(scan, spec, &intents, removed) {
                Ok(n) => discovered += n,
                Err(e) if filing::is_config_changed(&e) => return Err(config_changed()),
                Err(_) => {
                    scan_errors += 1;
                    self.store.scan_error(scan.name, &spec.folder)?;
                }
            }
        }
        Ok((discovered, scan_errors))
    }
    /// Discovers new UIDs of one watched folder and reconciles a window of it.
    /// A category or retired folder watched for the first time starts at its
    /// tip, so pre-existing content is never ingested.
    fn discover_folder(
        &mut self,
        scan: &Scan,
        spec: &WatchSpec,
        intents: &[Intent],
        removed: &mut Vec<String>,
    ) -> Result<usize> {
        let (name, h, folder) = (scan.name, scan.engine, spec.folder.as_str());
        let snapshot = h.snapshot(folder)?;
        let max_uid = snapshot.uid_next.saturating_sub(1);
        let last = match spec.role {
            WatchRole::Source => self.store.checkpoint(name, folder, &snapshot)?,
            WatchRole::Category | WatchRole::Retired => self
                .store
                .checkpoint_start_at(name, folder, &snapshot, max_uid)?,
        };
        if last > max_uid {
            return Err(anyhow!("UIDNEXT regressed without epoch reset"));
        }
        let through = max_uid.min(last.saturating_add(scan.limit));
        let envelopes = if through > last {
            h.discover(folder, last, through)?
        } else {
            vec![]
        };
        let after = h.snapshot(folder)?;
        if after.uid_validity != snapshot.uid_validity || after.uid_next < snapshot.uid_next {
            return Err(anyhow!("epoch changed while discovering"));
        }
        if envelopes.iter().any(|e| e.uid <= last || e.uid > through) {
            return Err(anyhow!("out of range source UID"));
        }
        self.require_unchanged()?;
        self.verify_source_binding(name, scan.account)?;
        let known_targets = observe::known_targets(intents, folder, snapshot.uid_validity);
        let rescan = observe::rescan_filter(&self.store, name, spec, snapshot.uid_validity)?;
        let opts = StageOptions {
            record_arrivals: scan.mode != FilingMode::Off,
            known_targets: &known_targets,
            rescan_filter: rescan.as_ref(),
        };
        let n = self.store.stage_with(
            name,
            folder,
            snapshot.uid_validity,
            through,
            &envelopes,
            scan.generation,
            through == max_uid,
            &opts,
        )?;
        removed.extend(self.reconcile_folder(scan, folder, &snapshot, through)?);
        Ok(n)
    }
    /// The next bounded reconciliation window of one folder; returns the
    /// messages whose occurrence disappeared. A folder still in the epoch it
    /// was first watched in is reconciled from its watch point only.
    fn reconcile_folder(
        &mut self,
        scan: &Scan,
        folder: &str,
        snapshot: &MailboxSnapshot,
        through: u64,
    ) -> Result<Vec<String>> {
        let epoch = snapshot.uid_validity;
        let floor = self.store.watch_floor(scan.name, folder, epoch)?;
        let cursor = self
            .store
            .reconcile_cursor(scan.name, folder, epoch)?
            .max(floor)
            .min(through);
        let end = through.min(cursor.saturating_add(scan.limit));
        if end <= cursor {
            return Ok(vec![]);
        }
        let present = scan.engine.discover(folder, cursor, end)?;
        let latest = scan.engine.snapshot(folder)?;
        if latest.uid_validity != epoch || latest.uid_next < snapshot.uid_next {
            return Err(anyhow!("epoch changed during reconciliation"));
        }
        let uids: Vec<u64> = present.iter().map(|e| e.uid).collect();
        self.store
            .reconcile_range_ids(scan.name, folder, epoch, cursor, end, &uids, end == through)
    }
    /// Step 5: fetch and classify queued messages. With `map` (filing on), a
    /// message with an occurrence in a source folder gets its placement (mail
    /// seen only in a category folder is placed by arrival resolution), and a
    /// message whose fetch would read through a conflicting alias stays queued.
    /// With jobs to lease and the key unavailable, nothing is leased and the
    /// key error is returned as the reason classification was skipped.
    fn fetch_and_classify(
        &mut self,
        name: &str,
        account: &AccountConfig,
        generation: &str,
        h: Option<&dyn MailEngine>,
        map: Option<&FolderMap>,
        limit: usize,
    ) -> Result<(Processed, Option<String>)> {
        let mut done = Processed::default();
        let no_conflicts = BTreeSet::new();
        let blocked = map.map_or(&no_conflicts, |m| &m.alias_conflicts);
        let ids = self.store.queued_outside(name, limit, blocked)?;
        if !ids.is_empty() {
            if let Some(reason) = self.classification_skipped() {
                return Ok((done, Some(reason)));
            }
        }
        for id in ids {
            let needed_fetch = self.required(name, &id)?.normalized.is_none();
            match self
                .process_one(name, account, generation, &id, h)?
                .as_str()
            {
                "classified" => done.classified += 1,
                "failed" => done.failed += 1,
                _ => done.cached += 1,
            }
            // The canonical row: a merge redirects `id` to the existing message.
            let row = self.required(name, &id)?;
            if needed_fetch && row.normalized.is_some() {
                done.fetched += 1;
            }
            if let Some(sources) = map.map(|m| m.sources.as_slice()) {
                let occurrences = self.store.occurrences_of(name, &row.id)?;
                if occurrences.iter().any(|(f, _, _)| sources.contains(f)) {
                    self.store.ensure_placement(name, &row.id, sources)?;
                }
            }
        }
        Ok((done, None))
    }
    fn required(&self, name: &str, id: &str) -> Result<Record> {
        self.store
            .record(name, id)?
            .ok_or_else(|| err(2, "message not found in account"))
    }
    fn item(&self, row: &Record) -> Result<Value> {
        let mut c = match &row.classification {
            Some(v) => serde_json::from_value::<Classification>(v.clone())?,
            None => Classification {
                state: row.status.clone(),
                urgency: None,
                category_id: None,
                action_required: None,
                taxonomy_revision: self.account(&row.account)?.taxonomy_revision,
                model: String::new(),
                classified_at: String::new(),
                raw: Value::Null,
                reasons: vec![],
            },
        };
        c.state = row.status.clone();
        if let Some(v) = row.overrides.get("urgency") {
            c.urgency = Some(serde_json::from_value(v.clone())?);
        }
        if let Some(v) = row.overrides.get("category_id") {
            c.category_id = v.as_str().map(str::to_owned);
        }
        if let Some(v) = row.overrides.get("action_required") {
            c.action_required = v.as_bool();
        }
        let source_present = self.store.source_presence(&row.account, &row.id)?;
        let reasons = if source_present == Some(false) && row.status == "ready" {
            vec![]
        } else {
            policy::attention(&c, &row.review_state)
        };
        let account = self.account(&row.account)?;
        let category_name = c
            .category_id
            .as_ref()
            .and_then(|id| account.categories.iter().find(|cat| &cat.id == id))
            .map(|cat| cat.name.clone());
        let placement = self.placement_item(row, &account)?;
        Ok(
            json!({"id":row.id,"account":row.account,"from":row.envelope.get("from").cloned().unwrap_or(json!([])),"subject":row.envelope.get("subject").cloned().unwrap_or(json!("")),"sent_at":row.envelope.get("sent_at").cloned().unwrap_or(Value::Null),"received_at":Value::Null,"first_observed_at":row.observed_at,"classification":{"state":c.state,"urgency":c.urgency,"category_id":c.category_id,"category_name":category_name,"action_required":c.action_required,"taxonomy_revision":c.taxonomy_revision,"model":c.model,"classified_at":c.classified_at,"reasons":c.reasons},"overrides":row.overrides,"review_state":row.review_state,"attention":!reasons.is_empty(),"attention_reasons":reasons,"error":row.error,"source_present":source_present,"placement":placement}),
        )
    }
    /// The `placement` object of an item (spec "Commands"); null without a
    /// placement. `folder` is the home while the location is known;
    /// `pending_action` is `move:<native>` while an explicit request resolves
    /// to a folder other than the home.
    fn placement_item(&self, row: &Record, account: &AccountConfig) -> Result<Value> {
        let Some(p) = self.store.placement(&row.account, &row.id)? else {
            return Ok(Value::Null);
        };
        let has_flag = row
            .envelope
            .get("flags")
            .and_then(Value::as_array)
            .is_some_and(|f| {
                f.iter()
                    .filter_map(Value::as_str)
                    .any(|f| f.eq_ignore_ascii_case("\\Flagged"))
            });
        let folder = p
            .home_folder
            .clone()
            .filter(|_| p.location_state == LocationState::Known);
        let pending_action = match p.desired_target.as_deref() {
            Some(target) => transitions::desired_folder(&self.store, account, &p, target)?
                .filter(|f| p.home_folder.as_ref() != Some(f))
                .map(|f| format!("move:{f}")),
            None => None,
        };
        Ok(json!({
            "folder": folder,
            "location_state": p.location_state.as_str(),
            "filed_by": p.filed_by,
            "pinned": p.pinned,
            "flagged": p.flagged_at.is_some() || has_flag,
            "blocked_reason": p.blocked_reason,
            "pending_action": pending_action,
        }))
    }
    fn coverage(&self, name: &str) -> Result<Value> {
        let mut coverage = self.store.coverage(name)?;
        if let Some(cfg) = self.account(name)?.engine_config() {
            let scans = coverage["scans"].as_array().unwrap();
            let all_configured = cfg.mailboxes().iter().all(|m| {
                scans
                    .iter()
                    .any(|s| s["mailbox"].as_str() == Some(m.as_str()))
            });
            if !all_configured {
                coverage["complete"] = json!(false);
            }
            coverage["configured_mailboxes"] = json!(cfg.mailboxes());
        }
        Ok(coverage)
    }
    pub fn list(&mut self, name: &str, opts: ListOptions) -> Result<Value> {
        self.ensure(name)?;
        if opts.view != "attention" && opts.view != "all" {
            return Err(err(2, "view must be attention or all"));
        }
        if !(1..=500).contains(&opts.limit) {
            return Err(err(2, "list limit must be 1..=500"));
        }
        if opts
            .urgency
            .as_ref()
            .is_some_and(|u| !["low", "medium", "high"].contains(&u.as_str()))
        {
            return Err(err(2, "invalid urgency"));
        }
        if let Some(category) = &opts.category {
            if !self
                .account(name)?
                .categories
                .iter()
                .any(|c| &c.id == category)
            {
                return Err(err(2, "unknown category"));
            }
        }
        let tx = self.store.db.unchecked_transaction()?;
        let revision = self.store.revision()?;
        let query_hash = hash(&serde_json::to_vec(&json!([
            name,
            opts.view,
            opts.category,
            opts.urgency,
            opts.action_required,
            self.config_bytes_hash
        ]))?);
        let offset = if let Some(cursor) = &opts.cursor {
            let fields: Vec<_> = cursor.split(':').collect();
            if fields.len() != 3 || fields[0] != revision.to_string() || fields[2] != query_hash {
                return Err(err(5, "cursor expired or belongs to a different query"));
            }
            fields[1]
                .parse::<usize>()
                .map_err(|_| err(2, "invalid cursor"))?
        } else {
            0
        };
        let mut items = Vec::new();
        for record in self.store.metadata_records(name)? {
            let item = self.item(&record)?;
            if opts.view == "attention" && item["attention"] != true {
                continue;
            }
            if opts
                .category
                .as_ref()
                .is_some_and(|v| item["classification"]["category_id"].as_str() != Some(v))
            {
                continue;
            }
            if opts
                .urgency
                .as_ref()
                .is_some_and(|v| item["classification"]["urgency"].as_str() != Some(v))
            {
                continue;
            }
            if opts
                .action_required
                .is_some_and(|v| item["classification"]["action_required"].as_bool() != Some(v))
            {
                continue;
            }
            items.push(item);
        }
        let total = items.len();
        if offset > total {
            return Err(err(2, "cursor is beyond query results"));
        }
        let end = offset.saturating_add(opts.limit).min(total);
        let next = if end < total {
            Some(format!("{revision}:{end}:{query_hash}"))
        } else {
            None
        };
        let response = json!({"schema_version":1,"items":&items[offset..end],"total":total,"next_cursor":next,"snapshot_revision":revision,"coverage":self.coverage(name)?});
        tx.commit()?;
        Ok(response)
    }
    pub fn read(&mut self, name: &str, id: &str) -> Result<Value> {
        self.ensure(name)?;
        let row = self.required(name, id)?;
        let mut item = self.item(&row)?;
        item["content"] = serde_json::to_value(&row.normalized)?;
        item["source_occurrences"] = json!(self.store.provenance(name, &row.id)?);
        item["model_decision"] = row.classification.unwrap_or(Value::Null);
        Ok(json!({"schema_version":1,"item":item,"content_available":row.normalized.is_some()}))
    }
    pub fn correct(
        &mut self,
        name: &str,
        id: &str,
        patch: Value,
        clear: Option<&str>,
    ) -> Result<Value> {
        let _config_lock = self.shared_config_lock()?;
        self.require_unchanged()?;
        let (account, _) = self.ensure(name)?;
        let row = self.required(name, id)?;
        let map = patch
            .as_object()
            .ok_or_else(|| err(2, "correction must be an object"))?;
        if map.is_empty() && clear.is_none() {
            return Err(err(2, "supply a correction or field to clear"));
        }
        let expected = row.overrides.clone();
        let mut overrides = row.overrides.clone();
        for (key, value) in map {
            match key.as_str() {
                "urgency" => {
                    serde_json::from_value::<Urgency>(value.clone())
                        .map_err(|_| err(2, "invalid urgency"))?;
                }
                "category_id" => {
                    if !account
                        .categories
                        .iter()
                        .any(|c| value.as_str() == Some(c.id.as_str()))
                    {
                        return Err(err(2, "unknown category"));
                    }
                }
                "action_required" => {
                    if !value.is_boolean() {
                        return Err(err(2, "action_required must be boolean"));
                    }
                }
                _ => return Err(err(2, "unknown correction field")),
            }
            overrides[key] = value.clone();
        }
        if let Some(key) = clear {
            if !["urgency", "category_id", "action_required"].contains(&key) {
                return Err(err(2, "unknown correction field"));
            }
            overrides.as_object_mut().unwrap().remove(key);
        }
        let touched = map.contains_key("category_id") || clear == Some("category_id");
        let placement = self.category_transition(name, &account, &row, &overrides, touched)?;
        if !self.store.correct_with_placement(
            name,
            &row.id,
            &expected,
            &overrides,
            placement.as_ref().map(|(read, changed)| (read, changed)),
        )? {
            return Err(err(5, "message corrections changed concurrently; retry"));
        }
        self.read(name, &row.id)
    }
    /// The shared configuration lock commands that may race a pass hold.
    fn shared_config_lock(&self) -> Result<File> {
        let config_lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.path.with_extension("lock"))?;
        FileExt::try_lock_shared(&config_lock)
            .map_err(|_| err_kind(5, ErrorKind::ConfigBusy, "configuration is being edited"))?;
        Ok(config_lock)
    }
    /// With filing on, a correction that sets or clears `category_id` is a
    /// placement transition (spec "Placement transitions"), persisted with
    /// the overrides in one transaction. `None` keeps today's path.
    fn category_transition(
        &self,
        name: &str,
        account: &AccountConfig,
        row: &Record,
        overrides: &Value,
        touched: bool,
    ) -> Result<Option<(Placement, Placement)>> {
        if !touched
            || account.filing.mode == FilingMode::Off
            || self.store.placement(name, &row.id)?.is_none()
        {
            return Ok(None);
        }
        let effective_category = overrides
            .get("category_id")
            .and_then(Value::as_str)
            .or_else(|| {
                row.classification
                    .as_ref()
                    .and_then(|c| c.get("category_id"))
                    .and_then(Value::as_str)
            })
            .map(str::to_owned);
        let t = Transition::CategoryChanged { effective_category };
        let planned =
            transitions::plan_transition(&self.store, name, &row.id, &t, &sources_of(account))?;
        Ok(Some(planned))
    }
    /// `filing pin`: pins the message in its source folder (spec "Placement
    /// transitions"), resolving an ambiguous location first.
    pub fn filing_pin(&mut self, name: &str, id: &str) -> Result<Value> {
        self.placement_command(name, id, Transition::Pin)
    }
    fn placement_command(&mut self, name: &str, id: &str, t: Transition) -> Result<Value> {
        let _config_lock = self.shared_config_lock()?;
        self.require_unchanged()?;
        let (account, _) = self.ensure(name)?;
        let row = self.required(name, id)?;
        let sources = sources_of(&account);
        transitions::apply_transition(&mut self.store, name, &row.id, t, &sources, &now())?;
        self.read(name, &row.id)
    }
    /// `filing unpin`: automatic filing applies again, once.
    pub fn filing_unpin(&mut self, name: &str, id: &str) -> Result<Value> {
        self.placement_command(name, id, Transition::Unpin)
    }
    /// `filing retry`: lifts a message's block or quarantine, releases a
    /// folder's safety pause (scheduling a rescan of it), or requeues an
    /// unresolved arrival's fetch.
    pub fn filing_retry(&mut self, name: &str, target: RetryTarget) -> Result<Value> {
        match target {
            RetryTarget::Message(id) => self.placement_command(name, &id, Transition::Retry),
            RetryTarget::Folder(folder) => {
                let _config_lock = self.shared_config_lock()?;
                self.require_unchanged()?;
                self.ensure(name)?;
                transitions::release_folder(&mut self.store, name, &folder, &now())?;
                self.folder_result(name, &folder)
            }
            RetryTarget::Arrival(arrival) => {
                let _config_lock = self.shared_config_lock()?;
                self.require_unchanged()?;
                let (_, generation) = self.ensure(name)?;
                transitions::retry_arrival(&mut self.store, name, arrival, &generation, &now())?;
                self.arrival_result(name, arrival)
            }
        }
    }
    /// `filing dismiss`: a reviewed unresolved arrival stops holding done
    /// inference, and a merge-conflict block it caused is lifted.
    pub fn filing_dismiss(&mut self, name: &str, arrival: i64) -> Result<Value> {
        let _config_lock = self.shared_config_lock()?;
        self.require_unchanged()?;
        self.ensure(name)?;
        transitions::dismiss_arrival(&mut self.store, name, arrival, &now())?;
        self.arrival_result(name, arrival)
    }
    /// `filing adopt`: confirms an existing folder whose role could not be
    /// verified; the next pass makes it `ok`.
    pub fn filing_adopt(&mut self, name: &str, folder: &str) -> Result<Value> {
        let _config_lock = self.shared_config_lock()?;
        self.require_unchanged()?;
        self.ensure(name)?;
        transitions::adopt_folder(&mut self.store, name, folder, &now())?;
        self.folder_result(name, folder)
    }
    fn folder_result(&self, name: &str, folder: &str) -> Result<Value> {
        Ok(
            json!({"schema_version":1,"account":name,"ok":true,"folder":self.store.folder_record(name, folder)?}),
        )
    }
    fn arrival_result(&self, name: &str, arrival: i64) -> Result<Value> {
        Ok(
            json!({"schema_version":1,"account":name,"ok":true,"arrival":self.store.arrival(name, arrival)?}),
        )
    }
    /// `filing log`: filing events, newest first, optionally of one message.
    pub fn filing_log(&mut self, name: &str, id: Option<&str>, limit: usize) -> Result<Value> {
        if !(1..=500).contains(&limit) {
            return Err(err(2, "log limit must be 1..=500"));
        }
        self.ensure(name)?;
        let id = id
            .map(|id| self.required(name, id))
            .transpose()?
            .map(|r| r.id);
        let items = self.store.events(name, id.as_deref(), limit)?;
        Ok(json!({"schema_version":1,"account":name,"items":items}))
    }
    /// `filing enable`: gives every category without one an explicit folder
    /// (its name), validates the folders and writes the mode, under the
    /// exclusive configuration lock. Idempotent.
    pub fn filing_enable(&mut self, name: &str, mode: FilingMode) -> Result<Value> {
        if mode == FilingMode::Off {
            return Err(err(2, "filing enable needs mode dry_run or live"));
        }
        if self.account(name)?.engine_config().is_none() {
            return Err(err(2, "account has no mail engine configured"));
        }
        let _lock = self.exclusive_config_lock()?;
        self.require_unchanged()?;
        self.ensure(name)?;
        self.write_filing_mode(name, mode)
    }
    /// `filing disable`: writes mode `off`; the next pass stops filing.
    pub fn filing_disable(&mut self, name: &str) -> Result<Value> {
        let _lock = self.exclusive_config_lock()?;
        self.require_unchanged()?;
        self.write_filing_mode(name, FilingMode::Off)
    }
    /// Edits the configuration as stored (relative paths stay relative); an
    /// unchanged configuration is not rewritten.
    fn write_filing_mode(&mut self, name: &str, mode: FilingMode) -> Result<Value> {
        let stored = config::load(&self.path).map_err(|_| err(2, "invalid configuration"))?;
        let mut updated = stored.clone();
        let a = updated
            .accounts
            .get_mut(name)
            .ok_or_else(|| err(2, "unknown account"))?;
        if mode != FilingMode::Off {
            for c in a.categories.iter_mut().filter(|c| c.folder.is_none()) {
                c.folder = Some(c.name.clone());
            }
        }
        a.filing.mode = mode;
        validate_edit(&updated, name)?;
        if serde_json::to_value(&updated)? != serde_json::to_value(&stored)? {
            config::save(&self.path, &updated)?;
            self.config_bytes_hash = hash(&fs::read(&self.path)?);
            resolve_paths(&mut updated, &self.path);
            self.config = updated;
        }
        let account = self.account(name)?;
        let folders: serde_json::Map<String, Value> = account
            .categories
            .iter()
            .map(|c| (c.id.clone(), json!(c.effective_folder())))
            .collect();
        Ok(
            json!({"schema_version":1,"account":name,"mode":filing::mode_str(mode),"folders":folders}),
        )
    }
    /// `filing status`: configuration and stored state only, no engine calls.
    pub fn filing_status(&mut self, name: &str) -> Result<Value> {
        let (account, _) = self.ensure(name)?;
        let state = self.store.filing_state(name)?;
        let map = observe::offline_map(&self.store, name, &account)?;
        let folders = self.store.folder_records(name)?;
        let sources = sources_of(&account);
        let mut intents = BTreeMap::<String, usize>::new();
        for intent in self.store.intents(name, false)? {
            *intents.entry(intent.state).or_default() += 1;
        }
        let moving = open_moves(&self.store, name)?;
        let (mut blocked, mut quarantined, mut ambiguous) = (vec![], vec![], vec![]);
        let (mut eligible, mut stale) = (0, vec![]);
        for (_, p, meta) in self.store.records_for_planning(name)? {
            match p.blocked_reason.as_deref() {
                Some("quarantined") => quarantined.push(p.message_id.clone()),
                Some(reason) => blocked.push(json!({"id": p.message_id, "blocked_reason": reason})),
                None => {}
            }
            if p.location_state == LocationState::Ambiguous {
                ambiguous.push(p.message_id.clone());
            }
            if unfiled_in_source(&p, &sources)
                && !p.pinned
                && !moving.contains(&p.message_id)
                && (p.eligible_once || inputs::is_new(&p, &meta, state.enabled_at.as_deref()))
            {
                eligible += 1;
            }
            // The planner leaves a request for a removed category pending.
            if p.desired_target
                .as_deref()
                .is_some_and(|t| t != "@source" && !account.categories.iter().any(|c| c.id == t))
            {
                stale.push(p.message_id);
            }
        }
        let unresolved = self.store.arrivals(name, Some("unresolved"))?;
        let last_pass = state.last_pass.clone().unwrap_or(Value::Null);
        Ok(json!({
            "schema_version": 1,
            "account": name,
            "mode": filing::mode_str(account.filing.mode),
            "engine_configured": account.engine_config().is_some(),
            "state_mode": state.mode,
            "enabled_at": state.enabled_at,
            "bootstrap_done": state.bootstrap_done,
            "capabilities": last_pass.get("capabilities").cloned().unwrap_or(Value::Null),
            "folders": folders,
            "paused_categories": paused_categories(&account, &map, &folders),
            "intents": intents,
            "blocked": blocked.len(),
            "blocked_ids": listed(&blocked),
            "quarantined": quarantined.len(),
            "quarantined_ids": listed(&quarantined),
            "ambiguous": ambiguous.len(),
            "ambiguous_ids": listed(&ambiguous),
            "unresolved_arrivals": unresolved.len(),
            "unresolved_arrival_items": listed(&unresolved),
            "eligible_unfiled": eligible,
            "stale_requests": {"count": stale.len(), "ids": listed(&stale)},
            "alias_conflicts": map.alias_conflicts,
            "problems": last_pass.get("problems").cloned().unwrap_or(json!([])),
            "last_pass": last_pass,
        }))
    }
    /// `filing plan`: the planner over stored state as a preview, with the
    /// offline folder map; read-only, no engine calls. Empty with filing off.
    pub fn filing_plan(&mut self, name: &str, limit: usize) -> Result<Value> {
        if !(1..=500).contains(&limit) {
            return Err(err(2, "plan limit must be 1..=500"));
        }
        let (account, generation) = self.ensure(name)?;
        let mode = filing_mode(&account);
        let plan = if mode == FilingMode::Off {
            Plan::default()
        } else {
            let map = observe::offline_map(&self.store, name, &account)?;
            let no_binding_check = || -> Result<()> { Ok(()) };
            let ctx = PassContext {
                account: name,
                cfg: &account,
                engine: &OfflineEngine,
                mode,
                generation: &generation,
                now: now(),
                max_attempts: self.config.policy.max_attempts,
                verify_binding: &no_binding_check,
            };
            planner::plan(&inputs::plan_input(&self.store, &ctx, &map, true)?)
        };
        let shown = &plan.actions[..plan.actions.len().min(limit)];
        Ok(
            json!({"schema_version":1,"account":name,"mode":filing::mode_str(mode),"folders_to_create":plan.folders_to_create,"actions":shown,"total":plan.actions.len()}),
        )
    }
    /// `filing backfill`: placements homed in a source folder that are
    /// unfiled, unpinned, unblocked and not yet eligible once; with `apply`
    /// (mode `live` only) each becomes eligible once in one transaction.
    pub fn filing_backfill(&mut self, name: &str, scope: Backfill, apply: bool) -> Result<Value> {
        let since = match scope {
            Backfill::All => None,
            Backfill::Days(days) if (1..=3650).contains(&days) => {
                Some(Utc::now() - Duration::days(days.into()))
            }
            Backfill::Days(_) => return Err(err(2, "backfill days must be 1..=3650")),
        };
        if !apply {
            let (account, _) = self.ensure(name)?;
            let ids: Vec<String> =
                backfill_candidates(&self.store, name, &sources_of(&account), since)?
                    .into_iter()
                    .map(|p| p.message_id)
                    .collect();
            return Ok(
                json!({"schema_version":1,"account":name,"matched":ids.len(),"items":listed(&ids)}),
            );
        }
        let _config_lock = self.shared_config_lock()?;
        self.require_unchanged()?;
        let (account, _) = self.ensure(name)?;
        if filing_mode(&account) != FilingMode::Live {
            return Err(err(2, "backfill --apply requires filing mode live"));
        }
        let sources = sources_of(&account);
        let scope = match scope {
            Backfill::All => json!("all"),
            Backfill::Days(days) => json!({ "days": days }),
        };
        for _ in 0..5 {
            let read = backfill_candidates(&self.store, name, &sources, since)?;
            if read.is_empty() {
                return Ok(json!({"schema_version":1,"account":name,"applied":0}));
            }
            let changed: Vec<Placement> = read
                .iter()
                .map(|p| Placement {
                    eligible_once: true,
                    desired_rev: p.desired_rev + 1,
                    ..p.clone()
                })
                .collect();
            let mut writes: Vec<FilingWrite> = read
                .iter()
                .zip(&changed)
                .map(|(read, placement)| FilingWrite::PlacementFrom { placement, read })
                .collect();
            writes.push(FilingWrite::Event {
                message_id: None,
                folder: None,
                kind: "backfill",
                detail: json!({"scope": scope, "applied": read.len()}),
            });
            if self.store.commit_filing(name, &writes, &now())? {
                return Ok(json!({"schema_version":1,"account":name,"applied":read.len()}));
            }
        }
        Err(err(5, "placements changed concurrently; retry"))
    }
    pub fn review(&mut self, name: &str, id: &str, done: bool) -> Result<Value> {
        let (_, generation) = self.ensure(name)?;
        let row = self.required(name, id)?;
        // Sets the review state and `done_inferred = 0` in one transaction.
        self.store.review(name, &row.id, done)?;
        if !done && row.generation != generation {
            self.store
                .requeue(name, std::slice::from_ref(&row.id), &generation)?;
        }
        self.read(name, &row.id)
    }
    pub fn categories(&mut self, name: &str) -> Result<Value> {
        let account = self.account(name)?;
        Ok(
            json!({"schema_version":1,"account":name,"taxonomy_revision":account.taxonomy_revision,"categories":account.categories}),
        )
    }
    /// The exclusive configuration lock of a command that edits the file.
    fn exclusive_config_lock(&self) -> Result<File> {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.path.with_extension("lock"))?;
        lock.try_lock_exclusive()
            .map_err(|_| err_kind(5, ErrorKind::ConfigBusy, "configuration is being edited"))?;
        Ok(lock)
    }
    /// With filing on, a category without `folder` keeps the folder its id
    /// had before (a rename does not move its folder); a new one uses its name.
    pub fn apply_categories(&mut self, name: &str, categories: Vec<Category>) -> Result<Value> {
        let _lock = self.exclusive_config_lock()?;
        self.require_unchanged()?;
        self.ensure(name)?;
        let ids: BTreeSet<_> = categories.iter().map(|c| c.id.as_str()).collect();
        for row in self.store.metadata_records(name)? {
            if let Some(id) = row.overrides["category_id"].as_str() {
                if !ids.contains(id) {
                    return Err(err(
                        2,
                        "category has manual corrections; remap or clear them before removal",
                    ));
                }
            }
        }
        let updated = self.categories_candidate(name, categories)?;
        config::save(&self.path, &updated)?;
        self.config_bytes_hash = hash(&fs::read(&self.path)?);
        self.config = updated;
        self.ensure(name)?;
        self.categories(name)
    }
    /// `categories validate --account`: the configuration checks of `apply`
    /// against the account's current configuration and filing mode, folder
    /// inheritance included. Writes nothing.
    pub fn validate_categories(&self, name: &str, categories: Vec<Category>) -> Result<Value> {
        let count = categories.len();
        self.categories_candidate(name, categories)?;
        Ok(json!({"schema_version":1,"account":name,"valid":true,"categories":count}))
    }
    /// The validated configuration with `categories` applied to account
    /// `name`: folders inherited with filing on, and the taxonomy revision
    /// advanced when the category semantics change.
    fn categories_candidate(&self, name: &str, categories: Vec<Category>) -> Result<AppConfig> {
        let previous = self.account(name)?;
        let mut categories = categories;
        if previous.filing.mode != FilingMode::Off {
            for c in categories.iter_mut().filter(|c| c.folder.is_none()) {
                let folder = match previous.categories.iter().find(|p| p.id == c.id) {
                    Some(before) => before.effective_folder().to_string(),
                    None => c.name.clone(),
                };
                c.folder = Some(folder);
            }
        }
        let semantic_changed =
            category_semantics(&previous.categories) != category_semantics(&categories);
        let mut updated = self.config.clone();
        let a = updated.accounts.get_mut(name).unwrap();
        a.categories = categories;
        if semantic_changed {
            a.taxonomy_revision = a
                .taxonomy_revision
                .checked_add(1)
                .ok_or_else(|| err(2, "taxonomy revision overflow"))?;
        }
        validate_edit(&updated, name)?;
        Ok(updated)
    }
    pub fn reclassify(
        &mut self,
        name: &str,
        since: Option<&str>,
        dry_run: bool,
        limit: usize,
    ) -> Result<Value> {
        let _lock = self.lock(name)?;
        let (account, generation) = self.ensure(name)?;
        if !(1..=1000).contains(&limit) {
            return Err(err(2, "limit must be 1..=1000"));
        }
        let since = since
            .map(|s| {
                chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                    .map_err(|_| err(2, "since must be YYYY-MM-DD"))
            })
            .transpose()?;
        let ids: Vec<String> = self
            .store
            .records(name)?
            .into_iter()
            .filter(|r| {
                since.is_none_or(|date| {
                    chrono::DateTime::parse_from_rfc3339(&r.observed_at)
                        .map(|t| t.date_naive() >= date)
                        .unwrap_or(false)
                })
            })
            .map(|r| r.id)
            .collect();
        if dry_run {
            return Ok(
                json!({"schema_version":1,"dry_run":true,"matched":ids.len(),"will_process":ids.len().min(limit)}),
            );
        }
        let matched = ids.len();
        self.store.requeue(name, &ids, &generation)?;
        let mut selected: Vec<_> = ids.into_iter().take(limit).collect();
        let skipped = if selected.is_empty() {
            None
        } else {
            self.classification_skipped()
        };
        if skipped.is_some() {
            selected.clear();
        }
        let h = self.engine(&account)?;
        let mut failed = 0;
        for id in &selected {
            if self.process_one(name, &account, &generation, id, h.as_deref())? == "failed" {
                failed += 1;
            }
        }
        let mut out = json!({"schema_version":1,"matched":matched,"reclassified":selected.len()-failed,"pending":self.coverage(name)?["pending_jobs"],"failed":failed,"partial":failed>0});
        mark_skipped(&mut out, skipped);
        Ok(out)
    }
    pub fn export(&mut self, name: &str) -> Result<Value> {
        self.ensure(name)?;
        let tx = self.store.db.unchecked_transaction()?;
        let rows = self.store.records(name)?;
        let mut items = vec![];
        for row in rows {
            items.push(json!({"metadata":self.item(&row)?,"content":row.normalized,"classification":row.classification,"source_occurrences":self.store.provenance(name,&row.id)?}));
        }
        let mut stmt=self.store.db.prepare("SELECT a.message_id,a.created_at,a.generation,a.result,a.error FROM attempts a JOIN messages m ON m.id=a.message_id WHERE m.account=? ORDER BY a.id")?;
        let attempts=stmt.query_map([name],|r|Ok(json!({"message_id":r.get::<_,String>(0)?,"created_at":r.get::<_,String>(1)?,"generation":r.get::<_,String>(2)?,"result":r.get::<_,Option<String>>(3)?,"error":r.get::<_,Option<String>>(4)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        let out = json!({"schema_version":1,"exported_at":now(),"account":name,"categories":self.account(name)?.categories,"items":items,"attempts":attempts,"coverage":self.coverage(name)?,"snapshot_revision":self.store.revision()?});
        tx.commit()?;
        Ok(out)
    }
}
/// Whether the binding stored for `account` in the state database of the
/// config at `config_path` (which need not exist yet) matches the one `cfg`
/// gives it, computed as `ensure` computes it. `None` without a state
/// database or without a row for the account. Reads only: the state
/// directory and the database are never created.
pub fn stored_binding_matches(
    config_path: &Path,
    cfg: &AppConfig,
    account: &str,
) -> Result<Option<bool>> {
    let config_path =
        fs::canonicalize(config_path).or_else(|_| std::path::absolute(config_path))?;
    let mut cfg = cfg.clone();
    resolve_paths(&mut cfg, &config_path);
    let db = cfg.state_dir.join("mailtriage.sqlite");
    if !db.is_file() {
        return Ok(None);
    }
    let Some(stored) = Store::stored_identity(&db, account)? else {
        return Ok(None);
    };
    let account = cfg
        .accounts
        .get(account)
        .ok_or_else(|| err(2, "unknown account"))?;
    Ok(Some(binding_identity(account, None)? == stored))
}
fn resolve_paths(cfg: &mut AppConfig, path: &Path) {
    let base = path.parent().unwrap_or(Path::new("."));
    if cfg.state_dir.is_relative() {
        cfg.state_dir = base.join(&cfg.state_dir);
    }
    for a in cfg.accounts.values_mut() {
        if let Some(EngineConfig::Himalaya(h)) = &mut a.engine {
            if h.config.is_relative() {
                h.config = base.join(&h.config);
            }
            if h.binary.components().count() > 1 && h.binary.is_relative() {
                h.binary = base.join(&h.binary);
            }
        }
    }
}
fn category_semantics(categories: &[Category]) -> Value {
    let mut cats:Vec<_>=categories.iter().map(|c|json!({"id":c.id,"description":c.description,"examples":c.examples,"catch_all":c.catch_all})).collect();
    cats.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    json!(cats)
}
fn generation(config: &AppConfig, account: &AccountConfig) -> Result<String> {
    // The key command is how the key is fetched, not what classifies mail.
    let mut provider = serde_json::to_value(&config.provider)?;
    if let Some(fields) = provider.as_object_mut() {
        fields.remove("api_key_command");
    }
    Ok(hash(&serde_json::to_vec(
        &json!({"rubric_version":1,"normalizer_version":1,"taxonomy_revision":account.taxonomy_revision,"categories":category_semantics(&account.categories),"brief":account.brief,"timezone":account.timezone,"provider":provider,"policy":config.policy}),
    )?))
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn check_binding(
    account: &AccountConfig,
    engine: Option<&dyn MailEngine>,
    expected: &str,
) -> Result<()> {
    if binding_identity(account, engine)? != expected {
        return Err(err(5, "Himalaya mailbox identity changed during operation"));
    }
    Ok(())
}

/// A result whose classification was skipped gains
/// `classification: {skipped: true, reason}` and is partial.
fn mark_skipped(out: &mut Value, skipped: Option<String>) {
    if let Some(reason) = skipped {
        out["classification"] = json!({"skipped": true, "reason": reason});
        out["partial"] = json!(true);
    }
}

fn config_changed() -> anyhow::Error {
    config_err("mail engine configuration changed during operation")
}

fn abort_on_config_change(e: anyhow::Error) -> anyhow::Error {
    if filing::is_config_changed(&e) {
        config_changed()
    } else {
        e
    }
}

/// A failed filing step counts as an error with a code-only problem; a mail
/// engine configuration change aborts the pass instead.
fn step_failed(e: anyhow::Error, code: &str, summary: &mut FilingSummary) -> Result<()> {
    if filing::is_config_changed(&e) {
        return Err(config_changed());
    }
    summary.errors += 1;
    summary.problems.push(code.into());
    Ok(())
}

/// Step 2: the folder map. With filing off, or when resolution fails, only
/// the sources are watched.
fn resolve_or_sources(
    store: &mut Store,
    filing: Option<&PassContext>,
    account: &AccountConfig,
    summary: &mut FilingSummary,
) -> Result<FolderMap> {
    let Some(ctx) = filing else {
        return Ok(FolderMap::sources_only(account));
    };
    match observe::resolve_folders(store, ctx, summary) {
        Ok(map) => Ok(map),
        Err(e) => {
            step_failed(e, "folder_resolution_failed", summary)?;
            Ok(FolderMap::sources_only(account))
        }
    }
}

/// Placement re-evaluation of `ids` (spec "Placement re-evaluation").
fn reevaluate_step(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    ids: &[String],
    summary: &mut FilingSummary,
) -> Result<()> {
    if let Err(e) = arrivals::reevaluate(store, ctx, map, ids, summary) {
        step_failed(e, "reevaluation_failed", summary)?;
    }
    Ok(())
}

/// Rescan completion (with the re-evaluation of rescan members not found),
/// then step 8, bootstrap and hydration.
fn observe_placements(
    store: &mut Store,
    ctx: &PassContext,
    map: &FolderMap,
    summary: &mut FilingSummary,
) -> Result<()> {
    if let Err(e) = observe::update_rescan_completion(store, ctx, map) {
        step_failed(e, "rescan_completion_failed", summary)?;
    }
    if let Err(e) = arrivals::complete_rescans(store, ctx, map, summary) {
        step_failed(e, "rescan_completion_failed", summary)?;
    }
    if let Err(e) = observe::bootstrap_and_hydrate(store, ctx, map, summary) {
        step_failed(e, "bootstrap_failed", summary)?;
    }
    Ok(())
}

/// The configured source folders of an account.
fn sources_of(account: &AccountConfig) -> Vec<String> {
    account
        .engine_config()
        .map(|e| e.mailboxes().to_vec())
        .unwrap_or_default()
}

/// At most this many ids or rows in a filing listing (`status`, `backfill`).
const LISTED: usize = 50;

/// The first `LISTED` entries.
fn listed<T>(all: &[T]) -> &[T] {
    &all[..all.len().min(LISTED)]
}

/// Messages with an open move intent: moved or being moved, but not yet
/// confirmed, so their placement still names the old home.
fn open_moves(store: &Store, name: &str) -> Result<BTreeSet<String>> {
    Ok(store
        .intents(name, true)?
        .into_iter()
        .filter(|i| i.kind == "move")
        .map(|i| i.message_id)
        .collect())
}

/// The filing mode a pass would run: the configured one, `off` without an engine.
fn filing_mode(account: &AccountConfig) -> FilingMode {
    match account.engine_config() {
        Some(_) => account.filing.mode,
        None => FilingMode::Off,
    }
}

/// `config::validate` for an edited configuration. With filing on, folder
/// problems name the categories that need a valid folder, then the source
/// mailboxes filing cannot write to safely.
fn validate_edit(updated: &AppConfig, name: &str) -> Result<()> {
    config::validate(updated).map_err(|_| {
        let account = &updated.accounts[name];
        let filing_on = account.filing.mode != FilingMode::Off;
        let ids = if filing_on {
            config::invalid_folder_ids(account)
        } else {
            vec![]
        };
        let sources = if filing_on {
            config::unsafe_source_mailboxes(account)
        } else {
            vec![]
        };
        if !ids.is_empty() {
            err(
                2,
                format!("categories need a valid folder: {}", ids.join(", ")),
            )
        } else if !sources.is_empty() {
            let names: Vec<String> = sources.iter().map(|m| format!("{m:?}")).collect();
            err(
                2,
                format!(
                    "source mailboxes are not safe to file from: {}; use {}",
                    names.join(", "),
                    config::SOURCE_RULE
                ),
            )
        } else {
            err(
                2,
                "invalid categories; require unique IDs, descriptions and exactly one catch-all",
            )
        }
    })
}

/// Known in a source folder and never filed.
fn unfiled_in_source(p: &Placement, sources: &[String]) -> bool {
    p.location_state == LocationState::Known
        && p.filed_at.is_none()
        && p.home_folder.as_ref().is_some_and(|h| sources.contains(h))
}

/// What `filing backfill` would make eligible: unfiled placements in a source
/// folder, not pinned, not blocked, without an open move intent and not
/// already eligible once (so a repeated apply changes nothing); with `since`,
/// only mail whose hydrated internal date is at or after it. In message id order.
fn backfill_candidates(
    store: &Store,
    name: &str,
    sources: &[String],
    since: Option<DateTime<Utc>>,
) -> Result<Vec<Placement>> {
    let moving = open_moves(store, name)?;
    Ok(store
        .records_for_planning(name)?
        .into_iter()
        .filter(|(_, p, meta)| {
            unfiled_in_source(p, sources)
                && !p.pinned
                && p.blocked_reason.is_none()
                && !p.eligible_once
                && !moving.contains(&p.message_id)
                && since.is_none_or(|since| inputs::dated_since(meta, since))
        })
        .map(|(_, p, _)| p)
        .collect())
}

/// Categories filed outside `INBOX` whose folder cannot receive mail now: its
/// native name collides with a source, or its recorded folder is not `ok` or
/// is under a safety pause. A folder never recorded is not paused; a pass
/// creates it.
fn paused_categories(
    account: &AccountConfig,
    map: &FolderMap,
    folders: &[FolderRecord],
) -> Vec<String> {
    account
        .categories
        .iter()
        .filter(|c| c.effective_folder() != "INBOX")
        .filter(|c| match map.native_for(&c.id) {
            None => true,
            Some(native) => folders
                .iter()
                .find(|r| r.native == native)
                .is_some_and(|r| r.state != "ok" || r.pause_reason.is_some()),
        })
        .map(|c| c.id.clone())
        .collect()
}

/// Discovery inputs shared by every watched folder of one pass.
struct Scan<'a> {
    name: &'a str,
    account: &'a AccountConfig,
    generation: &'a str,
    engine: &'a dyn MailEngine,
    mode: FilingMode,
    limit: u64,
}

/// Step 5 counters of a sync response.
#[derive(Default)]
struct Processed {
    classified: usize,
    fetched: usize,
    cached: usize,
    failed: usize,
}

fn binding_identity(account: &AccountConfig, engine: Option<&dyn MailEngine>) -> Result<String> {
    let source = match (account.engine_config(), engine) {
        (None, _) => Value::Null,
        (Some(_), Some(engine)) => engine.binding_identity()?,
        (Some(cfg), None) => engine::binding_source(&cfg)?,
    };
    Ok(hash(&serde_json::to_vec(
        &json!({"identity":account.identity,"source":source}),
    )?))
}

#[cfg(test)]
mod golden {
    use crate::domain::HimalayaConfig;

    #[test]
    fn binding_identity_is_unchanged_for_legacy_himalaya_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let toml = dir.path().join("himalaya.toml");
        std::fs::write(&toml, "[accounts.work]\nimap.server='imaps://example.test'\nimap.sasl.plain.username='work@example.test'\nimap.sasl.plain.password.cmd='pass show work'\n").unwrap();
        let mut cfg = crate::config::default_config();
        let a = cfg.accounts.get_mut("work").unwrap();
        a.identity = "work@example.test".into();
        a.himalaya = Some(HimalayaConfig {
            binary: "himalaya".into(),
            config: toml,
            account: "work".into(),
            mailboxes: vec!["INBOX".into()],
            expected_version: "2.1.0".into(),
            timeout_seconds: 30,
            max_output_bytes: 1_000_000,
        });
        crate::config::normalize(&mut cfg).unwrap();
        let a = &cfg.accounts["work"];
        assert_eq!(
            super::binding_identity(a, None).unwrap(),
            "c4a294832a82e8e274fb8bcdf9c2c0daa895f811efe13776ab60da3c7fd50aaa"
        );
    }

    #[test]
    fn generation_hash_is_unchanged_by_filing_and_folder_fields() {
        let mut cfg = crate::config::default_config();
        let expected = "6dfd7bf30e6bf1c0b27ee97af991ecf21dc5ac0400044c2f3adbda7f79d37514";
        assert_eq!(
            super::generation(&cfg, &cfg.accounts["work"]).unwrap(),
            expected
        );
        let a = cfg.accounts.get_mut("work").unwrap();
        a.filing.mode = crate::domain::FilingMode::Live;
        a.categories[0].folder = Some("Mail".into());
        assert_eq!(
            super::generation(&cfg, &cfg.accounts["work"]).unwrap(),
            expected
        );
    }

    #[test]
    fn generation_hash_ignores_the_key_command() {
        let mut cfg = crate::config::default_config();
        cfg.provider.api_key_command = Some(vec![
            "/usr/bin/security".into(),
            "find-generic-password".into(),
        ]);
        assert_eq!(
            super::generation(&cfg, &cfg.accounts["work"]).unwrap(),
            "6dfd7bf30e6bf1c0b27ee97af991ecf21dc5ac0400044c2f3adbda7f79d37514"
        );
    }
}

#[cfg(test)]
mod error_reasons {
    use super::{config_err, err, err_kind, ErrorKind, ServiceError};

    #[test]
    fn each_kind_maps_to_its_stable_reason() {
        assert_eq!(ErrorKind::Other.reason(), None);
        assert_eq!(ErrorKind::ConfigChanged.reason(), Some("config_changed"));
        assert_eq!(ErrorKind::ConfigBusy.reason(), Some("config_busy"));
        assert_eq!(ErrorKind::AccountBusy.reason(), Some("account_busy"));
        assert_eq!(
            ErrorKind::BindingConflict.reason(),
            Some("binding_conflict")
        );
    }

    #[test]
    fn errors_carry_their_code_kind_and_message() {
        let e = err_kind(5, ErrorKind::AccountBusy, "busy");
        let e = e.downcast_ref::<ServiceError>().unwrap();
        assert_eq!(
            (e.code, e.kind, e.message.as_str()),
            (5, ErrorKind::AccountBusy, "busy")
        );
        let e = err(2, "plain");
        assert_eq!(
            e.downcast_ref::<ServiceError>().unwrap().kind,
            ErrorKind::Other
        );
        let e = config_err("changed");
        let e = e.downcast_ref::<ServiceError>().unwrap();
        assert_eq!((e.code, e.kind), (5, ErrorKind::ConfigChanged));
    }
}
