//! Date ranges and interval buckets, PostHog conventions, always UTC.

use chrono::{DateTime, Datelike, Duration, Months, NaiveDate, NaiveDateTime, TimeZone, Utc};

use crate::contract::common::Interval;

use super::ExploreError;

/// Most buckets a series may have. Auto intervals widen to stay under it.
pub const MAX_BUCKETS: usize = 1_000;
/// Largest relative offset accepted (`-<n>d` etc.), to keep arithmetic safe.
const MAX_RELATIVE_AMOUNT: u32 = 100_000;
/// Longest date string accepted.
const MAX_DATE_LENGTH: usize = 64;

/// A resolved half-open range `[from, to)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedRange {
    /// `None` for `"all"`: the caller substitutes the oldest stored event.
    pub from: Option<DateTime<Utc>>,
    pub to: DateTime<Utc>,
}

fn invalid(field: &'static str) -> ExploreError {
    ExploreError::Invalid {
        field,
        message: "Unrecognized date. Use -24h, -7d, -4w, -3m, -1y, dStart, mStart, yStart, all, or an ISO 8601 date.",
    }
}

fn start_of_day(at: DateTime<Utc>) -> DateTime<Utc> {
    Utc.from_utc_datetime(&at.date_naive().and_hms_opt(0, 0, 0).unwrap_or_default())
}

fn day_start(day: NaiveDate) -> DateTime<Utc> {
    Utc.from_utc_datetime(&day.and_hms_opt(0, 0, 0).unwrap_or_default())
}

/// `-<n><unit>` relative to `now`. Hours keep the hour; larger units snap to
/// the start of the day, as PostHog does.
fn relative(text: &str, now: DateTime<Utc>) -> Option<(DateTime<Utc>, bool)> {
    let body = text.strip_prefix('-')?;
    let unit = body.chars().last()?;
    let amount: u32 = body[..body.len() - unit.len_utf8()].parse().ok()?;
    if amount > MAX_RELATIVE_AMOUNT {
        return None;
    }
    let shifted = match unit {
        'h' => {
            let at = now.checked_sub_signed(Duration::hours(i64::from(amount)))?;
            return Some((at, false));
        }
        'd' => now.checked_sub_signed(Duration::days(i64::from(amount)))?,
        'w' => now.checked_sub_signed(Duration::weeks(i64::from(amount)))?,
        'm' => now.checked_sub_months(Months::new(amount))?,
        'y' => now.checked_sub_months(Months::new(amount.checked_mul(12)?))?,
        _ => return None,
    };
    Some((start_of_day(shifted), true))
}

/// An absolute date or datetime. Returns the instant and whether it was a
/// bare date.
fn absolute(text: &str) -> Option<(DateTime<Utc>, bool)> {
    if let Ok(day) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return Some((day_start(day), true));
    }
    if let Ok(at) = DateTime::parse_from_rfc3339(text) {
        return Some((at.with_timezone(&Utc), false));
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
    ] {
        if let Ok(at) = NaiveDateTime::parse_from_str(text, format) {
            return Some((Utc.from_utc_datetime(&at), false));
        }
    }
    None
}

fn named(text: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let today = now.date_naive();
    match text {
        "dStart" => Some(day_start(today)),
        "mStart" => Some(day_start(today.with_day(1)?)),
        "yStart" => Some(day_start(NaiveDate::from_ymd_opt(today.year(), 1, 1)?)),
        _ => None,
    }
}

/// Resolve `date_from`/`date_to` against `now`.
pub fn resolve(
    date_from: &str,
    date_to: Option<&str>,
    now: DateTime<Utc>,
) -> Result<ResolvedRange, ExploreError> {
    let date_from = date_from.trim();
    if date_from.len() > MAX_DATE_LENGTH {
        return Err(invalid("date_from"));
    }
    let from = if date_from == "all" {
        None
    } else if let Some(at) = named(date_from, now) {
        Some(at)
    } else if let Some((at, _)) = relative(date_from, now) {
        Some(at)
    } else if let Some((at, _)) = absolute(date_from) {
        Some(at)
    } else {
        return Err(invalid("date_from"));
    };

    let to = match date_to.map(str::trim).filter(|text| !text.is_empty()) {
        None => now,
        Some(text) if text.len() > MAX_DATE_LENGTH => return Err(invalid("date_to")),
        Some(text) => {
            if let Some(at) = named(text, now) {
                at
            } else if let Some((at, whole_day)) = relative(text, now).or_else(|| absolute(text)) {
                // A bare day (or a day-snapped relative date) includes that
                // whole day.
                if whole_day {
                    at.checked_add_signed(Duration::days(1))
                        .ok_or_else(|| invalid("date_to"))?
                } else {
                    at
                }
            } else {
                return Err(invalid("date_to"));
            }
        }
    };
    if let Some(from) = from
        && from >= to
    {
        return Err(ExploreError::Invalid {
            field: "date_from",
            message: "date_from must be before date_to.",
        });
    }
    Ok(ResolvedRange { from, to })
}

/// DuckDB `date_trunc` unit of an interval.
pub fn unit(interval: Interval) -> &'static str {
    match interval {
        Interval::Hour => "hour",
        Interval::Day => "day",
        Interval::Week => "week",
        Interval::Month => "month",
    }
}

