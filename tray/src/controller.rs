//! The tray's state between menu clicks and command answers. The event loop
//! in `tray` turns a click into an `Action`, runs the `Work` that `act`
//! returns off the main thread with `run`, and hands each `Done` back to
//! `done`. Nothing here draws, so all of it is tested without a menu bar.
use crate::{
    autostart,
    cli::{self, Details, Finished, Request, Status},
    instances::{Windows, WINDOWS_ENV},
    model::{
        health::Observations,
        menu::{self, Action, Input, Menu, Notice, Verb},
    },
    paths::{self, Problem, Resolved},
    restart::{self, Identity},
};
use chrono::{DateTime, Duration, FixedOffset, Utc};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Command, Stdio},
};

/// How often the tray refreshes on its own.
pub const REFRESH_EVERY: std::time::Duration = std::time::Duration::from_secs(15);
/// How long a failed refresh keeps the last good menu, marked "(stale)".
pub const STALE_FOR: Duration = Duration::minutes(2);
/// How long a notice stays in the menu.
pub const NOTICE_FOR: Duration = Duration::seconds(60);
pub const TRAY_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Something to do off the main thread (or, for `Copy` and `Quit`, by the
/// event loop itself).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Work {
    /// Resolve the paths while unresolved, then `service status`, and
    /// `mailtriage --version` when the CLI file changed.
    Refresh,
    Service {
        verb: Verb,
        account: String,
    },
    Open(PathBuf),
    Window(String),
    /// Read whether the tray starts at login.
    AutostartStatus,
    Autostart(bool),
    Copy(String),
    Quit,
}

/// A finished piece of work.
#[derive(Debug, Clone)]
pub enum Done {
    Refreshed {
        paths: Result<Resolved, Problem>,
        status: Option<Finished>,
        version: Option<(Identity, Option<String>)>,
    },
    Service {
        verb: Verb,
        account: String,
        finished: Finished,
    },
    Opened(Result<(), String>),
    WindowStarted(u32),
    WindowNotice(String),
    WindowFailed(String),
    Autostart(Result<bool, String>),
}

/// The command-line flags, kept to resolve the paths again while unresolved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Flags {
    pub mailtriage: Option<PathBuf>,
    pub config: Option<PathBuf>,
}

/// What a worker needs, cloned off the controller.
#[derive(Debug, Clone)]
pub struct Context {
    pub paths: Result<Resolved, Problem>,
    pub flags: Flags,
    pub env: paths::Env,
    /// The program windows start from: this tray's own path.
    pub tray_exe: PathBuf,
    pub autostart: Option<autostart::Env>,
    /// The CLI file's identity when its version was last read.
    pub version_of: Option<Identity>,
}

pub struct Controller {
    pub paths: Result<Resolved, Problem>,
    pub flags: Flags,
    pub env: paths::Env,
    pub tray_exe: PathBuf,
    pub autostart_env: Option<autostart::Env>,
    pub autostart: bool,
    pub windows: Windows,
    status: Option<(Status, DateTime<Utc>)>,
    failure: Option<(String, Option<Details>, DateTime<Utc>)>,
    observations: Observations,
    busy: BTreeMap<String, Verb>,
    notices: Vec<(Notice, DateTime<Utc>)>,
    cli_version: Option<(Identity, Option<String>)>,
}

impl Controller {
    pub fn new(
        flags: Flags,
        env: paths::Env,
        tray_exe: PathBuf,
        autostart_env: Option<autostart::Env>,
        windows: Windows,
    ) -> Self {
        Self {
            paths: Err(Problem::NotSetUp(None)),
            flags,
            env,
            tray_exe,
            autostart_env,
            autostart: false,
            windows,
            status: None,
            failure: Some(("Checking…".into(), None, DateTime::<Utc>::MIN_UTC)),
            observations: Observations::default(),
            busy: BTreeMap::new(),
            notices: vec![],
            cli_version: None,
        }
    }

    pub fn context(&self) -> Context {
        Context {
            paths: self.paths.clone(),
            flags: self.flags.clone(),
            env: self.env.clone(),
            tray_exe: self.tray_exe.clone(),
            autostart: self.autostart_env.clone(),
            version_of: self.cli_version.as_ref().map(|(id, _)| *id),
        }
    }

    fn notice(&mut self, text: String, details: Option<Details>, now: DateTime<Utc>) {
        self.notices.push((Notice { text, details }, now));
    }

