//! Source-order timeline bucketing. Physical line positions are always the x
//! axis; event timestamps remain metadata for labels, queries, and anomaly
//! annotations rather than coordinates that can reorder the file.

use crate::core::document::LogDocument;
use std::sync::Arc;

pub const DEFAULT_BUCKETS: usize = 2048;

trait MatchLane<T> {
    fn as_match_slice(&self) -> &[T];
}

impl<T> MatchLane<T> for Vec<T> {
    fn as_match_slice(&self) -> &[T] {
        self
    }
}

impl<T> MatchLane<T> for Arc<Vec<T>> {
    fn as_match_slice(&self) -> &[T] {
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineDomain {
    /// Legacy/explicit analytical wall-clock domain. Primary GUI timelines do
    /// not construct this variant.
    Time { start_ms: i64, end_ms: i64 },
    /// X axis is the zero-based physical source-line position.
    Sequence,
}

#[derive(Clone, Debug)]
pub struct Timeline {
    pub domain: TimelineDomain,
    pub n_buckets: usize,
    /// Lines per bucket (whole-file density).
    pub density: Vec<u32>,
    /// Per-filter matches per bucket.
    pub filter_buckets: Vec<Vec<u32>>,
    /// Per-filter line IDs, sorted by their document x-value then line. The
    /// x-value is read from `LogDocument` during resolution instead of being
    /// duplicated for every hit.
    pub filter_lines: Vec<Vec<u32>>,
    /// Compatibility metadata for explicit legacy time-domain timelines.
    /// Source-line timelines do not require timestamp monotonicity.
    pub timestamps_monotonic: bool,
    /// Compatibility storage for explicit legacy time-domain timelines. The
    /// source-line builder leaves it empty and never allocates a sorted copy.
    pub out_of_order_density_lines: Arc<Vec<u32>>,
    pub max_density: u32,
    sequence_end: i64,
}

/// A filter-lane cell resolved for the current viewport and screen width.
/// A singleton carries its exact point so the UI can paint a precise marker and use
/// the real line as its click target; clusters retain their visible spread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResolvedFilterBin {
    pub count: u32,
    pub sole_point: Option<(u32, i64)>,
    pub first_x: i64,
    pub last_x: i64,
    pub first_line: u32,
    pub last_line: u32,
}

impl Timeline {
    pub fn build(doc: &LogDocument, filter_matches: &[Vec<usize>], n_buckets: usize) -> Self {
        Self::build_from_matches(doc, filter_matches, n_buckets)
    }

    /// Build from the GUI's compact 32-bit match indexes.
    pub fn build_u32(doc: &LogDocument, filter_matches: &[Vec<u32>], n_buckets: usize) -> Self {
        Self::build_from_matches(doc, filter_matches, n_buckets)
    }

    /// Build from independently shared GUI match lanes. Retained filters can
    /// then survive a filter-set edit without copying their hit vectors.
    pub fn build_shared_u32(
        doc: &LogDocument,
        filter_matches: &[Arc<Vec<u32>>],
        n_buckets: usize,
    ) -> Self {
        Self::build_from_matches(doc, filter_matches, n_buckets)
    }

