//! `mailtriage setup`: a guided first run. Every question is also a flag,
//! so agents run it without prompts. Setup writes only the mailtriage config
//! and its state directory: it makes no IMAP changes, never handles the API
//! key and never selects `live` filing.
use crate::{
    config, distribution,
    domain::{
        AccountConfig, AppConfig, Category, EngineConfig, FilingConfig, FilingMode, HimalayaConfig,
        ProviderConfig, UpdateMode,
    },
    engine::{
        self,
        versions::{self, Tested},
    },
    filing, process,
    prompt::Prompter,
    provider,
    secrets::{self, KeyStore},
    service::{self, err, err_kind, ErrorKind, Service, ServiceError},
    service_control,
    system_service::{self, Context, Manager},
};
use anyhow::{anyhow, Result};
use fs2::FileExt;
use serde_json::{json, Value};
use std::{
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

const HIMALAYA_TIMEOUT: Duration = Duration::from_secs(60);
const HIMALAYA_MAX_OUTPUT: usize = 1024 * 1024;

/// Every setup error starts with its step, then names the flag or the
/// command that fixes it.
const STEP_ACCOUNT: &str = "step 3 (account)";
const STEP_CLASSIFIER: &str = "step 5 (classifier)";
const STEP_KEY: &str = "step 5 (key)";
const STEP_SERVICE: &str = "step 10 (service)";
/// The docs page that records which mail providers passed the live check.
pub const PROVIDER_CHECK_URL: &str =
    "https://wir-drei-digital.github.io/mailtriage/guide/provider-check";

/// Answers given as flags. `None` (or empty) means: ask, or without
/// prompts take the default, or fail naming the flag.
#[derive(Debug, Clone, Default)]
pub struct SetupArgs {
    pub update: bool,
    /// Stdin is a terminal, so a key tool can prompt even with `--yes`.
    pub terminal: bool,
    pub himalaya_binary: Option<PathBuf>,
    /// `--himalaya-install`: answer yes to installing a private Himalaya
    /// when the one found is missing or untested, also without prompts.
    pub himalaya_install: bool,
    pub himalaya_config: Option<PathBuf>,
    pub himalaya_account: Option<String>,
    pub account: Option<String>,
    pub identity: Option<String>,
    pub timezone: Option<String>,
    pub brief: Option<String>,
    pub mailboxes: Vec<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub key_store: Option<KeyStore>,
    pub key_command: Option<String>,
    pub key_env: Option<String>,
    pub key_stored: bool,
    pub filing: Option<FilingMode>,
    /// `Some(true)` installs the background service, `Some(false)` skips
    /// it; `None` asks (default yes) or, without prompts, skips it.
    pub service: Option<bool>,
    pub interval_seconds: u64,
    pub limit: usize,
    /// `--updates`; `None` keeps the existing config's value (a new config
    /// gets `auto`).
    pub updates: Option<UpdateMode>,
}

/// What step 1 decided.
enum Intent {
    /// No config yet.
    Create,
    /// Update this existing account: chosen from the menu, or named by
    /// `--account`.
    Update(String),
    /// Add an account; its name must be new.
    Add,
    /// `--update` without prompts and without `--account`: update the
    /// account the name defaults to, or add it.
    UpdateOrAdd,
}

struct HimalayaChoice {
    binary: PathBuf,
    /// The tested version `--version` reported, as `X.Y.Z`; written to
    /// `expected_version`.
    version: String,
    toml: PathBuf,
    account: String,
    email: Option<String>,
}

struct HimalayaAccount {
    name: String,
    default: bool,
}

pub fn run(args: &SetupArgs, path: &Path, p: &mut Prompter) -> Result<Value> {
    // 1. Config, and the bytes it was read from for the write in step 8.
    let (mut cfg, intent, read) = load_target(args, path, p)?;
    // 2. Himalaya. An account being updated keeps its own by default.
    let stored = match &intent {
        Intent::Update(name) => cfg
            .accounts
            .get(name)
            .and_then(|account| stored_engine(account, path)),
        _ => None,
    };
    // Step 2's fix names the account when it is already known.
    let known = match &intent {
        Intent::Update(name) => Some(name.as_str()),
        _ => args.account.as_deref(),
    };
    let h = himalaya_step(args, p, stored.as_ref(), path, known)?;
    // 3. Account details.
    let name = account_name(args, p, &h, &cfg, &intent)?;
    let previous = cfg.accounts.get(&name).cloned();
    let identity = answer(
        p,
        STEP_ACCOUNT,
        args.identity.as_deref(),
        "--identity",
        "Your email address for this account",
        previous
            .as_ref()
            .map(|a| a.identity.as_str())
            .or(h.email.as_deref()),
        nonempty,
    )?;
    let zone = previous.as_ref().map_or_else(
        || {
            default_timezone(
                std::env::var("TZ").ok().as_deref(),
                fs::read_link("/etc/localtime").ok().as_deref(),
            )
        },
        |a| a.timezone.clone(),
    );
    let timezone = answer(
        p,
        STEP_ACCOUNT,
        args.timezone.as_deref(),
        "--timezone",
        "Time zone",
        Some(&zone),
        |t| {
            if t.is_empty() || t.contains(char::is_whitespace) {
                Err("Use a time zone name such as Europe/Berlin.".to_owned())
            } else {
                Ok(t.to_owned())
            }
        },
    )?;
    let brief = answer(
        p,
        STEP_ACCOUNT,
        args.brief.as_deref(),
        "--brief",
        "One line about you that helps classification (optional)",
        Some(previous.as_ref().map_or("", |a| a.brief.as_str())),
        |b| Ok(b.trim().to_owned()),
    )?;
    // 4. Folders.
    let old_engine = previous
        .as_ref()
        .and_then(|a| a.engine_config().map(|EngineConfig::Himalaya(e)| e));
    let mut engine = HimalayaConfig {
        binary: h.binary.clone(),
        config: h.toml.clone(),
        account: h.account.clone(),
        mailboxes: vec!["INBOX".to_owned()],
        expected_version: h.version.clone(),
        timeout_seconds: old_engine.as_ref().map_or(60, |e| e.timeout_seconds),
        max_output_bytes: old_engine
            .as_ref()
            .map_or(50_000_000, |e| e.max_output_bytes),
    };
    engine.mailboxes = folders_step(
        args,
        p,
        &engine,
        old_engine.as_ref().map(|e| e.mailboxes.as_slice()),
    )?;
    // 5. Classifier.
    let current = (!matches!(intent, Intent::Create)).then(|| cfg.provider.clone());
    let (provider, store) = classifier_step(args, p, current.as_ref())?;
    // 6. Categories: a new account gets the defaults, shown once the
    // config is written.
    let categories = match &previous {
        Some(a) => a.categories.clone(),
        None => default_categories(),
    };
    // 7. Filing.
    let mode = filing_step(
        args,
        p,
        previous.as_ref().map(|a| a.filing.mode),
        &engine.mailboxes,
    )?;
    // 8. Write.
    let filing_config = FilingConfig {
        mode,
        ..previous
            .as_ref()
            .map(|a| a.filing.clone())
            .unwrap_or_default()
    };
    cfg.accounts.insert(
        name.clone(),
        AccountConfig {
            identity,
            timezone,
            brief,
            taxonomy_revision: previous.as_ref().map_or(1, |a| a.taxonomy_revision),
            categories,
            himalaya: None,
            engine: Some(EngineConfig::Himalaya(engine.clone())),
            filing: filing_config,
        },
    );
    cfg.provider = provider;
    cfg.updates = args.updates.unwrap_or(cfg.updates);
    config::validate(&cfg).map_err(|e| {
        err(
            2,
            format!("step 8 (write): {e}; nothing was written; run setup again with other answers, or with --filing off for a category folder problem"),
        )
    })?;
    // `ensure` would refuse every later command for an account whose binding
    // changed (exit 5), so such an update writes nothing. A state database
    // that cannot be read is left to step 9, which reports it.
    if let Ok(Some(false)) = service::stored_binding_matches(path, &cfg, &name) {
        return Err(err_kind(
            5,
            ErrorKind::BindingConflict,
            format!(
                "{STEP_ACCOUNT}: account {name} is bound to its previous mailbox (identity, Himalaya account or IMAP server changed); keep them, or set this mailbox up under a new name with --account NEW"
            ),
        ));
    }
    let unwritable = || {
        err(
            3,
            format!(
                "step 8 (write): could not write {}; make its directory writable or pass --config",
                path.display()
            ),
        )
    };
    write_config(path, &cfg, read.as_deref(), unwritable)?;
    let path = fs::canonicalize(path).map_err(|_| unwritable())?;
    p.say(&format!("Wrote {}.", path.display()));
    let shown = printed_config(p, &path);
    let shown = shown.as_deref();
    if previous.is_none() {
        p.say(&categories_hint(
            shown,
            &name,
            &cfg.accounts[&name].categories,
        ));
    }
    // 9. Check.
    let doctor = doctor_step(p, &path, shown, &name, &cfg, &engine);
    // 10. Service, only when the state check passed: otherwise every pass
    // of the service would fail.
    let state_ok = doctor["items"]
        .as_array()
        .is_some_and(|items| items.iter().all(|i| i["check"] != "state"));
    let service = service_step(args, p, &path, shown, &name, &cfg.provider, state_ok)?;
    next_steps(p, shown, &name, mode, !service.is_null());
    Ok(json!({"schema_version": 1, "setup": {
        "config": path,
        "account": name,
        "mailboxes": engine.mailboxes,
        "provider": cfg.provider.kind,
        "model": cfg.provider.model,
        "key_source": key_source_value(&cfg.provider),
        "key_store": store.map(KeyStore::flag),
        "filing": filing::mode_str(mode),
        "updates": cfg.updates.as_str(),
        "doctor": doctor,
        "service": service,
    }}))
}

/// Step 8's write: under the exclusive config lock, and only when the file
/// still holds the bytes step 1 read (`None`: there was no file).
fn write_config(
    path: &Path,
    cfg: &AppConfig,
    read: Option<&[u8]>,
    unwritable: impl Fn() -> anyhow::Error,
) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|_| unwritable())?;
    let lock = config_lock(path).map_err(|_| unwritable())?;
    lock.try_lock_exclusive().map_err(|_| {
        err_kind(
            5,
            ErrorKind::ConfigBusy,
            "step 8 (write): configuration is being edited; nothing was written; run setup again when the other command has finished",
        )
    })?;
    if fs::read(path).ok().as_deref() != read {
        return Err(err_kind(
            5,
            ErrorKind::ConfigChanged,
            format!(
                "step 8 (write): {} changed since setup read it; nothing was written; run setup again",
                path.display()
            ),
        ));
    }
    config::save(path, cfg).map_err(|_| unwritable())
}

