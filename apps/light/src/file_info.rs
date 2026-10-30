//! What the project tree says beside a file: its size in the unit that suits it, and when it was last changed, in the
//! computer's local time.

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::i18n::tr;

/// `0 B`, `512 B`, `1.2 KB`, `48 KB`, `3.4 MB`, `1.1 GB`: one decimal below ten, none above.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} {}", tr("B"));
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    // 1023.95 KB would print as "1024 KB": that is the next unit
    if value.round() >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let unit = tr(UNITS[unit]);
    if value < 9.95 { format!("{value:.1} {unit}") } else { format!("{value:.0} {unit}") }
}

/// The month and day of a count of days since 1970-01-01 (`civil_from_days` of Howard Hinnant).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 }, month, day)
}

/// When a file was changed, as short as it can be said: the time today, `yesterday`, the day and month this year, and
/// the year too before that. `offset` is the local time's distance from UTC in seconds.
pub fn modified(time: SystemTime, now: SystemTime, offset: i64) -> String {
    let secs = |t: SystemTime| t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64) + offset;
    let (t, n) = (secs(time), secs(now));
    let (day, today) = (t.div_euclid(86_400), n.div_euclid(86_400));
    let (year, month, mday) = civil(day);
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let mon = tr(MONTHS[month as usize - 1]);
    if day == today {
        let s = t.rem_euclid(86_400);
        format!("{:02}:{:02}", s / 3600, s % 3600 / 60)
    } else if day + 1 == today {
        tr("yesterday").to_string()
    } else if year == civil(today).0 {
        format!("{mday} {mon}")
    } else {
        format!("{mday} {mon} {year}")
    }
}

/// How far the local time is from UTC, in seconds (read once: a change of the time zone shows after a restart).
pub fn local_offset() -> i64 {
    static OFFSET: OnceLock<i64> = OnceLock::new();
    *OFFSET.get_or_init(read_local_offset)
}

#[cfg(windows)]
fn read_local_offset() -> i64 {
    #[repr(C)]
    #[derive(Default)]
    struct Win32Time {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLocalTime(time: *mut Win32Time);
        fn GetSystemTime(time: *mut Win32Time);
    }
    let (mut local, mut utc) = (Win32Time::default(), Win32Time::default());
    // SAFETY: both functions only write the SYSTEMTIME they are given, whose layout this struct repeats
    unsafe {
        GetLocalTime(&mut local);
        GetSystemTime(&mut utc);
    }
    let minutes = |t: &Win32Time| {
        let days = days_from_civil(t.year as i64, t.month as i64, t.day as i64);
        days * 1440 + t.hour as i64 * 60 + t.minute as i64
    };
    // the two readings can straddle a minute: round to the quarter hour, the finest step of real time zones
    let diff = minutes(&local) - minutes(&utc);
    (diff as f64 / 15.0).round() as i64 * 15 * 60
}

#[cfg(windows)]
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

#[cfg(not(windows))]
fn read_local_offset() -> i64 {
    // `+0300`: what `date` says the zone is
    std::process::Command::new("date")
        .arg("+%z")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| {
            let s = s.trim();
            let sign = if s.starts_with('-') { -1 } else { 1 };
            let digits = s.trim_start_matches(['+', '-']);
            let (h, m) = (digits.get(0..2)?.parse::<i64>().ok()?, digits.get(2..4)?.parse::<i64>().ok()?);
            Some(sign * (h * 3600 + m * 60))
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn sizes_are_given_in_the_unit_that_suits_them() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(1023), "1023 B");
        assert_eq!(size(1024), "1.0 KB");
        assert_eq!(size(1536), "1.5 KB");
        assert_eq!(size(48 * 1024 + 100), "48 KB");
        assert_eq!(size(1024 * 1024 - 1), "1.0 MB");
        assert_eq!(size(3 * 1024 * 1024 + 400 * 1024), "3.4 MB");
        assert_eq!(size(5 * 1024 * 1024 * 1024), "5.0 GB");
    }

    #[test]
    fn dates_say_as_little_as_is_needed() {
        // 2026-10-30 14:05 UTC
        let now = UNIX_EPOCH + Duration::from_secs(1_793_369_100);
        assert_eq!(civil(1_793_369_100 / 86_400), (2026, 10, 30));
        assert_eq!(modified(now - Duration::from_secs(3600), now, 0), "13:05");
        // three hours ahead of UTC it is already 17:05
        assert_eq!(modified(now, now, 3 * 3600), "17:05");
        assert_eq!(modified(now - Duration::from_secs(86_400), now, 0), "yesterday");
        assert_eq!(modified(now - Duration::from_secs(20 * 86_400), now, 0), "10 Oct");
        assert_eq!(modified(now - Duration::from_secs(400 * 86_400), now, 0), "25 Sep 2025");
    }
}
