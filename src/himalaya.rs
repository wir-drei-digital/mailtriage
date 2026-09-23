//! Narrow, read-only Himalaya v2.1.0 IMAP adapter.
use crate::domain::{Address, HimalayaConfig, MailboxSnapshot, SourceEnvelope};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant};

const MAX_UIDS_PER_FETCH: u64 = 100;

pub struct Himalaya {
    config: HimalayaConfig,
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
        Ok(Self {
            config: config.clone(),
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
            let range = format!("{start}:{end}");
            let output = self.run(
                &["imap", "fetch", "--mailbox", mailbox, "--envelope", &range],
                true,
            )?;
            let fetched: Value =
                serde_json::from_slice(&output).context("invalid Himalaya fetch JSON")?;
            let messages = fetched
                .get("messages")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("Himalaya fetch lacks messages"))?;
            for message in messages {
                let uid = message
                    .get("uid")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| anyhow!("Himalaya fetch lacks UID"))?;
                if uid < start || uid > end {
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
                envelopes.push(SourceEnvelope {
                    uid,
                    subject,
                    from,
                    sent_at,
                });
            }
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

    fn check_mailbox(&self, mailbox: &str) -> Result<()> {
        if !self.config.mailboxes.iter().any(|m| m == mailbox) {
            bail!("mailbox is not configured for this account");
        }
        Ok(())
    }

    fn run(&self, args: &[&str], json: bool) -> Result<Vec<u8>> {
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
        let overflow_reader = Arc::clone(&overflow);
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("missing Himalaya stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("missing Himalaya stderr"))?;
        let output_thread = thread::spawn(move || read_bounded(stdout, limit, overflow_reader));
        let stderr_overflow = Arc::clone(&overflow);
        let stderr_thread = thread::spawn(move || read_bounded(stderr, 8192, stderr_overflow));
        let deadline = Instant::now() + Duration::from_secs(self.config.timeout_seconds);
        let mut exit_status = None;
        let status = loop {
            if overflow.load(Ordering::Relaxed) {
                terminate(&mut child);
                break Err(anyhow!("Himalaya output limit exceeded"));
            }
            if exit_status.is_none() {
                exit_status = child.try_wait().context("failed waiting for Himalaya")?;
            }
            if let Some(status) = exit_status {
                if output_thread.is_finished() && stderr_thread.is_finished() {
                    break Ok(status);
                }
            }
            if Instant::now() >= deadline {
                terminate(&mut child);
                break Err(anyhow!("Himalaya command timed out"));
            }
            thread::sleep(Duration::from_millis(10));
        };
        // On timeout or overflow, return immediately after terminating the
        // process group. A helper that escaped that group must not make a join
        // wait beyond the configured deadline.
        let status = status?;
        let output = output_thread
            .join()
            .map_err(|_| anyhow!("Himalaya stdout reader failed"))??;
        let _ = stderr_thread.join(); // Intentionally do not expose stderr: it may contain credentials or mail.
        if overflow.load(Ordering::Relaxed) {
            bail!("Himalaya output limit exceeded");
        }
        if !status.success() {
            bail!("Himalaya command failed with status {status}");
        }
        Ok(output)
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

fn read_bounded<R: Read>(mut input: R, limit: usize, overflow: Arc<AtomicBool>) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let size = input.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        if size > limit.saturating_sub(output.len()) {
            overflow.store(true, Ordering::Relaxed);
            break;
        }
        output.extend_from_slice(&buffer[..size]);
    }
    Ok(output)
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
