//! Date ranges and interval buckets, PostHog conventions, always UTC.
//!
//! Everything is integer microseconds since the Unix epoch. A resolved range
//! is half-open: `[from, to)`. Buckets are aligned starts (hour, day, week
//! starting Monday, month) from the bucket containing `from` up to the last
//! bucket starting before `to`.

use chrono::{DateTime, Datelike, Months, NaiveDate, NaiveDateTime, SecondsFormat, TimeZone, Utc};

use super::QueryError;
use crate::contract::common::{DateRange, Interval};

pub const HOUR_US: i64 = 3_600_000_000;
pub const DAY_US: i64 = 24 * HOUR_US;
pub const WEEK_US: i64 = 7 * DAY_US;
/// 1969-12-29T00:00:00Z, a Monday: weeks are aligned to it.
pub const WEEK_ORIGIN_US: i64 = -3 * DAY_US;
/// Explicit bound on buckets per series (a year of hours fits).
pub const MAX_BUCKETS: usize = 10_000;
/// Explicit bound on relative offsets (`-Nd` etc.).
const MAX_RELATIVE: i64 = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResolvedRange {
    /// Inclusive start, µs.
    pub from: i64,
    /// Exclusive end, µs.
    pub to: i64,
    pub interval: Interval,
}

fn floor_to(us: i64, unit: i64, origin: i64) -> i64 {
    us - (us - origin).rem_euclid(unit)
}

pub fn to_datetime(us: i64) -> DateTime<Utc> {
    Utc.timestamp_micros(us)
        .single()
        .unwrap_or(DateTime::<Utc>::MIN_UTC)
}

pub fn from_datetime(time: DateTime<Utc>) -> i64 {
    time.timestamp_micros()
}

/// Start of the bucket containing `us`.
pub fn trunc(us: i64, interval: Interval) -> i64 {
    match interval {
        Interval::Hour => floor_to(us, HOUR_US, 0),
        Interval::Day => floor_to(us, DAY_US, 0),
        Interval::Week => floor_to(us, WEEK_US, WEEK_ORIGIN_US),
        Interval::Month => {
            let date = to_datetime(us).date_naive();
            let first = NaiveDate::from_ymd_opt(date.year(), date.month(), 1).unwrap_or(date);
            date_start_us(first)
        }
    }
}

/// Start of the bucket after the one starting at `start`.
pub fn next_bucket(start: i64, interval: Interval) -> i64 {
    match interval {
        Interval::Hour => start.saturating_add(HOUR_US),
        Interval::Day => start.saturating_add(DAY_US),
        Interval::Week => start.saturating_add(WEEK_US),
        Interval::Month => {
            let date = to_datetime(start).date_naive();
            date.checked_add_months(Months::new(1))
                .map(date_start_us)
                .unwrap_or(i64::MAX)
        }
    }
}

/// Start of the bucket before the one starting at `start`.
pub fn previous_bucket(start: i64, interval: Interval) -> i64 {
    match interval {
        Interval::Hour => start.saturating_sub(HOUR_US),
        Interval::Day => start.saturating_sub(DAY_US),
        Interval::Week => start.saturating_sub(WEEK_US),
        Interval::Month => {
            let date = to_datetime(start).date_naive();
            date.checked_sub_months(Months::new(1))
                .map(date_start_us)
                .unwrap_or(i64::MIN)
        }
    }
}

pub fn date_start_us(date: NaiveDate) -> i64 {
    date.and_hms_opt(0, 0, 0)
        .map(|time| time.and_utc().timestamp_micros())
        .unwrap_or(0)
}

/// UTC day of an instant.
pub fn day_of(us: i64) -> NaiveDate {
    to_datetime(us).date_naive()
}

