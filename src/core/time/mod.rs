//! Time-format detection & extraction.
//!
//! One module per timestamp family (ISO-8601, `YYYY/MM/DD`, BSD syslog, Apache
//! CLF, epoch seconds/millis, logcat threadtime, glog). Each implements
//! [`TimeFormat`] and registers itself in [`TIME_FORMATS`]; [`TimeDetector`]
//! samples a few leading lines and picks the dominant family, then uses that
//! single fast path for the rest of the file. All values are normalized to
//! Unix epoch **milliseconds** (UTC).
//!
//! Adding a new timestamp family = add one `foo.rs` (struct + impl) and one
//! entry in [`TIME_FORMATS`]. Nothing else needs to change.

mod apache;
mod custom;
mod epoch;
mod glog;
mod iso;
mod iso12;
mod logcat_threadtime;
mod numeric;
mod rfc2822;
mod slash;
mod syslog;

use std::ops::Range;
use std::sync::Arc;

use chrono::{DateTime, Datelike, Local, NaiveDateTime};

pub use apache::Apache;
pub use custom::{CustomDateFormat, CustomTimeFormat, TimeComponents};
pub use epoch::Epoch;
pub use glog::Glog;
pub use iso::Iso;
pub use iso12::Iso12Hour;
pub use logcat_threadtime::LogcatThreadtime;
pub use numeric::{DayFirstDash, DayFirstDot, UsDash, UsSlash, YearFirstDot};
pub use rfc2822::Rfc2822;
pub use slash::Slash;
pub use syslog::Syslog;

/// How far into a line we look for a timestamp. Timestamps virtually always
/// live at (or near) the start of a log line; capping the search window keeps
/// the per-line cost tiny on 50MB+ files.
pub(crate) const SEARCH_WINDOW: usize = 96;

/// A pluggable timestamp family: a recognizer ([`TimeFormat::matches`]) and an
/// extractor ([`TimeFormat::extract`]) that returns epoch millis + byte span.
pub trait TimeFormat: Send + Sync {
    /// Human-readable family name (used in tests and diagnostics).
    fn name(&self) -> &str;

    /// Cheap recognizer: does this line carry this timestamp family?
    fn matches(&self, line: &str) -> bool;

    /// Extract (epoch millis, byte span) from a line, if this family applies.
    fn extract(&self, line: &str) -> Option<(i64, Range<usize>)>;

    /// Recognize the complete timestamp-shaped token even when conversion
    /// fails (for example, an impossible calendar date). Most families can
    /// use successful extraction as their structural recognizer; parsers with
    /// a separate lexical grammar override this method.
    fn recognize(&self, line: &str) -> Option<Range<usize>> {
        self.extract(line).map(|(_, span)| span)
    }
}

/// All registered time formats, in detection priority order (most common first).
pub static TIME_FORMATS: &[&'static dyn TimeFormat] = &[
    &Iso,
    &Slash,
    &UsSlash,
    &UsDash,
    &DayFirstDash,
    &DayFirstDot,
    &YearFirstDot,
    &Syslog,
    &Apache,
    &Epoch,
    &LogcatThreadtime,
    &Glog,
    &Iso12Hour,
    &Rfc2822,
];

/// Trim a line to the timestamp search window (zero-cost when already short).
pub(crate) fn window(line: &str) -> &str {
    if line.len() > SEARCH_WINDOW {
        line.get(..SEARCH_WINDOW).unwrap_or(line)
    } else {
        line
    }
}

/// A resolved timestamp family: either a built-in [`TimeFormat`] or a
/// user-defined custom format. Stored by documents so the per-line hot path can
/// call [`TimeFormatKind::extract`] without knowing which kind won detection.
#[derive(Clone)]
pub enum TimeFormatKind {
    BuiltIn(&'static dyn TimeFormat),
    Custom(Arc<CustomTimeFormat>),
    PinnedYearless(YearlessFamily, YearlessReference),
}

/// Clock snapshot used for yearless families. Tailing retains the same value
/// even if the system clock crosses midnight or New Year.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct YearlessReference {
    pub year: i32,
    pub epoch_seconds: i64,
    pub roll_back_future: bool,
}

impl YearlessReference {
    pub fn now() -> Self {
        let now = Local::now();
        Self {
            year: now.year(),
            epoch_seconds: now.timestamp(),
            roll_back_future: true,
        }
    }

    pub fn with_explicit_year(mut self, year: i32) -> Self {
        self.year = year;
        self.roll_back_future = false;
        self
    }
}

#[derive(Clone, Copy)]
pub enum YearlessFamily {
    Syslog,
    LogcatThreadtime,
    Glog,
}

