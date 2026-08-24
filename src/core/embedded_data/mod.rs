//! Viewport-scoped detection of structured values embedded inside log text.
//!
//! The registry deliberately keeps syntax detection out of the GUI. Each
//! detector owns only its grammar; presentation is selected by detector id in
//! `ui::log_view::embedded::highlighters`.

pub mod detectors;
mod model;
pub(crate) mod parse;
mod scan;

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::core::document::LogDocument;

pub use model::{AnalysisLimits, DataNode, Detection, RootKind, SourcePos, SourceSpan};
pub(crate) use scan::ScanWindow;

/// A bounded, cancellable embedded-data detector.
pub(crate) trait DataDetector: Send + Sync {
    fn id(&self) -> &'static str;

    fn detect(
        &self,
        window: &ScanWindow,
        limits: &AnalysisLimits,
        cancel: &AtomicBool,
    ) -> Vec<Detection>;
}

/// Registry of all built-in embedded-data detectors. The order is also the
/// tie-break order for exact-span collisions: strict formats beat permissive
/// debug and field syntaxes.
pub struct EmbeddedDataEngine {
    detectors: Vec<Box<dyn DataDetector>>,
}

impl Default for EmbeddedDataEngine {
    fn default() -> Self {
        Self {
            detectors: detectors::builtins(),
        }
    }
}

impl EmbeddedDataEngine {
    pub fn analyze_original_lines(
        &self,
        doc: &LogDocument,
        lines: Range<usize>,
        limits: AnalysisLimits,
        cancel: &AtomicBool,
    ) -> Vec<Detection> {
        let Some(window) = ScanWindow::from_document(doc, lines, &limits, cancel) else {
            return Vec::new();
        };
        let mut detections = Vec::new();
        for detector in &self.detectors {
            if cancel.load(Ordering::Relaxed) || detections.len() >= limits.max_results {
                break;
            }
            detections.extend(detector.detect(&window, &limits, cancel));
            detections.truncate(limits.max_results);
        }
        arbitrate_overlaps(&mut detections);
        detections.sort_by_key(|d| (d.span.start.line, d.span.start.byte));
        detections
    }
}

/// Keep a containing candidate, or the detector registered first for an exact
/// span. This prevents a JSON body from also being painted as loose fields.
fn arbitrate_overlaps(detections: &mut Vec<Detection>) {
    detections.sort_by_key(|d| {
        (
            detector_priority(d.detector_id),
            d.span.start.line,
            d.span.start.byte,
            std::cmp::Reverse(d.source_lines),
            std::cmp::Reverse(d.raw.len()),
        )
    });
    let mut accepted: Vec<Detection> = Vec::new();
    'candidate: for detection in detections.drain(..) {
        for existing in &accepted {
            if spans_overlap(existing.span, detection.span) {
                continue 'candidate;
            }
        }
        accepted.push(detection);
    }
    *detections = accepted;
}

fn detector_priority(detector_id: &str) -> u8 {
    match detector_id {
        // Strict/self-delimiting formats must beat a permissive key/value
        // candidate which happens to contain them.
        "json" => 0,
        "jwt" | "base64" | "hex" | "pem" | "encoded" => 1,
        "http" => 2,
        "stacktrace" => 3,
        "protobuf-text" => 4,
        "python" | "foundation" | "jvm-debug" => 5,
        "logfmt" | "fields" => 10,
        _ => 20,
    }
}

fn spans_overlap(a: SourceSpan, b: SourceSpan) -> bool {
    let a_start = (a.start.line, a.start.byte);
    let a_end = (a.end.line, a.end.byte);
    let b_start = (b.start.line, b.start.byte);
    let b_end = (b.end.line, b.end.byte);
    a_start < b_end && b_start < a_end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_arbitration_keeps_outer_value() {
        let span = |start, end| SourceSpan {
            start: SourcePos {
                line: 0,
                byte: start,
            },
            end: SourcePos { line: 0, byte: end },
        };
        let detection =
            |span, raw| Detection::structured("test", span, raw, DataNode::Object(vec![]));
        let mut input = vec![
            detection(span(0, 20), "outer"),
            detection(span(5, 10), "inner"),
        ];
        arbitrate_overlaps(&mut input);
        assert_eq!(input.len(), 1);
        assert_eq!(input[0].raw, "outer");
    }
}
