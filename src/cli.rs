use clap::{ArgGroup, Args, Parser, Subcommand};
use mailtriage::{
    config,
    domain::{Category, FilingMode, UpdateMode},
    engine::ConfigChanged,
    prompt::{self, Prompter},
    secrets::KeyStore,
    service::{
        is_config_change, Backfill, ErrorKind, ListOptions, RetryTarget, Service, ServiceError,
    },
    setup,
    system_service::{self, Context},
    update,
};
use serde_json::{json, Value};
use std::{
    fs,
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

#[derive(Parser)]
#[command(
    name = "mailtriage",
    version,
    about = "Local mail classification and attention queries"
)]
struct Cli {
    /// Config file (default: MAILTRIAGE_CONFIG, ./mailtriage.json if present, else ~/.config/mailtriage/mailtriage.json).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

impl Cli {
    /// The config path for every command except `init` and `setup`.
    fn config_path(&self) -> Result<PathBuf, CliError> {
        let cwd = std::env::current_dir()
            .map_err(|_| CliError::input("cannot read the working directory; pass --config"))?;
        let home = std::env::var_os("HOME").map(PathBuf::from);
        config::resolve_path(
            self.config.as_deref(),
            std::env::var_os("MAILTRIAGE_CONFIG").as_deref(),
            &cwd,
            home.as_deref(),
        )
        .map_err(|e| CliError::input(e.to_string()))
    }
}

#[derive(Subcommand)]
enum Command {
    Init,
    /// Guided first run: mail account, folders, classifier and key, filing.
    Setup(Box<SetupArg>),
    Doctor(AccountArg),
    Classify(ClassifyArg),
    Sync(ScanArg),
    Watch(WatchArg),
    List(ListArg),
    Read(IdArg),
    Correct(CorrectArg),
    Done(IdArg),
    Reopen(IdArg),
    Categories {
        #[command(subcommand)]
        command: CategoriesCommand,
    },
    Reclassify(ReclassifyArg),
    Export(AccountArg),
    /// File classified mail into per-category IMAP folders.
    Filing {
        #[command(subcommand)]
        command: FilingCommand,
    },
    /// Run `watch` in the background (launchd on macOS, systemd on Linux).
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    /// Install the newest stable release from GitHub; needs no config.
    Update(UpdateArg),
}

#[derive(Args)]
struct UpdateArg {
    /// Only report whether an update is available; install nothing.
    #[arg(long)]
    check: bool,
}

#[derive(Args)]
struct SetupArg {
    /// Change an existing config (required without prompts).
    #[arg(long)]
    update: bool,
    /// No prompts: flags and defaults only.
    #[arg(long, conflicts_with = "interactive")]
    yes: bool,
    /// Prompt even when stdin is not a terminal.
    #[arg(long)]
    interactive: bool,
    #[arg(long)]
    himalaya_binary: Option<PathBuf>,
    #[arg(long)]
    himalaya_config: Option<PathBuf>,
    #[arg(long)]
    himalaya_account: Option<String>,
    /// Account name in mailtriage (letters, digits, - and _).
    #[arg(long)]
    account: Option<String>,
    #[arg(long)]
    identity: Option<String>,
    #[arg(long)]
    timezone: Option<String>,
    /// One line about the recipient that helps classification.
    #[arg(long)]
    brief: Option<String>,
    /// A folder to watch; repeat for several.
    #[arg(long = "mailbox")]
    mailboxes: Vec<String>,
    #[arg(long, value_parser = ["openrouter", "fake"])]
    provider: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long, value_parser = KeyStore::ALL.map(KeyStore::flag))]
    key_store: Option<String>,
    /// Shell command that prints the key (stored as /bin/sh -c).
    #[arg(long)]
    key_command: Option<String>,
    /// Environment variable that holds the key.
    #[arg(long)]
    key_env: Option<String>,
    /// The key is already in the chosen store.
    #[arg(long)]
    key_stored: bool,
    #[arg(long, value_parser = ["off", "dry-run"])]
    filing: Option<String>,
    /// Install the background service at the end (default: ask; skip without prompts).
    #[arg(long, value_parser = ["install", "skip"])]
    service: Option<String>,
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..=86400))]
    interval_seconds: u64,
    #[arg(long, default_value_t = 100, value_parser = parse_limit)]
    limit: usize,
    /// What `watch` does about new releases (default: keep the config's value; auto for a new config).
    #[arg(long, value_parser = ["auto", "notify", "off"])]
    updates: Option<String>,
}