/// The lock file of the commands that edit `path`: `mailtriage.lock` next
/// to `mailtriage.json`, as `Service` uses it. Like `Service`, it is next
/// to the canonical path, so a symlinked config shares the lock of the file
/// it names; without a file yet, it is next to `path`.
fn config_lock(path: &Path) -> std::io::Result<File> {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path.with_extension("lock"))
}

/// Step 1: the config to change, what to do with it, and the bytes it was
/// read from (`None` when there is no config yet).
fn load_target(
    args: &SetupArgs,
    path: &Path,
    p: &mut Prompter,
) -> Result<(AppConfig, Intent, Option<Vec<u8>>)> {
    if !path.exists() {
        let mut cfg = config::default_config();
        cfg.accounts.clear();
        cfg.state_dir = PathBuf::from("state");
        return Ok((cfg, Intent::Create, None));
    }
    let bytes = fs::read(path).map_err(|e| {
        err(
            2,
            format!(
                "step 1 (config): cannot read {} ({e}); fix it or pass --config",
                path.display()
            ),
        )
    })?;
    let cfg = config::from_bytes(&bytes).map_err(|e| {
        err(
            2,
            format!(
                "step 1 (config): {} is not a valid config ({e:#}); fix it or pass --config",
                path.display()
            ),
        )
    })?;
    // `--account` answers the menu: an existing name is updated, a new one
    // added.
    let named = args.account.as_ref().map(|name| {
        if cfg.accounts.contains_key(name) {
            Intent::Update(name.clone())
        } else {
            Intent::Add
        }
    });
    if !p.enabled() {
        if !args.update {
            return Err(err(
                5,
                format!(
                    "step 1 (config): {} already exists; pass --update to change it",
                    path.display()
                ),
            ));
        }
        return Ok((cfg, named.unwrap_or(Intent::UpdateOrAdd), Some(bytes)));
    }
    p.say(&format!("A config already exists at {}.", path.display()));
    if let Some(intent) = named {
        return Ok((cfg, intent, Some(bytes)));
    }
    let actions = ["Update an account", "Add an account", "Abort"].map(String::from);
    match p.choose("What would you like to do?", &actions, 0)? {
        0 => {
            let names: Vec<String> = cfg.accounts.keys().cloned().collect();
            let pick = p.choose("Which account?", &names, 0)?;
            let name = names[pick].clone();
            Ok((cfg, Intent::Update(name), Some(bytes)))
        }
        1 => Ok((cfg, Intent::Add, Some(bytes))),
        _ => Err(err_kind(
            2,
            ErrorKind::SetupAborted,
            "setup aborted; nothing was changed",
        )),
    }
}

/// The Himalaya settings of an account being updated, with relative paths
/// resolved against the config's directory as `Service` resolves them, and
/// a bare binary name looked up on `PATH`.
fn stored_engine(account: &AccountConfig, config_path: &Path) -> Option<HimalayaConfig> {
    let EngineConfig::Himalaya(mut h) = account.engine_config()?;
    let base = std::path::absolute(config_path)
        .ok()?
        .parent()?
        .to_path_buf();
    if h.config.is_relative() {
        h.config = base.join(&h.config);
    }
    if h.binary.is_relative() {
        if h.binary.components().count() > 1 {
            h.binary = base.join(&h.binary);
        } else if let Some(found) = process::find_on_path(&h.binary.to_string_lossy()) {
            h.binary = found;
        }
    }
    Some(h)
}