    fn build_from_matches<T, M>(doc: &LogDocument, filter_matches: &[M], n_buckets: usize) -> Self
    where
        T: Copy + TryInto<usize>,
        M: MatchLane<T>,
    {
        let n_lines = doc.total_lines();
        let domain = TimelineDomain::Sequence;
        let nb = n_buckets.clamp(16, 8192);

        let bucket_of = |v: i64| -> usize {
            match domain {
                TimelineDomain::Time { start_ms, end_ms } => {
                    let span = (end_ms - start_ms).max(1);
                    ((v - start_ms).clamp(0, span) * (nb as i64 - 1) / span) as usize
                }
                TimelineDomain::Sequence => {
                    (v.clamp(0, n_lines as i64 - 1) * (nb as i64 - 1) / (n_lines as i64 - 1).max(1))
                        as usize
                }
            }
        };

        let x_of_line = |i: usize| -> i64 {
            match domain {
                TimelineDomain::Time { .. } => doc.ts_at(i),
                TimelineDomain::Sequence => i as i64,
            }
        };

        // The overview's unit is records, not physical lines. Rank queries
        // make this exact in O(bucket count), regardless of record length.
        let mut density = vec![0u32; nb];
        if n_lines > 0 {
            for (bucket, count) in density.iter_mut().enumerate() {
                let (start, end) = discrete_bin_bounds(0, n_lines as i64 - 1, bucket, nb);
                *count = doc
                    .count_records(
                        doc.trim_start + start.max(0) as usize,
                        doc.trim_start + end.max(0) as usize,
                    )
                    .min(u32::MAX as usize) as u32;
            }
        }
        let timestamps_monotonic = true;
        let max_density = density.iter().copied().max().unwrap_or(0);

        let mut filter_buckets = Vec::with_capacity(filter_matches.len());
        let mut filter_lines = Vec::with_capacity(filter_matches.len());
        for matches in filter_matches {
            let mut kb = vec![0u32; nb];
            let matches = matches.as_match_slice();
            let mut lines = Vec::with_capacity(matches.len());
            for &line in matches {
                let Ok(ln) = line.try_into() else {
                    continue;
                };
                let v = x_of_line(ln);
                if v < 0 {
                    continue;
                }
                kb[bucket_of(v)] += 1;
                lines.push(ln as u32);
            }
            filter_buckets.push(kb);
            filter_lines.push(lines);
        }

        Timeline {
            domain,
            n_buckets: nb,
            density,
            filter_buckets,
            filter_lines,
            timestamps_monotonic,
            out_of_order_density_lines: Arc::new(Vec::new()),
            max_density,
            sequence_end: n_lines.saturating_sub(1) as i64,
        }
    }

    /// Cheaply retain the document-wide density indexes while discarding old
    /// filter lanes. This lets filter edits rebuild only per-hit state.
    pub fn base_without_filters(&self) -> Self {
        Self {
            domain: self.domain,
            n_buckets: self.n_buckets,
            density: self.density.clone(),
            filter_buckets: Vec::new(),
            filter_lines: Vec::new(),
            timestamps_monotonic: self.timestamps_monotonic,
            out_of_order_density_lines: Arc::clone(&self.out_of_order_density_lines),
            max_density: self.max_density,
            sequence_end: self.sequence_end,
        }
    }

    /// Attach a new set of filter lanes to an existing document-wide base.
    /// Work is proportional to retained hits, not total document lines.
    pub fn with_shared_filter_matches(
        mut self,
        doc: &LogDocument,
        filter_matches: &[Arc<Vec<u32>>],
    ) -> Self {
        let total_lines = doc.total_lines();
        self.filter_buckets.clear();
        self.filter_lines.clear();
        self.filter_buckets.reserve(filter_matches.len());
        self.filter_lines.reserve(filter_matches.len());
        for matches in filter_matches {
            let mut buckets = vec![0u32; self.n_buckets];
            let mut lines = Vec::with_capacity(matches.len());
            for &line in matches.iter() {
                let line_index = line as usize;
                if line_index >= total_lines {
                    continue;
                }
                let x = self.x_of_line(doc, line);
                if x < 0 {
                    continue;
                }
                buckets[self.bucket_for(x, total_lines)] += 1;
                lines.push(line);
            }
            self.filter_buckets.push(buckets);
            self.filter_lines.push(lines);
        }
        self
    }

