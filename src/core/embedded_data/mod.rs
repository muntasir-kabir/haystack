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

/// Keep the higher-confidence detector for overlaps, with two containment
/// refinements: a detector keeps its own outer value, and a Swift/Foundation
/// object keeps ownership of JSON-compatible children. Loose key/value and
/// protobuf-like wrappers still yield to stricter nested values.
fn arbitrate_overlaps(detections: &mut Vec<Detection>) {
    let mut accepted: Vec<Detection> = Vec::new();
    for detection in detections.drain(..) {
        let conflicts = accepted
            .iter()
            .enumerate()
            .filter_map(|(index, existing)| {
                spans_overlap(existing.span, detection.span).then_some(index)
            })
            .collect::<Vec<_>>();
        if conflicts
            .iter()
            .any(|&index| !preferred_overlap(&detection, &accepted[index]))
        {
            continue;
        }
        for &index in conflicts.iter().rev() {
            accepted.remove(index);
        }
        accepted.push(detection);
    }
    *detections = accepted;
}

fn preferred_overlap(candidate: &Detection, existing: &Detection) -> bool {
    let candidate_contains = span_contains(candidate.span, existing.span);
    let existing_contains = span_contains(existing.span, candidate.span);
    if candidate_contains != existing_contains {
        let (container, nested, candidate_is_container) = if candidate_contains {
            (candidate, existing, true)
        } else {
            (existing, candidate, false)
        };
        let container_priority = detector_priority(container.detector_id);
        let nested_priority = detector_priority(nested.detector_id);
        let container_wins = container.detector_id == nested.detector_id
            || (container.detector_id == "foundation" && nested.detector_id == "json")
            || container_priority <= nested_priority;
        return container_wins == candidate_is_container;
    }

    let candidate_rank = (
        detector_priority(candidate.detector_id),
        std::cmp::Reverse(candidate.source_lines),
        std::cmp::Reverse(candidate.raw.len()),
        candidate.span.start.line,
        candidate.span.start.byte,
    );
    let existing_rank = (
        detector_priority(existing.detector_id),
        std::cmp::Reverse(existing.source_lines),
        std::cmp::Reverse(existing.raw.len()),
        existing.span.start.line,
        existing.span.start.byte,
    );
    candidate_rank < existing_rank
}

fn detector_priority(detector_id: &str) -> u8 {
    match detector_id {
        // Strict/self-delimiting formats must beat a permissive key/value
        // candidate which happens to contain them.
        "json" | "plist" | "binary-plist" => 0,
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

fn span_contains(outer: SourceSpan, inner: SourceSpan) -> bool {
    let outer_start = (outer.start.line, outer.start.byte);
    let outer_end = (outer.end.line, outer.end.byte);
    let inner_start = (inner.start.line, inner.start.byte);
    let inner_end = (inner.end.line, inner.end.byte);
    outer_start <= inner_start && outer_end >= inner_end
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

    #[test]
    fn structural_container_beats_strict_nested_value() {
        let outer = SourceSpan {
            start: SourcePos { line: 0, byte: 0 },
            end: SourcePos { line: 0, byte: 30 },
        };
        let inner = SourceSpan {
            start: SourcePos { line: 0, byte: 12 },
            end: SourcePos { line: 0, byte: 24 },
        };
        let mut input = vec![
            Detection::structured("json", inner, "[1,2]", DataNode::Array(vec![])),
            Detection::structured("foundation", outer, "Swift", DataNode::Object(vec![])),
        ];
        arbitrate_overlaps(&mut input);
        assert_eq!(input.len(), 1);
        assert_eq!(input[0].detector_id, "foundation");
    }

    #[test]
    fn loose_field_container_yields_to_strict_nested_value() {
        let outer = SourceSpan {
            start: SourcePos { line: 0, byte: 0 },
            end: SourcePos { line: 0, byte: 30 },
        };
        let inner = SourceSpan {
            start: SourcePos { line: 0, byte: 8 },
            end: SourcePos { line: 0, byte: 24 },
        };
        let mut input = vec![
            Detection::structured("logfmt", outer, "payload fields", DataNode::Object(vec![])),
            Detection::structured("json", inner, "{\"ok\":true}", DataNode::Object(vec![])),
        ];
        arbitrate_overlaps(&mut input);
        assert_eq!(input.len(), 1);
        assert_eq!(input[0].detector_id, "json");
    }

    #[test]
    fn protobuf_like_wrapper_yields_to_nested_json() {
        let outer = SourceSpan {
            start: SourcePos { line: 0, byte: 0 },
            end: SourcePos { line: 2, byte: 1 },
        };
        let inner = SourceSpan {
            start: SourcePos { line: 0, byte: 5 },
            end: SourcePos { line: 2, byte: 1 },
        };
        let mut input = vec![
            Detection::structured(
                "protobuf-text",
                outer,
                "INFO { json }",
                DataNode::Object(vec![]),
            ),
            Detection::structured("json", inner, "{\"ok\":true}", DataNode::Object(vec![])),
        ];
        arbitrate_overlaps(&mut input);
        assert_eq!(input.len(), 1);
        assert_eq!(input[0].detector_id, "json");
    }
}
