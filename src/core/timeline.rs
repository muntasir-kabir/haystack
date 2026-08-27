//! Timeline bucketing: turns per-line timestamps (or, for timeless files,
//! plain line numbers) into a fixed-resolution histogram that the UI can
//! paint in O(buckets) instead of O(lines).

use crate::core::document::LogDocument;

pub const DEFAULT_BUCKETS: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineDomain {
    /// X axis is wall-clock time (epoch millis).
    Time { start_ms: i64, end_ms: i64 },
    /// X axis is line number (file has no usable timestamps).
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
    /// Per-filter `(line_idx, x-value)` points, sorted by x-value then line.
    /// x-value is epoch ms in Time domain, line index in Sequence domain.
    pub filter_points: Vec<Vec<(u32, i64)>>,
    /// True when the document's valid timestamps are non-decreasing in line
    /// order. The adaptive resolver can then binary-search the document's
    /// existing timestamp index without duplicating it.
    pub timestamps_monotonic: bool,
    /// Sorted `(line_idx, x_value)` fallback used only for timestamped logs
    /// whose timestamps move backwards. Normal chronological logs leave this
    /// empty, avoiding another per-line timestamp allocation.
    pub out_of_order_density_points: Vec<(u32, i64)>,
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

    fn build_from_matches<T>(doc: &LogDocument, filter_matches: &[Vec<T>], n_buckets: usize) -> Self
    where
        T: Copy + TryInto<usize>,
    {
        let n_lines = doc.total_lines();
        let domain = match doc.time_range {
            Some((a, b)) if b > a => TimelineDomain::Time {
                start_ms: a,
                end_ms: b,
            },
            _ => TimelineDomain::Sequence,
        };
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

        let mut density = vec![0u32; nb];
        let mut timestamps_monotonic = true;
        let mut previous_timestamp: Option<i64> = None;
        for i in 0..n_lines {
            let v = x_of_line(i);
            if v < 0 {
                continue; // before first known timestamp
            }
            if matches!(domain, TimelineDomain::Time { .. }) {
                if previous_timestamp.is_some_and(|previous| v < previous) {
                    timestamps_monotonic = false;
                }
                previous_timestamp = Some(v);
            }
            density[bucket_of(v)] += 1;
        }
        let max_density = density.iter().copied().max().unwrap_or(0);

        let mut filter_buckets = Vec::with_capacity(filter_matches.len());
        let mut filter_points = Vec::with_capacity(filter_matches.len());
        for matches in filter_matches {
            let mut kb = vec![0u32; nb];
            let mut pts = Vec::with_capacity(matches.len());
            for &line in matches {
                let Ok(ln) = line.try_into() else {
                    continue;
                };
                let v = x_of_line(ln);
                if v < 0 {
                    continue;
                }
                kb[bucket_of(v)] += 1;
                pts.push((ln as u32, v));
            }
            // Match lists are line-ordered for navigation, but timeline range
            // queries must be x-ordered (timestamps may move backwards).
            pts.sort_unstable_by_key(|&(line, x)| (x, line));
            filter_buckets.push(kb);
            filter_points.push(pts);
        }

        let mut out_of_order_density_points = Vec::new();
        if matches!(domain, TimelineDomain::Time { .. }) && !timestamps_monotonic {
            out_of_order_density_points.reserve(n_lines);
            for line in 0..n_lines {
                let x = doc.ts_at(line);
                if x >= 0 {
                    out_of_order_density_points.push((line as u32, x));
                }
            }
            out_of_order_density_points.sort_unstable_by_key(|&(line, x)| (x, line));
        }

        Timeline {
            domain,
            n_buckets: nb,
            density,
            filter_buckets,
            filter_points,
            timestamps_monotonic,
            out_of_order_density_points,
            max_density,
            sequence_end: n_lines.saturating_sub(1) as i64,
        }
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

    /// Line index of the filter match nearest to x-value `v` (any filter).
    /// Uses binary search — O(kw · log n) instead of O(total matches).
    pub fn nearest_match_line(&self, v: i64) -> Option<usize> {
        let mut best: Option<(u32, i64)> = None;
        for pts in &self.filter_points {
            if pts.is_empty() {
                continue;
            }
            let idx = pts.partition_point(|&(_, x)| x < v);
            // Check the point at the insertion position (first >= v).
            if idx < pts.len() {
                let (line, x) = pts[idx];
                let d = (x - v).abs();
                if best.is_none_or(|(_, bd)| d < bd) {
                    best = Some((line, d));
                }
            }
            // Check the point just before the insertion position (last < v).
            if idx > 0 {
                let (line, x) = pts[idx - 1];
                let d = (x - v).abs();
                if best.is_none_or(|(_, bd)| d < bd) {
                    best = Some((line, d));
                }
            }
        }
        best.map(|(l, _)| l as usize)
    }

    /// Returns filter point slices that fall within [x_min, x_max].
    /// Points are `(line_index, x_value)`. Uses binary search per filter lane.
    pub fn points_in_range(&self, ki: usize, x_min: i64, x_max: i64) -> Option<&[(u32, i64)]> {
        let pts = self.filter_points.get(ki)?;
        if pts.is_empty() {
            return None;
        }
        let lo = pts.partition_point(|&(_, x)| x < x_min);
        let hi = pts.partition_point(|&(_, x)| x <= x_max);
        if lo >= hi {
            return None;
        }
        Some(&pts[lo..hi])
    }

    /// Number of filter points in [x_min, x_max] (for threshold checks).
    pub fn point_count_in_range(&self, ki: usize, x_min: i64, x_max: i64) -> usize {
        self.points_in_range(ki, x_min, x_max)
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
        ki: usize,
        x_min: i64,
        x_max: i64,
        columns: usize,
    ) -> Vec<ResolvedFilterBin> {
        let columns = columns.max(1);
        let Some(points) = self.filter_points.get(ki) else {
            return vec![ResolvedFilterBin::default(); columns];
        };
        let start = x_min.min(x_max);
        let end = x_min.max(x_max);
        let units = end.saturating_sub(start).saturating_add(1) as usize;
        if units <= columns {
            let mut bins = vec![ResolvedFilterBin::default(); columns];
            for offset in 0..units {
                let x = start.saturating_add(offset as i64);
                let lo = points.partition_point(|&(_, point_x)| point_x < x);
                let hi = points.partition_point(|&(_, point_x)| point_x <= x);
                if lo == hi {
                    continue;
                }
                let column = exact_point_column(x, start, end, columns);
                let count = hi - lo;
                bins[column] = ResolvedFilterBin {
                    count: count.min(u32::MAX as usize) as u32,
                    sole_point: (count == 1).then_some(points[lo]),
                    first_x: x,
                    last_x: x,
                    first_line: points[lo].0,
                    last_line: points[hi - 1].0,
                };
            }
            return bins;
        }
        let mut bins = Vec::with_capacity(columns);
        for column in 0..columns {
            let (start, end) = discrete_bin_bounds(x_min, x_max, column, columns);
            let lo = points.partition_point(|&(_, x)| x < start);
            let hi = points.partition_point(|&(_, x)| x < end);
            let count = hi.saturating_sub(lo);
            bins.push(if count == 0 {
                ResolvedFilterBin::default()
            } else {
                ResolvedFilterBin {
                    count: count.min(u32::MAX as usize) as u32,
                    sole_point: (count == 1).then_some(points[lo]),
                    first_x: points[lo].1,
                    last_x: points[hi - 1].1,
                    first_line: points[lo].0,
                    last_line: points[hi - 1].0,
                }
            });
        }
        bins
    }

    /// Nearest match in one filter lane, used when a resolved cluster is
    /// clicked. Ties prefer the point on or after the pointer.
    pub fn nearest_match_line_in_filter(&self, ki: usize, v: i64) -> Option<usize> {
        let points = self.filter_points.get(ki)?;
        nearest_point(points, v).map(|(line, _)| line as usize)
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
                nearest_point(&self.out_of_order_density_points, v).map(|(line, _)| line as usize)
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
                hi.saturating_sub(lo) as usize
            }
            TimelineDomain::Time { .. } if self.timestamps_monotonic => {
                let lo = lower_bound_document_timestamp(doc, start);
                let hi = lower_bound_document_timestamp(doc, end);
                hi.saturating_sub(lo)
            }
            TimelineDomain::Time { .. } => {
                let points = &self.out_of_order_density_points;
                let lo = points.partition_point(|&(_, x)| x < start);
                let hi = points.partition_point(|&(_, x)| x < end);
                hi.saturating_sub(lo)
            }
        }
    }
}

