//! In-memory `MailEngine` for service tests: folders with UIDs, epochs, flags,
//! roles and subscriptions, a call log, injectable faults, and simulated
//! client actions (moves, copies, deletes, flag changes, epoch resets).
//! Envelopes are read from each message's header lines.
//!
//! Call log: `calls()` lists every trait call in order. Write calls read
//! `create NAME`, `subscribe NAME`, `move FOLDER 1,2 -> TARGET` and
//! `flag FOLDER 1,2`. A call refused by `enforce_scope` is logged as
//! `refused <call>`, has no effect, does not consume a fault and is not
//! counted by `write_calls()`. After `fail_with_config_changed` triggers,
//! every call is logged as `config_changed <call>` and fails with
//! `ConfigChanged`, also without effect or write count. A write whose
//! arguments the Himalaya engine would reject before spawning (mailbox names
//! `raw::quote_mailbox` refuses; an empty, zero-UID or over-100 UID set) is
//! logged as `invalid <call>` and fails the same way, before scope checks.
use super::himalaya::{check_write_uids, parse_address};
use super::{
    raw, ConfigChanged, CopyUid, EngineCapabilities, FolderInfo, MailEngine, WriteOutcome,
};
use crate::domain::{MailboxSnapshot, SourceEnvelope};
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FakeOp {
    Move,
    Flag,
    /// `add_seen` (reply queue).
    Seen,
    Create,
    Subscribe,
}

/// A failure consumed by the next call of the matching `FakeOp`. `Create`
/// and `Subscribe` honour `ErrorAfter`; any other fault fails them like
/// `ErrorBefore`. On `Flag`, `PartialCopy` adds the flag but reports a2 NO.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// Return `Err`, no effect.
    ErrorBefore,
    /// Apply the effect, then return `Err` (lost response).
    ErrorAfter,
    /// MOVE copies but does not remove; a2 NO; COPYUID kept.
    PartialCopy,
    /// Reset the source folder's epoch, then execute.
    EpochRaceBefore,
    /// a1 NO; nothing happens.
    NoSelect,
}

#[derive(Clone)]
struct Msg {
    uid: u64,
    raw: Vec<u8>,
    internal_date: String,
    flags: BTreeSet<String>,
}

struct Folder {
    epoch: u64,
    uid_next: u64,
    msgs: Vec<Msg>,
    roles: Vec<String>,
    subscribed: bool,
}

struct State {
    folders: BTreeMap<String, Folder>,
    next_epoch: u64,
    caps: EngineCapabilities,
    binding: String,
    faults: Vec<(FakeOp, Fault)>,
    calls: Vec<(FakeOp, String)>,
    all_calls: Vec<String>,
    /// Configured sources once `enforce_scope` was called.
    sources: Option<BTreeSet<String>>,
    watch_scope: BTreeSet<String>,
    /// When false, `discover` omits transport metadata like a poor server.
    rich_discovery: bool,
    /// Call-log prefix from which the configuration counts as changed.
    config_changed_from: Option<String>,
    config_changed: bool,
    /// Folders `alias_conflicts` reports when asked about them.
    alias_conflicts: BTreeSet<String>,
    /// `alias_conflicts` fails, as for a configuration it cannot parse.
    alias_check_fails: bool,
    /// Folders whose next `snapshot` fails, one entry per failure.
    snapshot_faults: Vec<String>,
}

impl State {
    /// Logs a trait call, refusing it when a folder is outside the enforced scope.
    fn enter(&mut self, op: Option<FakeOp>, call: String, folders: &[&str]) -> Result<()> {
        if let Some(sources) = &self.sources {
            if folders
                .iter()
                .any(|f| !sources.contains(*f) && !self.watch_scope.contains(*f))
            {
                self.all_calls.push(format!("refused {call}"));
                bail!("mailbox is not configured for this account");
            }
        }
        if self
            .config_changed_from
            .as_deref()
            .is_some_and(|prefix| call.starts_with(prefix))
        {
            self.config_changed = true;
        }
        if self.config_changed {
            self.all_calls.push(format!("config_changed {call}"));
            return Err(ConfigChanged.into());
        }
        if let Some(op) = op {
            self.calls.push((op, call.clone()));
        }
        self.all_calls.push(call);
        Ok(())
    }

