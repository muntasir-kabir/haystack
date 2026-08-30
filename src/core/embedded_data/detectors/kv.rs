//! Shared bounded scanner for loose `key=value` and `key: value` events.
//!
//! The syntax is intentionally smaller than a programming language. It finds
//! exact field groups inside prose, keeps quoted/container values intact, and
//! applies extra confidence checks to single colon fields because ordinary
//! prose uses colons frequently.

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::core::embedded_data::parse::{balanced_end, scalar};
use crate::core::embedded_data::DataNode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Separator {
    Equals,
    Colon,
}

impl Separator {
    fn byte(self) -> u8 {
        match self {
            Self::Equals => b'=',
            Self::Colon => b':',
        }
    }
}

#[derive(Debug)]
pub(super) struct Group {
    pub start: usize,
    pub end: usize,
    pub source_ranges: Vec<Range<usize>>,
    pub fields: Vec<(String, DataNode)>,
}

#[derive(Debug)]
struct Field {
    start: usize,
    end: usize,
    key: String,
    value: DataNode,
    raw_value: String,
}

/// Find bounded field groups anywhere in a physical scan window.
pub(super) fn groups(
    bytes: &[u8],
    separator: Separator,
    max_depth: usize,
    max_results: usize,
    cancel: &AtomicBool,
) -> Vec<Group> {
    let mut out = Vec::new();
    let mut cursor = 0;
    while cursor < bytes.len() && out.len() < max_results {
        if cursor % 4096 == 0 && cancel.load(Ordering::Relaxed) {
            break;
        }
        let Some(first) = parse_field_at(bytes, cursor, separator, max_depth) else {
            cursor += 1;
            continue;
        };
        let start = first.start;
        let mut end = first.end;
        let mut parsed = vec![first];

        loop {
            let Some(next_start) = next_field_start(bytes, end) else {
                break;
            };
            let Some(next) = parse_field_at(bytes, next_start, separator, max_depth) else {
                break;
            };
            end = next.end;
            parsed.push(next);
        }

        let accepted = match separator {
            Separator::Equals => parsed.len() >= 2 || equals_single_is_confident(&parsed[0]),
            Separator::Colon => parsed.len() >= 2 || colon_single_is_confident(bytes, &parsed[0]),
        };
        if accepted {
            let source_ranges = parsed.iter().map(|field| field.start..field.end).collect();
            out.push(Group {
                start,
                end,
                source_ranges,
                fields: parsed
                    .into_iter()
                    .map(|field| (field.key, field.value))
                    .collect(),
            });
            cursor = end.max(cursor + 1);
        } else {
            cursor = start + 1;
        }
    }
    out
}

fn parse_field_at(
    bytes: &[u8],
    start: usize,
    separator: Separator,
    max_depth: usize,
) -> Option<Field> {
    let first = *bytes.get(start)?;
    if !is_key_start(first)
        || start
            .checked_sub(1)
            .and_then(|index| bytes.get(index))
            .is_some_and(|byte| is_key_byte(*byte))
        || inside_url_token(bytes, start)
    {
        return None;
    }

    let mut index = start + 1;
    while index < bytes.len() && is_key_byte(bytes[index]) {
        index += 1;
    }
    let key_end = index;
    while index < bytes.len() && matches!(bytes[index], b' ' | b'\t') {
        index += 1;
    }
    if bytes.get(index).copied()? != separator.byte() {
        return None;
    }
    if separator == Separator::Equals
        && (bytes.get(index + 1) == Some(&b'=')
            || index
                .checked_sub(1)
                .and_then(|previous| bytes.get(previous))
                .is_some_and(|byte| matches!(byte, b'!' | b'<' | b'>' | b'=')))
    {
        return None;
    }
    if separator == Separator::Colon && matches!(bytes.get(index + 1), Some(b':' | b'/')) {
        return None;
    }
    index += 1;
    while index < bytes.len() && matches!(bytes[index], b' ' | b'\t') {
        index += 1;
    }
    let value_start = index;
    let value_end = value_end(bytes, value_start, max_depth)?;
    if value_start == value_end {
        return None;
    }

    let key = std::str::from_utf8(&bytes[start..key_end]).ok()?.to_owned();
    let raw_value = std::str::from_utf8(&bytes[value_start..value_end])
        .ok()?
        .to_owned();
    Some(Field {
        start,
        end: value_end,
        key,
        value: scalar(&raw_value),
        raw_value,
    })
}

