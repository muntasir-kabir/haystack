use std::borrow::Cow;
use std::sync::Arc;

use aho_corasick::AhoCorasick;
use eframe::egui;
use egui::{Color32, FontId};

use haystack::core::document::LogDocument;
use haystack::core::embedded_data::Detection;
use haystack::core::field_query::FieldQuery;
use haystack::core::search::FilterHighlighter;
use haystack::core::settings::LogLineDisplayMode;

use crate::ui::app::model::{Filter, LogTab};
use crate::ui::theme::Theme;

/// Bytes beyond this are cut off when rendering a single row.
/// (The full bytes stay in the mmap; we just don't paint a novel per frame.)
pub const MAX_DISPLAY_BYTES: usize = 2000;

pub struct Highlights<'a> {
    pub filters: &'a [Filter],
    pub filter_matcher: Option<&'a FilterHighlighter>,
    pub search_matcher: Option<&'a FilterHighlighter>,
    pub search_template_id: Option<u32>,
    pub search_field: Option<&'a FieldQuery>,
    pub search_rows: Option<&'a [u32]>,
    pub filter_fields: Option<&'a [Option<FieldQuery>]>,
    pub filter_rows: Option<&'a [Arc<Vec<u32>>]>,
    pub keyword_ac: Option<&'a AhoCorasick>,
    pub embedded: Option<&'a [Detection]>,
}

impl<'a> Highlights<'a> {
    pub fn from_tab(tab: &'a LogTab) -> Self {
        Self {
            filters: &tab.filters,
            filter_matcher: tab.highlighter.as_deref(),
            search_matcher: tab.find_highlighter.as_deref(),
            search_template_id: tab.find_template_id,
            search_field: tab
                .find_active_spec
                .as_ref()
                .and_then(|spec| spec.field_query.as_ref()),
            search_rows: Some(&tab.find_matches),
            filter_fields: Some(&tab.filter_field_queries),
            filter_rows: Some(&tab.matches),
            keyword_ac: tab.keyword_automaton.as_deref(),
            embedded: Some(tab.embedded_detections.as_slice()),
        }
    }

    pub fn filters_only(filters: &'a [Filter], matcher: Option<&'a FilterHighlighter>) -> Self {
        Self {
            filters,
            filter_matcher: matcher,
            search_matcher: None,
            search_template_id: None,
            search_field: None,
            search_rows: None,
            filter_fields: None,
            filter_rows: None,
            keyword_ac: None,
            embedded: None,
        }
    }
}

/// Build the styled LayoutJob for one log line. This is shared by the central
/// log view and the pinned-lines preview.
pub fn line_job(
    doc: &LogDocument,
    highlights: &Highlights,
    idx: usize,
    selected: bool,
    font_id: FontId,
    theme: &Theme,
) -> egui::text::LayoutJob {
    line_job_for_mode(
        doc,
        highlights,
        idx,
        selected,
        font_id,
        theme,
        LogLineDisplayMode::Truncate,
    )
}

