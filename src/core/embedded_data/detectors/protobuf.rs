use super::detection;
use crate::core::embedded_data::parse::{balanced_end, scalar, split_top_level};
use crate::core::embedded_data::{AnalysisLimits, DataDetector, DataNode, Detection, ScanWindow};
use std::sync::atomic::{AtomicBool, Ordering};

pub const ID: &str = "protobuf-text";
pub(super) struct ProtobufDetector;
impl DataDetector for ProtobufDetector {
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
        for brace in 0..bytes.len() {
            if brace % 4096 == 0 && cancel.load(Ordering::Relaxed) {
                break;
            }
            if bytes[brace] != b'{' {
                continue;
            }
            let Some(end) = balanced_end(bytes, brace, limits.max_depth) else {
                continue;
            };
            let mut name_end = brace;
            while name_end > 0 && bytes[name_end - 1].is_ascii_whitespace() {
                name_end -= 1;
            }
            let prefix_start = bytes[..name_end]
                .iter()
                .rposition(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_' && *byte != b'.')
                .map(|offset| offset + 1)
                .unwrap_or(0);
            let Ok(name) = std::str::from_utf8(&bytes[prefix_start..name_end]) else {
                continue;
            };
            if name.trim().is_empty() {
                continue;
            }
            let Ok(raw) = std::str::from_utf8(&bytes[prefix_start..end]) else {
                continue;
            };
            if !raw.contains(':') {
                continue;
            }
            let mut root = DataNode::Object(vec![(
                name.trim().to_owned(),
                message(&raw[name.len()..], limits.max_depth),
            )]);
            if let DataNode::Object(entries) = &mut root {
                if entries.is_empty() {
                    continue;
                }
            }
            if let Some(value) = detection(window, self.id(), prefix_start, end, root) {
                out.push(value);
            }
            if out.len() >= limits.max_results {
                break;
            }
        }
        out
    }
}
fn message(raw: &str, max_depth: usize) -> DataNode {
    let body = raw
        .trim()
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
        .unwrap_or(raw);
    let mut entries = Vec::new();
    for part in split_top_level(body, &[',', ';', '\n']) {
        if let Some((key, value)) = part.split_once(':') {
            entries.push((
                key.trim().to_owned(),
                if value.trim().starts_with('{') {
                    message(value, max_depth.saturating_sub(1))
                } else {
                    scalar(value)
                },
            ));
        }
    }
    DataNode::Object(entries)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn message_reads_scalar() {
        assert!(matches!(message("{ id: 42 }", 8), DataNode::Object(_)));
    }
}
