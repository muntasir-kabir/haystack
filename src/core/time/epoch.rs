//! Epoch timestamps (seconds or milliseconds) at the start of a line.

use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;

use super::{window, TimeFormat};

static RE_EPOCH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(\d{13}|\d{10})[\s,|\]]").unwrap());

/// Epoch seconds/millis timestamps (`1784158530123 | ...`).
pub struct Epoch;

impl TimeFormat for Epoch {
    fn name(&self) -> &'static str {
        "epoch"
    }

    fn matches(&self, line: &str) -> bool {
        RE_EPOCH.is_match(window(line))
    }

    fn extract(&self, line: &str) -> Option<(i64, Range<usize>)> {
        let caps = RE_EPOCH.captures(window(line))?;
        let m = caps.get(0)?;
        let digits = caps.get(1)?.as_str();
        let value: i64 = digits.parse().ok()?;
        let ts = match digits.len() {
            13 if (1_000_000_000_000..5_000_000_000_000).contains(&value) => Some(value),
            10 if (1_000_000_000..5_000_000_000).contains(&value) => Some(value * 1000),
            _ => None,
        }?;
        Some((ts, m.range()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_positive() {
        assert!(Epoch.matches("1784158530123 | INFO | event"));
        assert!(Epoch.matches("1784158530 | INFO | event"));
    }

    #[test]
    fn matches_negative() {
        assert!(!Epoch.matches("2026-07-19 10:15:30 INFO hi"));
        assert!(!Epoch.matches("plain text"));
    }

    #[test]
    fn extracts_epoch_millis() {
        let (ms, span) = Epoch.extract("1784158530123 | INFO | event").unwrap();
        assert_eq!(ms, 1784158530123);
        assert_eq!(&"1784158530123 | INFO | event"[span], "1784158530123 ");
    }

    #[test]
    fn extracts_epoch_seconds() {
        let (ms, span) = Epoch.extract("1784158530 | INFO | event").unwrap();
        assert_eq!(ms, 1_784_158_530_000);
        assert_eq!(&"1784158530 | INFO | event"[span], "1784158530 ");
    }
}