/// Build a line job using the requested long-line presentation. Pinned-line
/// previews keep the bounded default via [`line_job`], while Log View can
/// render complete source text in its wrap and horizontal-scroll modes.
pub fn line_job_for_mode(
    doc: &LogDocument,
    highlights: &Highlights,
    idx: usize,
    selected: bool,
    font_id: FontId,
    theme: &Theme,
    display_mode: LogLineDisplayMode,
) -> egui::text::LayoutJob {
    let bg = if selected {
        theme.selection_bg
    } else {
        Color32::TRANSPARENT
    };
    let mut job = egui::text::LayoutJob::default();
    let fmt = |color: Color32| egui::text::TextFormat {
        font_id: font_id.clone(),
        color,
        background: bg,
        ..Default::default()
    };

    let line = doc.line(idx);
    let source_is_valid_utf8 = matches!(line, Cow::Borrowed(_));
    let visible_source_len = display_source_len_for_mode(&line, display_mode);
    let text = display_text_for_mode(&line, display_mode);
    let embedded_ranges = if source_is_valid_utf8 {
        embedded_ranges_for_line(doc, highlights, idx, visible_source_len)
    } else {
        Vec::new()
    };
    let timestamp_range = source_is_valid_utf8
        .then(|| doc.explicit_timestamp_at(idx).map(|(_, range)| range))
        .flatten()
        .and_then(|range| {
            let start = range.start.min(visible_source_len);
            let end = range.end.min(visible_source_len);
            (start < end).then_some(start..end)
        });

    let original = doc.trim_start + idx;
    let field_range = |field: &str| -> Option<std::ops::Range<usize>> {
        if !source_is_valid_utf8 {
            return None;
        }
        let span = doc.record_field_span_on_line(original, field)?;
        let base = doc.line_offsets[original];
        let start = usize::try_from(span.start.checked_sub(base)?)
            .ok()?
            .min(visible_source_len);
        let end = usize::try_from(span.end.checked_sub(base)?)
            .ok()?
            .min(visible_source_len);
        (start < end).then_some(start..end)
    };
    let search_field_hit = highlights.search_field.is_some()
        && highlights
            .search_rows
            .is_some_and(|rows| rows.binary_search(&(idx as u32)).is_ok());
    let search_field_range = search_field_hit
        .then(|| {
            highlights
                .search_field
                .and_then(|query| field_range(&query.field))
        })
        .flatten();
    let mut filter_field_ranges = Vec::new();
    if let (Some(fields), Some(rows)) = (highlights.filter_fields, highlights.filter_rows) {
        for (index, query) in fields.iter().enumerate() {
            if let Some(query) = query {
                if rows
                    .get(index)
                    .is_some_and(|rows| rows.binary_search(&(idx as u32)).is_ok())
                {
                    filter_field_ranges.push((
                        index,
                        field_range(&query.field).unwrap_or(0..visible_source_len),
                    ));
                }
            }
        }
    }
    let search_full_line = highlights
        .search_template_id
        .is_some_and(|template_id| doc.template_at(idx) == template_id)
        || (search_field_hit && search_field_range.is_none());

    let base = theme.log_text;
    match (
        highlights.filter_matcher,
        highlights.search_matcher,
        highlights.keyword_ac,
    ) {
        (None, None, None)
            if !search_full_line
                && search_field_range.is_none()
                && filter_field_ranges.is_empty() =>
        {
            append_segment_with_annotations(
                &mut job,
                &text,
                0..text.len(),
                fmt(base),
                &embedded_ranges,
                timestamp_range.as_ref(),
                theme.embedded_data,
                theme.timestamp,
            )
        }
        (filter_matcher, search_matcher, keyword_ac) => append_highlighted(
            &mut job,
            &text,
            filter_matcher,
            search_matcher,
            search_full_line,
            search_field_range,
            &filter_field_ranges,
            keyword_ac,
            highlights.filters,
            fmt(base),
            font_id,
            theme,
            &embedded_ranges,
            timestamp_range.as_ref(),
        ),
    }
    job
}

/// Return the original line or a UTF-8-safe display prefix.
#[allow(dead_code)] // Retained as the concise truncate-mode helper for tests/callers.
pub fn display_text(line: &str) -> Cow<'_, str> {
    display_text_for_mode(line, LogLineDisplayMode::Truncate)
}

/// Return a UTF-8-safe preview in truncate mode or the complete source text
/// in the full-line modes.
pub fn display_text_for_mode(line: &str, display_mode: LogLineDisplayMode) -> Cow<'_, str> {
    if display_mode != LogLineDisplayMode::Truncate {
        return Cow::Borrowed(line);
    }
    if line.len() <= MAX_DISPLAY_BYTES {
        return Cow::Borrowed(line);
    }

    let mut end = MAX_DISPLAY_BYTES;
    while end > 0 && !line.is_char_boundary(end) {
        end -= 1;
    }
    Cow::Owned(format!(
        "{}  …[{} bytes total, truncated]",
        &line[..end],
        line.len()
    ))
}

pub(super) fn display_source_len(line: &str) -> usize {
    display_source_len_for_mode(line, LogLineDisplayMode::Truncate)
}

pub(super) fn display_source_len_for_mode(line: &str, display_mode: LogLineDisplayMode) -> usize {
    if display_mode != LogLineDisplayMode::Truncate {
        return line.len();
    }
    if line.len() <= MAX_DISPLAY_BYTES {
        return line.len();
    }
    let mut end = MAX_DISPLAY_BYTES;
    while end > 0 && !line.is_char_boundary(end) {
        end -= 1;
    }
    end
}

