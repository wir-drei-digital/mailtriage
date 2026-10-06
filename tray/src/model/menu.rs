//! The tray menu: its entries and the action each sends back.
use super::{
    health::{self, Health, IconState, Observations, State, Warning},
    time::clock,
};
use crate::cli::{Details, FilingMode, ServiceInfo, Status};
use chrono::{DateTime, FixedOffset, Utc};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// What a menu item asks the tray to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Refresh,
    Start(String),
    Stop(String),
    Install(String),
    OpenLog(PathBuf),
    CopyLogCommand(String),
    EditCategories(String),
    ToggleAutostart,
    CopyDetails(String),
    Quit,
}

impl Action {
    /// The id of this action's menu item: its kind, then its argument
    /// (log paths come from JSON, so they are text). Equal actions have
    /// equal ids and different actions different ones, so a click that
    /// arrives after the menu was redrawn does what its label said, or
    /// nothing when the item is gone ([`Menu::action`]).
    pub fn id(&self) -> String {
        let with = |kind: &str, argument: &str| format!("{kind}:{argument}");
        match self {
            Action::Refresh => "refresh".into(),
            Action::Start(account) => with("start", account),
            Action::Stop(account) => with("stop", account),
            Action::Install(account) => with("install", account),
            Action::OpenLog(path) => with("open-log", &path.to_string_lossy()),
            Action::CopyLogCommand(text) => with("copy-log-command", text),
            Action::EditCategories(account) => with("edit-categories", account),
            Action::ToggleAutostart => "toggle-autostart".into(),
            Action::CopyDetails(text) => with("copy-details", text),
            Action::Quit => "quit".into(),
        }
    }
}

/// A service command in flight, shown instead of its menu item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Start,
    Stop,
    Install,
}

impl Verb {
    fn doing(self) -> &'static str {
        match self {
            Verb::Start => "Starting…",
            Verb::Stop => "Stopping…",
            Verb::Install => "Installing…",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// A line that cannot be clicked.
    Text(String),
    Item {
        label: String,
        action: Action,
    },
    Check {
        label: String,
        checked: bool,
        action: Action,
    },
    Submenu {
        label: String,
        entries: Vec<Entry>,
    },
    Separator,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Menu {
    /// The first line and the tooltip.
    pub summary: String,
    pub icon: IconState,
    pub entries: Vec<Entry>,
}

impl Menu {
    /// The action of the item whose id ([`Action::id`]) is `id`; `None`
    /// when this menu has no such item.
    pub fn action(&self, id: &str) -> Option<Action> {
        fn find(entries: &[Entry], id: &str) -> Option<Action> {
            entries.iter().find_map(|entry| match entry {
                Entry::Item { action, .. } | Entry::Check { action, .. } => {
                    (action.id() == id).then(|| action.clone())
                }
                Entry::Submenu { entries, .. } => find(entries, id),
                Entry::Text(_) | Entry::Separator => None,
            })
        }
        find(&self.entries, id)
    }
}

/// A short line under the summary, with "Show details" when it has some.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub details: Option<Details>,
}

/// Everything the menu shows.
pub struct Input<'a> {
    /// The last good `service status`, while it is shown.
    pub status: Option<&'a Status>,
    /// The status failed since; the menu is marked "(stale)".
    pub stale: bool,
    /// Why there is no status: the summary text and its details.
    pub failure: Option<(String, Option<Details>)>,
    pub observations: &'a Observations,
    pub now: DateTime<Utc>,
    pub local: DateTime<FixedOffset>,
    pub busy: &'a BTreeMap<String, Verb>,
    pub notices: &'a [Notice],
    pub autostart: bool,
    pub tray_version: &'a str,
    /// `mailtriage --version` of the CLI the tray runs.
    pub cli_version: Option<&'a str>,
    pub cli_path: Option<&'a Path>,
}

fn filing(mode: FilingMode) -> &'static str {
    match mode {
        FilingMode::Off => "filing off",
        FilingMode::DryRun => "filing dry run",
        FilingMode::Live => "filing live",
    }
}

