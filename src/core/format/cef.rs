//! Common Event Format (CEF): `CEF:0|Vendor|Product|Version|SigId|Name|Sev|ext`.
//! Pipe-delimited security events; timeless (no timestamp field).

use std::borrow::Cow;
use std::ops::Range;

use crate::core::time::TimeFormat;

use super::{FormatContext, LogFormat, Normalized};

/// CEF pipe-delimited events (ArcSight Common Event Format).
pub struct Cef;

impl LogFormat for Cef {
    fn name(&self) -> &'static str {
        "cef"
    }

    fn matches(&self, line: &str) -> bool {
        line.trim_start().starts_with("CEF:")
    }

    fn time_formats(&self) -> &'static [&'static dyn TimeFormat] {
        // CEF carries no timestamp; the timeline falls back to line numbers.
        &[]
    }

    fn normalize<'a>(
        &self,
        line: &'a str,
        _ts: Option<(i64, Range<usize>)>,
        ctx: &mut FormatContext<'_>,
    ) -> Normalized<'a> {
        let content = build_content(line, ctx);
        Normalized { ts: None, content }
    }
}

fn build_content<'a>(line: &'a str, ctx: &mut FormatContext<'_>) -> Cow<'a, str> {
    let parts: Vec<&str> = line.split('|').collect();
    let field = |i: usize| parts.get(i).map(|p| p.trim()).unwrap_or("-");
    // parts[0] = "CEF:Version", then Vendor, Product, Version, Signature, Name, Severity.
    let vendor = field(1);
    let product = field(2);
    let signature = field(4);
    let name = field(5);
    let severity = field(6);
    // Extension is `key=value` pairs; mask the dynamic values.
    let ext = field(7);
    let masked_ext = ctx.masker.mask_with_header(ext, &[], ctx.mask_cache);
    Cow::Owned(format!(
        "CEF {vendor} {product} {signature} {name} {severity} {masked_ext}"
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
        Cef.normalize(line, None, &mut ctx).content.into_owned()
    }

    #[test]
    fn matches_positive() {
        assert!(Cef.matches("CEF:0|VendorX|AppY|1.0|100|Login Success|3|suser=mkabir spt=443"));
    }

    #[test]
    fn matches_negative() {
        assert!(!Cef.matches("{\"time\": \"2026-08-15T19:40:01Z\"}"));
        assert!(!Cef.matches("<134>1 2026-08-15T19:40:20.123Z host app pid msgid - hi"));
        assert!(!Cef.matches("D/NetworkClient: Sending GET request"));
        assert!(!Cef.matches("2026-07-19 10:15:30 INFO plain"));
    }

    #[test]
    fn builds_header_and_masks_extension() {
        let out = normalize("CEF:0|VendorX|AppY|1.0|100|Login Success|3|suser=mkabir spt=443");
        assert!(
            out.starts_with("CEF VendorX AppY 100 Login Success 3"),
            "got: {out}"
        );
        // spt=443 masked to spt=<NUM>; username (no digits) stays literal.
        assert!(out.contains("spt=<NUM>"), "got: {out}");
        assert!(out.contains("suser=mkabir"), "got: {out}");
    }

    #[test]
    fn handles_missing_extension() {
        let out = normalize("CEF:0|VendorX|AppY|1.0|100|Login Success|3");
        assert!(
            out.starts_with("CEF VendorX AppY 100 Login Success 3"),
            "got: {out}"
        );
    }
}
