use super::detection;
use crate::core::embedded_data::parse::{balanced_end, parse_collection};
use crate::core::embedded_data::{AnalysisLimits, DataDetector, Detection, ScanWindow};
use std::sync::atomic::{AtomicBool, Ordering};

pub const ID: &str = "foundation";
pub(super) struct FoundationDetector;
impl DataDetector for FoundationDetector {
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
        for start in 0..window.bytes.len() {
            if start % 4096 == 0 && cancel.load(Ordering::Relaxed) {
                break;
            }
            if !matches!(window.bytes[start], b'{' | b'(') {
                continue;
            }
            let Some(end) = balanced_end(&window.bytes, start, limits.max_depth) else {
                continue;
            };
            let Ok(raw) = std::str::from_utf8(&window.bytes[start..end]) else {
                continue;
            };
            let valid = if raw.starts_with('{') {
                raw.contains('=') && raw.contains(';')
            } else {
                raw.contains(',') && !raw.contains('=')
            };
            if !valid {
                continue;
            }
            let Some(data) = parse_collection(raw, &['='], &[';', ','], 0, limits.max_depth) else {
                continue;
            };
            if let Some(value) = detection(window, self.id(), start, end, data) {
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
    use crate::core::embedded_data::parse::parse_collection;
    use crate::core::embedded_data::DataNode;
    #[test]
    fn parses_dictionary() {
        assert!(matches!(
            parse_collection("{ user = Ada; ok = true; }", &['='], &[';', ','], 0, 8),
            Some(DataNode::Object(_))
        ));
    }
}
