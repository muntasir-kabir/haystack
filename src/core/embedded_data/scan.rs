use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use super::{AnalysisLimits, SourcePos};
use crate::core::document::LogDocument;

#[derive(Debug)]
pub(crate) struct ScanWindow {
    pub(crate) bytes: Vec<u8>,
    lines: Vec<WindowLine>,
}
#[derive(Clone, Copy, Debug)]
struct WindowLine {
    original_line: usize,
    flat_start: usize,
    byte_len: usize,
}

impl ScanWindow {
    pub(crate) fn from_document(
        doc: &LogDocument,
        requested: Range<usize>,
        limits: &AnalysisLimits,
        cancel: &AtomicBool,
    ) -> Option<Self> {
        let end = requested
            .end
            .min(doc.total_lines_untrimmed())
            .min(requested.start.saturating_add(limits.max_lines));
        if requested.start >= end {
            return None;
        }
        let mut bytes = Vec::new();
        let mut lines = Vec::new();
        for original_line in requested.start..end {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let raw = doc.line_bytes_untrimmed(original_line);
            let separator_len = usize::from(!lines.is_empty());
            let remaining = limits.max_bytes.saturating_sub(bytes.len() + separator_len);
            if remaining == 0 {
                break;
            }
            if !lines.is_empty() {
                bytes.push(b'\n');
            }
            let take = raw.len().min(remaining);
            let flat_start = bytes.len();
            bytes.extend_from_slice(&raw[..take]);
            lines.push(WindowLine {
                original_line,
                flat_start,
                byte_len: take,
            });
            if take < raw.len() {
                break;
            }
        }
        (!lines.is_empty()).then_some(Self { bytes, lines })
    }
    pub(crate) fn source_pos(&self, flat_offset: usize) -> SourcePos {
        let line_idx = self
            .lines
            .partition_point(|line| line.flat_start <= flat_offset)
            .saturating_sub(1);
        let line = self.lines[line_idx];
        SourcePos {
            line: line.original_line,
            byte: flat_offset
                .saturating_sub(line.flat_start)
                .min(line.byte_len),
        }
    }
}
