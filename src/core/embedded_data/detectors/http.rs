use super::{detection, line_ranges};
use crate::core::embedded_data::parse::scalar;
use crate::core::embedded_data::{AnalysisLimits, DataDetector, DataNode, Detection, ScanWindow};
use std::sync::atomic::{AtomicBool, Ordering};

pub const ID: &str = "http";
pub(super) struct HttpDetector;
impl DataDetector for HttpDetector {
    fn id(&self) -> &'static str {
        ID
    }
    fn detect(
        &self,
        window: &ScanWindow,
        limits: &AnalysisLimits,
        cancel: &AtomicBool,
    ) -> Vec<Detection> {
        let ranges = line_ranges(&window.bytes).collect::<Vec<_>>();
        let mut out = Vec::new();
        for (index, &(start, end)) in ranges.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let Ok(first) = std::str::from_utf8(&window.bytes[start..end]) else {
                continue;
            };
            if !is_start_line(first) {
                continue;
            }
            let mut headers = Vec::new();
            let mut cursor = index + 1;
            let mut last = end;
            while let Some(&(header_start, header_end)) = ranges.get(cursor) {
                let Ok(line) = std::str::from_utf8(&window.bytes[header_start..header_end]) else {
                    break;
                };
                if line.trim().is_empty() {
                    last = header_end;
                    cursor += 1;
                    break;
                }
                let Some((name, value)) = line.split_once(':') else {
                    break;
                };
                if !valid_header_name(name) {
                    break;
                }
                headers.push((name.trim().to_owned(), scalar(value.trim())));
                last = header_end;
                cursor += 1;
            }
            if headers.len() < 2 {
                continue;
            }
            let mut root = vec![
                ("start_line".to_owned(), DataNode::String(first.to_owned())),
                ("headers".to_owned(), DataNode::Object(headers)),
            ];
            if let Some(query) = first
                .split_once('?')
                .and_then(|(_, rest)| rest.split_whitespace().next())
            {
                root.push(("query".to_owned(), form(query)));
            }
            if let Some((body_start, body_end)) = ranges.get(cursor).copied() {
                if let Ok(body) = std::str::from_utf8(&window.bytes[body_start..body_end]) {
                    if !body.trim().is_empty() {
                        root.push(("body".to_owned(), form(body)));
                        last = body_end;
                    }
                }
            }
            if let Some(value) = detection(window, self.id(), start, last, DataNode::Object(root)) {
                out.push(value);
            }
            if out.len() >= limits.max_results {
                break;
            }
        }
        out
    }
}

fn is_start_line(line: &str) -> bool {
    line.starts_with("HTTP/")
        || matches!(
            line.split_whitespace().next(),
            Some("GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS")
        )
}
fn valid_header_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}
fn form(text: &str) -> DataNode {
    let values = text
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            (decode(key), DataNode::String(decode(value)))
        })
        .collect::<Vec<_>>();
    if values.is_empty() {
        DataNode::String(text.to_owned())
    } else {
        DataNode::Object(values)
    }
}
fn decode(text: &str) -> String {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                if let (Some(a), Some(b)) = (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                    out.push(a * 16 + b);
                    index += 3;
                } else {
                    out.push(bytes[index]);
                    index += 1;
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn form_decodes_percent_and_plus() {
        assert_eq!(decode("Ada+%F0%9F%9A%80"), "Ada 🚀");
    }
}
