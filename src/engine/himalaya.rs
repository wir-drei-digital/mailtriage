//! Narrow Himalaya v2.1.0 IMAP adapter. Its only writes are folder create and
//! subscribe, UID MOVE and adding \Flagged, the last two through `imap raw`.
use super::{
    raw, ConfigChanged, EngineCapabilities, FolderInfo, MailEngine, WriteOutcome, SPECIAL_USE_ROLES,
};
use crate::domain::{Address, HimalayaConfig, MailboxSnapshot, SourceEnvelope};
use crate::service::err;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::thread;
use std::time::{Duration, Instant};

const MAX_UIDS_PER_FETCH: u64 = 100;
const MAX_UIDS_PER_WRITE: usize = 100;

pub struct Himalaya {
    config: HimalayaConfig,
    /// SHA-256 of the TOML read at open; every spawn re-checks it.
    config_hash: String,
    caps: OnceCell<EngineCapabilities>,
    /// Folders beyond `config.mailboxes` that this pass may touch.
    scope: RefCell<BTreeSet<String>>,
}

/// How a Himalaya process ended.
enum Ending {
    Exited(ExitStatus),
    /// Killed at the deadline.
    TimedOut,
    /// An output limit was exceeded; stdout is a prefix of what was written.
    Overflowed,
}

/// The stdout of one Himalaya process, kept even when it was killed.
struct Captured {
    stdout: Vec<u8>,
    ending: Ending,
}

impl Captured {
    fn finished(&self) -> bool {
        matches!(self.ending, Ending::Exited(_))
    }
}

/// One row of `imap list` JSON.
struct FolderRow {
    name: String,
    attributes: Vec<String>,
}

impl Himalaya {
    pub fn new(config: &HimalayaConfig) -> Result<Self> {
        if config.account.trim().is_empty() || config.mailboxes.is_empty() {
            bail!("Himalaya account and mailboxes must be configured");
        }
        if config.expected_version != "2.1.0" {
            bail!("Himalaya compatibility target must be 2.1.0");
        }
        if config.timeout_seconds == 0 || config.timeout_seconds > 600 {
            bail!("Himalaya timeout must be between 1 and 600 seconds");
        }
        if config.max_output_bytes == 0 || config.max_output_bytes > 256 * 1024 * 1024 {
            bail!("Himalaya output limit must be between 1 byte and 256 MiB");
        }
        if !config.config.is_file() {
            bail!("Himalaya configuration file is missing");
        }
        let toml = fs::read(&config.config)
            .map_err(|_| anyhow!("Himalaya configuration file cannot be read"))?;
        Ok(Self {
            config: config.clone(),
            config_hash: sha256_hex(&toml),
            caps: OnceCell::new(),
            scope: RefCell::new(BTreeSet::new()),
        })
    }

    pub fn version(&self) -> Result<String> {
        let output = self.run(&["--version"], false)?;
        let output_text =
            std::str::from_utf8(&output).context("invalid Himalaya version output")?;
        let version = output_text.lines().next().unwrap_or_default().trim();
        let mut words = version.split_ascii_whitespace();
        let expected = format!("v{}", self.config.expected_version);
        if words.next() != Some("himalaya")
            || words.next() != Some(expected.as_str())
            || !words.any(|feature| feature == "+imap")
        {
            bail!(
                "unsupported Himalaya version; expected {}",
                self.config.expected_version
            );
        }
        Ok(version.to_owned())
    }

