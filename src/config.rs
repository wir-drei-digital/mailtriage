use crate::domain::{
    AccountConfig, AppConfig, Category, EngineConfig, FilingConfig, FilingMode, PolicyConfig,
    ProviderConfig,
};
use anyhow::{bail, Context, Result};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};
use uuid::Uuid;

pub fn default_config() -> AppConfig {
    let mut accounts = BTreeMap::new();
    accounts.insert(
        "work".into(),
        AccountConfig {
            identity: "work@example.invalid".into(),
            timezone: "UTC".into(),
            brief: "Example account for offline demonstrations; replace before connecting mail."
                .into(),
            taxonomy_revision: 1,
            categories: vec![
                Category {
                    id: "correspondence".into(),
                    name: "Correspondence".into(),
                    description: "Direct conversations with people".into(),
                    examples: vec![],
                    catch_all: false,
                    folder: None,
                },
                Category {
                    id: "transactions".into(),
                    name: "Transactions".into(),
                    description: "Receipts, orders and account activity".into(),
                    examples: vec![],
                    catch_all: false,
                    folder: None,
                },
                Category {
                    id: "updates".into(),
                    name: "Updates".into(),
                    description: "Status and service updates".into(),
                    examples: vec![],
                    catch_all: false,
                    folder: None,
                },
                Category {
                    id: "newsletters".into(),
                    name: "Newsletters".into(),
                    description: "Editorial mail and subscriptions".into(),
                    examples: vec![],
                    catch_all: false,
                    folder: None,
                },
                Category {
                    id: "promotions".into(),
                    name: "Promotions".into(),
                    description: "Offers and marketing".into(),
                    examples: vec![],
                    catch_all: false,
                    folder: None,
                },
                Category {
                    id: "other".into(),
                    name: "Other".into(),
                    description: "Mail outside the other categories".into(),
                    examples: vec![],
                    catch_all: true,
                    folder: None,
                },
            ],
            himalaya: None,
            engine: None,
            filing: FilingConfig::default(),
        },
    );
    AppConfig {
        schema_version: 2,
        state_dir: "./mailtriage-state".into(),
        provider: ProviderConfig {
            kind: "fake".into(),
            model: "fake/offline".into(),
            endpoint: String::new(),
            api_key_command: None,
            api_key_env: String::new(),
            timeout_seconds: 30,
        },
        policy: PolicyConfig {
            choice_confidence_min: 0.75,
            action_yes_min: 0.8,
            action_no_max: 0.2,
            review_mode: true,
            max_body_chars: 20_000,
            max_attempts: 5,
            freshness_hours: 24,
        },
        accounts,
    }
}

/// Moves the legacy `himalaya` block into `engine` and marks the config as schema 2.
/// Refuses unknown schema versions so a newer file is never rewritten as schema 2.
pub fn normalize(config: &mut AppConfig) -> Result<()> {
    check_schema_version(config.schema_version)?;
    for (name, account) in config.accounts.iter_mut() {
        if let Some(h) = account.himalaya.take() {
            if account.engine.is_some() {
                bail!("account {name}: configure either engine or the legacy himalaya block, not both");
            }
            account.engine = Some(EngineConfig::Himalaya(h));
        }
    }
    config.schema_version = 2;
    Ok(())
}

fn check_schema_version(version: u32) -> Result<()> {
    if !(1..=2).contains(&version) {
        bail!("unsupported config schema_version {version}");
    }
    Ok(())
}

