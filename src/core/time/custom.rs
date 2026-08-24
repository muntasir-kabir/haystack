//! User-defined custom date formats (regex-based with named capture groups).
//!
//! A custom format is stored persistently as a [`CustomDateFormat`] (name +
//! regex text, JSON-serializable) and compiled on demand into a
//! [`CustomTimeFormat`] that implements the [`TimeFormat`] trait, so custom
//! recognizers participate in the same detection + extraction pipeline as the
//! built-in families.
//!
//! The regex must capture the following **named groups** (case-sensitive):
//! `year`, `month`, `day`, `hour`, `min`, `sec` (all required) plus optional
//! `ms` (milliseconds) and `ampm` (`AM`/`PM`, for 12-hour clocks).
//!
//! Example for `2026-08-14 4:08:23.668 PM`:
//! ```text
//! (?P<year>\d{4})-(?P<month>\d{2})-(?P<day>\d{2}) (?P<hour>\d{1,2}):(?P<min>\d{2}):(?P<sec>\d{2})\.(?P<ms>\d{3}) (?P<ampm>[AP]M)
//! ```

use std::ops::Range;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::{window, TimeFormat};

/// A persisted custom date-format definition (name + regex text).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CustomDateFormat {
    pub name: String,
    pub regex: String,
}

impl CustomDateFormat {
    /// Compile the regex into a runnable [`CustomTimeFormat`].
    pub fn compile(&self) -> Result<CustomTimeFormat, String> {
        Regex::new(&self.regex)
            .map(|re| CustomTimeFormat {
                name: self.name.clone(),
                regex: re,
            })
            .map_err(|e| format!("Invalid regex: {e}"))
    }

    /// Check the regex against a sample line and, if it matches, return the
    /// parsed timestamp components. Errors (invalid regex, bad values) surface
    /// as `Err`, a non-match as `Ok(None)`.
    pub fn preview(&self, line: &str) -> Result<Option<TimeComponents>, String> {
        self.compile()?.components(line)
    }

    /// Whether this definition is structurally valid (regex compiles) and
    /// matches the sample with all required named groups present.
    pub fn validate(&self, sample: &str) -> Result<(), String> {
        match self.preview(sample) {
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err(
                "Regex did not match the sample line. It must be a valid Rust regex with the \
                 required named groups: year, month, day, hour, min, sec (and optional ms, ampm)."
                    .to_string(),
            ),
            Err(e) => Err(e),
        }
    }
}

/// A compiled custom date format ready for matching/extraction.
#[derive(Clone)]
pub struct CustomTimeFormat {
    pub name: String,
    pub regex: Regex,
}

impl CustomTimeFormat {
    /// Convert back to the persistable definition (for saving to disk).
    pub fn to_definition(&self) -> CustomDateFormat {
        CustomDateFormat {
            name: self.name.clone(),
            regex: self.regex.as_str().to_string(),
        }
    }

    /// Extract the named-group components from a line (within the search
    /// window). `Err` describes an invalid regex/value, `Ok(None)` is a no-match.
    pub fn components(&self, line: &str) -> Result<Option<TimeComponents>, String> {
        let caps = match self.regex.captures(window(line)) {
            Some(c) => c,
            None => return Ok(None),
        };
        let field = |n: &str| -> Option<i64> {
            caps.name(n)
                .and_then(|m| m.as_str().trim().parse::<i64>().ok())
        };
        let year = field("year").ok_or("missing required group 'year'")?;
        let month = field("month").ok_or("missing required group 'month'")?;
        let day = field("day").ok_or("missing required group 'day'")?;
        let hour = field("hour").ok_or("missing required group 'hour'")?;
        let min = field("min").ok_or("missing required group 'min'")?;
        let sec = field("sec").ok_or("missing required group 'sec'")?;
        let ms = field("ms").unwrap_or(0);

        if !(1..=12).contains(&month)
            || !(1..=31).contains(&day)
            || min > 59
            || sec > 60
            || !(0..=999).contains(&ms)
        {
            return Err(
                "one of the parsed values is out of range (month 1-12, day 1-31, \
                 min 0-59, sec 0-60, ms 0-999)"
                    .to_string(),
            );
        }

        let hour24;
        if let Some(ampm) = caps.name("ampm") {
            let a = ampm.as_str().trim();
            if !a.eq_ignore_ascii_case("am") && !a.eq_ignore_ascii_case("pm") {
                return Err("group 'ampm' must be 'AM' or 'PM'".to_string());
            }
            if !(1..=12).contains(&hour) {
                return Err("with 'ampm' present the hour must be 1-12".to_string());
            }
            let pm = a.eq_ignore_ascii_case("pm");
            hour24 = (hour % 12) + if pm { 12 } else { 0 };
        } else {
            if !(0..=23).contains(&hour) {
                return Err(
                    "hour must be 0-23 (or add an 'ampm' group for a 12-hour clock)".to_string(),
                );
            }
            hour24 = hour;
        }

        Ok(Some(TimeComponents {
            year,
            month,
            day,
            hour: hour24,
            min,
            sec,
            ms,
        }))
    }
}

impl TimeFormat for CustomTimeFormat {
    fn name(&self) -> &str {
        &self.name
    }

