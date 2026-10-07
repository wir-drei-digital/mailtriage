//! Health per account from `service status`, the tray's own observations,
//! and the summary across accounts.
use crate::cli::{LastPass, ServiceInfo};
use chrono::{DateTime, Duration, Utc};
use std::collections::BTreeMap;

/// How long a job may be seen not running before it is a problem.
pub const RESTART_GRACE: Duration = Duration::minutes(2);

/// The health states, in the order the spec's table checks them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Unavailable,
    NotInstalled,
    OtherConfig,
    OtherConfigUnknown,
    Stopped,
    Unknown,
    Restarting,
    Error,
    Warning,
    Starting,
    Ok,
}

/// Why an account is in `State::Warning`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Warning {
    SomeMailSkipped,
    ConfigurationChanged,
    AnotherBusy,
    NoCheckSince,
    NoCheckYet,
}

/// One account's health.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Health {
    pub state: State,
    pub warning: Option<Warning>,
    /// When the problem started, or the last check for `Ok` and
    /// `NoCheckSince`.
    pub since: Option<DateTime<Utc>>,
    /// The job runs although its manager will not start it again.
    pub disabled_running: bool,
}

impl Health {
    fn of(state: State) -> Self {
        Self {
            state,
            warning: None,
            since: None,
            disabled_running: false,
        }
    }
}

/// What the tray has seen of one account since it started.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Observation {
    /// When the tray first saw the job not running (reset when it runs).
    pub not_running_since: Option<DateTime<Utc>>,
    /// The current PID and when the tray first saw it.
    pub pid: Option<u32>,
    pub pid_since: Option<DateTime<Utc>>,
}

/// Observations per account, kept in memory; a tray restart resets them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Observations(BTreeMap<String, Observation>);

impl Observations {
    pub fn observe(&mut self, services: &[ServiceInfo], now: DateTime<Utc>) {
        for s in services {
            let seen = self.0.entry(s.account.clone()).or_default();
            if s.running {
                seen.not_running_since = None;
                if seen.pid_since.is_none() || seen.pid != s.pid {
                    seen.pid = s.pid;
                    seen.pid_since = Some(now);
                }
            } else {
                seen.not_running_since.get_or_insert(now);
                seen.pid = None;
                seen.pid_since = None;
            }
        }
        self.0
            .retain(|name, _| services.iter().any(|s| &s.account == name));
    }

    pub fn get(&self, account: &str) -> Observation {
        self.0.get(account).cloned().unwrap_or_default()
    }
}

/// How old the last check may be: 3 × the interval plus 30 minutes. A
/// hand-edited interval too large for that saturates at `Duration::MAX`.
pub fn stale_after(s: &ServiceInfo) -> Duration {
    let seconds = s.interval_seconds.unwrap_or(60).saturating_mul(3);
    i64::try_from(seconds)
        .ok()
        .and_then(Duration::try_seconds)
        .and_then(|d| d.checked_add(&Duration::minutes(30)))
        .unwrap_or(Duration::MAX)
}

fn pass_warning(pass: &LastPass) -> Option<Warning> {
    match (pass.exit_code, pass.reason.as_deref()) {
        (4, _) => Some(Warning::SomeMailSkipped),
        (5, Some("config_changed")) => Some(Warning::ConfigurationChanged),
        (5, Some("account_busy")) => Some(Warning::AnotherBusy),
        _ => None,
    }
}

/// A pass that is neither clean, partial, nor one of the exit-5 reasons
/// `watch` survives.
fn pass_failed(pass: &LastPass) -> bool {
    pass.exit_code != 0 && pass_warning(pass).is_none()
}