/// Step 2: the Himalaya binary, its config and the account, checked. For
/// each, a flag wins, then the `stored` settings of an account being
/// updated, then `HIMALAYA_CONFIG` (config only), then discovery. `path`
/// (the config setup will write) and `account` (the mailtriage account,
/// when already known) go into the fix for a missing or untested Himalaya.
fn himalaya_step(
    args: &SetupArgs,
    p: &mut Prompter,
    stored: Option<&HimalayaConfig>,
    path: &Path,
    account: Option<&str>,
) -> Result<HimalayaChoice> {
    let found = match (&args.himalaya_binary, stored) {
        (Some(binary), _) => Some(absolute(binary, "--himalaya-binary")?),
        (None, Some(stored)) => Some(stored.binary.clone()),
        (None, None) => process::find_on_path("himalaya"),
    };
    let checked = match &found {
        Some(binary) => himalaya_version(binary).map(|version| (binary.clone(), version)),
        None => Err("himalaya is not on PATH".to_owned()),
    };
    let (binary, (line, tested)) = match checked {
        Ok(found) => found,
        Err(why) => private_himalaya(args, p, &why, path, account)?,
    };
    p.say(&format!("Using {line} at {}.", binary.display()));
    if in_homebrew_keg(&binary) {
        p.say(BREW_PIN_NOTE);
    }
    let explicit = match (&args.himalaya_config, stored) {
        (Some(toml), _) => {
            let toml = absolute(toml, "--himalaya-config")?;
            if !toml.is_file() {
                return Err(err(
                    2,
                    format!(
                        "step 2 (Himalaya): --himalaya-config: {} does not exist; pass the file that defines the account",
                        toml.display()
                    ),
                ));
            }
            Some(toml)
        }
        (None, Some(stored)) => Some(stored.config.clone()),
        (None, None) => himalaya_config_env()?,
    };
    // Without prompts the stored account stands in for --himalaya-account;
    // with prompts it is the menu's default.
    let stored_account = stored.map(|s| s.account.as_str());
    loop {
        let toml = explicit
            .clone()
            .or_else(default_himalaya_config)
            .filter(|t| t.is_file());
        let accounts = match &toml {
            Some(toml) => list_accounts(&binary, toml)?,
            None => Vec::new(),
        };
        let Some(toml) = toml.filter(|_| !accounts.is_empty()) else {
            if !p.enabled() {
                return Err(err(
                    3,
                    "step 2 (Himalaya): no Himalaya account with IMAP found; run `himalaya configure` or pass --himalaya-config",
                ));
            }
            if !p.confirm(
                "No Himalaya account with IMAP was found. Create one now with `himalaya configure`?",
                true,
            )? {
                return Err(err(
                    3,
                    "step 2 (Himalaya): no Himalaya account; run `himalaya configure`, then setup again",
                ));
            }
            configure(&binary, explicit.as_deref())?;
            continue;
        };
        let wanted = args
            .himalaya_account
            .as_deref()
            .or(stored_account.filter(|_| !p.enabled()));
        let name = match wanted {
            Some(name) if accounts.iter().any(|a| a.name == name) => name.to_owned(),
            Some(name) => {
                return Err(err(
                    2,
                    format!(
                        "step 2 (Himalaya): {} has no IMAP account named {name}; pass --himalaya-account with one of its accounts",
                        toml.display()
                    ),
                ))
            }
            None if !p.enabled() => {
                return Err(err(
                    2,
                    "step 2 (Himalaya): --himalaya-account is required without prompts",
                ))
            }
            None => {
                let mut options: Vec<String> = accounts.iter().map(|a| a.name.clone()).collect();
                options.push("Create a new account with `himalaya configure`".to_owned());
                let default = stored_account
                    .and_then(|s| accounts.iter().position(|a| a.name == s))
                    .or_else(|| accounts.iter().position(|a| a.default))
                    .unwrap_or(0);
                let pick = p.choose(
                    "Which Himalaya account should mailtriage use?",
                    &options,
                    default,
                )?;
                if pick == accounts.len() {
                    configure(&binary, explicit.as_deref())?;
                    continue;
                }
                accounts[pick].name.clone()
            }
        };
        check_account(&binary, &toml, &name)?;
        let email = account_email(&toml, &name);
        return Ok(HimalayaChoice {
            binary,
            version: tested.version.clone(),
            toml,
            account: name,
            email,
        });
    }
}

/// Printed once when the chosen Himalaya is in a Homebrew keg.
pub const BREW_PIN_NOTE: &str = "Homebrew may upgrade Himalaya to a version mailtriage has not tested; \"brew pin himalaya\" holds it, or run mailtriage himalaya install for a private copy.";

/// The first line of `binary --version` and its tested entry, or why the
/// binary cannot be used.
fn himalaya_version(binary: &Path) -> Result<(String, &'static Tested), String> {
    match run_himalaya(binary, &[OsStr::new("--version")]) {
        Ok(out) => {
            versions::check_version_output(&out).map_err(|u| format!("{}: {u}", binary.display()))
        }
        Err(_) => Err(format!("{} does not run", binary.display())),
    }
}

/// Step 2 for a Himalaya that is missing or untested (`why`): offers the
/// private copy (default yes); `--himalaya-install` answers yes, also
/// without prompts. Otherwise the step fails with the fix, which passes
/// `--config` when needed for `path`, and the account and Himalaya account
/// known so far.
fn private_himalaya(
    args: &SetupArgs,
    p: &mut Prompter,
    why: &str,
    path: &Path,
    account: Option<&str>,
) -> Result<(PathBuf, (String, &'static Tested))> {
    let newest = &versions::newest().version;
    let wanted = args.himalaya_install
        || (p.enabled() && {
            p.say(&format!("{why}."));
            p.confirm(&format!("Install Himalaya {newest} for mailtriage?"), true)?
        });
    if !wanted {
        let mut known = Vec::new();
        if let Some(account) = account {
            known.extend(["--account", account]);
        }
        if let Some(himalaya) = args.himalaya_account.as_deref() {
            known.extend(["--himalaya-account", himalaya]);
        }
        let fix = himalaya_install_fix(command_config(path).as_deref(), &known);
        return Err(err(
            3,
            format!("step 2 (Himalaya): {why}; {fix}, or pass --himalaya-install"),
        ));
    }
    p.say(&format!("Installing Himalaya {newest} for mailtriage."));
    let installed = distribution::himalaya::install_default().map_err(|e| {
        err(
            service::exit_code(&e),
            format!(
                "step 2 (Himalaya): could not install Himalaya {newest}: {}",
                error_text(&e)
            ),
        )
    })?;
    p.say(&format!(
        "Installed Himalaya {} at {}.",
        installed.version,
        installed.path.display()
    ));
    let version = himalaya_version(&installed.path)
        .map_err(|why| err(3, format!("step 2 (Himalaya): {why}")))?;
    Ok((installed.path, version))
}

/// The fix for a missing or untested Himalaya: the private copy, then setup
/// pointed at the path `himalaya install` prints. `config` and `args` go
/// into the setup command as in [`mailtriage_line`].
pub fn himalaya_install_fix(config: Option<&Path>, args: &[&str]) -> String {
    let path = distribution::himalaya::default_data_dir()
        .map(|data| distribution::himalaya::binary_path(&data, &versions::newest().version));
    let path = path.map_or_else(|| "<the path it prints>".to_owned(), |p| shell_line(&[p]));
    let setup = mailtriage_line(config, &["setup", "--update"], args);
    format!("run mailtriage himalaya install, then {setup} --himalaya-binary {path}")
}

/// Whether `binary` is in a Homebrew keg: its canonical path contains
/// `/Cellar/himalaya/`.
fn in_homebrew_keg(binary: &Path) -> bool {
    fs::canonicalize(binary).is_ok_and(|p| p.to_string_lossy().contains("/Cellar/himalaya/"))
}

/// Himalaya v2.1.0's config search order without `--config` (verified on
/// macOS; Linux follows the `dirs` crate): the platform config dir, then
/// `~/.config`, then `~/.himalayarc`.
pub fn himalaya_config_candidates(
    home: &Path,
    xdg_config_home: Option<&Path>,
    macos: bool,
) -> Vec<PathBuf> {
    let platform = if macos {
        home.join("Library/Application Support")
    } else {
        xdg_config_home
            .filter(|p| p.is_absolute())
            .map_or_else(|| home.join(".config"), Path::to_path_buf)
    };
    let mut candidates = vec![platform.join("himalaya/config.toml")];
    let dot_config = home.join(".config/himalaya/config.toml");
    if !candidates.contains(&dot_config) {
        candidates.push(dot_config);
    }
    candidates.push(home.join(".himalayarc"));
    candidates
}

fn default_himalaya_config() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)?;
    let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
    himalaya_config_candidates(&home, xdg.as_deref(), cfg!(target_os = "macos"))
        .into_iter()
        .find(|candidate| candidate.is_file())
}

