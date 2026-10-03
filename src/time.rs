//! Wall-clock time without a date crate: the Unix clock, RFC 3339 timestamps
//! as the agent CLIs write them, and the compact UTC stamps Corgi names files
//! with. Both directions of the calendar use Howard Hinnant's algorithms.

use std::time::{SystemTime, UNIX_EPOCH};

/// The wall clock in Unix seconds. Zero when the clock is before 1970, which
/// only makes every countdown read as elapsed.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// Parses an RFC 3339 timestamp such as `2026-09-09T14:00:00.123Z` or
/// `2026-09-09T14:00:00+02:00` into Unix seconds.
pub fn parse_rfc3339(text: &str) -> Option<u64> {
    let text = text.trim();
    let (date, rest) = text.split_once(['T', 't', ' '])?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let offset_index = rest.find(['Z', 'z', '+', '-']);
    let (time, offset) = match offset_index {
        Some(index) => rest.split_at(index),
        None => (rest, "Z"),
    };
    let time = time.split('.').next()?;
    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next().unwrap_or("0").parse().ok()?;
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let offset_seconds: i64 = match offset {
        "Z" | "z" | "" => 0,
        signed => {
            let sign = if signed.starts_with('-') { -1 } else { 1 };
            let mut parts = signed[1..].split(':');
            let offset_hours: i64 = parts.next()?.parse().ok()?;
            let offset_minutes: i64 = parts.next().unwrap_or("0").parse().ok()?;
            sign * (offset_hours * 3_600 + offset_minutes * 60)
        }
    };

    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_seconds;
    u64::try_from(seconds).ok()
}

/// `secs` (Unix seconds) as `YYYYMMDD-HHMMSS` in UTC.
pub fn utc_stamp(secs: u64) -> String {
    let (days, time) = (secs / 86_400, secs % 86_400);
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        time / 3_600,
        time / 60 % 60,
        time % 60
    )
}

/// `secs` (Unix seconds) as an RFC 3339 timestamp in UTC, such as
/// `2026-10-03T07:08:09Z`, which [`parse_rfc3339`] reads back.
pub fn rfc3339_utc(secs: u64) -> String {
    let (days, time) = (secs / 86_400, secs % 86_400);
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3_600,
        time / 60 % 60,
        time % 60
    )
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (i64::from(month) + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The proleptic Gregorian date `days` after 1970-01-01, as year, month and
/// day (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::{civil_from_days, days_from_civil, parse_rfc3339, rfc3339_utc, utc_stamp};

    #[test]
    fn rfc3339_timestamps_parse_into_unix_seconds() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(
            parse_rfc3339("2026-09-09T14:30:00+02:00"),
            parse_rfc3339("2026-09-09T12:30:00Z")
        );
        assert_eq!(
            parse_rfc3339("2026-09-09T14:30:00.250-01:30"),
            parse_rfc3339("2026-09-09T16:00:00Z")
        );
        assert_eq!(parse_rfc3339("not a date"), None);
    }

    #[test]
    fn utc_stamps_name_the_calendar_date_and_time() {
        assert_eq!(utc_stamp(0), "19700101-000000");
        assert_eq!(utc_stamp(951_868_800), "20000301-000000");
        assert_eq!(
            utc_stamp(parse_rfc3339("2026-09-24T07:08:09Z").expect("timestamp")),
            "20260924-070809"
        );
        let at = parse_rfc3339("2026-10-03T07:08:09Z").expect("timestamp");
        assert_eq!(rfc3339_utc(at), "2026-10-03T07:08:09Z");
    }

    #[test]
    fn the_calendar_round_trips_in_both_directions() {
        for days in [-719_468, -1, 0, 59, 60, 10_957, 20_720, 2_932_896] {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(
                days_from_civil(year, month as u32, day as u32),
                days,
                "{year}-{month}-{day}"
            );
        }
    }
}
