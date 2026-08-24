//! Multi-filter search powered by a single Aho-Corasick automaton:
//! every filter is matched in one pass over the file (case-sensitive, exact phrase),
//! so adding a 12th filter costs the same as adding the 1st.

use std::sync::atomic::{AtomicBool, Ordering};

use aho_corasick::AhoCorasick;

use crate::core::document::LogDocument;

/// Build the shared automaton for a filter set (also used by the GUI for
/// per-line highlighting of visible rows).
pub fn build_automaton(filters: &[String]) -> Option<AhoCorasick> {
    let pats: Vec<&str> = filters
        .iter()
        .map(|k| k.trim())
        .filter(|k| !k.is_empty())
        .collect();
    if pats.is_empty() {
        return None;
    }
    AhoCorasick::builder().build(pats).ok()
}

/// Scan the whole document for all filters in a single pass.
/// Returns one sorted, deduplicated list of line indices per filter.
/// Check `cancel` periodically; bailing early returns partial results.
pub fn scan_document(
    doc: &LogDocument,
    filters: &[String],
    cancel: &AtomicBool,
) -> Vec<Vec<usize>> {
    let mut out = vec![Vec::new(); filters.len()];
    let Some(ac) = build_automaton(filters) else {
        return out;
    };
    let mut hit = vec![false; filters.len()];
    let mut touched: Vec<usize> = Vec::with_capacity(filters.len());
    let n = doc.total_lines();
    for i in 0..n {
        if i % 16_384 == 0 && cancel.load(Ordering::Relaxed) {
            return out;
        }
        let line = doc.line(i);
        for m in ac.find_iter(line.as_ref()) {
            let p = m.pattern().as_usize();
            if !hit[p] {
                hit[p] = true;
                touched.push(p);
            }
        }
        for &p in &touched {
            hit[p] = false;
            out[p].push(i);
        }
        touched.clear();
    }
    out
}

/// GUI-oriented variant of [`scan_document`] that stores line indexes as
/// `u32`. A log with more than four billion addressable lines cannot be held
/// by this application in practice, while this halves the memory of dense
/// multi-filter results on 64-bit platforms.
pub fn scan_document_u32(
    doc: &LogDocument,
    filters: &[String],
    cancel: &AtomicBool,
) -> Vec<Vec<u32>> {
    let mut out = vec![Vec::new(); filters.len()];
    let Some(ac) = build_automaton(filters) else {
        return out;
    };
    let mut hit = vec![false; filters.len()];
    let mut touched: Vec<usize> = Vec::with_capacity(filters.len());
    for i in 0..doc.total_lines() {
        if i % 16_384 == 0 && cancel.load(Ordering::Relaxed) {
            return out;
        }
        let line = doc.line(i);
        for m in ac.find_iter(line.as_ref()) {
            let p = m.pattern().as_usize();
            if !hit[p] {
                hit[p] = true;
                touched.push(p);
            }
        }
        for &p in &touched {
            hit[p] = false;
            out[p].push(i as u32);
        }
        touched.clear();
    }
    out
}

/// Build a single-pattern automaton for the log view's find box / keyword
/// highlight. Unlike `build_automaton` (filter set, always case-sensitive) this
/// can fold ASCII case, which is what a find box is expected to do.
/// Returns `None` for an empty/whitespace-only needle.
pub fn build_find_automaton(needle: &str, case_insensitive: bool) -> Option<AhoCorasick> {
    let pat = needle.trim();
    if pat.is_empty() {
        return None;
    }
    AhoCorasick::builder()
        .ascii_case_insensitive(case_insensitive)
        .build([pat])
        .ok()
}

/// Scan for a single needle, returning the sorted line indices that contain it.
/// `subset` restricts the scan to those (already sorted, trim-relative) lines —
/// pass `None` to scan the whole document. Indices are trim-relative, matching
/// `scan_document`. `cancel` is checked periodically; bailing early returns the
/// partial result gathered so far.
pub fn find_lines(
    doc: &LogDocument,
    subset: Option<&[usize]>,
    needle: &str,
    case_insensitive: bool,
    cancel: &AtomicBool,
) -> Vec<usize> {
    let mut out = Vec::new();
    let Some(ac) = build_find_automaton(needle, case_insensitive) else {
        return out;
    };
    let n = doc.total_lines();
    match subset {
        Some(lines) => {
            for (step, &i) in lines.iter().enumerate() {
                if step % 16_384 == 0 && cancel.load(Ordering::Relaxed) {
                    return out;
                }
                if i >= n {
                    continue;
                }
                if ac.is_match(doc.line(i).as_ref()) {
                    out.push(i);
                }
            }
        }
        None => {
            for i in 0..n {
                if i % 16_384 == 0 && cancel.load(Ordering::Relaxed) {
                    return out;
                }
                if ac.is_match(doc.line(i).as_ref()) {
                    out.push(i);
                }
            }
        }
    }
    out
}

/// Count matches whose (forward-filled) timestamp falls inside [after, before].
pub fn count_in_window(
    doc: &LogDocument,
    matches: &[usize],
    after: Option<i64>,
    before: Option<i64>,
) -> usize {
    matches
        .iter()
        .filter(|&&l| {
            let t = doc.ts_at(l);
            after.map_or(true, |a| t >= a) && before.map_or(true, |b| t <= b)
        })
        .count()
}