/// `HIMALAYA_CONFIG`, if set; several `:`-separated files are refused
/// because mailtriage passes exactly one `--config`.
fn himalaya_config_env() -> Result<Option<PathBuf>> {
    let Some(value) = std::env::var_os("HIMALAYA_CONFIG").filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let paths: Vec<PathBuf> = std::env::split_paths(&value).collect();
    if paths.len() != 1 {
        return Err(err(
            2,
            "step 2 (Himalaya): HIMALAYA_CONFIG names several files; pass --himalaya-config with the file that defines the account",
        ));
    }
    Ok(Some(absolute(&paths[0], "--himalaya-config")?))
}

/// Runs Himalaya for setup's own checks; stdout when it exits 0.
fn run_himalaya(binary: &Path, args: &[&OsStr]) -> Result<Vec<u8>> {
    process::run_bounded(binary, args, HIMALAYA_TIMEOUT, HIMALAYA_MAX_OUTPUT)?
        .success()
        .ok_or_else(|| anyhow!("Himalaya command failed"))
}

fn list_accounts(binary: &Path, toml: &Path) -> Result<Vec<HimalayaAccount>> {
    let failed = || {
        err(
            3,
            format!(
                "step 2 (Himalaya): `{}` failed; check that file",
                shell_line(&[
                    binary.as_os_str(),
                    OsStr::new("--config"),
                    toml.as_os_str(),
                    OsStr::new("account"),
                    OsStr::new("list"),
                ])
            ),
        )
    };
    let out = run_himalaya(
        binary,
        &[
            OsStr::new("--config"),
            toml.as_os_str(),
            OsStr::new("--json"),
            OsStr::new("account"),
            OsStr::new("list"),
        ],
    )
    .map_err(|_| failed())?;
    let value: Value = serde_json::from_slice(&out).map_err(|_| failed())?;
    let rows = value["accounts"].as_array().ok_or_else(failed)?;
    Ok(rows
        .iter()
        .filter(|row| {
            row["backends"]
                .as_array()
                .is_some_and(|backends| backends.iter().any(|b| b == "imap"))
        })
        .filter_map(|row| {
            Some(HimalayaAccount {
                name: row["name"].as_str()?.to_owned(),
                default: row["default"] == true,
            })
        })
        .collect())
}

/// `account check` exits 0 even when the check fails, so its JSON decides.
fn check_account(binary: &Path, toml: &Path, name: &str) -> Result<()> {
    let ok = run_himalaya(
        binary,
        &[
            OsStr::new("--config"),
            toml.as_os_str(),
            OsStr::new("--account"),
            OsStr::new(name),
            OsStr::new("--backend"),
            OsStr::new("imap"),
            OsStr::new("--json"),
            OsStr::new("account"),
            OsStr::new("check"),
        ],
    )
    .ok()
    .and_then(|out| serde_json::from_slice::<Value>(&out).ok())
    .is_some_and(|report| {
        report["backends"].as_array().is_some_and(|backends| {
            backends
                .iter()
                .any(|b| b["backend"] == "imap" && b["ok"] == true)
        })
    });
    if ok {
        return Ok(());
    }
    Err(err(
        3,
        format!(
            "step 2 (Himalaya): account check failed for {name}; run `{}` to see why",
            shell_line(&[
                binary.as_os_str(),
                OsStr::new("--config"),
                toml.as_os_str(),
                OsStr::new("--account"),
                OsStr::new(name),
                OsStr::new("account"),
                OsStr::new("check"),
            ])
        ),
    ))
}

fn account_email(toml: &Path, name: &str) -> Option<String> {
    let text = fs::read_to_string(toml).ok()?;
    let value: toml::Value = toml::from_str(&text).ok()?;
    value
        .get("accounts")?
        .get(name)?
        .get("email")?
        .as_str()
        .map(str::to_owned)
}

/// Runs Himalaya's own wizard attached to the terminal. Its stdout goes to
/// stderr, so stdout keeps only setup's result.
fn configure(binary: &Path, toml: Option<&Path>) -> Result<()> {
    let mut command = Command::new(binary);
    if let Some(toml) = toml {
        command.arg("--config").arg(toml);
    }
    let status = command
        .arg("configure")
        .stdout(std::io::stderr())
        .status()
        .map_err(|_| err(3, "step 2 (Himalaya): could not start `himalaya configure`"))?;
    if !status.success() {
        return Err(err(
            3,
            "step 2 (Himalaya): `himalaya configure` did not finish; run it yourself, then setup again",
        ));
    }
    Ok(())
}

/// Step 3's name. An account chosen for update keeps its name.
fn account_name(
    args: &SetupArgs,
    p: &mut Prompter,
    h: &HimalayaChoice,
    cfg: &AppConfig,
    intent: &Intent,
) -> Result<String> {
    if let Intent::Update(name) = intent {
        return Ok(name.clone());
    }
    let must_be_new = matches!(intent, Intent::Add);
    answer(
        p,
        STEP_ACCOUNT,
        args.account.as_deref(),
        "--account",
        "Name for this account in mailtriage",
        Some(&h.account),
        |name| {
            if !config::valid_account_name(name) {
                Err("Use 1 to 64 ASCII letters, digits, - or _.".to_owned())
            } else if must_be_new && cfg.accounts.contains_key(name) {
                Err(format!("An account named {name} already exists."))
            } else {
                Ok(name.to_owned())
            }
        },
    )
}

/// `TZ` (a zone name, not a path), else the zone `/etc/localtime` points
/// to, else `UTC`.
pub fn default_timezone(tz: Option<&str>, localtime: Option<&Path>) -> String {
    if let Some(zone) = tz
        .map(|t| t.trim().trim_start_matches(':'))
        .filter(|t| !t.is_empty() && !t.starts_with('/'))
    {
        return zone.to_owned();
    }
    localtime
        .and_then(Path::to_str)
        .and_then(|target| target.split_once("zoneinfo/"))
        .map(|(_, zone)| zone)
        .filter(|zone| !zone.is_empty())
        .unwrap_or("UTC")
        .to_owned()
}

