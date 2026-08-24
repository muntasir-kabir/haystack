use super::{detection, line_ranges};
use crate::core::embedded_data::{AnalysisLimits, DataDetector, DataNode, Detection, ScanWindow};
use std::sync::atomic::{AtomicBool, Ordering};

pub const ID: &str = "encoded";
pub const JWT_ID: &str = "jwt";
pub const BASE64_ID: &str = "base64";
pub const HEX_ID: &str = "hex";
pub const PEM_ID: &str = "pem";
pub(super) struct EncodedDetector;
impl DataDetector for EncodedDetector {
    fn id(&self) -> &'static str {
        ID
    }
    fn detect(
        &self,
        window: &ScanWindow,
        limits: &AnalysisLimits,
        cancel: &AtomicBool,
    ) -> Vec<Detection> {
        let bytes = &window.bytes;
        let mut out = Vec::new();
        // PEM owns all following base64 lines and is unambiguous.
        for start in 0..bytes.len() {
            if start % 4096 == 0 && cancel.load(Ordering::Relaxed) {
                return out;
            }
            if !bytes[start..].starts_with(b"-----BEGIN ") {
                continue;
            }
            let header_start = start + b"-----BEGIN ".len();
            let Some(label_end) = bytes[header_start..]
                .windows(5)
                .position(|value| value == b"-----")
                .map(|offset| header_start + offset + 5)
            else {
                continue;
            };
            let Ok(header) = std::str::from_utf8(&bytes[start..label_end]) else {
                continue;
            };
            let label = header
                .trim_start_matches("-----BEGIN ")
                .trim_end_matches("-----");
            let footer = format!("-----END {label}-----");
            let Some(offset) = bytes[label_end..]
                .windows(footer.len())
                .position(|value| value == footer.as_bytes())
            else {
                continue;
            };
            let end = label_end + offset + footer.len();
            let data = metadata("PEM", label, end - start);
            if let Some(value) = detection(window, PEM_ID, start, end, data) {
                out.push(value);
            }
        }
        for (line_start, line_end) in line_ranges(bytes) {
            if out.len() >= limits.max_results {
                break;
            }
            let Ok(line) = std::str::from_utf8(&bytes[line_start..line_end]) else {
                continue;
            };
            for token in line
                .split_whitespace()
                .flat_map(|piece| piece.split([',', ';']))
            {
                let token = token
                    .trim_matches(|ch: char| matches!(ch, '\'' | '\"' | '(' | ')' | '[' | ']'));
                let kind = if looks_jwt(token) {
                    Some((JWT_ID, "JWT", "base64url JWT"))
                } else if looks_hex(token) {
                    Some((HEX_ID, "HEX", "base16"))
                } else if looks_base64(token) {
                    Some((BASE64_ID, "B64", "base64"))
                } else {
                    None
                };
                let Some((id, cue, encoding)) = kind else {
                    continue;
                };
                let offset = line.find(token).unwrap_or(0);
                let data = metadata(cue, encoding, token.len());
                if let Some(value) = detection(
                    window,
                    id,
                    line_start + offset,
                    line_start + offset + token.len(),
                    data,
                ) {
                    out.push(value);
                }
            }
        }
        out
    }
}
fn metadata(kind: &str, encoding: &str, bytes: usize) -> DataNode {
    DataNode::Object(vec![
        ("kind".to_owned(), DataNode::String(kind.to_owned())),
        ("encoding".to_owned(), DataNode::String(encoding.to_owned())),
        (
            "source_bytes".to_owned(),
            DataNode::Number(bytes.to_string()),
        ),
        (
            "decode".to_owned(),
            DataNode::String("explicit preview required".to_owned()),
        ),
    ])
}
fn looks_jwt(value: &str) -> bool {
    let parts = value.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts.iter().all(|part| {
            part.len() >= 8
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
}
fn looks_hex(value: &str) -> bool {
    let value = value.strip_prefix("0x").unwrap_or(value);
    value.len() >= 16 && value.len() % 2 == 0 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn looks_base64(value: &str) -> bool {
    value.len() >= 24
        && value.len() % 4 == 0
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recognizes_jwt_shape() {
        assert!(looks_jwt(
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.abcdefghijklmnop"
        ));
    }
}
