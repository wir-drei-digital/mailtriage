//! The categories window's state: accounts, the loaded categories, the
//! draft and its revisions, checks, apply, removal, closing and the refile
//! panel. `update` takes a message and returns the commands to run; the
//! editor draws the state and runs the commands.
use super::{
    health::{self, Observations, State},
    words,
};
use crate::cli::{
    Categories, Category, Changes, Details, Failure, FilingMode, RefileMarked, RefilePreview,
    Status, Validation,
};
use chrono::{DateTime, Utc};
use std::time::{Duration, Instant};

/// Timings, shortened by tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// From the last change to its check.
    pub check_delay: Duration,
    /// Between attempts while mailtriage is busy.
    pub retry_delay: Duration,
    /// Attempts after the first while mailtriage is busy.
    pub retries: u32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            check_delay: Duration::from_millis(500),
            retry_delay: Duration::from_secs(2),
            retries: 3,
        }
    }
}

/// One category as the form edits it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftCategory {
    pub id: String,
    pub name: String,
    pub description: String,
    /// One example per line.
    pub examples: String,
    pub default: bool,
    /// The "Mail folder" field; empty means "same as the name".
    pub folder: String,
    /// Loaded from mailtriage: its ID is fixed.
    pub existing: bool,
    /// The person typed an ID, so the name no longer suggests one.
    pub id_edited: bool,
}

impl DraftCategory {
    /// With filing on, the field shows the folder mail goes to now.
    pub fn loaded(c: &Category, filing_on: bool) -> Self {
        Self {
            id: c.id.clone(),
            name: c.name.clone(),
            description: c.description.clone(),
            examples: c.examples.join("\n"),
            default: c.catch_all,
            folder: if filing_on {
                c.effective_folder().to_owned()
            } else {
                c.folder.clone().unwrap_or_default()
            },
            existing: true,
            id_edited: false,
        }
    }

    /// The category as the draft file holds it. With filing on it always
    /// carries a folder: the field's text, or the name when it is empty.
    /// With filing off an empty field omits it.
    pub fn category(&self, filing_on: bool) -> Category {
        let folder = self.folder.trim();
        Category {
            id: self.id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            examples: self
                .examples
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect(),
            catch_all: self.default,
            folder: match (filing_on, folder.is_empty()) {
                (true, true) => Some(self.name.clone()),
                (_, false) => Some(folder.to_owned()),
                (false, true) => None,
            },
        }
    }
}

/// An ID from a name: lowercase letters, digits and `-`, accents folded,
/// unique among `taken` (by `-2`, `-3`, …).
pub fn suggest_id(name: &str, taken: &[&str]) -> String {
    let mut id = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        let folded = match c {
            'a'..='z' | '0'..='9' => c.to_string(),
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a".into(),
            'ç' | 'ć' | 'č' => "c".into(),
            'ď' | 'đ' => "d".into(),
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => "e".into(),
            'ğ' => "g".into(),
            'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' | 'ı' => "i".into(),
            'ł' | 'ľ' | 'ĺ' => "l".into(),
            'ñ' | 'ń' | 'ň' => "n".into(),
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => "o".into(),
            'ŕ' | 'ř' => "r".into(),
            'ś' | 'š' | 'ş' => "s".into(),
            'ť' | 'ţ' => "t".into(),
            'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' | 'ų' => "u".into(),
            'ý' | 'ÿ' => "y".into(),
            'ź' | 'ż' | 'ž' => "z".into(),
            'ß' => "ss".into(),
            'æ' => "ae".into(),
            'œ' => "oe".into(),
            _ => "-".into(),
        };
        if folded == "-" {
            if !id.is_empty() && !id.ends_with('-') {
                id.push('-');
            }
        } else {
            id.push_str(&folded);
        }
    }
    let base = match id.trim_end_matches('-') {
        "" => "category".to_owned(),
        trimmed => trimmed.to_owned(),
    };
    let mut candidate = base.clone();
    let mut n = 2;
    while taken.contains(&candidate.as_str()) {
        candidate = format!("{base}-{n}");
        n += 1;
    }
    candidate
}

/// Which field of a category changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Name,
    Description,
    Folder,
    Id,
    Examples,
}

/// An account in the selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountEntry {
    pub name: String,
    pub filing_mode: FilingMode,
}

/// The categories as loaded, which the draft is compared with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub account: String,
    pub digest: String,
    pub filing_mode: FilingMode,
    pub categories: Vec<Category>,
    pub draft: Vec<DraftCategory>,
}

impl Loaded {
    pub fn filing_on(&self) -> bool {
        self.filing_mode != FilingMode::Off
    }
}

/// The latest finished check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    None,
    Valid {
        account: String,
        revision: u64,
        changes: Changes,
    },
    Invalid {
        account: String,
        revision: u64,
        message: String,
        details: Details,
    },
}

/// A command that writes, during which the form is read-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Running {
    Applying { revision: u64, attempt: u32 },
    Moving,
}

/// What to do once unsaved edits are discarded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Then {
    Switch(String),
    Reload,
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    Discard {
        question: String,
        then: Then,
    },
    ConfirmApply {
        revision: u64,
        title: String,
        lines: Vec<String>,
        warning: Option<String>,
    },
    ConfirmRemove {
        index: usize,
        question: String,
    },
    CategoriesChanged,
    Details(Details),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    Saved(String),
    BusyRetrying,
    /// Busy after every retry: shown with "Try again".
    Busy(Details),
    Rechecking,
    Finishing,
    Error {
        message: String,
        details: Option<Details>,
    },
}

/// The refile panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refile {
    pub account: String,
    pub dry_run: bool,
    /// `None` while the preview loads.
    pub preview: Option<RefilePreview>,
    /// What the last move marked, with the service line when it applies.
    pub result: Option<String>,
    pub error: Option<(String, Details)>,
}

/// Something the person did, or a command's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    Start,
    Tick,
    SelectAccount(String),
    Reload,
    Select(usize),
    Add,
    Edit(usize, Field, String),
    SetDefault(usize),
    RequestRemove(usize),
    Revert,
    RequestApply,
    Confirm,
    Cancel,
    KeepEditing,
    ReloadDiscarding,
    TryAgain,
    ShowDetails(Details),
    MoveFiledMail,
    MoveFolder(String),
    MoveAll,
    CloseRequested,
    Status {
        load: u64,
        at: DateTime<Utc>,
        result: Result<Status, Failure>,
    },
    Exported {
        load: u64,
        result: Result<Categories, Failure>,
    },
    Checked {
        account: String,
        revision: u64,
        result: Result<Validation, Failure>,
    },
    Applied {
        revision: u64,
        result: Result<(), Failure>,
    },
    Previewed {
        account: String,
        result: Result<RefilePreview, Failure>,
    },
    Moved {
        account: String,
        result: Result<RefileMarked, Failure>,
    },
    MovedStatus {
        account: String,
        at: DateTime<Utc>,
        result: Result<Status, Failure>,
    },
}