fn nearest_point(points: &[(u32, i64)], v: i64) -> Option<(u32, i64)> {
    if points.is_empty() {
        return None;
    }
    let idx = points.partition_point(|&(_, x)| x < v);
    match (idx.checked_sub(1), points.get(idx).copied()) {
        (Some(previous), Some(next)) => {
            let previous = points[previous];
            if (v - previous.1).abs() < (next.1 - v).abs() {
                Some(previous)
            } else {
                Some(next)
            }
        }
        (Some(previous), None) => Some(points[previous]),
        (None, Some(next)) => Some(next),
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
            "logotomy_timeline_test_{}_{}.log",
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
    fn buckets_cover_all_timestamped_lines() {
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
        assert!(matches!(tl.domain, TimelineDomain::Time { .. }));
        let sum: u32 = tl.density.iter().sum();
        assert_eq!(sum as usize, doc.total_lines());
    }

    #[test]
    fn timeless_files_use_sequence_domain() {
        let doc = doc_with("alpha\nbeta\ngamma\n");
        let tl = Timeline::build(&doc, &[], 16);
        assert_eq!(tl.domain, TimelineDomain::Sequence);
        let sum: u32 = tl.density.iter().sum();
        assert_eq!(sum, 3);
    }

    #[test]
    fn filter_points_track_matches() {
        let doc = doc_with(
            "2026-07-19T10:00:00.000Z err a\n2026-07-19T10:01:00.000Z ok\n2026-07-19T10:02:00.000Z err b\n",
        );
        let tl = Timeline::build(&doc, &[vec![0, 2]], 16);
        assert_eq!(tl.filter_points.len(), 1);
        assert_eq!(tl.filter_points[0].len(), 2);
        let mid = doc.ts_at(1);
        // Equidistant tie → binary search picks the first >= v (line 2).
        assert!(tl.nearest_match_line(mid) == Some(0) || tl.nearest_match_line(mid) == Some(2));
        assert_eq!(tl.nearest_match_line(doc.ts_at(2)), Some(2));
    }

    #[test]
    fn filter_buckets_and_points_match_input_length() {
        // This verifies the invariant that filter_buckets.len() and
        // filter_points.len() always match the number of filter match
        // sets passed to Timeline::build, regardless of how many matches
        // each set contains. The UI depends on this consistency.
        let doc = doc_with(
            "2026-07-19T10:00:00.000Z a\n2026-07-19T10:01:00.000Z b\n2026-07-19T10:02:00.000Z c\n",
        );

        // ----- 0 filter sets -----
        let tl = Timeline::build(&doc, &[], 16);
        assert_eq!(tl.filter_buckets.len(), 0);
        assert_eq!(tl.filter_points.len(), 0);

        // ----- 1 filter set -----
        let tl = Timeline::build(&doc, &[vec![0, 2]], 16);
        assert_eq!(tl.filter_buckets.len(), 1);
        assert_eq!(tl.filter_points.len(), 1);

        // ----- 3 filter sets -----
        let tl = Timeline::build(&doc, &[vec![0], vec![2], vec![1]], 16);
        assert_eq!(tl.filter_buckets.len(), 3);
        assert_eq!(tl.filter_points.len(), 3);
        for i in 0..3 {
            assert_eq!(tl.filter_points[i].len(), 1);
        }

        // ----- 3 sets, some empty -----
        let tl = Timeline::build(&doc, &[vec![0], vec![], vec![1]], 16);
        assert_eq!(tl.filter_buckets.len(), 3);
        assert_eq!(tl.filter_points.len(), 3);
        assert_eq!(tl.filter_points[0].len(), 1);
        assert_eq!(tl.filter_points[1].len(), 0);
        assert_eq!(tl.filter_points[2].len(), 1);
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
        let (start, end) = doc.time_range.unwrap();
        for columns in [1, 2, 5, 64] {
            let bins = tl.resolve_density_bins(&doc, start, end, columns);
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
        let (start, end) = doc.time_range.unwrap();

        let coarse = tl.resolve_filter_bins(0, start, end, 1);
        assert_eq!(coarse[0].count, 3);
        assert_eq!(coarse[0].sole_point, None);

        let fine = tl.resolve_filter_bins(0, start, end, 3);
        assert_eq!(fine.iter().map(|bin| bin.count).sum::<u32>(), 3);
        assert!(fine
            .iter()
            .filter(|bin| bin.count == 1)
            .all(|bin| bin.sole_point.is_some()));

        let wide = tl.resolve_filter_bins(0, start, end, 101);
        let occupied: Vec<usize> = wide
            .iter()
            .enumerate()
            .filter_map(|(index, bin)| (bin.count > 0).then_some(index))
            .collect();
        assert_eq!(occupied, vec![0, 50, 100]);
    }

    #[test]
    fn identical_timestamps_remain_an_exact_cluster() {
        let doc = doc_with(
            "2026-07-19T10:00:00.000Z hit a\n\
             2026-07-19T10:00:00.000Z hit b\n\
             2026-07-19T10:00:00.001Z tail\n",
        );
        let tl = Timeline::build(&doc, &[vec![0, 1]], 16);
        let timestamp = doc.ts_at(0);
        let bins = tl.resolve_filter_bins(0, timestamp, timestamp, 1);
        assert_eq!(bins[0].count, 2);
        assert_eq!(bins[0].first_x, timestamp);
        assert_eq!(bins[0].last_x, timestamp);
    }

    #[test]
    fn out_of_order_timestamps_use_sorted_resolution_fallback() {
        let doc = doc_with(
            "2026-07-19T10:00:02.000Z late\n\
             2026-07-19T10:00:00.000Z early\n\
             2026-07-19T10:00:01.000Z middle\n",
        );
        let tl = Timeline::build(&doc, &[vec![0, 1, 2]], 16);
        assert!(!tl.timestamps_monotonic);
        assert_eq!(tl.out_of_order_density_points.len(), 3);
        let (start, end) = doc.time_range.unwrap();
        assert_eq!(
            tl.resolve_density_bins(&doc, start, end, 3)
                .iter()
                .sum::<u32>(),
            3
        );
        assert_eq!(tl.point_count_in_range(0, start, end), 3);
        assert_eq!(tl.nearest_match_line_in_filter(0, start), Some(1));
    }

    #[test]
    fn sequence_resolution_counts_exact_lines() {
        let doc = doc_with("a\nb\nc\nd\ne\n");
        let tl = Timeline::build(&doc, &[vec![0, 4]], 16);
        let bins = tl.resolve_density_bins(&doc, 0, 2, 8);
        assert_eq!(bins.iter().sum::<u32>(), 3);
    }
}
