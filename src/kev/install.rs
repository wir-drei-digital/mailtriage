//! Setup's managed install (Kev spec, "Managed install"). A step that is
//! already done is reused, so running setup again after a failure resumes.
//! It writes only below `<data dir>/mailtriage/kev`; `installed.json`
//! comes last, and setup writes the config after it.
use super::{
    installation,
    runtime::{self, Probe, Runtime, StopOutcome, STOP_WAIT},
    test_kev, Installed, Layout, REPO, TAG,
};
use crate::{
    domain::{AccountConfig, Address, NormalizedMessage, ProviderConfig},
    process,
    prompt::Prompter,
    provider::{self, DecisionRequest},
    service::err,
};
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    ffi::OsStr,
    fs,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// Every install error starts with this step.
pub const STEP: &str = "step 5 (Kev)";
const MIN_FREE_GB: u64 = 15;
const WARN_MEMORY_GB: u64 = 32;
/// Initial grace before diagnosing a supervisor that has not taken its lock.
const START_GRACE: Duration = Duration::from_secs(5);
const UV_INSTALLER: &str = "curl -LsSf https://astral.sh/uv/install.sh | sh";

/// Installs local Kev for `provider` (a managed `kev` block) and checks it
/// with one classification of a sample message with `account`'s
/// questions. `yes` agrees to the download without prompts.
pub fn install(
    p: &mut Prompter,
    provider: &ProviderConfig,
    account: &AccountConfig,
    yes: bool,
) -> Result<Value> {
    if !super::apple_silicon() {
        return Err(err(2, "step 5 (classifier): local Kev needs an Apple Silicon Mac; run your own Kev server and pass --kev-url"));
    }
    let mut rt = Runtime::for_config(provider).map_err(|e| {
        err(
            2,
            format!("{STEP}: {e}; check the provider block, then run `mailtriage setup` again"),
        )
    })?;
    // Setup checks stop requests frequently; the runtime keeps the configured idle.
    rt.check = Duration::from_secs(1);
    let layout = &rt.layout;
    let guard = installation::Guard::acquire(layout).map_err(|e| lifecycle_error(layout, e))?;
    runtime::shutdown_confirmed(layout).map_err(|e| lifecycle_error(layout, e))?;
    let dir = layout.root().display().to_string();
    let report = |action: &str| {
        json!({"mode": "managed", "dir": layout.root(), "action": action,
               "tag": TAG, "checkpoint": rt.checkpoint})
    };
    let installed = Installed::read(layout);
    let git = process::find_on_path("git");
    let current = installed.as_ref().is_some_and(|i| {
        i.matches(&rt.checkpoint)
            && layout.src().is_dir()
            && layout.src().join(".venv").is_dir()
            && layout.hf().is_dir()
            && process::is_executable(&i.uv)
            && git
                .as_ref()
                .is_some_and(|git| tag_of(git, &layout.src()).as_deref() == Some(TAG))
    });
    reject_unrelated(&rt, !current)?;
    if current {
        p.say(&format!("Local Kev is installed in {dir}."));
        return Ok(report("current"));
    }
    // 1. Room and memory.
    if let Some(free) = free_gb(layout.root()).filter(|&gb| gb < MIN_FREE_GB) {
        return Err(err(
            3,
            format!("{STEP}: {dir} has {free} GB free and local Kev needs {MIN_FREE_GB} GB; free some space, then run `mailtriage setup` again"),
        ));
    }
    if let Some(memory) = memory_gb().filter(|&gb| gb < WARN_MEMORY_GB) {
        p.say(&format!("This Mac has {memory} GB of memory. Local Kev uses about 12 GB while it classifies, so other programs may slow down."));
    }
    // 2. Agree to the download.
    let question = format!("Local Kev downloads about 9 GB to {dir} and uses about 12 GB of memory while it classifies. Continue?");
    let agreed = if p.enabled() {
        p.confirm(&question, true)?
    } else if yes {
        true
    } else {
        return Err(err(
            2,
            format!("{STEP}: local Kev downloads about 9 GB; pass --yes to agree, or choose another --provider"),
        ));
    };
    if !agreed {
        return Err(err(
            2,
            format!("{STEP}: local Kev was not installed and nothing was written; run `mailtriage setup` again to choose another classifier"),
        ));
    }
    // 3. Tools.
    let git = git.ok_or_else(|| {
        err(
            3,
            format!("{STEP}: git is not on PATH; install the Xcode command line tools with `xcode-select --install`, then run `mailtriage setup` again"),
        )
    })?;
    // Deliberately adopt the shared layout's owned supervisor, and confirm
    // its whole group stopped before changing code, dependencies or checkpoint.
    stop_confirmed(layout)?;
    if runtime::probe(&rt) != Probe::Nothing {
        return Err(port_error(&rt));
    }
    let uv = match process::find_on_path("uv") {
        Some(uv) => uv,
        None => install_uv(p, yes, layout, &guard)?,
    };
    // Invalidate the old success marker before any shared files change.
    match fs::remove_file(layout.installed()) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(lifecycle_error(layout, e)),
    }
    // 4. Code.
    fs::create_dir_all(layout.root()).map_err(|e| {
        err(
            3,
            format!("{STEP}: cannot create {dir} ({e}); check its permissions, then run `mailtriage setup` again"),
        )
    })?;
    checkout(p, &git, layout, &guard)?;
    // 5. Python environment (uv fetches the Python version Kev pins).
    p.say("Setting up Kev's Python environment with uv.");
    let mut sync = Command::new(&uv);
    sync.args(["sync", "--frozen", "--extra", "serve"])
        .current_dir(layout.src())
        .envs(layout.uv_env())
        .env_remove("KEV_API_KEY")
        .env_remove("HF_HUB_OFFLINE")
        .env_remove("UV_OFFLINE")
        .env_remove("UV_NO_SYNC");
    if !visible(&mut sync, layout, &guard) {
        return Err(err(
            3,
            format!(
                "{STEP}: uv sync failed in {}; see the output above, then run `mailtriage setup` again",
                layout.src().display()
            ),
        ));
    }
    // 6/7. Only this installer can authorize its download supervisor.
    // Explicit cleanup covers every error; success is never recorded when
    // the owned supervisor failed to acknowledge whole-group termination.
    let checked = start(p, &rt, &uv, &guard).and_then(|_| check(provider, account));
    stop_confirmed(layout)?;
    checked.map_err(|e| {
        if e.downcast_ref::<crate::service::ServiceError>().is_some() {
            return e;
        }
        err(
            3,
            format!(
                "{STEP}: the test classification failed ({e}); see {}; {}",
                layout.log().display(),
                weight_repair(layout)
            ),
        )
    })?;
    Installed {
        tag: TAG.to_owned(),
        checkpoint: rt.checkpoint.clone(),
        installed_at: chrono::Utc::now().to_rfc3339(),
        uv,
    }
    .write(layout)
    .map_err(|e| {
        err(
            3,
            format!(
                "{STEP}: cannot write {} ({e}); check its permissions, then run `mailtriage setup` again",
                layout.installed().display()
            ),
        )
    })?;
    p.say(&format!("Local Kev is installed in {dir}."));
    Ok(report("installed"))
}