/// A command for the editor to run; its answer comes back as a `Msg`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Status {
        load: u64,
    },
    Export {
        load: u64,
        account: String,
    },
    /// Write `body` to `draft-<revision>.json` (once) and validate it.
    Check {
        account: String,
        revision: u64,
        body: String,
    },
    /// Apply `draft-<revision>.json`, never rewritten.
    Apply {
        account: String,
        revision: u64,
        digest: String,
    },
    Preview {
        account: String,
    },
    Refile {
        account: String,
        folder: Option<String>,
    },
    MovedStatus {
        account: String,
    },
    DeleteDraft(u64),
    Close,
}

/// The window's whole state.
#[derive(Debug, Clone)]
pub struct Window {
    pub options: Options,
    pub accounts: Vec<AccountEntry>,
    /// The account shown in the selector.
    pub account: Option<String>,
    pub loaded: Option<Loaded>,
    pub draft: Vec<DraftCategory>,
    pub selected: usize,
    pub revision: u64,
    /// The running load's number.
    pub load: Option<u64>,
    loads: u64,
    load_account: Option<String>,
    pub check: Check,
    check_due: Option<Instant>,
    /// The revision whose check runs or is due.
    checking: Option<u64>,
    /// The revision whose draft file the accepted check used.
    accepted_file: Option<u64>,
    pub running: Option<Running>,
    retry_at: Option<Instant>,
    pub dialog: Option<Dialog>,
    pub notice: Option<Notice>,
    pub refile: Option<Refile>,
    /// The changes of the last apply, to open the refile panel after the reload.
    refile_after_load: bool,
    pub closing: bool,
    /// 3 once the first load failed (or mailtriage was not found).
    pub exit_code: i32,
    observations: Observations,
}

impl Window {
    /// A window that opens on `account` (the first one when `None`).
    pub fn new(options: Options, account: Option<String>) -> Self {
        Self {
            options,
            accounts: vec![],
            account,
            loaded: None,
            draft: vec![],
            selected: 0,
            revision: 0,
            load: None,
            loads: 0,
            load_account: None,
            check: Check::None,
            check_due: None,
            checking: None,
            accepted_file: None,
            running: None,
            retry_at: None,
            dialog: None,
            notice: None,
            refile: None,
            refile_after_load: false,
            closing: false,
            exit_code: 0,
            observations: Observations::default(),
        }
    }

    /// A window that can only show why it cannot work.
    pub fn failed(message: String) -> Self {
        let mut window = Self::new(Options::default(), None);
        window.notice = Some(Notice::Error {
            message,
            details: None,
        });
        window.exit_code = 3;
        window
    }

    pub fn loading(&self) -> bool {
        self.load.is_some()
    }

    pub fn filing_on(&self) -> bool {
        self.loaded.as_ref().is_some_and(Loaded::filing_on)
    }

    /// The form cannot be edited: nothing loaded, a load or a write runs,
    /// or the window is closing.
    pub fn read_only(&self) -> bool {
        self.loaded.is_none() || self.loading() || self.running.is_some() || self.closing
    }

    /// The selector and Reload are off while a write runs.
    pub fn selector_enabled(&self) -> bool {
        self.running.is_none() && !self.closing && !self.accounts.is_empty()
    }

    /// The draft differs from what was loaded.
    pub fn dirty(&self) -> bool {
        self.loaded.as_ref().is_some_and(|l| l.draft != self.draft)
    }

    /// The categories the draft file holds.
    pub fn draft_categories(&self) -> Vec<Category> {
        let filing_on = self.filing_on();
        self.draft.iter().map(|c| c.category(filing_on)).collect()
    }

    fn draft_body(&self) -> String {
        serde_json::json!({ "categories": self.draft_categories() }).to_string()
    }