    pub fn snapshot(&self, mailbox: &str) -> Result<MailboxSnapshot> {
        self.check_mailbox(mailbox)?;
        let output = self.run(&["imap", "status", mailbox], true)?;
        let status: Value =
            serde_json::from_slice(&output).context("invalid Himalaya status JSON")?;
        let uid_validity = status
            .get("uid_validity")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("Himalaya status lacks uid_validity"))?;
        let uid_next = status
            .get("uid_next")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("Himalaya status lacks uid_next"))?;
        if uid_validity == 0 || uid_next == 0 {
            bail!("Himalaya status contains invalid UID epoch");
        }
        Ok(MailboxSnapshot {
            uid_validity,
            uid_next,
        })
    }

    /// Discover UIDs in the inclusive interval `(after_uid, through_uid]`.
    pub fn discover(
        &self,
        mailbox: &str,
        after_uid: u64,
        through_uid: u64,
    ) -> Result<Vec<SourceEnvelope>> {
        self.check_mailbox(mailbox)?;
        if through_uid <= after_uid {
            return Ok(Vec::new());
        }
        let before = self.snapshot(mailbox)?;
        if through_uid >= before.uid_next {
            bail!("UID discovery range exceeds mailbox snapshot");
        }
        let mut envelopes = Vec::new();
        let mut start = after_uid
            .checked_add(1)
            .ok_or_else(|| anyhow!("UID range overflow"))?;
        while start <= through_uid {
            let end = start
                .saturating_add(MAX_UIDS_PER_FETCH - 1)
                .min(through_uid);
            envelopes.extend(self.fetch_envelopes(mailbox, &format!("{start}:{end}"))?);
            start = end
                .checked_add(1)
                .ok_or_else(|| anyhow!("UID range overflow"))?;
        }
        let after = self.snapshot(mailbox)?;
        if before.uid_validity != after.uid_validity || after.uid_next < before.uid_next {
            bail!("mailbox UID epoch changed during discovery");
        }
        envelopes.sort_by_key(|e| e.uid);
        envelopes.dedup_by_key(|e| e.uid);
        Ok(envelopes)
    }

    pub fn fetch(&self, mailbox: &str, uid: u64) -> Result<Vec<u8>> {
        self.check_mailbox(mailbox)?;
        if uid == 0 {
            bail!("UID must be positive");
        }
        let before = self.snapshot(mailbox)?;
        if uid >= before.uid_next {
            bail!("UID is outside mailbox snapshot");
        }
        let uid_text = uid.to_string();
        // No --json: raw mode writes the exact RFC 5322 bytes. No --seen.
        let raw = self.run(
            &["message", "read", "--mailbox", mailbox, "--raw", &uid_text],
            false,
        )?;
        let after = self.snapshot(mailbox)?;
        if before.uid_validity != after.uid_validity || after.uid_next < before.uid_next {
            bail!("mailbox UID epoch changed during fetch");
        }
        if raw.is_empty() {
            bail!("Himalaya returned an empty raw message");
        }
        Ok(raw)
    }

    /// Passes for a configured source mailbox or a folder in the watch scope.
    fn check_mailbox(&self, mailbox: &str) -> Result<()> {
        if !self.config.mailboxes.iter().any(|m| m == mailbox)
            && !self.scope.borrow().contains(mailbox)
        {
            bail!("mailbox is not configured for this account");
        }
        Ok(())
    }

    /// `--json imap fetch` for one UID set of at most 100 UIDs.
    fn fetch_envelopes(&self, mailbox: &str, set: &str) -> Result<Vec<SourceEnvelope>> {
        let requested: HashSet<u64> = raw::parse_uid_set(set)?.into_iter().collect();
        let output = self.run(
            &[
                "imap",
                "fetch",
                "--mailbox",
                mailbox,
                "--envelope",
                "--flags",
                "--internal-date",
                "--size",
                set,
            ],
            true,
        )?;
        let fetched: Value =
            serde_json::from_slice(&output).context("invalid Himalaya fetch JSON")?;
        let messages = fetched
            .get("messages")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("Himalaya fetch lacks messages"))?;
        let mut envelopes = Vec::with_capacity(messages.len());
        for message in messages {
            let uid = message
                .get("uid")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow!("Himalaya fetch lacks UID"))?;
            if !requested.contains(&uid) {
                bail!("Himalaya fetch returned UID outside requested range");
            }
            let env = message
                .get("envelope")
                .and_then(Value::as_object)
                .ok_or_else(|| anyhow!("Himalaya fetch lacks envelope"))?;
            let subject = env
                .get("subject")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let sent_at = env.get("date").and_then(Value::as_str).map(str::to_owned);
            let from = match env.get("from") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(values)) => values
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(parse_address)
                            .ok_or_else(|| anyhow!("invalid Himalaya sender"))
                    })
                    .collect::<Result<Vec<_>>>()?,
                Some(_) => bail!("invalid Himalaya envelope.from"),
            };
            let flags = match message.get("flags") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(values)) => values
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| anyhow!("invalid Himalaya flags"))
                    })
                    .collect::<Result<Vec<_>>>()?,
                Some(_) => bail!("invalid Himalaya flags"),
            };
            let size = match message.get("size") {
                None | Some(Value::Null) => None,
                Some(v) => Some(v.as_u64().ok_or_else(|| anyhow!("invalid Himalaya size"))?),
            };
            envelopes.push(SourceEnvelope {
                uid,
                subject,
                from,
                sent_at,
                message_id: optional_str(env.get("message_id"), "envelope.message_id")?,
                internal_date: optional_str(message.get("internal_date"), "internal_date")?,
                size,
                flags,
            });
        }
        Ok(envelopes)
    }

    /// The TOML bytes, provided they still hash to what `new` read.
    fn current_config(&self) -> Result<Vec<u8>> {
        let bytes = fs::read(&self.config.config).map_err(|_| ConfigChanged)?;
        if sha256_hex(&bytes) != self.config_hash {
            return Err(ConfigChanged.into());
        }
        Ok(bytes)
    }

    fn raw_text(&self, text: &str) -> Result<Captured> {
        self.spawn_capture(&["imap", "raw", "--", text], false)
    }

    /// Special-use roles per folder name from `LIST ... RETURN (SPECIAL-USE)`.
    fn listed_roles(&self) -> Result<HashMap<String, Vec<String>>> {
        let captured = self.raw_text("a1 LIST \"\" \"*\" RETURN (SPECIAL-USE)\r\n")?;
        let response = raw::parse(&captured.stdout);
        if !captured.finished() || response.completion("a1") != Some(raw::Completion::Ok) {
            bail!("IMAP LIST for special-use roles failed");
        }
        Ok(response
            .list
            .into_iter()
            .map(|line| (line.name, roles_of(&line.attributes)))
            .collect())
    }

    /// Runs Himalaya and returns stdout; errors on timeout, overflow or a
    /// non-zero exit.
    fn run(&self, args: &[&str], json: bool) -> Result<Vec<u8>> {
        let captured = self.spawn_capture(args, json)?;
        match captured.ending {
            Ending::TimedOut => bail!("Himalaya command timed out"),
            Ending::Overflowed => bail!("Himalaya output limit exceeded"),
            Ending::Exited(status) if !status.success() => {
                bail!("Himalaya command failed with status {status}")
            }
            Ending::Exited(_) => Ok(captured.stdout),
        }
    }

    /// Runs Himalaya and keeps whatever stdout it produced, even when it had
    /// to be killed. Refuses to spawn if the TOML changed since open.
    fn spawn_capture(&self, args: &[&str], json: bool) -> Result<Captured> {
        self.current_config()?;
        let mut command = Command::new(&self.config.binary);
        command
            .arg("--config")
            .arg(&self.config.config)
            .arg("--account")
            .arg(&self.config.account)
            .arg("--backend")
            .arg("imap");
        if json {
            command.arg("--json");
        }
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().context("failed to launch Himalaya")?;
        let limit = self.config.max_output_bytes;
        let overflow = Arc::new(AtomicBool::new(false));
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("missing Himalaya stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("missing Himalaya stderr"))?;
        let (sender, chunks) = mpsc::channel::<Vec<u8>>();
        let stdout_overflow = Arc::clone(&overflow);
        let output_thread = thread::spawn(move || {
            read_bounded(stdout, limit, &stdout_overflow, |chunk| {
                sender.send(chunk.to_vec()).is_ok()
            })
        });
        let stderr_overflow = Arc::clone(&overflow);
        // Intentionally do not keep stderr: it may contain credentials or mail.
        let stderr_thread =
            thread::spawn(move || read_bounded(stderr, 8192, &stderr_overflow, |_| true));
        let deadline = Instant::now() + Duration::from_secs(self.config.timeout_seconds);
        let mut exit_status = None;
        let ending = loop {
            if overflow.load(Ordering::Relaxed) {
                terminate(&mut child);
                break Ending::Overflowed;
            }
            if exit_status.is_none() {
                exit_status = child.try_wait().context("failed waiting for Himalaya")?;
            }
            if let Some(status) = exit_status {
                if output_thread.is_finished() && stderr_thread.is_finished() {
                    break Ending::Exited(status);
                }
            }
            if Instant::now() >= deadline {
                terminate(&mut child);
                break Ending::TimedOut;
            }
            thread::sleep(Duration::from_millis(10));
        };
        let mut output = Vec::new();
        let ending = match ending {
            Ending::Exited(status) => {
                // Both readers have finished, so these joins cannot block.
                output_thread
                    .join()
                    .map_err(|_| anyhow!("Himalaya stdout reader failed"))??;
                let _ = stderr_thread.join();
                for chunk in chunks.try_iter() {
                    output.extend(chunk);
                }
                if overflow.load(Ordering::Relaxed) {
                    Ending::Overflowed
                } else {
                    Ending::Exited(status)
                }
            }
            killed => {
                // The process group is dead, so the reader normally sees EOF at
                // once. A helper that escaped the group may still hold the
                // pipe; never wait on it for more than a second.
                let grace = Instant::now() + Duration::from_secs(1);
                while let Ok(chunk) =
                    chunks.recv_timeout(grace.saturating_duration_since(Instant::now()))
                {
                    output.extend(chunk);
                }
                killed
            }
        };
        Ok(Captured {
            stdout: output,
            ending,
        })
    }
}

