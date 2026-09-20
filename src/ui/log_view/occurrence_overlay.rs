//! Shift-click filter-occurrence context. The rows deliberately reuse the Log
//! View text layout so filters, timestamps, long-line policy, and structured
//! data cues stay visually consistent without adding another search surface.

use std::sync::Arc;

use eframe::egui;
use egui::{Color32, FontId, RichText};

use super::view::{line_job_for_mode, Highlights};
use crate::ui::app::model::LogTab;
use crate::ui::app::overlay;
use crate::ui::icons::{self, Icon};
use crate::ui::theme::Theme;

struct RowResult {
    response: egui::Response,
    open_embedded: Option<haystack::core::embedded_data::Detection>,
}

pub(super) fn show(ui: &mut egui::Ui, tab: &mut LogTab, theme: &Theme) {
    let Some(state) = tab.occurrence_overlay.as_ref() else {
        return;
    };
    let selected_line = state.selected_line;
    let before = state.before.clone();
    let after = state.after.clone();
    let detections = Arc::clone(&state.embedded_detections);
    let center_selected = state.center_selected;
    let pending_embedded = state.embedded_rx.is_some();
    let view_id = tab.focused_log_view_id;
    let ctx = ui.ctx().clone();
    let mut close = false;
    let mut centered = false;
    let mut open_embedded = None;
    let mut copy_context = false;
    let escape = ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));

    let modal_response = overlay::modal_without_title(
        &ctx,
        ("filter_occurrence_overlay", view_id),
        egui::vec2(900.0, 620.0),
        |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "Filter occurrences around line {}",
                        tab.doc.trim_start + selected_line + 1
                    ))
                    .strong(),
                );
                if pending_embedded {
                    ui.label(
                        RichText::new("Reading embedded data…")
                            .small()
                            .color(theme.text_muted),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(RichText::new("×").size(18.0))
                        .on_hover_text("Close (Esc)")
                        .clicked()
                    {
                        close = true;
                    }
                    if icons::icon_action_button(
                        ui,
                        Icon::Copy,
                        theme.text,
                        "Copy displayed context",
                    )
                    .clicked()
                    {
                        copy_context = true;
                    }
                });
            });
            ui.separator();

            let font = crate::ui::fonts::log_font(tab.log_font_size);
            let row_height = ui.ctx().fonts_mut(|fonts| fonts.row_height(&font)) + 2.0;
            let available = ui.available_size();
            let scroll = if tab.log_line_display_mode
                == haystack::core::settings::LogLineDisplayMode::HorizontalScroll
            {
                egui::ScrollArea::both()
            } else {
                egui::ScrollArea::vertical()
            }
            .id_salt(("filter_occurrence_scroll", view_id))
            .auto_shrink([false, false]);

            scroll
                .max_height(available.y.max(row_height * 3.0))
                .show(ui, |ui| {
                    let highlights = Highlights {
                        filters: &tab.filters,
                        filter_matcher: tab.highlighter.as_deref(),
                        search_matcher: None,
                        search_template_id: None,
                        search_field: None,
                        search_rows: None,
                        filter_fields: Some(&tab.filter_field_queries),
                        filter_rows: Some(&tab.matches),
                        keyword_ac: tab.keyword_automaton.as_deref(),
                        embedded: Some(detections.as_slice()),
                    };
                    for &line in &before {
                        let result = render_row(
                            ui,
                            tab,
                            &highlights,
                            line,
                            false,
                            font.clone(),
                            theme,
                            row_height,
                        );
                        if open_embedded.is_none() {
                            open_embedded = result.open_embedded;
                        }
                    }
                    if !before.is_empty() {
                        ui.add_space(row_height);
                    }
                    let result = render_row(
                        ui,
                        tab,
                        &highlights,
                        selected_line,
                        true,
                        font.clone(),
                        theme,
                        row_height,
                    );
                    if center_selected {
                        result.response.scroll_to_me(Some(egui::Align::Center));
                        centered = true;
                    }
                    if open_embedded.is_none() {
                        open_embedded = result.open_embedded;
                    }
                    if !after.is_empty() {
                        ui.add_space(row_height);
                    }
                    for &line in &after {
                        let result = render_row(
                            ui,
                            tab,
                            &highlights,
                            line,
                            false,
                            font.clone(),
                            theme,
                            row_height,
                        );
                        if open_embedded.is_none() {
                            open_embedded = result.open_embedded;
                        }
                    }
                });
        },
    );
    if centered {
        if let Some(state) = tab.occurrence_overlay.as_mut() {
            state.center_selected = false;
        }
    }
    if let Some(detection) = open_embedded {
        tab.embedded_inspector = Some(detection);
        tab.embedded_inspector_anchor = Some(ctx.content_rect().center());
    }
    if copy_context {
        ctx.copy_text(context_text(tab, &before, selected_line, &after));
        tab.pending_toast = Some("Occurrence context copied".to_owned());
    }
    if escape || modal_response.backdrop_response.clicked() || close {
        tab.close_occurrence_overlay();
    }
}

