//! Refreshing the release information: a reservation, the release list,
//! then the result or the failure recorded in the cache.
use super::{
    cache::{Cache, CacheFile, ErrorRecord},
    github::Net,
    release::{self, CachedRelease},
    schedule, COMPONENTS,
};
use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};

/// Whether the reservation must be written before the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reservation {
    /// `watch`: without a written reservation there is no request.
    Required,
    /// `update`: a cache that cannot be written is a warning.
    BestEffort,
}

/// A successful check.
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    /// The highest stable release, `None` when there is none.
    pub release: Option<CachedRelease>,
    pub checked_at: String,
    /// Cache writes that failed, each starting with `cannot write the
    /// update cache`: the reservation (`BestEffort` only) and the record of
    /// the result (either mode).
    pub warnings: Vec<String>,
}

/// Whether `watch` refreshes the release information now: when
/// `next_check_at` has come or is missing, and sooner when the cached
/// release was recorded by a version that did not know every component
/// (a `release.archives` key is missing, which means unknown, not "no
/// archive"). That early refresh waits for a failed check's backoff and
/// for a reservation, which is at most an hour ahead, while a successful
/// check schedules the next one a day ahead. So a watcher that cannot
/// reach GitHub asks once, not at every pass.
pub fn due(file: &CacheFile, now: DateTime<Utc>) -> bool {
    let next = file.next_check_at.as_deref();
    if schedule::due(next, now) {
        return true;
    }
    let unknown = file
        .release
        .as_ref()
        .is_some_and(|r| COMPONENTS.iter().any(|c| !r.knows(*c)));
    unknown
        && file.check_failures == 0
        && next
            .and_then(schedule::parse)
            .is_some_and(|at| at > schedule::reservation(now))
}

/// One refresh: `next_check_at = now + 1 h` first, then the release list.
/// Success stores `release`, schedules the next check in 24 h plus up to an
/// hour and clears the failures; failure counts it and backs off. Errors
/// are the check's message.
pub fn refresh(net: &Net, cache: &Cache, reservation: Reservation) -> Result<Checked> {
    let mut warnings = Vec::new();
    let reserved = cache.update(|c| {
        c.next_check_at = Some(schedule::stamp(schedule::reservation(Utc::now())));
    });
    if let Err(e) = reserved {
        let message = format!("cannot write the update cache: {e:#}");
        match reservation {
            Reservation::Required => return Err(anyhow!("{message}; no check was made")),
            Reservation::BestEffort => warnings.push(message),
        }
    }
    match net.releases() {
        Ok(list) => {
            let release = release::select(&list, release::platform(), COMPONENTS);
            let now = Utc::now();
            let checked_at = schedule::stamp(now);
            let jitter = (schedule::random_unit() * 61.0) as i64;
            let recorded = cache.update(|c| {
                c.release = release.clone();
                c.checked_at = Some(checked_at.clone());
                c.next_check_at = Some(schedule::stamp(schedule::after_success(now, jitter)));
                c.check_failures = 0;
                c.last_check_error = None;
            });
            if let Err(e) = recorded {
                warnings.push(format!(
                    "cannot write the update cache: {e:#}; the check's result was not recorded"
                ));
            }
            Ok(Checked {
                release,
                checked_at,
                warnings,
            })
        }
        Err(error) => {
            let now = Utc::now();
            let jitter = schedule::random_unit() * 0.2 - 0.1;
            let _ = cache.update(|c| {
                c.check_failures = c.check_failures.saturating_add(1);
                c.next_check_at = Some(schedule::stamp(schedule::after_failure(
                    now,
                    c.check_failures,
                    jitter,
                    error.retry_after,
                    error.rate_reset,
                )));
                c.last_check_error = Some(ErrorRecord {
                    at: schedule::stamp(now),
                    message: error.message.clone(),
                });
            });
            Err(anyhow!(error.message))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::{cache::CacheFile, CLI};
    use chrono::{DateTime, Duration, TimeZone};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 3, 8, 0, 0).unwrap()
    }

    /// A cache whose release has archive entries for `known` components.
    fn file(known: &[&str], next: Option<DateTime<Utc>>, failures: u32) -> CacheFile {
        CacheFile {
            release: Some(CachedRelease {
                version: "0.3.0".into(),
                release_url: "u".into(),
                published_at: None,
                archives: known.iter().map(|k| ((*k).to_owned(), None)).collect(),
                sums: None,
            }),
            next_check_at: next.map(schedule::stamp),
            check_failures: failures,
            ..CacheFile::default()
        }
    }

    /// R11-1: a release an older version recorded (no `mailtriage-tray`
    /// key) is refreshed before its daily deadline, but a reservation or a
    /// failed check's backoff still holds the next check off, so an offline
    /// watcher does not ask every pass.
    #[test]
    fn a_release_without_every_components_key_is_refreshed_early_once() {
        let both = [CLI.name, "mailtriage-tray"];
        let day = Some(now() + Duration::hours(24));
        assert!(!due(&file(&both, day, 0), now()));
        assert!(due(&file(&both, Some(now()), 0), now()));
        assert!(due(&file(&both, None, 0), now()));

        let older = [CLI.name];
        assert!(due(&file(&older, day, 0), now()), "the daily schedule");
        // A reservation: at most an hour ahead.
        for ahead in [Duration::minutes(30), Duration::hours(1)] {
            assert!(
                !due(&file(&older, Some(now() + ahead), 0), now()),
                "{ahead}"
            );
        }
        // A failed check's backoff, however long.
        assert!(!due(
            &file(&older, Some(now() + Duration::hours(8)), 1),
            now()
        ));
        assert!(due(&file(&older, Some(now()), 1), now()));
        // No release: nothing is unknown.
        let none = CacheFile {
            release: None,
            ..file(&older, day, 0)
        };
        assert!(!due(&none, now()));
    }
}