    /// Mirrors the Himalaya engine's argument checks: refuses (logged as
    /// `invalid <call>`) before any effect, scope check or fault.
    fn validate(&mut self, call: &str, folders: &[&str], uids: Option<&[u64]>) -> Result<()> {
        let checked = folders
            .iter()
            .try_for_each(|f| raw::quote_mailbox(f).map(drop))
            .and_then(|()| uids.map_or(Ok(()), check_write_uids));
        if checked.is_err() {
            self.all_calls.push(format!("invalid {call}"));
        }
        checked
    }

    fn take_fault(&mut self, op: FakeOp) -> Option<Fault> {
        let i = self.faults.iter().position(|(o, _)| *o == op)?;
        Some(self.faults.remove(i).1)
    }

    fn get(&self, name: &str) -> Result<&Folder> {
        self.folders
            .get(name)
            .ok_or_else(|| anyhow!("mailbox does not exist"))
    }

    /// For test helpers, which treat an unknown folder or UID as a test bug.
    fn folder(&mut self, name: &str) -> &mut Folder {
        self.folders
            .get_mut(name)
            .unwrap_or_else(|| panic!("FakeEngine: no folder {name}"))
    }

    fn msg(&mut self, folder: &str, uid: u64) -> &mut Msg {
        self.folder(folder)
            .msgs
            .iter_mut()
            .find(|m| m.uid == uid)
            .unwrap_or_else(|| panic!("FakeEngine: no UID {uid} in {folder}"))
    }

    fn add(&mut self, name: &str, roles: &[&str], subscribed: bool) {
        let epoch = self.new_epoch();
        self.folders.insert(
            name.to_owned(),
            Folder {
                epoch,
                uid_next: 1,
                msgs: Vec::new(),
                roles: roles.iter().map(|r| r.to_string()).collect(),
                subscribed,
            },
        );
    }

    fn new_epoch(&mut self) -> u64 {
        let epoch = self.next_epoch;
        self.next_epoch += 1;
        epoch
    }

    /// Appends under the folder's next UID, which it returns.
    fn append(&mut self, folder: &str, mut msg: Msg) -> u64 {
        let f = self.folder(folder);
        let uid = f.uid_next;
        msg.uid = uid;
        f.uid_next += 1;
        f.msgs.push(msg);
        uid
    }

    fn remove(&mut self, folder: &str, uid: u64) -> Option<Msg> {
        let msgs = &mut self.folder(folder).msgs;
        let i = msgs.iter().position(|m| m.uid == uid)?;
        Some(msgs.remove(i))
    }

    fn reset_epoch(&mut self, folder: &str) {
        let epoch = self.new_epoch();
        let f = self.folder(folder);
        f.epoch = epoch;
        for (i, m) in f.msgs.iter_mut().enumerate() {
            m.uid = i as u64 + 1;
        }
        f.uid_next = f.msgs.len() as u64 + 1;
    }

    /// The SELECT half of a write session. Applies the faults that act before
    /// SELECT and returns the session epoch; `None` means a1 NO.
    fn select(&mut self, folder: &str, fault: Option<Fault>) -> Result<Option<u64>> {
        match fault {
            Some(Fault::ErrorBefore) => bail!("injected fault before the write"),
            Some(Fault::NoSelect) => return Ok(None),
            Some(Fault::EpochRaceBefore) if self.folders.contains_key(folder) => {
                self.reset_epoch(folder)
            }
            _ => {}
        }
        Ok(self.folders.get(folder).map(|f| f.epoch))
    }
}