impl SetupArg {
    fn to_args(&self, terminal: bool) -> setup::SetupArgs {
        setup::SetupArgs {
            update: self.update,
            terminal,
            himalaya_binary: self.himalaya_binary.clone(),
            himalaya_config: self.himalaya_config.clone(),
            himalaya_account: self.himalaya_account.clone(),
            account: self.account.clone(),
            identity: self.identity.clone(),
            timezone: self.timezone.clone(),
            brief: self.brief.clone(),
            mailboxes: self.mailboxes.clone(),
            provider: self.provider.clone(),
            model: self.model.clone(),
            key_store: self.key_store.as_deref().and_then(KeyStore::from_flag),
            key_command: self.key_command.clone(),
            key_env: self.key_env.clone(),
            key_stored: self.key_stored,
            filing: self.filing.as_deref().map(|f| {
                if f == "off" {
                    FilingMode::Off
                } else {
                    FilingMode::DryRun
                }
            }),
            service: self.service.as_deref().map(|s| s == "install"),
            interval_seconds: self.interval_seconds,
            limit: self.limit,
            updates: self.updates.as_deref().and_then(UpdateMode::parse),
        }
    }
}

#[derive(Args)]
struct AccountArg {
    #[arg(long)]
    account: String,
}

#[derive(Args)]
struct IdArg {
    #[arg(long)]
    account: String,
    #[arg(long)]
    id: String,
}

#[derive(Args)]
struct ClassifyArg {
    #[arg(long)]
    account: String,
    /// Input path, or - to read from stdin.
    #[arg(long)]
    input: PathBuf,
    #[arg(long, value_parser = ["rfc822", "json"])]
    format: String,
}

#[derive(Args)]
struct ScanArg {
    #[arg(long)]
    account: String,
    #[arg(long, default_value_t = 100, value_parser = parse_limit)]
    limit: usize,
}

#[derive(Args)]
struct WatchArg {
    #[arg(long)]
    account: String,
    #[arg(long, default_value_t = 100, value_parser = parse_limit)]
    limit: usize,
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..=86400))]
    interval_seconds: u64,
}

#[derive(Args)]
struct ListArg {
    #[arg(long)]
    account: String,
    #[arg(long, default_value = "attention", value_parser = ["attention", "all"])]
    view: String,
    #[arg(long)]
    category: Option<String>,
    #[arg(long, value_parser = ["low", "medium", "high"])]
    urgency: Option<String>,
    #[arg(long)]
    action_required: Option<bool>,
    #[arg(long, default_value_t = 50, value_parser = parse_limit)]
    limit: usize,
    #[arg(long)]
    cursor: Option<String>,
}

#[derive(Args)]
struct CorrectArg {
    #[arg(long)]
    account: String,
    #[arg(long)]
    id: String,
    #[arg(long, value_parser = ["low", "medium", "high"])]
    urgency: Option<String>,
    #[arg(long)]
    category: Option<String>,
    #[arg(long)]
    action_required: Option<bool>,
    #[arg(long, value_parser = ["urgency", "category", "category_id", "action_required"])]
    clear: Option<String>,
}

#[derive(Subcommand)]
enum CategoriesCommand {
    Export(AccountArg),
    Validate {
        #[arg(long)]
        file: PathBuf,
        /// Validate against this account's configuration, including the
        /// folder rules when its filing is on.
        #[arg(long)]
        account: Option<String>,
    },
    Apply {
        #[arg(long)]
        account: String,
        #[arg(long)]
        file: PathBuf,
    },
}

