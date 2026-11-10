//! Wall-clock time without a date crate: the Unix clock, RFC 3339 timestamps
//! as the agent CLIs write them, the compact UTC stamps Corgi names files
//! with, and the local clock and date a row shows. Both directions of the
//! calendar use Howard Hinnant's algorithms.

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

/// Month names as a row shows them, `DD Mon`.
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// A moment together with the offset from UTC the local clock had at it, so
/// it can be shown on the wall clock without asking the system again.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LocalTime {
    /// Unix seconds.
    pub secs: u64,
    /// Seconds the local clock is ahead of UTC, negative west of Greenwich.
    pub offset: i64,
}

impl LocalTime {
    /// `secs` (Unix seconds) on this machine's local clock.
    pub fn at(secs: u64) -> Self {
        Self {
            secs,
            offset: local_offset(secs),
        }
    }

    /// The local time and date as `HH:MM DD Mon`, such as `10:21 20 Oct`.
    pub fn clock_and_date(self) -> String {
        let local = self.secs as i64 + self.offset;
        let (days, time) = (local.div_euclid(86_400), local.rem_euclid(86_400));
        let (_, month, day) = civil_from_days(days);
        format!(
            "{:02}:{:02} {day:02} {}",
            time / 3_600,
            time / 60 % 60,
            MONTHS[(month - 1) as usize]
        )
    }
}

/// Seconds the local clock was ahead of UTC at `secs`, by the system's time
/// zone rules (`TZ`, else `/etc/localtime`), so a moment across a daylight
/// saving change gets the offset it had then. Zero when the system cannot say.
#[cfg(unix)]
fn local_offset(secs: u64) -> i64 {
    let Ok(time) = libc::time_t::try_from(secs) else {
        return 0;
    };
    // SAFETY: `localtime_r` only writes the `tm` it is given, which is plain
    // data for which all zeroes is a valid value.
    let mut local: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&time, &mut local) }.is_null() {
        return 0;
    }
    // `c_long`, which is narrower than `i64` on 32-bit targets.
    local.tm_gmtoff as i64
}

#[cfg(not(unix))]
fn local_offset(_secs: u64) -> i64 {
    0
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
    use super::{
        LocalTime, civil_from_days, days_from_civil, parse_rfc3339, rfc3339_utc, utc_stamp,
    };

    fn at(text: &str) -> u64 {
        parse_rfc3339(text).expect("timestamp")
    }

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

    #[test]
    fn the_local_clock_and_date_shift_by_the_offset() {
        let local = |text: &str, offset: i64| {
            LocalTime {
                secs: at(text),
                offset,
            }
            .clock_and_date()
        };
        assert_eq!(local("2026-10-20T10:21:59Z", 0), "10:21 20 Oct");
        assert_eq!(local("2026-10-20T08:21:00Z", 2 * 3_600), "10:21 20 Oct");
        // An offset carries the clock across midnight in either direction,
        // and the date across the end of a month and of a year.
        assert_eq!(local("2026-10-20T23:30:00Z", 2 * 3_600), "01:30 21 Oct");
        assert_eq!(local("2026-10-01T01:00:00Z", -5 * 3_600), "20:00 30 Sep");
        assert_eq!(
            local("2026-12-31T22:15:00Z", 5 * 3_600 + 1_800),
            "03:45 01 Jan"
        );
        assert_eq!(local("2027-01-01T00:59:00Z", -3_600), "23:59 31 Dec");
        assert_eq!(local("2028-02-28T23:00:00Z", 3_600), "00:00 29 Feb");
        // Before 1970 in local time, the clock still reads forwards.
        assert_eq!(local("1970-01-01T00:00:00Z", -3_600), "23:00 31 Dec");
    }

    #[test]
    fn the_local_offset_comes_from_the_system() {
        // Whatever this machine's zone, its offset is a whole number of
        // minutes within a day, and the moment keeps its Unix seconds.
        let now = LocalTime::at(at("2026-10-20T10:21:00Z"));
        assert_eq!(now.secs, at("2026-10-20T10:21:00Z"));
        assert!(now.offset.abs() < 86_400 && now.offset % 60 == 0, "{now:?}");
    }
}
