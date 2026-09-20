//! Multi-filter search powered by a single Aho-Corasick automaton:
//! every filter is matched in one pass over the file.  The legacy helpers below
//! keep the original case-sensitive phrase semantics; [`scan_advanced`] adds
//! per-filter case, polarity, and regex controls for the GUI.

use std::sync::atomic::{AtomicBool, Ordering};

use aho_corasick::AhoCorasick;
use regex::{Regex, RegexBuilder};

use crate::core::document::LogDocument;
use crate::core::field_query::{CompiledFieldQuery, FieldQuery};

/// Hard caps keep user supplied regular expressions predictable. Rust's regex
/// engine is linear-time (no catastrophic backtracking); these caps also bound
/// compilation and DFA memory.
pub const MAX_REGEX_PATTERN_BYTES: usize = 4_096;
const REGEX_SIZE_LIMIT: usize = 1 << 20;
const REGEX_DFA_SIZE_LIMIT: usize = 1 << 20;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FilterPolarity {
    #[default]
    Include,
    Exclude,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FilterJoin {
    #[default]
    Any,
    All,
}

/// A filter as configured by the advanced filter UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterSpec {
    pub text: String,
    pub case_sensitive: bool,
    pub polarity: FilterPolarity,
    pub regex: bool,
    /// When present, select mined Drain template IDs instead of matching text.
    /// `text` remains the human-readable timeline lane label.
    pub template_id: Option<u32>,
    /// Typed capture query; ordinary text and Template-ID modes leave this empty.
    pub field_query: Option<FieldQuery>,
}

impl FilterSpec {
    pub fn phrase(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            case_sensitive: true,
            polarity: FilterPolarity::Include,
            regex: false,
            template_id: None,
            field_query: None,
        }
    }

    /// Whether two visual filter specifications select the same physical
    /// lines. Polarity is deliberately excluded because include/exclude only
    /// changes lane composition after the scan.
    pub fn has_same_matcher(&self, other: &Self) -> bool {
        self.text == other.text
            && self.case_sensitive == other.case_sensitive
            && self.regex == other.regex
            && self.template_id == other.template_id
            && self.field_query == other.field_query
    }
}

/// Parse the Template ID spellings accepted by the UI. Keeping this
/// beside the matcher prevents UI paths from disagreeing about what an ID is.
pub fn parse_template_id(input: &str) -> Result<u32, String> {
    let input = input.trim();
    let digits = input
        .strip_prefix("T{")
        .and_then(|rest| rest.strip_suffix('}'))
        .or_else(|| input.strip_prefix('T'))
        .unwrap_or(input);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Template ID must be digits, Tdigits, or T{digits}.".to_string());
    }
    digits
        .parse::<u32>()
        .map_err(|_| "Template ID is outside the supported range.".to_string())
}

/// Line counts made available to a UI while a long scan is running.
#[derive(Default)]
pub struct ScanProgress {
    pub scanned_lines: std::sync::atomic::AtomicUsize,
    pub total_lines: std::sync::atomic::AtomicUsize,
}

/// Validate a regex before it is accepted into a filter. The Rust regex engine
/// deliberately has no backtracking, so cancellation between lines is the
/// relevant scan-time safety boundary.
pub fn validate_regex(pattern: &str, case_sensitive: bool) -> Result<(), String> {
    if pattern.len() > MAX_REGEX_PATTERN_BYTES {
        return Err(format!(
            "regex is limited to {MAX_REGEX_PATTERN_BYTES} bytes"
        ));
    }
    RegexBuilder::new(pattern)
        .case_insensitive(!case_sensitive)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_DFA_SIZE_LIMIT)
        .build()
        .map(|_| ())
        .map_err(|err| err.to_string())
}

/// Validate the parts of a matcher that are entered as text. Template IDs are
/// already normalized into `FilterSpec::template_id` by the UI, while phrase
/// matching needs no compilation-time validation.
pub fn validate_matcher(spec: &FilterSpec) -> Result<(), String> {
    if spec.field_query.is_some() {
        return Ok(());
    }
    if spec.template_id.is_some() || !spec.regex {
        return Ok(());
    }
    validate_regex(spec.text.trim(), spec.case_sensitive)
}

enum CompiledFilter {
    Phrase(AhoCorasick),
    Regex(Regex),
}

impl CompiledFilter {
    fn is_match(&self, line: &str) -> bool {
        match self {
            Self::Phrase(ac) => ac.is_match(line),
            Self::Regex(regex) => regex.is_match(line),
        }
    }
}

