use super::detection;
use crate::core::embedded_data::parse::{balanced_end, parse_collection};
use crate::core::embedded_data::{AnalysisLimits, DataDetector, DataNode, Detection, ScanWindow};
use std::sync::atomic::{AtomicBool, Ordering};

pub const ID: &str = "jvm-debug";
pub(super) struct JvmDebugDetector;
impl DataDetector for JvmDebugDetector {
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
        let bytes = &window.bytes;
        for start in 0..bytes.len() {
            if start % 4096 == 0 && cancel.load(Ordering::Relaxed) {
                break;
            }
            let (candidate_start, opener, type_name) = if bytes[start] == b'{' {
                (start, start, None)
            } else if bytes[start].is_ascii_alphabetic() {
                let name_end = bytes[start..]
                    .iter()
                    .position(|byte| {
                        !byte.is_ascii_alphanumeric() && *byte != b'_' && *byte != b'.'
                    })
                    .map(|offset| start + offset)
                    .unwrap_or(bytes.len());
                if name_end < bytes.len() && matches!(bytes[name_end], b'(' | b'[') {
                    (
                        start,
                        name_end,
                        std::str::from_utf8(&bytes[start..name_end]).ok(),
                    )
                } else {
                    continue;
                }
            } else {
                continue;
            };
            let Some(end) = balanced_end(bytes, opener, limits.max_depth) else {
                continue;
            };
            let Ok(raw) = std::str::from_utf8(&bytes[candidate_start..end]) else {
                continue;
            };
            if !raw.contains('=') || raw.contains(';') {
                continue;
            }
            let enclosed = &raw[raw.find(['{', '(', '[']).unwrap()..];
            let Some(mut data) = parse_collection(enclosed, &['='], &[','], 0, limits.max_depth)
            else {
                continue;
            };
            if let Some(type_name) = type_name {
                if let DataNode::Object(entries) = &mut data {
                    entries.insert(
                        0,
                        ("$type".to_owned(), DataNode::String(type_name.to_owned())),
                    );
                }
            }
            if let Some(value) = detection(window, self.id(), candidate_start, end, data) {
                out.push(value);
            }
            if out.len() >= limits.max_results {
                break;
            }
        }
        out
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn profile_id_is_stable() {
        assert_eq!(super::ID, "jvm-debug");
    }
}
