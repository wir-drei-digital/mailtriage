//! Refreshing the release information: a reservation, the release list,
//! then the result or the failure recorded in the cache.
use super::{
    cache::{Cache, ErrorRecord},
    github::Net,
    release::{self, CachedRelease},
    schedule, COMPONENTS,
};
use anyhow::{anyhow, Result};
use chrono::Utc;

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
