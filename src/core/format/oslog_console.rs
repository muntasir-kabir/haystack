//! iOS OSLog console lines: `[Subsystem:Category] LEVEL: message`.
//! (The *simplified* console shape; the real Unified Logging System `log show`
//! columnar format lives in `os_log.rs`.)

use std::borrow::Cow;
use std::ops::Range;

use crate::core::time::TimeFormat;

use super::{FormatContext, LogFormat, Normalized};

/// `[Subsystem:Category] LEVEL:` lines (the simplified OSLog console shape).
pub struct OsLogConsole;

/// Known OSLog / unified-log level words (case-preserving).
fn is_level_word(w: &str) -> bool {
    matches!(
        w,
        "INFO"
            | "DEBUG"
            | "ERROR"
            | "WARNING"
            | "WARN"
            | "NOTICE"
            | "FAULT"
            | "CRITICAL"
            | "TRACE"
            | "DEFAULT"
            | "Info"
            | "Debug"
            | "Error"
            | "Fault"
            | "Default"
            | "Notice"
            | "Trace"
            | "Warning"
    )
}

impl LogFormat for OsLogConsole {
    fn name(&self) -> &'static str {
        "oslog_console"
    }

    fn matches(&self, line: &str) -> bool {
        let t = line.trim_start();
        if !t.starts_with('[') {
            return false;
        }
        let Some(close) = t.find(']') else {
            return false;
        };
        // Inside must be "Subsystem:Category" (a colon separates them).
        if !t[1..close].contains(':') {
            return false;
        }
        let rest = t[close + 1..].trim_start();
        let end = rest
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(rest.len());
        is_level_word(&rest[..end])
    }

    fn time_formats(&self) -> &'static [&'static dyn TimeFormat] {
        // This console shape carries no timestamp (the full OSLog form does).
        &[]
    }

    fn normalize<'a>(
        &self,
        line: &'a str,
        _ts: Option<(i64, Range<usize>)>,
        ctx: &mut FormatContext<'_>,
    ) -> Normalized<'a> {
        Normalized {
            ts: None,
            content: build_content(line, ctx),
        }
    }
}

fn build_content<'a>(line: &'a str, ctx: &mut FormatContext<'_>) -> Cow<'a, str> {
    let t = line.trim_start();
    let Some(close) = t.find(']') else {
        return ctx.masker.mask_with_header(line, &[], ctx.mask_cache);
    };
    let subcat = &t[1..close];
    let rest = t[close + 1..].trim_start();
    let level_end = rest
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(rest.len());
    let level = &rest[..level_end];
    let msg = rest[level_end..].trim_start_matches(|c: char| c == ':' || c.is_whitespace());
    let masked = ctx.masker.mask_with_header(msg, &[], ctx.mask_cache);
    Cow::Owned(format!("{subcat} {level} {masked}"))
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
        OsLogConsole
            .normalize(line, None, &mut ctx)
            .content
            .into_owned()
    }

    #[test]
    fn matches_positive() {
        assert!(OsLogConsole
            .matches("[UI:Navigation] INFO: Transitioning from HomeView to SettingsView"));
        assert!(OsLogConsole
            .matches("[Storage:CoreData] DEBUG: Saved 12 records in background context"));
        assert!(OsLogConsole.matches("[Sync:CloudKit] ERROR: Permission denied for record zone"));
    }

    #[test]
    fn matches_negative() {
        assert!(!OsLogConsole.matches("{\"time\": \"2026-08-15T19:40:01Z\"}"));
        assert!(!OsLogConsole.matches("CEF:0|Vendor|Product|1.0|100|Name|3|"));
        assert!(!OsLogConsole.matches("<134>1 2026-08-15T19:40:20.123Z host app pid msgid - hi"));
        assert!(!OsLogConsole.matches("D/NetworkClient: Sending GET request"));
        assert!(!OsLogConsole.matches("2026-07-19 10:15:30 INFO plain"));
        // JSON array has no colon before the first ']'.
        assert!(!OsLogConsole.matches("[1,2,3]"));
    }

    #[test]
    fn normalizes_subcategory_and_masks_msg() {
        let out = normalize("[Storage:CoreData] DEBUG: Saved 12 records in background context");
        assert!(out.starts_with("Storage:CoreData DEBUG "), "got: {out}");
        assert!(out.contains("Saved <NUM> records"), "got: {out}");
    }
}
