//! `YYYY/MM/DD HH:MM:SS` timestamps (slash-separated date).

use std::ops::Range;
use std::sync::LazyLock;

use chrono::NaiveDateTime;
use regex::Regex;

use super::{window, TimeFormat};

static RE_SLASH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{4}/\d{2}/\d{2}[ T]\d{2}:\d{2}:\d{2}(?:[.,]\d+)?").unwrap());

/// `YYYY/MM/DD HH:MM:SS` timestamps.
pub struct Slash;

impl TimeFormat for Slash {
    fn name(&self) -> &'static str {
        "YYYY/MM/DD"
    }

    fn matches(&self, line: &str) -> bool {
        RE_SLASH.find(window(line)).is_some()
    }

    fn extract(&self, line: &str) -> Option<(i64, Range<usize>)> {
        let m = RE_SLASH.find(window(line))?;
        Some((parse_slash(m.as_str())?, m.range()))
    }
}

fn parse_slash(raw: &str) -> Option<i64> {
    let s = raw.replace('T', " ").replace(',', ".");
    NaiveDateTime::parse_from_str(&s, "%Y/%m/%d %H:%M:%S%.f")
        .ok()
        .map(|n| n.and_utc().timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::time::format_ms;

    #[test]
    fn matches_positive() {
        assert!(Slash.matches("2026/07/19 10:15:30 INFO hi"));
    }

    #[test]
    fn matches_negative() {
        assert!(!Slash.matches("2026-07-19 10:15:30 INFO hi"));
        assert!(!Slash.matches("plain text"));
    }

    #[test]
    fn extracts_slash_date() {
        let (ms, span) = Slash.extract("2026/07/19 10:15:30 INFO hi").unwrap();
        assert_eq!(format_ms(ms), "2026-07-19 10:15:30.000");
        assert_eq!(&"2026/07/19 10:15:30 INFO hi"[span], "2026/07/19 10:15:30");
    }

    #[test]
    fn extracts_slash_fraction() {
        let (ms, _) = Slash.extract("2026/07/19 10:15:30.125 INFO hi").unwrap();
        assert_eq!(format_ms(ms), "2026-07-19 10:15:30.125");
    }
}
