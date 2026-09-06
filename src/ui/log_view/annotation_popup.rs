//! Delayed source callouts for embedded data and explicit timestamps.
//!
//! This module owns the hover lifecycle separately from the row renderer. In
//! particular, a visible callout is sticky while the pointer crosses other
//! highlighted source fragments on the way to the callout.

use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Color32, Pos2, Rect, RichText, Stroke};

use logotomy::core::embedded_data::{DataNode, Detection};
use logotomy::core::time::format_ms;

use super::embedded::presentation;
use super::view::{inspector_mode_for, inspector_mode_for_open};
use crate::ui::app::model::{AnnotationHoverKey, AnnotationHoverState, LogTab};
use crate::ui::icons::{self, Icon};
use crate::ui::theme::Theme;

/// Pointer dwell before a source callout appears.
const HOVER_DELAY: Duration = Duration::from_millis(800);
/// Time the callout remains alive after the pointer leaves its active region.
const HOVER_GRACE: Duration = Duration::from_millis(420);
/// Width of the forgiving source-to-callout handoff corridor.
const HANDOFF_RADIUS: f32 = 14.0;
const HOVER_MOVE_RESET: f32 = 3.0;

const POPUP_INSET: f32 = 8.0;
/// Tune this single value when adjusting the callout's visual transparency.
const ANNOTATION_POPUP_SURFACE_ALPHA: f32 = 0.78;
const POPUP_MIN_WIDTH: f32 = 260.0;
const POPUP_MAX_WIDTH: f32 = 560.0;
const POPUP_MAX_HEIGHT: f32 = 460.0;
const POPUP_MIN_HEIGHT: f32 = 96.0;
const POPUP_HEADER_HEIGHT: f32 = 76.0;
const POPUP_ROW_HEIGHT: f32 = 18.0;
const PREVIEW_MAX_LINES: usize = 32;
const PREVIEW_MAX_LINE_CHARS: usize = 72;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Candidate {
    pub(crate) key: AnnotationHoverKey,
    pub(crate) source_rect: Rect,
}

pub(crate) fn update(
    tab: &mut LogTab,
    candidate: Option<Candidate>,
    suppress: bool,
    ctx: &egui::Context,
    now: Instant,
) {
    if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        tab.annotation_hover = None;
        return;
    }

    let (pointer_pos, pointer_delta) = ctx.input(|input| {
        (
            input.pointer.latest_pos(),
            input
                .pointer
                .motion()
                .unwrap_or_else(|| input.pointer.delta()),
        )
    });

    let pointer_over_bubble = tab.annotation_hover.as_ref().is_some_and(|state| {
        pointer_pos.is_some_and(|pointer| {
            state
                .bubble_rect
                .is_some_and(|bubble| bubble.expand(4.0).contains(pointer))
        })
    });
    let pointer_in_active_region = tab.annotation_hover.as_ref().is_some_and(|state| {
        pointer_pos.is_some_and(|pointer| {
            state.source_rect.expand(4.0).contains(pointer)
                || state
                    .bubble_rect
                    .is_some_and(|bubble| pointer_in_handoff(pointer, state.source_rect, bubble))
        })
    });

    let clicked_outside = ctx.input(|input| {
        input
            .pointer
            .any_click()
            .then(|| input.pointer.interact_pos())
            .flatten()
    });
    if clicked_outside.is_some_and(|position| {
        tab.annotation_hover
            .as_ref()
            .and_then(|state| state.bubble_rect)
            .is_some_and(|bubble| should_dismiss(false, Some(position), bubble))
    }) {
        tab.annotation_hover = None;
        return;
    }

    // Pointer-down/scroll gestures dismiss a pending source callout. A
    // visible callout remains interactive if the pointer is already over it.
    if suppress && !pointer_over_bubble {
        tab.annotation_hover = None;
        return;
    }

    // Once shown, the current callout owns the hover interaction. Other
    // underlines may report a candidate while the pointer crosses them, but
    // they must not replace the callout before the user can reach it.
    if tab
        .annotation_hover
        .as_ref()
        .is_some_and(|state| state.bubble_rect.is_some())
    {
        if pointer_in_active_region {
            if let Some(state) = tab.annotation_hover.as_mut() {
                state.last_seen_at = now;
            }
        } else if tab
            .annotation_hover
            .as_ref()
            .is_some_and(|state| now.duration_since(state.last_seen_at) > HOVER_GRACE)
        {
            // Do not immediately adopt the competing candidate in this same
            // frame. The new target gets a fresh dwell period on the next
            // frame, avoiding popup flicker while crossing dense highlights.
            tab.annotation_hover = None;
            return;
        }
        request_repaint_for_state(tab, ctx, now);
        return;
    }

    match candidate {
        Some(candidate) => match tab.annotation_hover.as_mut() {
            Some(state) if state.key == candidate.key => {
                if pointer_delta.length() > HOVER_MOVE_RESET {
                    state.started_at = now;
                }
                state.source_rect = candidate.source_rect;
                state.last_seen_at = now;
            }
            _ => {
                tab.annotation_hover = Some(AnnotationHoverState {
                    key: candidate.key,
                    source_rect: candidate.source_rect,
                    started_at: now,
                    last_seen_at: now,
                    bubble_rect: None,
                });
            }
        },
        None => {
            if pointer_in_active_region {
                if let Some(state) = tab.annotation_hover.as_mut() {
                    state.last_seen_at = now;
                }
            } else if tab
                .annotation_hover
                .as_ref()
                .is_some_and(|state| now.duration_since(state.last_seen_at) > HOVER_GRACE)
            {
                tab.annotation_hover = None;
            }
        }
    }

    request_repaint_for_state(tab, ctx, now);
}