fn lifecycle_error(layout: &Layout, e: impl std::fmt::Display) -> anyhow::Error {
    err(3, format!("{STEP}: {e}; shutdown must be confirmed before changing this install. If the supervisor is still alive, retry `mailtriage setup --update --yes`. For abandoned ownership, stop mailtriage callers, independently verify the previous Kev process group has ended (do not signal saved PIDs), then hold exclusive locks on {} and {} while removing {} and truncating {}; retry `mailtriage setup --update --yes`", layout.installation_lock().display(), layout.lock().display(), layout.ownership().display(), layout.lock().display()))
}

fn stop_confirmed(layout: &Layout) -> Result<()> {
    match runtime::stop(layout, STOP_WAIT) {
        Ok(StopOutcome::AlreadyStopped | StopOutcome::Stopped) => Ok(()),
        Ok(StopOutcome::TimedOut) => Err(lifecycle_error(layout, "local Kev shutdown timed out")),
        Err(e) => Err(lifecycle_error(layout, e)),
    }
}

fn port_error(rt: &Runtime) -> anyhow::Error {
    err(3, format!("{STEP}: port {} is used by another program; stop it yourself or change provider.endpoint, then run `mailtriage setup --update` again", rt.port))
}

fn reject_unrelated(rt: &Runtime, needs_mutation: bool) -> Result<()> {
    let running =
        runtime::try_supervisor_running(&rt.layout).map_err(|e| lifecycle_error(&rt.layout, e))?;
    match runtime::probe(rt) {
        Probe::Nothing => Ok(()),
        Probe::Ready if running => Ok(()),
        // A live layout lock holder can be deliberately adopted even when
        // a crashed installer invalidated its receipt. Stop addresses only
        // that holder; port silence is checked again before any mutation.
        Probe::Other if running && needs_mutation => Ok(()),
        _ => Err(port_error(rt)),
    }
}