fn value_end(bytes: &[u8], start: usize, max_depth: usize) -> Option<usize> {
    let first = *bytes.get(start)?;
    if matches!(first, b'\'' | b'"' | b'`') {
        let mut index = start + 1;
        let mut escaped = false;
        while index < bytes.len() {
            let byte = bytes[index];
            index += 1;
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == first {
                return Some(index);
            }
        }
        return None;
    }
    if matches!(first, b'{' | b'[' | b'(' | b'<') {
        return balanced_end(bytes, start, max_depth);
    }

    let mut index = start;
    while index < bytes.len() && !matches!(bytes[index], b' ' | b'\t' | b'\r' | b'\n' | b',' | b';')
    {
        index += 1;
    }
    Some(index)
}

/// Return the next adjacent field start. A newline only joins a group when the
/// following physical line is indented, preventing adjacent timestamped log
/// records from being joined into one synthetic event.
fn next_field_start(bytes: &[u8], mut index: usize) -> Option<usize> {
    let mut consumed = false;
    let mut saw_newline = false;
    let mut indent_after_newline = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b' ' | b'\t' => {
                consumed = true;
                if saw_newline {
                    indent_after_newline += 1;
                }
                index += 1;
            }
            b',' | b';' if !saw_newline => {
                consumed = true;
                index += 1;
            }
            b'\r' if bytes.get(index + 1) == Some(&b'\n') && !saw_newline => {
                consumed = true;
                saw_newline = true;
                index += 2;
            }
            b'\n' if !saw_newline => {
                consumed = true;
                saw_newline = true;
                index += 1;
            }
            _ => break,
        }
    }
    if !consumed || (saw_newline && indent_after_newline == 0) {
        None
    } else {
        Some(index)
    }
}

fn equals_single_is_confident(field: &Field) -> bool {
    let raw = field.raw_value.trim();
    raw == "<null>" || !raw.starts_with(['{', '[', '(', '<'])
}

fn colon_single_is_confident(bytes: &[u8], field: &Field) -> bool {
    if looks_source_location(&field.key, &field.raw_value) {
        return false;
    }
    let raw = field.raw_value.trim();
    let strongly_delimited = raw
        .as_bytes()
        .first()
        .is_some_and(|byte| matches!(byte, b'\'' | b'"' | b'`' | b'{' | b'[' | b'(' | b'<'))
        || raw.starts_with('/')
        || raw.contains("://");
    strongly_delimited
        || (line_contains_only(bytes, field.start, field.end) && !field.key.contains('.'))
        || (structured_key(&field.key) && structured_scalar(raw))
}