/// `ErrorAfter` loses the response of a write that already took effect.
fn respond<T>(out: T, fault: Option<Fault>) -> Result<T> {
    if fault == Some(Fault::ErrorAfter) {
        bail!("injected fault after the write");
    }
    Ok(out)
}

/// The first value of a header line, matched case-insensitively.
fn header(raw: &[u8], name: &str) -> Option<String> {
    String::from_utf8_lossy(raw)
        .split('\n')
        .map(|line| line.trim_end_matches('\r'))
        .take_while(|line| !line.is_empty())
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name)
                .then(|| value.trim().to_owned())
        })
}

fn envelope(m: &Msg) -> SourceEnvelope {
    SourceEnvelope {
        uid: m.uid,
        subject: header(&m.raw, "Subject").unwrap_or_default(),
        from: header(&m.raw, "From")
            .map(|from| vec![parse_address(&from)])
            .unwrap_or_default(),
        sent_at: header(&m.raw, "Date"),
        message_id: header(&m.raw, "Message-ID"),
        internal_date: Some(m.internal_date.clone()),
        size: Some(m.raw.len() as u64),
        flags: m.flags.iter().cloned().collect(),
    }
}

/// Clones share one mailbox, so a test keeps a handle while the service owns another.
#[derive(Clone)]
pub struct FakeEngine {
    state: Arc<Mutex<State>>,
}