fn embedded_ranges_for_line(
    doc: &LogDocument,
    highlights: &Highlights<'_>,
    idx: usize,
    visible_source_len: usize,
) -> Vec<std::ops::Range<usize>> {
    let original_line = doc.trim_start + idx;
    let mut ranges = Vec::new();
    for detection in highlights.embedded.into_iter().flatten() {
        ranges.extend(source_ranges_for_line(
            detection,
            original_line,
            visible_source_len,
        ));
    }
    ranges
}

pub(super) fn source_ranges_for_line(
    detection: &Detection,
    original_line: usize,
    visible_source_len: usize,
) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    for span in &detection.source_spans {
        if !span.includes_line(original_line) {
            continue;
        }
        let start = if span.start.line == original_line {
            span.start.byte
        } else {
            0
        };
        let end = if span.end.line == original_line {
            span.end.byte
        } else {
            visible_source_len
        };
        let start = start.min(visible_source_len);
        let end = end.min(visible_source_len);
        if start < end {
            ranges.push(start..end);
        }
    }
    ranges
}

#[derive(Clone, Copy, PartialEq)]
enum HighlightKind {
    Filter(usize),
    Search,
    Keyword,
}

/// Append text with precedence search > keyword > filter.
fn append_highlighted(
    job: &mut egui::text::LayoutJob,
    text: &str,
    filter_matcher: Option<&FilterHighlighter>,
    search_matcher: Option<&FilterHighlighter>,
    search_full_line: bool,
    search_field_range: Option<std::ops::Range<usize>>,
    filter_field_ranges: &[(usize, std::ops::Range<usize>)],
    keyword_ac: Option<&AhoCorasick>,
    filters: &[Filter],
    base_fmt: egui::text::TextFormat,
    font_id: FontId,
    theme: &Theme,
    embedded_ranges: &[std::ops::Range<usize>],
    timestamp_range: Option<&std::ops::Range<usize>>,
) {
    let mut spans: Vec<(std::ops::Range<usize>, HighlightKind)> = Vec::new();
    let mut covered: Vec<std::ops::Range<usize>> = Vec::new();

    if search_full_line && !text.is_empty() {
        add_highlight_span(
            &mut spans,
            &mut covered,
            0..text.len(),
            HighlightKind::Search,
        );
    } else if let Some(range) = search_field_range {
        add_highlight_span(&mut spans, &mut covered, range, HighlightKind::Search);
    } else if let Some(matcher) = search_matcher {
        for (_, range) in matcher.spans(text) {
            add_highlight_span(&mut spans, &mut covered, range, HighlightKind::Search);
        }
    }
    if let Some(ac) = keyword_ac {
        for m in ac.find_iter(text) {
            add_highlight_span(
                &mut spans,
                &mut covered,
                m.start()..m.end(),
                HighlightKind::Keyword,
            );
        }
    }
    if let Some(matcher) = filter_matcher {
        for (filter, range) in matcher.spans(text) {
            add_highlight_span(
                &mut spans,
                &mut covered,
                range,
                HighlightKind::Filter(filter),
            );
        }
    }
    for (filter, range) in filter_field_ranges {
        add_highlight_span(
            &mut spans,
            &mut covered,
            range.clone(),
            HighlightKind::Filter(*filter),
        );
    }

    if spans.is_empty() {
        append_segment_with_annotations(
            job,
            text,
            0..text.len(),
            base_fmt,
            embedded_ranges,
            timestamp_range,
            theme.embedded_data,
            theme.timestamp,
        );
        return;
    }

    spans.sort_by_key(|(range, _)| range.start);
    let mut pos = 0;
    for (range, highlight) in spans {
        if pos < range.start {
            append_segment_with_annotations(
                job,
                text,
                pos..range.start,
                base_fmt.clone(),
                embedded_ranges,
                timestamp_range,
                theme.embedded_data,
                theme.timestamp,
            );
        }
        let highlight_fmt = match highlight {
            HighlightKind::Filter(filter) => {
                let color = filters
                    .get(filter)
                    .map(|item| item.color)
                    .unwrap_or(Color32::YELLOW);
                let background =
                    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 51);
                egui::text::TextFormat {
                    font_id: font_id.clone(),
                    // Filter colours identify the lane; they are not used as
                    // foreground text colours because some categorical hues
                    // do not provide normal-text contrast on light surfaces.
                    color: base_fmt.color,
                    background: preserve_selection_background(background, base_fmt.background),
                    ..Default::default()
                }
            }
            HighlightKind::Search => egui::text::TextFormat {
                font_id: font_id.clone(),
                color: base_fmt.color,
                background: preserve_selection_background(
                    theme.search_highlight_bg,
                    base_fmt.background,
                ),
                ..Default::default()
            },
            HighlightKind::Keyword => egui::text::TextFormat {
                font_id: font_id.clone(),
                color: base_fmt.color,
                background: preserve_selection_background(
                    theme.keyword_highlight_bg,
                    base_fmt.background,
                ),
                ..Default::default()
            },
        };
        append_segment_with_annotations(
            job,
            text,
            range.clone(),
            highlight_fmt,
            embedded_ranges,
            timestamp_range,
            theme.embedded_data,
            theme.timestamp,
        );
        pos = range.end;
    }
    if pos < text.len() {
        append_segment_with_annotations(
            job,
            text,
            pos..text.len(),
            base_fmt,
            embedded_ranges,
            timestamp_range,
            theme.embedded_data,
            theme.timestamp,
        );
    }
}

