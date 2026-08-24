//! Android logcat threadtime: `07-15 22:00:01.123` (yearless `MM-DD`).

use std::ops::Range;
use std::sync::LazyLock;

use chrono::{Datelike, Local, NaiveDateTime};
use regex::Regex;

use super::{window, TimeFormat};

static RE_LOGCAT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{2}-\d{2}\s\d{2}:\d{2}:\d{2}\.\d{3}").unwrap());

/// Android logcat threadtime `07-15 22:00:01.123` timestamps (yearless).
pub struct LogcatThreadtime;

impl TimeFormat for LogcatThreadtime {
    fn name(&self) -> &'static str {
        "logcat threadtime"
    }

    fn matches(&self, line: &str) -> bool {
        RE_LOGCAT.find(window(line)).is_some()
    }

    fn extract(&self, line: &str) -> Option<(i64, Range<usize>)> {
        let m = RE_LOGCAT.find(window(line))?;
        Some((parse_logcat(m.as_str())?, m.range()))
    }
}

/// Logcat `MM-DD HH:MM:SS.mmm` carries no year; assume the current one
/// (with the same future-rollback as syslog).
fn parse_logcat(raw: &str) -> Option<i64> {
    let year = Local::now().year();
    let with_year = format!("{year}-{raw}");
    let mut n = NaiveDateTime::parse_from_str(&with_year, "%Y-%m-%d %H:%M:%S%.f").ok()?;
    if n.and_utc().timestamp() > Local::now().timestamp() + 86_400 {
        n = n.with_year(year - 1)?;
    }
    Some(n.and_utc().timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::time::format_ms;

    #[test]
    fn matches_positive() {
        assert!(LogcatThreadtime.matches("07-15 22:00:01.123  1234  5678 I Tag: hello"));
    }

    #[test]
    fn matches_negative() {
        assert!(!LogcatThreadtime.matches("2026-07-19 10:15:30 INFO hi"));
        assert!(!LogcatThreadtime.matches("plain text"));
    }

    #[test]
    fn extracts_logcat_threadtime() {
        let (ms, span) = LogcatThreadtime
            .extract("07-15 22:00:01.123  1234  5678 I Tag: hello")
            .unwrap();
        assert_eq!(
            &"07-15 22:00:01.123  1234  5678 I Tag: hello"[span],
            "07-15 22:00:01.123"
        );
        let mut year = Local::now().year();
        let parsed = NaiveDateTime::parse_from_str(
            &format!("{year}-07-15 22:00:01.123"),
            "%Y-%m-%d %H:%M:%S%.f",
        )
        .unwrap();
        if parsed.and_utc().timestamp() > Local::now().timestamp() + 86_400 {
            year -= 1;
        }
        assert_eq!(format_ms(ms), format!("{year}-07-15 22:00:01.123"));
    }
}
