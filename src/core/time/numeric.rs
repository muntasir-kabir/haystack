//! Common numeric date layouts found in application and platform logs.

use std::ops::Range;
use std::sync::LazyLock;

use chrono::NaiveDateTime;
use regex::Regex;

use super::{window, TimeFormat};

static RE_US_SLASH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*\d{1,2}/\d{1,2}/\d{4}[ T]\d{2}:\d{2}:\d{2}(?:[.,]\d+)?").unwrap()
});
static RE_DAY_DASH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*\d{1,2}-\d{1,2}-\d{4}[ T]\d{2}:\d{2}:\d{2}(?:[.,]\d+)?").unwrap()
});
static RE_DAY_DOT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*\d{1,2}\.\d{1,2}\.\d{4}[ T]\d{2}:\d{2}:\d{2}(?:[.,]\d+)?").unwrap()
});
static RE_YEAR_DOT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*\d{4}\.\d{1,2}\.\d{1,2}[ T]\d{2}:\d{2}:\d{2}(?:[.,]\d+)?").unwrap()
});

pub struct UsSlash;
pub struct UsDash;
pub struct DayFirstDash;
pub struct DayFirstDot;
pub struct YearFirstDot;

macro_rules! numeric_format {
    ($type:ident, $name:literal, $regex:ident, $pattern:literal) => {
        impl TimeFormat for $type {
            fn name(&self) -> &'static str {
                $name
            }

            fn matches(&self, line: &str) -> bool {
                $regex.find(window(line)).is_some()
            }

            fn extract(&self, line: &str) -> Option<(i64, Range<usize>)> {
                let m = $regex.find(window(line))?;
                let raw = m.as_str().trim();
                let normalized = raw.replace('T', " ").replace(',', ".");
                let n = NaiveDateTime::parse_from_str(&normalized, $pattern).ok()?;
                Some((n.and_utc().timestamp_millis(), m.range()))
            }
        }
    };
}

numeric_format!(UsSlash, "MM/DD/YYYY", RE_US_SLASH, "%m/%d/%Y %H:%M:%S%.f");
numeric_format!(UsDash, "MM-DD-YYYY", RE_DAY_DASH, "%m-%d-%Y %H:%M:%S%.f");
numeric_format!(
    DayFirstDash,
    "DD-MM-YYYY",
    RE_DAY_DASH,
    "%d-%m-%Y %H:%M:%S%.f"
);
numeric_format!(
    DayFirstDot,
    "DD.MM.YYYY",
    RE_DAY_DOT,
    "%d.%m.%Y %H:%M:%S%.f"
);
numeric_format!(
    YearFirstDot,
    "YYYY.MM.DD",
    RE_YEAR_DOT,
    "%Y.%m.%d %H:%M:%S%.f"
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::time::format_ms;

    #[test]
    fn extracts_common_numeric_layouts() {
        let cases: &[(&dyn TimeFormat, &str, &str)] = &[
            (
                &UsSlash,
                "08/20/2026 10:15:30.125 INFO",
                "2026-08-20 10:15:30.125",
            ),
            (
                &UsDash,
                "08-20-2026 10:15:30 INFO",
                "2026-08-20 10:15:30.000",
            ),
            (
                &DayFirstDash,
                "20-08-2026 10:15:30,250 WARN",
                "2026-08-20 10:15:30.250",
            ),
            (
                &DayFirstDot,
                "20.08.2026 10:15:30 DEBUG",
                "2026-08-20 10:15:30.000",
            ),
            (
                &YearFirstDot,
                "2026.08.20 10:15:30.5 INFO",
                "2026-08-20 10:15:30.500",
            ),
        ];
        for (format, line, expected) in cases {
            let (ms, _) = format.extract(line).unwrap();
            assert_eq!(format_ms(ms), *expected);
        }
    }

    #[test]
    fn regional_formats_do_not_match_iso_or_plain_text() {
        assert!(!UsSlash.matches("2026-08-20 10:15:30 INFO"));
        assert!(UsDash.extract("20-08-2026 10:15:30 INFO").is_none());
        assert!(!DayFirstDash.matches("plain text"));
        assert!(!DayFirstDot.matches("20/08/2026 10:15:30"));
    }
}