    fn matches(&self, line: &str) -> bool {
        self.regex.is_match(window(line))
    }

    fn extract(&self, line: &str) -> Option<(i64, Range<usize>)> {
        let m = self.regex.find(window(line))?;
        let comps = match self.components(line) {
            Ok(Some(c)) => c,
            _ => return None,
        };
        Some((comps.epoch_ms()?, m.range()))
    }
}
/// The per-component breakdown of a parsed custom timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeComponents {
    pub year: i64,
    pub month: i64,
    pub day: i64,
    /// 24-hour clock value (already converted when an `ampm` group was used).
    pub hour: i64,
    pub min: i64,
    pub sec: i64,
    pub ms: i64,
}

impl TimeComponents {
    /// Epoch millis, treating the parsed wall-clock as UTC (matching the
    /// other naive time families in this crate).
    pub fn epoch_ms(&self) -> Option<i64> {
        let days = days_from_civil(self.year, self.month, self.day);
        Some(
            days * 86_400_000
                + self.hour * 3_600_000
                + self.min * 60_000
                + self.sec * 1_000
                + self.ms,
        )
    }

    /// Human-readable verification string in the requested format.
    pub fn describe(&self) -> String {
        format!(
            "Year: {}, Month: {}, Date: {}, Hour: {}, Min: {}, Sec: {}, MILLI SECOND: {}",
            self.year, self.month, self.day, self.hour, self.min, self.sec, self.ms
        )
    }
}

/// Days since 1970-01-01 (Howard Hinnant's civil calendar algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::time::format_ms;

    fn twelve_hour() -> CustomDateFormat {
        CustomDateFormat {
            name: "12h".into(),
            regex: r"(?P<year>\d{4})-(?P<month>\d{2})-(?P<day>\d{2}) (?P<hour>\d{1,2}):(?P<min>\d{2}):(?P<sec>\d{2})\.(?P<ms>\d{3}) (?P<ampm>[AP]M)".into(),
        }
    }

    fn dash_milli() -> CustomDateFormat {
        CustomDateFormat {
            name: "dash".into(),
            regex: r"(?P<year>\d{4})-(?P<month>\d{2})-(?P<day>\d{2}) (?P<hour>\d{1,2}):(?P<min>\d{2}):(?P<sec>\d{2})\.(?P<ms>\d{3})".into(),
        }
    }

    #[test]
    fn parses_12h_components_with_ampm_conversion() {
        let def = twelve_hour();
        let comps = def
            .preview("2026-08-14 4:08:23.668 PM [com]")
            .unwrap()
            .unwrap();
        assert_eq!(comps.year, 2026);
        assert_eq!(comps.month, 8);
        assert_eq!(comps.day, 14);
        assert_eq!(comps.hour, 16); // 4 PM → 16
        assert_eq!(comps.min, 8);
        assert_eq!(comps.sec, 23);
        assert_eq!(comps.ms, 668);
        assert_eq!(
            format_ms(comps.epoch_ms().unwrap()),
            "2026-08-14 16:08:23.668"
        );
        assert_eq!(
            comps.describe(),
            "Year: 2026, Month: 8, Date: 14, Hour: 16, Min: 8, Sec: 23, MILLI SECOND: 668"
        );
    }

    #[test]
    fn parses_24h_components() {
        let def = dash_milli();
        let comps = def
            .preview("2026-08-14 09:05:03.250 prefix")
            .unwrap()
            .unwrap();
        assert_eq!(comps.hour, 9);
        assert_eq!(
            format_ms(comps.epoch_ms().unwrap()),
            "2026-08-14 09:05:03.250"
        );
    }

    #[test]
    fn rejects_invalid_regex() {
        let def = CustomDateFormat {
            name: "bad".into(),
            regex: r"(unclosed".into(),
        };
        assert!(def.compile().is_err());
        assert!(def.preview("anything").is_err());
    }

    #[test]
    fn rejects_missing_named_groups() {
        let def = CustomDateFormat {
            name: "incomplete".into(),
            regex: r"(?P<year>\d{4})-x".into(),
        };
        assert!(def.preview("2026-x").is_err());
    }

    #[test]
    fn returns_none_when_no_match() {
        let def = twelve_hour();
        assert!(def.preview("no timestamp here").unwrap().is_none());
    }

    #[test]
    fn custom_implements_timeformat_extract() {
        let def = twelve_hour();
        let compiled = def.compile().unwrap();
        let line = "2026-08-14 4:08:23.668 PM [com]";
        let (ms, span) = compiled.extract(line).unwrap();
        assert_eq!(format_ms(ms), "2026-08-14 16:08:23.668");
        assert_eq!(&line[span], "2026-08-14 4:08:23.668 PM");
        assert!(compiled.matches(line));
        assert_eq!(compiled.name(), "12h");
        assert_eq!(compiled.to_definition(), def);
    }

    #[test]
    fn serde_round_trips_list() {
        let defs = vec![twelve_hour(), dash_milli()];
        let json = serde_json::to_string_pretty(&defs).unwrap();
        let back: Vec<CustomDateFormat> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, defs);
    }
}
