//! RFC 2822 / email-style timestamps sometimes emitted by mail and gateway logs.

use std::ops::Range;
use std::sync::LazyLock;

use chrono::DateTime;
use regex::Regex;

use super::{window, TimeFormat};

static RE_RFC2822: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:Mon|Tue|Wed|Thu|Fri|Sat|Sun),?\s+\d{1,2}\s+(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)\s+\d{4}\s+\d{2}:\d{2}:\d{2}\s+(?:[+-]\d{4}|GMT|UTC)").unwrap()
});

pub struct Rfc2822;

impl TimeFormat for Rfc2822 {
    fn name(&self) -> &'static str {
        "RFC 2822"
    }

    fn matches(&self, line: &str) -> bool {
        RE_RFC2822.find(window(line)).is_some()
    }

    fn extract(&self, line: &str) -> Option<(i64, Range<usize>)> {
        let m = RE_RFC2822.find(window(line))?;
        let raw = m.as_str().replace("GMT", "+0000").replace("UTC", "+0000");
        let normalized = if raw.contains(",") {
            raw
        } else {
            raw.replacen(' ', ", ", 1)
        };
        let dt = DateTime::parse_from_str(&normalized, "%a, %d %b %Y %H:%M:%S %z").ok()?;
        Some((dt.timestamp_millis(), m.range()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::time::format_ms;

    #[test]
    fn extracts_rfc2822_timestamp() {
        let line = "Date: Thu, 20 Aug 2026 10:15:30 +0600 gateway accepted";
        let (ms, span) = Rfc2822.extract(line).unwrap();
        assert_eq!(format_ms(ms), "2026-08-20 04:15:30.000");
        assert_eq!(&line[span], "Thu, 20 Aug 2026 10:15:30 +0600");
    }
}
