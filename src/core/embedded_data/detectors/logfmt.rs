use super::detection_with_ranges;
use super::kv::{groups, Separator};
use crate::core::embedded_data::{AnalysisLimits, DataDetector, DataNode, Detection, ScanWindow};
use std::sync::atomic::AtomicBool;

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
        groups(
            &window.bytes,
            Separator::Equals,
            limits.max_depth,
            limits.max_results,
            cancel,
        )
        .into_iter()
        .filter_map(|group| {
            detection_with_ranges(
                window,
                self.id(),
                group.start,
                group.end,
                group.source_ranges,
                DataNode::Object(group.fields),
            )
        })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detector_id_is_stable() {
        assert_eq!(ID, "logfmt");
    }
}