impl MailEngine for Himalaya {
    fn version(&self) -> Result<String> {
        Himalaya::version(self)
    }
    fn binding_identity(&self) -> Result<Value> {
        source_binding(&self.config)
    }
    fn snapshot(&self, folder: &str) -> Result<MailboxSnapshot> {
        Himalaya::snapshot(self, folder)
    }
    fn discover(&self, folder: &str, after: u64, through: u64) -> Result<Vec<SourceEnvelope>> {
        Himalaya::discover(self, folder, after, through)
    }
    fn fetch_raw(&self, folder: &str, uid: u64) -> Result<Vec<u8>> {
        self.fetch(folder, uid)
    }

    fn capabilities(&self) -> Result<EngineCapabilities> {
        if let Some(caps) = self.caps.get() {
            return Ok(caps.clone());
        }
        let captured = self.raw_text("a1 CAPABILITY\r\na2 NAMESPACE\r\n")?;
        let response = raw::parse(&captured.stdout);
        if !captured.finished()
            || response.completion("a1") != Some(raw::Completion::Ok)
            || response.completion("a2").is_none()
        {
            bail!("IMAP CAPABILITY and NAMESPACE did not complete");
        }
        let has = |name: &str| response.capabilities.iter().any(|c| c == name);
        let (personal_prefix, delimiter) = response.personal_namespace.clone().unwrap_or_default();
        let caps = EngineCapabilities {
            move_supported: has("MOVE"),
            uidplus: has("UIDPLUS"),
            special_use: has("SPECIAL-USE"),
            delimiter,
            personal_prefix,
        };
        let _ = self.caps.set(caps.clone());
        Ok(caps)
    }

