//! Runs one `mailtriage … --json` command with a time limit and reads its
//! JSON into typed results. Every action of the tray and the window is one
//! of these commands, so an agent can do the same with the CLI.
use chrono::{DateTime, FixedOffset};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// Limit for `service status`.
pub const STATUS_TIMEOUT: Duration = Duration::from_secs(30);
/// Limit for every other command.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_STDOUT: u64 = 4 * 1024 * 1024;
const MAX_STDERR: u64 = 64 * 1024;

/// One command the tray or the window runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Version,
    Status,
    Start(String),
    Stop(String),
    Install(String),
    Export(String),
    Validate {
        account: String,
        file: PathBuf,
    },
    Apply {
        account: String,
        file: PathBuf,
        digest: String,
    },
    Refile {
        account: String,
        folder: Option<String>,
        apply: bool,
    },
}

impl Request {
    pub fn timeout(&self) -> Duration {
        match self {
            Request::Status => STATUS_TIMEOUT,
            _ => COMMAND_TIMEOUT,
        }
    }
}

/// The exact program and arguments of a command, kept for "Show details".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: PathBuf,
    pub args: Vec<String>,
}

impl Invocation {
    /// The command as a person would type it.
    pub fn line(&self) -> String {
        std::iter::once(self.program.display().to_string())
            .chain(self.args.iter().cloned())
            .map(|word| quote(&word))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// `word`, single-quoted for a POSIX shell when it holds anything but
/// letters, digits and `-_./=:@+,`.
pub fn quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:@+,".contains(c))
    {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// The `mailtriage` program and the config every command names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cli {
    pub program: PathBuf,
    /// Absolute and canonical; `None` only for the first `service status`
    /// that learns which config the CLI resolves.
    pub config: Option<PathBuf>,
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

impl Cli {
    /// The argument list of `request`: the documented command, then
    /// `--config <absolute path>`.
    pub fn invocation(&self, request: &Request) -> Invocation {
        let words = |list: &[&str]| list.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        let mut args = match request {
            Request::Version => {
                return Invocation {
                    program: self.program.clone(),
                    args: words(&["--version"]),
                }
            }
            Request::Status => words(&["service", "status", "--json"]),
            Request::Start(a) => words(&["service", "start", "--account", a, "--json"]),
            Request::Stop(a) => words(&["service", "stop", "--account", a, "--json"]),
            Request::Install(a) => words(&["service", "install", "--account", a, "--json"]),
            Request::Export(a) => words(&["categories", "export", "--account", a, "--json"]),
            Request::Validate { account, file } => words(&[
                "categories",
                "validate",
                "--account",
                account,
                "--file",
                &path_arg(file),
                "--json",
            ]),
            Request::Apply {
                account,
                file,
                digest,
            } => words(&[
                "categories",
                "apply",
                "--account",
                account,
                "--file",
                &path_arg(file),
                "--expect-digest",
                digest,
                "--json",
            ]),
            Request::Refile {
                account,
                folder,
                apply,
            } => {
                let mut args = words(&["filing", "refile", "--account", account]);
                if let Some(folder) = folder {
                    args.extend(words(&["--folder", folder]));
                }
                if *apply {
                    args.push("--apply".into());
                }
                args.push("--json".into());
                args
            }
        };
        if let Some(config) = &self.config {
            args.extend(["--config".to_owned(), path_arg(config)]);
        }
        Invocation {
            program: self.program.clone(),
            args,
        }
    }

    /// Runs `request` and waits for it; call it off the UI thread.
    pub fn run(&self, request: &Request) -> Finished {
        run(&self.invocation(request), request.timeout())
    }
}

/// How a command ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ending {
    Exited(i32),
    Signalled,
    TimedOut(Duration),
    CouldNotStart(String),
}

/// A command that ended, with what it printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    pub invocation: Invocation,
    pub ending: Ending,
    pub stdout: String,
    pub stderr: String,
}

/// What "Show details" shows and copies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Details {
    pub command: String,
    pub exit_code: Option<i32>,
    pub output: String,
}

impl Details {
    pub fn text(&self) -> String {
        let code = self
            .exit_code
            .map_or_else(|| "none".to_owned(), |c| c.to_string());
        format!(
            "$ {}\nexit code: {code}\n{}",
            self.command,
            self.output.trim_end()
        )
    }
}

impl Finished {
    pub fn exit_code(&self) -> Option<i32> {
        match self.ending {
            Ending::Exited(code) => Some(code),
            _ => None,
        }
    }

    pub fn details(&self) -> Details {
        let mut output = self.stdout.clone();
        if !self.stderr.is_empty() {
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(&self.stderr);
        }
        Details {
            command: self.invocation.line(),
            exit_code: self.exit_code(),
            output,
        }
    }
}

fn read_limited(mut input: impl Read + Send + 'static, limit: u64) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut data = Vec::new();
        let _ = (&mut input).take(limit).read_to_end(&mut data);
        // Drain the rest so the child never blocks on a full pipe.
        let _ = std::io::copy(&mut input, &mut std::io::sink());
        String::from_utf8_lossy(&data).into_owned()
    })
}

