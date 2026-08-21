//! Date formatting without a timezone database.
//!
//! Git stores each signature as a UTC timestamp plus the author's UTC offset in
//! minutes. That is enough to reconstruct the wall-clock time the author saw,
//! with no tz database and no dependency: apply the offset and convert. Showing
//! the author's local time is also the correct thing to show — it is what the
//! commit says happened.

/// Days from the civil epoch (1970-01-01) to a `(year, month, day)`.
///
/// Howard Hinnant's `civil_from_days`, which is exact for the proleptic
/// Gregorian calendar over the whole range we care about.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Split a timestamp into calendar parts in the given UTC offset.
fn parts(unix_seconds: i64, offset_minutes: i32) -> (i64, u32, u32, u32, u32) {
    let local = unix_seconds + i64::from(offset_minutes) * 60;
    // Floor division, so pre-epoch timestamps don't round toward zero.
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    (y, m, d, (secs / 3600) as u32, (secs % 3600 / 60) as u32)
}

/// `12 Mar 2024` — for columns where the year always matters.
pub fn date(unix_seconds: i64, offset_minutes: i32) -> String {
    let (y, m, d, _, _) = parts(unix_seconds, offset_minutes);
    format!("{d} {} {y}", MONTHS[(m - 1) as usize])
}

/// `12 Mar 2024 at 14:05` — for detail panes.
pub fn date_time(unix_seconds: i64, offset_minutes: i32) -> String {
    let (y, m, d, hh, mm) = parts(unix_seconds, offset_minutes);
    format!("{d} {} {y} at {hh:02}:{mm:02}", MONTHS[(m - 1) as usize])
}

/// `3 days ago` — for the commit list, where recency is what you scan for.
///
/// `now` is passed in rather than read from the clock so this stays a pure
/// function and can be tested.
pub fn relative(unix_seconds: i64, now: i64) -> String {
    let delta = now - unix_seconds;
    if delta < 0 {
        // Clock skew, or a commit dated in the future. Don't say "-2 minutes".
        return "just now".to_owned();
    }
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    const WEEK: i64 = 7 * DAY;
    const MONTH: i64 = 30 * DAY;
    const YEAR: i64 = 365 * DAY;

    let (n, unit) = match delta {
        d if d < MINUTE => return "just now".to_owned(),
        d if d < HOUR => (d / MINUTE, "minute"),
        d if d < DAY => (d / HOUR, "hour"),
        d if d < WEEK => (d / DAY, "day"),
        d if d < MONTH => (d / WEEK, "week"),
        d if d < YEAR => (d / MONTH, "month"),
        d => (d / YEAR, "year"),
    };
    if n == 1 {
        format!("1 {unit} ago")
    } else {
        format!("{n} {unit}s ago")
    }
}

/// Seconds since the Unix epoch, for callers that need a `now` to compare to.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_is_1970() {
        assert_eq!(date(0, 0), "1 Jan 1970");
    }

    #[test]
    fn known_dates_round_trip() {
        // 2024-01-01T00:00:00Z
        assert_eq!(date(1_704_067_200, 0), "1 Jan 2024");
        // A leap day, to exercise the calendar arithmetic.
        assert_eq!(date(1_709_164_800, 0), "29 Feb 2024");
        assert_eq!(date_time(1_709_208_300, 0), "29 Feb 2024 at 12:05");
    }

    #[test]
    fn offsets_shift_the_wall_clock() {
        // 23:30 UTC in a +02:00 zone is 01:30 the next day.
        assert_eq!(date_time(1_704_065_400, 120), "1 Jan 2024 at 01:30");
        // And in a -05:00 zone it is still the previous day.
        assert_eq!(date_time(1_704_065_400, -300), "31 Dec 2023 at 18:30");
    }

    #[test]
    fn pre_epoch_does_not_round_toward_zero() {
        assert_eq!(date(-1, 0), "31 Dec 1969");
    }

    #[test]
    fn relative_units() {
        let now = 1_704_067_200;
        assert_eq!(relative(now, now), "just now");
        assert_eq!(relative(now - 90, now), "1 minute ago");
        assert_eq!(relative(now - 7200, now), "2 hours ago");
        assert_eq!(relative(now - 3 * 86_400, now), "3 days ago");
        assert_eq!(relative(now - 400 * 86_400, now), "1 year ago");
        // Future timestamps must not produce negative counts.
        assert_eq!(relative(now + 500, now), "just now");
    }
}