impl TimeFormatKind {
    pub fn name(&self) -> String {
        match self {
            TimeFormatKind::BuiltIn(f) => f.name().to_string(),
            TimeFormatKind::Custom(c) => c.name.clone(),
            TimeFormatKind::PinnedYearless(family, _) => match family {
                YearlessFamily::Syslog => "BSD syslog",
                YearlessFamily::LogcatThreadtime => "logcat threadtime",
                YearlessFamily::Glog => "glog",
            }
            .to_string(),
        }
    }

    pub fn extract(&self, line: &str) -> Option<(i64, Range<usize>)> {
        match self {
            TimeFormatKind::BuiltIn(f) => f.extract(line),
            TimeFormatKind::Custom(c) => c.extract(line),
            TimeFormatKind::PinnedYearless(family, reference) => match family {
                YearlessFamily::Syslog => syslog::extract_at(line, *reference),
                YearlessFamily::LogcatThreadtime => logcat_threadtime::extract_at(line, *reference),
                YearlessFamily::Glog => glog::extract_at(line, *reference),
            },
        }
    }

    pub fn recognize(&self, line: &str) -> Option<Range<usize>> {
        match self {
            TimeFormatKind::BuiltIn(format) => format.recognize(line),
            TimeFormatKind::Custom(custom) => custom.recognize(line),
            TimeFormatKind::PinnedYearless(_, _) => self.extract(line).map(|(_, span)| span),
        }
    }

    pub fn pin_yearless(self, reference: YearlessReference) -> Self {
        match &self {
            TimeFormatKind::BuiltIn(format) => match format.name() {
                "BSD syslog" => Self::PinnedYearless(YearlessFamily::Syslog, reference),
                "logcat threadtime" => {
                    Self::PinnedYearless(YearlessFamily::LogcatThreadtime, reference)
                }
                "glog" => Self::PinnedYearless(YearlessFamily::Glog, reference),
                _ => self,
            },
            _ => self,
        }
    }

    pub fn yearless_reference(&self) -> Option<YearlessReference> {
        match self {
            Self::PinnedYearless(_, reference) => Some(*reference),
            _ => None,
        }
    }
}

/// Samples lines and picks the dominant timestamp family.
pub struct TimeDetector;