fn request_repaint_for_state(tab: &LogTab, ctx: &egui::Context, now: Instant) {
    let Some(state) = tab.annotation_hover.as_ref() else {
        return;
    };
    if state.bubble_rect.is_none() && now.duration_since(state.started_at) < HOVER_DELAY {
        ctx.request_repaint_after(HOVER_DELAY.saturating_sub(now.duration_since(state.started_at)));
    } else {
        ctx.request_repaint_after(HOVER_GRACE);
    }
}

/// Returns true for the source, popup, or forgiving visual corridor joining
/// them. The corridor deliberately remains active when another underline is
/// underneath it.
fn pointer_in_handoff(pointer: Pos2, source: Rect, bubble: Rect) -> bool {
    if source.expand(4.0).contains(pointer) || bubble.expand(4.0).contains(pointer) {
        return true;
    }

    let bubble_is_above = bubble.center().y < source.center().y;
    let source_anchor = if bubble_is_above {
        source.center_top()
    } else {
        source.center_bottom()
    };
    let bubble_anchor_x = source_anchor
        .x
        .clamp(bubble.left() + 6.0, bubble.right() - 6.0);
    let bubble_anchor = Pos2::new(
        bubble_anchor_x,
        if bubble_is_above {
            bubble.bottom()
        } else {
            bubble.top()
        },
    );
    let segment = bubble_anchor - source_anchor;
    let length_sq = segment.length_sq();
    if length_sq <= f32::EPSILON {
        return false;
    }
    let progress = ((pointer - source_anchor).dot(segment) / length_sq).clamp(0.0, 1.0);
    pointer.distance(source_anchor + segment * progress) <= HANDOFF_RADIUS
}