    /// The work a menu click asks for.
    pub fn act(&mut self, action: Action, now: DateTime<Utc>) -> Vec<Work> {
        match action {
            Action::Refresh => vec![Work::Refresh],
            Action::Start(account) | Action::Stop(account) | Action::Install(account)
                if self.busy.contains_key(&account) =>
            {
                vec![]
            }
            Action::Start(account) => self.service(Verb::Start, account),
            Action::Stop(account) => self.service(Verb::Stop, account),
            Action::Install(account) => self.service(Verb::Install, account),
            Action::OpenLog(path) => {
                let listed = self
                    .status
                    .as_ref()
                    .is_some_and(|(s, _)| s.services.iter().any(|s| s.log_paths.contains(&path)));
                if listed {
                    vec![Work::Open(path)]
                } else {
                    self.notice(
                        "That log is not listed any more; refresh and try again.".into(),
                        None,
                        now,
                    );
                    vec![]
                }
            }
            Action::CopyLogCommand(text) | Action::CopyDetails(text) => vec![Work::Copy(text)],
            Action::EditCategories(account) => {
                if self.paths.is_ok() {
                    vec![Work::Window(account)]
                } else {
                    vec![]
                }
            }
            Action::ToggleAutostart => vec![Work::Autostart(!self.autostart)],
            Action::Quit => vec![Work::Quit],
        }
    }

    fn service(&mut self, verb: Verb, account: String) -> Vec<Work> {
        self.busy.insert(account.clone(), verb);
        vec![Work::Service { verb, account }]
    }

    /// Takes an answer; returns follow-up work (a refresh after an action).
    pub fn done(&mut self, done: Done, now: DateTime<Utc>) -> Vec<Work> {
        match done {
            Done::Refreshed {
                paths,
                status,
                version,
            } => {
                if self.paths.is_err() {
                    self.paths = paths;
                }
                if let Some(version) = version {
                    self.cli_version = Some(version);
                }
                match (&self.paths, status) {
                    (Err(problem), _) => {
                        let details = match problem {
                            Problem::Status(f) => Some(f.details.clone()),
                            _ => None,
                        };
                        self.failure = Some((problem.text(), details, now));
                    }
                    (Ok(_), Some(finished)) => match cli::status(&finished) {
                        Ok(status) => {
                            self.observations.observe(&status.services, now);
                            self.status = Some((status, now));
                            self.failure = None;
                        }
                        Err(f) => {
                            let since = match &self.failure {
                                Some((_, _, since)) if self.status.is_some() => *since,
                                _ => now,
                            };
                            self.failure = Some((f.message, Some(f.details), since));
                        }
                    },
                    (Ok(_), None) => {}
                }
                vec![]
            }
            Done::Service {
                verb,
                account,
                finished,
            } => {
                self.busy.remove(&account);
                if let Err(f) = cli::done(&finished) {
                    let doing = match verb {
                        Verb::Start => "start",
                        Verb::Stop => "stop",
                        Verb::Install => "install",
                    };
                    self.notice(
                        format!("Could not {doing} {account}: {}", f.message),
                        Some(f.details),
                        now,
                    );
                }
                vec![Work::Refresh]
            }
            Done::Opened(Err(e)) | Done::WindowFailed(e) => {
                self.notice(e, None, now);
                vec![]
            }
            Done::Opened(Ok(())) => vec![],
            Done::WindowStarted(pid) => {
                self.windows.track(pid);
                vec![]
            }
            Done::WindowNotice(text) => {
                self.notice(text, None, now);
                vec![]
            }
            Done::Autostart(Ok(enabled)) => {
                self.autostart = enabled;
                vec![]
            }
            Done::Autostart(Err(e)) => {
                self.notice(format!("Start at login: {e}"), None, now);
                vec![]
            }
        }
    }

    /// The menu now.
    pub fn menu(&self, now: DateTime<Utc>, local: DateTime<FixedOffset>) -> Menu {
        let fresh = |at: &DateTime<Utc>| now - *at < NOTICE_FOR;
        let notices: Vec<Notice> = self
            .notices
            .iter()
            .filter(|(_, at)| fresh(at))
            .map(|(n, _)| n.clone())
            .collect();
        let (status, stale, failure) = match (&self.status, &self.failure) {
            (Some((status, _)), None) => (Some(status), false, None),
            (Some((status, _)), Some((_, _, since))) if now - *since < STALE_FOR => {
                (Some(status), true, None)
            }
            (_, Some((text, details, _))) => (None, false, Some((text.clone(), details.clone()))),
            (None, None) => (None, false, None),
        };
        let cli_path = self.paths.as_ref().ok().map(|p| p.cli.as_path());
        menu::build(&Input {
            status,
            stale,
            failure,
            observations: &self.observations,
            now,
            local,
            busy: &self.busy,
            notices: &notices,
            autostart: self.autostart,
            tray_version: TRAY_VERSION,
            cli_version: self.cli_version.as_ref().and_then(|(_, v)| v.as_deref()),
            cli_path,
        })
    }