    /// Extend a source-line timeline after append. Overview bins are rebuilt
    /// exactly from record-rank queries in O(bucket count); no old line or
    /// timestamp index is scanned and clock disorder is irrelevant.
    pub fn extend_append(
        mut self,
        doc: &LogDocument,
        old_line_count: usize,
        filter_matches: &[Arc<Vec<u32>>],
    ) -> Option<Self> {
        let new_line_count = doc.total_lines();
        if old_line_count > new_line_count {
            return None;
        }
        if self.domain != TimelineDomain::Sequence {
            return None;
        }
        self.sequence_end = new_line_count.saturating_sub(1) as i64;
        self.density = vec![0; self.n_buckets];
        if new_line_count > 0 {
            for (bucket, count) in self.density.iter_mut().enumerate() {
                let (start, end) =
                    discrete_bin_bounds(0, new_line_count as i64 - 1, bucket, self.n_buckets);
                *count = doc
                    .count_records(
                        doc.trim_start + start.max(0) as usize,
                        doc.trim_start + end.max(0) as usize,
                    )
                    .min(u32::MAX as usize) as u32;
            }
        }
        self.max_density = self.density.iter().copied().max().unwrap_or(0);
        Some(self.with_shared_filter_matches(doc, filter_matches))
    }

    /// X-value at the center of a bucket (epoch ms or line index).
    pub fn bucket_center(&self, idx: usize) -> i64 {
        match self.domain {
            TimelineDomain::Time { start_ms, end_ms } => {
                let span = (end_ms - start_ms).max(1);
                start_ms + span * idx as i64 / (self.n_buckets as i64 - 1).max(1)
            }
            TimelineDomain::Sequence => {
                self.sequence_end * idx as i64 / (self.n_buckets as i64 - 1).max(1)
            }
        }
    }

    /// Which bucket an x-value (epoch ms or line index) lands in.
    pub fn bucket_for(&self, v: i64, total_lines: usize) -> usize {
        match self.domain {
            TimelineDomain::Time { start_ms, end_ms } => {
                let span = (end_ms - start_ms).max(1);
                ((v - start_ms).clamp(0, span) * (self.n_buckets as i64 - 1) / span) as usize
            }
            TimelineDomain::Sequence => {
                (v.clamp(0, total_lines as i64 - 1) * (self.n_buckets as i64 - 1)
                    / (total_lines as i64 - 1).max(1)) as usize
            }
        }
    }

    #[inline]
    fn x_of_line(&self, doc: &LogDocument, line: u32) -> i64 {
        match self.domain {
            TimelineDomain::Time { .. } => doc.ts_at(line as usize),
            TimelineDomain::Sequence => line as i64,
        }
    }

    /// Line index of the filter match nearest to x-value `v` (any filter).
    /// Uses binary search — O(kw · log n) instead of O(total matches).
    pub fn nearest_match_line(&self, doc: &LogDocument, v: i64) -> Option<usize> {
        let mut best: Option<(u32, i64)> = None;
        for lines in &self.filter_lines {
            if lines.is_empty() {
                continue;
            }
            let idx = lines.partition_point(|&line| self.x_of_line(doc, line) < v);
            // Check the point at the insertion position (first >= v).
            if idx < lines.len() {
                let line = lines[idx];
                let x = self.x_of_line(doc, line);
                let d = (x - v).abs();
                if best.is_none_or(|(_, bd)| d < bd) {
                    best = Some((line, d));
                }
            }
            // Check the point just before the insertion position (last < v).
            if idx > 0 {
                let line = lines[idx - 1];
                let x = self.x_of_line(doc, line);
                let d = (x - v).abs();
                if best.is_none_or(|(_, bd)| d < bd) {
                    best = Some((line, d));
                }
            }
        }
        best.map(|(l, _)| l as usize)
    }

    /// Returns x-sorted filter line IDs that fall within [x_min, x_max].
    pub fn lines_in_range(
        &self,
        doc: &LogDocument,
        ki: usize,
        x_min: i64,
        x_max: i64,
    ) -> Option<&[u32]> {
        let lines = self.filter_lines.get(ki)?;
        if lines.is_empty() {
            return None;
        }
        let lo = lines.partition_point(|&line| self.x_of_line(doc, line) < x_min);
        let hi = lines.partition_point(|&line| self.x_of_line(doc, line) <= x_max);
        if lo >= hi {
            return None;
        }
        Some(&lines[lo..hi])
    }

