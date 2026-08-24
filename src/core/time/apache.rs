//! Apache Common/Combined Log Format: `10/Oct/2024:13:55:36 -0700`.

use std::ops::Range;
use std::sync::LazyLock;

use chrono::DateTime;
use regex::Regex;

use super::{window, TimeFormat};

static RE_APACHE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d{2}/[A-Za-z]{3}/\d{4}:\d{2}:\d{2}:\d{2}\s[+-]\d{4}").unwrap());

/// Apache CLF `10/Oct/2024:13:55:36 -0700` timestamps.
pub struct Apache;

impl TimeFormat for Apache {
    fn name(&self) -> &'static str {
        "Apache CLF"
    }

    fn matches(&self, line: &str) -> bool {
        RE_APACHE.find(window(line)).is_some()
    }

    fn extract(&self, line: &str) -> Option<(i64, Range<usize>)> {
        let m = RE_APACHE.find(window(line))?;
        Some((parse_apache(m.as_str())?, m.range()))
    }
}

fn parse_apache(raw: &str) -> Option<i64> {
    DateTime::parse_from_str(raw, "%d/%b/%Y:%H:%M:%S %z")
        .ok()
        .map(|dt| dt.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::time::format_ms;

    #[test]
    fn matches_positive() {
        assert!(Apache.matches("127.0.0.1 - - [10/Oct/2024:13:55:36 +0000] \"GET /\""));
    }

    #[test]
    fn matches_negative() {
        assert!(!Apache.matches("2026-07-19 10:15:30 INFO hi"));
        assert!(!Apache.matches("plain text"));
    }

    #[test]
    fn extracts_apache() {
        let (ms, span) = Apache
            .extract("127.0.0.1 - - [10/Oct/2024:13:55:36 +0000] \"GET /\"")
            .unwrap();
        assert_eq!(format_ms(ms), "2024-10-10 13:55:36.000");
        assert_eq!(
            &"127.0.0.1 - - [10/Oct/2024:13:55:36 +0000] \"GET /\""[span],
            "10/Oct/2024:13:55:36 +0000"
        );
    }
}
