use super::{detection, line_ranges};
use crate::core::embedded_data::{AnalysisLimits, DataDetector, DataNode, Detection, ScanWindow};
use std::sync::atomic::{AtomicBool, Ordering};

pub const ID: &str = "stacktrace";
pub(super) struct StacktraceDetector;
impl DataDetector for StacktraceDetector {
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
        let mut i = 0;
        while i < ranges.len() && out.len() < limits.max_results {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let (start, end) = ranges[i];
            let Ok(first) = std::str::from_utf8(&window.bytes[start..end]) else {
                i += 1;
                continue;
            };
            let python = first.contains("Traceback (most recent call last)");
            let java = looks_java_exception(first)
                && ranges
                    .get(i + 1)
                    .and_then(|(s, e)| std::str::from_utf8(&window.bytes[*s..*e]).ok())
                    .is_some_and(is_java_frame);
            let js = first.contains("Error")
                && ranges
                    .get(i + 1)
                    .and_then(|(s, e)| std::str::from_utf8(&window.bytes[*s..*e]).ok())
                    .is_some_and(is_js_frame);
            if !(python || java || js) {
                i += 1;
                continue;
            }
            let mut frames = Vec::new();
            let mut last = end;
            let mut cursor = i + 1;
            while let Some(&(frame_start, frame_end)) = ranges.get(cursor) {
                let Ok(line) = std::str::from_utf8(&window.bytes[frame_start..frame_end]) else {
                    break;
                };
                let frame = if python {
                    parse_python_frame(line)
                } else if java {
                    parse_java_frame(line)
                } else {
                    parse_js_frame(line)
                };
                if let Some(frame) = frame {
                    frames.push(frame);
                    last = frame_end;
                    cursor += 1;
                    if frames.len() >= limits.max_frames {
                        break;
                    }
                } else if line.starts_with("Caused by:") || line.starts_with("Suppressed:") {
                    last = frame_end;
                    cursor += 1;
                } else {
                    break;
                }
            }
            if frames.is_empty() {
                i += 1;
                continue;
            }
            let data = DataNode::Object(vec![
                (
                    "exception".to_owned(),
                    DataNode::String(first.trim().to_owned()),
                ),
                ("frames".to_owned(), DataNode::Array(frames)),
            ]);
            if let Some(value) = detection(window, self.id(), start, last, data) {
                out.push(value);
            }
            i = cursor;
        }
        out
    }
}
fn looks_java_exception(line: &str) -> bool {
    line.contains("Exception") || line.contains("Error:") || line.starts_with("Caused by:")
}
fn is_java_frame(line: &str) -> bool {
    line.trim_start().starts_with("at ") || line.trim_start().starts_with("...")
}
fn is_js_frame(line: &str) -> bool {
    line.trim_start().starts_with("at ")
}
fn parse_java_frame(line: &str) -> Option<DataNode> {
    let value = line.trim();
    value
        .strip_prefix("at ")
        .map(|frame| {
            DataNode::Object(vec![(
                "frame".to_owned(),
                DataNode::String(frame.to_owned()),
            )])
        })
        .or_else(|| {
            value.strip_prefix("...").map(|frame| {
                DataNode::Object(vec![(
                    "elided".to_owned(),
                    DataNode::String(frame.trim().to_owned()),
                )])
            })
        })
}
fn parse_js_frame(line: &str) -> Option<DataNode> {
    parse_java_frame(line)
}
fn parse_python_frame(line: &str) -> Option<DataNode> {
    let value = line.trim();
    value.strip_prefix("File ").map(|frame| {
        DataNode::Object(vec![(
            "frame".to_owned(),
            DataNode::String(frame.to_owned()),
        )])
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identifies_java_frame() {
        assert!(is_java_frame("  at app.Main.run(Main.java:4)"));
    }
}