#[derive(Args)]
struct ReclassifyArg {
    #[arg(long)]
    account: String,
    #[arg(long)]
    since: Option<String>,
    #[arg(long)]
    dry_run: bool,
    #[arg(long, default_value_t = 100, value_parser = parse_limit)]
    limit: usize,
}

#[derive(Subcommand)]
enum FilingCommand {
    /// Mode, folders, intents, blocks and the last pass; no mailbox access.
    Status(AccountArg),
    /// Give every category a folder, validate, and set the filing mode.
    Enable(EnableArg),
    /// Set the filing mode to off.
    Disable(AccountArg),
    /// Preview what the next pass would create, move and flag.
    Plan(PlanArg),
    /// Make existing inbox mail eligible for filing once.
    Backfill(BackfillArg),
    /// Keep a message in its source folder.
    Pin(IdArg),
    /// Let automatic filing apply to a pinned message again, once.
    Unpin(IdArg),
    /// Lift a message block, a folder pause, or requeue an unresolved arrival.
    Retry(RetryArg),
    /// Mark a reviewed unresolved arrival dismissed.
    Dismiss(ArrivalArg),
    /// Confirm an existing folder whose role could not be verified.
    Adopt(FolderArg),
    /// Recent filing events, newest first.
    Log(LogArg),
}

#[derive(Subcommand)]
enum ServiceCommand {
    /// Install and start `watch` for an account.
    Install(ServiceInstallArg),
    /// Stop and remove the account's service.
    Uninstall(AccountArg),
    /// Whether the service is installed and running, and the last sync pass.
    Status(AccountArg),
}

#[derive(Args)]
struct ServiceInstallArg {
    #[arg(long)]
    account: String,
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..=86400))]
    interval_seconds: u64,
    #[arg(long, default_value_t = 100, value_parser = parse_limit)]
    limit: usize,
}

#[derive(Args)]
struct EnableArg {
    #[arg(long)]
    account: String,
    #[arg(long, value_parser = ["dry-run", "live"])]
    mode: String,
}

#[derive(Args)]
struct PlanArg {
    #[arg(long)]
    account: String,
    #[arg(long, default_value_t = 50, value_parser = parse_limit)]
    limit: usize,
}

#[derive(Args)]
#[command(group(ArgGroup::new("scope").required(true).args(["days", "all"])))]
struct BackfillArg {
    #[arg(long)]
    account: String,
    /// Mail received within the last N days (1..=3650).
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=3650))]
    days: Option<u32>,
    /// All mail in the source folders.
    #[arg(long)]
    all: bool,
    /// Make the matched mail eligible (requires filing mode live).
    #[arg(long)]
    apply: bool,
}

#[derive(Args)]
#[command(group(ArgGroup::new("target").required(true).args(["id", "folder", "arrival"])))]
struct RetryArg {
    #[arg(long)]
    account: String,
    #[arg(long)]
    id: Option<String>,
    #[arg(long)]
    folder: Option<String>,
    #[arg(long, value_parser = clap::value_parser!(i64).range(1..))]
    arrival: Option<i64>,
}

#[derive(Args)]
struct ArrivalArg {
    #[arg(long)]
    account: String,
    #[arg(long, value_parser = clap::value_parser!(i64).range(1..))]
    arrival: i64,
}

#[derive(Args)]
struct FolderArg {
    #[arg(long)]
    account: String,
    #[arg(long)]
    folder: String,
}

#[derive(Args)]
struct LogArg {
    #[arg(long)]
    account: String,
    #[arg(long)]
    id: Option<String>,
    #[arg(long, default_value_t = 50, value_parser = parse_limit)]
    limit: usize,
}

fn parse_limit(value: &str) -> Result<usize, String> {
    let limit = value
        .parse::<usize>()
        .map_err(|_| "limit must be 1..=500".to_owned())?;
    if (1..=500).contains(&limit) {
        Ok(limit)
    } else {
        Err("limit must be 1..=500".to_owned())
    }
}

