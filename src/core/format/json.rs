//! Structured JSON log lines: `{"time": …, "lvl": …, "msg": …}`.

use std::borrow::Cow;
use std::ops::Range;

use serde_json::Value;

use crate::core::record::{HeaderTime, RecordClassification};
use crate::core::time::{parse_time_param, TimeFormat, TimeFormatKind};

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
        matches!(serde_json::from_str::<Value>(line), Ok(Value::Object(_)))
    }

    fn time_formats(&self) -> &'static [&'static dyn TimeFormat] {
        // Timestamp is a field inside the object, not a positional prefix.
        &[]
    }

    fn classify_header(
        &self,
        line: &str,
        _time_format: Option<&TimeFormatKind>,
    ) -> Option<RecordClassification> {
        classify_header_with_time_key(line, None)
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

pub(crate) fn detect_time_key<'a>(lines: impl Iterator<Item = &'a str>) -> Option<String> {
    let objects = lines
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|value| match value {
            Value::Object(object) => Some(object),
            _ => None,
        })
        .collect::<Vec<_>>();
    TIME_KEYS
        .iter()
        .find(|key| objects.iter().any(|object| object.contains_key(**key)))
        .map(|key| (*key).to_string())
}

pub(crate) fn classify_header_with_time_key(
    line: &str,
    selected_key: Option<&str>,
) -> Option<RecordClassification> {
    let value = serde_json::from_str::<Value>(line).ok()?;
    classify_value_with_time_key(&value, line, selected_key)
}

pub(crate) fn classify_value_with_time_key(
    value: &Value,
    line: &str,
    selected_key: Option<&str>,
) -> Option<RecordClassification> {
    let Value::Object(object) = value else {
        return None;
    };
    let selected = selected_key.or_else(|| {
        TIME_KEYS
            .iter()
            .find(|key| object.contains_key(**key))
            .copied()
    });
    let time = selected
        .and_then(|key| object.get(key).map(|value| (key, value)))
        .map_or(HeaderTime::Missing, |(key, value)| {
            let span = json_field_value_span(line, key);
            let parsed = match value {
                Value::String(value) => parse_time_param(value),
                Value::Number(value) => number_to_ms(value),
                _ => None,
            };
            match (parsed, span) {
                (Some(value), Some(span)) => HeaderTime::Known { value, span },
                (_, span) => HeaderTime::Invalid { span },
            }
        });
    Some(RecordClassification::Header {
        time,
        header_span: 0..0,
        message_span: 0..line.len(),
    })
}

pub(crate) fn normalize_value<'a>(
    value: &Value,
    line: &'a str,
    ctx: &mut FormatContext<'_>,
) -> Normalized<'a> {
    Normalized {
        ts: extract_ts(value, line),
        content: build_content(value, line, ctx),
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
                        let span = json_field_value_span(line, key)?;
                        return Some((ms, span));
                    }
                }
                Value::Number(n) => {
                    if let Some(ms) = number_to_ms(n) {
                        let span = json_field_value_span(line, key)?;
                        return Some((ms, span));
                    }
                }
                _ => continue,
            }
        }
    }
    None
}

/// Locate a top-level JSON field value without confusing a repeated timestamp
/// string in another field for the timestamp source. String spans exclude the
/// surrounding quotes; numeric spans cover the complete literal.
fn json_field_value_span(line: &str, wanted: &str) -> Option<Range<usize>> {
    let bytes = line.as_bytes();
    let mut cursor = skip_json_ws(bytes, 0);
    if bytes.get(cursor) != Some(&b'{') {
        return None;
    }
    cursor += 1;
    loop {
        cursor = skip_json_ws(bytes, cursor);
        if bytes.get(cursor) == Some(&b'}') {
            return None;
        }
        if bytes.get(cursor) == Some(&b',') {
            cursor = skip_json_ws(bytes, cursor + 1);
        }
        let key_start = cursor;
        let key_end = json_string_end(bytes, key_start)?;
        let key = serde_json::from_str::<String>(&line[key_start..key_end]).ok()?;
        cursor = skip_json_ws(bytes, key_end);
        if bytes.get(cursor) != Some(&b':') {
            return None;
        }
        let value_start = skip_json_ws(bytes, cursor + 1);
        let value_end = json_value_end(bytes, value_start)?;
        if key == wanted {
            return if bytes.get(value_start) == Some(&b'"') {
                Some(value_start + 1..value_end.saturating_sub(1))
            } else {
                Some(value_start..value_end)
            };
        }
        cursor = value_end;
    }
}

fn skip_json_ws(bytes: &[u8], mut cursor: usize) -> usize {
    while bytes
        .get(cursor)
        .is_some_and(|byte| byte.is_ascii_whitespace())
    {
        cursor += 1;
    }
    cursor
}

fn json_string_end(bytes: &[u8], start: usize) -> Option<usize> {
    if bytes.get(start) != Some(&b'"') {
        return None;
    }
    let mut cursor = start + 1;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => cursor = cursor.checked_add(2)?,
            b'"' => return Some(cursor + 1),
            _ => cursor += 1,
        }
    }
    None
}

fn json_value_end(bytes: &[u8], start: usize) -> Option<usize> {
    match bytes.get(start)? {
        b'"' => json_string_end(bytes, start),
        b'{' | b'[' => {
            let mut stack = vec![bytes[start]];
            let mut cursor = start + 1;
            while cursor < bytes.len() {
                match bytes[cursor] {
                    b'"' => cursor = json_string_end(bytes, cursor)?,
                    b'{' | b'[' => {
                        stack.push(bytes[cursor]);
                        cursor += 1;
                    }
                    b'}' => {
                        if stack.pop()? != b'{' {
                            return None;
                        }
                        cursor += 1;
                        if stack.is_empty() {
                            return Some(cursor);
                        }
                    }
                    b']' => {
                        if stack.pop()? != b'[' {
                            return None;
                        }
                        cursor += 1;
                        if stack.is_empty() {
                            return Some(cursor);
                        }
                    }
                    _ => cursor += 1,
                }
            }
            None
        }
        _ => {
            let mut cursor = start;
            while bytes
                .get(cursor)
                .is_some_and(|byte| !byte.is_ascii_whitespace() && !matches!(*byte, b',' | b'}'))
            {
                cursor += 1;
            }
            (cursor > start).then_some(cursor)
        }
    }
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
    fn timestamp_span_uses_the_timestamp_field_not_a_repeated_value() {
        let line = r#"{"msg":"2026-08-15T19:40:01Z", "time":"2026-08-15T19:40:01Z"}"#;
        let value: Value = serde_json::from_str(line).unwrap();
        let (_, span) = extract_ts(&value, line).unwrap();
        assert_eq!(&line[span.clone()], "2026-08-15T19:40:01Z");
        assert_eq!(span.start, line.rfind("2026-08-15T19:40:01Z").unwrap());
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