pub(crate) fn show(ui: &mut egui::Ui, tab: &mut LogTab, theme: &Theme) {
    let Some(state) = tab.annotation_hover.as_ref() else {
        return;
    };
    if state.started_at.elapsed() < HOVER_DELAY {
        return;
    }

    let key = state.key;
    let source_rect = state.source_rect;
    let (badge, title, summary, lines, source, accent, detection, secondary_label, normalized) =
        match key {
            AnnotationHoverKey::Embedded { .. } => {
                let Some(detection) = tab
                    .embedded_detections
                    .iter()
                    .find(|detection| AnnotationHoverKey::for_detection(detection) == key)
                    .cloned()
                else {
                    tab.annotation_hover = None;
                    return;
                };
                let profile = presentation(detection.detector_id);
                (
                    profile.badge,
                    profile.title,
                    detection.summary(),
                    preview_lines(&detection),
                    detection.raw.clone(),
                    theme.embedded_data,
                    Some(detection),
                    "Open inspector",
                    None,
                )
            }
            AnnotationHoverKey::Timestamp { span } => {
                let Some(relative_line) = span.start.line.checked_sub(tab.doc.trim_start) else {
                    tab.annotation_hover = None;
                    return;
                };
                let Some((epoch_ms, range)) = tab.doc.explicit_timestamp_at(relative_line) else {
                    tab.annotation_hover = None;
                    return;
                };
                if range.start != span.start.byte || range.end != span.end.byte {
                    tab.annotation_hover = None;
                    return;
                }
                let line = tab.doc.line(relative_line);
                let Some(raw) = line.get(range) else {
                    tab.annotation_hover = None;
                    return;
                };
                let family = tab.doc.time_format_name().unwrap_or_else(|| {
                    if tab.doc.format_name() == "json" {
                        "JSON field".to_owned()
                    } else {
                        "field-based".to_owned()
                    }
                });
                let normalized = format!("{} UTC", format_ms(epoch_ms));
                (
                    "TIME",
                    "Timestamp",
                    format!("{family} · normalized to UTC"),
                    vec![truncate_line(raw, PREVIEW_MAX_LINE_CHARS)],
                    raw.to_owned(),
                    theme.timestamp,
                    None,
                    "Copy UTC",
                    Some(normalized),
                )
            }
        };

    let screen = ui.ctx().content_rect();
    let size = popup_size(&lines, screen);
    let bubble_width = size.x;
    let estimated_height = size.y;
    let space_above = source_rect.top() - screen.top() - POPUP_INSET;
    let space_below = screen.bottom() - source_rect.bottom() - POPUP_INSET;
    let above = space_above >= estimated_height || space_above > space_below;
    let min_x = screen.left() + POPUP_INSET;
    let max_x = (screen.right() - bubble_width - POPUP_INSET).max(min_x);
    let x = (source_rect.center().x - bubble_width * 0.5).clamp(min_x, max_x);
    let preferred_y = if above {
        source_rect.top() - estimated_height - 7.0
    } else {
        source_rect.bottom() + 7.0
    };
    let min_y = screen.top() + POPUP_INSET;
    let max_y = (screen.bottom() - estimated_height - POPUP_INSET).max(min_y);
    let y = preferred_y.clamp(min_y, max_y);

    let mut copy_source = false;
    let mut secondary_action = false;
    let area = egui::Area::new(egui::Id::new(("annotation_hover_bubble", key)))
        .order(egui::Order::Foreground)
        .default_size(size)
        .constrain_to(screen.shrink(POPUP_INSET))
        .fixed_pos(Pos2::new(x, y))
        .show(ui.ctx(), |ui| {
            egui::Frame::NONE
                .fill(scaled_alpha(theme.surface, ANNOTATION_POPUP_SURFACE_ALPHA))
                .stroke(Stroke::new(1.0, scaled_alpha(accent, 0.72)))
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.set_width(bubble_width - 20.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(badge).strong().color(accent));
                        ui.label(RichText::new(title).strong().color(theme.text));
                    });
                    ui.label(RichText::new(summary).small().color(theme.text_muted));
                    egui::ScrollArea::vertical()
                        .max_height((estimated_height - POPUP_HEADER_HEIGHT).max(24.0))
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for line in &lines {
                                ui.label(RichText::new(line).monospace().color(theme.text));
                            }
                        });
                    ui.horizontal(|ui| {
                        copy_source = icons::action_button(
                            ui,
                            Icon::Copy,
                            "Copy source",
                            theme.text,
                            "Copy the exact source text",
                        )
                        .clicked();
                        secondary_action = icons::action_button(
                            ui,
                            Icon::Info,
                            secondary_label,
                            theme.text,
                            secondary_label,
                        )
                        .clicked();
                    });
                });
        });

    let bubble_rect = area.response.rect;
    if let Some(state) = tab.annotation_hover.as_mut() {
        state.bubble_rect = Some(bubble_rect);
        if area.response.hovered() {
            state.last_seen_at = Instant::now();
        }
    }

    let arrow_x = source_rect
        .center()
        .x
        .clamp(bubble_rect.left() + 10.0, bubble_rect.right() - 10.0);
    let arrow = if above {
        vec![
            Pos2::new(arrow_x - 5.0, bubble_rect.bottom() - 1.0),
            Pos2::new(arrow_x + 5.0, bubble_rect.bottom() - 1.0),
            Pos2::new(source_rect.center().x, source_rect.top()),
        ]
    } else {
        vec![
            Pos2::new(arrow_x - 5.0, bubble_rect.top() + 1.0),
            Pos2::new(arrow_x + 5.0, bubble_rect.top() + 1.0),
            Pos2::new(source_rect.center().x, source_rect.bottom()),
        ]
    };
    ui.ctx()
        .layer_painter(area.response.layer_id)
        .add(egui::Shape::convex_polygon(
            arrow,
            scaled_alpha(theme.surface, ANNOTATION_POPUP_SURFACE_ALPHA),
            Stroke::new(1.0, scaled_alpha(accent, 0.72)),
        ));

    if copy_source {
        ui.ctx().copy_text(source);
        tab.pending_toast = Some("Copied source annotation".to_owned());
        if let Some(state) = tab.annotation_hover.as_mut() {
            state.last_seen_at = Instant::now();
        }
    }
    if secondary_action {
        if let Some(detection) = detection {
            tab.embedded_inspector_mode = inspector_mode_for_open(
                tab.embedded_inspector_mode,
                inspector_mode_for(detection.detector_id),
            );
            tab.embedded_inspector_anchor = Some(source_rect.left_center());
            tab.embedded_inspector = Some(detection);
            tab.annotation_hover = None;
        } else if let Some(normalized) = normalized {
            ui.ctx().copy_text(normalized);
            tab.pending_toast = Some("Copied normalized timestamp".to_owned());
            if let Some(state) = tab.annotation_hover.as_mut() {
                state.last_seen_at = Instant::now();
            }
        }
    }
}