pub fn validate(config: &AppConfig) -> Result<()> {
    check_schema_version(config.schema_version)?;
    if config.state_dir.as_os_str().is_empty() {
        bail!("state_dir cannot be empty");
    }
    if config.provider.model.trim().is_empty() {
        bail!("provider.model cannot be empty");
    }
    crate::provider::validate_configuration(&config.provider)?;
    let p = &config.policy;
    for (name, value) in [
        ("choice_confidence_min", p.choice_confidence_min),
        ("action_yes_min", p.action_yes_min),
        ("action_no_max", p.action_no_max),
    ] {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            bail!("policy.{name} must be between 0 and 1");
        }
    }
    if p.action_no_max >= p.action_yes_min {
        bail!("policy action thresholds must leave an uncertainty interval");
    }
    if p.max_body_chars == 0 || p.max_body_chars > 1_000_000 {
        bail!("policy.max_body_chars must be 1..=1000000");
    }
    if p.max_attempts == 0 || p.max_attempts > 100 {
        bail!("policy.max_attempts must be 1..=100");
    }
    if p.freshness_hours == 0 {
        bail!("policy.freshness_hours must be positive");
    }
    if config.accounts.is_empty() {
        bail!("at least one account is required");
    }
    for (name, account) in &config.accounts {
        if name.trim().is_empty() || name != name.trim() {
            bail!("account names must be nonempty and trimmed");
        }
        if account.identity.trim().is_empty() {
            bail!("account {name}: identity cannot be empty");
        }
        if account.timezone.trim().is_empty() {
            bail!("account {name}: timezone cannot be empty");
        }
        if account.taxonomy_revision == 0 {
            bail!("account {name}: taxonomy_revision must be positive");
        }
        if account.categories.is_empty() {
            bail!("account {name}: at least one category is required");
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut catch_all = 0;
        for cat in &account.categories {
            if !valid_id(&cat.id) {
                bail!("account {name}: invalid category id {:?}", cat.id);
            }
            if !ids.insert(cat.id.as_str()) {
                bail!("account {name}: duplicate category id {:?}", cat.id);
            }
            if cat.name.trim().is_empty() || cat.description.trim().is_empty() {
                bail!(
                    "account {name}: category {:?} needs name and description",
                    cat.id
                );
            }
            catch_all += usize::from(cat.catch_all);
        }
        if catch_all != 1 {
            bail!("account {name}: exactly one catch_all category is required");
        }
        if account.himalaya.is_some() && account.engine.is_some() {
            bail!("account {name}: configure either engine or the legacy himalaya block, not both");
        }
        if let Some(EngineConfig::Himalaya(h)) = &account.engine_config() {
            if h.binary.as_os_str().is_empty()
                || h.config.as_os_str().is_empty()
                || h.account.trim().is_empty()
                || h.expected_version.trim().is_empty()
            {
                bail!("account {name}: incomplete Himalaya configuration");
            }
            if h.mailboxes.is_empty() || h.mailboxes.iter().any(|m| m.trim().is_empty()) {
                bail!("account {name}: Himalaya mailboxes cannot be empty");
            }
            if h.timeout_seconds == 0 || h.max_output_bytes == 0 {
                bail!("account {name}: Himalaya limits must be positive");
            }
        }
        if !(1..=1000).contains(&account.filing.max_actions_per_pass) {
            bail!("account {name}: filing.max_actions_per_pass must be 1..=1000");
        }
        if account.filing.mode != FilingMode::Off {
            if let Some((id, problem)) = folder_problems(account).first() {
                bail!("account {name}: category {id:?} {problem}");
            }
            if let Some(mailbox) = unsafe_source_mailboxes(account).first() {
                bail!("account {name}: source mailbox {mailbox:?} must be {SOURCE_RULE} while filing is on");
            }
        }
    }
    Ok(())
}

/// Categories whose effective folder breaks the filing rules, in category
/// order: an invalid name, or a folder an earlier category already uses
/// (compared case-insensitively; `INBOX` may repeat). Applies whatever the
/// filing mode; `validate` enforces it only with filing on.
pub fn invalid_folder_ids(account: &AccountConfig) -> Vec<String> {
    folder_problems(account)
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

/// The rule `unsafe_source_mailboxes` enforces, for error messages.
pub const SOURCE_RULE: &str = "printable ASCII without \\, \" or & and no leading -";

/// Configured source mailboxes that filing cannot name safely in raw IMAP
/// text: not printable ASCII, containing `\`, `"` or `&`, or starting with
/// `-`. Applies whatever the filing mode; `validate` enforces it only with
/// filing on.
pub fn unsafe_source_mailboxes(account: &AccountConfig) -> Vec<String> {
    let Some(EngineConfig::Himalaya(h)) = account.engine_config() else {
        return vec![];
    };
    h.mailboxes
        .iter()
        .filter(|m| {
            !m.chars().all(|c| (' '..='~').contains(&c))
                || m.contains(['\\', '"', '&'])
                || m.starts_with('-')
        })
        .cloned()
        .collect()
}

fn folder_problems(account: &AccountConfig) -> Vec<(String, &'static str)> {
    let mut folders = std::collections::BTreeSet::new();
    let mut problems = Vec::new();
    for cat in &account.categories {
        let folder = cat.effective_folder();
        if !valid_folder_name(folder) {
            problems.push((cat.id.clone(), "needs a valid folder"));
        } else if folder != "INBOX" && !folders.insert(folder.to_ascii_lowercase()) {
            problems.push((cat.id.clone(), "reuses another category's folder"));
        }
    }
    problems
}

/// A single printable-ASCII path segment, or the literal `INBOX` (stay in the source folder).
/// `&` is refused: it is the modified UTF-7 shift character in IMAP mailbox names.
fn valid_folder_name(folder: &str) -> bool {
    folder == "INBOX"
        || (!folder.is_empty()
            && folder.len() <= 200
            && folder.trim() == folder
            && !folder.eq_ignore_ascii_case("inbox")
            && folder.chars().all(|c| (' '..='~').contains(&c))
            && !folder.starts_with('-')
            && !folder.contains(['/', '.', '*', '%', '"', '\\', '&']))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
pub fn load(path: &Path) -> Result<AppConfig> {
    let data = fs::read(path).with_context(|| format!("read config {}", path.display()))?;
    let mut config: AppConfig = serde_json::from_slice(&data).context("parse config JSON")?;
    normalize(&mut config)?;
    validate(&config)?;
    Ok(config)
}

pub fn save(path: &Path, config: &AppConfig) -> Result<()> {
    let mut config = config.clone();
    normalize(&mut config)?;
    validate(&config)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)
        .with_context(|| format!("create config directory {}", parent.display()))?;
    let filename = path
        .file_name()
        .context("config path must name a file")?
        .to_string_lossy();
    let tmp = parent.join(format!(".{filename}.{}.tmp", Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp).context("create temporary config")?;
        serde_json::to_writer_pretty(&mut file, &config).context("serialize config")?;
        file.write_all(b"\n")?;
        file.sync_all().context("sync temporary config")?;
        fs::rename(&tmp, path).context("replace config atomically")?;
        if let Ok(dir) = OpenOptions::new().read(true).open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}
