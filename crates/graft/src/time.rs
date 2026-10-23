//! Dates as people read them: "5 min ago" for recent ones, a short date
//! otherwise, always in the local time zone.

use jiff::Timestamp;
use jiff::tz::TimeZone;

pub fn now() -> i64 {
    Timestamp::now().as_second()
}

/// "just now", "12 min ago", "3 h ago", "yesterday", "4 days ago", then a date.
pub fn relative(seconds: i64, now: i64) -> String {
    let age = now - seconds;
    match age {
        i64::MIN..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", age / 60),
        3600..86_400 => format!("{} h ago", age / 3600),
        86_400..172_800 => "yesterday".to_owned(),
        172_800..604_800 => format!("{} days ago", age / 86_400),
        _ => format(seconds, "%-d %b %Y"),
    }
}

/// "23 Oct 2026, 14:05".
pub fn absolute(seconds: i64) -> String {
    format(seconds, "%-d %b %Y, %H:%M")
}

fn format(seconds: i64, pattern: &str) -> String {
    match Timestamp::from_second(seconds) {
        Ok(ts) => ts
            .to_zoned(TimeZone::system())
            .strftime(pattern)
            .to_string(),
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_steps() {
        let now = 1_800_000_000;
        assert_eq!(relative(now - 5, now), "just now");
        assert_eq!(relative(now - 600, now), "10 min ago");
        assert_eq!(relative(now - 7200, now), "2 h ago");
        assert_eq!(relative(now - 90_000, now), "yesterday");
        assert_eq!(relative(now - 3 * 86_400, now), "3 days ago");
        assert!(relative(now - 30 * 86_400, now).contains("20"));
    }
}