/// Repair only our known private cache directory, without depending on Hub
/// internals. Missing directories are fetched normally on the next install.
fn weight_repair(layout: &Layout) -> String {
    format!("for missing or corrupt weights, stop mailtriage callers and verify owned shutdown first; while holding an exclusive lock on {}, move {} aside and remove {}, then run `mailtriage setup --update --yes` again", layout.installation_lock().display(), layout.hf().display(), layout.installed().display())
}

/// Runs `command` with its output on stderr (stdout carries setup's
/// result); `true` when it exits 0.
fn visible(command: &mut Command, layout: &Layout, guard: &installation::Guard) -> bool {
    let run = || -> io::Result<bool> {
        let mut helper = Command::new(std::env::current_exe()?);
        let cwd = match command.get_current_dir() {
            Some(path) => path.to_owned(),
            None => std::env::current_dir()?,
        };
        helper
            .args(["kev", "mutate", "--dir"])
            .arg(layout.root())
            .arg("--program")
            .arg(command.get_program())
            .arg("--cwd")
            .arg(cwd)
            .arg("--timeout-seconds")
            .arg(super::download_timeout().as_secs().to_string());
        for (name, value) in command.get_envs() {
            match value {
                Some(value) => {
                    helper.env(name, value);
                }
                None => {
                    helper.env_remove(name);
                }
            }
        }
        installation::clear_authorization(&mut helper);
        guard.retain_for_mutation(&mut helper)?;
        helper
            .arg("--")
            .args(command.get_args())
            .stdin(Stdio::null())
            .stdout(Stdio::from(io::stderr()))
            .stderr(Stdio::inherit());
        Ok(helper.status()?.success())
    };
    run().unwrap_or(false)
}

/// Free space for `dir`, in GB: the test value, else `df -Pk` on its
/// nearest existing ancestor; `None` when it cannot be told.
fn free_gb(dir: &Path) -> Option<u64> {
    if let Some(gb) = test_kev().free_gb {
        return Some(gb);
    }
    let existing = dir.ancestors().find(|p| p.exists())?;
    let out = process::run_bounded(
        Path::new("/bin/df"),
        &[OsStr::new("-Pk"), existing.as_os_str()],
        Duration::from_secs(10),
        64 * 1024,
    )
    .ok()?
    .success()?;
    let text = String::from_utf8(out).ok()?;
    let kb: u64 = text
        .lines()
        .nth(1)?
        .split_whitespace()
        .nth(3)?
        .parse()
        .ok()?;
    Some(kb / (1024 * 1024))
}

