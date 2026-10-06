//! When to check and when to retry an install: reservations, the daily
//! check with jitter, and capped exponential backoff. Pure functions; the
//! callers pass the time and the random draws.
use chrono::{DateTime, Duration, SecondsFormat, Utc};

/// A check or install reserves this long before its request, so a process
/// that dies during the request does not retry sooner after a restart.
pub fn reservation(now: DateTime<Utc>) -> DateTime<Utc> {
    now + Duration::hours(1)
}

/// After a successful check: 24 h plus `jitter_minutes` (0 to 60).
pub fn after_success(now: DateTime<Utc>, jitter_minutes: i64) -> DateTime<Utc> {
    now + Duration::hours(24) + Duration::minutes(jitter_minutes.clamp(0, 60))
}

/// After the `failures`-th failed check in a row: 1 h × 2^(failures − 1),
/// at most 24 h, scaled by `1 + jitter` (jitter −0.1 to 0.1); later when
/// `Retry-After` or the rate-limit reset says so (each capped at 24 h).
pub fn after_failure(
    now: DateTime<Utc>,
    failures: u32,
    jitter: f64,
    retry_after: Option<DateTime<Utc>>,
    rate_reset: Option<DateTime<Utc>>,
) -> DateTime<Utc> {
    let doubled = 60i64 << failures.clamp(1, 6).saturating_sub(1);
    let minutes = doubled.min(24 * 60) as f64 * (1.0 + jitter.clamp(-0.1, 0.1));
    let backoff = now + Duration::seconds((minutes * 60.0).round() as i64);
    let cap = now + Duration::hours(24);
    [retry_after, rate_reset]
        .into_iter()
        .flatten()
        .map(|hint| hint.min(cap))
        .fold(backoff, DateTime::max)
}

/// After the `failures`-th failed install in a row: 1 h, doubling, at most 24 h.
pub fn install_backoff(now: DateTime<Utc>, failures: u32) -> DateTime<Utc> {
    let doubled = 1i64 << failures.clamp(1, 6).saturating_sub(1);
    now + Duration::hours(doubled.min(24))
}

/// Whether the time `at` (RFC 3339; missing or unreadable means now) has come.
pub fn due(at: Option<&str>, now: DateTime<Utc>) -> bool {
    at.and_then(parse).is_none_or(|at| at <= now)
}

/// Timestamps in `update.json` and in output: RFC 3339, seconds, `Z`.
pub fn stamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub fn parse(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

/// A random number in 0..1 for jitter, from the UUID generator mailtriage
/// already uses.
pub fn random_unit() -> f64 {
    (uuid::Uuid::new_v4().as_u128() >> 75) as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 3, 8, 0, 0).unwrap()
    }

    #[test]
    fn success_waits_a_day_plus_up_to_an_hour() {
        assert_eq!(after_success(now(), 0), now() + Duration::hours(24));
        assert_eq!(after_success(now(), 60), now() + Duration::hours(25));
        assert_eq!(after_success(now(), 600), now() + Duration::hours(25));
        for _ in 0..100 {
            let at = after_success(now(), (random_unit() * 61.0) as i64);
            assert!(at >= now() + Duration::hours(24) && at <= now() + Duration::hours(25));
        }
    }

    #[test]
    fn failures_double_from_an_hour_up_to_a_day() {
        let hours =
            |failures| (after_failure(now(), failures, 0.0, None, None) - now()).num_minutes() / 60;
        assert_eq!(
            [1, 2, 3, 4, 5, 6, 7, 50].map(hours),
            [1, 2, 4, 8, 16, 24, 24, 24]
        );
        let low = after_failure(now(), 1, -0.1, None, None) - now();
        let high = after_failure(now(), 1, 0.1, None, None) - now();
        assert_eq!((low.num_minutes(), high.num_minutes()), (54, 66));
        let capped = after_failure(now(), 9, 0.1, None, None) - now();
        assert_eq!(capped.num_minutes(), 24 * 66);
    }

    #[test]
    fn server_hints_win_when_later() {
        let later = now() + Duration::hours(5);
        let sooner = now() + Duration::minutes(5);
        assert_eq!(after_failure(now(), 1, 0.0, Some(later), None), later);
        assert_eq!(after_failure(now(), 1, 0.0, None, Some(later)), later);
        assert_eq!(
            after_failure(now(), 1, 0.0, Some(sooner), Some(sooner)),
            now() + Duration::hours(1)
        );
        let far = now() + Duration::days(30);
        assert_eq!(
            after_failure(now(), 1, 0.0, Some(far), None),
            now() + Duration::hours(24)
        );
    }

    #[test]
    fn install_backoff_doubles_and_is_capped() {
        let hours = |failures| (install_backoff(now(), failures) - now()).num_hours();
        assert_eq!([1, 2, 3, 4, 5, 6, 30].map(hours), [1, 2, 4, 8, 16, 24, 24]);
    }

    #[test]
    fn reservations_and_due_times() {
        assert_eq!(reservation(now()), now() + Duration::hours(1));
        assert!(due(None, now()));
        assert!(due(Some("garbage"), now()));
        assert!(due(Some(&stamp(now())), now()));
        assert!(!due(Some(&stamp(now() + Duration::seconds(1))), now()));
        assert_eq!(stamp(now()), "2026-11-03T08:00:00Z");
        assert_eq!(parse("2026-11-03T08:00:00Z"), Some(now()));
    }
}
