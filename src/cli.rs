use clap::{Args, Parser, Subcommand};
use mailtriage::{
    config,
    domain::Category,
    service::{ListOptions, Service, ServiceError},
};
use serde_json::{json, Value};
use std::{
    fs,
    io::{self, Read, Write},
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
    #[arg(long, global = true, default_value = "mailtriage.json")]
    config: PathBuf,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Init,
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
}
impl CliError {
    fn input(message: impl Into<String>) -> Self {
        Self {
            code: 2,
            message: message.into(),
        }
    }
    fn operational() -> Self {
        Self {
            code: 3,
            message: "Operation failed; check configuration and dependency availability".into(),
        }
    }
}

fn service_error(error: anyhow::Error) -> CliError {
    if let Some(error) = error.downcast_ref::<ServiceError>() {
        return CliError {
            code: error.code,
            message: error.message.clone(),
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

fn print_error(error: &CliError, json_mode: bool) {
    if json_mode {
        let _ = print_value(
            &json!({"schema_version":1,"error":{"code":error.code,"message":error.message}}),
            true,
        );
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
            if cli.config.exists() {
                return Err(CliError::input(format!(
                    "config already exists: {}",
                    cli.config.display()
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
            config::save(&cli.config, &value)
                .map_err(|_| CliError::input("could not write config"))?;
            Ok(
                json!({"schema_version":1,"config":cli.config,"provider":"fake","account":account,"state_dir":".state"}),
            )
        }
        Command::Doctor(arg) => open(&cli.config)?
            .doctor(&arg.account)
            .map_err(service_error),
        Command::Classify(arg) => {
            let data = read_input(&arg.input, 16 * 1024 * 1024)?;
            open(&cli.config)?
                .classify(&arg.account, &data, &arg.format)
                .map_err(service_error)
        }
        Command::Sync(arg) => open(&cli.config)?
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
            open(&cli.config)?
                .list(&arg.account, opts)
                .map_err(service_error)
        }
        Command::Read(arg) => open(&cli.config)?
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
            open(&cli.config)?
                .correct(&arg.account, &arg.id, Value::Object(patch), clear)
                .map_err(service_error)
        }
        Command::Done(arg) => open(&cli.config)?
            .review(&arg.account, &arg.id, true)
            .map_err(service_error),
        Command::Reopen(arg) => open(&cli.config)?
            .review(&arg.account, &arg.id, false)
            .map_err(service_error),
        Command::Categories { command } => match command {
            CategoriesCommand::Export(arg) => open(&cli.config)?
                .categories(&arg.account)
                .map_err(service_error),
            CategoriesCommand::Validate { file } => {
                let categories = read_categories(file)?;
                validate_categories(&categories)?;
                Ok(json!({"schema_version":1,"valid":true,"categories":categories.len()}))
            }
            CategoriesCommand::Apply { account, file } => {
                let categories = read_categories(file)?;
                open(&cli.config)?
                    .apply_categories(account, categories)
                    .map_err(service_error)
            }
        },
        Command::Reclassify(arg) => open(&cli.config)?
            .reclassify(&arg.account, arg.since.as_deref(), arg.dry_run, arg.limit)
            .map_err(service_error),
        Command::Export(arg) => open(&cli.config)?
            .export(&arg.account)
            .map_err(service_error),
    }
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
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    ctrlc::set_handler(move || {
        flag.store(true, Ordering::SeqCst);
    })
    .map_err(|_| CliError::operational())?;
    let mut passes = 0u64;
    let mut partial_passes = 0u64;
    while !stop.load(Ordering::SeqCst) {
        let result = open(&cli.config)?
            .sync(&arg.account, arg.limit)
            .map_err(service_error)?;
        if result.get("partial").and_then(Value::as_bool) == Some(true) {
            partial_passes += 1;
        }
        passes += 1;
        print_value(&result, cli.json).map_err(|_| CliError::operational())?;
        for _ in 0..arg.interval_seconds.saturating_mul(5) {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(Duration::from_millis(200));
        }
    }
    Ok(
        json!({"schema_version":1,"watch":{"account":arg.account,"passes":passes,"partial_passes":partial_passes,"stopped":true},"partial":partial_passes>0}),
    )
}