fn line_contains_only(bytes: &[u8], start: usize, end: usize) -> bool {
    let line_start = bytes[..start]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    let line_end = bytes[end..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(bytes.len(), |offset| end + offset);
    let leading = bytes[line_start..start]
        .iter()
        .all(|byte| matches!(byte, b' ' | b'\t' | b'\r'));
    let trailing = bytes[end..line_end]
        .iter()
        .all(|byte| matches!(byte, b' ' | b'\t' | b'\r'));
    leading && trailing
}

fn structured_key(key: &str) -> bool {
    key.bytes().any(|byte| matches!(byte, b'_' | b'.' | b'-'))
        || key
            .as_bytes()
            .windows(2)
            .any(|pair| pair[0].is_ascii_lowercase() && pair[1].is_ascii_uppercase())
}

fn structured_scalar(raw: &str) -> bool {
    raw.bytes().any(|byte| matches!(byte, b'_' | b'-'))
        || raw.bytes().all(|byte| byte.is_ascii_digit())
        || matches!(
            raw.to_ascii_lowercase().as_str(),
            "true" | "false" | "yes" | "no" | "null" | "nil" | "none"
        )
}

fn looks_source_location(key: &str, raw: &str) -> bool {
    const EXTENSIONS: &[&str] = &[
        ".swift", ".m", ".mm", ".java", ".kt", ".rs", ".py", ".js", ".ts", ".go", ".cs", ".c",
        ".h", ".cc", ".cpp",
    ];
    raw.bytes().all(|byte| byte.is_ascii_digit())
        && EXTENSIONS.iter().any(|extension| key.ends_with(extension))
}

fn inside_url_token(bytes: &[u8], start: usize) -> bool {
    let token_start = bytes[..start]
        .iter()
        .rposition(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | b',' | b';'))
        .map_or(0, |index| index + 1);
    bytes[token_start..start]
        .windows(3)
        .any(|window| window == b"://")
}

fn is_key_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_key_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(input: &str, separator: Separator) -> Vec<Group> {
        groups(input.as_bytes(), separator, 32, 64, &AtomicBool::new(false))
    }

    #[test]
    fn accepts_single_equals_and_spaces_around_separator() {
        let values = scan(
            "INFO error = 'HKErrorAuthorizationDenied'",
            Separator::Equals,
        );
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].fields.len(), 1);
        assert_eq!(values[0].start, 5);
    }

    #[test]
    fn leaves_single_enclosed_values_to_structural_detectors() {
        assert!(scan("response={ok: true}", Separator::Equals).is_empty());
        assert_eq!(scan("value=<null>", Separator::Equals).len(), 1);
    }

    #[test]
    fn keeps_all_quote_styles_as_one_value() {
        for input in ["message=\"data 1\"", "message='data 1'", "message=`data 1`"] {
            let values = scan(input, Separator::Equals);
            assert_eq!(values.len(), 1, "{input}");
            assert!(matches!(
                &values[0].fields[0].1,
                DataNode::String(value) if value == "data 1"
            ));
        }
    }

    #[test]
    fn keeps_url_and_path_values_intact() {
        let values = scan(
            "path=/var/data/somepath url=myapp://billing/9236 route_known=YES",
            Separator::Equals,
        );
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].fields.len(), 3);
        assert!(matches!(
            &values[0].fields[1].1,
            DataNode::String(value) if value == "myapp://billing/9236"
        ));
    }

    #[test]
    fn does_not_extract_query_parameters_from_standalone_url() {
        assert!(scan("GET https://example.test/path?x=1&y=2", Separator::Equals).is_empty());
    }

    #[test]
    fn supports_multiline_quoted_and_balanced_values() {
        for input in [
            "message=\"first\nsecond\" status=ok",
            "payload={\n  \"ok\": true\n} status=ok",
        ] {
            let values = scan(input, Separator::Equals);
            assert_eq!(values.len(), 1, "{input}");
            assert_eq!(values[0].fields.len(), 2, "{input}");
            assert!(values[0].end > input.find('\n').unwrap());
        }
    }

    #[test]
    fn accepts_structured_single_colon_inside_prose() {
        let values = scan(
            "Memory warning received pressureLevel: deep_link",
            Separator::Colon,
        );
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].fields[0].0, "pressureLevel");
    }

    #[test]
    fn rejects_colon_false_positives() {
        for input in [
            "2026-07-15 22:02:08 INFO complete",
            "GET https://example.test/path",
            "ViewController.swift:92 restored",
            "request complete: this is ordinary prose",
            "com.example.app: Transitioning",
        ] {
            assert!(scan(input, Separator::Colon).is_empty(), "{input}");
        }
    }

    #[test]
    fn does_not_join_unindented_physical_records() {
        let values = scan("first=1\nsecond=2", Separator::Equals);
        assert_eq!(values.len(), 2);
        assert!(values.iter().all(|group| group.fields.len() == 1));
    }
}