impl TimeDetector {
    /// Detect among all registered families. Returns `None` for timeless text.
    pub fn detect<S: AsRef<str>>(
        sample: impl Iterator<Item = S>,
    ) -> Option<&'static dyn TimeFormat> {
        Self::detect_among(sample, TIME_FORMATS)
    }

    /// Detect among a restricted set of families (used by log formats that
    /// only allow a specific timestamp shape, e.g. RFC 5424 → ISO only).
    pub fn detect_among<S: AsRef<str>>(
        sample: impl Iterator<Item = S>,
        candidates: &'static [&'static dyn TimeFormat],
    ) -> Option<&'static dyn TimeFormat> {
        let lines: Vec<S> = sample.take(512).collect();
        if lines.is_empty() {
            return None;
        }
        let mut best: Option<(&'static dyn TimeFormat, usize)> = None;
        for &fmt in candidates {
            let hits = lines
                .iter()
                .filter(|l| fmt.extract(l.as_ref()).is_some())
                .count();
            if best.map_or(true, |(_, b)| hits > b) {
                best = Some((fmt, hits));
            }
        }
        // Require a meaningful hit rate so we don't latch onto noise.
        let (fmt, hits) = best?;
        let needed = (lines.len() / 4).max(2).min(lines.len());
        if hits >= needed {
            Some(fmt)
        } else {
            None
        }
    }

    /// Detect among built-in candidates plus the supplied custom formats,
    /// picking the dominant family (built-in or custom) as [`TimeFormatKind`].
    /// Applies the same hit-rate threshold as [`TimeDetector::detect_among`].
    pub fn detect_any<S: AsRef<str>>(
        sample: impl Iterator<Item = S>,
        builtin: &'static [&'static dyn TimeFormat],
        custom: &[CustomTimeFormat],
    ) -> Option<TimeFormatKind> {
        let lines: Vec<S> = sample.take(512).collect();
        if lines.is_empty() {
            return None;
        }
        let mut best: Option<(TimeFormatKind, usize)> = None;
        for &fmt in builtin {
            let hits = lines
                .iter()
                .filter(|l| fmt.extract(l.as_ref()).is_some())
                .count();
            if best.as_ref().map_or(true, |(_, b)| hits > *b) {
                best = Some((TimeFormatKind::BuiltIn(fmt), hits));
            }
        }
        for c in custom {
            let hits = lines
                .iter()
                .filter(|l| c.extract(l.as_ref()).is_some())
                .count();
            if best.as_ref().map_or(true, |(_, b)| hits > *b) {
                best = Some((TimeFormatKind::Custom(Arc::new(c.clone())), hits));
            }
        }
        let (kind, hits) = best?;
        let needed = (lines.len() / 4).max(2).min(lines.len());
        if hits >= needed {
            Some(kind)
        } else {
            None
        }
    }

    /// Choose from structurally prefiltered candidate-header lines. Unlike
    /// the legacy physical-line percentage vote, two independent valid
    /// headers are sufficient. Equal evidence is reported as unresolved.
    pub fn detect_any_from_headers<S: AsRef<str>>(
        sample: impl Iterator<Item = S>,
        builtin: &'static [&'static dyn TimeFormat],
        custom: &[CustomTimeFormat],
    ) -> Option<(TimeFormatKind, usize, Vec<String>)> {
        let lines: Vec<S> = sample.take(8_192).collect();
        let mut scores = builtin
            .iter()
            .map(|format| {
                let kind = TimeFormatKind::BuiltIn(*format);
                let hits = lines
                    .iter()
                    .filter(|line| kind.extract(line.as_ref()).is_some())
                    .count();
                (kind, hits)
            })
            .chain(custom.iter().cloned().map(|format| {
                let kind = TimeFormatKind::Custom(Arc::new(format));
                let hits = lines
                    .iter()
                    .filter(|line| kind.extract(line.as_ref()).is_some())
                    .count();
                (kind, hits)
            }))
            .filter(|(_, hits)| *hits > 0)
            .collect::<Vec<_>>();
        scores.sort_by(|left, right| right.1.cmp(&left.1));
        let (winner, hits) = scores.first().cloned()?;
        let conflicts = scores
            .iter()
            .skip(1)
            .filter(|(_, other_hits)| *other_hits == hits)
            .map(|(candidate, _)| candidate.name())
            .collect::<Vec<_>>();
        let enough = hits >= 2 || (lines.len() <= 4 && hits == 1);
        (enough && conflicts.is_empty()).then_some((winner, hits, conflicts))
    }

    /// Detect among all built-in families plus the supplied custom formats.
    pub fn detect_with_custom<S: AsRef<str>>(
        sample: impl Iterator<Item = S>,
        custom: &[CustomTimeFormat],
    ) -> Option<TimeFormatKind> {
        Self::detect_any(sample, TIME_FORMATS, custom)
    }
}

/// Render epoch millis as a compact UTC timestamp for display / MCP output.
pub fn format_ms(ms: i64) -> String {
    match chrono::DateTime::from_timestamp_millis(ms) {
        Some(dt) => dt.format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
        None => format!("{ms}"),
    }
}

