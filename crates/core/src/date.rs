//! Calendar dates in `YYYY-MM-DD` form, as used for P3M snapshots.
//!
//! Only what rok needs: validation, comparison as strings (the format sorts correctly), the
//! current UTC date, and adding days. Dates are proleptic Gregorian.

use std::time::{SystemTime, UNIX_EPOCH};

/// Whether `s` is a calendar date in `YYYY-MM-DD` form.
pub fn is_valid(s: &str) -> bool {
    parse(s).is_some()
}

/// Parses `YYYY-MM-DD` into (year, month, day).
fn parse(s: &str) -> Option<(i64, u32, u32)> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    // Bytes 4 and 7 are ASCII `-`, so the slices below fall on character boundaries.
    let num = |r: std::ops::Range<usize>| -> Option<u32> {
        let part = &s[r];
        part.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| part.parse().ok())
            .flatten()
    };
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    ((1..=12).contains(&m) && (1..=days_in_month(i64::from(y), m)).contains(&d)).then_some((
        i64::from(y),
        m,
        d,
    ))
}

fn days_in_month(y: i64, m: u32) -> u32 {
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if leap => 29,
        _ => 28,
    }
}

/// Days since 1970-01-01 (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date for a day count since 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil_from_days(z: i64) -> String {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Today's date in UTC.
pub fn today_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    civil_from_days(i64::try_from(secs / 86_400).unwrap_or(0))
}

/// `date` plus `days` (which may be negative). Returns `None` for an invalid date.
pub fn add_days(date: &str, days: i64) -> Option<String> {
    let (y, m, d) = parse(date)?;
    Some(civil_from_days(days_from_civil(y, m, d) + days))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_dates() {
        assert!(is_valid("2024-02-29"));
        assert!(is_valid("2000-02-29"));
        assert!(!is_valid("1900-02-29"));
        assert!(!is_valid("2023-02-29"));
        assert!(!is_valid("2026-13-01"));
        assert!(!is_valid("2026-00-10"));
        assert!(!is_valid("2026-1-01"));
        assert!(!is_valid("26-10-04xx"));
        assert!(!is_valid("2026-10-0é"));
    }

    #[test]
    fn adds_days_across_boundaries() {
        assert_eq!(add_days("2026-10-06", 1).as_deref(), Some("2026-10-07"));
        assert_eq!(add_days("2026-12-31", 1).as_deref(), Some("2027-01-01"));
        assert_eq!(add_days("2024-03-01", -1).as_deref(), Some("2024-02-29"));
        assert_eq!(add_days("1970-01-01", 0).as_deref(), Some("1970-01-01"));
        assert_eq!(add_days("2026-02-30", 1), None);
    }

    #[test]
    fn today_is_a_valid_recent_date() {
        let today = today_utc();
        assert!(is_valid(&today));
        assert!(today.as_str() > "2026-01-01");
    }
}