/// Compiled per-filter matchers for paint-time highlighting. Unlike the older
/// shared phrase automaton, this preserves each filter's case and regex mode.
pub struct FilterHighlighter {
    matchers: Vec<Option<CompiledFilter>>,
}

impl FilterHighlighter {
    /// Return `(filter_index, byte_range)` spans for every matching filter.
    pub fn spans(&self, text: &str) -> Vec<(usize, std::ops::Range<usize>)> {
        let mut spans = Vec::new();
        for (index, matcher) in self.matchers.iter().enumerate() {
            match matcher {
                Some(CompiledFilter::Phrase(ac)) => {
                    spans.extend(ac.find_iter(text).map(|m| (index, m.start()..m.end())))
                }
                Some(CompiledFilter::Regex(regex)) => {
                    spans.extend(regex.find_iter(text).map(|m| (index, m.start()..m.end())))
                }
                None => {}
            }
        }
        spans
    }
}

pub fn build_filter_highlighter(filters: &[FilterSpec]) -> Result<FilterHighlighter, String> {
    Ok(FilterHighlighter {
        matchers: filters
            .iter()
            .map(compile_filter)
            .collect::<Result<_, _>>()?,
    })
}

fn compile_filter(spec: &FilterSpec) -> Result<Option<CompiledFilter>, String> {
    if spec.template_id.is_some() || spec.field_query.is_some() {
        return Ok(None);
    }
    let text = spec.text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    if spec.regex {
        if text.len() > MAX_REGEX_PATTERN_BYTES {
            return Err(format!(
                "regex is limited to {MAX_REGEX_PATTERN_BYTES} bytes"
            ));
        }
        return RegexBuilder::new(text)
            .case_insensitive(!spec.case_sensitive)
            .size_limit(REGEX_SIZE_LIMIT)
            .dfa_size_limit(REGEX_DFA_SIZE_LIMIT)
            .build()
            .map(CompiledFilter::Regex)
            .map(Some)
            .map_err(|err| err.to_string());
    }
    AhoCorasick::builder()
        .ascii_case_insensitive(!spec.case_sensitive)
        .build([text])
        .map(CompiledFilter::Phrase)
        .map(Some)
        .map_err(|err| err.to_string())
}

/// Scan each advanced filter independently. The result has one sorted line
/// list per spec, including excluded filters so their timeline lane remains
/// useful. Invalid regexes fail before the scan begins.
pub fn scan_advanced(
    doc: &LogDocument,
    filters: &[FilterSpec],
    cancel: &AtomicBool,
    progress: Option<&ScanProgress>,
) -> Result<Vec<Vec<u32>>, String> {
    scan_advanced_range(doc, filters, 0..doc.total_lines(), cancel, progress)
}

/// Scan only a trim-relative line range. Live append uses this to extend
/// completed lanes without revisiting the existing document prefix.
pub fn scan_advanced_range(
    doc: &LogDocument,
    filters: &[FilterSpec],
    range: std::ops::Range<usize>,
    cancel: &AtomicBool,
    progress: Option<&ScanProgress>,
) -> Result<Vec<Vec<u32>>, String> {
    let compiled: Vec<Option<CompiledFilter>> = filters
        .iter()
        .map(compile_filter)
        .collect::<Result<_, _>>()?;
    let field_matchers: Vec<Option<CompiledFieldQuery>> = filters
        .iter()
        .map(|spec| {
            spec.field_query
                .as_ref()
                .map(|query| query.compile(doc))
                .transpose()
        })
        .collect::<Result<_, _>>()?;
    let has_field_matcher = field_matchers.iter().any(Option::is_some);
    let mut cached_owner: Option<usize> = None;
    let mut cached_field_hits = vec![false; filters.len()];
    let mut out = vec![Vec::new(); filters.len()];
    let start = range.start.min(doc.total_lines());
    let end = range.end.min(doc.total_lines()).max(start);
    let total = end - start;
    if let Some(progress) = progress {
        progress.total_lines.store(total, Ordering::Relaxed);
        progress.scanned_lines.store(0, Ordering::Relaxed);
    }
    for (processed, line_index) in (start..end).enumerate() {
        if processed % 1_024 == 0 {
            if cancel.load(Ordering::Relaxed) {
                return Ok(out);
            }
            if let Some(progress) = progress {
                progress.scanned_lines.store(processed, Ordering::Relaxed);
            }
        }
        if has_field_matcher {
            let original = doc.trim_start + line_index;
            let owner = doc
                .record_range_containing(original)
                .map(|range| range.start);
            if owner != cached_owner || processed == 0 {
                cached_owner = owner;
                for (slot, matcher) in cached_field_hits.iter_mut().zip(&field_matchers) {
                    *slot = matcher
                        .as_ref()
                        .map_or(Ok(false), |query| query.matches(doc, original))?;
                }
            }
        }
        let line = doc.line(line_index);
        for (filter_index, matcher) in compiled.iter().enumerate() {
            let template_matches = filters[filter_index]
                .template_id
                .is_some_and(|template_id| doc.template_at(line_index) == template_id);
            if template_matches
                || cached_field_hits[filter_index]
                || matcher
                    .as_ref()
                    .is_some_and(|matcher| matcher.is_match(line.as_ref()))
            {
                out[filter_index].push(line_index as u32);
            }
        }
    }
    if let Some(progress) = progress {
        progress.scanned_lines.store(total, Ordering::Relaxed);
    }
    Ok(out)
}

