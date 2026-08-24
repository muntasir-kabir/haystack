//! ISO-8601 / RFC 3339 timestamps: `YYYY-MM-DD[T ]HH:MM:SS[.fff][Z|±HH:MM]`.

use std::ops::Range;
use std::sync::LazyLock;

use chrono::{DateTime, NaiveDateTime};
use regex::Regex;

use super::{window, TimeFormat};

static RE_ISO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:[.,]\d+)?(?:\s?(?:Z|[+-]\d{2}:?\d{2}))?")
        .unwrap()
});

/// ISO-8601 / RFC 3339 timestamps — the most common log timestamp shape.
pub struct Iso;

impl TimeFormat for Iso {
    fn name(&self) -> &'static str {
        "ISO-8601"
    }

    fn matches(&self, line: &str) -> bool {
        RE_ISO.find(window(line)).is_some()
    }

    fn extract(&self, line: &str) -> Option<(i64, Range<usize>)> {
        let m = RE_ISO.find(window(line))?;
        Some((parse_iso(m.as_str())?, m.range()))
    }
}

fn parse_iso(raw: &str) -> Option<i64> {
    // Fast path: fixed-position scalar parse of the common ISO-8601 shape
    // (`YYYY-MM-DD[T ]HH:MM:SS[.fff][Z|±HH:MM]`). Chrono's strptime machinery
    // costs ~1µs/call; this is ~10ns. Falls back to chrono for oddballs.
    if let Some(ms) = parse_iso_fast(raw) {
        return Some(ms);
    }
    let mut s = raw.replace('T', " ").replace(',', ".");
    if let Some(stripped) = s.strip_suffix('Z') {
        s = format!("{stripped}+00:00");
    }
    // Zone markers only ever appear after the date portion (index >= 10);
    // the '-' at positions 4 and 7 are date separators, not zones.
    let tail = s.get(10..).unwrap_or("");
    let has_zone = tail.contains('+') || tail.contains('-');
    if has_zone {
        const ZONED: &[&str] = &[
            "%Y-%m-%d %H:%M:%S%.f%:z",
            "%Y-%m-%d %H:%M:%S%.f %:z",
            "%Y-%m-%d %H:%M:%S%.f%z",
            "%Y-%m-%d %H:%M:%S%.f %z",
        ];
        for fmt in ZONED {
            if let Ok(dt) = DateTime::parse_from_str(&s, fmt) {
                return Some(dt.timestamp_millis());
            }
        }
    }
    const NAIVE: &[&str] = &["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"];
    for fmt in NAIVE {
        if let Ok(naive) = NaiveDateTime::parse_from_str(&s, fmt) {
            return Some(naive.and_utc().timestamp_millis());
        }
    }
    None
}

/// Scalar ISO-8601 parser for the dominant shapes. Returns epoch millis.
fn parse_iso_fast(raw: &str) -> Option<i64> {
    let b = raw.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let digit = |i: usize| -> Option<i64> {
        let c = b[i];
        if c.is_ascii_digit() {
            Some((c - b'0') as i64)
        } else {
            None
        }
    };
    let num = |i: usize, n: usize| -> Option<i64> {
        let mut v = 0i64;
        for k in 0..n {
            v = v * 10 + digit(i + k)?;
        }
        Some(v)
    };
    if b[4] != b'-'
        || b[7] != b'-'
        || (b[10] != b'T' && b[10] != b' ')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let year = num(0, 4)?;
    let month = num(5, 2)?;
    let day = num(8, 2)?;
    let hour = num(11, 2)?;
    let min = num(14, 2)?;
    let sec = num(17, 2)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || min > 59 || sec > 60 {
        return None;
    }
    let mut ms = 0i64;
    let mut i = 19;
    // Fractional seconds: `.123` or `,123` (first 3 digits = millis).
    if i < b.len() && (b[i] == b'.' || b[i] == b',') {
        i += 1;
        let mut frac_digits = 0;
        while i < b.len() && b[i].is_ascii_digit() {
            if frac_digits < 3 {
                ms = ms * 10 + (b[i] - b'0') as i64;
            }
            frac_digits += 1;
            i += 1;
        }
        while frac_digits < 3 {
            ms *= 10;
            frac_digits += 1;
        }
    }
    // Zone: `Z` or `±HH[:]?MM`.
    let mut offset_ms = 0i64;
    if i < b.len() && b[i] == b'Z' {
        // UTC, no offset.
    } else if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        let sign = if b[i] == b'+' { 1i64 } else { -1i64 };
        i += 1;
        let zh = num(i, 2)?;
        i += 2;
        let zm = if i < b.len() && b[i] == b':' {
            i += 1;
            num(i, 2)?
        } else if i + 1 < b.len() {
            num(i, 2)?
        } else {
            0
        };
        offset_ms = sign * (zh * 3_600_000 + zm * 60_000);
    }
    let days = days_from_civil(year, month, day);
    let epoch_ms =
        days * 86_400_000 + hour * 3_600_000 + min * 60_000 + sec * 1_000 + ms - offset_ms;
    Some(epoch_ms)
}

/// Days since 1970-01-01 (Howard Hinnant's civil calendar algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (m + 9) % 12; // Mar=0..Feb=11
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::time::format_ms;

    #[test]
    fn matches_positive() {
        assert!(Iso.matches("2026-07-19T10:15:30.123Z INFO hello"));
        assert!(Iso.matches("2026-07-19 10:15:30,456 WARN something"));
        // ISO also appears inside JSON strings and RFC 5424 lines.
        assert!(Iso.matches("{\"time\": \"2026-08-15T19:40:01Z\", \"lvl\": 30}"));
        assert!(Iso.matches("<134>1 2026-08-15T19:40:20.123Z host app pid msgid - hi"));
    }

    #[test]
    fn matches_negative() {
        assert!(!Iso.matches("07-15 22:00:01.123  1234  5678 I Tag: hello"));
        assert!(!Iso.matches("1784158530123 | INFO | event"));
        assert!(!Iso.matches("Jan  5 03:22:11 host sshd[1]: hi"));
        assert!(!Iso.matches("plain text no time"));
    }

    #[test]
    fn extracts_iso_with_z() {
        let (ms, span) = Iso.extract("2026-07-19T10:15:30.123Z INFO hello").unwrap();
        assert_eq!(format_ms(ms), "2026-07-19 10:15:30.123");
        assert_eq!(
            &"2026-07-19T10:15:30.123Z INFO hello"[span],
            "2026-07-19T10:15:30.123Z"
        );
    }

    #[test]
    fn extracts_iso_space_comma_millis() {
        let (ms, span) = Iso
            .extract("2026-07-19 10:15:30,456 WARN something")
            .unwrap();
        assert_eq!(format_ms(ms), "2026-07-19 10:15:30.456");
        assert_eq!(
            &"2026-07-19 10:15:30,456 WARN something"[span],
            "2026-07-19 10:15:30,456"
        );
    }

    #[test]
    fn extracts_iso_with_offset() {
        let (ms, span) = Iso.extract("2026-07-19T10:15:30+06:00 INFO tz").unwrap();
        // 10:15:30 at +06:00 == 04:15:30 UTC
        assert_eq!(format_ms(ms), "2026-07-19 04:15:30.000");
        assert_eq!(
            &"2026-07-19T10:15:30+06:00 INFO tz"[span],
            "2026-07-19T10:15:30+06:00"
        );
    }
}