struct CliError {
    code: i32,
    message: String,
    /// The machine-readable `reason` of the JSON error object, if any.
    reason: Option<&'static str>,
}
impl CliError {
    fn input(message: impl Into<String>) -> Self {
        Self {
            code: 2,
            message: message.into(),
            reason: None,
        }
    }
    fn operational() -> Self {
        Self {
            code: 3,
            message: "Operation failed; check configuration and dependency availability".into(),
            reason: None,
        }
    }
}

fn service_error(error: anyhow::Error) -> CliError {
    if let Some(error) = error.downcast_ref::<ServiceError>() {
        return CliError {
            code: error.code,
            message: error.message.clone(),
            reason: error.kind.reason(),
        };
    }
    if error.downcast_ref::<ConfigChanged>().is_some() {
        return CliError {
            code: 5,
            message: "mail engine configuration changed during operation".into(),
            reason: ErrorKind::ConfigChanged.reason(),
        };
    }
    CliError::operational()
}

fn print_value(value: &Value, json_mode: bool) -> io::Result<()> {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    if json_mode {
        serde_json::to_writer(&mut out, value)?;
    } else {
        serde_json::to_writer_pretty(&mut out, value)?;
    }
    out.write_all(b"\n")
}

/// The JSON error object; `reason` appears only when the error has one.
fn error_json(error: &CliError) -> Value {
    let mut object = json!({"code":error.code,"message":error.message});
    if let Some(reason) = error.reason {
        object["reason"] = json!(reason);
    }
    json!({"schema_version":1,"error":object})
}

fn print_error(error: &CliError, json_mode: bool) {
    if json_mode {
        let _ = print_value(&error_json(error), true);
    } else {
        eprintln!("mailtriage: {}", error.message);
    }
}

pub fn run() -> i32 {
    let args: Vec<_> = std::env::args_os().collect();
    let json_mode = args.iter().any(|a| a == "--json");
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            if json_mode {
                print_error(&CliError::input(error.to_string().trim().to_owned()), true);
            } else {
                let _ = error.print();
            }
            return error.exit_code();
        }
    };
    match execute(&cli) {
        Ok(value) => {
            let partial = value
                .get("partial")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if print_value(&value, cli.json).is_err() {
                return 3;
            }
            if partial {
                4
            } else {
                0
            }
        }
        Err(error) => {
            print_error(&error, cli.json);
            error.code
        }
    }
}

fn open(path: &Path) -> Result<Service, CliError> {
    Service::open(path).map_err(service_error)
}