/// Combine already-scanned filter lanes into a visible set. Include filters
/// use `join`; every exclude filter is subtracted afterwards. Empty include
/// sets start from all lines, making exclusion-only filters useful.
pub fn combine_filter_matches(
    line_count: usize,
    filters: &[FilterSpec],
    matches: &[Vec<u32>],
    join: FilterJoin,
) -> Vec<usize> {
    let mut visible = vec![false; line_count];
    let includes: Vec<usize> = filters
        .iter()
        .enumerate()
        .filter_map(|(i, filter)| (filter.polarity == FilterPolarity::Include).then_some(i))
        .collect();
    if includes.is_empty() {
        visible.fill(true);
    } else if join == FilterJoin::All {
        visible.fill(true);
        for &i in &includes {
            let mut lane = vec![false; line_count];
            for &line in matches.get(i).into_iter().flatten() {
                if (line as usize) < line_count {
                    lane[line as usize] = true;
                }
            }
            for (result, hit) in visible.iter_mut().zip(lane) {
                *result &= hit;
            }
        }
    } else {
        for &i in &includes {
            for &line in matches.get(i).into_iter().flatten() {
                if (line as usize) < line_count {
                    visible[line as usize] = true;
                }
            }
        }
    }
    for (i, filter) in filters.iter().enumerate() {
        if filter.polarity == FilterPolarity::Exclude {
            for &line in matches.get(i).into_iter().flatten() {
                if (line as usize) < line_count {
                    visible[line as usize] = false;
                }
            }
        }
    }
    visible
        .into_iter()
        .enumerate()
        .filter_map(|(i, yes)| yes.then_some(i))
        .collect()
}

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

/// Compact GUI variant of [`find_lines`]. Both the optional filtered subset
/// and returned matches stay 32-bit throughout the background scan.
pub fn find_lines_u32(
    doc: &LogDocument,
    subset: Option<&[u32]>,
    needle: &str,
    case_insensitive: bool,
    cancel: &AtomicBool,
) -> Vec<u32> {
    let mut out = Vec::new();
    let Some(ac) = build_find_automaton(needle, case_insensitive) else {
        return out;
    };
    let n = doc.total_lines();
    match subset {
        Some(lines) => {
            for (step, &line) in lines.iter().enumerate() {
                if step % 16_384 == 0 && cancel.load(Ordering::Relaxed) {
                    return out;
                }
                let index = line as usize;
                if index < n && ac.is_match(doc.line(index).as_ref()) {
                    out.push(line);
                }
            }
        }
        None => {
            for index in 0..n {
                if index % 16_384 == 0 && cancel.load(Ordering::Relaxed) {
                    return out;
                }
                if ac.is_match(doc.line(index).as_ref()) {
                    out.push(index as u32);
                }
            }
        }
    }
    out
}