impl Default for FakeEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeEngine {
    /// INBOX at epoch 1 (later epochs start at 100), MOVE, UIDPLUS and
    /// SPECIAL-USE on, personal prefix "" with delimiter '/', no scope enforcement.
    pub fn new() -> Self {
        let inbox = Folder {
            epoch: 1,
            uid_next: 1,
            msgs: Vec::new(),
            roles: Vec::new(),
            subscribed: true,
        };
        Self {
            state: Arc::new(Mutex::new(State {
                folders: BTreeMap::from([("INBOX".to_owned(), inbox)]),
                next_epoch: 100,
                caps: EngineCapabilities {
                    move_supported: true,
                    uidplus: true,
                    special_use: true,
                    delimiter: Some('/'),
                    personal_prefix: String::new(),
                },
                binding: "fake".into(),
                faults: Vec::new(),
                calls: Vec::new(),
                all_calls: Vec::new(),
                sources: None,
                watch_scope: BTreeSet::new(),
                rich_discovery: true,
                config_changed_from: None,
                config_changed: false,
                alias_conflicts: BTreeSet::new(),
                alias_check_fails: false,
                snapshot_faults: Vec::new(),
            })),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    /// A folder created by the user's client: new epoch, subscribed.
    pub fn add_folder(&self, name: &str, roles: &[&str]) {
        let mut s = self.state();
        assert!(
            !s.folders.contains_key(name),
            "FakeEngine: folder {name} exists"
        );
        s.add(name, roles, true);
    }

    pub fn remove_folder(&self, name: &str) {
        self.state()
            .folders
            .remove(name)
            .unwrap_or_else(|| panic!("FakeEngine: no folder {name}"));
    }

    /// Internal date = now.
    pub fn deliver(&self, folder: &str, raw: &[u8]) -> u64 {
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        self.deliver_at(folder, raw, &now)
    }

    pub fn deliver_at(&self, folder: &str, raw: &[u8], internal_date: &str) -> u64 {
        let msg = Msg {
            uid: 0,
            raw: raw.to_vec(),
            internal_date: internal_date.to_owned(),
            flags: BTreeSet::new(),
        };
        self.state().append(folder, msg)
    }

    /// Keeps the internal date and flags; returns the UID in `to`.
    pub fn client_move(&self, from: &str, uid: u64, to: &str) -> u64 {
        let mut s = self.state();
        let msg = s
            .remove(from, uid)
            .unwrap_or_else(|| panic!("FakeEngine: no UID {uid} in {from}"));
        s.append(to, msg)
    }

    /// Keeps the internal date and flags; returns the UID in `to`.
    pub fn client_copy(&self, from: &str, uid: u64, to: &str) -> u64 {
        let mut s = self.state();
        let msg = s.msg(from, uid).clone();
        s.append(to, msg)
    }

    pub fn client_delete(&self, folder: &str, uid: u64) {
        self.state()
            .remove(folder, uid)
            .unwrap_or_else(|| panic!("FakeEngine: no UID {uid} in {folder}"));
    }

    pub fn client_set_flag(&self, folder: &str, uid: u64, flag: &str, on: bool) {
        let mut s = self.state();
        let flags = &mut s.msg(folder, uid).flags;
        if on {
            flags.insert(flag.to_owned());
        } else {
            flags.remove(flag);
        }
    }

    /// New epoch; messages renumbered from 1 in their current order.
    pub fn reset_epoch(&self, folder: &str) {
        self.state().reset_epoch(folder);
    }

    pub fn set_capabilities(&self, move_supported: bool, uidplus: bool, special_use: bool) {
        let caps = &mut self.state().caps;
        caps.move_supported = move_supported;
        caps.uidplus = uidplus;
        caps.special_use = special_use;
    }

    pub fn set_prefix(&self, prefix: &str, delimiter: char) {
        let caps = &mut self.state().caps;
        caps.personal_prefix = prefix.to_owned();
        caps.delimiter = Some(delimiter);
    }

    pub fn set_binding(&self, binding: &str) {
        self.state().binding = binding.to_owned();
    }

    /// Consumed by the next matching call that is not refused by scope.
    pub fn inject(&self, op: FakeOp, fault: Fault) {
        self.state().faults.push((op, fault));
    }

    /// The next `snapshot(folder)` that is not refused by scope fails
    /// without effect (a transient read error); later ones succeed again.
    pub fn fail_next_snapshot(&self, folder: &str) {
        self.state().snapshot_faults.push(folder.to_owned());
    }

    /// Move, Flag, Create and Subscribe calls, including failed but not scope-refused ones.
    pub fn write_calls(&self) -> usize {
        self.state().calls.len()
    }

    /// Every trait call in order, e.g. `move INBOX 1,2 -> Newsletters`.
    pub fn calls(&self) -> Vec<String> {
        self.state().all_calls.clone()
    }

    pub fn uids(&self, folder: &str) -> Vec<u64> {
        self.state()
            .folder(folder)
            .msgs
            .iter()
            .map(|m| m.uid)
            .collect()
    }

    /// Sorted.
    pub fn flags(&self, folder: &str, uid: u64) -> Vec<String> {
        self.state()
            .msg(folder, uid)
            .flags
            .iter()
            .cloned()
            .collect()
    }

    /// Every `(folder, uid)` holding a message with this Message-ID.
    pub fn locate(&self, message_id: &str) -> Vec<(String, u64)> {
        let s = self.state();
        let mut found = Vec::new();
        for (name, folder) in &s.folders {
            for m in &folder.msgs {
                if header(&m.raw, "Message-ID").as_deref() == Some(message_id) {
                    found.push((name.clone(), m.uid));
                }
            }
        }
        found
    }

    pub fn epoch(&self, folder: &str) -> u64 {
        self.state().folder(folder).epoch
    }

    /// False for a missing folder.
    pub fn subscribed(&self, folder: &str) -> bool {
        self.state()
            .folders
            .get(folder)
            .is_some_and(|f| f.subscribed)
    }

    /// `false`: `discover` omits Message-ID, internal date, size and flags;
    /// `envelopes` always returns them.
    pub fn set_rich_discovery(&self, on: bool) {
        self.state().rich_discovery = on;
    }

    /// Mirrors the Himalaya engine after its TOML changed: from the first trait
    /// call whose log line starts with `from_call` (e.g. `"list"`,
    /// `"discover INBOX"`) on, every call fails with `ConfigChanged`.
    pub fn fail_with_config_changed(&self, from_call: &str) {
        self.state().config_changed_from = Some(from_call.to_owned());
    }

    /// Mirrors a fresh engine open after the TOML changed: it reads the
    /// current configuration, so calls succeed again.
    pub fn reload_config(&self) {
        let mut s = self.state();
        s.config_changed_from = None;
        s.config_changed = false;
    }

    /// Folders that a client-side alias resolves elsewhere; `alias_conflicts`
    /// reports those it is asked about. Replaces the previous set.
    pub fn set_alias_conflicts(&self, folders: &[&str]) {
        self.state().alias_conflicts = folders.iter().map(|f| f.to_string()).collect();
    }

    /// Makes `alias_conflicts` fail (or succeed again), as the Himalaya
    /// engine does for a configuration it cannot read or parse.
    pub fn fail_alias_check(&self, fails: bool) {
        self.state().alias_check_fails = fails;
    }

    /// Strict scope (mirrors the Himalaya engine): when set, every
    /// folder-specific trait call on a folder outside `sources` ∪ the last
    /// `set_watch_scope` list returns Err without effect.
    pub fn enforce_scope(&self, sources: &[&str]) {
        self.state().sources = Some(sources.iter().map(|s| s.to_string()).collect());
    }
}

impl MailEngine for FakeEngine {
    fn version(&self) -> Result<String> {
        self.state().enter(None, "version".into(), &[])?;
        Ok("fake".into())
    }

    fn binding_identity(&self) -> Result<Value> {
        let mut s = self.state();
        s.enter(None, "binding_identity".into(), &[])?;
        Ok(json!({"engine": "fake", "binding": s.binding}))
    }

    fn snapshot(&self, folder: &str) -> Result<MailboxSnapshot> {
        let mut s = self.state();
        s.enter(None, format!("snapshot {folder}"), &[folder])?;
        if let Some(i) = s.snapshot_faults.iter().position(|f| f == folder) {
            s.snapshot_faults.remove(i);
            bail!("injected snapshot failure");
        }
        let f = s.get(folder)?;
        Ok(MailboxSnapshot {
            uid_validity: f.epoch,
            uid_next: f.uid_next,
        })
    }

    fn discover(&self, folder: &str, after: u64, through: u64) -> Result<Vec<SourceEnvelope>> {
        let mut s = self.state();
        s.enter(
            None,
            format!("discover {folder} {after}..{through}"),
            &[folder],
        )?;
        let rich = s.rich_discovery;
        Ok(s.get(folder)?
            .msgs
            .iter()
            .filter(|m| m.uid > after && m.uid <= through)
            .map(envelope)
            .map(|e| {
                if rich {
                    e
                } else {
                    SourceEnvelope {
                        message_id: None,
                        internal_date: None,
                        size: None,
                        flags: Vec::new(),
                        ..e
                    }
                }
            })
            .collect())
    }

    fn fetch_raw(&self, folder: &str, uid: u64) -> Result<Vec<u8>> {
        let mut s = self.state();
        s.enter(None, format!("fetch {folder} {uid}"), &[folder])?;
        s.get(folder)?
            .msgs
            .iter()
            .find(|m| m.uid == uid)
            .map(|m| m.raw.clone())
            .ok_or_else(|| anyhow!("message does not exist"))
    }

    fn capabilities(&self) -> Result<EngineCapabilities> {
        let mut s = self.state();
        s.enter(None, "capabilities".into(), &[])?;
        Ok(s.caps.clone())
    }

    fn list_folders(&self) -> Result<Vec<FolderInfo>> {
        let mut s = self.state();
        s.enter(None, "list".into(), &[])?;
        let special_use = s.caps.special_use;
        Ok(s.folders
            .iter()
            .map(|(name, f)| FolderInfo {
                name: name.clone(),
                attributes: Vec::new(),
                roles: special_use.then(|| f.roles.clone()),
                subscribed: f.subscribed,
            })
            .collect())
    }

    /// Unsubscribed, like IMAP CREATE. An existing folder is an error.
    fn create_folder(&self, native: &str) -> Result<()> {
        let mut s = self.state();
        let call = format!("create {native}");
        s.validate(&call, &[native], None)?;
        s.enter(Some(FakeOp::Create), call, &[native])?;
        let fault = s.take_fault(FakeOp::Create);
        if fault.is_some_and(|f| f != Fault::ErrorAfter) {
            bail!("injected fault before the write");
        }
        if s.folders.contains_key(native) {
            bail!("mailbox already exists");
        }
        s.add(native, &[], false);
        respond((), fault)
    }

    fn subscribe_folder(&self, native: &str) -> Result<()> {
        let mut s = self.state();
        let call = format!("subscribe {native}");
        s.validate(&call, &[native], None)?;
        s.enter(Some(FakeOp::Subscribe), call, &[native])?;
        let fault = s.take_fault(FakeOp::Subscribe);
        if fault.is_some_and(|f| f != Fault::ErrorAfter) {
            bail!("injected fault before the write");
        }
        s.folders
            .get_mut(native)
            .ok_or_else(|| anyhow!("mailbox does not exist"))?
            .subscribed = true;
        respond((), fault)
    }

    fn envelopes(&self, folder: &str, uids: &[u64]) -> Result<Vec<SourceEnvelope>> {
        let mut s = self.state();
        s.enter(
            None,
            format!("envelopes {folder} {}", raw::uid_set(uids)),
            &[folder],
        )?;
        Ok(s.get(folder)?
            .msgs
            .iter()
            .filter(|m| uids.contains(&m.uid))
            .map(envelope)
            .collect())
    }

    fn move_messages(&self, folder: &str, uids: &[u64], target: &str) -> Result<WriteOutcome> {
        self.move_with(folder, uids, target)
    }

    fn add_flagged(&self, folder: &str, uids: &[u64]) -> Result<WriteOutcome> {
        self.store_flag(folder, uids, ("flag", FakeOp::Flag, "\\Flagged"))
    }

    fn add_seen(&self, folder: &str, uids: &[u64]) -> Result<WriteOutcome> {
        self.store_flag(folder, uids, ("seen", FakeOp::Seen, "\\Seen"))
    }

    fn set_watch_scope(&self, folders: &[String]) {
        let mut s = self.state();
        s.all_calls.push(format!("scope {}", folders.join(",")));
        s.watch_scope = folders.iter().cloned().collect();
    }

    fn alias_conflicts(&self, folders: &[String]) -> Result<Vec<String>> {
        let mut s = self.state();
        s.enter(None, format!("alias_conflicts {}", folders.join(",")), &[])?;
        if s.alias_check_fails {
            bail!("cannot read the Himalaya configuration: it is not valid TOML");
        }
        Ok(folders
            .iter()
            .filter(|f| s.alias_conflicts.contains(*f))
            .cloned()
            .collect())
    }
}

impl FakeEngine {
    /// `add_flagged` and `add_seen`: SELECT, then `+FLAGS.SILENT (flag)`.
    fn store_flag(
        &self,
        folder: &str,
        uids: &[u64],
        (verb, op, flag): (&str, FakeOp, &str),
    ) -> Result<WriteOutcome> {
        let mut s = self.state();
        let call = format!("{verb} {folder} {}", raw::uid_set(uids));
        s.validate(&call, &[folder], Some(uids))?;
        s.enter(Some(op), call, &[folder])?;
        let fault = s.take_fault(op);
        let Some(epoch) = s.select(folder, fault)? else {
            return Ok(WriteOutcome::default());
        };
        for m in &mut s.folder(folder).msgs {
            if uids.contains(&m.uid) {
                m.flags.insert(flag.into());
            }
        }
        let out = WriteOutcome {
            selected: true,
            session_epoch: Some(epoch),
            completed: fault != Some(Fault::PartialCopy),
            copyuid: None,
        };
        respond(out, fault)
    }

    fn move_with(&self, folder: &str, uids: &[u64], target: &str) -> Result<WriteOutcome> {
        let mut s = self.state();
        let call = format!("move {folder} {} -> {target}", raw::uid_set(uids));
        s.validate(&call, &[folder, target], Some(uids))?;
        s.enter(Some(FakeOp::Move), call, &[folder, target])?;
        let fault = s.take_fault(FakeOp::Move);
        let Some(epoch) = s.select(folder, fault)? else {
            return Ok(WriteOutcome::default());
        };
        let mut out = WriteOutcome {
            selected: true,
            session_epoch: Some(epoch),
            completed: false,
            copyuid: None,
        };
        // Without MOVE the command fails; a missing target is NO [TRYCREATE].
        if !s.caps.move_supported || !s.folders.contains_key(target) {
            return respond(out, fault);
        }
        let partial = fault == Some(Fault::PartialCopy);
        let mut pairs = Vec::new();
        for &uid in uids {
            let msg = if partial {
                s.get(folder)?.msgs.iter().find(|m| m.uid == uid).cloned()
            } else {
                s.remove(folder, uid)
            };
            if let Some(msg) = msg {
                pairs.push((uid, s.append(target, msg)));
            }
        }
        out.completed = !partial;
        if s.caps.uidplus && !pairs.is_empty() {
            out.copyuid = Some(CopyUid {
                target_epoch: s.get(target)?.epoch,
                pairs,
            });
        }
        respond(out, fault)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::MailEngine;

    fn raw(id: &str) -> Vec<u8> {
        format!("Message-ID: <{id}@t>\r\nSubject: s\r\nFrom: A <a@t>\r\n\r\nbody\r\n").into_bytes()
    }

    #[test]
    fn move_with_copyuid_and_epochs() {
        let f = FakeEngine::new();
        f.add_folder("News", &[]);
        let u = f.deliver("INBOX", &raw("a"));
        let out = f.move_messages("INBOX", &[u], "News").unwrap();
        assert!(out.selected && out.completed);
        assert_eq!(out.session_epoch, Some(f.epoch("INBOX")));
        let cu = out.copyuid.unwrap();
        assert_eq!(cu.target_epoch, f.epoch("News"));
        assert_eq!(f.locate("<a@t>"), vec![("News".to_string(), cu.pairs[0].1)]);
        assert_eq!(f.write_calls(), 1);
    }

    #[test]
    fn faults_and_capabilities() {
        let f = FakeEngine::new();
        f.add_folder("News", &[]);
        let u = f.deliver("INBOX", &raw("a"));
        f.inject(FakeOp::Move, Fault::ErrorAfter);
        assert!(f.move_messages("INBOX", &[u], "News").is_err());
        assert_eq!(f.locate("<a@t>")[0].0, "News");
        let v = f.deliver("INBOX", &raw("b"));
        f.inject(FakeOp::Move, Fault::PartialCopy);
        let out = f.move_messages("INBOX", &[v], "News").unwrap();
        assert!(!out.completed);
        assert_eq!(f.locate("<b@t>").len(), 2);
        let w = f.deliver("INBOX", &raw("c"));
        let before = f.epoch("INBOX");
        f.inject(FakeOp::Move, Fault::EpochRaceBefore);
        let out = f.move_messages("INBOX", &[w], "News").unwrap();
        assert_ne!(out.session_epoch, Some(before));
        f.set_capabilities(false, false, false);
        assert!(!f.capabilities().unwrap().move_supported);
        assert!(f.list_folders().unwrap().iter().all(|x| x.roles.is_none()));
    }

    #[test]
    fn envelopes_and_client_actions() {
        let f = FakeEngine::new();
        f.add_folder("News", &[]);
        let u = f.deliver_at("INBOX", &raw("a"), "2026-01-01T00:00:00+00:00");
        let e = &f.envelopes("INBOX", &[u, 99]).unwrap()[0];
        assert_eq!(e.message_id.as_deref(), Some("<a@t>"));
        assert_eq!(
            e.internal_date.as_deref(),
            Some("2026-01-01T00:00:00+00:00")
        );
        f.client_set_flag("INBOX", u, "\\Flagged", true);
        let n = f.client_move("INBOX", u, "News");
        assert_eq!(f.flags("News", n), vec!["\\Flagged".to_string()]);
        f.reset_epoch("News");
        assert_eq!(f.uids("News"), vec![1]);
    }

    #[test]
    fn invalid_write_arguments_are_refused_like_himalaya() {
        let f = FakeEngine::new();
        f.add_folder("News", &[]);
        let u = f.deliver("INBOX", &raw("a"));
        f.inject(FakeOp::Move, Fault::ErrorAfter);
        assert!(f.move_messages("INBOX", &[], "News").is_err());
        assert!(f.move_messages("INBOX", &[0], "News").is_err());
        let too_many: Vec<u64> = (1..=101).collect();
        assert!(f.move_messages("INBOX", &too_many, "News").is_err());
        assert!(f.move_messages("INBOX", &[u], "A&B").is_err());
        assert!(f.add_flagged("INBOX", &[]).is_err());
        assert!(f.add_flagged("-x", &[u]).is_err());
        assert!(f.create_folder("Grüße").is_err());
        assert!(f.subscribe_folder("").is_err());
        assert_eq!(f.write_calls(), 0, "refused before any write");
        assert!(f
            .calls()
            .contains(&"invalid move INBOX  -> News".to_string()));
        assert_eq!(f.locate("<a@t>"), vec![("INBOX".to_string(), u)]);
        assert!(
            f.move_messages("INBOX", &[u], "News").is_err(),
            "the fault survived the invalid calls"
        );
        assert_eq!(f.locate("<a@t>")[0].0, "News");
    }

    #[test]
    fn enforced_scope_refuses_folders_outside_sources_and_watch_scope() {
        let f = FakeEngine::new();
        f.add_folder("News", &[]);
        let u = f.deliver("INBOX", &raw("a"));
        f.enforce_scope(&["INBOX"]);
        f.inject(FakeOp::Move, Fault::ErrorBefore);
        assert!(f.snapshot("News").is_err());
        assert!(f.envelopes("News", &[1]).is_err());
        assert!(f.move_messages("INBOX", &[u], "News").is_err());
        assert!(f.add_flagged("News", &[1]).is_err());
        assert!(f.create_folder("Promotions").is_err());
        assert!(f.subscribe_folder("News").is_err());
        assert_eq!(f.write_calls(), 0, "refused calls are not write calls");
        assert!(f
            .calls()
            .contains(&"refused move INBOX 1 -> News".to_string()));
        assert!(!f.calls().iter().any(|c| c.starts_with("move ")));
        assert_eq!(f.locate("<a@t>"), vec![("INBOX".to_string(), u)]);
        assert!(f.snapshot("INBOX").is_ok(), "sources stay allowed");
        f.set_watch_scope(&["News".to_string()]);
        assert!(
            f.move_messages("INBOX", &[u], "News").is_err(),
            "the fault survived the refused call"
        );
        let out = f.move_messages("INBOX", &[u], "News").unwrap();
        assert!(out.completed);
        assert_eq!(f.write_calls(), 2);
        f.set_watch_scope(&[]);
        assert!(
            f.snapshot("News").is_err(),
            "the last scope replaces earlier ones"
        );
    }
}