/// Step 4: folders to watch, from the server's list.
fn folders_step(
    args: &SetupArgs,
    p: &mut Prompter,
    engine: &HimalayaConfig,
    previous: Option<&[String]>,
) -> Result<Vec<String>> {
    let failed = || {
        err(
            3,
            format!(
                "step 4 (folders): could not list the folders of {}; run `{}`",
                engine.account,
                shell_line(&[
                    engine.binary.as_os_str(),
                    OsStr::new("--config"),
                    engine.config.as_os_str(),
                    OsStr::new("--account"),
                    OsStr::new(&engine.account),
                    OsStr::new("imap"),
                    OsStr::new("list"),
                    OsStr::new("--all"),
                ])
            ),
        )
    };
    let folders: Vec<String> = engine::open(&EngineConfig::Himalaya(engine.clone()))
        .and_then(|e| e.list_folders())
        .map_err(|_| failed())?
        .into_iter()
        .filter(|f| {
            !f.attributes
                .iter()
                .any(|a| a.eq_ignore_ascii_case("\\Noselect"))
        })
        .map(|f| f.name)
        .collect();
    if folders.is_empty() {
        return Err(failed());
    }
    if !args.mailboxes.is_empty() {
        let mut chosen: Vec<String> = Vec::new();
        for name in &args.mailboxes {
            if !folders.contains(name) {
                return Err(err(
                    2,
                    format!(
                        "step 4 (folders): --mailbox: no folder named {name} on the server; pass one of the server's folders"
                    ),
                ));
            }
            if !chosen.contains(name) {
                chosen.push(name.clone());
            }
        }
        return Ok(chosen);
    }
    let wanted = previous.map_or_else(|| vec!["INBOX".to_owned()], <[String]>::to_vec);
    let mut defaults: Vec<usize> = wanted
        .iter()
        .filter_map(|w| folders.iter().position(|f| f == w))
        .collect();
    if defaults.is_empty() {
        defaults.push(
            folders
                .iter()
                .position(|f| f.eq_ignore_ascii_case("INBOX"))
                .unwrap_or(0),
        );
    }
    if !p.enabled() {
        return Ok(defaults.iter().map(|&i| folders[i].clone()).collect());
    }
    let picked = p.choose_many(
        "Which folders should mailtriage watch?",
        &folders,
        &defaults,
    )?;
    Ok(picked.into_iter().map(|i| folders[i].clone()).collect())
}

/// Step 5: the classifier, and the key store when one was chosen now.
fn classifier_step(
    args: &SetupArgs,
    p: &mut Prompter,
    current: Option<&ProviderConfig>,
) -> Result<(ProviderConfig, Option<KeyStore>)> {
    let key_flag = [
        (args.key_store.is_some(), "--key-store"),
        (args.key_command.is_some(), "--key-command"),
        (args.key_env.is_some(), "--key-env"),
        (args.key_stored, "--key-stored"),
    ]
    .into_iter()
    .find_map(|(given, flag)| given.then_some(flag));
    if let (Some(flag), Some(kind)) = (key_flag, args.provider.as_deref()) {
        if provider::provider_for(kind).is_ok_and(|p| p.key_account().is_none()) {
            return Err(err(
                2,
                format!("{STEP_KEY}: {flag} has no effect with --provider {kind}; drop it"),
            ));
        }
    }
    let flags = args.provider.is_some() || args.model.is_some() || key_flag.is_some();
    if let Some(current) = current.filter(|_| !flags) {
        let keep = !p.enabled()
            || p.confirm(
                &format!("Keep the current classifier ({})?", describe(current)),
                true,
            )?;
        if keep {
            return Ok((current.clone(), None));
        }
    }
    let kind = match args.provider.as_deref() {
        Some(kind) => kind.to_owned(),
        None if p.enabled() => {
            let options: Vec<String> = provider::KINDS
                .iter()
                .map(|&kind| registered(kind).label().to_owned())
                .collect();
            provider::KINDS[p.choose("Which classifier?", &options, 0)?].to_owned()
        }
        None => provider::KINDS[0].to_owned(),
    };
    let adapter = provider::provider_for(&kind).map_err(|e| {
        err(
            2,
            format!(
                "{STEP_CLASSIFIER}: {e}; use --provider {}",
                provider::KINDS.join(" or ")
            ),
        )
    })?;
    // A provider without a key has nothing to ask (the offline demo).
    let Some(account) = adapter.key_account() else {
        return Ok((adapter.new_config(), None));
    };
    // A current provider of this kind changes only where asked, so its
    // generation hash, and with it every classification, stays put.
    let current = current.filter(|c| c.kind == kind);
    let mut provider = current.cloned().unwrap_or_else(|| adapter.new_config());
    provider.model = answer(
        p,
        STEP_CLASSIFIER,
        args.model.as_deref(),
        "--model",
        "Model",
        Some(&provider.model),
        nonempty,
    )?;
    // A current key stays where it is unless a key flag moves it: `--model`
    // or `--provider` alone never switch its source.
    if flags && key_flag.is_none() && current.is_some() {
        return Ok((provider, None));
    }
    let default_env = adapter.new_config().api_key_env;
    let current = current.map(|c| current_store(c, account));
    let store = key_step(args, p, &mut provider, current, account, &default_env)?;
    Ok((provider, Some(store)))
}

/// A kind from `provider::KINDS`, which `provider_for` always knows.
fn registered(kind: &str) -> &'static dyn provider::DecisionProvider {
    provider::provider_for(kind).expect("every kind in KINDS is registered")
}

/// Where a current provider gets its key: the tool-backed store whose read
/// command for `account` it runs, another command, or the environment.
fn current_store(provider: &ProviderConfig, account: &str) -> KeyStore {
    match &provider.api_key_command {
        Some(command) => secrets::store_of(command, account).unwrap_or(KeyStore::Command),
        None => KeyStore::Env,
    }
}

fn describe(provider: &ProviderConfig) -> String {
    match (provider.kind.as_str(), &provider.api_key_command) {
        ("fake", _) => "offline demo".to_owned(),
        (_, Some(_)) => format!("{}, key from a key command", provider.model),
        (_, None) => format!("{}, key from ${}", provider.model, provider.api_key_env),
    }
}

/// The store from `--key-store`, implied by `--key-command`/`--key-env`,
/// or chosen from the menu (first option without prompts). The menu's
/// default is the `current` store when it is offered (a command whose
/// store is not offered defaults to `command`), else the first option.
fn chosen_store(args: &SetupArgs, p: &mut Prompter, current: Option<KeyStore>) -> Result<KeyStore> {
    let implied =
        match (args.key_command.is_some(), args.key_env.is_some()) {
            (true, true) => return Err(err(
                2,
                "step 5 (key): --key-command and --key-env exclude each other; pass one of them",
            )),
            (true, false) => Some(KeyStore::Command),
            (false, true) => Some(KeyStore::Env),
            (false, false) => None,
        };
    match (args.key_store, implied) {
        (Some(store), Some(other)) if store != other => Err(err(
            2,
            format!(
                "step 5 (key): --key-store {} conflicts with {}; drop one of them",
                store.flag(),
                if other == KeyStore::Command {
                    "--key-command"
                } else {
                    "--key-env"
                }
            ),
        )),
        (Some(store), _) | (None, Some(store)) => Ok(store),
        (None, None) => {
            let options = secrets::key_store_options(cfg!(target_os = "macos"), |tool| {
                process::find_on_path(tool).is_some()
            });
            if !p.enabled() {
                return Ok(options[0]);
            }
            let offered = |store| options.iter().position(|&o| o == store);
            let default = current
                .and_then(|store| offered(store).or_else(|| offered(KeyStore::Command)))
                .unwrap_or(0);
            let labels: Vec<String> = options.iter().map(|o| o.label().to_owned()).collect();
            Ok(options[p.choose(
                "Where should mailtriage get the OpenRouter key?",
                &labels,
                default,
            )?])
        }
    }
}

