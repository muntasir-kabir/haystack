//! Plain/unstructured fallback: timestamp strip + learned-header masking.

use std::borrow::Cow;
use std::ops::Range;

use crate::core::masking::classify_token_public;
use crate::core::time::{TimeFormat, TIME_FORMATS};

use super::{FormatContext, LogFormat, Normalized};

/// The always-matching fallback for unstructured text logs. Reuses the
/// generic pipeline: strip the timestamp span, then mask dynamic values with
/// the per-file learned header slots.
pub struct Plain;

impl LogFormat for Plain {
    fn name(&self) -> &'static str {
        "plain"
    }

    fn matches(&self, _line: &str) -> bool {
        true
    }

    fn time_formats(&self) -> &'static [&'static dyn TimeFormat] {
        TIME_FORMATS
    }

    fn uses_learned_header(&self) -> bool {
        true
    }

    fn normalize<'a>(
        &self,
        line: &'a str,
        ts: Option<(i64, Range<usize>)>,
        ctx: &mut FormatContext<'_>,
    ) -> Normalized<'a> {
        let content = match &ts {
            Some((_, span)) => {
                let mut owned = line.to_owned();
                owned.replace_range(span.clone(), "");
                let masked = ctx
                    .masker
                    .mask_with_header(&owned, ctx.header_slots, ctx.mask_cache);
                Cow::Owned(masked.into_owned())
            }
            None => {
                let masked = ctx
                    .masker
                    .mask_with_header(line, ctx.header_slots, ctx.mask_cache);
                match masked {
                    Cow::Borrowed(_) => Cow::Borrowed(line),
                    Cow::Owned(o) => Cow::Owned(o),
                }
            }
        };
        Normalized { ts, content }
    }
}

/// Normalize a header token for constant comparison: strip surrounding
/// brackets/quotes and trailing separators so `[INFO]` vs `INFO`, `Tag:` vs
/// `Tag`, and `"GET` vs `GET` don't fragment the constant vote.
fn normalize_header_token(tok: &str) -> &str {
    tok.trim_matches(|c| matches!(c, '[' | ']' | '(' | ')' | '<' | '>' | '"' | '\''))
        .trim_end_matches([':', ',', ';'])
}

/// Learn forced mask slots for the common line header from sampled lines
/// (already timestamp-stripped). A leading position becomes a forced slot
/// when ≥60% of sampled lines have a token there that classifies to the
/// SAME dynamic mask class (IP/NUM/HEX/...). The header extends while
/// positions are "structured" — either ≥80% identical tokens (constant) or
/// a consistent dynamic class — and stops at the first free-text position.
pub(crate) fn learn_header_slots(sampled: &[String]) -> Vec<Option<&'static str>> {
    const MAX_HEADER: usize = 8;
    if sampled.len() < 8 {
        return Vec::new();
    }
    let n = sampled.len();
    let tokenized: Vec<Vec<&str>> = sampled
        .iter()
        .map(|l| l.split_whitespace().collect())
        .collect();
    let mut slots: Vec<Option<&'static str>> = Vec::new();
    for pos in 0..MAX_HEADER {
        let mut exact: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        let mut class: std::collections::HashMap<&'static str, usize> =
            std::collections::HashMap::new();
        let mut present = 0usize;
        for toks in &tokenized {
            if let Some(tok) = toks.get(pos) {
                present += 1;
                *exact.entry(normalize_header_token(tok)).or_default() += 1;
                if let Some(c) = classify_token_public(tok) {
                    *class.entry(c).or_default() += 1;
                }
            }
        }
        // Most lines must even have a token at this position. (Tolerates a
        // few wrapped/continuation lines: 80% presence is enough.)
        if present * 10 < n * 8 {
            break;
        }
        let constant = exact.values().max().copied().unwrap_or(0) * 10 >= n * 8;
        let dynamic = class
            .iter()
            .max_by_key(|(_, c)| *c)
            .filter(|(_, c)| **c * 10 >= n * 6)
            .map(|(k, _)| *k);
        match (constant, dynamic) {
            // Consistently dynamic position → forced mask slot.
            (false, Some(mask)) => slots.push(Some(mask)),
            // Constant position (level, app name) → keep as-is.
            (true, _) => slots.push(None),
            (false, None) => {
                // Closed-set position: a handful of distinct values (log
                // levels, tags) covering nearly all lines. Not a mask slot,
                // but structured — the header continues past it.
                let mut counts: Vec<usize> = exact.values().copied().collect();
                counts.sort_unstable_by(|a, b| b.cmp(a));
                let top: usize = counts.iter().take(8).sum();
                if top * 100 >= present * 95 {
                    slots.push(None);
                } else {
                    break;
                }
            }
        }
    }
    // Only bother if we actually found dynamic slots to mask.
    if slots.iter().any(|s| s.is_some()) {
        slots
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_everything() {
        assert!(Plain.matches("any line at all"));
        assert!(Plain.matches("{\"json\": true}"));
        assert!(Plain.matches("CEF:0|Vendor|..."));
    }

    #[test]
    fn time_formats_are_all() {
        assert_eq!(Plain.time_formats().len(), TIME_FORMATS.len());
    }

    #[test]
    fn uses_learned_header() {
        assert!(Plain.uses_learned_header());
    }

    #[test]
    fn header_slots_learn_dynamic_position() {
        let lines: Vec<String> = (0..100).map(|i| format!("worker-{} INFO msg", i)).collect();
        let slots = learn_header_slots(&lines);
        assert_eq!(slots.first(), Some(&Some(crate::core::masking::MASK_NUM)));
    }

    #[test]
    fn header_slots_stop_at_free_text() {
        // Position 1 is a different alphabetic word on every line: free text,
        // not a constant and not a numeric class → the header stops there.
        let lines: Vec<String> = (0..100)
            .map(|i| {
                let mut n = i;
                let mut word = String::new();
                loop {
                    word.push((b'a' + (n % 26) as u8) as char);
                    n /= 26;
                    if n == 0 {
                        break;
                    }
                }
                format!("INFO {word} hello {i}")
            })
            .collect();
        let slots = learn_header_slots(&lines);
        assert!(
            slots.is_empty(),
            "header should stop at free text: {slots:?}"
        );
    }
}