fn preview_lines(detection: &Detection) -> Vec<String> {
    let preview = match &detection.data {
        DataNode::Object(_) | DataNode::Array(_) => detection.pretty.as_str(),
        _ => {
            return vec![truncate_line(
                &compact_source_preview(&detection.raw, 180),
                PREVIEW_MAX_LINE_CHARS,
            )]
        }
    };
    let mut lines = preview
        .lines()
        .map(|line| truncate_line(line, PREVIEW_MAX_LINE_CHARS))
        .collect::<Vec<_>>();
    if lines.is_empty() {
        lines.push(String::new());
    }
    if lines.len() > PREVIEW_MAX_LINES {
        let omitted = lines.len() - PREVIEW_MAX_LINES + 1;
        lines.truncate(PREVIEW_MAX_LINES - 1);
        lines.push(format!("… {omitted} more lines; open inspector for all"));
    }
    lines
}

fn truncate_line(line: &str, max_chars: usize) -> String {
    let mut chars = line.chars();
    let prefix = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

fn popup_size(lines: &[String], screen: Rect) -> egui::Vec2 {
    let available_width = (screen.width() - POPUP_INSET * 2.0).max(180.0);
    let max_width = available_width.min(POPUP_MAX_WIDTH);
    let min_width = available_width.min(POPUP_MIN_WIDTH).max(180.0);
    let longest_line = lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(1);
    let width = (longest_line as f32 * 7.2 + 28.0).clamp(min_width, max_width);
    let available_height = (screen.height() - POPUP_INSET * 2.0).max(POPUP_MIN_HEIGHT);
    let max_height = available_height.min(POPUP_MAX_HEIGHT);
    let height = (POPUP_HEADER_HEIGHT + lines.len().max(1) as f32 * POPUP_ROW_HEIGHT)
        .clamp(POPUP_MIN_HEIGHT, max_height);
    egui::vec2(width, height)
}

fn compact_source_preview(raw: &str, max_chars: usize) -> String {
    let compact = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = compact.chars();
    let preview = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{preview}…")
    } else {
        preview
    }
}

fn scaled_alpha(color: Color32, factor: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        color.r(),
        color.g(),
        color.b(),
        (color.a() as f32 * factor).round() as u8,
    )
}

fn should_dismiss(escape: bool, click: Option<Pos2>, bubble: Rect) -> bool {
    escape || click.is_some_and(|position| !bubble.contains(position))
}

#[cfg(test)]
mod tests {
    use super::*;
    use logotomy::core::embedded_data::{DataNode, SourcePos, SourceSpan};

    fn candidate(key: AnnotationHoverKey) -> Candidate {
        Candidate {
            key,
            source_rect: Rect::from_min_max(Pos2::new(100.0, 80.0), Pos2::new(140.0, 96.0)),
        }
    }

    fn embedded_key(offset: usize) -> AnnotationHoverKey {
        AnnotationHoverKey::Embedded {
            detector_id: "logfmt",
            span: SourceSpan {
                start: SourcePos {
                    line: 0,
                    byte: offset,
                },
                end: SourcePos {
                    line: 0,
                    byte: offset + 5,
                },
            },
        }
    }