#[cfg(unix)]
fn kill_group(pid: u32) {
    // SAFETY: kill has no memory preconditions; the child leads its own group.
    unsafe {
        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_group(_pid: u32) {}

/// Runs `invocation` with stdin closed, killing it (and its process group)
/// at `timeout`. Only this child's PID is waited for.
pub fn run(invocation: &Invocation, timeout: Duration) -> Finished {
    let finished = |ending, stdout: String, stderr: String| Finished {
        invocation: invocation.clone(),
        ending,
        stdout,
        stderr,
    };
    let mut command = Command::new(&invocation.program);
    command
        .args(&invocation.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            return finished(
                Ending::CouldNotStart(e.to_string()),
                String::new(),
                String::new(),
            )
        }
    };
    let stdout = read_limited(child.stdout.take().expect("piped"), MAX_STDOUT);
    let stderr = read_limited(child.stderr.take().expect("piped"), MAX_STDERR);
    let deadline = Instant::now() + timeout;
    let ending = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code().map_or(Ending::Signalled, Ending::Exited),
            Ok(None) if Instant::now() >= deadline => {
                kill_group(child.id());
                let _ = child.kill();
                let _ = child.wait();
                break Ending::TimedOut(timeout);
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(e) => {
                kill_group(child.id());
                let _ = child.wait();
                break Ending::CouldNotStart(e.to_string());
            }
        }
    };
    finished(
        ending,
        stdout.join().unwrap_or_default(),
        stderr.join().unwrap_or_default(),
    )
}

/// A command that did not give the tray what it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// Plain words for the person.
    pub message: String,
    /// The JSON error's machine-readable `reason`.
    pub reason: Option<String>,
    /// The JSON error's `code`.
    pub code: Option<i32>,
    pub details: Details,
}

impl Failure {
    pub fn new(message: impl Into<String>, details: Details) -> Self {
        Self {
            message: message.into(),
            reason: None,
            code: None,
            details,
        }
    }

    pub fn has_reason(&self, reason: &str) -> bool {
        self.reason.as_deref() == Some(reason)
    }
}

/// The text shown when the CLI lacks a field or a command the tray needs.
pub const OUTDATED: &str = "mailtriage is older than the tray; run `mailtriage update`.";

/// How mailtriage's argument parser (clap) starts the message of a usage
/// error, which `--json` prints as an error object with code and exit 2.
const PARSER_ERROR: &str = "error: ";

/// The command's JSON result, or its failure: a JSON error object, no
/// JSON at all, a timeout or a program that could not start.
pub fn outcome(finished: &Finished) -> Result<Value, Failure> {
    let fail = |message: String| Failure::new(message, finished.details());
    match &finished.ending {
        Ending::CouldNotStart(e) => Err(fail(format!("mailtriage could not start: {e}"))),
        Ending::TimedOut(_) => Err(fail("mailtriage took too long to answer".into())),
        Ending::Signalled => Err(fail("mailtriage stopped unexpectedly".into())),
        Ending::Exited(_) => {
            let value: Value = serde_json::from_str(finished.stdout.trim())
                .map_err(|_| fail("mailtriage gave an answer the tray cannot read".into()))?;
            if let Some(error) = value.get("error") {
                let message = error["message"].as_str().unwrap_or("mailtriage failed");
                let code = error["code"].as_i64().map(|c| c as i32);
                // The parser rejected the tray's arguments: a mailtriage
                // older than the tray. Its text goes to the details.
                if finished.exit_code() == Some(2)
                    && code == Some(2)
                    && message.starts_with(PARSER_ERROR)
                {
                    let mut details = finished.details();
                    details.output = format!("{message}\n{}", details.output);
                    return Err(Failure {
                        message: OUTDATED.to_owned(),
                        reason: None,
                        code,
                        details,
                    });
                }
                return Err(Failure {
                    message: message.to_owned(),
                    reason: error["reason"].as_str().map(str::to_owned),
                    code,
                    details: finished.details(),
                });
            }
            Ok(value)
        }
    }
}