    /// Drops notices that are no longer shown.
    pub fn expire(&mut self, now: DateTime<Utc>) {
        self.notices.retain(|(_, at)| now - *at < NOTICE_FOR);
    }

    /// No command runs for this tray: a restart may happen now.
    pub fn idle(&self) -> bool {
        self.busy.is_empty()
    }
}

fn opener() -> &'static str {
    if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    }
}

/// Runs `work` and passes each answer to `emit` (a window passes two: its
/// PID, and later a line it printed). Blocks; call it off the main thread.
pub fn run(work: Work, ctx: &Context, emit: &dyn Fn(Done)) {
    match work {
        Work::Refresh => {
            let paths = match &ctx.paths {
                Ok(resolved) => Ok(resolved.clone()),
                Err(_) => paths::resolve(
                    ctx.flags.mailtriage.as_deref(),
                    ctx.flags.config.as_deref(),
                    &ctx.env,
                ),
            };
            let (status, version) = match &paths {
                Ok(resolved) => {
                    let cli = resolved.cli();
                    let version = restart::identity(&resolved.cli)
                        .ok()
                        .filter(|id| ctx.version_of != Some(*id))
                        .map(|id| (id, cli::version(&cli.run(&Request::Version))));
                    (Some(cli.run(&Request::Status)), version)
                }
                Err(_) => (None, None),
            };
            emit(Done::Refreshed {
                paths,
                status,
                version,
            });
        }
        Work::Service { verb, account } => {
            let Ok(resolved) = &ctx.paths else {
                return;
            };
            let request = match verb {
                Verb::Start => Request::Start(account.clone()),
                Verb::Stop => Request::Stop(account.clone()),
                Verb::Install => Request::Install(account.clone()),
            };
            emit(Done::Service {
                verb,
                account,
                finished: resolved.cli().run(&request),
            });
        }
        Work::Open(path) => {
            let finished = cli::run(
                &cli::Invocation {
                    program: PathBuf::from(opener()),
                    args: vec![path.display().to_string()],
                },
                std::time::Duration::from_secs(10),
            );
            emit(Done::Opened(match finished.exit_code() {
                Some(0) => Ok(()),
                _ => Err(format!("Could not open {}", path.display())),
            }));
        }
        Work::Window(account) => {
            let Ok(resolved) = &ctx.paths else {
                return;
            };
            let child = Command::new(&ctx.tray_exe)
                .arg("categories")
                .arg("--account")
                .arg(&account)
                .arg("--config")
                .arg(&resolved.config)
                .arg("--mailtriage")
                .arg(&resolved.cli)
                .env_remove(WINDOWS_ENV)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .spawn();
            match child {
                Ok(mut child) => {
                    emit(Done::WindowStarted(child.id()));
                    if let Some(out) = child.stdout.take() {
                        for line in BufReader::new(out).lines().map_while(Result::ok) {
                            if !line.trim().is_empty() {
                                emit(Done::WindowNotice(line));
                            }
                        }
                    }
                }
                Err(e) => emit(Done::WindowFailed(format!(
                    "Could not open the categories window: {e}"
                ))),
            }
        }
        Work::AutostartStatus => {
            if let Some(env) = &ctx.autostart {
                emit(Done::Autostart(
                    autostart::status(env)
                        .map(|e| e.enabled)
                        .map_err(|e| e.message),
                ));
            }
        }
        Work::Autostart(enable) => {
            let Some(env) = &ctx.autostart else {
                emit(Done::Autostart(Err("HOME is not set".into())));
                return;
            };
            let result = match (enable, &ctx.paths) {
                (true, Ok(resolved)) => autostart::enable(env, &resolved.cli, &resolved.config),
                (true, Err(problem)) => {
                    emit(Done::Autostart(Err(problem.text())));
                    return;
                }
                (false, _) => autostart::disable(env),
            };
            emit(Done::Autostart(
                result.map(|e| e.enabled).map_err(|e| e.message),
            ));
        }
        Work::Copy(_) | Work::Quit => {}
    }
}
