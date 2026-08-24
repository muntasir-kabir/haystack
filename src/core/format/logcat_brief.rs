//! Android logcat *brief* format: `D/Tag: message` (tag-based, timeless).

use std::borrow::Cow;
use std::ops::Range;

use crate::core::time::TimeFormat;

use super::{FormatContext, LogFormat, Normalized};

/// Logcat brief lines: a priority letter, `/`, a tag, then the message.
pub struct LogcatBrief;

impl LogFormat for LogcatBrief {
    fn name(&self) -> &'static str {
        "logcat_brief"
    }

    fn matches(&self, line: &str) -> bool {
        let b = line.trim_start().as_bytes();
        if b.len() < 3 {
            return false;
        }
        matches!(b[0], b'V' | b'D' | b'I' | b'W' | b'E' | b'F') && b[1] == b'/'
    }

    fn time_formats(&self) -> &'static [&'static dyn TimeFormat] {
        // Brief format carries no timestamp (threadtime is a different format).
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
    let priority = &t[0..1];
    let rest = &t[1..];
    let Some(rest) = rest.strip_prefix('/') else {
        return ctx.masker.mask_with_header(line, &[], ctx.mask_cache);
    };
    // Tag runs up to ':', '(' or whitespace.
    let tag_end = rest
        .find(|c: char| c == ':' || c == '(' || c.is_whitespace())
        .unwrap_or(rest.len());
    let tag = &rest[..tag_end];
    // Strip `:` / `(pid):` before the message.
    let msg = rest[tag_end..].trim_start_matches(|c: char| {
        c == ':' || c == ' ' || c == '(' || c == ')' || c.is_ascii_digit()
    });
    let masked = ctx.masker.mask_with_header(msg, &[], ctx.mask_cache);
    Cow::Owned(format!("{priority} {tag} {masked}"))
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
        LogcatBrief
            .normalize(line, None, &mut ctx)
            .content
            .into_owned()
    }

    #[test]
    fn matches_positive() {
        assert!(LogcatBrief.matches("D/NetworkClient: Sending GET request"));
        assert!(LogcatBrief.matches("I/UserProfile: Successfully cached profile"));
        assert!(LogcatBrief.matches("E/LocationSvc: GMS connection suspended"));
    }

    #[test]
    fn matches_negative() {
        assert!(!LogcatBrief.matches("{\"time\": \"2026-08-15T19:40:01Z\"}"));
        assert!(!LogcatBrief.matches("CEF:0|Vendor|Product|1.0|100|Name|3|"));
        assert!(!LogcatBrief.matches("<134>1 2026-08-15T19:40:20.123Z host app pid msgid - hi"));
        assert!(!LogcatBrief.matches("2026-07-19 10:15:30 INFO plain"));
        assert!(!LogcatBrief.matches("07-15 22:00:01.123  1234  5678 I Tag: hello"));
    }

    #[test]
    fn normalizes_tag_and_masks_msg() {
        let out = normalize("I/UserProfile: Successfully cached profile image for user_id: 992");
        assert!(out.starts_with("I UserProfile "), "got: {out}");
        assert!(out.contains("user_id: <NUM>"), "got: {out}");
    }

    #[test]
    fn strips_pid_from_tag() {
        let out = normalize("I/ActivityManager(1234): Start proc");
        assert!(out.starts_with("I ActivityManager "), "got: {out}");
        assert!(!out.contains("1234"), "got: {out}");
    }
}
