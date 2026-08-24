//! Apple Unified Logging System (ULS) — the columnar `log show` text exports.
//!
//! Recognizes the fixed-column output of `log show` (default and
//! `--style compact`), e.g.:
//!
//! ```text
//! 2026-08-15 19:40:30.123456+0300 0x1a2b3c Default 0x0 12345 2 com.app: message
//! 2026-08-15 19:40:30.123456+0300 Df Default 0x0 12345 2 [Nav] com.app: message
//! ```
//!
//! (The raw on-disk `tracev3` store is binary and out of scope for a
//! line-oriented analyzer; this handles the *text* form.)

use std::borrow::Cow;
use std::ops::Range;

use crate::core::time::{Iso, TimeFormat};

use super::{FormatContext, LogFormat, Normalized};

/// Unified logging message types (`log show` "Type" column).
const LEVELS: &[&str] = &[
    "Default", "Info", "Debug", "Error", "Fault", "Activity", "Notice", "Trace",
];

/// `--style compact` two-char type codes (right after the timestamp).
const TYPE_CODES: &[&str] = &["Df", "Db", "I", "E", "F", "A", "N", "T"];

/// Apple Unified Logging System text exports (`log show` columnar format).
pub struct OsLog;

/// ULS uses an ISO-8601 timestamp with microsecond precision + `±HHMM` offset.
static OS_LOG_TIMES: &[&'static dyn TimeFormat] = &[&Iso];

impl LogFormat for OsLog {
    fn name(&self) -> &'static str {
        "os_log"
    }

    fn matches(&self, line: &str) -> bool {
        let t = line.trim_start();
        let Some((_, span)) = Iso.extract(t) else {
            return false;
        };
        // The timestamp must lead the line (ULS always prints it first).
        if span.start != 0 {
            return false;
        }
        // Right after the timestamp: a `0x…` thread/activity id and a level word.
        let after = &t[span.end..];
        let mut saw_hex = false;
        let mut saw_level = false;
        for tok in after.split_whitespace().take(6) {
            if is_hex_token(tok) {
                saw_hex = true;
            }
            if is_level_word(tok) {
                saw_level = true;
            }
        }
        saw_hex && saw_level
    }

    fn time_formats(&self) -> &'static [&'static dyn TimeFormat] {
        OS_LOG_TIMES
    }

    fn normalize<'a>(
        &self,
        line: &'a str,
        ts: Option<(i64, Range<usize>)>,
        ctx: &mut FormatContext<'_>,
    ) -> Normalized<'a> {
        Normalized {
            ts,
            content: build_content(line, ctx),
        }
    }
}

fn is_hex_token(tok: &str) -> bool {
    tok.len() >= 3
        && (tok.starts_with("0x") || tok.starts_with("0X"))
        && tok[2..].bytes().all(|b| b.is_ascii_hexdigit())
}

fn is_level_word(tok: &str) -> bool {
    LEVELS.contains(&tok)
}

fn is_type_code(tok: &str) -> bool {
    TYPE_CODES.contains(&tok)
}

fn build_content<'a>(line: &'a str, ctx: &mut FormatContext<'_>) -> Cow<'a, str> {
    let t = line.trim_start();
    let Some((_, span)) = Iso.extract(t) else {
        return ctx.masker.mask_with_header(line, &[], ctx.mask_cache);
    };
    let after = &t[span.end..];
    let mut level = "Default";
    let mut body = String::with_capacity(after.len());
    for (i, tok) in after.split_whitespace().enumerate() {
        if is_hex_token(tok) {
            continue; // thread / activity id (noise)
        }
        if is_level_word(tok) {
            level = tok;
            continue;
        }
        if i == 0 && is_type_code(tok) {
            continue; // `--style compact` type code right after the timestamp
        }
        if tok.bytes().all(|b| b.is_ascii_digit()) {
            continue; // pid / ttl (noise)
        }
        body.push_str(tok);
        body.push(' ');
    }
    let content = format!("OSLOG {level} {}", body.trim_end());
    Cow::Owned(
        ctx.masker
            .mask_with_header(&content, &[], ctx.mask_cache)
            .into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::masking::{LogMasker, MaskCache};

    fn normalize(line: &str) -> String {
        let masker = LogMasker::default();
        let mut cache = MaskCache::default();
        let mut ctx = FormatContext {
            masker: &masker,
            mask_cache: &mut cache,
            header_slots: &[],
        };
        OsLog.normalize(line, None, &mut ctx).content.into_owned()
    }

    #[test]
    fn matches_positive() {
        assert!(OsLog.matches(
            "2026-08-15 19:40:30.123456+0300 0x1a2b3c Default 0x0 12345 2 com.app: Transitioning"
        ));
        assert!(OsLog.matches(
            "2026-08-15 19:40:30.123456+0300 Df Default 0x0 12345 2 [Nav] com.app: Transitioning"
        ));
        assert!(OsLog
            .matches("2026-08-15 19:40:30.123456+0300 0x1a2b3c Error 0x99 99 1 com.app: Failed"));
    }

    #[test]
    fn matches_negative() {
        // Plain ISO + all-caps INFO (no `0x…`, not a ULS level word).
        assert!(!OsLog.matches("2026-08-15 19:40:30.123456+0300 INFO message"));
        // No leading timestamp → excluded (JSON / CEF / RFC 5424 / logcat).
        assert!(!OsLog.matches("{\"time\": \"2026-08-15T19:40:01Z\"}"));
        assert!(!OsLog.matches("CEF:0|Vendor|Product|1.0|100|Name|3|"));
        assert!(!OsLog.matches("<134>1 2026-08-15T19:40:20.123Z host app pid msgid - hi"));
        assert!(!OsLog.matches("D/NetworkClient: Sending GET request"));
        // Simplified console shape is a different format.
        assert!(!OsLog.matches("[UI:Navigation] INFO: Transitioning from HomeView"));
    }

    #[test]
    fn normalizes_default_columns_and_masks_msg() {
        let out = normalize(
            "2026-08-15 19:40:30.123456+0300 0x1a2b3c Default 0x0 12345 2 com.app: Transitioning to SettingsView for user_id=42",
        );
        assert!(out.starts_with("OSLOG Default"), "got: {out}");
        assert!(out.contains("com.app:"), "got: {out}");
        assert!(out.contains("user_id=<NUM>"), "got: {out}");
        assert!(!out.contains("0x1a2b3c"), "got: {out}");
        assert!(!out.contains("12345"), "got: {out}");
    }

    #[test]
    fn normalizes_compact_columns() {
        let out = normalize(
            "2026-08-15 19:40:30.123456+0300 Df Default 0x0 12345 2 [Navigation] com.app: Transitioning",
        );
        assert!(out.starts_with("OSLOG Default"), "got: {out}");
        assert!(out.contains("[Navigation]"), "got: {out}");
        assert!(out.contains("com.app:"), "got: {out}");
        assert!(!out.contains("Df"), "got: {out}");
        assert!(!out.contains("0x0"), "got: {out}");
    }

    #[test]
    fn captures_error_level() {
        let out = normalize(
            "2026-08-15 19:40:30.123456+0300 0x1a2b3c Error 0x99 99 1 com.app: Permission denied for zone 'UserFiles'",
        );
        assert!(out.starts_with("OSLOG Error"), "got: {out}");
        assert!(out.contains("Permission denied"), "got: {out}");
    }
}
