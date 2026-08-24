//! Log-format detection & normalization.
//!
//! One module per log-format family (JSON, CEF, RFC 5424, logcat brief, Apple
//! Unified Logging, iOS OSLog console, and the `plain` unstructured fallback).
//! Each implements [`LogFormat`] and registers in [`FORMATS`]; [`FormatDetector`] samples
//! leading lines and picks the dominant family before the analysis pass.
//!
//! Pipeline: `FormatDetector` → `TimeDetector` (format-constrained) →
//! per-format [`LogFormat::normalize`] → Drain. Adding a new format = one
//! `foo.rs` (struct + impl) + one entry in [`FORMATS`]. Nothing else changes.

mod cef;
mod json;
mod logcat_brief;
mod os_log;
mod oslog_console;
mod plain;
mod rfc5424;

use std::borrow::Cow;
use std::ops::Range;

use crate::core::masking::{LogMasker, MaskCache};
use crate::core::time::{CustomTimeFormat, TimeDetector, TimeFormat, TimeFormatKind};

pub use cef::Cef;
pub use json::Json;
pub use logcat_brief::LogcatBrief;
pub use os_log::OsLog;
pub use oslog_console::OsLogConsole;
pub use plain::Plain;
pub use rfc5424::Rfc5424;

pub(crate) use plain::learn_header_slots;

/// Per-document state shared with every format's `normalize` call.
pub struct FormatContext<'a> {
    pub masker: &'a LogMasker,
    pub mask_cache: &'a mut MaskCache,
    /// Learned header slots (used by the `plain` generic path).
    pub header_slots: &'a [Option<&'static str>],
}

/// The per-line output of [`LogFormat::normalize`].
pub struct Normalized<'a> {
    /// Extracted timestamp (epoch millis + byte span). `None` when timeless.
    pub ts: Option<(i64, Range<usize>)>,
    /// The canonical string fed to Drain (already masked/normalized).
    pub content: Cow<'a, str>,
}

/// A pluggable log format: a recognizer ([`LogFormat::matches`]), a set of
/// allowed timestamp families ([`LogFormat::time_formats`]), and a normalizer
/// ([`LogFormat::normalize`]) that turns a raw line into Drain input.
pub trait LogFormat: Send + Sync {
    /// Human-readable name (used in tests and diagnostics).
    fn name(&self) -> &'static str;

    /// Cheap recognizer: does this line start with this format's signature?
    fn matches(&self, line: &str) -> bool;

    /// Timestamp families this format may use. Empty = timeless (no timeline).
    fn time_formats(&self) -> &'static [&'static dyn TimeFormat];

    /// Whether this format uses the generic learned-header masking path.
    fn uses_learned_header(&self) -> bool {
        false
    }

    /// Normalize one line into (timestamp, Drain content).
    fn normalize<'a>(
        &self,
        line: &'a str,
        ts: Option<(i64, Range<usize>)>,
        ctx: &mut FormatContext<'_>,
    ) -> Normalized<'a>;
}