fn execute(cli: &Cli) -> Result<Value, CliError> {
    match &cli.command {
        Command::Init => {
            let path = cli
                .config
                .clone()
                .unwrap_or_else(|| PathBuf::from(config::CONFIG_FILE));
            if path.exists() {
                return Err(CliError::input(format!(
                    "config already exists: {}",
                    path.display()
                )));
            }
            let mut value = config::default_config();
            value.state_dir = PathBuf::from(".state");
            let account = value
                .accounts
                .keys()
                .next()
                .cloned()
                .ok_or_else(|| CliError::input("default config has no account"))?;
            config::save(&path, &value).map_err(|_| CliError::input("could not write config"))?;
            Ok(
                json!({"schema_version":1,"config":path,"provider":"fake","account":account,"state_dir":".state"}),
            )
        }
        Command::Setup(arg) => setup(cli, arg),
        Command::Doctor(arg) => {
            let mut service = open(&cli.config_path()?)?;
            let mut report = service.doctor(&arg.account).map_err(service_error)?;
            let unit = update::report::unit_of(&arg.account);
            report["update"] = update::report::doctor_block(service.config.updates, unit);
            Ok(report)
        }
        Command::Classify(arg) => {
            let data = read_input(&arg.input, 16 * 1024 * 1024)?;
            open(&cli.config_path()?)?
                .classify(&arg.account, &data, &arg.format)
                .map_err(service_error)
        }
        Command::Sync(arg) => open(&cli.config_path()?)?
            .sync(&arg.account, arg.limit)
            .map_err(service_error),
        Command::Watch(arg) => watch(cli, arg),
        Command::List(arg) => {
            let opts = ListOptions {
                view: arg.view.clone(),
                category: arg.category.clone(),
                urgency: arg.urgency.clone(),
                action_required: arg.action_required,
                limit: arg.limit,
                cursor: arg.cursor.clone(),
            };
            open(&cli.config_path()?)?
                .list(&arg.account, opts)
                .map_err(service_error)
        }
        Command::Read(arg) => open(&cli.config_path()?)?
            .read(&arg.account, &arg.id)
            .map_err(service_error),
        Command::Correct(arg) => {
            let mut patch = serde_json::Map::new();
            if let Some(value) = &arg.urgency {
                patch.insert("urgency".into(), json!(value));
            }
            if let Some(value) = &arg.category {
                patch.insert("category_id".into(), json!(value));
            }
            if let Some(value) = arg.action_required {
                patch.insert("action_required".into(), json!(value));
            }
            if patch.is_empty() && arg.clear.is_none() {
                return Err(CliError::input("specify an override or --clear"));
            }
            let clear = arg
                .clear
                .as_deref()
                .map(|s| if s == "category" { "category_id" } else { s });
            if let Some(clear) = clear {
                if patch.contains_key(clear) {
                    return Err(CliError::input("cannot set and clear the same field"));
                }
            }
            open(&cli.config_path()?)?
                .correct(&arg.account, &arg.id, Value::Object(patch), clear)
                .map_err(service_error)
        }
        Command::Done(arg) => open(&cli.config_path()?)?
            .review(&arg.account, &arg.id, true)
            .map_err(service_error),
        Command::Reopen(arg) => open(&cli.config_path()?)?
            .review(&arg.account, &arg.id, false)
            .map_err(service_error),
        Command::Categories { command } => match command {
            CategoriesCommand::Export(arg) => open(&cli.config_path()?)?
                .categories(&arg.account)
                .map_err(service_error),
            CategoriesCommand::Validate { file, account } => {
                let categories = read_categories(file)?;
                if let Some(account) = account {
                    return open(&cli.config_path()?)?
                        .validate_categories(account, categories)
                        .map_err(service_error);
                }
                validate_categories(&categories)?;
                Ok(json!({"schema_version":1,"valid":true,"categories":categories.len()}))
            }
            CategoriesCommand::Apply { account, file } => {
                let categories = read_categories(file)?;
                open(&cli.config_path()?)?
                    .apply_categories(account, categories)
                    .map_err(service_error)
            }
        },
        Command::Reclassify(arg) => open(&cli.config_path()?)?
            .reclassify(&arg.account, arg.since.as_deref(), arg.dry_run, arg.limit)
            .map_err(service_error),
        Command::Export(arg) => open(&cli.config_path()?)?
            .export(&arg.account)
            .map_err(service_error),
        Command::Filing { command } => filing(&cli.config_path()?, command),
        Command::Service { command } => service_command(&cli.config_path()?, command),
        Command::Update(arg) => {
            update::command::run(arg.check, &update::install::EnvHooks).map_err(service_error)
        }
    }
}

fn setup(cli: &Cli, arg: &SetupArg) -> Result<Value, CliError> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let path = config::setup_path(
        cli.config.as_deref(),
        std::env::var_os("MAILTRIAGE_CONFIG").as_deref(),
        home.as_deref(),
    )
    .map_err(|e| CliError::input(format!("step 1 (config): {e}")))?;
    let terminal = io::stdin().is_terminal();
    let mut prompt = Prompter::new(
        prompt::stdin_unbuffered(),
        io::stderr(),
        !arg.yes && (arg.interactive || terminal),
    );
    setup::run(&arg.to_args(terminal), &path, &mut prompt).map_err(service_error)
}