/// This Mac's memory in GB: the test value, else `sysctl -n hw.memsize`.
fn memory_gb() -> Option<u64> {
    if let Some(gb) = test_kev().memory_gb {
        return Some(gb);
    }
    let out = process::run_bounded(
        Path::new("/usr/sbin/sysctl"),
        &["-n", "hw.memsize"],
        Duration::from_secs(10),
        4096,
    )
    .ok()?
    .success()?;
    let bytes: u64 = String::from_utf8(out).ok()?.trim().parse().ok()?;
    Some(bytes / (1024 * 1024 * 1024))
}

/// Step 3 for a missing uv: `brew install uv` when Homebrew is there and
/// you agree (`--yes` agrees), else the installer command.
fn install_uv(
    p: &mut Prompter,
    yes: bool,
    layout: &Layout,
    guard: &installation::Guard,
) -> Result<PathBuf> {
    let Some(brew) = process::find_on_path("brew") else {
        return Err(err(
            3,
            format!("{STEP}: uv is not on PATH; install it with `{UV_INSTALLER}`, then run `mailtriage setup` again"),
        ));
    };
    let wanted = if p.enabled() {
        p.confirm(
            "uv is not on PATH. Install it with Homebrew (brew install uv)?",
            true,
        )?
    } else {
        yes
    };
    if !wanted {
        return Err(err(
            3,
            format!("{STEP}: uv is not on PATH; install it with `brew install uv`, then run `mailtriage setup` again"),
        ));
    }
    p.say("Installing uv with Homebrew.");
    if !visible(Command::new(&brew).args(["install", "uv"]), layout, guard) {
        return Err(err(
            3,
            format!("{STEP}: `brew install uv` failed; see the output above, then run `mailtriage setup` again"),
        ));
    }
    process::find_on_path("uv").ok_or_else(|| {
        err(
            3,
            format!("{STEP}: uv is still not on PATH after `brew install uv`; open a new terminal, then run `mailtriage setup` again"),
        )
    })
}

/// Step 4: `src` at `TAG`, reused when it is there; otherwise cloned into
/// `src.partial` and renamed, so a broken clone never passes for one.
fn checkout(
    p: &mut Prompter,
    git: &Path,
    layout: &Layout,
    guard: &installation::Guard,
) -> Result<()> {
    let src = layout.src();
    if src.exists() {
        if tag_of(git, &src).as_deref() == Some(TAG) {
            p.say(&format!("Reusing Kev {TAG} in {}.", src.display()));
            return Ok(());
        }
        fs::remove_dir_all(&src).map_err(|e| {
            err(
                3,
                format!(
                    "{STEP}: cannot replace {} ({e}); remove it, then run `mailtriage setup` again",
                    src.display()
                ),
            )
        })?;
    }
    let partial = layout.partial();
    if partial.exists() {
        fs::remove_dir_all(&partial).map_err(|e| err(3, format!("{STEP}: cannot remove {} ({e}); check its permissions, then run `mailtriage setup` again", partial.display())))?;
    }
    p.say(&format!("Downloading Kev {TAG}."));
    let mut clone = Command::new(git);
    clone
        .args(["clone", "--depth", "1", "--branch", TAG, REPO])
        .arg(&partial)
        .env("GIT_TERMINAL_PROMPT", "0");
    if !visible(&mut clone, layout, guard) {
        return Err(err(
            3,
            format!("{STEP}: `git clone` of {REPO} failed; check your network, then run `mailtriage setup` again"),
        ));
    }
    fs::rename(&partial, &src).map_err(|e| {
        err(
            3,
            format!(
                "{STEP}: cannot move {} to {} ({e}); then run `mailtriage setup` again",
                partial.display(),
                src.display()
            ),
        )
    })
}

/// The tag `src` is checked out at, if exactly one.
fn tag_of(git: &Path, src: &Path) -> Option<String> {
    let args = [
        OsStr::new("-C"),
        src.as_os_str(),
        OsStr::new("describe"),
        OsStr::new("--tags"),
        OsStr::new("--exact-match"),
    ];
    let out = process::run_bounded(git, &args, Duration::from_secs(10), 4096)
        .ok()?
        .success()?;
    Some(String::from_utf8_lossy(&out).trim().to_owned())
}