/// (first_seen, last_seen) across matches, using forward-filled timestamps.
pub fn time_range_of(doc: &LogDocument, matches: &[usize]) -> Option<(i64, i64)> {
    let mut lo = i64::MAX;
    let mut hi = i64::MIN;
    for &l in matches {
        let t = doc.ts_at(l);
        if t < 0 {
            continue;
        }
        lo = lo.min(t);
        hi = hi.max(t);
    }
    if lo <= hi {
        Some((lo, hi))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn doc_with(content: &str) -> (LogDocument, PathBuf) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "logotomy_search_test_{}_{}.log",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        (LogDocument::open(&path).unwrap(), path)
    }

    #[test]
    fn finds_filters_case_sensitively() {
        let (doc, path) = doc_with(
            "2026-07-19T10:00:00.000Z ERROR disk full\n\
             2026-07-19T10:00:01.000Z info nothing to see\n\
             2026-07-19T10:00:02.000Z WARN error recovering\n",
        );
        let filters = vec!["error".to_string(), "INFO".to_string()];
        let m = scan_document(&doc, &filters, &AtomicBool::new(false));
        assert_eq!(m[0], vec![2]);
        assert_eq!(m[1], Vec::<usize>::new());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn dedupes_multiple_hits_on_one_line() {
        let (doc, path) = doc_with("2026-07-19T10:00:00.000Z error error error\n");
        let filters = vec!["error".to_string()];
        let m = scan_document(&doc, &filters, &AtomicBool::new(false));
        assert_eq!(m[0], vec![0]);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn compact_scan_matches_usize_scan() {
        let (doc, path) = doc_with("alpha\nbeta alpha\ngamma\n");
        let filters = vec!["alpha".to_string(), "beta".to_string()];
        let compact = scan_document_u32(&doc, &filters, &AtomicBool::new(false));
        assert_eq!(compact, vec![vec![0, 1], vec![1]]);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn counts_within_time_window() {
        let (doc, path) = doc_with(
            "2026-07-19T10:00:00.000Z err a\n\
             2026-07-19T10:05:00.000Z err b\n\
             2026-07-19T10:10:00.000Z err c\n",
        );
        let filters = vec!["err".to_string()];
        let m = scan_document(&doc, &filters, &AtomicBool::new(false));
        let mid = doc.ts_at(1);
        assert_eq!(count_in_window(&doc, &m[0], None, None), 3);
        assert_eq!(count_in_window(&doc, &m[0], Some(mid), None), 2);
        assert_eq!(count_in_window(&doc, &m[0], None, Some(mid)), 2);
        assert_eq!(count_in_window(&doc, &m[0], Some(mid + 1), None), 1);
        let (lo, hi) = time_range_of(&doc, &m[0]).unwrap();
        assert_eq!(lo, doc.ts_at(0));
        assert_eq!(hi, doc.ts_at(2));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn matches_exact_case_sensitive_phrase() {
        // Multi-word exact phrase must match verbatim; a case-different variant
        // must not (regression: logotomy iOS log line).
        let (doc, path) = doc_with(
            "2026-07-15 22:00:02.107175+0300 MyApp[12345:13] <WARNING> AnalyticsTracker.swift:136 CoreData fetch exceeded threshold entity=LogEntry count=15000\n",
        );
        let exact = vec!["<WARNING> AnalyticsTracker.swift:136".to_string()];
        let m = scan_document(&doc, &exact, &AtomicBool::new(false));
        assert_eq!(m[0], vec![0]);

        let lower = vec!["<warning> AnalyticsTracker.swift:136".to_string()];
        let m2 = scan_document(&doc, &lower, &AtomicBool::new(false));
        assert_eq!(m2[0], Vec::<usize>::new());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn find_lines_folds_case_when_asked() {
        let (doc, path) = doc_with(
            "2026-07-19T10:00:00.000Z ERROR disk full\n\
             2026-07-19T10:00:01.000Z info nothing to see\n\
             2026-07-19T10:00:02.000Z WARN error recovering\n",
        );
        let cancel = AtomicBool::new(false);
        // Case-insensitive: both the upper- and lower-case spellings match.
        assert_eq!(find_lines(&doc, None, "error", true, &cancel), vec![0, 2]);
        // Case-sensitive: only the exact spelling.
        assert_eq!(find_lines(&doc, None, "error", false, &cancel), vec![2]);
        assert_eq!(find_lines(&doc, None, "ERROR", false, &cancel), vec![0]);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn find_lines_restricts_to_the_subset() {
        let (doc, path) = doc_with(
            "2026-07-19T10:00:00.000Z err a\n\
             2026-07-19T10:00:01.000Z err b\n\
             2026-07-19T10:00:02.000Z err c\n\
             2026-07-19T10:00:03.000Z ok  d\n",
        );
        let cancel = AtomicBool::new(false);
        assert_eq!(find_lines(&doc, None, "err", true, &cancel), vec![0, 1, 2]);
        // Line 1 is filtered out of the view, so it must not be reported.
        let subset = [0usize, 2, 3];
        assert_eq!(
            find_lines(&doc, Some(&subset), "err", true, &cancel),
            vec![0, 2]
        );
        // Out-of-range subset entries are skipped, not panicked on.
        let stale = [0usize, 999];
        assert_eq!(
            find_lines(&doc, Some(&stale), "err", true, &cancel),
            vec![0]
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn find_lines_returns_empty_for_a_blank_needle() {
        let (doc, path) = doc_with("2026-07-19T10:00:00.000Z err a\n");
        let cancel = AtomicBool::new(false);
        assert!(find_lines(&doc, None, "", true, &cancel).is_empty());
        assert!(find_lines(&doc, None, "   ", true, &cancel).is_empty());
        assert!(build_find_automaton("", true).is_none());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn find_lines_bails_out_when_cancelled() {
        let (doc, path) = doc_with(
            "2026-07-19T10:00:00.000Z err a\n\
             2026-07-19T10:00:01.000Z err b\n",
        );
        let cancel = AtomicBool::new(true);
        assert!(find_lines(&doc, None, "err", true, &cancel).is_empty());
        std::fs::remove_file(path).ok();
    }
}