    fn list_folders(&self) -> Result<Vec<FolderInfo>> {
        let all = folder_rows(&self.run(&["imap", "list", "--all"], true)?)?;
        let subscribed: HashSet<String> = folder_rows(&self.run(&["imap", "list"], true)?)?
            .into_iter()
            .map(|row| row.name)
            .collect();
        let listed = if self.capabilities()?.special_use {
            Some(self.listed_roles()?)
        } else {
            None
        };
        Ok(all
            .into_iter()
            .map(|row| {
                // Roles are known only where SPECIAL-USE is advertised and the
                // folder was listed with RETURN (SPECIAL-USE) or carries a role.
                let roles = listed.as_ref().and_then(|listed| {
                    let from_json = roles_of(&row.attributes);
                    match listed.get(&row.name) {
                        Some(roles) => Some(merge_roles(roles, &from_json)),
                        None if from_json.is_empty() => None,
                        None => Some(from_json),
                    }
                });
                FolderInfo {
                    subscribed: subscribed.contains(&row.name),
                    name: row.name,
                    attributes: row.attributes,
                    roles,
                }
            })
            .collect())
    }

    fn create_folder(&self, native: &str) -> Result<()> {
        raw::quote_mailbox(native)?;
        self.check_mailbox(native)?;
        self.run(&["imap", "create", native], false)?;
        Ok(())
    }

    fn subscribe_folder(&self, native: &str) -> Result<()> {
        raw::quote_mailbox(native)?;
        self.check_mailbox(native)?;
        self.run(&["imap", "subscribe", native], false)?;
        Ok(())
    }