/// The account row's state text.
pub fn state_text(h: &Health, local: DateTime<FixedOffset>) -> String {
    let at = |t: Option<DateTime<Utc>>| t.map(|t| clock(t, local)).unwrap_or_default();
    match h.state {
        State::Ok => format!("Running · checked {}", at(h.since)),
        State::Starting => "Starting…".into(),
        State::Restarting => "Restarting…".into(),
        State::Warning => match h.warning {
            Some(Warning::SomeMailSkipped) => "Some mail skipped".into(),
            Some(Warning::ConfigurationChanged) => "Configuration changed".into(),
            Some(Warning::AnotherBusy) => "Another mailtriage was busy".into(),
            Some(Warning::NoCheckSince) => format!("No check since {}", at(h.since)),
            Some(Warning::NoCheckYet) | None => "No check yet".into(),
        },
        State::Error => format!("Problem since {}", at(h.since)),
        State::Stopped => "Stopped".into(),
        State::NotInstalled => "Not installed".into(),
        State::OtherConfig => "Runs another config".into(),
        State::OtherConfigUnknown | State::Unknown => "Status unknown".into(),
        State::Unavailable => "No background service on this system".into(),
    }
}

fn glyph(state: State) -> &'static str {
    match health::severity(state) {
        health::Severity::Fine => "●",
        health::Severity::Off => "○",
        _ => "▲",
    }
}

/// The CLI's version: `update.installed` of a service whose executable is
/// the CLI the tray runs, else `mailtriage --version`.
fn cli_version<'a>(input: &'a Input) -> Option<&'a str> {
    input
        .status
        .and_then(|status| {
            status.services.iter().find_map(|s| {
                let update = s.update.as_ref()?;
                (update.executable.as_deref() == input.cli_path)
                    .then_some(update.installed.as_deref())
                    .flatten()
            })
        })
        .or(input.cli_version)
}

fn account(s: &ServiceInfo, h: &Health, input: &Input, config: &Path) -> Entry {
    let local = input.local;
    let mut label = s.account.clone();
    if s.identity != s.account {
        label += &format!(" ({})", s.identity);
    }
    label += &format!("  {} {}", glyph(h.state), state_text(h, local));
    let mut entries = vec![];
    if h.state == State::Unavailable {
        entries.push(Entry::Text(state_text(h, local)));
    } else {
        let first = if s.running {
            "Running"
        } else if !s.installed {
            "Not installed"
        } else if h.state == State::Stopped {
            "Stopped"
        } else {
            "Not running"
        };
        entries.push(Entry::Text(format!("{first} · {}", filing(s.filing_mode))));
    }
    if s.installed {
        entries.push(Entry::Text(match &s.last_pass {
            None => "No check yet".into(),
            Some(pass) => {
                let mut line = format!(
                    "Last check {}",
                    clock(pass.finished_at.with_timezone(&Utc), local)
                );
                line += match (pass.exit_code, pass.reason.as_deref()) {
                    (0, _) => "",
                    (4, _) => ", some mail skipped",
                    (5, Some("config_changed")) => ", configuration changed",
                    (5, Some("account_busy")) => ", another mailtriage was busy",
                    _ => ", failed",
                };
                if let Some(version) = &pass.version {
                    line += &format!(" · v{version}");
                }
                line
            }
        }));
    }
    match h.state {
        State::OtherConfig => {
            let other = [&s.service_config, &s.file_config]
                .into_iter()
                .flatten()
                .find(|p| p.as_path() != config);
            if let Some(other) = other {
                entries.push(Entry::Text(format!("Runs {}", other.display())));
            }
        }
        State::OtherConfigUnknown => {
            entries.push(Entry::Text(if s.needs_daemon_reload == Some(true) {
                format!(
                    "The service file changed; run `mailtriage service install --account {}`",
                    s.account
                )
            } else {
                "The tray cannot tell which config this service runs.".into()
            }))
        }
        State::Unknown => entries.push(Entry::Text(
            "The tray cannot tell whether this service starts at login.".into(),
        )),
        State::Error => {
            let at = h.since.map(|t| clock(t, local)).unwrap_or_default();
            entries.push(Entry::Text(if health::last_pass_failed(s) && s.running {
                format!("The last check failed at {at}")
            } else {
                format!("Not running since {at}")
            }));
        }
        _ => {}
    }
    if h.disabled_running {
        entries.push(Entry::Text(
            "Disabled: won't start again after logout".into(),
        ));
    }
    entries.push(Entry::Separator);
    let name = s.account.clone();
    if let Some(verb) = input.busy.get(&s.account) {
        entries.push(Entry::Text(verb.doing().into()));
    } else {
        let item = match h.state {
            State::Ok | State::Starting | State::Restarting | State::Warning | State::Error => {
                Some(("Stop service", Action::Stop(name.clone())))
            }
            State::Stopped => Some(("Start service", Action::Start(name.clone()))),
            State::NotInstalled => Some(("Install service", Action::Install(name.clone()))),
            _ => None,
        };
        if let Some((label, action)) = item {
            entries.push(Entry::Item {
                label: label.into(),
                action,
            });
        }
    }
    if s.installed {
        match s.manager.as_str() {
            "launchd" => {
                if let Some(log) = s.log_paths.first() {
                    entries.push(Entry::Item {
                        label: "Open log".into(),
                        action: Action::OpenLog(log.clone()),
                    });
                }
                if let (State::Error, Some(err)) = (h.state, s.log_paths.get(1)) {
                    entries.push(Entry::Item {
                        label: "Open error log".into(),
                        action: Action::OpenLog(err.clone()),
                    });
                }
            }
            "systemd" => entries.push(Entry::Item {
                label: "Copy log command".into(),
                action: Action::CopyLogCommand(format!(
                    "journalctl --user -u mailtriage-{name}.service -e"
                )),
            }),
            _ => {}
        }
    }
    entries.push(Entry::Item {
        label: "Edit categories…".into(),
        action: Action::EditCategories(name),
    });
    Entry::Submenu { label, entries }
}