/// `value` as `T` after checking that every JSON pointer in `required`
/// exists; a missing one means the CLI is older than the tray.
fn typed<T: DeserializeOwned>(
    finished: &Finished,
    value: Value,
    required: &[&str],
) -> Result<T, Failure> {
    if let Some(missing) = required.iter().find(|p| value.pointer(p).is_none()) {
        let mut details = finished.details();
        details.output = format!("missing field {missing}\n{}", details.output);
        return Err(Failure::new(OUTDATED, details));
    }
    serde_json::from_value(value).map_err(|_| {
        Failure::new(
            "mailtriage gave an answer the tray cannot read",
            finished.details(),
        )
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FilingMode {
    Off,
    DryRun,
    Live,
}

/// `service status`'s `last_pass`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LastPass {
    pub finished_at: DateTime<FixedOffset>,
    pub partial: bool,
    pub exit_code: i64,
    pub mode: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
}

/// The part of `service status`'s `update` block the tray reads.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct UpdateInfo {
    #[serde(default)]
    pub executable: Option<PathBuf>,
    #[serde(default)]
    pub installed: Option<String>,
}

/// One account's object of `service status`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ServiceInfo {
    pub account: String,
    pub manager: String,
    pub installed: bool,
    pub running: bool,
    pub pid: Option<u32>,
    #[serde(default)]
    pub unit_path: Option<PathBuf>,
    pub log_paths: Vec<PathBuf>,
    pub last_pass: Option<LastPass>,
    #[serde(default)]
    pub service_config: Option<PathBuf>,
    #[serde(default)]
    pub file_config: Option<PathBuf>,
    #[serde(default)]
    pub needs_daemon_reload: Option<bool>,
    pub config_matches: Option<bool>,
    pub enabled: Option<bool>,
    pub enablement: String,
    pub interval_seconds: Option<u64>,
    pub filing_mode: FilingMode,
    pub identity: String,
    #[serde(default)]
    pub update: Option<UpdateInfo>,
}

/// `service status` without `--account`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Status {
    pub config: PathBuf,
    pub services: Vec<ServiceInfo>,
}

const SERVICE_FIELDS: [&str; 13] = [
    "account",
    "manager",
    "installed",
    "running",
    "pid",
    "log_paths",
    "last_pass",
    "config_matches",
    "enabled",
    "enablement",
    "interval_seconds",
    "filing_mode",
    "identity",
];

pub fn status(finished: &Finished) -> Result<Status, Failure> {
    let value = outcome(finished)?;
    let count = value["services"].as_array().map_or(0, Vec::len);
    let mut required = vec!["/config".to_owned(), "/services".to_owned()];
    for i in 0..count {
        required.extend(SERVICE_FIELDS.iter().map(|f| format!("/services/{i}/{f}")));
    }
    let required: Vec<&str> = required.iter().map(String::as_str).collect();
    typed(finished, value, &required)
}

/// A category as `categories export` prints it and a draft file holds it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Category {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub examples: Vec<String>,
    #[serde(default)]
    pub catch_all: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

impl Category {
    pub fn effective_folder(&self) -> &str {
        self.folder.as_deref().unwrap_or(&self.name)
    }
}

/// `categories export`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Categories {
    pub account: String,
    pub categories: Vec<Category>,
    pub digest: String,
}

pub fn categories(finished: &Finished) -> Result<Categories, Failure> {
    typed(
        finished,
        outcome(finished)?,
        &["/account", "/categories", "/digest"],
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Removed {
    pub id: String,
    pub folder: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Change {
    pub id: String,
    pub from: String,
    pub to: String,
}

/// `categories validate --account`'s `changes`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Changes {
    pub added: Vec<String>,
    pub removed: Vec<Removed>,
    pub renamed: Vec<Change>,
    pub folders_changed: Vec<Change>,
    pub edited: Vec<String>,
    pub reclassifies: bool,
}

/// `categories validate --account`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Validation {
    pub digest: String,
    pub changes: Changes,
}

pub fn validation(finished: &Finished) -> Result<Validation, Failure> {
    typed(finished, outcome(finished)?, &["/digest", "/changes"])
}

/// Any successful result whose content the caller does not need.
pub fn done(finished: &Finished) -> Result<(), Failure> {
    outcome(finished).map(|_| ())
}

/// One home folder of `filing refile`'s preview.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RefileFolder {
    pub folder: Option<String>,
    pub native: String,
    pub retired: bool,
    pub candidates: u64,
    pub waiting: u64,
}

impl RefileFolder {
    /// The name a person knows: the configured one, else the server's.
    pub fn name(&self) -> &str {
        self.folder.as_deref().unwrap_or(&self.native)
    }
}

/// `filing refile --json` without `--apply`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RefilePreview {
    pub total: u64,
    pub folders: Vec<RefileFolder>,
    pub waiting: u64,
    #[serde(default)]
    pub skipped: BTreeMap<String, u64>,
}

pub fn refile_preview(finished: &Finished) -> Result<RefilePreview, Failure> {
    typed(
        finished,
        outcome(finished)?,
        &["/total", "/folders", "/waiting"],
    )
}

/// `filing refile --apply --json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct RefileMarked {
    pub marked: u64,
    #[serde(default)]
    pub waiting_marked: u64,
}

pub fn refile_marked(finished: &Finished) -> Result<RefileMarked, Failure> {
    typed(finished, outcome(finished)?, &["/marked"])
}

/// `mailtriage --version`'s version.
pub fn version(finished: &Finished) -> Option<String> {
    if finished.exit_code() != Some(0) {
        return None;
    }
    finished
        .stdout
        .trim()
        .strip_prefix("mailtriage ")
        .map(str::to_owned)
}