/// Step 6: the supervisor with downloads allowed. Its log is shown while
/// the weights download, for up to `download_timeout()`.
fn start(p: &mut Prompter, rt: &Runtime, uv: &Path, guard: &installation::Guard) -> Result<()> {
    let log = rt.layout.log();
    let failed = |what: String| {
        err(
            3,
            format!(
                "{STEP}: {what}; see {}; {}",
                log.display(),
                weight_repair(&rt.layout)
            ),
        )
    };
    let mut shown = fs::metadata(&log).map_or(0, |m| m.len());
    runtime::spawn_install_supervisor(rt, uv, guard)
        .map_err(|e| failed(format!("cannot start Kev ({e})")))?;
    p.say(
        "Starting Kev. The first start downloads the model (about 9 GB), which can take a while.",
    );
    let began = Instant::now();
    let wait = super::download_timeout();
    loop {
        shown = show_log(p, &log, shown);
        match runtime::probe(rt) {
            Probe::Ready => {
                if runtime::try_supervisor_running(&rt.layout).map_err(|e| lifecycle_error(&rt.layout,e))? {
                    runtime::shutdown_confirmed(&rt.layout).map_err(|e| lifecycle_error(&rt.layout,e))?;
                    return Ok(());
                }
                return Err(port_error(rt));
            },
            Probe::Other => {
                return Err(err(
                    3,
                    format!("{STEP}: port {} is used by another program; stop it, then run `mailtriage setup` again", rt.port),
                ))
            }
            Probe::Nothing => {}
        }
        if began.elapsed() > START_GRACE && !runtime::supervisor_running(&rt.layout) {
            show_log(p, &log, shown);
            return Err(failed("Kev stopped while it started".to_owned()));
        }
        if began.elapsed() >= wait {
            let wait = match wait.as_secs() {
                s if s >= 60 => format!("{} minutes", s / 60),
                s => format!("{s} seconds"),
            };
            return Err(failed(format!("Kev did not answer within {wait}")));
        }
        thread::sleep(Duration::from_secs(1));
    }
}

/// Shows the complete lines `log` gained after byte `shown`, indented and
/// cut to 160 characters; returns the new offset.
fn show_log(p: &mut Prompter, log: &Path, shown: u64) -> u64 {
    let Ok(mut file) = fs::File::open(log) else {
        return shown;
    };
    let mut bytes = Vec::new();
    if file
        .seek(SeekFrom::Start(shown))
        .and_then(|_| file.read_to_end(&mut bytes))
        .is_err()
    {
        return shown;
    }
    let Some(end) = bytes.iter().rposition(|&b| b == b'\n') else {
        return shown;
    };
    for line in String::from_utf8_lossy(&bytes[..end]).split(['\n', '\r']) {
        let line = line.trim();
        if !line.is_empty() {
            p.say(&format!("  {}", line.chars().take(160).collect::<String>()));
        }
    }
    shown + end as u64 + 1
}

/// Step 7: one classification of a fixed message, straight to the running
/// server: `installed.json` is not written yet, so the managed path would
/// refuse. It must pass the contract check.
fn check(provider: &ProviderConfig, account: &AccountConfig) -> Result<()> {
    let mut direct = provider.clone();
    direct.managed = None;
    let request = DecisionRequest::new(account, &sample(), chrono::Utc::now().to_rfc3339());
    let decision = provider::provider_for("kev")?.decide(&direct, &request, None)?;
    provider::check_decision(&decision, &request.questions)
}

fn sample() -> NormalizedMessage {
    NormalizedMessage {
        raw_sha256: "kev-setup-check".into(),
        from: vec![Address {
            email: "alex@example.invalid".into(),
            name: Some("Alex".into()),
        }],
        to: vec![],
        cc: vec![],
        subject: "Lunch on Friday?".into(),
        sent_at: None,
        body: "Hi, are you free for lunch on Friday at noon? Let me know by Thursday.".into(),
        incomplete: false,
        warnings: vec![],
    }
}