    /// Number of filter points in [x_min, x_max] (for threshold checks).
    pub fn point_count_in_range(
        &self,
        doc: &LogDocument,
        ki: usize,
        x_min: i64,
        x_max: i64,
    ) -> usize {
        self.lines_in_range(doc, ki, x_min, x_max)
            .map(|s| s.len())
            .unwrap_or(0)
    }

    /// Exact density re-resolution for an inclusive viewport. The amount of
    /// work depends on `columns`, not on the number of visible log lines.
    pub fn resolve_density_bins(
        &self,
        doc: &LogDocument,
        x_min: i64,
        x_max: i64,
        columns: usize,
    ) -> Vec<u32> {
        let columns = columns.max(1);
        let start = x_min.min(x_max);
        let end = x_min.max(x_max);
        let units = end.saturating_sub(start).saturating_add(1) as usize;
        if units <= columns {
            let mut bins = vec![0u32; columns];
            for offset in 0..units {
                let x = start.saturating_add(offset as i64);
                let column = exact_point_column(x, start, end, columns);
                let count = self.density_count_half_open(doc, x, x.saturating_add(1));
                bins[column] = bins[column].saturating_add(count.min(u32::MAX as usize) as u32);
            }
            return bins;
        }
        let mut bins = Vec::with_capacity(columns);
        for column in 0..columns {
            let (start, end) = discrete_bin_bounds(x_min, x_max, column, columns);
            let count = self.density_count_half_open(doc, start, end);
            bins.push(count.min(u32::MAX as usize) as u32);
        }
        bins
    }

    /// Exact per-filter re-resolution for an inclusive viewport.
    pub fn resolve_filter_bins(
        &self,
        doc: &LogDocument,
        ki: usize,
        x_min: i64,
        x_max: i64,
        columns: usize,
    ) -> Vec<ResolvedFilterBin> {
        let columns = columns.max(1);
        let Some(lines) = self.filter_lines.get(ki) else {
            return vec![ResolvedFilterBin::default(); columns];
        };
        let start = x_min.min(x_max);
        let end = x_min.max(x_max);
        let units = end.saturating_sub(start).saturating_add(1) as usize;
        if units <= columns {
            let mut bins = vec![ResolvedFilterBin::default(); columns];
            for offset in 0..units {
                let x = start.saturating_add(offset as i64);
                let lo = lines.partition_point(|&line| self.x_of_line(doc, line) < x);
                let hi = lines.partition_point(|&line| self.x_of_line(doc, line) <= x);
                if lo == hi {
                    continue;
                }
                let column = exact_point_column(x, start, end, columns);
                let count = hi - lo;
                let first_line = lines[lo];
                let last_line = lines[hi - 1];
                bins[column] = ResolvedFilterBin {
                    count: count.min(u32::MAX as usize) as u32,
                    sole_point: (count == 1).then_some((first_line, x)),
                    first_x: x,
                    last_x: x,
                    first_line,
                    last_line,
                };
            }
            return bins;
        }
        let mut bins = Vec::with_capacity(columns);
        for column in 0..columns {
            let (start, end) = discrete_bin_bounds(x_min, x_max, column, columns);
            let lo = lines.partition_point(|&line| self.x_of_line(doc, line) < start);
            let hi = lines.partition_point(|&line| self.x_of_line(doc, line) < end);
            let count = hi.saturating_sub(lo);
            bins.push(if count == 0 {
                ResolvedFilterBin::default()
            } else {
                let first_line = lines[lo];
                let last_line = lines[hi - 1];
                let first_x = self.x_of_line(doc, first_line);
                let last_x = self.x_of_line(doc, last_line);
                ResolvedFilterBin {
                    count: count.min(u32::MAX as usize) as u32,
                    sole_point: (count == 1).then_some((first_line, first_x)),
                    first_x,
                    last_x,
                    first_line,
                    last_line,
                }
            });
        }
        bins
    }

