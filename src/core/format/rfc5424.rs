//! RFC 5424 syslog: `<PRI>VERSION TIMESTAMP HOST APP PROCID MSGID [SD] MSG`.

use std::borrow::Cow;
use std::ops::Range;

use crate::core::time::{Iso, TimeFormat};

use super::{FormatContext, LogFormat, Normalized};

/// RFC 5424 structured syslog lines (facility/severity PRI + structured header).
pub struct Rfc5424;

/// RFC 5424 uses an ISO-8601 timestamp after the PRI/version.
static RFC5424_TIMES: &[&'static dyn TimeFormat] = &[&Iso];

impl LogFormat for Rfc5424 {
    fn name(&self) -> &'static str {
        "rfc5424"
    }

    fn matches(&self, line: &str) -> bool {
        let b = line.trim_start().as_bytes();
        if b.first() != Some(&b'<') {
            return false;
        }
        let mut i = 1;
        let mut digits = 0;
        while i < b.len() && b[i].is_ascii_digit() {
            digits += 1;
            i += 1;
        }
        if !(1..=3).contains(&digits) {
            return false;
        }
        if b.get(i) != Some(&b'>') {
            return false;
        }
        // After ">" comes the VERSION digit (e.g. "1").
        matches!(b.get(i + 1), Some(c) if c.is_ascii_digit())
    }

    fn time_formats(&self) -> &'static [&'static dyn TimeFormat] {
        RFC5424_TIMES
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

/// Build Drain content: keep APP + MSGID (structural constants), mask the
/// dynamic HOST/PROCID, and mask the free-form MSG.
fn build_content<'a>(line: &'a str, ctx: &mut FormatContext<'_>) -> Cow<'a, str> {
    let Some(gt) = line.find('>') else {
        return ctx.masker.mask_with_header(line, &[], ctx.mask_cache);
    };
    let after = line[gt + 1..].trim_start();
    // "VERSION TIMESTAMP HOST APP PROCID MSGID [SD] MSG"
    let Some(space) = after.find(' ') else {
        return ctx.masker.mask_with_header(line, &[], ctx.mask_cache);
    };
    let after_ver = after[space + 1..].trim_start();
    let mut words = after_ver.split_whitespace();
    let _ts = words.next();
    let _host = words.next();
    let app = words.next().unwrap_or("-");
    let procid = words.next().unwrap_or("-");
    let msgid = words.next().unwrap_or("-");
    let rest: Vec<&str> = words.collect();
    // Strip structured-data (`[...]`, may span multiple whitespace words) or a
    // lone "-" placeholder before the message.
    let msg = if rest.first().is_some_and(|w| *w == "-") {
        rest[1..].join(" ")
    } else if rest.first().is_some_and(|w| w.starts_with('[')) {
        let mut end = 0;
        for (i, w) in rest.iter().enumerate() {
            end = i;
            if w.contains(']') {
                break;
            }
        }
        rest[(end + 1).min(rest.len())..].join(" ")
    } else {
        rest.join(" ")
    };
    let msg_masked = ctx.masker.mask_with_header(&msg, &[], ctx.mask_cache);
    let procid_masked = ctx.masker.mask_with_header(procid, &[], ctx.mask_cache);
    let msgid_masked = ctx.masker.mask_with_header(msgid, &[], ctx.mask_cache);
    Cow::Owned(format!(
        "RFC5424 <HOST> {app} {procid_masked} {msgid_masked} {msg_masked}"
    ))
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
        Rfc5424.normalize(line, None, &mut ctx).content.into_owned()
    }

    #[test]
    fn matches_positive() {
        assert!(Rfc5424
            .matches("<134>1 2026-08-15T19:40:20.123Z srv-alpha auth-api 1201 tx_882 - Login"));
    }

    #[test]
    fn matches_negative() {
        assert!(!Rfc5424.matches("{\"time\": \"2026-08-15T19:40:01Z\"}"));
        assert!(!Rfc5424.matches("CEF:0|Vendor|Product|1.0|100|Name|3|"));
        assert!(!Rfc5424.matches("D/NetworkClient: Sending GET request"));
        assert!(!Rfc5424.matches("2026-07-19 10:15:30 INFO plain"));
        // "<" but not a valid PRI prefix.
        assert!(!Rfc5424.matches("<not-a-pri> message"));
    }

    #[test]
    fn normalizes_header_and_masks_msg() {
        let out = normalize(
            "<134>1 2026-08-15T19:40:20.123Z srv-alpha auth-api 1201 tx_882 - Login successful",
        );
        assert!(
            out.starts_with("RFC5424 <HOST> auth-api <NUM> tx_<NUM>"),
            "got: {out}"
        );
        assert!(out.contains("Login successful"), "got: {out}");
    }

    #[test]
    fn strips_structured_data() {
        let out = normalize("<131>1 2026-08-15T19:40:22.456Z srv-alpha db-proxy 1205 tx_883 [meta dbsize=\"GB\"] Query slow: SELECT * FROM users");
        assert!(out.contains("db-proxy"), "got: {out}");
        assert!(out.contains("Query slow"), "got: {out}");
        assert!(!out.contains("[meta"), "got: {out}");
    }
}
