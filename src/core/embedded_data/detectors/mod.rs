//! Syntax-specific embedded-data detectors. Keep each detector isolated so a
//! permissive debug grammar cannot accidentally change another format.

mod encoded;
mod fields;
mod foundation;
mod http;
mod json;
mod jvm_debug;
mod logfmt;
mod protobuf;
mod python;
mod stacktrace;

use super::DataDetector;
use super::{DataNode, Detection, ScanWindow, SourceSpan};

pub(super) fn builtins() -> Vec<Box<dyn DataDetector>> {
    vec![
        Box::new(json::JsonDetector),
        Box::new(encoded::EncodedDetector),
        Box::new(http::HttpDetector),
        Box::new(stacktrace::StacktraceDetector),
        Box::new(protobuf::ProtobufDetector),
        Box::new(python::PythonDetector),
        Box::new(foundation::FoundationDetector),
        Box::new(jvm_debug::JvmDebugDetector),
        Box::new(logfmt::LogfmtDetector),
        Box::new(fields::FieldsDetector),
    ]
}

fn detection(
    window: &ScanWindow,
    id: &'static str,
    start: usize,
    end: usize,
    data: DataNode,
) -> Option<Detection> {
    let raw = std::str::from_utf8(window.bytes.get(start..end)?).ok()?;
    let span = SourceSpan {
        start: window.source_pos(start),
        end: window.source_pos(end),
    };
    Some(Detection::structured(id, span, raw, data))
}

fn line_ranges(bytes: &[u8]) -> impl Iterator<Item = (usize, usize)> + '_ {
    let mut start = 0usize;
    std::iter::from_fn(move || {
        if start >= bytes.len() {
            return None;
        }
        let end = bytes[start..]
            .iter()
            .position(|&byte| byte == b'\n')
            .map(|offset| start + offset)
            .unwrap_or(bytes.len());
        let result = (start, end);
        start = end.saturating_add(1);
        Some(result)
    })
}