/// How the provider gets the key. Setup never reads the key except to
/// confirm that a command prints one. `current` is the store of the
/// provider being changed, if any; `account` is its key store account and
/// `default_env` the variable a new config of its kind names.
fn key_step(
    args: &SetupArgs,
    p: &mut Prompter,
    provider: &mut ProviderConfig,
    current: Option<KeyStore>,
    account: &str,
    default_env: &str,
) -> Result<KeyStore> {
    let store = chosen_store(args, p, current)?;
    match store {
        KeyStore::Keychain | KeyStore::SecretService | KeyStore::Pass => {
            let tool_name = store.tool().expect("tool-backed store");
            let tool = process::find_on_path(tool_name).ok_or_else(|| {
                err(
                    2,
                    format!(
                        "step 5 (key): --key-store {}: {tool_name} is not on PATH",
                        store.flag()
                    ),
                )
            })?;
            let read = secrets::read_command(store, &tool, account).expect("tool-backed store");
            let save = secrets::store_command(store, &tool, account).expect("tool-backed store");
            let stored = secrets::run_key_command(&read).is_ok();
            let reuse = stored
                && (args.key_stored
                    || !p.enabled()
                    || p.confirm(
                        &format!("A key is already stored in {}. Use it?", store.label()),
                        true,
                    )?);
            if !reuse {
                if args.key_stored {
                    return Err(err(
                        3,
                        format!(
                            "step 5 (key): no key found in {}; store it with `{}`",
                            store.label(),
                            shell_line(&save)
                        ),
                    ));
                }
                if !p.enabled() && !args.terminal {
                    return Err(err(
                        2,
                        format!(
                            "step 5 (key): storing the key needs a terminal; run `{}`, then pass --key-stored, or use --key-store env",
                            shell_line(&save)
                        ),
                    ));
                }
                p.say(&format!(
                    "{tool_name} will now ask for your OpenRouter key; mailtriage never sees it."
                ));
                // The tool prompts on the terminal; its stdout is not shown.
                let status = Command::new(&save[0])
                    .args(&save[1..])
                    .stdout(Stdio::null())
                    .status();
                if !status.is_ok_and(|s| s.success()) {
                    return Err(err(
                        3,
                        format!("step 5 (key): `{}` failed", shell_line(&save)),
                    ));
                }
                secrets::run_key_command(&read).map_err(|e| {
                    err(
                        3,
                        format!(
                            "step 5 (key): {e} after storing it; check `{}`",
                            shell_line(&read)
                        ),
                    )
                })?;
            }
            p.say(&format!("The key is in {}.", store.label()));
            provider.api_key_command = Some(read);
        }
        KeyStore::Command => loop {
            let text = answer(
                p,
                STEP_KEY,
                args.key_command.as_deref(),
                "--key-command",
                "Command that prints the key (run with /bin/sh -c)",
                None,
                nonempty,
            )?;
            let command = vec!["/bin/sh".to_owned(), "-c".to_owned(), text];
            match secrets::run_key_command(&command) {
                Ok(_) => {
                    p.say("The key command printed a key.");
                    provider.api_key_command = Some(command);
                    break;
                }
                Err(e) if args.key_command.is_none() && p.enabled() => {
                    p.say(&format!("  {e}. Try another command."))
                }
                Err(e) => return Err(err(3, format!("step 5 (key): {e}; check --key-command"))),
            }
        },
        KeyStore::Env => {
            let current = if provider.api_key_env.is_empty() {
                default_env.to_owned()
            } else {
                provider.api_key_env.clone()
            };
            let name = answer(
                p,
                STEP_KEY,
                args.key_env.as_deref(),
                "--key-env",
                "Environment variable that holds the key",
                Some(&current),
                |n| {
                    if provider::valid_env_name(n) {
                        Ok(n.to_owned())
                    } else {
                        Err("Use upper-case letters, digits and _.".to_owned())
                    }
                },
            )?;
            if !std::env::var(&name).is_ok_and(|v| !v.trim().is_empty()) {
                p.say(&format!(
                    "{name} is not set here; set it wherever mailtriage runs."
                ));
            }
            provider.api_key_env = name;
            provider.api_key_command = None;
        }
    }
    Ok(store)
}

fn default_categories() -> Vec<Category> {
    let mut categories = config::default_config()
        .accounts
        .remove("work")
        .map(|a| a.categories)
        .unwrap_or_default();
    for category in &mut categories {
        category.folder = Some(category.name.clone());
    }
    categories
}

fn categories_hint(config: Option<&Path>, name: &str, categories: &[Category]) -> String {
    let names: Vec<&str> = categories.iter().map(|c| c.name.as_str()).collect();
    let file = ["--account", name, "--file", "categories.json"];
    format!(
        "Categories: {}. Each files into a folder of the same name. To change them: `{} > categories.json`, edit the file, `{}`, then `{}`.",
        names.join(", "),
        mailtriage_line(config, &["categories", "export"], &["--account", name]),
        mailtriage_line(config, &["categories", "validate"], &file),
        mailtriage_line(config, &["categories", "apply"], &file),
    )
}

/// Step 7: `off` or `dry_run`; a `live` account stays live. Filing needs
/// source folders it can name safely.
fn filing_step(
    args: &SetupArgs,
    p: &mut Prompter,
    previous: Option<FilingMode>,
    mailboxes: &[String],
) -> Result<FilingMode> {
    let blocked = mailboxes.iter().find(|m| config::unsafe_source_mailbox(m));
    let refuse = |mode: FilingMode| -> Result<()> {
        match blocked {
            Some(m) if mode != FilingMode::Off => Err(err(
                2,
                format!(
                    "step 7 (filing): folder {m:?} must be {} while filing is on; watch other folders or pass --filing off",
                    config::SOURCE_RULE
                ),
            )),
            _ => Ok(()),
        }
    };
    if let Some(mode) = args.filing {
        refuse(mode)?;
        return Ok(mode);
    }
    if previous == Some(FilingMode::Live) {
        refuse(FilingMode::Live)?;
        p.say("Filing stays live.");
        return Ok(FilingMode::Live);
    }
    let default = previous.unwrap_or(FilingMode::DryRun);
    if !p.enabled() {
        refuse(default)?;
        return Ok(default);
    }
    let options = [
        "Dry run: plan the moves and show them, change nothing (recommended)",
        "Off: classify only",
    ]
    .map(String::from);
    loop {
        let pick = p.choose(
            "File mail into one IMAP folder per category?",
            &options,
            usize::from(default == FilingMode::Off),
        )?;
        let mode = [FilingMode::DryRun, FilingMode::Off][pick];
        match refuse(mode) {
            Ok(()) => return Ok(mode),
            Err(e) => p.say(&format!("  {}", error_text(&e))),
        }
    }
}

