//! Starting local Kev on demand (Kev spec, `ensure_running`) and stopping
//! it. Errors are `ProviderUnavailable`, so classification is skipped, not
//! failed.
use super::{control, installation, Installed, Layout, CHECK, IDLE, START_TIMEOUT, STOP_GRACE};
use crate::{
    domain::ProviderConfig,
    process,
    provider::{kev, ProviderUnavailable},
};
use fs2::FileExt;
use serde_json::{json, Value};
use std::{
    ffi::OsStr,
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub const NOT_INSTALLED: &str = "local Kev is not installed; run mailtriage setup --update";
/// How long `stop` waits for a supervisor to exit.
pub const STOP_WAIT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_secs(1);
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// One managed server: where its files are, its port and checkpoint, and
/// the mailtriage that runs its supervisor.
#[derive(Debug, Clone)]
pub struct Runtime {
    pub layout: Layout,
    pub port: u16,
    pub checkpoint: String,
    pub exe: PathBuf,
    pub start_timeout: Duration,
    pub idle: Duration,
    pub check: Duration,
}

impl Runtime {
    /// A managed `config`'s server: this user's layout, the endpoint's
    /// port, this program.
    pub fn for_config(config: &ProviderConfig) -> Result<Self, ProviderUnavailable> {
        let unavailable = |m: &str| ProviderUnavailable(m.to_owned());
        let checkpoint = config
            .managed
            .as_ref()
            .map(|m| m.checkpoint.clone())
            .ok_or_else(|| unavailable("provider.managed is missing"))?;
        let port = reqwest::Url::parse(&config.endpoint)
            .ok()
            .and_then(|url| url.port())
            .ok_or_else(|| unavailable("provider.endpoint names no port"))?;
        let layout = Layout::for_user()
            .ok_or_else(|| unavailable("local Kev needs HOME to find its files"))?;
        let exe = std::env::current_exe()
            .map_err(|_| unavailable("cannot find this mailtriage program to start Kev"))?;
        Ok(Self {
            layout,
            port,
            checkpoint,
            exe,
            start_timeout: super::test_kev()
                .start_seconds
                .map_or(START_TIMEOUT, Duration::from_secs),
            idle: IDLE,
            check: CHECK,
        })
    }

    fn did_not_start(&self) -> ProviderUnavailable {
        ProviderUnavailable(format!(
            "local Kev did not start; see {}",
            self.layout.log().display()
        ))
    }

    fn port_taken(&self) -> ProviderUnavailable {
        ProviderUnavailable(format!("port {} is used by another program", self.port))
    }
}

/// What answers on a managed server's port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// Kev, serving `kev-latest` from this checkpoint.
    Ready,
    /// Something else.
    Other,
    /// Nothing.
    Nothing,
}

pub fn probe(rt: &Runtime) -> Probe {
    let url = format!("http://127.0.0.1:{}/v1/models", rt.port);
    match kev::list_models(&url, None, PROBE_TIMEOUT) {
        Ok(models)
            if models.iter().any(|m| {
                m.name == kev::DEFAULT_MODEL && m.run.as_deref() == Some(rt.checkpoint.as_str())
            }) =>
        {
            Probe::Ready
        }
        Err(kev::ModelsError::Unreachable) => Probe::Nothing,
        _ => Probe::Other,
    }
}

/// `ensure_running_with` for a managed config.
pub fn ensure_running(config: &ProviderConfig) -> Result<(), ProviderUnavailable> {
    ensure_running_with(&Runtime::for_config(config)?)
}