fn context_text(tab: &LogTab, before: &[usize], selected_line: usize, after: &[usize]) -> String {
    let mut sections = Vec::with_capacity(3);
    if !before.is_empty() {
        sections.push(before);
    }
    sections.push(std::slice::from_ref(&selected_line));
    if !after.is_empty() {
        sections.push(after);
    }

    sections
        .into_iter()
        .map(|rows| {
            rows.iter()
                .map(|&line| format!("{}: {}", tab.doc.trim_start + line + 1, tab.doc.line(line)))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn render_row(
    ui: &mut egui::Ui,
    tab: &LogTab,
    highlights: &Highlights<'_>,
    line: usize,
    selected: bool,
    font: FontId,
    theme: &Theme,
    row_height: f32,
) -> RowResult {
    let char_width = ui.ctx().fonts_mut(|fonts| fonts.glyph_width(&font, ' '));
    let gutter_digits = tab.doc.total_lines().max(1).to_string().len();
    let gutter_width = char_width * (gutter_digits as f32 + 2.0) + 12.0;
    let background = if selected {
        theme.selection_bg
    } else {
        Color32::TRANSPARENT
    };
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), row_height),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            let gutter = ui
                .allocate_exact_size(egui::vec2(gutter_width, row_height), egui::Sense::hover())
                .0;
            ui.painter().rect_filled(
                gutter,
                egui::CornerRadius::ZERO,
                if selected {
                    background
                } else {
                    theme.gutter_bg
                },
            );
            ui.painter().text(
                gutter.left_center(),
                egui::Align2::LEFT_CENTER,
                format!("{:>9}: ", tab.doc.trim_start + line + 1),
                font.clone(),
                theme.gutter,
            );
            let job = line_job_for_mode(
                &tab.doc,
                highlights,
                line,
                selected,
                font,
                theme,
                tab.log_line_display_mode,
            );
            let label = egui::Label::new(job).selectable(true);
            let mut response = ui.add(
                if tab.log_line_display_mode == haystack::core::settings::LogLineDisplayMode::Wrap {
                    label.wrap()
                } else {
                    label.extend()
                },
            );
            if !selected {
                response = response.on_hover_text(occurrence_time_tooltip(
                    tab,
                    tab.occurrence_overlay
                        .as_ref()
                        .map(|overlay| overlay.selected_line)
                        .unwrap_or(line),
                    line,
                ));
            }
            let mut open_embedded = None;
            response.context_menu(|ui| {
                if ui.button("Copy full line").clicked() {
                    ui.ctx().copy_text(tab.doc.line(line).into_owned());
                    ui.close();
                }
                let original_line = tab.doc.trim_start + line;
                for detection in highlights
                    .embedded
                    .into_iter()
                    .flatten()
                    .filter(|detection| detection.span.includes_line(original_line))
                {
                    if ui.button("Open embedded item").clicked() {
                        open_embedded = Some(detection.clone());
                        ui.close();
                    }
                }
            });
            RowResult {
                response,
                open_embedded,
            }
        },
    )
    .inner
}

fn occurrence_time_tooltip(tab: &LogTab, selected_line: usize, line: usize) -> String {
    let source_time = tab.doc.ts_at_opt(selected_line);
    let occurrence_time = tab.doc.ts_at_opt(line);
    relative_occurrence_text(source_time, occurrence_time, selected_line, line)
}

fn relative_occurrence_text(
    source_time: Option<i64>,
    occurrence_time: Option<i64>,
    selected_line: usize,
    line: usize,
) -> String {
    if let Some(delta) = occurrence_time
        .zip(source_time)
        .map(|(time, source)| time - source)
    {
        if delta < 0 {
            return format!("{} ago", format_human_duration_ms(delta.saturating_abs()));
        }
        if delta > 0 {
            return format!("after {}", format_human_duration_ms(delta));
        }
        return "at the same time".to_owned();
    }
    if line < selected_line {
        format!("{} lines ago", selected_line - line)
    } else if line > selected_line {
        format!("after {} lines", line - selected_line)
    } else {
        "selected source line".to_owned()
    }
}

fn format_human_duration_ms(ms: i64) -> String {
    let ms = ms.max(0);
    if ms >= 3_600_000 {
        let hours = ms / 3_600_000;
        let minutes = (ms % 3_600_000) / 60_000;
        if minutes == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h {minutes}m")
        }
    } else if ms >= 60_000 {
        let minutes = ms / 60_000;
        let seconds = (ms % 60_000) / 1_000;
        if seconds == 0 {
            format!("{minutes}m")
        } else {
            format!("{minutes}m {seconds}sec")
        }
    } else if ms >= 1_000 {
        format!("{:.1}sec", ms as f64 / 1_000.0)
    } else {
        format!("{ms}ms")
    }
}

#[cfg(test)]
mod tests {
    use super::relative_occurrence_text;

    #[test]
    fn occurrence_tooltip_uses_human_time_before_and_after_the_source() {
        assert_eq!(
            relative_occurrence_text(Some(90_000), Some(0), 20, 10),
            "1m 30sec ago"
        );
        assert_eq!(
            relative_occurrence_text(Some(0), Some(4_260_000), 10, 20),
            "after 1h 11m"
        );
    }

    #[test]
    fn occurrence_tooltip_falls_back_to_source_line_distance_without_times() {
        assert_eq!(relative_occurrence_text(None, None, 20, 10), "10 lines ago");
        assert_eq!(
            relative_occurrence_text(None, None, 10, 20),
            "after 10 lines"
        );
    }
}