fn notices(input: &Input, entries: &mut Vec<Entry>) {
    for notice in input.notices {
        entries.push(Entry::Text(notice.text.clone()));
        if let Some(details) = &notice.details {
            entries.push(Entry::Item {
                label: "Show details".into(),
                action: Action::CopyDetails(details.text()),
            });
        }
    }
}

fn footer(input: &Input, entries: &mut Vec<Entry>) {
    entries.push(Entry::Separator);
    entries.push(Entry::Item {
        label: "Refresh now".into(),
        action: Action::Refresh,
    });
    entries.push(Entry::Check {
        label: "Start at login".into(),
        checked: input.autostart,
        action: Action::ToggleAutostart,
    });
    entries.push(Entry::Item {
        label: "Quit".into(),
        action: Action::Quit,
    });
}

/// The menu for `input`.
pub fn build(input: &Input) -> Menu {
    let mut entries = vec![];
    let Some(status) = input.status else {
        let (text, details) = input
            .failure
            .clone()
            .unwrap_or_else(|| ("Checking…".into(), None));
        let summary = format!("mailtriage: {text}");
        entries.push(Entry::Text(summary.clone()));
        if let Some(details) = details {
            entries.push(Entry::Item {
                label: "Show details".into(),
                action: Action::CopyDetails(details.text()),
            });
        }
        notices(input, &mut entries);
        footer(input, &mut entries);
        return Menu {
            summary,
            icon: IconState::Error,
            entries,
        };
    };
    let healths: Vec<Health> = status
        .services
        .iter()
        .map(|s| health::health(s, &input.observations.get(&s.account), input.now))
        .collect();
    let states: Vec<(String, State)> = status
        .services
        .iter()
        .zip(&healths)
        .map(|(s, h)| (s.account.clone(), h.state))
        .collect();
    let mut summary = format!("mailtriage: {}", health::summary(&states));
    if input.stale {
        summary += " (stale)";
    }
    entries.push(Entry::Text(summary.clone()));
    if let Some(cli) = cli_version(input).filter(|v| *v != input.tray_version) {
        entries.push(Entry::Text(format!(
            "mailtriage {cli} and tray {} differ; run mailtriage update",
            input.tray_version
        )));
    }
    notices(input, &mut entries);
    entries.push(Entry::Separator);
    for (s, h) in status.services.iter().zip(&healths) {
        entries.push(account(s, h, input, &status.config));
    }
    footer(input, &mut entries);
    Menu {
        summary,
        icon: health::icon(&states.iter().map(|(_, s)| *s).collect::<Vec<_>>()),
        entries,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{LastPass, UpdateInfo};
    use chrono::Duration;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-06T12:05:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn local() -> DateTime<FixedOffset> {
        now().with_timezone(&FixedOffset::east_opt(0).unwrap())
    }

    fn service(account: &str) -> ServiceInfo {
        ServiceInfo {
            account: account.into(),
            manager: "launchd".into(),
            installed: true,
            running: true,
            pid: Some(1),
            unit_path: None,
            log_paths: vec!["/l/x.log".into(), "/l/x.err".into()],
            last_pass: Some(LastPass {
                finished_at: (now() - Duration::minutes(2)).fixed_offset(),
                partial: false,
                exit_code: 0,
                mode: "live".into(),
                reason: None,
                version: None,
            }),
            service_config: Some("/c.json".into()),
            file_config: Some("/c.json".into()),
            needs_daemon_reload: None,
            config_matches: Some(true),
            enabled: Some(true),
            enablement: "enabled".into(),
            interval_seconds: Some(60),
            filing_mode: FilingMode::Live,
            identity: account.into(),
            update: None,
        }
    }

    fn menu_with(services: Vec<ServiceInfo>, busy: &BTreeMap<String, Verb>) -> Menu {
        let status = Status {
            config: "/c.json".into(),
            services,
        };
        let mut seen = Observations::default();
        seen.observe(&status.services, now() - Duration::minutes(10));
        build(&Input {
            status: Some(&status),
            stale: false,
            failure: None,
            observations: &seen,
            now: now(),
            local: local(),
            busy,
            notices: &[],
            autostart: true,
            tray_version: "0.1.0",
            cli_version: Some("0.1.0"),
            cli_path: Some(Path::new("/usr/local/bin/mailtriage")),
        })
    }

    fn submenu(menu: &Menu, index: usize) -> (String, Vec<Entry>) {
        let accounts: Vec<&Entry> = menu
            .entries
            .iter()
            .filter(|e| matches!(e, Entry::Submenu { .. }))
            .collect();
        match accounts[index] {
            Entry::Submenu { label, entries } => (label.clone(), entries.clone()),
            _ => unreachable!(),
        }
    }

    fn labels(entries: &[Entry]) -> Vec<String> {
        entries
            .iter()
            .map(|e| match e {
                Entry::Text(t) => t.clone(),
                Entry::Item { label, .. } | Entry::Check { label, .. } => label.clone(),
                Entry::Submenu { label, .. } => format!("▸ {label}"),
                Entry::Separator => "—".into(),
            })
            .collect()
    }

    #[test]
    fn a_running_account_and_a_not_installed_one() {
        let mut info = service("info");
        info.installed = false;
        info.running = false;
        info.last_pass = None;
        info.enabled = Some(false);
        let mut daniel = service("daniel");
        daniel.identity = "daniel@example.com".into();
        if let Some(pass) = &mut daniel.last_pass {
            pass.exit_code = 4;
            pass.version = Some("0.3.0".into());
        }
        let menu = menu_with(vec![daniel, info], &BTreeMap::new());
        assert_eq!(menu.summary, "mailtriage: daniel needs attention");
        assert_eq!(menu.icon, IconState::Warning);
        assert_eq!(
            labels(&menu.entries),
            [
                "mailtriage: daniel needs attention",
                "—",
                "▸ daniel (daniel@example.com)  ▲ Some mail skipped",
                "▸ info  ○ Not installed",
                "—",
                "Refresh now",
                "Start at login",
                "Quit"
            ]
        );
        let (_, daniel) = submenu(&menu, 0);
        assert_eq!(
            labels(&daniel),
            [
                "Running · filing live",
                "Last check 12:03, some mail skipped · v0.3.0",
                "—",
                "Stop service",
                "Open log",
                "Edit categories…"
            ]
        );
        let (_, info) = submenu(&menu, 1);
        assert_eq!(
            labels(&info),
            [
                "Not installed · filing live",
                "—",
                "Install service",
                "Edit categories…"
            ]
        );
    }

    #[test]
    fn the_service_item_follows_the_state() {
        let mut stopped = service("a");
        stopped.running = false;
        stopped.enabled = Some(false);
        let mut other = service("b");
        other.config_matches = Some(false);
        other.service_config = Some("/other.json".into());
        let mut unknown = service("c");
        unknown.config_matches = None;
        unknown.needs_daemon_reload = Some(true);
        let mut unavailable = service("d");
        unavailable.manager = "none".into();
        unavailable.installed = false;
        unavailable.running = false;
        let mut systemd = service("e");
        systemd.manager = "systemd".into();
        systemd.log_paths = vec![];
        let busy = BTreeMap::from([("e".to_owned(), Verb::Stop)]);
        let menu = menu_with(vec![stopped, other, unknown, unavailable, systemd], &busy);
        let rows: Vec<Vec<String>> = (0..5).map(|i| labels(&submenu(&menu, i).1)).collect();
        assert_eq!(
            rows[0],
            [
                "Stopped · filing live",
                "Last check 12:03",
                "—",
                "Start service",
                "Open log",
                "Edit categories…"
            ]
        );
        assert_eq!(
            rows[1],
            [
                "Running · filing live",
                "Last check 12:03",
                "Runs /other.json",
                "—",
                "Open log",
                "Edit categories…"
            ]
        );
        assert_eq!(
            rows[2][2],
            "The service file changed; run `mailtriage service install --account c`"
        );
        assert!(!rows[2].iter().any(|l| l.ends_with("service")));
        assert_eq!(
            rows[3],
            [
                "No background service on this system",
                "—",
                "Edit categories…"
            ]
        );
        assert_eq!(
            rows[4],
            [
                "Running · filing live",
                "Last check 12:03",
                "—",
                "Stopping…",
                "Copy log command",
                "Edit categories…"
            ]
        );
        let (_, e) = submenu(&menu, 4);
        assert!(e.contains(&Entry::Item {
            label: "Copy log command".into(),
            action: Action::CopyLogCommand("journalctl --user -u mailtriage-e.service -e".into())
        }));
    }

    #[test]
    fn errors_add_the_error_log_and_disabled_jobs_say_so() {
        let mut failing = service("a");
        if let Some(pass) = &mut failing.last_pass {
            pass.exit_code = 3;
        }
        failing.enabled = Some(false);
        let menu = menu_with(vec![failing], &BTreeMap::new());
        assert_eq!(menu.icon, IconState::Error);
        let (label, entries) = submenu(&menu, 0);
        assert_eq!(label, "a  ▲ Problem since 12:03");
        assert_eq!(
            labels(&entries),
            [
                "Running · filing live",
                "Last check 12:03, failed",
                "The last check failed at 12:03",
                "Disabled: won't start again after logout",
                "—",
                "Stop service",
                "Open log",
                "Open error log",
                "Edit categories…"
            ]
        );
    }

    #[test]
    fn a_version_mismatch_adds_a_line() {
        let mut s = service("a");
        s.update = Some(UpdateInfo {
            executable: Some("/usr/local/bin/mailtriage".into()),
            installed: Some("0.2.0".into()),
        });
        let menu = menu_with(vec![s], &BTreeMap::new());
        assert_eq!(
            labels(&menu.entries)[1],
            "mailtriage 0.2.0 and tray 0.1.0 differ; run mailtriage update"
        );
    }

    #[test]
    fn a_failure_shows_its_text_and_details() {
        let details = Details {
            command: "mailtriage service status --json".into(),
            exit_code: Some(3),
            output: "boom".into(),
        };
        let seen = Observations::default();
        let menu = build(&Input {
            status: None,
            stale: false,
            failure: Some((
                "mailtriage could not start: x".into(),
                Some(details.clone()),
            )),
            observations: &seen,
            now: now(),
            local: local(),
            busy: &BTreeMap::new(),
            notices: &[],
            autostart: false,
            tray_version: "0.1.0",
            cli_version: None,
            cli_path: None,
        });
        assert_eq!(menu.summary, "mailtriage: mailtriage could not start: x");
        assert_eq!(
            menu.entries[1],
            Entry::Item {
                label: "Show details".into(),
                action: Action::CopyDetails(details.text())
            }
        );
    }
}
