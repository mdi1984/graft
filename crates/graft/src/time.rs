//! Dates as people read them: "5 min ago" for recent ones, a short date
//! otherwise, always in the local time zone.

use jiff::Timestamp;
use jiff::tz::TimeZone;

pub fn now() -> i64 {
    Timestamp::now().as_second()
}

/// "just now", "12 min ago", "3 h ago", "yesterday", "4 days ago", then a date.
pub fn relative(seconds: i64, now: i64) -> String {
    relative_in(seconds, now, &TimeZone::system())
}

/// "23 Oct 2026, 14:05".
pub fn absolute(seconds: i64) -> String {
    absolute_in(seconds, &TimeZone::system())
}

fn relative_in(seconds: i64, now: i64, zone: &TimeZone) -> String {
    let age = now - seconds;
    match age {
        i64::MIN..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", age / 60),
        3600..86_400 => format!("{} h ago", age / 3600),
        86_400..172_800 => "yesterday".to_owned(),
        172_800..604_800 => format!("{} days ago", age / 86_400),
        _ => format(seconds, "%-d %b %Y", zone),
    }
}

fn absolute_in(seconds: i64, zone: &TimeZone) -> String {
    format(seconds, "%-d %b %Y, %H:%M", zone)
}

/// "14:05:09".
pub fn clock(seconds: i64) -> String {
    format(seconds, "%H:%M:%S", &TimeZone::system())
}

fn format(seconds: i64, pattern: &str, zone: &TimeZone) -> String {
    match Timestamp::from_second(seconds) {
        Ok(ts) => ts.to_zoned(zone.clone()).strftime(pattern).to_string(),
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 15 Jan 2027, 08:00 UTC.
    const NOW: i64 = 1_800_000_000;

    fn relative(seconds: i64) -> String {
        relative_in(seconds, NOW, &TimeZone::UTC)
    }

    #[test]
    fn relative_steps() {
        assert_eq!(relative(NOW - 59), "just now");
        assert_eq!(relative(NOW - 60), "1 min ago");
        assert_eq!(relative(NOW - 600), "10 min ago");
        assert_eq!(relative(NOW - 3599), "59 min ago");
        assert_eq!(relative(NOW - 3600), "1 h ago");
        assert_eq!(relative(NOW - 86_399), "23 h ago");
        assert_eq!(relative(NOW - 86_400), "yesterday");
        assert_eq!(relative(NOW - 172_800), "2 days ago");
        assert_eq!(relative(NOW - 604_799), "6 days ago");
        assert_eq!(relative(NOW - 604_800), "8 Jan 2027");
        assert_eq!(relative(NOW - 30 * 86_400), "16 Dec 2026");
        assert_eq!(relative(NOW + 30), "just now", "a clock ahead");
    }

    #[test]
    fn absolute_dates_have_the_time() {
        assert_eq!(absolute_in(NOW, &TimeZone::UTC), "15 Jan 2027, 08:00");
    }
}