/// Step 9: `doctor`, each item with the one command that fixes it.
fn doctor_step(
    p: &mut Prompter,
    path: &Path,
    shown: Option<&Path>,
    name: &str,
    cfg: &AppConfig,
    engine: &HimalayaConfig,
) -> Value {
    let provider = &cfg.provider;
    let mut items = Vec::new();
    match Service::open(path).and_then(|mut service| service.doctor(name)) {
        Err(e) => items.push(check_item(
            "state",
            false,
            Some(error_text(&e)),
            format!("check {} or choose another --account", path.display()),
        )),
        Ok(report) => {
            let key = &report["provider"];
            items.push(check_item(
                "provider",
                key["configuration_valid"] == true,
                None,
                format!("check the provider block in {}", path.display()),
            ));
            // Asking for the store keeps setup from keeping the broken key
            // along with the classifier.
            let key_fix = if key["key_source"] == "command" {
                let store = provider
                    .api_key_command
                    .as_deref()
                    .zip(provider::key_account(provider))
                    .and_then(|(command, account)| secrets::store_of(command, account))
                    .unwrap_or(KeyStore::Command);
                let setup = mailtriage_line(
                    shown,
                    &["setup", "--update"],
                    &["--account", name, "--key-store", store.flag()],
                );
                if store == KeyStore::Command {
                    format!("run `{setup}` and give a command that prints the key")
                } else {
                    format!("run `{setup}` to store the key again")
                }
            } else {
                format!(
                    "export {}=<your OpenRouter key> where mailtriage runs",
                    provider.api_key_env
                )
            };
            items.push(check_item(
                "key",
                key["key_present"] == true,
                key["key_error"].as_str().map(str::to_owned),
                key_fix,
            ));
            let transport = &report["transport"];
            let (error, fix) = if transport["tested"] == false {
                (
                    transport["error"].as_str().map(str::to_owned),
                    himalaya_install_fix(shown, &["--account", name]),
                )
            } else {
                let check = shell_line(&[
                    engine.binary.as_os_str(),
                    OsStr::new("--config"),
                    engine.config.as_os_str(),
                    OsStr::new("--account"),
                    OsStr::new(&engine.account),
                    OsStr::new("account"),
                    OsStr::new("check"),
                ]);
                (None, format!("run `{check}`"))
            };
            items.push(check_item("mail", transport["ready"] == true, error, fix));
            if let Some(filing) = report.get("filing") {
                let problems = filing["problems"].as_array().map_or(0, Vec::len);
                items.push(check_item(
                    "filing",
                    problems == 0,
                    None,
                    format!(
                        "run `{}`",
                        mailtriage_line(shown, &["filing", "status"], &["--account", name])
                    ),
                ));
            }
        }
    }
    // Like doctor's top-level `ready`, setup's ignores the update item.
    let ready = items.iter().all(|i| i["ready"] == true);
    let unit = crate::update::report::unit_of(name);
    let update = crate::update::report::doctor_block(cfg.updates, unit);
    if update["ready"] == false {
        items.push(check_item(
            "update",
            false,
            update["reason"].as_str().map(str::to_owned),
            update["fix"].as_str().unwrap_or_default().to_owned(),
        ));
    }
    p.say("Checks:");
    for item in &items {
        let check = item["check"].as_str().unwrap_or_default();
        match item["fix"].as_str() {
            None => p.say(&format!("  ok         {check}")),
            Some(fix) => p.say(&format!("  not ready  {check}: {fix}")),
        }
    }
    json!({"ready": ready, "items": items})
}

/// One doctor item; `error` and `fix` only when it is not ready.
fn check_item(check: &str, ready: bool, error: Option<String>, fix: String) -> Value {
    let mut item = json!({"check": check, "ready": ready});
    if !ready {
        if let Some(error) = error {
            item["error"] = json!(error);
        }
        item["fix"] = json!(fix);
    }
    item
}

/// What to run next. With the service installed, `sync` and `watch` would
/// compete with it for the account lock, so they are not suggested.
fn next_steps(
    p: &mut Prompter,
    shown: Option<&Path>,
    name: &str,
    mode: FilingMode,
    installed: bool,
) {
    let account = ["--account", name];
    p.say("");
    if installed {
        p.say(&format!(
            "Next: the background service runs `watch`. `{}` shows how it runs; `{}` lists the mail that needs attention.",
            mailtriage_line(shown, &["service", "status"], &account),
            mailtriage_line(shown, &["list"], &account),
        ));
    } else {
        p.say(&format!(
            "Next: `{}` classifies new mail; `{}` keeps doing it.",
            mailtriage_line(shown, &["sync"], &account),
            mailtriage_line(shown, &["watch"], &account),
        ));
    }
    if mode == FilingMode::DryRun {
        p.say(&format!(
            "Filing is a dry run: `{}` shows what would move.",
            mailtriage_line(shown, &["filing", "plan"], &account),
        ));
        p.say(&format!(
            "Go live only after the provider check ({PROVIDER_CHECK_URL}): `{}`.",
            mailtriage_line(
                shown,
                &["filing", "enable"],
                &["--account", name, "--mode", "live"]
            ),
        ));
    }
}

/// Step 10: the background service. Asked with prompts (default yes, but
/// no for a key from an environment variable, which the service does not
/// inherit); without prompts only `--service install` installs it. Never
/// installed when `state_ok` is false.
fn service_step(
    args: &SetupArgs,
    p: &mut Prompter,
    path: &Path,
    shown: Option<&Path>,
    name: &str,
    provider: &ProviderConfig,
    state_ok: bool,
) -> Result<Value> {
    let retry = install_command(args, path, name);
    if !state_ok {
        if args.service == Some(true) || (args.service.is_none() && p.enabled()) {
            p.say(&format!(
                "Not installing the background service: the state check failed, so every pass would fail. Fix it, then run `{retry}`."
            ));
        }
        return Ok(Value::Null);
    }
    let wanted = match args.service {
        Some(wanted) => wanted,
        None if p.enabled() => match Context::detect() {
            Ok(_) => {
                let env_key =
                    provider::key_account(provider).is_some() && provider.api_key_command.is_none();
                if env_key {
                    // The store a fresh setup would offer first: the
                    // platform's own when its tool is here, else a command.
                    let store = secrets::key_store_options(cfg!(target_os = "macos"), |tool| {
                        process::find_on_path(tool).is_some()
                    })[0];
                    let store = mailtriage_line(
                        shown,
                        &["setup", "--update"],
                        &["--account", name, "--key-store", store.flag()],
                    );
                    p.say(&format!(
                        "The key comes from {}, which the background service does not inherit, so the default is no. Store the key first: `{store}`.",
                        provider.api_key_env
                    ));
                }
                p.confirm(
                    &format!(
                        "Run mailtriage in the background now (`watch` every {} seconds)?",
                        args.interval_seconds
                    ),
                    !env_key,
                )?
            }
            Err(e) => {
                p.say(&format!(
                    "Skipping the background service: {}.",
                    error_text(&e)
                ));
                false
            }
        },
        None => false,
    };
    if !wanted {
        return Ok(Value::Null);
    }
    let installed = Context::detect().and_then(|ctx| {
        let _lock = service_control::lock(&ctx, name, service_control::SERVICE_LOCK_WAIT)?;
        let service = Service::open(path)?;
        let out = system_service::install_account(
            &service,
            path,
            name,
            args.interval_seconds,
            args.limit,
            &ctx,
        )?;
        Ok((ctx, out))
    });
    // The config is already written, so the fix is the service command
    // alone; the exit code is the original error's.
    let (ctx, out) = installed.map_err(|e| {
        err(
            service::exit_code(&e),
            format!(
                "{STEP_SERVICE}: {}; the config is written: {}",
                error_text(&e),
                service_fix(SERVICE_SUPPORTED, &retry)
            ),
        )
    })?;
    p.say(&format!(
        "Installed the background service ({}).",
        ctx.manager.name()
    ));
    if let Some(note) = out["note"].as_str() {
        p.say(note);
    }
    if ctx.manager == Manager::Systemd {
        p.say("To keep it running while you are logged out: loginctl enable-linger $USER");
    }
    Ok(out)
}

/// Whether this platform has a service manager setup can use.
const SERVICE_SUPPORTED: bool = cfg!(any(target_os = "macos", target_os = "linux"));

/// What fixes a failed step 10 once the config is written: the service
/// command alone, or, where no service is supported, nothing to retry.
fn service_fix(supported: bool, retry: &str) -> String {
    if supported {
        format!("fix this, then run `{retry}`")
    } else {
        "drop --service install".to_owned()
    }
}

/// `mailtriage service install` for the written config, `name` and this
/// run's interval and limit.
fn install_command(args: &SetupArgs, path: &Path, name: &str) -> String {
    let interval = args.interval_seconds.to_string();
    let limit = args.limit.to_string();
    mailtriage_line(
        Some(path),
        &["service", "install"],
        &[
            "--account",
            name,
            "--interval-seconds",
            &interval,
            "--limit",
            &limit,
        ],
    )
}