/// Structured formats, in detection priority order (most specific first).
/// `Plain` is deliberately absent — it matches everything and is the fallback.
pub static FORMATS: &[&'static dyn LogFormat] =
    &[&Json, &Cef, &Rfc5424, &OsLog, &LogcatBrief, &OsLogConsole];

/// Samples lines and picks the dominant log format (falls back to `Plain`).
pub struct FormatDetector;

impl FormatDetector {
    pub fn detect<S: AsRef<str>>(sample: impl Iterator<Item = S>) -> &'static dyn LogFormat {
        let lines: Vec<S> = sample.take(512).collect();
        if lines.is_empty() {
            return &Plain;
        }
        let mut best: Option<(&'static dyn LogFormat, usize)> = None;
        for &fmt in FORMATS {
            let hits = lines.iter().filter(|l| fmt.matches(l.as_ref())).count();
            if best.map_or(true, |(_, b)| hits > b) {
                best = Some((fmt, hits));
            }
        }
        let (fmt, hits) = match best {
            Some(x) => x,
            None => return &Plain,
        };
        // Require a meaningful hit rate so a few stray structured lines in an
        // otherwise-plain file don't flip the whole file's format.
        let needed = (lines.len() / 4).max(2).min(lines.len());
        if hits >= needed {
            fmt
        } else {
            &Plain
        }
    }

    /// Resolve the timestamp family for a detected format from a sample.
    pub fn detect_time<S: AsRef<str>>(
        sample: impl Iterator<Item = S>,
        format: &'static dyn LogFormat,
    ) -> Option<&'static dyn TimeFormat> {
        TimeDetector::detect_among(sample, format.time_formats())
    }

    /// Resolve the timestamp family for a detected format from a sample,
    /// considering the format's built-in families plus user-supplied custom
    /// recognizers. Returns a [`TimeFormatKind`] that may be built-in or custom.
    pub fn detect_time_custom<S: AsRef<str>>(
        sample: impl Iterator<Item = S>,
        format: &'static dyn LogFormat,
        custom: &[CustomTimeFormat],
    ) -> Option<TimeFormatKind> {
        TimeDetector::detect_any(sample, format.time_formats(), custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sample lines for every supported format (and a couple of plain ones).
    fn sample_lines() -> Vec<(&'static str, &'static str)> {
        vec![
            ("json", "{\"time\": \"2026-08-15T19:40:01Z\", \"lvl\": 30, \"msg\": \"Page load: /home\"}"),
            ("cef", "CEF:0|VendorX|AppY|1.0|100|Login Success|3|suser=mkabir spt=443"),
            ("rfc5424", "<134>1 2026-08-15T19:40:20.123Z srv-alpha auth-api 1201 tx_882 - Login successful"),
            ("logcat_brief", "D/NetworkClient: Sending GET request to https://myserver.com"),
            ("os_log", "2026-08-15 19:40:30.123456+0300 0x1a2b3c Default 0x0 12345 2 com.app: Transitioning"),
            ("oslog_console", "[UI:Navigation] INFO: Transitioning from HomeView to SettingsView"),
            ("plain", "2026-08-15 19:40:30 [INFO] [AuthService] - Login attempt for user: mkabir"),
        ]
    }

    /// The validator: exactly one (or zero) structured format matches a line.
    #[test]
    fn single_line_matches_at_most_one_format() {
        for (_, line) in sample_lines() {
            let matched: Vec<&'static str> = FORMATS
                .iter()
                .filter(|f| f.matches(line))
                .map(|f| f.name())
                .collect();
            assert!(
                matched.len() <= 1,
                "line matched {} formats: {matched:?}\n  line: {line}",
                matched.len()
            );
        }
    }

    /// The validator: the detector picks the intended format for each sample.
    #[test]
    fn detector_picks_intended_format() {
        for (expected, line) in sample_lines() {
            let detected = FormatDetector::detect(std::iter::repeat(line).take(4)).name();
            assert_eq!(detected, expected, "for line: {line}");
        }
    }

    /// A mostly-plain file with a couple of stray structured lines stays plain.
    #[test]
    fn stray_structured_lines_do_not_flip_format() {
        let mut lines: Vec<String> = Vec::new();
        for i in 0..100 {
            lines.push(format!(
                "2026-08-15 19:40:{:02} [INFO] plain log line {i}",
                i % 60
            ));
        }
        lines.push("CEF:0|Vendor|Product|1.0|100|Name|3|".to_string());
        lines.push("{\"lvl\": 30}".to_string());
        let detected = FormatDetector::detect(lines.iter().map(|s| s.as_str())).name();
        assert_eq!(detected, "plain");
    }

    /// The detector resolves a timestamp family for a structured format.
    #[test]
    fn detect_time_uses_format_time_formats() {
        // CEF is timeless → None even though ISO lines exist elsewhere.
        let cef_time = FormatDetector::detect_time(
            std::iter::once("CEF:0|Vendor|Product|1.0|100|Name|3|"),
            &Cef,
        );
        assert!(cef_time.is_none());
        // RFC 5424 → ISO.
        let rfc_time = FormatDetector::detect_time(
            std::iter::once("<134>1 2026-08-15T19:40:20.123Z srv auth-api 1201 tx - hi"),
            &Rfc5424,
        );
        assert_eq!(rfc_time.map(|f| f.name()), Some("ISO-8601"));
    }
}