/// Parse a user-supplied time value (RFC3339, "YYYY-MM-DD HH:MM:SS",
/// "YYYY-MM-DD", or epoch millis) into epoch millis. Used by the MCP server
/// and by field-based timestamps (e.g. JSON `time` fields).
pub fn parse_time_param(s: &str) -> Option<i64> {
    let s = s.trim();
    if let Ok(n) = s.parse::<i64>() {
        return Some(if s.len() <= 11 { n * 1000 } else { n });
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp_millis());
    }
    let normalized = s.replace('T', " ");
    for fmt in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S", "%Y-%m-%d"] {
        if let Ok(n) = NaiveDateTime::parse_from_str(&normalized, fmt) {
            return Some(n.and_utc().timestamp_millis());
        }
    }
    // Date-only form parses as NaiveDate.
    if let Ok(d) = chrono::NaiveDate::parse_from_str(&normalized, "%Y-%m-%d") {
        return d
            .and_hms_milli_opt(0, 0, 0, 0)
            .map(|n| n.and_utc().timestamp_millis());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect_name(line: &str) -> Option<&'static str> {
        TimeDetector::detect(std::iter::once(line)).map(|f| f.name())
    }

    #[test]
    fn detector_picks_intended_format() {
        assert_eq!(
            detect_name("2026-07-19T10:15:30.123Z INFO hello"),
            Some("ISO-8601")
        );
        assert_eq!(
            detect_name("2026/07/19 10:15:30 INFO slash"),
            Some("YYYY/MM/DD")
        );
        assert_eq!(
            detect_name("08/20/2026 10:15:30.125 INFO us"),
            Some("MM/DD/YYYY")
        );
        assert_eq!(
            detect_name("20-08-2026 10:15:30 INFO european"),
            Some("DD-MM-YYYY")
        );
        assert_eq!(
            detect_name("08-20-2026 10:15:30 INFO us"),
            Some("MM-DD-YYYY")
        );
        assert_eq!(
            detect_name("Thu, 20 Aug 2026 10:15:30 +0000 gateway"),
            Some("RFC 2822")
        );
        assert_eq!(
            detect_name("Jan  5 03:22:11 host sshd[1]: hi"),
            Some("BSD syslog")
        );
        assert_eq!(
            detect_name("127.0.0.1 - - [10/Oct/2024:13:55:36 +0000] \"GET /\""),
            Some("Apache CLF")
        );
        assert_eq!(detect_name("1784158530123 | INFO | event"), Some("epoch"));
        assert_eq!(
            detect_name("07-15 22:00:01.123  1234  5678 I Tag: hello"),
            Some("logcat threadtime")
        );
        assert_eq!(
            detect_name("I0715 22:00:01.123456 12345 server.cc:42] started"),
            Some("glog")
        );
    }

    #[test]
    fn rejects_timeless_text() {
        assert_eq!(detect_name("just a plain line of text, no time here"), None);
    }

    #[test]
    fn detect_among_restricts_candidates() {
        // A logcat-threadtime line should NOT be detected when only ISO is allowed.
        let allowed: &'static [&'static dyn TimeFormat] = &[&Iso];
        let result = TimeDetector::detect_among(
            std::iter::once("07-15 22:00:01.123  1234  5678 I Tag: hello"),
            allowed,
        );
        assert!(result.is_none());
        // But ISO still detects from the same candidate set.
        let result = TimeDetector::detect_among(
            std::iter::once("2026-07-19T10:15:30.123Z INFO hello"),
            allowed,
        );
        assert_eq!(result.map(|f| f.name()), Some("ISO-8601"));
    }

    #[test]
    fn parses_time_params() {
        assert_eq!(parse_time_param("1784158530123"), Some(1784158530123));
        assert_eq!(
            parse_time_param("2026-07-19T10:15:30Z"),
            parse_time_param("2026-07-19 10:15:30")
        );
        assert!(parse_time_param("not a time").is_none());
    }

    #[test]
    fn detector_picks_12_hour_iso_family() {
        assert_eq!(
            detect_name("2026-08-14 4:08:23.668 PM [com.apple.main-thread:18836] D hi"),
            Some("ISO-8601 12h AM/PM")
        );
    }

    #[test]
    fn detect_with_custom_wins_when_dominant() {
        let def = CustomDateFormat {
            name: "underscore-date".into(),
            regex: r"(?P<year>\d{4})_(?P<month>\d{2})_(?P<day>\d{2}) (?P<hour>\d{2}):(?P<min>\d{2}):(?P<sec>\d{2})".into(),
        };
        let compiled = def.compile().unwrap();
        let lines = vec![
            "2026_08_15 10:08:00 alpha",
            "2026_08_15 10:08:01 beta",
            "no timestamp here",
        ];
        let kind = TimeDetector::detect_with_custom(lines.iter(), &[compiled]).unwrap();
        assert!(matches!(kind, TimeFormatKind::Custom(_)));
        assert_eq!(kind.name(), "underscore-date");
    }

    #[test]
    fn detect_without_custom_still_falls_back_to_builtins() {
        let lines = vec!["2026-08-15 10:08:00 alpha", "2026-08-15 10:08:01 beta"];
        let kind = TimeDetector::detect_with_custom(lines.iter(), &[]).unwrap();
        assert!(matches!(kind, TimeFormatKind::BuiltIn(_)));
        assert_eq!(kind.name(), "ISO-8601");
    }

    #[test]
    fn pinned_yearless_families_keep_the_document_reference() {
        let reference = YearlessReference {
            year: 2024,
            epoch_seconds: 0,
            roll_back_future: false,
        };
        for (format, line) in [
            (TimeFormatKind::BuiltIn(&Syslog), "Jan  5 03:22:11 host"),
            (
                TimeFormatKind::BuiltIn(&LogcatThreadtime),
                "01-05 03:22:11.123  1234 5678 I Tag: hi",
            ),
            (
                TimeFormatKind::BuiltIn(&Glog),
                "I0105 03:22:11.123456 hello",
            ),
        ] {
            let pinned = format.pin_yearless(reference);
            assert_eq!(pinned.yearless_reference(), Some(reference));
            let value = pinned.extract(line).unwrap().0;
            assert_eq!(format_ms(value).get(..4), Some("2024"));
            assert_eq!(pinned.clone().extract(line).unwrap().0, value);
        }
    }
}
