//! Structured JSON log lines: `{"time": …, "lvl": …, "msg": …}`.

use std::borrow::Cow;
use std::ops::Range;

use serde_json::Value;

use crate::core::time::{parse_time_param, TimeFormat};

use super::{FormatContext, LogFormat, Normalized};

/// One JSON object per line (the common structured-logging production shape).
pub struct Json;

/// Field names that carry the event timestamp (checked in priority order).
const TIME_KEYS: &[&str] = &["time", "timestamp", "@timestamp", "ts", "datetime"];

/// Field names whose string value is treated as free-form message content
/// (masked + mined by Drain) rather than a fixed `<key=class>` metadata slot.
const MSG_KEYS: &[&str] = &["msg", "message", "log", "text", "event"];

impl LogFormat for Json {
    fn name(&self) -> &'static str {
        "json"
    }

    fn matches(&self, line: &str) -> bool {
        let t = line.trim_start();
        t.starts_with('{') && t.trim_end().ends_with('}')
    }

    fn time_formats(&self) -> &'static [&'static dyn TimeFormat] {
        // Timestamp is a field inside the object, not a positional prefix.
        &[]
    }

    fn normalize<'a>(
        &self,
        line: &'a str,
        _ts: Option<(i64, Range<usize>)>,
        ctx: &mut FormatContext<'_>,
    ) -> Normalized<'a> {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            // Not valid JSON → treat the whole line as free text.
            let masked = ctx.masker.mask_with_header(line, &[], ctx.mask_cache);
            return Normalized {
                ts: None,
                content: masked,
            };
        };
        let ts = extract_ts(&value, line);
        let content = build_content(&value, line, ctx);
        Normalized { ts, content }
    }
}

fn extract_ts(value: &Value, line: &str) -> Option<(i64, Range<usize>)> {
    let Value::Object(obj) = value else {
        return None;
    };
    for key in TIME_KEYS {
        if let Some(val) = obj.get(*key) {
            match val {
                Value::String(s) => {
                    if let Some(ms) = parse_time_param(s) {
                        let span = line.find(s).map(|i| i..i + s.len())?;
                        return Some((ms, span));
                    }
                }
                Value::Number(n) => {
                    if let Some(ms) = number_to_ms(n) {
                        let repr = n.to_string();
                        let span = line.find(&repr).map(|i| i..i + repr.len())?;
                        return Some((ms, span));
                    }
                }
                _ => continue,
            }
        }
    }
    None
}

fn number_to_ms(n: &serde_json::Number) -> Option<i64> {
    if let Some(i) = n.as_i64() {
        return if i >= 1_000_000_000_000 {
            Some(i)
        } else if i >= 1_000_000_000 {
            Some(i * 1000)
        } else {
            None
        };
    }
    if let Some(f) = n.as_f64() {
        return if f >= 1_000_000_000_000.0 {
            Some(f as i64)
        } else if f >= 1_000_000_000.0 {
            Some((f * 1000.0) as i64)
        } else {
            None
        };
    }
    None
}

fn build_content<'a>(value: &Value, line: &'a str, ctx: &mut FormatContext<'_>) -> Cow<'a, str> {
    let Value::Object(obj) = value else {
        return ctx.masker.mask_with_header(line, &[], ctx.mask_cache);
    };
    let mut keys: Vec<&String> = obj.keys().collect();
    keys.sort();
    let mut out = String::with_capacity(line.len());
    for key in keys {
        let val = &obj[key];
        if TIME_KEYS.contains(&key.as_str()) {
            continue;
        }
        if MSG_KEYS.contains(&key.as_str()) {
            if let Value::String(s) = val {
                let masked = ctx.masker.mask_with_header(s, &[], ctx.mask_cache);
                out.push_str(masked.as_ref());
                out.push(' ');
                continue;
            }
        }
        out.push_str(key);
        out.push('=');
        out.push_str(value_class(val));
        out.push(' ');
    }
    let trimmed = out.trim_end();
    if trimmed.is_empty() {
        Cow::Owned("<EMPTY>".to_string())
    } else {
        Cow::Owned(trimmed.to_string())
    }
}

fn value_class(val: &Value) -> &'static str {
    match val {
        Value::Null => "<NULL>",
        Value::Bool(_) => "<BOOL>",
        Value::Number(_) => "<NUM>",
        Value::Array(_) | Value::Object(_) => "<JSON>",
        Value::String(_) => "<STR>",
    }
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
        Json.normalize(line, None, &mut ctx).content.into_owned()
    }

    #[test]
    fn matches_positive() {
        assert!(Json.matches("{\"time\": \"2026-08-15T19:40:01Z\", \"lvl\": 30}"));
        assert!(Json.matches("  {  \"a\": 1 }  "));
    }

    #[test]
    fn matches_negative() {
        assert!(!Json.matches("CEF:0|Vendor|Product|1.0|100|Name|3|"));
        assert!(!Json.matches("<134>1 2026-08-15T19:40:20.123Z host app pid msgid - hi"));
        assert!(!Json.matches("D/NetworkClient: Sending GET request"));
        assert!(!Json.matches("[UI:Navigation] INFO: Transitioning"));
        assert!(!Json.matches("2026-07-19 10:15:30 INFO plain"));
    }

    #[test]
    fn extracts_time_field() {
        let v: Value = serde_json::from_str("{\"time\": \"2026-08-15T19:40:01Z\"}").unwrap();
        let line = "{\"time\": \"2026-08-15T19:40:01Z\"}";
        let (ms, span) = extract_ts(&v, line).unwrap();
        assert_eq!(&line[span], "2026-08-15T19:40:01Z");
        assert_eq!(crate::core::time::format_ms(ms), "2026-08-15 19:40:01.000");
    }

    #[test]
    fn extracts_epoch_number_field() {
        let v: Value = serde_json::from_str("{\"timestamp\": 1784158530123}").unwrap();
        let (ms, _) = extract_ts(&v, "{\"timestamp\": 1784158530123}").unwrap();
        assert_eq!(ms, 1784158530123);
    }

    #[test]
    fn builds_schema_content_with_masked_msg() {
        let out = normalize("{\"time\": \"2026-08-15T19:40:01Z\", \"lvl\": 30, \"msg\": \"Page load: /v1/user\", \"env\": \"prod\"}");
        // lvl=<NUM>, env=<STR>, msg masked ("/v1/user" → <PATH>), time dropped.
        assert!(out.contains("lvl=<NUM>"), "got: {out}");
        assert!(out.contains("env=<STR>"), "got: {out}");
        assert!(out.contains("Page load: <PATH>"), "got: {out}");
        assert!(!out.contains("time="), "got: {out}");
    }

    #[test]
    fn invalid_json_falls_back_to_masking() {
        // A `{...}` line that isn't valid JSON should not panic.
        let out = normalize("{ not json }");
        assert!(!out.is_empty());
    }
}