fn filing(config: &Path, command: &FilingCommand) -> Result<Value, CliError> {
    let mut service = open(config)?;
    match command {
        FilingCommand::Status(arg) => service.filing_status(&arg.account),
        FilingCommand::Enable(arg) => {
            let mode = if arg.mode == "live" {
                FilingMode::Live
            } else {
                FilingMode::DryRun
            };
            service.filing_enable(&arg.account, mode)
        }
        FilingCommand::Disable(arg) => service.filing_disable(&arg.account),
        FilingCommand::Plan(arg) => service.filing_plan(&arg.account, arg.limit),
        FilingCommand::Backfill(arg) => {
            let scope = match arg.days {
                Some(days) => Backfill::Days(days),
                None => Backfill::All,
            };
            service.filing_backfill(&arg.account, scope, arg.apply)
        }
        FilingCommand::Pin(arg) => service.filing_pin(&arg.account, &arg.id),
        FilingCommand::Unpin(arg) => service.filing_unpin(&arg.account, &arg.id),
        FilingCommand::Retry(arg) => {
            let target = match (&arg.id, &arg.folder, arg.arrival) {
                (Some(id), _, _) => RetryTarget::Message(id.clone()),
                (_, Some(folder), _) => RetryTarget::Folder(folder.clone()),
                (_, _, Some(arrival)) => RetryTarget::Arrival(arrival),
                _ => return Err(CliError::input("specify --id, --folder or --arrival")),
            };
            service.filing_retry(&arg.account, target)
        }
        FilingCommand::Dismiss(arg) => service.filing_dismiss(&arg.account, arg.arrival),
        FilingCommand::Adopt(arg) => service.filing_adopt(&arg.account, &arg.folder),
        FilingCommand::Log(arg) => service.filing_log(&arg.account, arg.id.as_deref(), arg.limit),
    }
    .map_err(service_error)
}

fn service_command(path: &Path, command: &ServiceCommand) -> Result<Value, CliError> {
    let result = match command {
        ServiceCommand::Install(arg) => Context::detect().and_then(|ctx| {
            let service = Service::open(path)?;
            system_service::install_account(
                &service,
                path,
                &arg.account,
                arg.interval_seconds,
                arg.limit,
                &ctx,
            )
        }),
        ServiceCommand::Uninstall(arg) => {
            if !config::valid_account_name(&arg.account) {
                return Err(CliError::input("account name cannot name a service"));
            }
            Context::detect().and_then(|ctx| system_service::uninstall(&ctx, &arg.account))
        }
        ServiceCommand::Status(arg) => Service::open(path).and_then(|service| {
            system_service::status_account(
                &service,
                path,
                &arg.account,
                Context::detect().ok().as_ref(),
            )
        }),
    };
    result
        .map(|service| json!({"schema_version": 1, "service": service}))
        .map_err(service_error)
}

fn read_input(path: &Path, max_bytes: usize) -> Result<Vec<u8>, CliError> {
    let mut input: Box<dyn Read> = if path == Path::new("-") {
        Box::new(io::stdin())
    } else {
        Box::new(
            fs::File::open(path)
                .map_err(|_| CliError::input(format!("cannot read {}", path.display())))?,
        )
    };
    let mut data = Vec::new();
    input
        .by_ref()
        .take((max_bytes + 1) as u64)
        .read_to_end(&mut data)
        .map_err(|_| CliError::input("cannot read input"))?;
    if data.len() > max_bytes {
        return Err(CliError::input("input exceeds size limit"));
    }
    Ok(data)
}

fn read_categories(path: &Path) -> Result<Vec<Category>, CliError> {
    let data = read_input(path, 1024 * 1024)?;
    let value: Value =
        serde_json::from_slice(&data).map_err(|_| CliError::input("invalid category JSON"))?;
    let array = if value.is_array() {
        value
    } else {
        value.get("categories").cloned().ok_or_else(|| {
            CliError::input("expected a category array or an object with categories")
        })?
    };
    serde_json::from_value(array).map_err(|_| CliError::input("invalid categories"))
}

