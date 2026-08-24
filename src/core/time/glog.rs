//! C++ glog timestamps: `I0715 22:00:01.123456` (level letter + `MMDD`, yearless).

use std::ops::Range;
use std::sync::LazyLock;

use chrono::{Datelike, Local, NaiveDateTime};
use regex::Regex;

use super::{window, TimeFormat};

static RE_GLOG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[IWEF]\d{4}\s\d{2}:\d{2}:\d{2}(?:\.\d+)?").unwrap());

/// C++ glog `I0715 22:00:01.123456` timestamps (yearless).
pub struct Glog;

impl TimeFormat for Glog {
    fn name(&self) -> &'static str {
        "glog"
    }

    fn matches(&self, line: &str) -> bool {
        RE_GLOG.find(window(line)).is_some()
    }

    fn extract(&self, line: &str) -> Option<(i64, Range<usize>)> {
        let m = RE_GLOG.find(window(line))?;
        Some((parse_glog(m.as_str())?, m.range()))
    }
}

/// Glog `I0715 22:00:01.123456`: level letter + MMDD + clock, no year.
fn parse_glog(raw: &str) -> Option<i64> {
    let year = Local::now().year();
    // Rewrite `I0715 ...` → `2026-07-15 ...` for a single strptime call.
    let mm = raw.get(1..3)?;
    let dd = raw.get(3..5)?;
    let clock = raw.get(6..)?;
    let normalized = format!("{year}-{mm}-{dd} {clock}");
    let mut n = NaiveDateTime::parse_from_str(&normalized, "%Y-%m-%d %H:%M:%S%.f").ok()?;
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
        assert!(Glog.matches("I0715 22:00:01.123456 12345 server.cc:42] started"));
    }

    #[test]
    fn matches_negative() {
        assert!(!Glog.matches("2026-07-19 10:15:30 INFO hi"));
        assert!(!Glog.matches("plain text"));
    }

    #[test]
    fn extracts_glog() {
        let (ms, span) = Glog
            .extract("I0715 22:00:01.123456 12345 server.cc:42] started")
            .unwrap();
        assert_eq!(
            &"I0715 22:00:01.123456 12345 server.cc:42] started"[span],
            "I0715 22:00:01.123456"
        );
        let mut year = Local::now().year();
        let parsed = NaiveDateTime::parse_from_str(
            &format!("{year}-07-15 22:00:01.123456"),
            "%Y-%m-%d %H:%M:%S%.f",
        )
        .unwrap();
        if parsed.and_utc().timestamp() > Local::now().timestamp() + 86_400 {
            year -= 1;
        }
        assert_eq!(format_ms(ms), format!("{year}-07-15 22:00:01.123"));
    }
}