    #[test]
    fn structured_preview_keeps_each_kv_on_its_own_line() {
        let detection = Detection::structured(
            "logfmt",
            SourceSpan {
                start: SourcePos { line: 0, byte: 0 },
                end: SourcePos { line: 0, byte: 16 },
            },
            "first=one second=2",
            DataNode::Object(vec![
                ("first".to_owned(), DataNode::String("one".to_owned())),
                ("second".to_owned(), DataNode::Number("2".to_owned())),
            ]),
        );
        assert_eq!(
            preview_lines(&detection),
            vec![
                "{".to_owned(),
                "  first: \"one\",".to_owned(),
                "  second: 2".to_owned(),
                "}".to_owned()
            ]
        );
    }

    #[test]
    fn preview_lines_truncate_long_values_and_large_payloads() {
        let detection = Detection::structured(
            "logfmt",
            SourceSpan {
                start: SourcePos { line: 0, byte: 0 },
                end: SourcePos { line: 0, byte: 12 },
            },
            "large=value",
            DataNode::Object(
                (0..40)
                    .map(|index| {
                        (
                            format!("key{index}"),
                            DataNode::String("x".repeat(PREVIEW_MAX_LINE_CHARS + 10)),
                        )
                    })
                    .collect(),
            ),
        );
        let lines = preview_lines(&detection);
        assert_eq!(lines.len(), PREVIEW_MAX_LINES);
        assert!(lines
            .iter()
            .all(|line| line.chars().count() <= PREVIEW_MAX_LINE_CHARS + 32));
        assert!(lines
            .last()
            .is_some_and(|line| line.contains("open inspector")));
    }

    #[test]
    fn popup_size_grows_with_content_but_stays_within_limits() {
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 900.0));
        let small = popup_size(&["a: 1".to_owned()], screen);
        let large = popup_size(&vec!["x".repeat(72); PREVIEW_MAX_LINES], screen);
        assert!(large.x > small.x);
        assert!(large.x <= POPUP_MAX_WIDTH);
        assert!(large.y <= POPUP_MAX_HEIGHT);
    }

    #[test]
    fn visible_popup_ignores_competing_highlights_until_grace_expires() {
        let path = std::env::temp_dir().join(format!(
            "logotomy_annotation_popup_{}.log",
            std::process::id()
        ));
        std::fs::write(&path, "INFO value=one other=two\n").unwrap();
        let doc = logotomy::core::document::LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        let first = candidate(embedded_key(5));
        let second = candidate(embedded_key(15));
        let bubble = Rect::from_min_max(Pos2::new(20.0, 10.0), Pos2::new(260.0, 70.0));
        egui::__run_test_ui(|ui| {
            let now = Instant::now();
            update(&mut tab, Some(first), false, ui.ctx(), now);
            tab.annotation_hover.as_mut().unwrap().bubble_rect = Some(bubble);
            update(
                &mut tab,
                Some(second),
                false,
                ui.ctx(),
                now + Duration::from_millis(100),
            );
            assert_eq!(tab.annotation_hover.as_ref().unwrap().key, first.key);

            update(
                &mut tab,
                Some(second),
                false,
                ui.ctx(),
                now + HOVER_GRACE + Duration::from_millis(1),
            );
            assert!(tab.annotation_hover.is_none());
        });
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn handoff_corridor_is_wider_than_the_source_gap() {
        let source = Rect::from_min_max(Pos2::new(100.0, 80.0), Pos2::new(140.0, 96.0));
        let bubble = Rect::from_min_max(Pos2::new(20.0, 10.0), Pos2::new(260.0, 70.0));
        assert!(pointer_in_handoff(Pos2::new(120.0, 75.0), source, bubble));
        assert!(!pointer_in_handoff(Pos2::new(175.0, 75.0), source, bubble));
    }

    #[test]
    fn compact_preview_is_single_line_and_bounded() {
        assert_eq!(
            compact_source_preview("first\n  second", 20),
            "first second"
        );
        assert_eq!(compact_source_preview("abcdefgh", 4), "abcd…");
    }

    #[test]
    fn popup_dismisses_on_escape_or_click_outside() {
        let bubble = Rect::from_min_max(Pos2::new(20.0, 20.0), Pos2::new(120.0, 80.0));
        assert!(should_dismiss(true, None, bubble));
        assert!(should_dismiss(false, Some(Pos2::new(10.0, 10.0)), bubble));
        assert!(!should_dismiss(false, Some(Pos2::new(50.0, 50.0)), bubble));
    }

    #[test]
    fn popup_surface_alpha_is_tunable_and_transparent() {
        assert!(ANNOTATION_POPUP_SURFACE_ALPHA > 0.0);
        assert!(ANNOTATION_POPUP_SURFACE_ALPHA < 1.0);
    }
}