/// The config that printed `mailtriage` commands must pass: `None` when
/// commands run here without `--config` find `path` (canonical). Warns
/// once when a `./mailtriage.json` in the working directory shadows it.
fn printed_config(p: &mut Prompter, path: &Path) -> Option<PathBuf> {
    let shown = command_config(path)?;
    let Ok(cwd) = std::env::current_dir() else {
        return Some(shown);
    };
    let local = cwd.join(config::CONFIG_FILE);
    let env = std::env::var_os("MAILTRIAGE_CONFIG");
    if env.as_deref().is_none_or(OsStr::is_empty) && local.exists() {
        p.say(&format!(
            "Warning: {} takes precedence over {} for commands run in {} without --config, so the commands below pass --config.",
            local.display(),
            path.display(),
            cwd.display()
        ));
    }
    Some(shown)
}

/// `printed_config` without the warning, also for a config not written
/// yet: `None` when commands run here without `--config` find `path`, else
/// `path` as it is or will be once written (see [`settled`]).
fn command_config(path: &Path) -> Option<PathBuf> {
    let path = settled(path);
    let Ok(cwd) = std::env::current_dir() else {
        return Some(path);
    };
    let env = std::env::var_os("MAILTRIAGE_CONFIG");
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let found = config::resolve_path(None, env.as_deref(), &cwd, home.as_deref())
        .ok()
        .map(|found| settled(&found));
    (found.as_deref() != Some(path.as_path())).then_some(path)
}

/// `path` made absolute, with its deepest existing ancestor resolved by
/// `fs::canonicalize`: its canonical path once the rest is created. An
/// existing path is simply canonical.
fn settled(path: &Path) -> PathBuf {
    let Ok(path) = std::path::absolute(path) else {
        return path.to_owned();
    };
    let mut rest = Vec::new();
    let mut base = path.as_path();
    loop {
        if let Ok(real) = fs::canonicalize(base) {
            return rest.iter().rev().fold(real, |p, name| p.join(name));
        }
        match (base.parent(), base.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                base = parent;
            }
            _ => return path.clone(),
        }
    }
}

/// A `mailtriage` command for messages: the subcommand `words`, then
/// `--config` when `config` is given, then `args`.
fn mailtriage_line(config: Option<&Path>, words: &[&str], args: &[&str]) -> String {
    let mut line: Vec<&OsStr> = vec![OsStr::new("mailtriage")];
    line.extend(words.iter().map(OsStr::new));
    if let Some(config) = config {
        line.extend([OsStr::new("--config"), config.as_os_str()]);
    }
    line.extend(args.iter().map(OsStr::new));
    shell_line(&line)
}

fn key_source_value(provider: &ProviderConfig) -> Value {
    if provider::key_account(provider).is_none() {
        Value::Null
    } else {
        json!(secrets::key_source(provider))
    }
}

/// A flag's value; else the prompt's answer; else the default; without
/// prompts and without a default, exit 2 naming `step` and the flag.
fn answer(
    p: &mut Prompter,
    step: &str,
    flag: Option<&str>,
    flag_name: &str,
    question: &str,
    default: Option<&str>,
    check: impl Fn(&str) -> Result<String, String>,
) -> Result<String> {
    let invalid = |reason: String| err(2, format!("{step}: {flag_name}: {reason}"));
    if let Some(value) = flag {
        return check(value).map_err(invalid);
    }
    if p.enabled() {
        return p.ask(question, default, check);
    }
    match default {
        Some(value) => check(value).map_err(invalid),
        None => Err(err(
            2,
            format!("{step}: {flag_name} is required without prompts"),
        )),
    }
}

fn nonempty(value: &str) -> Result<String, String> {
    if value.trim().is_empty() {
        Err("This cannot be empty.".to_owned())
    } else {
        Ok(value.trim().to_owned())
    }
}

/// A step 2 path given by `flag` (or `HIMALAYA_CONFIG`), made absolute.
fn absolute(path: &Path, flag: &str) -> Result<PathBuf> {
    std::path::absolute(path).map_err(|_| {
        err(
            2,
            format!(
                "step 2 (Himalaya): {flag}: cannot resolve {}; pass an absolute path",
                path.display()
            ),
        )
    })
}

fn error_text(error: &anyhow::Error) -> String {
    error
        .downcast_ref::<ServiceError>()
        .map_or_else(|| "operation failed".to_owned(), |e| e.message.clone())
}

/// A command line for messages, quoting words the shell would split.
pub(crate) fn shell_line<S: AsRef<OsStr>>(words: &[S]) -> String {
    words
        .iter()
        .map(|word| {
            let word = word.as_ref().to_string_lossy();
            if !word.is_empty()
                && word
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./=:@+,".contains(c))
            {
                word.into_owned()
            } else {
                format!("'{}'", word.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_zone_defaults() {
        assert_eq!(
            default_timezone(Some("Europe/Berlin"), None),
            "Europe/Berlin"
        );
        assert_eq!(
            default_timezone(Some(":America/New_York"), None),
            "America/New_York"
        );
        assert_eq!(
            default_timezone(
                Some("/etc/localtime"),
                Some(Path::new("/var/db/timezone/zoneinfo/Europe/Vienna"))
            ),
            "Europe/Vienna"
        );
        assert_eq!(
            default_timezone(None, Some(Path::new("/usr/share/zoneinfo/Asia/Tokyo"))),
            "Asia/Tokyo"
        );
        assert_eq!(
            default_timezone(Some(""), Some(Path::new("/etc/other"))),
            "UTC"
        );
        assert_eq!(default_timezone(None, None), "UTC");
    }

    #[test]
    fn himalaya_config_search_order() {
        let home = Path::new("/h");
        assert_eq!(
            himalaya_config_candidates(home, Some(Path::new("/x")), true),
            vec![
                PathBuf::from("/h/Library/Application Support/himalaya/config.toml"),
                PathBuf::from("/h/.config/himalaya/config.toml"),
                PathBuf::from("/h/.himalayarc"),
            ]
        );
        assert_eq!(
            himalaya_config_candidates(home, Some(Path::new("/x")), false),
            vec![
                PathBuf::from("/x/himalaya/config.toml"),
                PathBuf::from("/h/.config/himalaya/config.toml"),
                PathBuf::from("/h/.himalayarc"),
            ]
        );
        assert_eq!(
            himalaya_config_candidates(home, Some(Path::new("relative")), false),
            vec![
                PathBuf::from("/h/.config/himalaya/config.toml"),
                PathBuf::from("/h/.himalayarc"),
            ]
        );
    }

    /// Final review I3: where no service manager is supported, retrying the
    /// install cannot help.
    #[test]
    fn a_failed_service_step_names_its_fix() {
        let retry = "mailtriage service install --config /c.json --account work";
        assert_eq!(
            service_fix(true, retry),
            format!("fix this, then run `{retry}`")
        );
        assert_eq!(service_fix(false, retry), "drop --service install");
    }

    #[test]
    fn account_names_fit_service_and_file_names() {
        assert!(config::valid_account_name("work-2_b"));
        assert!(config::valid_account_name(&"a".repeat(64)));
        assert!(!config::valid_account_name(&"a".repeat(65)));
        for bad in ["", "my work", "a.b", "ä", "a/b"] {
            assert!(!config::valid_account_name(bad), "{bad}");
        }
    }
}