/// Kev answers on its port, started here if nothing does; records a
/// request. Concurrent callers may both start a supervisor: the lock keeps
/// one, and both wait until it answers.
pub fn ensure_running_with(rt: &Runtime) -> Result<(), ProviderUnavailable> {
    if !rt.layout.root().is_dir() {
        return Err(ProviderUnavailable(NOT_INSTALLED.to_owned()));
    }
    let _startup =
        installation::startup(&rt.layout).map_err(|e| ProviderUnavailable(e.to_string()))?;
    // Read the shared receipt only after excluding mutations.
    let installed = Installed::read(&rt.layout)
        .filter(|i| i.matches(&rt.checkpoint))
        .ok_or_else(|| ProviderUnavailable(NOT_INSTALLED.to_owned()))?;
    shutdown_confirmed(&rt.layout).map_err(|e| ProviderUnavailable(e.to_string()))?;
    match probe(rt) {
        Probe::Ready => {
            shutdown_confirmed(&rt.layout).map_err(|e| ProviderUnavailable(e.to_string()))?;
            rt.layout.touch_last_used();
            return Ok(());
        }
        Probe::Other => return Err(rt.port_taken()),
        Probe::Nothing => {}
    }
    spawn_supervisor(rt, &installed.uv, false).map_err(|_| rt.did_not_start())?;
    let deadline = Instant::now() + rt.start_timeout;
    loop {
        thread::sleep(POLL);
        match probe(rt) {
            Probe::Ready => {
                shutdown_confirmed(&rt.layout).map_err(|e| ProviderUnavailable(e.to_string()))?;
                rt.layout.touch_last_used();
                return Ok(());
            }
            Probe::Other => return Err(rt.port_taken()),
            Probe::Nothing if Instant::now() >= deadline => return Err(rt.did_not_start()),
            Probe::Nothing => {}
        }
    }
}

#[cfg(unix)]
unsafe extern "C" {
    fn setsid() -> i32;
}

/// Starts `mailtriage kev run` detached, in its own session with stdin and
/// output closed. Ending its parent's process group leaves it running;
/// an independent session does not escape systemd cgroups. A thread reaps it, so a long `watch` keeps no zombies.
pub fn spawn_supervisor(rt: &Runtime, uv: &Path, allow_download: bool) -> std::io::Result<()> {
    let _startup = installation::startup(&rt.layout)?;
    spawn(rt, uv, allow_download, None)
}

pub(crate) fn spawn_install_supervisor(
    rt: &Runtime,
    uv: &Path,
    guard: &installation::Guard,
) -> std::io::Result<()> {
    spawn(rt, uv, true, Some(guard))
}