    /// The name of category `id`: the draft's, else the loaded one's.
    pub fn name_of(&self, id: &str) -> String {
        self.draft
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.name.clone())
            .or_else(|| {
                self.loaded
                    .as_ref()
                    .and_then(|l| l.categories.iter().find(|c| c.id == id))
                    .map(|c| c.name.clone())
            })
            .unwrap_or_else(|| id.to_owned())
    }

    fn current_check(&self) -> Option<&Check> {
        let account = self.account.as_deref()?;
        match &self.check {
            Check::Valid {
                account: a,
                revision,
                ..
            }
            | Check::Invalid {
                account: a,
                revision,
                ..
            } if a == account && *revision == self.revision => Some(&self.check),
            _ => None,
        }
    }

    /// The changes of the current revision's valid check.
    pub fn changes(&self) -> Option<&Changes> {
        match self.current_check() {
            Some(Check::Valid { changes, .. }) => Some(changes),
            _ => None,
        }
    }

    /// The current revision's check is due or running.
    pub fn checking(&self) -> bool {
        self.dirty() && self.current_check().is_none()
    }

    /// Why Apply is disabled; `None` when it is enabled.
    pub fn apply_blocked(&self) -> Option<&'static str> {
        if self.loading() {
            return Some("Wait until loading finishes");
        }
        if self.loaded.is_none() {
            return Some(words::NOT_LOADED);
        }
        if self.running.is_some() || self.closing {
            return Some("Wait until the current action finishes");
        }
        if !self.dirty() {
            return Some("No changes to apply");
        }
        match self.current_check() {
            Some(Check::Valid { .. }) => None,
            Some(_) => Some("Fix the problem shown below first"),
            None => Some("Checking your changes…"),
        }
    }

    /// The footer's text about the draft: the change summary or the error.
    pub fn check_text(&self) -> Option<String> {
        let name = |id: &str| self.name_of(id);
        match self.current_check()? {
            Check::Valid { changes, .. } => {
                Some(words::change_summary(changes, &name, self.filing_on()))
            }
            Check::Invalid { message, .. } => Some(words::plain_error(message, &name)),
            Check::None => None,
        }
    }

    /// The details of the current revision's failed check.
    pub fn check_details(&self) -> Option<&Details> {
        match self.current_check()? {
            Check::Invalid { details, .. } => Some(details),
            _ => None,
        }
    }

    /// When the next timer is due, for the editor's repaint.
    pub fn next_deadline(&self) -> Option<Instant> {
        [self.check_due, self.retry_at].into_iter().flatten().min()
    }

    fn start_load(&mut self, account: Option<String>) -> Vec<Effect> {
        self.loads += 1;
        self.load = Some(self.loads);
        self.load_account = account;
        self.check_due = None;
        self.checking = None;
        vec![Effect::Status { load: self.loads }]
    }

    /// An edit: the notice of an earlier command goes, a new revision starts.
    fn changed(&mut self, now: Instant) {
        self.notice = None;
        self.bump(now);
    }

    /// A new revision, checked after a pause when the draft differs.
    fn bump(&mut self, now: Instant) {
        self.revision += 1;
        if self.dirty() {
            self.check_due = Some(now + self.options.check_delay);
            self.checking = Some(self.revision);
        } else {
            self.check_due = None;
            self.checking = None;
        }
    }

    fn check_effect(&mut self) -> Vec<Effect> {
        let Some(account) = self.account.clone() else {
            return vec![];
        };
        self.check_due = None;
        self.checking = Some(self.revision);
        vec![Effect::Check {
            account,
            revision: self.revision,
            body: self.draft_body(),
        }]
    }

    fn apply_effect(&mut self, revision: u64, attempt: u32) -> Vec<Effect> {
        let (Some(account), Some(loaded)) = (self.account.clone(), &self.loaded) else {
            return vec![];
        };
        self.running = Some(Running::Applying { revision, attempt });
        vec![Effect::Apply {
            account,
            revision,
            digest: loaded.digest.clone(),
        }]
    }

    /// Effects that finish a write; closes the window when it was asked to.
    fn finished(&mut self, mut effects: Vec<Effect>) -> Vec<Effect> {
        self.running = None;
        if self.closing {
            effects.push(Effect::Close);
        }
        effects
    }

    fn discard_or(&mut self, then: Then, now: Instant) -> Vec<Effect> {
        if !self.dirty() {
            return self.after_discard(then, now);
        }
        let question = match (&then, &self.loaded) {
            (Then::Close, _) | (_, None) => "Discard changes?".to_owned(),
            (_, Some(l)) => format!("Discard changes to {}?", l.account),
        };
        self.dialog = Some(Dialog::Discard { question, then });
        vec![]
    }

    /// Drops the unsaved edits, the notice about them and their check (a
    /// new revision), then does `then`.
    fn after_discard(&mut self, then: Then, now: Instant) -> Vec<Effect> {
        if let Some(loaded) = &self.loaded {
            self.draft = loaded.draft.clone();
        }
        self.changed(now);
        match then {
            Then::Close => vec![Effect::Close],
            Then::Reload => {
                let account = self.account.clone();
                self.start_load(account)
            }
            Then::Switch(account) => {
                self.account = Some(account.clone());
                self.refile = None;
                self.start_load(Some(account))
            }
        }
    }

    fn select_after_load(&mut self) -> usize {
        self.draft
            .iter()
            .position(|c| !c.default)
            .unwrap_or_default()
    }

    /// Handles `msg` at `now` and returns the commands to run.
    pub fn update(&mut self, msg: Msg, now: Instant) -> Vec<Effect> {
        match msg {
            Msg::Start => {
                let account = self.account.clone();
                self.start_load(account)
            }
            Msg::Tick => {
                let mut effects = vec![];
                if self.check_due.is_some_and(|due| now >= due)
                    && !self.loading()
                    && self.running.is_none()
                {
                    effects.extend(self.check_effect());
                }
                if let (Some(due), Some(Running::Applying { revision, attempt })) =
                    (self.retry_at, self.running)
                {
                    if now >= due {
                        self.retry_at = None;
                        effects.extend(self.apply_effect(revision, attempt + 1));
                    }
                }
                effects
            }
            Msg::SelectAccount(name) => {
                if self.account.as_deref() == Some(&name) || !self.selector_enabled() {
                    return vec![];
                }
                self.discard_or(Then::Switch(name), now)
            }
            Msg::Reload => {
                if !self.selector_enabled() {
                    return vec![];
                }
                self.discard_or(Then::Reload, now)
            }
            Msg::Select(index) => {
                if index < self.draft.len() {
                    self.selected = index;
                }
                vec![]
            }
            Msg::Add => {
                if self.read_only() {
                    return vec![];
                }
                let names: Vec<&str> = self.draft.iter().map(|c| c.name.as_str()).collect();
                let mut name = "New category".to_owned();
                let mut n = 2;
                while names.contains(&name.as_str()) {
                    name = format!("New category {n}");
                    n += 1;
                }
                let taken: Vec<&str> = self.draft.iter().map(|c| c.id.as_str()).collect();
                let id = suggest_id(&name, &taken);
                self.draft.push(DraftCategory {
                    id,
                    name,
                    description: String::new(),
                    examples: String::new(),
                    default: false,
                    folder: String::new(),
                    existing: false,
                    id_edited: false,
                });
                self.selected = self.draft.len() - 1;
                self.changed(now);
                vec![]
            }
            Msg::Edit(index, field, value) => {
                if self.read_only() || index >= self.draft.len() {
                    return vec![];
                }
                let taken: Vec<String> = self
                    .draft
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != index)
                    .map(|(_, c)| c.id.clone())
                    .collect();
                let c = &mut self.draft[index];
                match field {
                    Field::Name => {
                        if !c.existing && !c.id_edited {
                            let taken: Vec<&str> = taken.iter().map(String::as_str).collect();
                            c.id = suggest_id(&value, &taken);
                        }
                        c.name = value;
                    }
                    Field::Description => c.description = value,
                    Field::Folder => c.folder = value,
                    Field::Examples => c.examples = value,
                    Field::Id => {
                        if c.existing {
                            return vec![];
                        }
                        c.id = value;
                        c.id_edited = true;
                    }
                }
                self.changed(now);
                vec![]
            }
            Msg::SetDefault(index) => {
                if self.read_only() || index >= self.draft.len() || self.draft[index].default {
                    return vec![];
                }
                for (i, c) in self.draft.iter_mut().enumerate() {
                    c.default = i == index;
                }
                self.changed(now);
                vec![]
            }
            Msg::RequestRemove(index) => {
                if self.read_only() || index >= self.draft.len() || self.draft[index].default {
                    return vec![];
                }
                let c = &self.draft[index];
                let folder = (c.existing && self.filing_on()).then(|| {
                    self.loaded
                        .as_ref()
                        .and_then(|l| l.categories.iter().find(|p| p.id == c.id))
                        .map(|p| p.effective_folder().to_owned())
                        .unwrap_or_else(|| c.name.clone())
                });
                self.dialog = Some(Dialog::ConfirmRemove {
                    index,
                    question: words::remove_question(&c.name, folder.as_deref()),
                });
                vec![]
            }
            Msg::Revert => {
                if self.read_only() || !self.dirty() {
                    return vec![];
                }
                if let Some(loaded) = &self.loaded {
                    self.draft = loaded.draft.clone();
                }
                self.selected = self.selected.min(self.draft.len().saturating_sub(1));
                self.changed(now);
                vec![]
            }
            Msg::RequestApply => {
                if self.apply_blocked().is_some() {
                    return vec![];
                }
                let Some(changes) = self.changes().cloned() else {
                    return vec![];
                };
                let name = |id: &str| self.name_of(id);
                let account = self.account.clone().unwrap_or_default();
                self.dialog = Some(Dialog::ConfirmApply {
                    revision: self.revision,
                    title: format!("Apply these changes to {account}?"),
                    lines: words::change_lines(&changes, &name, self.filing_on()),
                    warning: changes
                        .reclassifies
                        .then(|| words::RECLASSIFY_WARNING.to_owned()),
                });
                vec![]
            }
            Msg::Confirm => match self.dialog.take() {
                Some(Dialog::ConfirmApply { revision, .. }) => {
                    if revision != self.revision || self.apply_blocked().is_some() {
                        return vec![];
                    }
                    self.notice = None;
                    self.apply_effect(revision, 0)
                }
                Some(Dialog::ConfirmRemove { index, .. }) => {
                    if index < self.draft.len() && !self.read_only() {
                        self.draft.remove(index);
                        self.selected = self.selected.min(self.draft.len().saturating_sub(1));
                        self.changed(now);
                    }
                    vec![]
                }
                Some(Dialog::Discard { then, .. }) => self.after_discard(then, now),
                other => {
                    self.dialog = other;
                    vec![]
                }
            },
            Msg::Cancel => {
                if let Some(Dialog::Discard {
                    then: Then::Switch(_),
                    ..
                }) = &self.dialog
                {
                    // The selector shows the loaded account again.
                    self.account = self.loaded.as_ref().map(|l| l.account.clone());
                }
                self.dialog = None;
                vec![]
            }
            Msg::KeepEditing => {
                self.dialog = None;
                vec![]
            }
            Msg::ReloadDiscarding => {
                self.dialog = None;
                self.after_discard(Then::Reload, now)
            }
            Msg::TryAgain => {
                if matches!(self.notice, Some(Notice::Busy(_))) && self.apply_blocked().is_none() {
                    self.notice = None;
                    let revision = self.revision;
                    return self.apply_effect(revision, 0);
                }
                vec![]
            }
            Msg::ShowDetails(details) => {
                self.dialog = Some(Dialog::Details(details));
                vec![]
            }
            Msg::MoveFiledMail => {
                let Some(loaded) = &self.loaded else {
                    return vec![];
                };
                if !loaded.filing_on() || self.running.is_some() {
                    return vec![];
                }
                self.refile = Some(Refile {
                    account: loaded.account.clone(),
                    dry_run: loaded.filing_mode == FilingMode::DryRun,
                    preview: None,
                    result: None,
                    error: None,
                });
                vec![Effect::Preview {
                    account: loaded.account.clone(),
                }]
            }
            Msg::MoveFolder(native) => self.move_mail(Some(native)),
            Msg::MoveAll => self.move_mail(None),
            Msg::CloseRequested => {
                if self.running.is_some() {
                    self.closing = true;
                    self.notice = Some(Notice::Finishing);
                    return vec![];
                }
                self.discard_or(Then::Close, now)
            }
            Msg::Status { load, at, result } => {
                if self.load != Some(load) {
                    return vec![];
                }
                match result {
                    Ok(status) => {
                        self.observations.observe(&status.services, at);
                        self.accounts = status
                            .services
                            .iter()
                            .map(|s| AccountEntry {
                                name: s.account.clone(),
                                filing_mode: s.filing_mode,
                            })
                            .collect();
                        let wanted = self
                            .load_account
                            .clone()
                            .filter(|a| self.accounts.iter().any(|e| &e.name == a))
                            .or_else(|| self.accounts.first().map(|e| e.name.clone()));
                        let Some(account) = wanted else {
                            return self.load_failed("This config has no accounts.".into(), None);
                        };
                        self.account = Some(account.clone());
                        vec![Effect::Export { load, account }]
                    }
                    Err(failure) => self.load_failed(failure.message, Some(failure.details)),
                }
            }
            Msg::Exported { load, result } => {
                if self.load != Some(load) {
                    return vec![];
                }
                match result {
                    Ok(exported) => {
                        let filing_mode = self
                            .accounts
                            .iter()
                            .find(|a| a.name == exported.account)
                            .map_or(FilingMode::Off, |a| a.filing_mode);
                        let filing_on = filing_mode != FilingMode::Off;
                        let draft: Vec<DraftCategory> = exported
                            .categories
                            .iter()
                            .map(|c| DraftCategory::loaded(c, filing_on))
                            .collect();
                        self.account = Some(exported.account.clone());
                        self.loaded = Some(Loaded {
                            account: exported.account.clone(),
                            digest: exported.digest,
                            filing_mode,
                            categories: exported.categories,
                            draft: draft.clone(),
                        });
                        self.draft = draft;
                        self.selected = self.select_after_load();
                        self.load = None;
                        self.check = Check::None;
                        self.bump(now);
                        if std::mem::take(&mut self.refile_after_load) && filing_on {
                            self.refile = Some(Refile {
                                account: exported.account.clone(),
                                dry_run: filing_mode == FilingMode::DryRun,
                                preview: None,
                                result: None,
                                error: None,
                            });
                            return vec![Effect::Preview {
                                account: exported.account,
                            }];
                        }
                        if self
                            .refile
                            .as_ref()
                            .is_some_and(|r| r.account != exported.account)
                        {
                            self.refile = None;
                        }
                        vec![]
                    }
                    Err(failure) => self.load_failed(failure.message, Some(failure.details)),
                }
            }
            Msg::Checked {
                account,
                revision,
                result,
            } => {
                if self.account.as_deref() != Some(&account) || revision != self.revision {
                    return vec![Effect::DeleteDraft(revision)];
                }
                self.checking = None;
                if self.notice == Some(Notice::Rechecking) {
                    self.notice = None;
                }
                let mut effects = vec![];
                if let Some(old) = self.accepted_file.replace(revision) {
                    if old != revision {
                        effects.push(Effect::DeleteDraft(old));
                    }
                }
                self.check = match result {
                    Ok(validation) => Check::Valid {
                        account,
                        revision,
                        changes: validation.changes,
                    },
                    Err(failure) => Check::Invalid {
                        account,
                        revision,
                        message: failure.message,
                        details: failure.details,
                    },
                };
                effects
            }
            Msg::Applied { revision, result } => {
                let Some(Running::Applying { attempt, .. }) = self.running else {
                    return vec![];
                };
                match result {
                    Ok(()) => {
                        let changes = self.changes().cloned().unwrap_or_default();
                        self.notice = Some(Notice::Saved(words::saved(&changes)));
                        self.refile_after_load = self.filing_on()
                            && (!changes.added.is_empty()
                                || !changes.removed.is_empty()
                                || !changes.folders_changed.is_empty()
                                || changes.reclassifies);
                        // The applied draft is saved: no edit to discard
                        // while the reload runs.
                        if let Some(loaded) = &mut self.loaded {
                            loaded.draft = self.draft.clone();
                        }
                        let account = self.account.clone();
                        let load = self.start_load(account);
                        self.finished(load)
                    }
                    Err(failure) if failure.has_reason("categories_changed") => {
                        self.dialog = Some(Dialog::CategoriesChanged);
                        self.finished(vec![])
                    }
                    Err(failure) if failure.has_reason("config_busy") => {
                        if attempt < self.options.retries && !self.closing {
                            self.notice = Some(Notice::BusyRetrying);
                            self.running = Some(Running::Applying { revision, attempt });
                            self.retry_at = Some(now + self.options.retry_delay);
                            vec![]
                        } else {
                            self.notice = Some(Notice::Busy(failure.details));
                            self.finished(vec![])
                        }
                    }
                    Err(failure) if failure.has_reason("config_changed") => {
                        self.notice = Some(Notice::Rechecking);
                        self.check = Check::None;
                        self.running = None;
                        let effects = self.check_effect();
                        self.finished(effects)
                    }
                    Err(failure) => {
                        let name = |id: &str| self.name_of(id);
                        self.notice = Some(Notice::Error {
                            message: words::plain_error(&failure.message, &name),
                            details: Some(failure.details),
                        });
                        self.finished(vec![])
                    }
                }
            }
            Msg::Previewed { account, result } => {
                let Some(refile) = self.refile.as_mut().filter(|r| r.account == account) else {
                    return vec![];
                };
                match result {
                    Ok(preview) => {
                        refile.preview = Some(preview);
                        refile.error = None;
                    }
                    Err(failure) => refile.error = Some((failure.message, failure.details)),
                }
                vec![]
            }
            Msg::Moved { account, result } => {
                let mut effects = vec![];
                if let Some(refile) = self.refile.as_mut().filter(|r| r.account == account) {
                    match result {
                        Ok(marked) => {
                            refile.result = Some(words::moved(&marked));
                            refile.error = None;
                            effects.push(Effect::MovedStatus {
                                account: account.clone(),
                            });
                            effects.push(Effect::Preview { account });
                        }
                        Err(failure) => refile.error = Some((failure.message, failure.details)),
                    }
                }
                self.finished(effects)
            }
            Msg::MovedStatus {
                account,
                at,
                result,
            } => {
                let Ok(status) = result else {
                    return vec![];
                };
                self.observations.observe(&status.services, at);
                let Some(service) = status.services.iter().find(|s| s.account == account) else {
                    return vec![];
                };
                let state = health::health(service, &self.observations.get(&account), at).state;
                let running = matches!(
                    state,
                    State::Ok | State::Starting | State::Restarting | State::Warning
                );
                if let Some(refile) = self.refile.as_mut().filter(|r| r.account == account) {
                    if let (false, Some(result)) = (running, refile.result.as_mut()) {
                        result.push(' ');
                        result.push_str(words::SERVICE_NOT_RUNNING);
                    }
                }
                vec![]
            }
        }
    }

    fn move_mail(&mut self, folder: Option<String>) -> Vec<Effect> {
        let Some(refile) = &self.refile else {
            return vec![];
        };
        if refile.dry_run || self.running.is_some() || self.loading() {
            return vec![];
        }
        self.running = Some(Running::Moving);
        vec![Effect::Refile {
            account: refile.account.clone(),
            folder,
        }]
    }

    fn load_failed(&mut self, message: String, details: Option<Details>) -> Vec<Effect> {
        self.load = None;
        if self.loaded.is_none() {
            self.exit_code = 3;
        }
        // The selector shows the account that is still loaded.
        self.account = self
            .loaded
            .as_ref()
            .map(|l| l.account.clone())
            .or(self.account.take());
        self.notice = Some(Notice::Error { message, details });
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{RefileFolder, ServiceInfo};
    use std::collections::BTreeMap;

    fn t0() -> Instant {
        Instant::now()
    }

    fn details() -> Details {
        Details {
            command: "mailtriage".into(),
            exit_code: Some(5),
            output: String::new(),
        }
    }

    fn failure(reason: Option<&str>, message: &str) -> Failure {
        Failure {
            message: message.into(),
            reason: reason.map(str::to_owned),
            code: Some(5),
            details: details(),
        }
    }

    fn service(account: &str, filing_mode: FilingMode) -> ServiceInfo {
        ServiceInfo {
            account: account.into(),
            manager: "launchd".into(),
            installed: true,
            running: true,
            pid: Some(1),
            unit_path: None,
            log_paths: vec![],
            last_pass: None,
            service_config: None,
            file_config: None,
            needs_daemon_reload: None,
            config_matches: Some(true),
            enabled: Some(true),
            enablement: "enabled".into(),
            interval_seconds: Some(60),
            filing_mode,
            identity: account.into(),
            update: None,
        }
    }

    fn status() -> Status {
        Status {
            config: "/c.json".into(),
            services: vec![
                service("daniel", FilingMode::Live),
                service("info", FilingMode::Off),
            ],
        }
    }

    fn category(id: &str, name: &str, default: bool) -> Category {
        Category {
            id: id.into(),
            name: name.into(),
            description: format!("{name} mail"),
            examples: vec![],
            catch_all: default,
            folder: Some(name.into()),
        }
    }

    fn exported(account: &str) -> Categories {
        Categories {
            account: account.into(),
            categories: vec![
                category("news", "News", false),
                category("promo", "Promotions", false),
                category("other", "Other", true),
            ],
            digest: format!("v1:{account}"),
        }
    }

    fn valid(changes: Changes) -> Result<Validation, Failure> {
        Ok(Validation {
            digest: "v1:daniel".into(),
            changes,
        })
    }

    /// A window with `daniel` loaded.
    fn opened() -> Window {
        let mut w = Window::new(
            Options {
                check_delay: Duration::from_secs(1),
                retry_delay: Duration::from_secs(1),
                retries: 3,
            },
            None,
        );
        assert_eq!(w.update(Msg::Start, t0()), [Effect::Status { load: 1 }]);
        assert!(w.read_only());
        assert_eq!(
            w.update(
                Msg::Status {
                    load: 1,
                    at: Utc::now(),
                    result: Ok(status())
                },
                t0()
            ),
            [Effect::Export {
                load: 1,
                account: "daniel".into()
            }]
        );
        assert!(w.read_only(), "read-only while loading");
        w.update(
            Msg::Exported {
                load: 1,
                result: Ok(exported("daniel")),
            },
            t0(),
        );
        assert!(!w.read_only());
        w
    }

    fn rename_news(w: &mut Window, to: &str) {
        w.update(Msg::Edit(0, Field::Name, to.into()), t0());
    }

    /// Runs the due check and answers it.
    fn check(w: &mut Window, result: Result<Validation, Failure>) {
        let later = t0() + Duration::from_secs(5);
        let effects = w.update(Msg::Tick, later);
        let Some(Effect::Check {
            account, revision, ..
        }) = effects.first().cloned()
        else {
            panic!("no check: {effects:?}");
        };
        w.update(
            Msg::Checked {
                account,
                revision,
                result,
            },
            later,
        );
    }

    fn renamed() -> Changes {
        Changes {
            renamed: vec![crate::cli::Change {
                id: "news".into(),
                from: "News".into(),
                to: "Newsletters".into(),
            }],
            ..Changes::default()
        }
    }

    #[test]
    fn edits_advance_the_revision_and_are_checked_after_a_pause() {
        let mut w = opened();
        let start = w.revision;
        rename_news(&mut w, "Newsletters");
        assert_eq!(w.revision, start + 1);
        assert_eq!(w.apply_blocked(), Some("Checking your changes…"));
        assert_eq!(w.update(Msg::Tick, t0()), []);
        let effects = w.update(Msg::Tick, t0() + Duration::from_secs(5));
        let Effect::Check { body, revision, .. } = &effects[0] else {
            panic!()
        };
        assert_eq!(*revision, start + 1);
        let sent: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(sent["categories"][0]["name"], "Newsletters");
        // Filing is on: the folder stays explicit.
        assert_eq!(sent["categories"][0]["folder"], "News");
        w.update(
            Msg::Checked {
                account: "daniel".into(),
                revision: start + 1,
                result: valid(renamed()),
            },
            t0(),
        );
        assert_eq!(w.apply_blocked(), None);
        assert_eq!(w.check_text().unwrap(), "1 renamed");
    }

    #[test]
    fn older_and_other_account_answers_are_ignored_and_their_files_deleted() {
        let mut w = opened();
        rename_news(&mut w, "Newsletters");
        let old = w.revision;
        rename_news(&mut w, "Newsletter");
        assert_eq!(
            w.update(
                Msg::Checked {
                    account: "daniel".into(),
                    revision: old,
                    result: valid(renamed())
                },
                t0()
            ),
            [Effect::DeleteDraft(old)]
        );
        assert_eq!(w.apply_blocked(), Some("Checking your changes…"));
        assert_eq!(
            w.update(
                Msg::Checked {
                    account: "info".into(),
                    revision: w.revision,
                    result: valid(renamed())
                },
                t0()
            ),
            [Effect::DeleteDraft(w.revision)]
        );
        assert_eq!(w.apply_blocked(), Some("Checking your changes…"));
    }

    #[test]
    fn a_delayed_valid_answer_does_not_enable_apply_after_an_invalid_one() {
        let mut w = opened();
        rename_news(&mut w, "Newsletters");
        let old = w.revision;
        w.update(Msg::Edit(0, Field::Description, String::new()), t0());
        check(
            &mut w,
            Err(failure(
                None,
                "invalid categories; require unique IDs, descriptions and exactly one catch-all",
            )),
        );
        assert_eq!(w.apply_blocked(), Some("Fix the problem shown below first"));
        assert!(w.check_text().unwrap().starts_with("Every category needs"));
        w.update(
            Msg::Checked {
                account: "daniel".into(),
                revision: old,
                result: valid(renamed()),
            },
            t0(),
        );
        assert_eq!(w.apply_blocked(), Some("Fix the problem shown below first"));
    }

    #[test]
    fn superseded_loads_are_ignored_and_switching_with_edits_asks_first() {
        let mut w = opened();
        rename_news(&mut w, "Newsletters");
        assert_eq!(w.update(Msg::SelectAccount("info".into()), t0()), []);
        assert_eq!(
            w.dialog,
            Some(Dialog::Discard {
                question: "Discard changes to daniel?".into(),
                then: Then::Switch("info".into())
            })
        );
        w.update(Msg::Cancel, t0());
        assert_eq!(w.account.as_deref(), Some("daniel"));
        assert_eq!(w.draft[0].name, "Newsletters");
        w.update(Msg::SelectAccount("info".into()), t0());
        assert_eq!(w.update(Msg::Confirm, t0()), [Effect::Status { load: 2 }]);
        assert!(w.read_only());
        // Reload again before the first answer: load 2 is superseded.
        w.update(Msg::Reload, t0());
        assert_eq!(w.load, Some(3));
        assert_eq!(
            w.update(
                Msg::Status {
                    load: 2,
                    at: Utc::now(),
                    result: Ok(status())
                },
                t0()
            ),
            []
        );
        assert!(w.read_only());
        let effects = w.update(
            Msg::Status {
                load: 3,
                at: Utc::now(),
                result: Ok(status()),
            },
            t0(),
        );
        assert_eq!(
            effects,
            [Effect::Export {
                load: 3,
                account: "info".into()
            }]
        );
        w.update(
            Msg::Exported {
                load: 3,
                result: Ok(exported("info")),
            },
            t0(),
        );
        assert_eq!(w.loaded.as_ref().unwrap().account, "info");
        assert!(!w.dirty());
    }

    #[test]
    fn apply_confirms_then_runs_with_the_load_digest_and_reloads() {
        let mut w = opened();
        rename_news(&mut w, "Newsletters");
        check(&mut w, valid(renamed()));
        let revision = w.revision;
        assert_eq!(w.update(Msg::RequestApply, t0()), []);
        let Some(Dialog::ConfirmApply { lines, warning, .. }) = &w.dialog else {
            panic!()
        };
        assert_eq!(lines, &["Rename News to Newsletters"]);
        assert_eq!(warning, &None);
        assert_eq!(
            w.update(Msg::Confirm, t0()),
            [Effect::Apply {
                account: "daniel".into(),
                revision,
                digest: "v1:daniel".into()
            }]
        );
        // Read-only while it runs: an edit is ignored.
        assert!(w.read_only() && !w.selector_enabled());
        rename_news(&mut w, "X");
        assert_eq!(w.draft[0].name, "Newsletters");
        let effects = w.update(
            Msg::Applied {
                revision,
                result: Ok(()),
            },
            t0(),
        );
        assert_eq!(effects, [Effect::Status { load: 2 }]);
        assert_eq!(
            w.notice,
            Some(Notice::Saved(
                "Saved. New mail uses these categories.".into()
            ))
        );
        assert!(w.read_only(), "read-only during the reload after a save");
        // The saved draft is no unsaved edit: closing does not ask.
        assert!(!w.dirty(), "not dirty during the reload after a save");
        assert_eq!(w.clone().update(Msg::CloseRequested, t0()), [Effect::Close]);
        // The reload keeps what the save said.
        w.update(
            Msg::Status {
                load: 2,
                at: Utc::now(),
                result: Ok(status()),
            },
            t0(),
        );
        w.update(
            Msg::Exported {
                load: 2,
                result: Ok(exported("daniel")),
            },
            t0(),
        );
        assert!(matches!(w.notice, Some(Notice::Saved(_))));
        assert!(!w.read_only());
    }

    #[test]
    fn a_reclassifying_apply_warns_and_opens_the_refile_panel_after_the_reload() {
        let mut w = opened();
        w.update(Msg::Add, t0());
        let added = w.draft.len() - 1;
        w.update(Msg::Edit(added, Field::Name, "Travel".into()), t0());
        w.update(Msg::Edit(added, Field::Description, "Trips".into()), t0());
        assert_eq!(w.draft[added].id, "travel");
        let changes = Changes {
            added: vec!["travel".into()],
            reclassifies: true,
            ..Changes::default()
        };
        check(&mut w, valid(changes));
        w.update(Msg::RequestApply, t0());
        let Some(Dialog::ConfirmApply { warning, lines, .. }) = &w.dialog else {
            panic!()
        };
        assert_eq!(lines, &["Add Travel"]);
        assert_eq!(warning.as_deref(), Some(words::RECLASSIFY_WARNING));
        let revision = w.revision;
        w.update(Msg::Confirm, t0());
        w.update(
            Msg::Applied {
                revision,
                result: Ok(()),
            },
            t0(),
        );
        w.update(
            Msg::Status {
                load: 2,
                at: Utc::now(),
                result: Ok(status()),
            },
            t0(),
        );
        assert_eq!(
            w.update(
                Msg::Exported {
                    load: 2,
                    result: Ok(exported("daniel"))
                },
                t0()
            ),
            [Effect::Preview {
                account: "daniel".into()
            }]
        );
        assert!(w.refile.is_some());
    }

    fn applying() -> (Window, u64) {
        let mut w = opened();
        rename_news(&mut w, "Newsletters");
        check(&mut w, valid(renamed()));
        w.update(Msg::RequestApply, t0());
        w.update(Msg::Confirm, t0());
        let revision = w.revision;
        (w, revision)
    }

    #[test]
    fn categories_changed_offers_a_reload_and_keep_editing_keeps_the_draft() {
        let (mut w, revision) = applying();
        w.update(
            Msg::Applied {
                revision,
                result: Err(failure(Some("categories_changed"), "x")),
            },
            t0(),
        );
        assert_eq!(w.dialog, Some(Dialog::CategoriesChanged));
        w.update(Msg::KeepEditing, t0());
        assert_eq!(w.draft[0].name, "Newsletters");
        assert!(!w.read_only());
        w.dialog = Some(Dialog::CategoriesChanged);
        assert_eq!(
            w.update(Msg::ReloadDiscarding, t0()),
            [Effect::Status { load: 2 }]
        );
        assert_eq!(w.draft[0].name, "News", "the edits are discarded");
        assert!(!w.dirty());
        assert!(w.revision > revision, "revisions only grow");
        assert_eq!(w.check_text(), None, "the discarded draft's check is gone");
    }

    #[test]
    fn config_busy_is_retried_three_times_then_offers_try_again() {
        let (mut w, revision) = applying();
        let busy = || {
            Err(failure(
                Some("config_busy"),
                "configuration is being edited",
            ))
        };
        for attempt in 1..=3 {
            assert_eq!(
                w.update(
                    Msg::Applied {
                        revision,
                        result: busy()
                    },
                    t0()
                ),
                []
            );
            assert_eq!(w.notice, Some(Notice::BusyRetrying));
            assert!(w.read_only());
            let effects = w.update(Msg::Tick, t0() + Duration::from_secs(5));
            assert_eq!(
                effects,
                [Effect::Apply {
                    account: "daniel".into(),
                    revision,
                    digest: "v1:daniel".into()
                }],
                "attempt {attempt}"
            );
        }
        w.update(
            Msg::Applied {
                revision,
                result: busy(),
            },
            t0(),
        );
        assert_eq!(w.notice, Some(Notice::Busy(details())));
        assert!(!w.read_only());
        assert_eq!(w.update(Msg::TryAgain, t0()).len(), 1);
    }

    #[test]
    fn config_changed_checks_the_draft_again() {
        let (mut w, revision) = applying();
        let effects = w.update(
            Msg::Applied {
                revision,
                result: Err(failure(Some("config_changed"), "changed")),
            },
            t0(),
        );
        assert!(matches!(&effects[..], [Effect::Check { revision: r, .. }] if *r == revision));
        assert_eq!(w.notice, Some(Notice::Rechecking));
        assert_eq!(w.apply_blocked(), Some("Checking your changes…"));
        // The answer replaces the notice with the check's result.
        w.update(
            Msg::Checked {
                account: "daniel".into(),
                revision,
                result: valid(renamed()),
            },
            t0(),
        );
        assert_eq!(w.notice, None);
        assert_eq!(w.apply_blocked(), None);
        assert_eq!(w.check_text().unwrap(), "1 renamed");
    }

    #[test]
    fn a_discard_clears_the_notice_and_the_old_check() {
        // An apply fails on daniel; switching to info discards its error too.
        let (mut w, revision) = applying();
        w.update(
            Msg::Applied {
                revision,
                result: Err(failure(None, "category has manual corrections: promo")),
            },
            t0(),
        );
        assert!(matches!(w.notice, Some(Notice::Error { .. })));
        w.update(Msg::SelectAccount("info".into()), t0());
        assert_eq!(w.update(Msg::Confirm, t0()), [Effect::Status { load: 2 }]);
        assert_eq!(w.notice, None, "daniel's error does not follow the switch");
        assert!(w.revision > revision, "revisions only grow");
        assert_eq!(w.check_text(), None);
        // "Saved." stays through the reload after a save, not a switch.
        let (mut w, revision) = applying();
        w.update(
            Msg::Applied {
                revision,
                result: Ok(()),
            },
            t0(),
        );
        w.update(
            Msg::Status {
                load: 2,
                at: Utc::now(),
                result: Ok(status()),
            },
            t0(),
        );
        w.update(
            Msg::Exported {
                load: 2,
                result: Ok(exported("daniel")),
            },
            t0(),
        );
        assert!(matches!(w.notice, Some(Notice::Saved(_))));
        assert_eq!(
            w.update(Msg::SelectAccount("info".into()), t0()),
            [Effect::Status { load: 3 }]
        );
        assert_eq!(w.notice, None, "Saved. does not follow the switch");
    }

    #[test]
    fn try_again_applies_only_a_draft_that_can_be_applied() {
        // Busy, then Reload discards the draft: Try again has nothing to apply.
        let (mut w, revision) = applying();
        w.options.retries = 0;
        w.update(
            Msg::Applied {
                revision,
                result: Err(failure(
                    Some("config_busy"),
                    "configuration is being edited",
                )),
            },
            t0(),
        );
        assert_eq!(w.notice, Some(Notice::Busy(details())));
        w.update(Msg::Reload, t0());
        assert_eq!(w.update(Msg::Confirm, t0()), [Effect::Status { load: 2 }]);
        assert_eq!(
            w.update(Msg::TryAgain, t0()),
            [],
            "the discarded draft is not applied"
        );
        assert_eq!(w.notice, None, "the busy notice goes with its draft");
        assert_eq!(w.draft[0].name, "News");
        assert_eq!(w.check_text(), None);
        // A busy notice does nothing while Apply is blocked, here by the
        // reload after a save whose check is still the current one.
        let (mut w, revision) = applying();
        w.update(
            Msg::Applied {
                revision,
                result: Ok(()),
            },
            t0(),
        );
        assert!(w.changes().is_some() && w.apply_blocked().is_some());
        w.notice = Some(Notice::Busy(details()));
        assert_eq!(w.update(Msg::TryAgain, t0()), []);
    }

    #[test]
    fn removal_asks_first_and_the_default_stays_single() {
        let mut w = opened();
        w.update(Msg::RequestRemove(2), t0());
        assert_eq!(w.dialog, None, "the default category cannot be removed");
        w.update(Msg::RequestRemove(1), t0());
        assert_eq!(
            w.dialog,
            Some(Dialog::ConfirmRemove {
                index: 1,
                question: "Remove Promotions? Mail in Promotions stays there until you move it; new mail is sorted into the remaining categories.".into()
            })
        );
        assert_eq!(w.draft.len(), 3, "nothing changes before confirming");
        assert_eq!(w.update(Msg::Confirm, t0()), []);
        assert_eq!(w.draft.len(), 2);
        assert!(w.dirty());
        w.update(Msg::SetDefault(0), t0());
        assert_eq!(
            w.draft.iter().filter(|c| c.default).count(),
            1,
            "exactly one default"
        );
        assert!(w.draft[0].default);
    }

    #[test]
    fn folder_serialization_with_filing_on_and_off() {
        let c = DraftCategory {
            id: "news".into(),
            name: "Newsletters".into(),
            description: "d".into(),
            examples: " a \n\nb\n".into(),
            default: false,
            folder: String::new(),
            existing: false,
            id_edited: false,
        };
        let on = c.category(true);
        assert_eq!(on.folder.as_deref(), Some("Newsletters"));
        assert_eq!(on.examples, ["a", "b"]);
        assert_eq!(c.category(false).folder, None);
        let typed = DraftCategory {
            folder: " News ".into(),
            ..c
        };
        assert_eq!(typed.category(true).folder.as_deref(), Some("News"));
        assert_eq!(typed.category(false).folder.as_deref(), Some("News"));
        // A loaded category shows the folder mail goes to now.
        let stored = Category {
            folder: None,
            ..category("news", "News", false)
        };
        assert_eq!(DraftCategory::loaded(&stored, true).folder, "News");
        assert_eq!(DraftCategory::loaded(&stored, false).folder, "");
    }

    #[test]
    fn suggested_ids() {
        assert_eq!(suggest_id("Café & Bars", &[]), "cafe-bars");
        assert_eq!(suggest_id("Größe Ärger", &[]), "grosse-arger");
        assert_eq!(suggest_id("  Spaß!! ", &[]), "spass");
        assert_eq!(suggest_id("日本", &[]), "category");
        assert_eq!(suggest_id("Travel", &["travel"]), "travel-2");
        assert_eq!(suggest_id("Travel", &["travel", "travel-2"]), "travel-3");
        let mut w = opened();
        w.update(Msg::Add, t0());
        let i = w.draft.len() - 1;
        w.update(Msg::Edit(i, Field::Id, "trips".into()), t0());
        w.update(Msg::Edit(i, Field::Name, "Travel".into()), t0());
        assert_eq!(w.draft[i].id, "trips", "a typed ID stays");
        w.update(Msg::Edit(0, Field::Id, "changed".into()), t0());
        assert_eq!(w.draft[0].id, "news", "an existing ID is read-only");
    }

    #[test]
    fn apply_explains_why_it_is_disabled() {
        let mut w = Window::new(Options::default(), None);
        w.update(Msg::Start, t0());
        assert_eq!(w.apply_blocked(), Some("Wait until loading finishes"));
        // Nothing loaded and no load running: no waiting helps.
        assert_eq!(
            Window::failed("mailtriage not found".into()).apply_blocked(),
            Some(words::NOT_LOADED)
        );
        w = opened();
        assert_eq!(w.apply_blocked(), Some("No changes to apply"));
        rename_news(&mut w, "Newsletters");
        assert_eq!(w.apply_blocked(), Some("Checking your changes…"));
        rename_news(&mut w, "News");
        assert_eq!(w.apply_blocked(), Some("No changes to apply"));
        let (w, _) = applying();
        assert_eq!(
            w.apply_blocked(),
            Some("Wait until the current action finishes")
        );
    }

    #[test]
    fn closing_asks_about_edits_and_waits_for_a_command() {
        let mut w = opened();
        assert_eq!(w.update(Msg::CloseRequested, t0()), [Effect::Close]);
        rename_news(&mut w, "Newsletters");
        assert_eq!(w.update(Msg::CloseRequested, t0()), []);
        assert_eq!(
            w.dialog,
            Some(Dialog::Discard {
                question: "Discard changes?".into(),
                then: Then::Close
            })
        );
        assert_eq!(w.update(Msg::Confirm, t0()), [Effect::Close]);
        let (mut w, revision) = applying();
        assert_eq!(w.update(Msg::CloseRequested, t0()), []);
        assert_eq!(w.notice, Some(Notice::Finishing));
        let effects = w.update(
            Msg::Applied {
                revision,
                result: Err(failure(None, "boom")),
            },
            t0(),
        );
        assert_eq!(effects, [Effect::Close]);
    }

    #[test]
    fn the_first_load_failing_is_exit_3() {
        let mut w = Window::new(Options::default(), Some("daniel".into()));
        w.update(Msg::Start, t0());
        w.update(
            Msg::Status {
                load: 1,
                at: Utc::now(),
                result: Ok(status()),
            },
            t0(),
        );
        w.update(
            Msg::Exported {
                load: 1,
                result: Err(failure(None, "unknown account")),
            },
            t0(),
        );
        assert_eq!(w.exit_code, 3);
        assert_eq!(
            w.notice,
            Some(Notice::Error {
                message: "unknown account".into(),
                details: Some(details())
            })
        );
        assert!(w.read_only());
        assert_eq!(w.apply_blocked(), Some(words::NOT_LOADED));
    }

    fn preview() -> RefilePreview {
        RefilePreview {
            total: 38,
            folders: vec![RefileFolder {
                folder: Some("Promotions".into()),
                native: "INBOX.Promotions".into(),
                retired: true,
                candidates: 30,
                waiting: 12,
            }],
            waiting: 12,
            skipped: BTreeMap::new(),
        }
    }

    #[test]
    fn the_refile_panel_moves_a_folder_and_reports_a_stopped_service() {
        let mut w = opened();
        assert_eq!(
            w.update(Msg::MoveFiledMail, t0()),
            [Effect::Preview {
                account: "daniel".into()
            }]
        );
        w.update(
            Msg::Previewed {
                account: "daniel".into(),
                result: Ok(preview()),
            },
            t0(),
        );
        assert_eq!(
            w.update(Msg::MoveFolder("INBOX.Promotions".into()), t0()),
            [Effect::Refile {
                account: "daniel".into(),
                folder: Some("INBOX.Promotions".into())
            }]
        );
        assert!(w.read_only());
        let effects = w.update(
            Msg::Moved {
                account: "daniel".into(),
                result: Ok(RefileMarked {
                    marked: 30,
                    waiting_marked: 12,
                }),
            },
            t0(),
        );
        assert_eq!(
            effects,
            [
                Effect::MovedStatus {
                    account: "daniel".into()
                },
                Effect::Preview {
                    account: "daniel".into()
                }
            ]
        );
        let mut stopped = status();
        stopped.services[0].running = false;
        stopped.services[0].enabled = Some(false);
        w.update(
            Msg::MovedStatus {
                account: "daniel".into(),
                at: Utc::now(),
                result: Ok(stopped),
            },
            t0(),
        );
        assert_eq!(
            w.refile.as_ref().unwrap().result.as_deref(),
            Some("Marked 30 messages; 12 more will move if their new category calls for it. The background service is not running. Start it, or run `mailtriage sync`, to move them.")
        );
    }

    #[test]
    fn dry_run_shows_the_preview_only() {
        let mut w = opened();
        w.loaded.as_mut().unwrap().filing_mode = FilingMode::DryRun;
        w.update(Msg::MoveFiledMail, t0());
        assert!(w.refile.as_ref().unwrap().dry_run);
        assert_eq!(w.update(Msg::MoveAll, t0()), []);
        // Filing off: no panel at all.
        w.loaded.as_mut().unwrap().filing_mode = FilingMode::Off;
        w.refile = None;
        assert_eq!(w.update(Msg::MoveFiledMail, t0()), []);
    }
}