pub fn rfc3339(us: i64) -> String {
    to_datetime(us).to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub fn rfc3339_micros(us: i64) -> String {
    to_datetime(us).to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// Parse a bucket start as given back in an actor selection.
pub fn parse_instant(text: &str) -> Option<i64> {
    if let Ok(time) = DateTime::parse_from_rfc3339(text) {
        return Some(time.with_timezone(&Utc).timestamp_micros());
    }
    for format in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"] {
        if let Ok(time) = NaiveDateTime::parse_from_str(text, format) {
            return Some(time.and_utc().timestamp_micros());
        }
    }
    NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .ok()
        .map(date_start_us)
}

pub fn label(us: i64, interval: Interval) -> String {
    let time = to_datetime(us);
    match interval {
        Interval::Hour => time.format("%-d %b %H:%M").to_string(),
        Interval::Day => time.format("%a %-d %b").to_string(),
        Interval::Week => time.format("%-d %b").to_string(),
        Interval::Month => time.format("%b %Y").to_string(),
    }
}

impl ResolvedRange {
    /// Bucket starts covering the range, bounded by [`MAX_BUCKETS`].
    pub fn buckets(&self) -> Result<Vec<i64>, QueryError> {
        let mut buckets = Vec::new();
        let mut start = trunc(self.from, self.interval);
        while start < self.to {
            if buckets.len() == MAX_BUCKETS {
                return Err(QueryError::invalid(format!(
                    "the date range has more than {MAX_BUCKETS} intervals; use a larger interval"
                )));
            }
            buckets.push(start);
            start = next_bucket(start, self.interval);
        }
        Ok(buckets)
    }

    /// The period of equal length right before this one.
    pub fn previous(&self) -> Self {
        let length = self.to - self.from;
        Self {
            from: self.from.saturating_sub(length),
            to: self.from,
            interval: self.interval,
        }
    }

    pub fn date_from(&self) -> String {
        rfc3339(self.from)
    }

    /// Inclusive end, RFC 3339 with microseconds.
    pub fn date_to(&self) -> String {
        rfc3339_micros(self.to - 1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Point {
    /// An exact instant the caller wrote down.
    Exact(i64),
    /// A calendar position; aligned to the hour/day boundary.
    Calendar(i64),
    /// `all`: from the first event.
    All,
}

fn parse_point(text: &str, now: DateTime<Utc>) -> Result<Point, QueryError> {
    let text = text.trim();
    let invalid = || QueryError::invalid(format!("invalid date `{text}`"));
    if text.len() > 64 {
        return Err(invalid());
    }
    match text {
        "all" => return Ok(Point::All),
        "dStart" => return Ok(Point::Calendar(from_datetime(now))),
        "mStart" => {
            let date = now.date_naive();
            let first =
                NaiveDate::from_ymd_opt(date.year(), date.month(), 1).ok_or_else(invalid)?;
            return Ok(Point::Calendar(date_start_us(first)));
        }
        "yStart" => {
            let first = NaiveDate::from_ymd_opt(now.year(), 1, 1).ok_or_else(invalid)?;
            return Ok(Point::Calendar(date_start_us(first)));
        }
        _ => {}
    }
    if let Some(rest) = text.strip_prefix('-')
        && let Some(unit) = rest.chars().last()
        && unit.is_ascii_alphabetic()
    {
        let amount: i64 = rest[..rest.len() - 1].parse().map_err(|_| invalid())?;
        if !(0..=MAX_RELATIVE).contains(&amount) {
            return Err(invalid());
        }
        let instant = match unit {
            'h' => now.checked_sub_signed(chrono::Duration::hours(amount)),
            'd' => now.checked_sub_signed(chrono::Duration::days(amount)),
            'w' => now.checked_sub_signed(chrono::Duration::weeks(amount)),
            'm' => now.checked_sub_months(Months::new(amount as u32)),
            'y' => now.checked_sub_months(Months::new((amount * 12) as u32)),
            _ => None,
        }
        .ok_or_else(invalid)?;
        return Ok(Point::Calendar(from_datetime(instant)));
    }
    if let Ok(date) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return Ok(Point::Calendar(date_start_us(date)));
    }
    if text.contains('T') || text.contains(' ') {
        return parse_instant(text).map(Point::Exact).ok_or_else(invalid);
    }
    Err(invalid())
}

fn align_down(us: i64, interval: Interval) -> i64 {
    match interval {
        Interval::Hour => trunc(us, Interval::Hour),
        _ => trunc(us, Interval::Day),
    }
}

fn align_end(us: i64, interval: Interval) -> i64 {
    match interval {
        Interval::Hour => trunc(us, Interval::Hour).saturating_add(HOUR_US),
        _ => trunc(us, Interval::Day).saturating_add(DAY_US),
    }
}

/// Resolve a PostHog date range. `earliest` supplies the first event's
/// timestamp for `date_from: "all"`.
pub fn resolve(
    range: &DateRange,
    interval: Interval,
    now: DateTime<Utc>,
    earliest: impl FnOnce() -> Result<Option<i64>, QueryError>,
) -> Result<ResolvedRange, QueryError> {
    let now_us = from_datetime(now);
    let from = match parse_point(&range.date_from, now)? {
        Point::Exact(us) => us,
        Point::Calendar(us) => align_down(us, interval),
        Point::All => align_down(earliest()?.unwrap_or(now_us), interval),
    };
    let to = match range.date_to.as_deref() {
        None | Some("") => align_end(now_us, interval),
        Some(text) => match parse_point(text, now)? {
            Point::Exact(us) => us.saturating_add(1),
            Point::Calendar(us) => align_end(us, interval),
            Point::All => return Err(QueryError::invalid("date_to cannot be `all`")),
        },
    };
    if from >= to {
        return Err(QueryError::invalid("date_from must be before date_to"));
    }
    Ok(ResolvedRange { from, to, interval })
}

pub fn date_from_is_all(range: &DateRange) -> bool {
    range.date_from.trim() == "all"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 4, 15, 30, 0).unwrap()
    }

    fn range(from: &str, to: Option<&str>) -> DateRange {
        DateRange {
            date_from: from.into(),
            date_to: to.map(Into::into),
        }
    }

    fn at(text: &str) -> i64 {
        parse_instant(text).unwrap()
    }

    #[test]
    fn relative_ranges_follow_posthog_conventions() {
        let r = resolve(&range("-7d", None), Interval::Day, now(), || Ok(None)).unwrap();
        assert_eq!(r.from, at("2026-09-27T00:00:00Z"));
        assert_eq!(r.to, at("2026-10-05T00:00:00Z"));
        assert_eq!(r.buckets().unwrap().len(), 8);

        let r = resolve(&range("-24h", None), Interval::Hour, now(), || Ok(None)).unwrap();
        assert_eq!(r.from, at("2026-10-03T15:00:00Z"));
        assert_eq!(r.to, at("2026-10-04T16:00:00Z"));
        assert_eq!(r.buckets().unwrap().len(), 25);

        let r = resolve(&range("mStart", None), Interval::Day, now(), || Ok(None)).unwrap();
        assert_eq!(r.from, at("2026-10-01T00:00:00Z"));
        let r = resolve(&range("yStart", None), Interval::Month, now(), || Ok(None)).unwrap();
        assert_eq!(r.buckets().unwrap().len(), 10);
        let r = resolve(&range("-3m", None), Interval::Month, now(), || Ok(None)).unwrap();
        assert_eq!(r.from, at("2026-07-04T00:00:00Z"));
        assert_eq!(r.buckets().unwrap()[0], at("2026-07-01T00:00:00Z"));
        let r = resolve(&range("dStart", None), Interval::Hour, now(), || Ok(None)).unwrap();
        assert_eq!(r.from, at("2026-10-04T15:00:00Z"));
    }

    #[test]
    fn absolute_ranges_and_all() {
        let r = resolve(
            &range("2026-01-01", Some("2026-01-31")),
            Interval::Week,
            now(),
            || Ok(None),
        )
        .unwrap();
        assert_eq!(r.from, at("2026-01-01T00:00:00Z"));
        assert_eq!(r.to, at("2026-02-01T00:00:00Z"));
        // 2026-01-01 is a Thursday: first week bucket starts Monday 2025-12-29.
        assert_eq!(r.buckets().unwrap()[0], at("2025-12-29T00:00:00Z"));
        assert_eq!(r.date_to(), "2026-01-31T23:59:59.999999Z");

        let r = resolve(
            &range("2026-01-01T10:00:00Z", Some("2026-01-01T12:00:00Z")),
            Interval::Hour,
            now(),
            || Ok(None),
        )
        .unwrap();
        assert_eq!(r.from, at("2026-01-01T10:00:00Z"));
        assert_eq!(r.to, at("2026-01-01T12:00:00Z") + 1);
        assert_eq!(r.buckets().unwrap().len(), 3);

        let first = at("2026-03-05T08:00:00Z");
        let r = resolve(&range("all", None), Interval::Day, now(), || {
            Ok(Some(first))
        })
        .unwrap();
        assert_eq!(r.from, at("2026-03-05T00:00:00Z"));

        assert!(resolve(&range("-7x", None), Interval::Day, now(), || Ok(None)).is_err());
        assert!(resolve(&range("garbage", None), Interval::Day, now(), || Ok(None)).is_err());
        assert!(
            resolve(
                &range("2026-02-01", Some("2026-01-01")),
                Interval::Day,
                now(),
                || Ok(None)
            )
            .is_err()
        );
        assert!(
            resolve(&range("-100000d", None), Interval::Hour, now(), || Ok(None))
                .unwrap()
                .buckets()
                .is_err()
        );
    }

    #[test]
    fn truncation_handles_weeks_months_and_pre_epoch() {
        assert_eq!(
            trunc(at("2026-10-04T15:30:00Z"), Interval::Week),
            at("2026-09-28T00:00:00Z")
        );
        assert_eq!(
            trunc(at("2026-09-28T00:00:00Z"), Interval::Week),
            at("2026-09-28T00:00:00Z")
        );
        assert_eq!(
            trunc(at("1969-12-31T23:00:00Z"), Interval::Day),
            at("1969-12-31T00:00:00Z")
        );
        assert_eq!(
            next_bucket(at("2026-01-01T00:00:00Z"), Interval::Month),
            at("2026-02-01T00:00:00Z")
        );
        assert_eq!(
            previous_bucket(at("2026-03-01T00:00:00Z"), Interval::Month),
            at("2026-02-01T00:00:00Z")
        );
        assert_eq!(
            label(at("2026-10-04T00:00:00Z"), Interval::Day),
            "Sun 4 Oct"
        );
    }
}