/// The first matching row of the spec's health table.
pub fn health(s: &ServiceInfo, seen: &Observation, now: DateTime<Utc>) -> Health {
    if s.manager == "none" {
        return Health::of(State::Unavailable);
    }
    if !s.installed {
        return Health::of(State::NotInstalled);
    }
    match s.config_matches {
        Some(false) => return Health::of(State::OtherConfig),
        None => return Health::of(State::OtherConfigUnknown),
        Some(true) => {}
    }
    let pass = s.last_pass.as_ref();
    let finished = pass.map(|p| p.finished_at.with_timezone(&Utc));
    if !s.running {
        match s.enabled {
            Some(false) => return Health::of(State::Stopped),
            None => return Health::of(State::Unknown),
            Some(true) => {}
        }
        let since = seen.not_running_since.unwrap_or(now);
        let state = if now - since < RESTART_GRACE {
            State::Restarting
        } else {
            State::Error
        };
        return Health {
            since: Some(since),
            ..Health::of(state)
        };
    }
    let running = |state, warning, since| Health {
        state,
        warning,
        since,
        disabled_running: s.enabled == Some(false),
    };
    let Some(pass) = pass else {
        let seen_since = seen.pid_since.unwrap_or(now);
        return if now - seen_since > stale_after(s) {
            running(State::Warning, Some(Warning::NoCheckYet), None)
        } else {
            running(State::Starting, None, None)
        };
    };
    if pass_failed(pass) {
        return running(State::Error, None, finished);
    }
    if let Some(warning) = pass_warning(pass) {
        return running(State::Warning, Some(warning), finished);
    }
    if finished.is_some_and(|at| now - at > stale_after(s)) {
        return running(State::Warning, Some(Warning::NoCheckSince), finished);
    }
    running(State::Ok, None, finished)
}

/// Whether the last pass failed (for the error's detail line).
pub fn last_pass_failed(s: &ServiceInfo) -> bool {
    s.last_pass.as_ref().is_some_and(pass_failed)
}

/// How much a state needs a person, for the icon and the summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Off,
    Fine,
    Warning,
    Error,
}

pub fn severity(state: State) -> Severity {
    match state {
        State::Error | State::Unknown | State::OtherConfigUnknown => Severity::Error,
        State::Warning => Severity::Warning,
        State::Ok | State::Starting | State::Restarting => Severity::Fine,
        State::Stopped | State::NotInstalled | State::OtherConfig | State::Unavailable => {
            Severity::Off
        }
    }
}

/// The tray icon: the worst state across accounts, "off" when no account
/// runs or needs attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconState {
    Ok,
    Warning,
    Error,
    Off,
}

pub fn icon(states: &[State]) -> IconState {
    match states.iter().map(|s| severity(*s)).max() {
        Some(Severity::Error) => IconState::Error,
        Some(Severity::Warning) => IconState::Warning,
        Some(Severity::Fine) => IconState::Ok,
        Some(Severity::Off) | None => IconState::Off,
    }
}