/// Blend a translucent local cue over the selected-row fill so a selected row
/// remains visibly selected even where a search/filter span is painted.
fn preserve_selection_background(highlight: Color32, selection: Color32) -> Color32 {
    if selection == Color32::TRANSPARENT || highlight == Color32::TRANSPARENT {
        return if highlight == Color32::TRANSPARENT {
            selection
        } else {
            highlight
        };
    }
    let alpha = f32::from(highlight.a()) / 255.0;
    let inverse = 1.0 - alpha;
    Color32::from_rgba_unmultiplied(
        (f32::from(highlight.r()) * alpha + f32::from(selection.r()) * inverse).round() as u8,
        (f32::from(highlight.g()) * alpha + f32::from(selection.g()) * inverse).round() as u8,
        (f32::from(highlight.b()) * alpha + f32::from(selection.b()) * inverse).round() as u8,
        255,
    )
}

fn append_segment_with_annotations(
    job: &mut egui::text::LayoutJob,
    text: &str,
    range: std::ops::Range<usize>,
    format: egui::text::TextFormat,
    embedded_ranges: &[std::ops::Range<usize>],
    timestamp_range: Option<&std::ops::Range<usize>>,
    _embedded_color: Color32,
    _timestamp_color: Color32,
) {
    if range.is_empty() {
        return;
    }
    let mut boundaries = vec![range.start, range.end];
    for embedded in embedded_ranges {
        if embedded.end > range.start && embedded.start < range.end {
            boundaries.push(embedded.start.max(range.start));
            boundaries.push(embedded.end.min(range.end));
        }
    }
    if let Some(timestamp) = timestamp_range {
        if timestamp.end > range.start && timestamp.start < range.end {
            boundaries.push(timestamp.start.max(range.start));
            boundaries.push(timestamp.end.min(range.end));
        }
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    for pair in boundaries.windows(2) {
        let segment = pair[0]..pair[1];
        if segment.is_empty() {
            continue;
        }
        let segment_format = format.clone();
        // Inspectable source values use a local dotted cue painted by the row
        // renderer. Keeping that cue out of every text span avoids persistent
        // timestamp and payload underlines while preserving a keyboard/context
        // menu path and a stronger hover state.
        job.append(&text[segment], 0.0, segment_format);
    }
}

fn add_highlight_span(
    spans: &mut Vec<(std::ops::Range<usize>, HighlightKind)>,
    covered: &mut Vec<std::ops::Range<usize>>,
    range: std::ops::Range<usize>,
    highlight: HighlightKind,
) {
    let mut cursor = range.start;
    for existing in covered.iter() {
        if existing.end <= cursor {
            continue;
        }
        if existing.start >= range.end {
            break;
        }
        if cursor < existing.start {
            spans.push((cursor..existing.start.min(range.end), highlight));
        }
        cursor = cursor.max(existing.end);
        if cursor >= range.end {
            break;
        }
    }
    if cursor < range.end {
        spans.push((cursor..range.end, highlight));
    }

    let mut merged = range;
    let first = covered.partition_point(|existing| existing.end < merged.start);
    let mut last = first;
    while last < covered.len() && covered[last].start <= merged.end {
        merged.start = merged.start.min(covered[last].start);
        merged.end = merged.end.max(covered[last].end);
        last += 1;
    }
    covered.splice(first..last, std::iter::once(merged));
}