fn validate_categories(categories: &[Category]) -> Result<(), CliError> {
    let mut candidate = config::default_config();
    candidate
        .accounts
        .values_mut()
        .next()
        .ok_or_else(|| CliError::input("default config has no account"))?
        .categories = categories.to_vec();
    config::validate(&candidate).map_err(|error| CliError::input(error.to_string()))
}

fn watch(cli: &Cli, arg: &WatchArg) -> Result<Value, CliError> {
    // Before anything else: which file this process runs, for the restart rule.
    let mut updates = update::watch::WatchUpdates::start(cli.json);
    let path = cli.config_path()?;
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    ctrlc::set_handler(move || {
        flag.store(true, Ordering::SeqCst);
    })
    .map_err(|_| CliError::operational())?;
    let stopped = || stop.load(Ordering::SeqCst);
    // The update hooks run only between passes, when no `Service` is open.
    let tally = watch_loop(
        stopped,
        arg.interval_seconds,
        cli.json,
        |moment| match moment {
            Moment::BeforePass => updates.before_pass(&path, &stopped),
            Moment::Waiting => updates.while_waiting(&stopped),
        },
        || Service::open(&path)?.sync(&arg.account, arg.limit),
    )?;
    let partial = tally.partial_passes > 0 || tally.skipped_passes > 0;
    Ok(
        json!({"schema_version":1,"watch":{"account":arg.account,"passes":tally.passes,"partial_passes":tally.partial_passes,"skipped_passes":tally.skipped_passes,"stopped":true},"partial":partial}),
    )
}

#[derive(Debug, Default, PartialEq, Eq)]
struct WatchTally {
    passes: u64,
    partial_passes: u64,
    /// Passes skipped because a configuration changed mid-pass.
    skipped_passes: u64,
}

/// When `watch_loop` calls its `between` hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Moment {
    /// Before each pass.
    BeforePass,
    /// Every 5 s while waiting for the next pass.
    Waiting,
}