/// Start of the bucket holding `at` (weeks start on Monday).
pub fn truncate(at: DateTime<Utc>, interval: Interval) -> DateTime<Utc> {
    let day = at.date_naive();
    match interval {
        Interval::Hour => {
            let seconds = at.timestamp().div_euclid(3_600) * 3_600;
            Utc.timestamp_opt(seconds, 0).single().unwrap_or(at)
        }
        Interval::Day => day_start(day),
        Interval::Week => {
            day_start(day - Duration::days(i64::from(day.weekday().num_days_from_monday())))
        }
        Interval::Month => day_start(day.with_day(1).unwrap_or(day)),
    }
}

fn next(at: DateTime<Utc>, interval: Interval) -> Option<DateTime<Utc>> {
    match interval {
        Interval::Hour => at.checked_add_signed(Duration::hours(1)),
        Interval::Day => at.checked_add_signed(Duration::days(1)),
        Interval::Week => at.checked_add_signed(Duration::weeks(1)),
        Interval::Month => at.checked_add_months(Months::new(1)),
    }
}

/// Bucket starts covering `[from, to)`, or `None` past [`MAX_BUCKETS`].
pub fn buckets(
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    interval: Interval,
) -> Option<Vec<DateTime<Utc>>> {
    let mut out = Vec::new();
    let mut at = truncate(from, interval);
    while at < to {
        if out.len() == MAX_BUCKETS {
            return None;
        }
        out.push(at);
        at = next(at, interval)?;
    }
    Some(out)
}

/// The requested interval, or the finest sensible one: hourly up to two
/// days, then daily, widening until the bucket count fits.
pub fn choose_interval(
    requested: Option<Interval>,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<(Interval, Vec<DateTime<Utc>>), ExploreError> {
    if let Some(interval) = requested {
        return buckets(from, to, interval)
            .map(|days| (interval, days))
            .ok_or(ExploreError::Invalid {
                field: "interval",
                message: "Too many buckets for this interval; choose a wider one.",
            });
    }
    let first = if to - from <= Duration::hours(48) {
        Interval::Hour
    } else {
        Interval::Day
    };
    for interval in [first, Interval::Day, Interval::Week, Interval::Month] {
        if let Some(days) = buckets(from, to, interval) {
            return Ok((interval, days));
        }
    }
    Err(ExploreError::Invalid {
        field: "date_from",
        message: "The date range is too long.",
    })
}

/// First and exclusive-last UTC day touched by `[from, to)`.
pub fn day_span(from: DateTime<Utc>, to: DateTime<Utc>) -> (NaiveDate, NaiveDate) {
    let last = (to - Duration::microseconds(1)).date_naive();
    (from.date_naive(), last.succ_opt().unwrap_or(last))
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
    fn resolves_posthog_forms() {
        let now = at("2026-03-15T13:45:00Z");
        let range = resolve("-7d", None, now).unwrap();
        assert_eq!(range.from, Some(at("2026-03-08T00:00:00Z")));
        assert_eq!(range.to, now);
        assert_eq!(
            resolve("-24h", None, now).unwrap().from,
            Some(at("2026-03-14T13:45:00Z"))
        );
        assert_eq!(
            resolve("mStart", None, now).unwrap().from,
            Some(at("2026-03-01T00:00:00Z"))
        );
        assert_eq!(
            resolve("-1m", None, now).unwrap().from,
            Some(at("2026-02-15T00:00:00Z"))
        );
        let range = resolve("2026-03-01", Some("2026-03-02"), now).unwrap();
        assert_eq!(range.from, Some(at("2026-03-01T00:00:00Z")));
        assert_eq!(range.to, at("2026-03-03T00:00:00Z"));
        assert_eq!(resolve("all", None, now).unwrap().from, None);
        assert!(resolve("yesterday-ish", None, now).is_err());
        assert!(resolve("-999999999d", None, now).is_err());
        assert!(resolve("2026-03-20", None, now).is_err());
    }

    #[test]
    fn buckets_are_aligned_and_bounded() {
        let days = buckets(
            at("2026-03-08T05:00:00Z"),
            at("2026-03-10T00:00:00Z"),
            Interval::Day,
        )
        .unwrap();
        assert_eq!(
            days,
            vec![at("2026-03-08T00:00:00Z"), at("2026-03-09T00:00:00Z")]
        );
        let weeks = buckets(
            at("2026-03-11T00:00:00Z"),
            at("2026-03-17T00:00:00Z"),
            Interval::Week,
        )
        .unwrap();
        assert_eq!(
            weeks,
            vec![at("2026-03-09T00:00:00Z"), at("2026-03-16T00:00:00Z")]
        );
        assert!(
            buckets(
                at("2000-01-01T00:00:00Z"),
                at("2026-01-01T00:00:00Z"),
                Interval::Hour
            )
            .is_none()
        );
        let (interval, _) =
            choose_interval(None, at("2000-01-01T00:00:00Z"), at("2026-01-01T00:00:00Z")).unwrap();
        assert_eq!(interval, Interval::Month);
    }
}
