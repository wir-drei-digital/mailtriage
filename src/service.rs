use crate::{
    config,
    domain::*,
    engine::{self, himalaya::source_binding, MailEngine},
    normalize, policy, provider,
    store::{now, Record, Store},
};
use anyhow::{anyhow, Result};
use chrono::{Duration, Utc};
use fs2::FileExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fmt,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    rc::Rc,
};

#[derive(Debug)]
pub struct ServiceError {
    pub code: i32,
    pub message: String,
}
impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for ServiceError {}
pub(crate) fn err(code: i32, message: impl Into<String>) -> anyhow::Error {
    ServiceError {
        code,
        message: message.into(),
    }
    .into()
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
pub struct Service {
    pub config: AppConfig,
    path: PathBuf,
    config_bytes_hash: String,
    pub store: Store,
    engine_override: Option<Rc<dyn MailEngine>>,
}
impl Service {
    pub fn open(path: &Path) -> Result<Self> {
        let path = fs::canonicalize(path)
            .map_err(|_| err(2, "configuration not found; run init or pass --config"))?;
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
        })
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
        self.store.ensure_account(name,&identity,&generation).map_err(|_|err(5,"account binding changed or state unavailable; verify config and use a new namespace for a different mailbox"))?;
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
        f.try_lock_exclusive()
            .map_err(|_| err(5, "an account worker is already running"))?;
        Ok(f)
    }
    fn verify_source_binding(&self, name: &str, account: &AccountConfig) -> Result<()> {
        let expected: String =
            self.store
                .db
                .query_row("SELECT identity FROM accounts WHERE name=?", [name], |r| {
                    r.get(0)
                })?;
        if binding_identity(account, self.engine_override.as_deref())? != expected {
            return Err(err(5, "Himalaya mailbox identity changed during operation"));
        }
        Ok(())
    }
    fn unchanged(&self) -> Result<bool> {
        Ok(hash(&fs::read(&self.path)?) == self.config_bytes_hash)
    }
    fn require_unchanged(&self) -> Result<()> {
        if !self.unchanged()? {
            return Err(err(
                5,
                "configuration changed during command; retry with current configuration",
            ));
        }
        Ok(())
    }
    pub fn doctor(&mut self, name: &str) -> Result<Value> {
        let (account, _) = self.ensure(name)?;
        let provider_valid = provider::validate_configuration(&self.config.provider).is_ok();
        let key_present = self.config.provider.kind == "fake"
            || std::env::var(&self.config.provider.api_key_env)
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false);
        let transport = match self.engine(&account).map(|e| e.map(|e| e.version())) {
            Ok(None) => json!({"configured":false,"ready":true}),
            Ok(Some(Ok(v))) => json!({"configured":true,"ready":true,"version":v}),
            _ => {
                json!({"configured":true,"ready":false,"error":"Himalaya version/config check failed"})
            }
        };
        Ok(
            json!({"schema_version":1,"account":name,"ready":provider_valid&&key_present&&transport["ready"]==true,"provider":{"kind":self.config.provider.kind,"model":self.config.provider.model,"configuration_valid":provider_valid,"key_present":key_present},"transport":transport,"review_mode":self.config.policy.review_mode,"state_dir":self.config.state_dir,"live_checks_performed":false,"coverage":self.coverage(name)?}),
        )
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
        let outcome = self.process_one(name, &account, &generation, &id, None)?;
        let row = self.required(name, &id)?;
        Ok(
            json!({"schema_version":1,"item":self.item(&row)?,"outcome":outcome,"partial":outcome=="failed"}),
        )
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
        match provider::classify(
            &self.config.provider,
            account,
            &message,
            &self.config.policy,
        ) {
            Ok(result) => {
                if !self.unchanged()? {
                    self.store.fail(
                        id,
                        generation,
                        "configuration changed during classification; retry required",
                        self.config.policy.max_attempts,
                    )?;
                    return Err(err(
                        5,
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
    pub fn sync(&mut self, name: &str, limit: usize) -> Result<Value> {
        if !(1..=1000).contains(&limit) {
            return Err(err(2, "sync limit must be 1..=1000"));
        }
        let _lock = self.lock(name)?;
        let (account, generation) = self.ensure(name)?;
        let engine_config = account.engine_config();
        let h = self.engine(&account)?;
        let mut discovered = 0;
        let mut scan_errors = 0;
        if let (Some(h), Some(cfg)) = (&h, &engine_config) {
            h.version()?;
            for mailbox in cfg.mailboxes() {
                let attempt = (|| -> Result<usize> {
                    let snapshot = h.snapshot(mailbox)?;
                    let last = self.store.checkpoint(name, mailbox, &snapshot)?;
                    let max_uid = snapshot.uid_next.saturating_sub(1);
                    if last > max_uid {
                        return Err(anyhow!("UIDNEXT regressed without epoch reset"));
                    }
                    let through = max_uid.min(last.saturating_add(limit as u64));
                    let envelopes = if through > last {
                        h.discover(mailbox, last, through)?
                    } else {
                        vec![]
                    };
                    let after = h.snapshot(mailbox)?;
                    if after.uid_validity != snapshot.uid_validity
                        || after.uid_next < snapshot.uid_next
                    {
                        return Err(anyhow!("epoch changed while discovering"));
                    }
                    if envelopes.iter().any(|e| e.uid <= last || e.uid > through) {
                        return Err(anyhow!("out of range source UID"));
                    }
                    self.require_unchanged()?;
                    self.verify_source_binding(name, &account)?;
                    let n = self.store.stage(
                        name,
                        mailbox,
                        snapshot.uid_validity,
                        through,
                        &envelopes,
                        &generation,
                        through == max_uid,
                    )?;
                    let cursor = self
                        .store
                        .reconcile_cursor(name, mailbox, snapshot.uid_validity)?
                        .min(through);
                    let end = through.min(cursor.saturating_add(limit as u64));
                    if end > cursor {
                        let present = h.discover(mailbox, cursor, end)?;
                        let latest = h.snapshot(mailbox)?;
                        if latest.uid_validity != snapshot.uid_validity
                            || latest.uid_next < snapshot.uid_next
                        {
                            return Err(anyhow!("epoch changed during reconciliation"));
                        }
                        self.store.reconcile_range(
                            name,
                            mailbox,
                            snapshot.uid_validity,
                            cursor,
                            end,
                            &present.iter().map(|e| e.uid).collect::<Vec<_>>(),
                            end == through,
                        )?;
                    }
                    Ok(n)
                })();
                match attempt {
                    Ok(n) => discovered += n,
                    Err(_) => {
                        scan_errors += 1;
                        self.store.scan_error(name, mailbox)?;
                    }
                }
            }
        }
        let ids = self.store.queued(name, limit)?;
        let mut classified = 0;
        let mut fetched = 0;
        let mut cached = 0;
        let mut failed = 0;
        for id in ids {
            let needed_fetch = self.required(name, &id)?.normalized.is_none();
            match self
                .process_one(name, &account, &generation, &id, h.as_deref())?
                .as_str()
            {
                "classified" => classified += 1,
                "failed" => failed += 1,
                _ => cached += 1,
            }
            if needed_fetch && self.required(name, &id)?.normalized.is_some() {
                fetched += 1;
            }
        }
        let coverage = self.coverage(name)?;
        Ok(
            json!({"schema_version":1,"account":name,"discovered":discovered,"fetched":fetched,"classified":classified,"cached":cached,"pending":coverage["pending_jobs"],"failed":failed,"scan_errors":scan_errors,"partial":failed+scan_errors>0,"coverage":self.coverage(name)?}),
        )
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
        Ok(
            json!({"id":row.id,"account":row.account,"from":row.envelope.get("from").cloned().unwrap_or(json!([])),"subject":row.envelope.get("subject").cloned().unwrap_or(json!("")),"sent_at":row.envelope.get("sent_at").cloned().unwrap_or(Value::Null),"received_at":Value::Null,"first_observed_at":row.observed_at,"classification":{"state":c.state,"urgency":c.urgency,"category_id":c.category_id,"category_name":category_name,"action_required":c.action_required,"taxonomy_revision":c.taxonomy_revision,"model":c.model,"classified_at":c.classified_at,"reasons":c.reasons},"overrides":row.overrides,"review_state":row.review_state,"attention":!reasons.is_empty(),"attention_reasons":reasons,"error":row.error,"source_present":source_present}),
        )
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
        let config_lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.path.with_extension("lock"))?;
        FileExt::try_lock_shared(&config_lock)
            .map_err(|_| err(5, "configuration is being edited"))?;
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
        let mut overrides = row.overrides;
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
        if !self
            .store
            .update_overrides(name, &row.id, &expected, &overrides)?
        {
            return Err(err(5, "message corrections changed concurrently; retry"));
        }
        self.read(name, &row.id)
    }
    pub fn review(&mut self, name: &str, id: &str, done: bool) -> Result<Value> {
        let (_, generation) = self.ensure(name)?;
        let row = self.required(name, id)?;
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
    pub fn apply_categories(&mut self, name: &str, categories: Vec<Category>) -> Result<Value> {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.path.with_extension("lock"))?;
        lock.try_lock_exclusive()
            .map_err(|_| err(5, "configuration is being edited"))?;
        self.require_unchanged()?;
        self.ensure(name)?;
        let previous = self.account(name)?;
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
        config::validate(&updated).map_err(|_| {
            err(
                2,
                "invalid categories; require unique IDs, descriptions and exactly one catch-all",
            )
        })?;
        config::save(&self.path, &updated)?;
        self.config_bytes_hash = hash(&fs::read(&self.path)?);
        self.config = updated;
        self.ensure(name)?;
        self.categories(name)
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
        let selected: Vec<_> = ids.into_iter().take(limit).collect();
        let h = self.engine(&account)?;
        let mut failed = 0;
        for id in &selected {
            if self.process_one(name, &account, &generation, id, h.as_deref())? == "failed" {
                failed += 1;
            }
        }
        Ok(
            json!({"schema_version":1,"matched":matched,"reclassified":selected.len()-failed,"pending":self.coverage(name)?["pending_jobs"],"failed":failed,"partial":failed>0}),
        )
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
    Ok(hash(&serde_json::to_vec(
        &json!({"rubric_version":1,"normalizer_version":1,"taxonomy_revision":account.taxonomy_revision,"categories":category_semantics(&account.categories),"brief":account.brief,"timezone":account.timezone,"provider":config.provider,"policy":config.policy}),
    )?))
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn binding_identity(account: &AccountConfig, engine: Option<&dyn MailEngine>) -> Result<String> {
    let source = match (account.engine_config(), engine) {
        (None, _) => Value::Null,
        (Some(_), Some(engine)) => engine.binding_identity()?,
        (Some(EngineConfig::Himalaya(h)), None) => source_binding(&h)?,
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
}
