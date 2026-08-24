use std::borrow::Cow;

use aho_corasick::AhoCorasick;
use eframe::egui;
use egui::{Color32, FontId, Stroke};

use logotomy::core::document::LogDocument;
use logotomy::core::embedded_data::Detection;

use crate::ui::app::model::{Filter, LogTab};
use crate::ui::theme::Theme;

/// Bytes beyond this are cut off when rendering a single row.
/// (The full bytes stay in the mmap; we just don't paint a novel per frame.)
pub const MAX_DISPLAY_BYTES: usize = 2000;

pub struct Highlights<'a> {
    pub filters: &'a [Filter],
    pub filter_ac: Option<&'a AhoCorasick>,
    pub search_ac: Option<&'a AhoCorasick>,
    pub keyword_ac: Option<&'a AhoCorasick>,
    pub embedded: Option<&'a [Detection]>,
}

impl<'a> Highlights<'a> {
    pub fn from_tab(tab: &'a LogTab) -> Self {
        Self {
            filters: &tab.filters,
            filter_ac: tab.highlighter.as_deref(),
            search_ac: tab.find_automaton.as_deref(),
            keyword_ac: tab.keyword_automaton.as_deref(),
            embedded: Some(tab.embedded_detections.as_slice()),
        }
    }

    pub fn filters_only(filters: &'a [Filter], ac: Option<&'a AhoCorasick>) -> Self {
        Self {
            filters,
            filter_ac: ac,
            search_ac: None,
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
    let visible_source_len = display_source_len(&line);
    let text = display_text(&line);
    let embedded_ranges = if source_is_valid_utf8 {
        embedded_ranges_for_line(doc, highlights, idx, visible_source_len)
    } else {
        Vec::new()
    };

    let base = theme.log_text;
    match (
        highlights.filter_ac,
        highlights.search_ac,
        highlights.keyword_ac,
    ) {
        (None, None, None) => append_segment_with_embedded(
            &mut job,
            &text,
            0..text.len(),
            fmt(base),
            &embedded_ranges,
            theme.embedded_data,
        ),
        (filter_ac, search_ac, keyword_ac) => append_highlighted(
            &mut job,
            &text,
            filter_ac,
            search_ac,
            keyword_ac,
            highlights.filters,
            fmt(base),
            font_id,
            theme,
            &embedded_ranges,
        ),
    }
    job
}

/// Return the original line or a UTF-8-safe display prefix.
pub fn display_text(line: &str) -> Cow<'_, str> {
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

fn display_source_len(line: &str) -> usize {
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
        if !detection.span.includes_line(original_line) {
            continue;
        }
        let start = if detection.span.start.line == original_line {
            detection.span.start.byte
        } else {
            0
        };
        let end = if detection.span.end.line == original_line {
            detection.span.end.byte
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
    filter_ac: Option<&AhoCorasick>,
    search_ac: Option<&AhoCorasick>,
    keyword_ac: Option<&AhoCorasick>,
    filters: &[Filter],
    base_fmt: egui::text::TextFormat,
    font_id: FontId,
    theme: &Theme,
    embedded_ranges: &[std::ops::Range<usize>],
) {
    let mut spans: Vec<(std::ops::Range<usize>, HighlightKind)> = Vec::new();
    let mut covered: Vec<std::ops::Range<usize>> = Vec::new();

    if let Some(ac) = search_ac {
        for m in ac.find_iter(text) {
            add_highlight_span(
                &mut spans,
                &mut covered,
                m.start()..m.end(),
                HighlightKind::Search,
            );
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
    if let Some(ac) = filter_ac {
        for m in ac.find_iter(text) {
            let filter = m.pattern().as_usize();
            add_highlight_span(
                &mut spans,
                &mut covered,
                m.start()..m.end(),
                HighlightKind::Filter(filter),
            );
        }
    }

    if spans.is_empty() {
        append_segment_with_embedded(
            job,
            text,
            0..text.len(),
            base_fmt,
            embedded_ranges,
            theme.embedded_data,
        );
        return;
    }

    spans.sort_by_key(|(range, _)| range.start);
    let mut pos = 0;
    for (range, highlight) in spans {
        if pos < range.start {
            append_segment_with_embedded(
                job,
                text,
                pos..range.start,
                base_fmt.clone(),
                embedded_ranges,
                theme.embedded_data,
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
                    color,
                    background,
                    ..Default::default()
                }
            }
            HighlightKind::Search => egui::text::TextFormat {
                font_id: font_id.clone(),
                color: base_fmt.color,
                background: theme.search_highlight_bg,
                ..Default::default()
            },
            HighlightKind::Keyword => egui::text::TextFormat {
                font_id: font_id.clone(),
                color: base_fmt.color,
                background: theme.keyword_highlight_bg,
                ..Default::default()
            },
        };
        append_segment_with_embedded(
            job,
            text,
            range.clone(),
            highlight_fmt,
            embedded_ranges,
            theme.embedded_data,
        );
        pos = range.end;
    }
    if pos < text.len() {
        append_segment_with_embedded(
            job,
            text,
            pos..text.len(),
            base_fmt,
            embedded_ranges,
            theme.embedded_data,
        );
    }
}

fn append_segment_with_embedded(
    job: &mut egui::text::LayoutJob,
    text: &str,
    range: std::ops::Range<usize>,
    format: egui::text::TextFormat,
    embedded_ranges: &[std::ops::Range<usize>],
    underline_color: Color32,
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
    boundaries.sort_unstable();
    boundaries.dedup();
    for pair in boundaries.windows(2) {
        let segment = pair[0]..pair[1];
        if segment.is_empty() {
            continue;
        }
        let mut segment_format = format.clone();
        if embedded_ranges
            .iter()
            .any(|embedded| embedded.start < segment.end && embedded.end > segment.start)
        {
            segment_format.underline = Stroke::new(1.0, underline_color);
        }
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
