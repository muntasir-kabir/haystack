//! 12-hour ISO-style timestamps: `YYYY-MM-DD H:MM:SS[.fff] AM/PM`.
//!
//! A distinct family from [`crate::core::time::Iso`]: the hour is **not**
//! zero-padded (single digit allowed) and the clock is **12-hour with an
//! `AM`/`PM` marker**. This is a common shape in macOS/iOS console dumps, e.g.
//! `2026-08-14 4:08:23.668 PM`. The `AM`/`PM` marker may be preceded by a
//! regular space or the narrow / non-breaking spaces (`U+202F`, `U+00A0`)
//! that OS console output sometimes inserts before the suffix.

use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;

use super::{window, TimeFormat};

static RE_ISO12: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\d{4}-\d{2}-\d{2}[T ]\d{1,2}:\d{2}:\d{2}(?:[.,]\d+)?[\s\u{202F}\u{00A0}]?[ap]m",
    )
    .unwrap()
});

/// 12-hour ISO-style timestamps (`2026-08-14 4:08:23.668 PM`).
pub struct Iso12Hour;

impl TimeFormat for Iso12Hour {
    fn name(&self) -> &str {
        "ISO-8601 12h AM/PM"
    }

    fn matches(&self, line: &str) -> bool {
        RE_ISO12.is_match(window(line))
    }

    fn extract(&self, line: &str) -> Option<(i64, Range<usize>)> {
        let m = RE_ISO12.find(window(line))?;
        Some((parse_iso12(m.as_str())?, m.range()))
    }
}

/// Parse a `YYYY-MM-DD H:MM:SS[.fff] AM/PM` string into epoch millis (UTC).
/// Supports 1- or 2-digit hours and performs the 12h→24h conversion.
fn parse_iso12(raw: &str) -> Option<i64> {
    let b = raw.as_bytes();
    // The date portion is fixed-width: `YYYY-MM-DD` then `T`/` `.
    if b.len() < 16 || b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b' ') {
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
    let year = num(0, 4)?;
    let month = num(5, 2)?;
    let day = num(8, 2)?;

    // Hour width is 1 or 2 digits; detect by whether the char right after the
    // first hour digit is the `:` (1-digit hour) or another digit (2-digit).
    let hlen = if b.get(12) == Some(&b':') { 1 } else { 2 };
    let colon1 = 11 + hlen;
    if b.get(colon1) != Some(&b':') {
        return None;
    }
    let hour = num(11, hlen)?;
    let min = num(colon1 + 1, 2)?;
    let colon2 = colon1 + 3;
    if b.get(colon2) != Some(&b':') {
        return None;
    }
    let sec = num(colon2 + 1, 2)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 12 || min > 59 || sec > 60 {
        return None;
    }

    // Fractional seconds: `.123` or `,123` (first 3 digits = millis).
    let mut ms = 0i64;
    let mut i = colon2 + 3;
    if b.get(i) == Some(&b'.') || b.get(i) == Some(&b',') {
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

    // Skip the optional space / narrow-no-break space before the AM/PM mark.
    while i < b.len() {
        if b[i] == b' ' || b[i] == b'\t' {
            i += 1;
        } else if b[i] == 0xC2 {
            i += 2; // U+00A0 (C2 A0)
        } else if b[i] == 0xE2 {
            i += 3; // U+202F (E2 80 AF)
        } else {
            break;
        }
    }

    // AM/PM marker (case-insensitive).
    let pm = b[i] == b'P' || b[i] == b'p';
    let am = b[i] == b'A' || b[i] == b'a';
    if !(am || pm) {
        return None;
    }
    let hour24 = (hour % 12) + if pm { 12 } else { 0 };

    let days = days_from_civil(year, month, day);
    Some(days * 86_400_000 + hour24 * 3_600_000 + min * 60_000 + sec * 1_000 + ms)
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

    #[test]
    fn matches_positive() {
        assert!(Iso12Hour.matches("2026-08-14 4:08:23.668 PM [com.apple] hi"));
        assert!(Iso12Hour.matches("2026-08-14 04:08:23.668 AM thing"));
        assert!(Iso12Hour.matches("2026-08-14 12:00:00 PM"));
        assert!(Iso12Hour.matches("2026-08-14 12:00:00 AM"));
        // Narrow no-break space before PM (U+202F).
        assert!(Iso12Hour.matches("2026-08-14 4:08:23.668\u{202f}PM [com]"));
        // Non-breaking space (U+00A0) before PM.
        assert!(Iso12Hour.matches("2026-08-14 4:08:23.668\u{a0}PM"));
    }

    #[test]
    fn matches_negative() {
        // 24-hour clock without any AM/PM marker is NOT this family.
        assert!(!Iso12Hour.matches("2026-08-14 16:08:23 thing"));
        assert!(!Iso12Hour.matches("2026-08-14 4:08:23"));
        assert!(!Iso12Hour.matches("plain text no time"));
        assert!(!Iso12Hour.matches("Jan  5 03:22:11 host hi"));
    }

    #[test]
    fn extracts_pm_single_digit_hour() {
        // The exact sample.log shape → 4:08:23.668 PM == 16:08:23.668 UTC.
        let (ms, span) = Iso12Hour
            .extract("2026-08-14 4:08:23.668 PM [com.apple.main-thread:18836] D hi")
            .unwrap();
        assert_eq!(format_ms(ms), "2026-08-14 16:08:23.668");
        assert_eq!(
            &"2026-08-14 4:08:23.668 PM [com]"[span],
            "2026-08-14 4:08:23.668 PM"
        );
    }

    #[test]
    fn extracts_am() {
        let (ms, _) = Iso12Hour.extract("2026-08-14 04:08:23.123 AM").unwrap();
        assert_eq!(format_ms(ms), "2026-08-14 04:08:23.123");
    }

    #[test]
    fn extracts_midnight_and_noon_edges() {
        let (noon, _) = Iso12Hour.extract("2026-08-14 12:15:30.000 PM").unwrap();
        assert_eq!(format_ms(noon), "2026-08-14 12:15:30.000");
        let (mid, _) = Iso12Hour.extract("2026-08-14 12:15:30.000 AM").unwrap();
        assert_eq!(format_ms(mid), "2026-08-14 00:15:30.000");
    }

    #[test]
    fn extracts_lowercase_marker() {
        let (ms, span) = Iso12Hour.extract("2026-08-14 4:08:23.668 pm").unwrap();
        assert_eq!(format_ms(ms), "2026-08-14 16:08:23.668");
        assert_eq!(
            &"2026-08-14 4:08:23.668 pm"[span],
            "2026-08-14 4:08:23.668 pm"
        );
    }
}
