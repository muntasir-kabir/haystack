//! BSD syslog timestamps: `Jan  5 10:00:00` (month name, day, clock, no year).

use std::ops::Range;
use std::sync::LazyLock;

use chrono::{Datelike, Local, NaiveDateTime};
use regex::Regex;

use super::{window, TimeFormat};

static RE_SYSLOG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z][a-z]{2}\s{1,2}\d{1,2}\s\d{2}:\d{2}:\d{2}").unwrap());

/// BSD syslog `Jan  5 10:00:00` timestamps (yearless).
pub struct Syslog;

impl TimeFormat for Syslog {
    fn name(&self) -> &'static str {
        "BSD syslog"
    }

    fn matches(&self, line: &str) -> bool {
        RE_SYSLOG.find(window(line)).is_some()
    }

    fn extract(&self, line: &str) -> Option<(i64, Range<usize>)> {
        let m = RE_SYSLOG.find(window(line))?;
        Some((parse_syslog(m.as_str())?, m.range()))
    }
}

fn parse_syslog(raw: &str) -> Option<i64> {
    // Syslog lines carry no year; assume the current one.
    let year = Local::now().year();
    let with_year = format!("{year} {raw}");
    let mut n = NaiveDateTime::parse_from_str(&with_year, "%Y %b %e %H:%M:%S").ok()?;
    // A log from Dec 31 parsed on Jan 2 would land ~1 year in the future.
    // If the parsed date is far in the future, roll it back a year.
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
        assert!(Syslog.matches("Jan  5 03:22:11 host sshd[1]: hi"));
    }

    #[test]
    fn matches_negative() {
        assert!(!Syslog.matches("2026-07-19 10:15:30 INFO hi"));
        assert!(!Syslog.matches("plain text"));
    }

    #[test]
    fn extracts_syslog() {
        let (ms, span) = Syslog.extract("Jan  5 03:22:11 host sshd[1]: hi").unwrap();
        // Mirror parse_syslog's year rollback so the test is deterministic
        // even when run in the first days of January.
        let mut year = Local::now().year();
        let parsed =
            NaiveDateTime::parse_from_str(&format!("{year} Jan  5 03:22:11"), "%Y %b %e %H:%M:%S")
                .unwrap();
        if parsed.and_utc().timestamp() > Local::now().timestamp() + 86_400 {
            year -= 1;
        }
        assert_eq!(format_ms(ms), format!("{year}-01-05 03:22:11.000"));
        assert_eq!(&"Jan  5 03:22:11 host sshd[1]: hi"[span], "Jan  5 03:22:11");
    }
}
