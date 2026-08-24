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
    /// Per-filter (line_idx, x-value) points, sorted by line index.
    /// x-value is epoch ms in Time domain, line index in Sequence domain.
    pub filter_points: Vec<Vec<(u32, i64)>>,
    pub max_density: u32,
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
        for i in 0..n_lines {
            let v = x_of_line(i);
            if v < 0 {
                continue; // before first known timestamp
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
            filter_buckets.push(kb);
            filter_points.push(pts);
        }

        Timeline {
            domain,
            n_buckets: nb,
            density,
            filter_buckets,
            filter_points,
            max_density,
        }
    }

    /// X-value at the center of a bucket (epoch ms or line index).
    pub fn bucket_center(&self, idx: usize) -> i64 {
        match self.domain {
            TimelineDomain::Time { start_ms, end_ms } => {
                let span = (end_ms - start_ms).max(1);
                start_ms + span * idx as i64 / (self.n_buckets as i64 - 1).max(1)
            }
            TimelineDomain::Sequence => idx as i64,
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
                if best.map_or(true, |(_, bd)| d < bd) {
                    best = Some((line, d));
                }
            }
            // Check the point just before the insertion position (last < v).
            if idx > 0 {
                let (line, x) = pts[idx - 1];
                let d = (x - v).abs();
                if best.map_or(true, |(_, bd)| d < bd) {
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
}