/// Runs `run_pass` (a fresh open and one sync) until `stopped`, printing
/// each pass and waiting `interval_seconds` between passes. `between` runs
/// before each pass and every 5 s of the wait; it cannot fail a pass. A
/// pass that failed because `mailtriage.json` or the mail engine's
/// configuration changed mid-pass is skipped: its error object is printed
/// and the next pass reads the current configuration. Any other error, a
/// changed account binding included, ends the loop.
fn watch_loop(
    stopped: impl Fn() -> bool,
    interval_seconds: u64,
    json_mode: bool,
    mut between: impl FnMut(Moment),
    mut run_pass: impl FnMut() -> anyhow::Result<Value>,
) -> Result<WatchTally, CliError> {
    let mut tally = WatchTally::default();
    while !stopped() {
        between(Moment::BeforePass);
        if stopped() {
            break;
        }
        match run_pass() {
            Ok(result) => {
                if result.get("partial").and_then(Value::as_bool) == Some(true) {
                    tally.partial_passes += 1;
                }
                print_value(&result, json_mode).map_err(|_| CliError::operational())?;
            }
            Err(error) if is_config_change(&error) => {
                tally.skipped_passes += 1;
                print_error(&service_error(error), json_mode);
            }
            Err(error) => return Err(service_error(error)),
        }
        tally.passes += 1;
        for tick in 1..=interval_seconds.saturating_mul(5) {
            if stopped() {
                break;
            }
            thread::sleep(Duration::from_millis(200));
            if tick % 25 == 0 {
                between(Moment::Waiting);
            }
        }
    }
    Ok(tally)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn service_err(code: i32, kind: ErrorKind) -> anyhow::Error {
        ServiceError {
            code,
            message: "m".into(),
            kind,
        }
        .into()
    }

    /// `watch_loop` over scripted passes, stopping once they are used up.
    fn run(passes: Vec<anyhow::Result<Value>>) -> (Result<WatchTally, CliError>, usize) {
        let count = passes.len();
        let left = Cell::new(count);
        let mut passes = passes.into_iter();
        let result = watch_loop(
            || left.get() == 0,
            0,
            true,
            |_| {},
            || {
                left.set(left.get() - 1);
                passes.next().unwrap()
            },
        );
        (result, count - left.get())
    }

    /// Final review I1: a pass whose `mailtriage.json` or Himalaya TOML
    /// changed mid-pass is skipped and `watch` continues.
    #[test]
    fn watch_skips_a_pass_whose_configuration_changed() {
        let ok = || Ok(json!({"partial": false}));
        let (result, ran) = run(vec![
            Err(service_err(5, ErrorKind::ConfigChanged)),
            Err(mailtriage::engine::ConfigChanged.into()),
            ok(),
        ]);
        assert_eq!(ran, 3);
        let tally = result.unwrap_or_else(|e| panic!("stopped: {} {}", e.code, e.message));
        assert_eq!(
            tally,
            WatchTally {
                passes: 3,
                partial_passes: 0,
                skipped_passes: 2,
            }
        );
    }

    /// The update hook runs before every pass; it cannot fail or skip one.
    #[test]
    fn the_update_hook_runs_before_each_pass() {
        let moments = std::cell::RefCell::new(Vec::new());
        let left = Cell::new(2);
        let tally = watch_loop(
            || left.get() == 0,
            0,
            true,
            |moment| moments.borrow_mut().push(moment),
            || {
                left.set(left.get() - 1);
                Ok(json!({"partial": false}))
            },
        )
        .unwrap_or_else(|e| panic!("stopped: {} {}", e.code, e.message));
        assert_eq!(*moments.borrow(), [Moment::BeforePass, Moment::BeforePass]);
        assert_eq!(
            tally,
            WatchTally {
                passes: 2,
                partial_passes: 0,
                skipped_passes: 0,
            }
        );
    }

    /// Any other error, a changed account binding included, stops `watch`.
    #[test]
    fn watch_stops_on_other_errors() {
        for error in [
            service_err(5, ErrorKind::BindingConflict),
            service_err(5, ErrorKind::AccountBusy),
            service_err(5, ErrorKind::ConfigBusy),
            service_err(5, ErrorKind::Other),
            service_err(2, ErrorKind::Other),
            anyhow::anyhow!("operational"),
        ] {
            let (result, ran) = run(vec![
                Ok(json!({"partial": false})),
                Err(error),
                Ok(json!({})),
            ]);
            assert_eq!(ran, 2);
            assert!(result.is_err());
        }
        let (result, _) = run(vec![Err(service_err(5, ErrorKind::Other))]);
        let e = result.err().unwrap();
        assert_eq!((e.code, e.message.as_str()), (5, "m"));
    }

    /// The error object of a pass the mail engine refused because its
    /// configuration changed carries the conflict code.
    #[test]
    fn an_engine_configuration_change_is_a_conflict() {
        let e = service_error(mailtriage::engine::ConfigChanged.into());
        assert_eq!(e.code, 5);
        assert_eq!(e.reason, Some("config_changed"));
    }

    #[test]
    fn service_errors_pass_their_reason_through() {
        let e = service_error(service_err(5, ErrorKind::AccountBusy));
        assert_eq!(
            (e.code, e.message.as_str(), e.reason),
            (5, "m", Some("account_busy"))
        );
        assert_eq!(service_error(service_err(5, ErrorKind::Other)).reason, None);
        assert_eq!(service_error(anyhow::anyhow!("x")).reason, None);
        assert_eq!(CliError::input("x").reason, None);
        assert_eq!(CliError::operational().reason, None);
    }

    /// The JSON error object names a `reason` only when the error has one.
    #[test]
    fn the_json_error_object_has_a_reason_only_when_set() {
        let busy = service_error(service_err(5, ErrorKind::AccountBusy));
        assert_eq!(
            error_json(&busy),
            json!({"schema_version":1,"error":{"code":5,"message":"m","reason":"account_busy"}})
        );
        assert_eq!(
            error_json(&CliError::input("bad")),
            json!({"schema_version":1,"error":{"code":2,"message":"bad"}})
        );
    }
}