/// Search one advanced matcher over a whole document or its current visible
/// subset. This deliberately uses the same compiler and Template ID branch as
/// timeline filters, so a matcher can move between the two UIs unchanged.
pub fn find_advanced_u32(
    doc: &LogDocument,
    subset: Option<&[u32]>,
    spec: &FilterSpec,
    cancel: &AtomicBool,
) -> Result<Vec<u32>, String> {
    validate_matcher(spec)?;
    let matcher = compile_filter(spec)?;
    let field_matcher = spec
        .field_query
        .as_ref()
        .map(|query| query.compile(doc))
        .transpose()?;
    let mut cached_owner = None;
    let mut cached_field_hit = false;
    let mut out = Vec::new();
    let n = doc.total_lines();
    let mut visit = |line: u32, step: usize| -> Result<bool, String> {
        if step % 16_384 == 0 && cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let index = line as usize;
        let field_matches = if index < n && field_matcher.is_some() {
            let original = doc.trim_start + index;
            let owner = doc
                .record_range_containing(original)
                .map(|range| range.start);
            if owner != cached_owner || step == 0 {
                cached_owner = owner;
                cached_field_hit = field_matcher
                    .as_ref()
                    .map_or(Ok(false), |query| query.matches(doc, original))?;
            }
            cached_field_hit
        } else {
            false
        };
        if index < n
            && (spec
                .template_id
                .is_some_and(|id| doc.template_at(index) == id)
                || field_matches
                || matcher
                    .as_ref()
                    .is_some_and(|matcher| matcher.is_match(doc.line(index).as_ref())))
        {
            out.push(line);
        }
        Ok(true)
    };
    match subset {
        Some(lines) => {
            for (step, &line) in lines.iter().enumerate() {
                if !visit(line, step)? {
                    break;
                }
            }
        }
        None => {
            for index in 0..n {
                if !visit(index as u32, index)? {
                    break;
                }
            }
        }
    }
    Ok(out)
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
            "haystack_search_test_{}_{}.log",
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
    fn advanced_filters_support_case_polarity_and_join() {
        let (doc, path) = doc_with("ERROR api timeout\nerror api ok\nINFO api timeout\n");
        let filters = vec![
            FilterSpec {
                text: "error".into(),
                case_sensitive: false,
                polarity: FilterPolarity::Include,
                regex: false,
                template_id: None,
                field_query: None,
            },
            FilterSpec {
                text: "timeout".into(),
                case_sensitive: true,
                polarity: FilterPolarity::Include,
                regex: false,
                template_id: None,
                field_query: None,
            },
            FilterSpec {
                text: "INFO".into(),
                case_sensitive: true,
                polarity: FilterPolarity::Exclude,
                regex: false,
                template_id: None,
                field_query: None,
            },
        ];
        let matches = scan_advanced(&doc, &filters, &AtomicBool::new(false), None).unwrap();
        assert_eq!(matches, vec![vec![0, 1], vec![0, 2], vec![2]]);
        assert_eq!(
            combine_filter_matches(3, &filters, &matches, FilterJoin::Any),
            vec![0, 1]
        );
        assert_eq!(
            combine_filter_matches(3, &filters, &matches, FilterJoin::All),
            vec![0]
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn advanced_regex_is_case_configurable_and_bounded() {
        let (doc, path) = doc_with("WARN 42\nwarn 7\nINFO 8\n");
        let filters = vec![FilterSpec {
            text: r"warn \d+".into(),
            case_sensitive: false,
            polarity: FilterPolarity::Include,
            regex: true,
            template_id: None,
            field_query: None,
        }];
        let matches = scan_advanced(&doc, &filters, &AtomicBool::new(false), None).unwrap();
        assert_eq!(matches, vec![vec![0, 1]]);
        assert!(validate_regex("(", true).is_err());
        assert!(validate_regex(&"x".repeat(MAX_REGEX_PATTERN_BYTES + 1), true).is_err());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn exclusion_only_starts_with_the_whole_document() {
        let filters = vec![FilterSpec {
            text: "skip".into(),
            case_sensitive: true,
            polarity: FilterPolarity::Exclude,
            regex: false,
            template_id: None,
            field_query: None,
        }];
        assert_eq!(
            combine_filter_matches(4, &filters, &[vec![1, 3]], FilterJoin::Any),
            vec![0, 2]
        );
    }

    #[test]
    fn advanced_scan_reports_completed_progress() {
        let (doc, path) = doc_with("one\ntwo\nthree\n");
        let progress = ScanProgress::default();
        let filters = vec![FilterSpec::phrase("two")];
        let matches =
            scan_advanced(&doc, &filters, &AtomicBool::new(false), Some(&progress)).unwrap();
        assert_eq!(matches, vec![vec![1]]);
        assert_eq!(progress.total_lines.load(Ordering::Relaxed), 3);
        assert_eq!(progress.scanned_lines.load(Ordering::Relaxed), 3);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn advanced_range_scan_returns_original_line_ids() {
        let (doc, path) = doc_with("hit old\nmiss\nhit new\nnew hit\n");
        let progress = ScanProgress::default();
        let filters = vec![FilterSpec::phrase("hit")];
        let matches = scan_advanced_range(
            &doc,
            &filters,
            2..doc.total_lines(),
            &AtomicBool::new(false),
            Some(&progress),
        )
        .unwrap();
        assert_eq!(matches, vec![vec![2, 3]]);
        assert_eq!(progress.total_lines.load(Ordering::Relaxed), 2);
        assert_eq!(progress.scanned_lines.load(Ordering::Relaxed), 2);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn advanced_highlighter_finds_regex_and_case_folded_spans() {
        let filters = vec![
            FilterSpec {
                text: r"err(or)?".into(),
                case_sensitive: false,
                polarity: FilterPolarity::Include,
                regex: true,
                template_id: None,
                field_query: None,
            },
            FilterSpec {
                text: "disk".into(),
                case_sensitive: false,
                polarity: FilterPolarity::Include,
                regex: false,
                template_id: None,
                field_query: None,
            },
        ];
        let highlighter = build_filter_highlighter(&filters).unwrap();
        assert_eq!(
            highlighter.spans("ERROR DISK failure"),
            vec![(0, 0..5), (1, 6..10)]
        );
    }

    #[test]
    fn template_id_filters_match_mined_templates_not_text() {
        let (doc, path) =
            doc_with("INFO connected user=1\nINFO connected user=2\nERROR disk full\n");
        let connected = doc.template_at(0);
        assert_eq!(connected, doc.template_at(1));
        assert_ne!(connected, doc.template_at(2));
        let filters = vec![FilterSpec {
            text: format!("T{{{connected}}}"),
            case_sensitive: true,
            polarity: FilterPolarity::Include,
            regex: false,
            template_id: Some(connected),
            field_query: None,
        }];
        assert_eq!(
            scan_advanced(&doc, &filters, &AtomicBool::new(false), None).unwrap(),
            vec![vec![0, 1]],
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn template_id_input_accepts_the_forms_shown_by_the_ui() {
        assert_eq!(parse_template_id("42"), Ok(42));
        assert_eq!(parse_template_id("T42"), Ok(42));
        assert_eq!(parse_template_id(" T{42} "), Ok(42));
        assert!(parse_template_id("T").is_err());
        assert!(parse_template_id("T{4x}").is_err());
        assert!(parse_template_id("T{4294967296}").is_err());
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
        // must not (regression: haystack iOS log line).
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
    fn compact_find_matches_platform_sized_results() {
        let (doc, path) = doc_with(
            "2026-07-19T10:00:00.000Z err a\n\
             2026-07-19T10:00:01.000Z ok b\n\
             2026-07-19T10:00:02.000Z ERR c\n",
        );
        let cancel = AtomicBool::new(false);
        let subset_usize = [0usize, 2];
        let subset_u32 = [0u32, 2];
        let expected = find_lines(&doc, Some(&subset_usize), "err", true, &cancel);
        let compact = find_lines_u32(&doc, Some(&subset_u32), "err", true, &cancel);
        assert_eq!(
            compact
                .iter()
                .map(|&line| line as usize)
                .collect::<Vec<_>>(),
            expected
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn advanced_find_uses_the_same_modes_as_filter_scans() {
        let (doc, path) = doc_with(
            "INFO Error code=42\n\
             INFO error code=7\n\
             WARN other\n",
        );
        let template_id = doc.template_at(0);
        let specs = [
            FilterSpec {
                text: "Error".into(),
                case_sensitive: true,
                polarity: FilterPolarity::Include,
                regex: false,
                template_id: None,
                field_query: None,
            },
            FilterSpec {
                text: "error".into(),
                case_sensitive: false,
                polarity: FilterPolarity::Include,
                regex: false,
                template_id: None,
                field_query: None,
            },
            FilterSpec {
                text: r"code=\d+".into(),
                case_sensitive: true,
                polarity: FilterPolarity::Include,
                regex: true,
                template_id: None,
                field_query: None,
            },
            FilterSpec {
                text: format!("T{{{template_id}}}"),
                case_sensitive: true,
                polarity: FilterPolarity::Include,
                regex: false,
                template_id: Some(template_id),
                field_query: None,
            },
        ];
        let cancel = AtomicBool::new(false);
        for spec in &specs {
            let filter_matches = scan_advanced(&doc, &[spec.clone()], &cancel, None).unwrap();
            let search_matches = find_advanced_u32(&doc, None, spec, &cancel).unwrap();
            assert_eq!(search_matches, filter_matches[0]);
        }
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