    fn envelopes(&self, folder: &str, uids: &[u64]) -> Result<Vec<SourceEnvelope>> {
        self.check_mailbox(folder)?;
        if uids.contains(&0) {
            bail!("UID must be positive");
        }
        let mut envelopes = Vec::new();
        for chunk in uids.chunks(MAX_UIDS_PER_FETCH as usize) {
            envelopes.extend(self.fetch_envelopes(folder, &raw::uid_set(chunk))?);
        }
        envelopes.sort_by_key(|e| e.uid);
        envelopes.dedup_by_key(|e| e.uid);
        Ok(envelopes)
    }

    fn move_messages(&self, folder: &str, uids: &[u64], target: &str) -> Result<WriteOutcome> {
        self.check_mailbox(folder)?;
        self.check_mailbox(target)?;
        check_write_uids(uids)?;
        let text = format!(
            "a1 SELECT {}\r\na2 UID MOVE {} {}\r\n",
            raw::quote_mailbox(folder)?,
            raw::uid_set(uids),
            raw::quote_mailbox(target)?
        );
        write_outcome(&self.raw_text(&text)?)
    }

    fn add_flagged(&self, folder: &str, uids: &[u64]) -> Result<WriteOutcome> {
        self.check_mailbox(folder)?;
        check_write_uids(uids)?;
        let text = format!(
            "a1 SELECT {}\r\na2 UID STORE {} +FLAGS.SILENT (\\Flagged)\r\n",
            raw::quote_mailbox(folder)?,
            raw::uid_set(uids)
        );
        write_outcome(&self.raw_text(&text)?)
    }

    fn set_watch_scope(&self, folders: &[String]) {
        *self.scope.borrow_mut() = folders.iter().cloned().collect();
    }

    fn alias_conflicts(&self, folders: &[String]) -> Result<Vec<String>> {
        let toml = String::from_utf8(self.current_config()?)
            .map_err(|_| err(2, "invalid Himalaya TOML configuration"))?;
        let parsed: toml::Value =
            toml::from_str(&toml).map_err(|_| err(2, "invalid Himalaya TOML configuration"))?;
        let Some(aliases) = parsed
            .get("accounts")
            .and_then(|v| v.get(&self.config.account))
            .and_then(|v| v.get("mailbox"))
            .and_then(|v| v.get("alias"))
        else {
            return Ok(Vec::new());
        };
        let aliases = aliases
            .as_table()
            .ok_or_else(|| err(2, "invalid Himalaya mailbox alias table"))?
            .iter()
            .map(|(key, native)| {
                native
                    .as_str()
                    .map(|native| (key.as_str(), native))
                    .ok_or_else(|| err(2, "invalid Himalaya mailbox alias table"))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(folders
            .iter()
            .filter(|folder| {
                aliases
                    .iter()
                    .any(|(key, native)| key.eq_ignore_ascii_case(folder) && native != folder)
            })
            .cloned()
            .collect())
    }
}

/// Maps one SELECT-plus-command session. `Err` means the SELECT result was not
/// captured (or SELECT succeeded without UIDVALIDITY), so the session's effect
/// is unknown. A captured SELECT failure is `selected: false` even if the
/// process was then killed.
fn write_outcome(captured: &Captured) -> Result<WriteOutcome> {
    let r = raw::parse(&captured.stdout);
    let Some(select) = r.completion("a1") else {
        bail!("mail engine write produced no usable response");
    };
    let selected = select == raw::Completion::Ok;
    if selected && r.uidvalidity.is_none() {
        bail!("SELECT response lacks UIDVALIDITY");
    }
    Ok(WriteOutcome {
        selected,
        session_epoch: r.uidvalidity,
        completed: selected
            && captured.finished()
            && r.completion("a2") == Some(raw::Completion::Ok),
        copyuid: r.copyuid,
    })
}

fn check_write_uids(uids: &[u64]) -> Result<()> {
    if uids.is_empty() || uids.len() > MAX_UIDS_PER_WRITE {
        bail!("a write takes between 1 and {MAX_UIDS_PER_WRITE} UIDs");
    }
    if uids.contains(&0) {
        bail!("UID must be positive");
    }
    Ok(())
}

/// Parses `imap list` JSON: an array of rows or an object with `mailboxes`.
fn folder_rows(output: &[u8]) -> Result<Vec<FolderRow>> {
    let value: Value = serde_json::from_slice(output).context("invalid Himalaya list JSON")?;
    let rows = match &value {
        Value::Array(rows) => rows,
        Value::Object(map) => map
            .get("mailboxes")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("Himalaya list lacks mailboxes"))?,
        _ => bail!("invalid Himalaya list JSON"),
    };
    rows.iter()
        .map(|row| {
            let name = row
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| anyhow!("Himalaya list row lacks a name"))?;
            let attributes = match row.get("attributes") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(values)) => values
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| anyhow!("invalid Himalaya list attributes"))
                    })
                    .collect::<Result<Vec<_>>>()?,
                Some(_) => bail!("invalid Himalaya list attributes"),
            };
            Ok(FolderRow {
                name: name.to_owned(),
                attributes,
            })
        })
        .collect()
}