/// The summary line after "mailtriage: ": who needs attention; else that
/// no service manager exists; else, when nothing runs for this config but
/// accounts run another one, which; else how many accounts run.
pub fn summary(accounts: &[(String, State)]) -> String {
    let attention: Vec<&str> = accounts
        .iter()
        .filter(|(_, s)| severity(*s) >= Severity::Warning)
        .map(|(name, _)| name.as_str())
        .collect();
    match attention.as_slice() {
        [] => {}
        [one] => return format!("{one} needs attention"),
        [a, b] => return format!("{a} and {b} need attention"),
        many => return format!("{} accounts need attention", many.len()),
    }
    if !accounts.is_empty() && accounts.iter().all(|(_, s)| *s == State::Unavailable) {
        return "No background service on this system".into();
    }
    let running = accounts
        .iter()
        .filter(|(_, s)| severity(*s) == Severity::Fine)
        .count();
    // Their jobs run, for another config: not "Stopped".
    let other: Vec<&str> = accounts
        .iter()
        .filter(|(_, s)| *s == State::OtherConfig)
        .map(|(name, _)| name.as_str())
        .collect();
    if running == 0 && !other.is_empty() {
        return match other.as_slice() {
            [_] if accounts.len() == 1 => "Runs another config".into(),
            _ if other.len() == accounts.len() => "Every account runs another config".into(),
            [one] => format!("{one} runs another config"),
            many => format!("{} accounts run another config", many.len()),
        };
    }
    match running {
        0 => "Stopped".into(),
        n if n == accounts.len() => "All accounts running".into(),
        n => format!("{n} of {} accounts running", accounts.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::FilingMode;
    use chrono::DateTime;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-06T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn minutes_ago(m: i64) -> DateTime<Utc> {
        now() - Duration::minutes(m)
    }

    fn pass(finished: DateTime<Utc>, exit_code: i64, reason: Option<&str>) -> LastPass {
        LastPass {
            finished_at: finished.fixed_offset(),
            partial: exit_code == 4,
            exit_code,
            mode: "live".into(),
            reason: reason.map(str::to_owned),
            version: None,
        }
    }

    /// An installed, enabled, running service with a clean pass a minute ago.
    fn running() -> ServiceInfo {
        ServiceInfo {
            account: "daniel".into(),
            manager: "launchd".into(),
            installed: true,
            running: true,
            pid: Some(4242),
            unit_path: None,
            log_paths: vec![],
            last_pass: Some(pass(minutes_ago(1), 0, None)),
            service_config: None,
            file_config: None,
            needs_daemon_reload: None,
            config_matches: Some(true),
            enabled: Some(true),
            enablement: "enabled".into(),
            interval_seconds: Some(60),
            filing_mode: FilingMode::Live,
            identity: "daniel".into(),
            update: None,
        }
    }

    fn stopped() -> ServiceInfo {
        ServiceInfo {
            running: false,
            pid: None,
            ..running()
        }
    }

    /// Seen running with this PID since `pid_since`, or not running since
    /// `not_running_since`.
    fn seen(
        not_running_since: Option<DateTime<Utc>>,
        pid_since: Option<DateTime<Utc>>,
    ) -> Observation {
        Observation {
            not_running_since,
            pid: pid_since.map(|_| 4242),
            pid_since,
        }
    }

    fn state(s: &ServiceInfo, o: &Observation) -> (State, Option<Warning>) {
        let h = health(s, o, now());
        (h.state, h.warning)
    }

    #[test]
    fn each_row_of_the_table() {
        let fresh = seen(None, Some(minutes_ago(1)));
        let none = ServiceInfo {
            manager: "none".into(),
            installed: false,
            ..stopped()
        };
        assert_eq!(state(&none, &fresh).0, State::Unavailable);
        let missing = ServiceInfo {
            installed: false,
            ..stopped()
        };
        assert_eq!(state(&missing, &fresh).0, State::NotInstalled);
        let other = ServiceInfo {
            config_matches: Some(false),
            ..running()
        };
        assert_eq!(state(&other, &fresh).0, State::OtherConfig);
        let unknown_config = ServiceInfo {
            config_matches: None,
            ..running()
        };
        assert_eq!(state(&unknown_config, &fresh).0, State::OtherConfigUnknown);
        // Stopped wins over an old error.
        let off = ServiceInfo {
            enabled: Some(false),
            last_pass: Some(pass(minutes_ago(90), 3, None)),
            ..stopped()
        };
        assert_eq!(state(&off, &fresh).0, State::Stopped);
        let unknown = ServiceInfo {
            enabled: None,
            ..stopped()
        };
        assert_eq!(state(&unknown, &fresh).0, State::Unknown);
        assert_eq!(
            state(&stopped(), &seen(Some(minutes_ago(1)), None)).0,
            State::Restarting
        );
        assert_eq!(
            state(&stopped(), &seen(Some(minutes_ago(2)), None)).0,
            State::Error
        );
        let starting = ServiceInfo {
            last_pass: None,
            ..running()
        };
        assert_eq!(state(&starting, &fresh).0, State::Starting);
        assert_eq!(state(&running(), &fresh), (State::Ok, None));
    }

    #[test]
    fn restarting_turns_into_an_error_after_two_minutes() {
        let mut seen_by_tray = Observations::default();
        seen_by_tray.observe(&[stopped()], now() - Duration::seconds(119));
        let h = health(&stopped(), &seen_by_tray.get("daniel"), now());
        assert_eq!(h.state, State::Restarting);
        seen_by_tray.observe(&[stopped()], now());
        let h = health(
            &stopped(),
            &seen_by_tray.get("daniel"),
            now() + Duration::seconds(1),
        );
        assert_eq!(h.state, State::Error);
        assert_eq!(h.since, Some(now() - Duration::seconds(119)));
        // Running again resets the observation.
        seen_by_tray.observe(&[running()], now());
        assert_eq!(seen_by_tray.get("daniel").not_running_since, None);
    }

    #[test]
    fn exit_5_reasons_are_warnings_or_errors() {
        let fresh = seen(None, Some(minutes_ago(1)));
        for (code, reason, expected) in [
            (4, None, (State::Warning, Some(Warning::SomeMailSkipped))),
            (
                5,
                Some("config_changed"),
                (State::Warning, Some(Warning::ConfigurationChanged)),
            ),
            (
                5,
                Some("account_busy"),
                (State::Warning, Some(Warning::AnotherBusy)),
            ),
            (5, Some("binding_conflict"), (State::Error, None)),
            (5, None, (State::Error, None)),
            (3, None, (State::Error, None)),
            (2, None, (State::Error, None)),
        ] {
            let s = ServiceInfo {
                last_pass: Some(pass(minutes_ago(1), code, reason)),
                ..running()
            };
            assert_eq!(state(&s, &fresh), expected, "{code} {reason:?}");
        }
    }

    #[test]
    fn staleness_exactly_at_and_past_the_limit() {
        let fresh = seen(None, Some(minutes_ago(1)));
        // 3 × 60 s + 30 min = 33 minutes.
        let at = ServiceInfo {
            last_pass: Some(pass(minutes_ago(33), 0, None)),
            ..running()
        };
        assert_eq!(state(&at, &fresh), (State::Ok, None));
        let past = ServiceInfo {
            last_pass: Some(pass(minutes_ago(33) - Duration::seconds(1), 0, None)),
            ..running()
        };
        assert_eq!(
            state(&past, &fresh),
            (State::Warning, Some(Warning::NoCheckSince))
        );
    }

    /// A hand-edited, absurd `interval_seconds` neither overflows nor
    /// panics: the check is just never stale.
    #[test]
    fn a_huge_interval_saturates() {
        // Past u64, past i64, past chrono's range, and at its very end.
        let at_the_end = (i64::MAX / 1000 / 3) as u64;
        for interval in [u64::MAX, u64::MAX / 3, 1 << 62, 1 << 58, at_the_end] {
            let mut s = running();
            s.interval_seconds = Some(interval);
            assert_eq!(stale_after(&s), Duration::MAX, "{interval}");
            let seen_long_ago = seen(None, Some(minutes_ago(600)));
            s.last_pass = Some(pass(minutes_ago(60 * 24 * 365), 0, None));
            assert_eq!(state(&s, &seen_long_ago).0, State::Ok, "{interval}");
            s.last_pass = None;
            assert_eq!(state(&s, &seen_long_ago).0, State::Starting, "{interval}");
        }
        let mut s = running();
        s.interval_seconds = Some(600);
        assert_eq!(stale_after(&s), Duration::minutes(60));
    }

    #[test]
    fn no_pass_yet_short_and_long_after_the_pid_appeared() {
        let s = ServiceInfo {
            last_pass: None,
            ..running()
        };
        assert_eq!(
            state(&s, &seen(None, Some(minutes_ago(33)))),
            (State::Starting, None)
        );
        assert_eq!(
            state(&s, &seen(None, Some(minutes_ago(34)))),
            (State::Warning, Some(Warning::NoCheckYet))
        );
        // A new PID starts the clock again.
        let mut o = Observations::default();
        o.observe(std::slice::from_ref(&s), minutes_ago(40));
        let restarted = ServiceInfo {
            pid: Some(5000),
            ..s.clone()
        };
        o.observe(std::slice::from_ref(&restarted), minutes_ago(1));
        assert_eq!(state(&restarted, &o.get("daniel")), (State::Starting, None));
    }

    #[test]
    fn a_running_job_that_is_disabled_keeps_its_runtime_state() {
        let s = ServiceInfo {
            enabled: Some(false),
            ..running()
        };
        let h = health(&s, &seen(None, Some(minutes_ago(1))), now());
        assert_eq!(h.state, State::Ok);
        assert!(h.disabled_running);
    }

    #[test]
    fn the_icon_and_summary_show_the_worst_state() {
        use State::*;
        assert_eq!(icon(&[Ok, Warning, Starting]), IconState::Warning);
        assert_eq!(icon(&[Ok, Error]), IconState::Error);
        assert_eq!(icon(&[Ok, OtherConfigUnknown]), IconState::Error);
        assert_eq!(icon(&[Ok, Stopped]), IconState::Ok);
        assert_eq!(icon(&[Restarting]), IconState::Ok);
        assert_eq!(
            icon(&[Stopped, NotInstalled, OtherConfig, Unavailable]),
            IconState::Off
        );
        let named = |list: &[(&str, State)]| {
            summary(
                &list
                    .iter()
                    .map(|(n, s)| (n.to_string(), *s))
                    .collect::<Vec<_>>(),
            )
        };
        assert_eq!(
            named(&[("daniel", Ok), ("info", Starting)]),
            "All accounts running"
        );
        assert_eq!(
            named(&[("daniel", Warning), ("info", Ok)]),
            "daniel needs attention"
        );
        assert_eq!(
            named(&[("daniel", Error), ("info", Unknown)]),
            "daniel and info need attention"
        );
        assert_eq!(
            named(&[("a", Error), ("b", Error), ("c", Warning)]),
            "3 accounts need attention"
        );
        assert_eq!(
            named(&[("daniel", Stopped), ("info", NotInstalled)]),
            "Stopped"
        );
        assert_eq!(
            named(&[("daniel", Ok), ("info", NotInstalled)]),
            "1 of 2 accounts running"
        );
        assert_eq!(
            named(&[("daniel", Unavailable)]),
            "No background service on this system"
        );
    }

    /// An account whose job runs for another config is not "Stopped": when
    /// nothing runs for this config, the summary says which accounts run
    /// another one. The icon stays "off".
    #[test]
    fn the_summary_names_accounts_that_run_another_config() {
        use State::*;
        let named = |list: &[(&str, State)]| {
            summary(
                &list
                    .iter()
                    .map(|(n, s)| (n.to_string(), *s))
                    .collect::<Vec<_>>(),
            )
        };
        assert_eq!(named(&[("daniel", OtherConfig)]), "Runs another config");
        assert_eq!(
            named(&[("daniel", OtherConfig), ("info", OtherConfig)]),
            "Every account runs another config"
        );
        assert_eq!(
            named(&[("daniel", OtherConfig), ("info", Stopped)]),
            "daniel runs another config"
        );
        assert_eq!(
            named(&[("daniel", OtherConfig), ("info", NotInstalled)]),
            "daniel runs another config"
        );
        assert_eq!(
            named(&[("a", OtherConfig), ("b", OtherConfig), ("c", Stopped)]),
            "2 accounts run another config"
        );
        // A running account and attention keep their lines.
        assert_eq!(
            named(&[("daniel", OtherConfig), ("info", Ok)]),
            "1 of 2 accounts running"
        );
        assert_eq!(
            named(&[("daniel", OtherConfig), ("info", Error)]),
            "info needs attention"
        );
        assert_eq!(icon(&[OtherConfig]), IconState::Off);
        assert_eq!(icon(&[OtherConfig, Stopped]), IconState::Off);
    }
}