    /// Nearest match in one filter lane, used when a resolved cluster is
    /// clicked. Ties prefer the point on or after the pointer.
    pub fn nearest_match_line_in_filter(
        &self,
        doc: &LogDocument,
        ki: usize,
        v: i64,
    ) -> Option<usize> {
        let lines = self.filter_lines.get(ki)?;
        nearest_line_by_x(self, doc, lines, v).map(|(line, _)| line as usize)
    }

    /// Nearest real log line to a timeline coordinate. This replaces the
    /// former UI-side linear scan when clicking an unfiltered histogram.
    pub fn nearest_line(&self, doc: &LogDocument, v: i64) -> Option<usize> {
        let total = doc.total_lines();
        if total == 0 {
            return None;
        }
        match self.domain {
            TimelineDomain::Sequence => Some(v.clamp(0, total.saturating_sub(1) as i64) as usize),
            TimelineDomain::Time { .. } if self.timestamps_monotonic => {
                let next = lower_bound_document_timestamp(doc, v);
                match (next.checked_sub(1), (next < total).then_some(next)) {
                    (Some(previous), Some(next)) => {
                        let previous_x = doc.ts_at(previous);
                        let next_x = doc.ts_at(next);
                        if previous_x >= 0 && (v - previous_x).abs() < (next_x - v).abs() {
                            Some(previous)
                        } else {
                            Some(next)
                        }
                    }
                    (Some(previous), None) => Some(previous),
                    (None, Some(next)) => Some(next),
                    (None, None) => None,
                }
            }
            TimelineDomain::Time { .. } => {
                nearest_line_by_x(self, doc, &self.out_of_order_density_lines, v)
                    .map(|(line, _)| line as usize)
            }
        }
    }