/// The special-use roles among `attributes`, spelled as in `SPECIAL_USE_ROLES`.
fn roles_of(attributes: &[String]) -> Vec<String> {
    SPECIAL_USE_ROLES
        .iter()
        .filter(|role| attributes.iter().any(|a| a.eq_ignore_ascii_case(role)))
        .map(|role| role.to_string())
        .collect()
}

fn merge_roles(listed: &[String], extra: &[String]) -> Vec<String> {
    let mut roles = listed.to_vec();
    for role in extra {
        if !roles.contains(role) {
            roles.push(role.clone());
        }
    }
    roles
}

fn optional_str(value: Option<&Value>, field: &str) -> Result<Option<String>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => bail!("invalid Himalaya {field}"),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

// Credentials rotate without changing the mailbox namespace. Hash connection
// identity, never retain or expose the account's secret-bearing TOML.
pub fn source_binding(h: &HimalayaConfig) -> Result<Value> {
    let bytes = fs::read_to_string(&h.config)
        .map_err(|_| err(2, "Himalaya configuration cannot be read"))?;
    let parsed: toml::Value =
        toml::from_str(&bytes).map_err(|_| err(2, "invalid Himalaya TOML configuration"))?;
    let account = parsed
        .get("accounts")
        .and_then(|v| v.get(&h.account))
        .ok_or_else(|| err(2, "Himalaya account is missing from its configuration"))?;
    let mut imap = serde_json::to_value(
        account
            .get("imap")
            .ok_or_else(|| err(2, "Himalaya account must configure IMAP"))?,
    )?;
    let server = imap
        .get("server")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| err(2, "Himalaya IMAP server is missing"))?
        .to_string();
    scrub_secrets(&mut imap);
    Ok(json!({"account":h.account,"server":server,"imap_identity":imap}))
}
fn scrub_secrets(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, value) in map.iter_mut() {
                if ["password", "passwd", "token", "secret"]
                    .iter()
                    .any(|word| key.to_ascii_lowercase().contains(word))
                {
                    *value = json!("<credential>");
                } else {
                    scrub_secrets(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                scrub_secrets(value);
            }
        }
        _ => {}
    }
}

fn terminate(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        // The child starts in its own process group, so helpers cannot keep pipes open.
        unsafe {
            kill(-(child.id() as i32), 9);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Reads `input` to EOF, handing each chunk to `sink`. Raises `overflow` and
/// stops once more than `limit` bytes arrive; also stops when `sink` refuses.
fn read_bounded<R: Read>(
    mut input: R,
    limit: usize,
    overflow: &AtomicBool,
    mut sink: impl FnMut(&[u8]) -> bool,
) -> std::io::Result<()> {
    let mut total = 0usize;
    let mut buffer = [0u8; 8192];
    loop {
        let size = input.read(&mut buffer)?;
        if size == 0 {
            return Ok(());
        }
        if size > limit.saturating_sub(total) {
            overflow.store(true, Ordering::Relaxed);
            return Ok(());
        }
        total += size;
        if !sink(&buffer[..size]) {
            return Ok(());
        }
    }
}

fn parse_address(display: &str) -> Address {
    let trimmed = display.trim();
    if let Some((name, rest)) = trimmed.rsplit_once('<') {
        if let Some(email) = rest.strip_suffix('>') {
            return Address {
                email: email.trim().to_owned(),
                name: Some(name.trim().trim_matches('"').to_owned()).filter(|n| !n.is_empty()),
            };
        }
    }
    Address {
        email: trimmed.to_owned(),
        name: None,
    }
}
