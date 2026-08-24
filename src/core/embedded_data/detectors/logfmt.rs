use super::{detection, line_ranges};
use crate::core::embedded_data::parse::scalar;
use crate::core::embedded_data::{AnalysisLimits, DataDetector, DataNode, Detection, ScanWindow};
use std::sync::atomic::{AtomicBool, Ordering};

pub const ID: &str = "logfmt";
pub(super) struct LogfmtDetector;

impl DataDetector for LogfmtDetector {
    fn id(&self) -> &'static str {
        ID
    }
    fn detect(
        &self,
        window: &ScanWindow,
        limits: &AnalysisLimits,
        cancel: &AtomicBool,
    ) -> Vec<Detection> {
        let mut out = Vec::new();
        for (line_start, line_end) in line_ranges(&window.bytes) {
            if cancel.load(Ordering::Relaxed) || out.len() >= limits.max_results {
                break;
            }
            let Ok(line) = std::str::from_utf8(&window.bytes[line_start..line_end]) else {
                continue;
            };
            let Some((start, end, fields)) = parse_line(line) else {
                continue;
            };
            if fields.len() < 2 {
                continue;
            }
            if let Some(value) = detection(
                window,
                self.id(),
                line_start + start,
                line_start + end,
                DataNode::Object(fields),
            ) {
                out.push(value);
            }
        }
        out
    }
}

fn parse_line(line: &str) -> Option<(usize, usize, Vec<(String, DataNode)>)> {
    let bytes = line.as_bytes();
    let mut index = 0;
    let mut first = None;
    let mut last = 0;
    let mut fields = Vec::new();
    while index < bytes.len() {
        while index < bytes.len() && matches!(bytes[index], b' ' | b'\t' | b',' | b';') {
            index += 1;
        }
        let key_start = index;
        while index < bytes.len()
            && (bytes[index].is_ascii_alphanumeric() || matches!(bytes[index], b'_' | b'.' | b'-'))
        {
            index += 1;
        }
        if index == key_start || index >= bytes.len() || bytes[index] != b'=' {
            index = key_start.saturating_add(1);
            continue;
        }
        let key = &line[key_start..index];
        index += 1;
        let value_start = index;
        if index < bytes.len() && matches!(bytes[index], b'\'' | b'\"') {
            let quote = bytes[index];
            index += 1;
            let mut escaped = false;
            while index < bytes.len() {
                let byte = bytes[index];
                index += 1;
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == quote {
                    break;
                }
            }
            if index == bytes.len() && bytes[index - 1] != quote {
                return None;
            }
        } else {
            while index < bytes.len() && !matches!(bytes[index], b' ' | b'\t' | b',' | b';') {
                index += 1;
            }
        }
        let value = &line[value_start..index];
        first.get_or_insert(key_start);
        last = index;
        fields.push((key.to_owned(), scalar(value)));
    }
    Some((first?, last, fields))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_quoted_logfmt_pairs() {
        let (_, _, values) = parse_line("INFO user=42 status=\"ok value\" duration_ms=18").unwrap();
        assert_eq!(values.len(), 3);
        assert!(matches!(values[1].1, DataNode::String(_)));
    }
}
