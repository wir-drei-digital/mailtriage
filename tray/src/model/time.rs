//! Times as a person reads them.
use chrono::{DateTime, FixedOffset, Utc};

/// Local `HH:MM`, with the date when `at` is not today.
pub fn clock(at: DateTime<Utc>, now: DateTime<FixedOffset>) -> String {
    let local = at.with_timezone(now.offset());
    if local.date_naive() == now.date_naive() {
        local.format("%H:%M").to_string()
    } else {
        local.format("%b %-d, %H:%M").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn today_is_the_time_and_other_days_add_the_date() {
        let now = DateTime::parse_from_rfc3339("2026-10-06T12:30:00+02:00").unwrap();
        assert_eq!(clock(at("2026-10-06T10:03:00Z"), now), "12:03");
        assert_eq!(clock(at("2026-10-05T21:59:00Z"), now), "Oct 5, 23:59");
        // Midnight local time belongs to the local day, not the UTC one.
        assert_eq!(clock(at("2026-10-05T22:00:00Z"), now), "00:00");
    }
}