    fn density_count_half_open(&self, doc: &LogDocument, start: i64, end: i64) -> usize {
        if end <= start {
            return 0;
        }
        match self.domain {
            TimelineDomain::Sequence => {
                let total = doc.total_lines() as i64;
                let lo = start.clamp(0, total);
                let hi = end.clamp(0, total);
                doc.count_records(doc.trim_start + lo as usize, doc.trim_start + hi as usize)
            }
            TimelineDomain::Time { .. } if self.timestamps_monotonic => {
                let lo = lower_bound_document_timestamp(doc, start);
                let hi = lower_bound_document_timestamp(doc, end);
                hi.saturating_sub(lo)
            }
            TimelineDomain::Time { .. } => {
                let lines = &self.out_of_order_density_lines;
                let lo = lines.partition_point(|&line| self.x_of_line(doc, line) < start);
                let hi = lines.partition_point(|&line| self.x_of_line(doc, line) < end);
                hi.saturating_sub(lo)
            }
        }
    }
}

fn nearest_line_by_x(
    timeline: &Timeline,
    doc: &LogDocument,
    lines: &[u32],
    v: i64,
) -> Option<(u32, i64)> {
    if lines.is_empty() {
        return None;
    }
    let idx = lines.partition_point(|&line| timeline.x_of_line(doc, line) < v);
    match (idx.checked_sub(1), lines.get(idx).copied()) {
        (Some(previous_index), Some(next_line)) => {
            let previous_line = lines[previous_index];
            let previous_x = timeline.x_of_line(doc, previous_line);
            let next_x = timeline.x_of_line(doc, next_line);
            if (v - previous_x).abs() < (next_x - v).abs() {
                Some((previous_line, previous_x))
            } else {
                Some((next_line, next_x))
            }
        }
        (Some(previous_index), None) => {
            let line = lines[previous_index];
            Some((line, timeline.x_of_line(doc, line)))
        }
        (None, Some(line)) => Some((line, timeline.x_of_line(doc, line))),
        (None, None) => None,
    }
}

fn lower_bound_document_timestamp(doc: &LogDocument, target: i64) -> usize {
    let mut lo = 0usize;
    let mut hi = doc.total_lines();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if doc.ts_at(mid) < target {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

/// Return `[start, end)` integer-domain bounds for one screen cell. Empty
/// cells are valid when the viewport contains fewer domain units than pixels.
fn discrete_bin_bounds(x_min: i64, x_max: i64, column: usize, columns: usize) -> (i64, i64) {
    let start = x_min.min(x_max);
    let inclusive_end = x_min.max(x_max);
    let units = inclusive_end.saturating_sub(start).saturating_add(1) as i128;
    let offset = |index: usize| -> i64 {
        let value = units * index as i128 / columns.max(1) as i128;
        value.min(i64::MAX as i128) as i64
    };
    (
        start.saturating_add(offset(column)),
        start.saturating_add(offset(column + 1)),
    )
}

fn exact_point_column(x: i64, start: i64, end: i64, columns: usize) -> usize {
    if columns <= 1 || end <= start {
        return 0;
    }
    let numerator = x.saturating_sub(start) as i128 * (columns - 1) as i128;
    let denominator = end.saturating_sub(start).max(1) as i128;
    (numerator / denominator).clamp(0, (columns - 1) as i128) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn doc_with(content: &str) -> LogDocument {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "haystack_timeline_test_{}_{}.log",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        let doc = LogDocument::open(&path).unwrap();
        std::fs::remove_file(path).ok();
        doc
    }

    #[test]
    fn record_overview_covers_all_headers_in_source_line_domain() {
        let mut content = String::new();
        for i in 0..1000 {
            content.push_str(&format!(
                "2026-07-19T10:{:02}:{:02}.000Z INFO line {}\n",
                (i / 60) % 60,
                i % 60,
                i
            ));
        }
        let doc = doc_with(&content);
        let tl = Timeline::build(&doc, &[], 128);
        assert_eq!(tl.domain, TimelineDomain::Sequence);
        let sum: u32 = tl.density.iter().sum();
        assert_eq!(sum as usize, doc.record_count());
    }

    #[test]
    fn timeless_files_use_sequence_domain() {
        let doc = doc_with("alpha\nbeta\ngamma\n");
        let tl = Timeline::build(&doc, &[], 16);
        assert_eq!(tl.domain, TimelineDomain::Sequence);
        let sum: u32 = tl.density.iter().sum();
        assert_eq!(sum, 0, "unresolved files use neutral record coverage");
    }

    #[test]
    fn filter_lines_track_matches_without_duplicating_timestamps() {
        let doc = doc_with(
            "2026-07-19T10:00:00.000Z err a\n2026-07-19T10:01:00.000Z ok\n2026-07-19T10:02:00.000Z err b\n",
        );
        let tl = Timeline::build(&doc, &[vec![0, 2]], 16);
        assert_eq!(tl.filter_lines, vec![vec![0, 2]]);
        assert_eq!(std::mem::size_of_val(&tl.filter_lines[0][0]), 4);
        let mid = doc.ts_at(1);
        // Equidistant tie → binary search picks the first >= v (line 2).
        assert!(
            tl.nearest_match_line(&doc, mid) == Some(0)
                || tl.nearest_match_line(&doc, mid) == Some(2)
        );
        assert_eq!(tl.nearest_match_line(&doc, doc.ts_at(2)), Some(2));
    }

    #[test]
    fn retained_density_base_rebuilds_only_filter_indexes() {
        let doc = doc_with(
            "2026-07-19T10:00:00.000Z alpha\n\
             2026-07-19T10:01:00.000Z beta\n\
             2026-07-19T10:02:00.000Z alpha beta\n",
        );
        let full = Timeline::build_u32(&doc, &[vec![0, 2], vec![1, 2]], 16);
        let shared = vec![Arc::new(vec![0, 2]), Arc::new(vec![1, 2])];
        let rebuilt = full
            .base_without_filters()
            .with_shared_filter_matches(&doc, &shared);
        assert_eq!(rebuilt.domain, full.domain);
        assert_eq!(rebuilt.density, full.density);
        assert_eq!(rebuilt.filter_buckets, full.filter_buckets);
        assert_eq!(rebuilt.filter_lines, full.filter_lines);
        assert!(Arc::ptr_eq(
            &rebuilt.out_of_order_density_lines,
            &full.out_of_order_density_lines
        ));
    }

    #[test]
    fn append_extension_reads_only_new_density_lines_and_keeps_exact_resolution() {
        let old_doc = doc_with(
            "2026-07-19T10:00:00.000Z alpha\n\
             2026-07-19T10:01:00.000Z beta\n",
        );
        let new_doc = doc_with(
            "2026-07-19T10:00:00.000Z alpha\n\
             2026-07-19T10:01:00.000Z beta\n\
             2026-07-19T10:02:00.000Z alpha\n\
             2026-07-19T10:03:00.000Z gamma\n",
        );
        let old = Timeline::build_u32(&old_doc, &[vec![0]], 16);
        let matches = vec![Arc::new(vec![0, 2])];
        let extended = old
            .base_without_filters()
            .extend_append(&new_doc, 2, &matches)
            .expect("chronological append should be incremental");
        assert_eq!(extended.density.iter().sum::<u32>(), 4);
        assert_eq!(extended.filter_lines, vec![vec![0, 2]]);
        assert_eq!(
            extended
                .resolve_density_bins(&new_doc, 0, 3, 64)
                .iter()
                .sum::<u32>(),
            4
        );
    }

    #[test]
    fn filter_buckets_and_points_match_input_length() {
        // This verifies the invariant that filter_buckets.len() and
        // filter_lines.len() always match the number of filter match
        // sets passed to Timeline::build, regardless of how many matches
        // each set contains. The UI depends on this consistency.
        let doc = doc_with(
            "2026-07-19T10:00:00.000Z a\n2026-07-19T10:01:00.000Z b\n2026-07-19T10:02:00.000Z c\n",
        );

        // ----- 0 filter sets -----
        let tl = Timeline::build(&doc, &[], 16);
        assert_eq!(tl.filter_buckets.len(), 0);
        assert_eq!(tl.filter_lines.len(), 0);

        // ----- 1 filter set -----
        let tl = Timeline::build(&doc, &[vec![0, 2]], 16);
        assert_eq!(tl.filter_buckets.len(), 1);
        assert_eq!(tl.filter_lines.len(), 1);

        // ----- 3 filter sets -----
        let tl = Timeline::build(&doc, &[vec![0], vec![2], vec![1]], 16);
        assert_eq!(tl.filter_buckets.len(), 3);
        assert_eq!(tl.filter_lines.len(), 3);
        for i in 0..3 {
            assert_eq!(tl.filter_lines[i].len(), 1);
        }

        // ----- 3 sets, some empty -----
        let tl = Timeline::build(&doc, &[vec![0], vec![], vec![1]], 16);
        assert_eq!(tl.filter_buckets.len(), 3);
        assert_eq!(tl.filter_lines.len(), 3);
        assert_eq!(tl.filter_lines[0].len(), 1);
        assert_eq!(tl.filter_lines[1].len(), 0);
        assert_eq!(tl.filter_lines[2].len(), 1);
    }

    #[test]
    fn adaptive_density_preserves_counts_at_multiple_resolutions() {
        let doc = doc_with(
            "2026-07-19T10:00:00.000Z a\n\
             2026-07-19T10:00:00.001Z b\n\
             2026-07-19T10:00:00.002Z c\n\
             2026-07-19T10:00:00.003Z d\n\
             2026-07-19T10:00:00.004Z e\n",
        );
        let tl = Timeline::build(&doc, &[], 16);
        for columns in [1, 2, 5, 64] {
            let bins = tl.resolve_density_bins(&doc, 0, 4, columns);
            assert_eq!(bins.iter().sum::<u32>(), 5, "columns={columns}");
        }
    }

    #[test]
    fn adaptive_filter_bins_split_clusters_into_singletons_when_zoomed() {
        let doc = doc_with(
            "2026-07-19T10:00:00.000Z hit\n\
             2026-07-19T10:00:00.001Z hit\n\
             2026-07-19T10:00:00.002Z hit\n",
        );
        let tl = Timeline::build(&doc, &[vec![0, 1, 2]], 16);
        let coarse = tl.resolve_filter_bins(&doc, 0, 0, 2, 1);
        assert_eq!(coarse[0].count, 3);
        assert_eq!(coarse[0].sole_point, None);

        let fine = tl.resolve_filter_bins(&doc, 0, 0, 2, 3);
        assert_eq!(fine.iter().map(|bin| bin.count).sum::<u32>(), 3);
        assert!(fine
            .iter()
            .filter(|bin| bin.count == 1)
            .all(|bin| bin.sole_point.is_some()));

        let wide = tl.resolve_filter_bins(&doc, 0, 0, 2, 101);
        let occupied: Vec<usize> = wide
            .iter()
            .enumerate()
            .filter_map(|(index, bin)| (bin.count > 0).then_some(index))
            .collect();
        assert_eq!(occupied, vec![0, 50, 100]);
    }

    #[test]
    fn identical_timestamps_keep_distinct_source_positions() {
        let doc = doc_with(
            "2026-07-19T10:00:00.000Z hit a\n\
             2026-07-19T10:00:00.000Z hit b\n\
             2026-07-19T10:00:00.001Z tail\n",
        );
        let tl = Timeline::build(&doc, &[vec![0, 1]], 16);
        let bins = tl.resolve_filter_bins(&doc, 0, 0, 1, 2);
        assert_eq!(bins.iter().map(|bin| bin.count).sum::<u32>(), 2);
        assert_eq!(bins[0].first_x, 0);
        assert_eq!(bins[1].last_x, 1);
    }

    #[test]
    fn out_of_order_timestamps_never_reorder_source_positions() {
        let doc = doc_with(
            "2026-07-19T10:00:02.000Z late\n\
             2026-07-19T10:00:00.000Z early\n\
             2026-07-19T10:00:01.000Z middle\n",
        );
        let tl = Timeline::build(&doc, &[vec![0, 1, 2]], 16);
        assert_eq!(tl.domain, TimelineDomain::Sequence);
        assert!(tl.out_of_order_density_lines.is_empty());
        assert_eq!(tl.filter_lines[0], vec![0, 1, 2]);
        assert_eq!(
            tl.resolve_density_bins(&doc, 0, 2, 3).iter().sum::<u32>(),
            3
        );
        assert_eq!(tl.point_count_in_range(&doc, 0, 0, 2), 3);
        assert_eq!(tl.nearest_match_line_in_filter(&doc, 0, 0), Some(0));
    }

    #[test]
    fn sequence_resolution_counts_exact_lines() {
        let doc = doc_with(
            "2026-07-19T10:00:00Z a\n\
             2026-07-19T10:00:01Z b\n\
             2026-07-19T10:00:02Z c\n\
             2026-07-19T10:00:03Z d\n\
             2026-07-19T10:00:04Z e\n",
        );
        let tl = Timeline::build(&doc, &[vec![0, 4]], 16);
        let bins = tl.resolve_density_bins(&doc, 0, 2, 8);
        assert_eq!(bins.iter().sum::<u32>(), 3);
    }
}