fn spawn(
    rt: &Runtime,
    uv: &Path,
    allow_download: bool,
    guard: Option<&installation::Guard>,
) -> std::io::Result<()> {
    shutdown_confirmed(&rt.layout)?;
    let mut command = Command::new(&rt.exe);
    command
        .args(["kev", "run", "--dir"])
        .arg(rt.layout.root())
        .args([
            "--port",
            &rt.port.to_string(),
            "--checkpoint",
            &rt.checkpoint,
            "--uv",
        ])
        .arg(uv)
        .args([
            "--idle-seconds",
            &rt.idle.as_secs().to_string(),
            "--check-seconds",
            &rt.check.as_secs().to_string(),
            "--stop-grace-seconds",
            &STOP_GRACE.as_secs().to_string(),
        ])
        .env_remove("KEV_API_KEY")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    installation::clear_authorization(&mut command);
    if let Some(guard) = guard {
        guard.authorize(&rt.layout, &mut command)?;
    }
    if allow_download {
        command.arg("--allow-download");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe and touches no memory.
        unsafe {
            command.pre_exec(|| {
                if setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = command.spawn()?;
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn lock_file(layout: &Layout) -> std::io::Result<fs::File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(layout.lock())
}

/// Whether an installer or its supervised writer holds exclusive mutation
/// ownership. Starts no process and creates no file. A busy layout is not
/// ready even if an old receipt exists; server.lock may belong to a writer.
pub fn installation_in_progress(layout: &Layout) -> std::io::Result<bool> {
    installation::in_progress(layout)
}

/// Checks the supervisor lock without hiding filesystem or locking errors.
pub fn try_supervisor_running(layout: &Layout) -> std::io::Result<bool> {
    match fs::metadata(layout.root()) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    }
    let file = lock_file(layout)?;
    match file.try_lock_exclusive() {
        Ok(()) => {
            FileExt::unlock(&file)?;
            Ok(false)
        }
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(true),
        Err(e) => Err(e),
    }
}

/// Conservative convenience check: an unreadable lock is never proof of shutdown.
pub fn supervisor_running(layout: &Layout) -> bool {
    try_supervisor_running(layout).unwrap_or(true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopOutcome {
    AlreadyStopped,
    Stopped,
    TimedOut,
}

/// Whether no supervisor holds the lock and the last owned group has
/// confirmed termination. An abandoned/corrupt journal returns an error.
/// This inspection starts no process and sends no stop request.
pub fn shutdown_confirmed(layout: &Layout) -> std::io::Result<bool> {
    match fs::metadata(layout.root()) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(e) => return Err(e),
    }
    let file = lock_file(layout)?;
    match file.try_lock_exclusive() {
        Ok(()) => {
            control::confirm_terminated(layout, &file)?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(false),
        Err(e) => Err(e),
    }
}

/// Read-only ownership inspection for doctor. A missing lock alongside
/// ownership evidence is uncertain; doctor never creates a control file.
enum ShutdownInspectionError {
    LockAccess(std::io::Error),
    Other(std::io::Error),
}

fn inspect_shutdown(layout: &Layout) -> Result<bool, ShutdownInspectionError> {
    match fs::metadata(layout.root()) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(e) => return Err(ShutdownInspectionError::Other(e)),
    }
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .open(layout.lock())
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return if layout.ownership().exists() {
                Err(ShutdownInspectionError::Other(control::unconfirmed(layout)))
            } else {
                Ok(true)
            };
        }
        Err(e) => return Err(ShutdownInspectionError::LockAccess(e)),
    };
    match file.try_lock_exclusive() {
        Ok(()) => {
            control::confirm_terminated(layout, &file).map_err(ShutdownInspectionError::Other)?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(false),
        Err(e) => Err(ShutdownInspectionError::LockAccess(e)),
    }
}

fn not_ready(out: &mut Value, error: impl Into<String>, fix: impl Into<String>) {
    out["ready"] = json!(false);
    out["error"] = json!(error.into());
    out["fix"] = json!(fix.into());
}

/// Read-only Kev diagnostics for doctor; never starts or stops a process.
pub fn status(config: &ProviderConfig, key: Option<&str>) -> Value {
    let managed = config.managed.is_some();
    let mut out =
        json!({"mode": if managed { "managed" } else { "server" }, "endpoint": config.endpoint});
    if managed {
        out["installed"] = json!(false);
        out["running"] = json!(false);
        let rt = match Runtime::for_config(config) {
            Ok(rt) => rt,
            Err(e) => {
                not_ready(&mut out, e.0, "run `mailtriage setup --update`");
                return out;
            }
        };
        out["dir"] = json!(rt.layout.root());
        let repair_ownership = format!("stop all callers, then verify no live mailtriage kev run/mutate or git/uv/brew descendants remain; preserve {} as a backup before running `mailtriage setup --update --yes`", rt.layout.root().display());
        match installation_in_progress(&rt.layout) {
            Ok(true) => {
                not_ready(
                    &mut out,
                    format!(
                        "local Kev installation is in progress in {}",
                        rt.layout.root().display()
                    ),
                    "wait for setup to finish, then run `mailtriage setup --update` again",
                );
                return out;
            }
            Err(e) => {
                not_ready(
                    &mut out,
                    format!("local Kev installation state is uncertain: {e}"),
                    repair_ownership,
                );
                return out;
            }
            Ok(false) => {}
        }
        let stopped = match inspect_shutdown(&rt.layout) {
            Ok(stopped) => stopped,
            Err(ShutdownInspectionError::LockAccess(e))
                if e.kind() == std::io::ErrorKind::PermissionDenied =>
            {
                not_ready(
                    &mut out,
                    format!("cannot read and write {}: {e}", rt.layout.lock().display()),
                    format!(
                        "correct ownership and permissions for {}, then rerun `mailtriage doctor`",
                        rt.layout.lock().display()
                    ),
                );
                return out;
            }
            Err(ShutdownInspectionError::LockAccess(e) | ShutdownInspectionError::Other(e)) => {
                not_ready(&mut out, e.to_string(), repair_ownership);
                return out;
            }
        };
        let installed = match Installed::read(&rt.layout) {
            Some(i) if i.matches(&rt.checkpoint) => i,
            other => {
                if let Some(i) = other {
                    out["installed_checkpoint"] = json!(i.checkpoint);
                }
                not_ready(&mut out, NOT_INSTALLED, "run `mailtriage setup --update`");
                return out;
            }
        };
        let source = rt.layout.src();
        let tag_ok = process::find_on_path("git")
            .and_then(|git| {
                let args = [
                    OsStr::new("-C"),
                    source.as_os_str(),
                    OsStr::new("describe"),
                    OsStr::new("--tags"),
                    OsStr::new("--exact-match"),
                ];
                process::run_bounded(&git, &args, Duration::from_secs(10), 4096)
                    .ok()?
                    .success()
            })
            .is_some_and(|tag| String::from_utf8_lossy(&tag).trim() == super::TAG);
        if !source.is_dir()
            || !source.join(".venv").is_dir()
            || !rt.layout.hf().is_dir()
            || !process::is_executable(&installed.uv)
            || !tag_ok
        {
            not_ready(
                &mut out,
                "local Kev installation is incomplete or its source tag changed",
                "run `mailtriage setup --update`",
            );
            return out;
        }
        out["installed"] = json!(true);
        out["tag"] = json!(installed.tag);
        out["checkpoint"] = json!(installed.checkpoint);
        match probe(&rt) {
            Probe::Ready if !stopped => {
                out["running"] = json!(true);
                out["ready"] = json!(true);
            }
            Probe::Nothing if stopped => out["ready"] = json!(true),
            Probe::Nothing => not_ready(
                &mut out,
                "local Kev owns its lock but did not answer",
                "check the live Kev process and its log, then run `mailtriage setup --update`",
            ),
            _ => not_ready(
                &mut out,
                rt.port_taken().0,
                format!("stop the program on port {}", rt.port),
            ),
        }
        return out;
    }
    let url = kev::models_url(&config.endpoint).unwrap_or_default();
    match kev::list_models(&url, key, Duration::from_secs(10)) {
        Ok(models) if models.iter().any(|m| m.name == config.model) => {
            out["reachable"] = json!(true);
            out["ready"] = json!(true);
        }
        Ok(_) => {
            out["reachable"] = json!(true);
            not_ready(
                &mut out,
                format!("the Kev server does not serve {}", config.model),
                "check provider.model, or the model your Kev server runs",
            );
        }
        Err(e) => {
            out["reachable"] = json!(e != kev::ModelsError::Unreachable);
            not_ready(
                &mut out,
                format!("{e} at {url}"),
                "start your Kev server, or check provider.endpoint and its key",
            );
        }
    }
    out
}

/// Requests the current lock holder to stop through its control directory.
/// Success requires a free lock and durable proof of owned-group cleanup;
/// supervisor disappearance alone is an error. No persisted PID is signalled.
pub fn stop(layout: &Layout, wait: Duration) -> std::io::Result<StopOutcome> {
    match fs::metadata(layout.root()) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(StopOutcome::AlreadyStopped)
        }
        Err(e) => return Err(e),
    }
    let file = lock_file(layout)?;
    match file.try_lock_exclusive() {
        Ok(()) => {
            control::confirm_terminated(layout, &file)?;
            control::clear_request(layout)?;
            return Ok(StopOutcome::AlreadyStopped);
        }
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
        Err(e) => return Err(e),
    }
    control::request_stop(layout)?;
    let deadline = Instant::now() + wait;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => {
                control::confirm_terminated(layout, &file)?;
                control::clear_request(layout)?;
                return Ok(StopOutcome::Stopped);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e),
        }
        if Instant::now() >= deadline {
            return Ok(StopOutcome::TimedOut);
        }
        thread::sleep(Duration::from_millis(100));
    }
}
