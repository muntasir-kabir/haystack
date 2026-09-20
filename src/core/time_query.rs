//! Event-time lookup that is independent of GUI timeline coordinates.

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::core::document::LogDocument;
use crate::core::record::TimeProvenance;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimeBounds {
    pub start: Option<i64>,
    pub end: Option<i64>,
}

impl TimeBounds {
    pub fn contains(self, timestamp: i64) -> bool {
        timestamp >= 0
            && self.start.is_none_or(|start| timestamp >= start)
            && self.end.is_none_or(|end| timestamp <= end)
    }

    pub fn validate(self) -> Result<Self, String> {
        if matches!((self.start, self.end), (Some(start), Some(end)) if end < start) {
            Err("end time is before start time".to_string())
        } else {
            Ok(self)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeSelection {
    /// Matching trim-relative physical line IDs, always in source order.
    pub lines: Vec<u32>,
    /// Smallest contiguous source range containing all matches. This is for
    /// GUI trim operations only and can include intervening nonmatches.
    pub envelope: Option<Range<usize>>,
}

/// Select physical lines by their owning record time. Unknown-time records are
/// excluded whenever a predicate is present; continuations participate through
/// their inherited owner time. Results never follow clock order.
pub fn select_source_order(
    doc: &LogDocument,
    source: Range<usize>,
    bounds: TimeBounds,
    cancel: Option<&AtomicBool>,
) -> Result<TimeSelection, String> {
    let bounds = bounds.validate()?;
    let start = source.start.min(doc.total_lines());
    let end = source.end.min(doc.total_lines()).max(start);
    let mut lines = Vec::new();
    for line in start..end {
        if line % 4_096 == 0 && cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err("time query cancelled".to_string());
        }
        if bounds.contains(doc.ts_at(line)) {
            lines.push(line as u32);
        }
    }
    let envelope = lines
        .first()
        .zip(lines.last())
        .map(|(first, last)| *first as usize..*last as usize + 1);
    Ok(TimeSelection { lines, envelope })
}

/// Closest valid record header to `target`, with equal-distance ties resolved
/// to the earliest physical source line. Continuations are not returned.
pub fn nearest_record_time(doc: &LogDocument, target: i64) -> Option<usize> {
    let mut best: Option<(u64, usize)> = None;
    for line in 0..doc.total_lines() {
        if doc.time_provenance_at(line) != Some(TimeProvenance::Explicit) {
            continue;
        }
        let distance = doc.ts_at(line).abs_diff(target);
        if best.is_none_or(|current| (distance, line) < current) {
            best = Some((distance, line));
        }
    }
    best.map(|(_, line)| line)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockDelta {
    pub previous_line: usize,
    pub line: usize,
    pub delta_ms: i64,
}

/// Signed deltas between consecutive valid headers in physical file order.
pub fn clock_deltas(doc: &LogDocument) -> Vec<ClockDelta> {
    let mut previous = None;
    let mut deltas = Vec::new();
    for line in 0..doc.total_lines() {
        if doc.time_provenance_at(line) != Some(TimeProvenance::Explicit) {
            continue;
        }
        let time = doc.ts_at(line);
        if let Some((previous_line, previous_time)) = previous {
            deltas.push(ClockDelta {
                previous_line,
                line,
                delta_ms: time - previous_time,
            });
        }
        previous = Some((line, time));
    }
    deltas
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::Write;
    use std::path::PathBuf;

    use super::*;

    fn document(content: &str) -> (LogDocument, PathBuf) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "haystack_time_query_{}_{}.log",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        File::create(&path)
            .unwrap()
            .write_all(content.as_bytes())
            .unwrap();
        (LogDocument::open(&path).unwrap(), path)
    }

    #[test]
    fn disordered_selection_stays_in_source_order_and_includes_continuations() {
        let (doc, path) = document(
            "2026-09-11T10:00:02Z first\n\
             continuation first\n\
             2026-09-11T10:00:00Z second\n\
             continuation second\n\
             2026-09-11T10:00:01Z third\n",
        );
        let low = doc.ts_at(4);
        let high = doc.ts_at(0);
        let selected = select_source_order(
            &doc,
            0..doc.total_lines(),
            TimeBounds {
                start: Some(low),
                end: Some(high),
            },
            None,
        )
        .unwrap();
        assert_eq!(selected.lines, vec![0, 1, 4]);
        assert_eq!(selected.envelope, Some(0..5));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn nearest_uses_headers_and_ties_by_earliest_line() {
        let (doc, path) = document(
            "2026-09-11T10:00:02Z first\n\
             continuation\n\
             2026-09-11T10:00:00Z second\n",
        );
        let midpoint = doc.ts_at(2) + 1_000;
        assert_eq!(nearest_record_time(&doc, midpoint), Some(0));
        let deltas = clock_deltas(&doc);
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].delta_ms, -2_000);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn unknown_records_are_excluded_by_time_predicates() {
        let (doc, path) = document(
            "<34>1 2026-09-11T10:00:00Z host app 1 one - first\n\
             <34>1 - host app 2 two - missing\n\
             missing continuation\n",
        );
        let selected = select_source_order(
            &doc,
            0..doc.total_lines(),
            TimeBounds {
                start: Some(doc.ts_at(0)),
                end: None,
            },
            None,
        )
        .unwrap();
        assert_eq!(selected.lines, vec![0]);
        std::fs::remove_file(path).ok();
    }
}
