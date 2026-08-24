//! Central log view: virtualized with `show_rows`, so a 5M-line file renders
//! the same ~40 visible rows per frame as a 40-line file. Each row shows the
//! line number, the Drain template ID, and filter-highlighted text.
//!
//! Also provides a right-click context menu (pin / add analysis).

use std::borrow::Cow;
use std::cell::Cell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Color32, FontId, Pos2, Rect, RichText, Stroke, StrokeKind};

use logotomy::core::document::LogDocument;
use logotomy::core::embedded_data::{DataNode, Detection};
use logotomy::core::time::format_ms;

use super::embedded::presentation;
use crate::ui::app::model::{EmbeddedInspectorMode, LogTab, PinEntry, TrimAction, MAX_FILTERS};
use crate::ui::icons::{self, Icon};
use crate::ui::theme::Theme;

#[path = "highlight.rs"]
mod highlight;
pub use highlight::{line_job, Highlights};

/// Number of digits reserved by the line-number gutter before it grows.
const GUTTER_DIGITS: usize = 9;
/// Extra breathing room after the line number and separator.
const GUTTER_PADDING: f32 = 12.0;
/// Small baseline adjustment so the gutter sits closer to source text.
const GUTTER_VERTICAL_OFFSET: f32 = 1.0;
/// Minimum pointer displacement (px) to distinguish a drag from a click.
const DRAG_THRESHOLD: f32 = 3.0;

/// Action returned from a single row render, to be applied after the
/// scroll-area closure so we avoid borrow conflicts with `tab`.
enum RowAction {
    Select,
    Pin,
    TrimRight,
    TrimLeft,
    Keyword(String),
    OpenData(Detection, Pos2),
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub fn show(ui: &mut egui::Ui, tab: &mut LogTab, theme: &Theme) {
    let n = tab.doc.total_lines();
    if n == 0 {
        ui.centered_and_justified(|ui| {
            ui.label(RichText::new("Empty file. Nothing to heck about.").color(theme.placeholder));
        });
        return;
    }

    // ---- font size + row height ----
    // Log text uses the embedded Space Mono face (see `ui/fonts`).
    let font_id = crate::ui::fonts::log_font(tab.log_font_size);
    let row_height = ui.ctx().fonts_mut(|f| f.row_height(&font_id)) + 2.0;
    let avail_height = ui.available_height();
    let max_visible_lines = if row_height > 0.0 {
        (avail_height / row_height).floor() as usize
    } else {
        0
    };

    // ---- toolbar (font size controls + lines-visible label) ----
    show_toolbar(ui, tab, theme, max_visible_lines);

    // Deferred actions from context menu (avoid borrow conflicts inside show_rows).
    let mut context_pin: Option<usize> = None;
    let mut context_trim: Option<TrimAction> = None;
    let mut open_data: Option<Detection> = None;
    let mut suppress_select: bool = false;

    let total_visible = match &tab.visible_lines {
        Some(vis) => vis.len(),
        None => n,
    };
    let available = ui.available_size();

    // Cell to capture the exact rendered range from show_rows, avoiding
    // offset-based approximation that can drift due to partial rows and egui buffering.
    let rendered_range: Cell<Option<(usize, usize)>> = Cell::new(None);
    // Take the pending scroll before entering the closure to avoid borrow conflicts.
    let pending = tab.pending_scroll.take();
    // One-shot preserve-anchor set by a filter change; top-aligns the viewport.
    let preserve_anchor = tab.preserve_anchor.take();

    // ---- scroll area setup (vertical) ----
    let char_width = ui.ctx().fonts_mut(|f| f.glyph_width(&font_id, ' '));
    let gutter_width = line_gutter_width(char_width);
    let mut scroll_area = egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .id_salt("log_scroll");

    if let Some(offset) =
        compute_pending_scroll_offset(tab, pending, row_height, avail_height, total_visible)
    {
        scroll_area = scroll_area.vertical_scroll_offset(offset);
    } else if let Some(anchor) = preserve_anchor {
        // No pending scroll (diamond click etc.), so honor a filter-change
        // anchor by top-aligning the preserved reference line.
        if let Some(offset) = compute_preserve_anchor_offset(tab, anchor, row_height, total_visible)
        {
            scroll_area = scroll_area.vertical_scroll_offset(offset);
        }
    }

    // ---- set item_spacing.y = 0.0 BEFORE show_rows so egui's internal
    //      row_height_with_spacing == row_height (no drift) ----
    ui.spacing_mut().item_spacing.y = 0.0;

    // Give the scroll area the full remaining width so its native scrollbar
    // is placed at the container's right edge.
    let inner_resp = ui.allocate_ui_with_layout(
        available,
        egui::Layout::top_down_justified(egui::Align::LEFT),
        |ui| {
            let output = scroll_area.show_rows(ui, row_height, total_visible, |ui, range| {
                // Capture the exact range egui determined to be visible.
                // range.end is exclusive, so last = end - 1.
                if !range.is_empty() {
                    rendered_range.set(Some((range.start, range.end - 1)));
                }
                for vi in range {
                    let i = match &tab.visible_lines {
                        Some(vis) => vis[vi],
                        None => vi,
                    };
                    let selected = tab.context_line == Some(i);

                    let action = render_row(
                        ui,
                        &tab.doc,
                        &Highlights::from_tab(tab),
                        i,
                        selected,
                        font_id.clone(),
                        theme,
                        row_height,
                        tab.selection_range,
                        char_width,
                        gutter_width,
                    );

                    let is_select = matches!(action, Some(RowAction::Select));
                    match action {
                        Some(RowAction::Select) if !suppress_select => {
                            tab.set_keyword_highlight(None);
                            tab.context_line = Some(i);
                            tab.sync_timeline_selection_to_line(i);
                            tab.ensure_visible();
                        }
                        Some(RowAction::Pin) => {
                            context_pin = Some(i);
                        }
                        Some(RowAction::TrimRight) => {
                            context_trim = Some(TrimAction::TrimRight(i));
                        }
                        Some(RowAction::TrimLeft) => {
                            context_trim = Some(TrimAction::TrimLeft(i));
                        }
                        Some(RowAction::Keyword(kw)) => {
                            // Only paint the keyword highlight; never touch the
                            // search box (Esc / single-click clears it).
                            tab.set_keyword_highlight(Some(kw));
                        }
                        Some(RowAction::OpenData(detection, anchor)) => {
                            open_data = Some(detection);
                            tab.embedded_inspector_anchor = Some(anchor);
                        }
                        _ => {}
                    }

                    // If this row was a click but we later determine it was actually a drag,
                    // suppress the select action. For simplicity, we track whether any row
                    // received a click this frame and suppress on the next frame if drag was detected.
                    if is_select && tab.drag_selecting {
                        suppress_select = true;
                    }
                }
            });
            output
        },
    );
    let output = inner_resp.inner;

    // Apply deferred context menu actions.
    apply_context_actions(tab, context_pin, context_trim);
    if let Some(detection) = open_data {
        // Keep the user's Tree/Pretty/Raw choice for ordinary structured data.
        // Stack traces and encoded values still get their safe, detector-specific
        // starting views whenever one of those payloads is opened.
        tab.embedded_inspector_mode = inspector_mode_for_open(
            tab.embedded_inspector_mode,
            inspector_mode_for(detection.detector_id),
        );
        tab.embedded_inspector = Some(detection);
    }

    // ---- compute viewport_range for timeline shadow ----
    update_viewport_range(tab, &rendered_range, pending);
    if tab.schedule_embedded_scan() {
        ui.ctx().request_repaint_after(Duration::from_millis(60));
    }

    // ---- arrow-key find navigation ----
    if tab.pin_modal.is_none() && tab.pending_selection.is_none() {
        let search_has_focus = tab.find_rx.is_some() || !tab.find_query.is_empty();
        let search_input_focused = ui
            .ctx()
            .memory(|memory| memory.has_focus(egui::Id::new("log_find_input")));
        ui.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft)
                && !search_input_focused
                && (tab.selected_lane.is_some() || !search_has_focus)
            {
                if tab.selected_lane.is_some() {
                    tab.select_lane_previous();
                } else {
                    tab.find_prev();
                }
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight)
                && !search_input_focused
                && (tab.selected_lane.is_some() || !search_has_focus)
            {
                if tab.selected_lane.is_some() {
                    tab.select_lane_next();
                } else {
                    tab.find_next();
                }
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) && !search_input_focused {
                navigate_vertical(tab, false);
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) && !search_input_focused {
                navigate_vertical(tab, true);
            }
        });
    }

    // ---- Esc peels one layer at a time ----
    if tab.pin_modal.is_none()
        && tab.pending_selection.is_none()
        && ui.input(|i| i.key_pressed(egui::Key::Escape))
    {
        if tab.keyword_highlight.is_some() {
            tab.set_keyword_highlight(None);
        } else if !tab.find_query.is_empty() {
            tab.clear_find();
            tab.find_input.clear();
        }
    }

    // ---- pointer-driven drag selection ----
    let pointer = ui.input(|i| i.pointer.clone());
    let inner_rect = output.inner_rect;

    if pointer.primary_pressed() {
        if let Some(press_pos) = pointer.latest_pos() {
            if inner_rect.contains(press_pos) {
                // Don't start a new drag if a popup is visible — the press is on the buttons
                if tab.pending_selection.is_none() && tab.pin_modal.is_none() {
                    tab.drag_selecting = true;
                    tab.drag_start_pos = Some(press_pos);
                    if let Some(row) = row_under_pointer(
                        ui,
                        inner_rect,
                        output.state.offset.y,
                        row_height,
                        total_visible,
                        &tab.visible_lines,
                    ) {
                        tab.drag_start_line = Some(row);
                        tab.drag_current_line = Some(row);
                        tab.selection_range = Some((row, row));
                    }
                }
            }
        }
    }

    if tab.drag_selecting {
        if let Some(row) = row_under_pointer(
            ui,
            inner_rect,
            output.state.offset.y,
            row_height,
            total_visible,
            &tab.visible_lines,
        ) {
            tab.drag_current_line = Some(row);
            if let (Some(start), Some(current)) = (tab.drag_start_line, tab.drag_current_line) {
                let lo = start.min(current);
                let hi = start.max(current);
                tab.selection_range = Some((lo, hi));
            }
        }

        // Use !primary_down() instead of primary_released() because
        // pointer.released can be consumed by show_rows or other widgets
        if !pointer.primary_down() {
            tab.drag_selecting = false;
            let release_pos = pointer.latest_pos();
            let is_drag = tab
                .drag_start_pos
                .zip(release_pos)
                .is_some_and(|(start, end)| start.distance(end) >= DRAG_THRESHOLD);
            if is_drag {
                if let (Some(start), Some(end)) = (tab.drag_start_line, tab.drag_current_line) {
                    let lo = start.min(end);
                    let hi = start.max(end);
                    tab.pending_selection = Some((lo, hi));
                    tab.selection_popup_pos = release_pos;
                } else {
                    tab.selection_range = None;
                    tab.pending_selection = None;
                }
            } else {
                tab.selection_range = None;
                tab.pending_selection = None;
            }
            tab.drag_start_line = None;
            tab.drag_current_line = None;
            if tab.pending_selection.is_none() {
                tab.drag_start_pos = None;
            }
        }
    }

    // If the primary button was released outside a drag (no pending_selection),
    // clear transient selection so it doesn't linger.
    if !tab.drag_selecting
        && tab.selection_range.is_some()
        && tab.pending_selection.is_none()
        && !pointer.primary_down()
    {
        tab.selection_range = None;
    }

    // ---- selection popup (single "📌 Pin" button) ----
    if let Some(pending_range) = tab.pending_selection {
        let (start, end) = pending_range;
        let count = match &tab.visible_lines {
            Some(vis) => visible_lines_in_range(vis, start, end).len(),
            None => end - start + 1,
        };

        let popup_id = egui::Id::new("selection_popup");
        if tab.selection_popup_opened_at.is_none() {
            tab.selection_popup_opened_at = Some(Instant::now());
        }
        let popup_anchor_pos = tab
            .selection_popup_pos
            .unwrap_or_else(|| ui.input(|i| i.pointer.latest_pos().unwrap_or_default()));
        let popup_pos = popup_anchor_pos + egui::vec2(8.0, 8.0);

        let area = egui::Area::new(popup_id)
            .current_pos(popup_pos)
            .order(egui::Order::Foreground)
            .fixed_pos(popup_pos);
        let area_resp = area.show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(240.0);
                ui.label(
                    RichText::new(format!("Selected: {} lines ({}-{})", count, start, end))
                        .strong()
                        .size(13.0),
                );
                ui.separator();

                ui.horizontal(|ui| {
                    if ui.button("Pin").clicked() {
                        tab.pin_modal = Some(pending_range);
                        tab.pin_comment.clear();
                        tab.pending_selection = None;
                        tab.drag_start_pos = None;
                        tab.selection_popup_pos = None;
                        tab.selection_popup_opened_at = None;
                    }
                });

                if ui.button("Cancel").clicked() {
                    tab.selection_range = None;
                    tab.pending_selection = None;
                    tab.drag_start_pos = None;
                    tab.selection_popup_pos = None;
                    tab.selection_popup_opened_at = None;
                }
            });
        });

        if area_resp.response.hovered() {
            tab.selection_popup_opened_at = Some(Instant::now());
        } else if tab
            .selection_popup_opened_at
            .is_some_and(|opened| opened.elapsed() >= Duration::from_secs(2))
        {
            tab.selection_range = None;
            tab.pending_selection = None;
            tab.drag_start_pos = None;
            tab.selection_popup_pos = None;
            tab.selection_popup_opened_at = None;
        } else {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }

        // Close on click outside
        if ui.input(|i| i.pointer.any_click()) {
            if let Some(click_pos) = ui.input(|i| i.pointer.interact_pos()) {
                if !area_resp.response.rect.contains(click_pos) {
                    tab.selection_range = None;
                    tab.pending_selection = None;
                    tab.drag_start_pos = None;
                    tab.selection_popup_pos = None;
                    tab.selection_popup_opened_at = None;
                }
            }
        }
    }

    // (The pin modal moved to `pin_modal_ui`, drawn at the app level so it
    // works even when the Log view isn't the focused dock tab.)
}

/// Render the pin creation/editing modal (comment + log preview). Called from
/// the app level so it works regardless of which dock tab or detached
/// viewport is focused. Reuses the same window for creating a new pin and for
/// editing an existing one (when `tab.pin_edit_index` is set).
pub fn pin_modal_ui(ui: &mut egui::Ui, tab: &mut LogTab, theme: &Theme) {
    let Some(range) = tab.pin_modal else { return };
    let (start, end) = range;
    // Compute the actual visible lines within the range (respects text filters).
    let visible_in_range: Vec<usize> = match &tab.visible_lines {
        Some(vis) => visible_lines_in_range(vis, start, end).to_vec(),
        None => (start..=end).collect(),
    };
    let actual_count = visible_in_range.len();

    let ts = if tab.doc.ts_at_opt(start).is_some_and(|v| v >= 0) {
        format_ms(tab.doc.ts_at(start))
    } else {
        format!("line {}", start + 1)
    };

    let editing = tab.pin_edit_index.is_some();
    let title = if editing {
        format!("Edit pin — {} lines", actual_count)
    } else {
        format!("Pin — {} lines", actual_count)
    };
    let font_id = crate::ui::fonts::log_font(tab.log_font_size);

    let mut open = true;
    let mut do_save = false;
    let mut do_cancel = false;
    egui::Window::new(title)
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(500.0)
        .default_height(400.0)
        .show(ui.ctx(), |ui| {
            ui.horizontal(|ui| {
                let ctx = ui.ctx().clone();
                ui.add(icons::icon_image(&ctx, Icon::Date, 13.0, theme.text));
                ui.label(RichText::new(format!("Viewed on {ts}")).strong().size(13.0));
            });
            ui.separator();

            ui.label(RichText::new("Comment (optional):").strong());
            let text_resp = ui.add_sized(
                egui::vec2(ui.available_width(), 60.0),
                egui::TextEdit::multiline(&mut tab.pin_comment)
                    .hint_text(
                        "Type your comment. Enter to save, Shift+Enter for newline, Esc to cancel…",
                    )
                    .desired_width(f32::INFINITY),
            );
            // Auto-focus the text input when the modal opens
            text_resp.request_focus();

            // Save on Enter (no Shift), Esc to cancel
            if text_resp.lost_focus() {
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) && !ui.input(|i| i.modifiers.shift)
                {
                    do_save = true;
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    do_cancel = true;
                }
            }

            // Also handle Enter/Esc on the TextEdit while focused (not just on lost_focus)
            let enter_pressed =
                ui.input(|i| i.key_pressed(egui::Key::Enter)) && !ui.input(|i| i.modifiers.shift);
            let esc_pressed = ui.input(|i| i.key_pressed(egui::Key::Escape));
            if text_resp.has_focus() {
                if enter_pressed {
                    do_save = true;
                }
                if esc_pressed {
                    do_cancel = true;
                }
            }

            ui.add_space(8.0);
            ui.label(RichText::new("Selected lines:").strong().size(11.0));

            let max_preview = 20;
            if actual_count > max_preview {
                egui::ScrollArea::vertical()
                    .max_height(180.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for &line_idx in visible_in_range.iter().take(max_preview / 2) {
                            let job = line_job(
                                &tab.doc,
                                &Highlights::filters_only(&tab.filters, tab.highlighter.as_deref()),
                                line_idx,
                                false,
                                font_id.clone(),
                                theme,
                            );
                            ui.add(egui::Label::new(job));
                        }
                        ui.label(
                            RichText::new(format!("… {} more lines …", actual_count - max_preview))
                                .italics()
                                .color(theme.text_muted),
                        );
                        for &line_idx in visible_in_range.iter().rev().take(max_preview / 2).rev() {
                            let job = line_job(
                                &tab.doc,
                                &Highlights::filters_only(&tab.filters, tab.highlighter.as_deref()),
                                line_idx,
                                false,
                                font_id.clone(),
                                theme,
                            );
                            ui.add(egui::Label::new(job));
                        }
                    });
            } else {
                egui::ScrollArea::vertical()
                    .max_height(180.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for &line_idx in &visible_in_range {
                            let job = line_job(
                                &tab.doc,
                                &Highlights::filters_only(&tab.filters, tab.highlighter.as_deref()),
                                line_idx,
                                false,
                                font_id.clone(),
                                theme,
                            );
                            ui.add(egui::Label::new(job));
                        }
                    });
            }

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    do_save = true;
                }
                if ui.button("Cancel").clicked() {
                    do_cancel = true;
                }
            });
        });

    if do_save {
        save_pin(tab, range);
    } else if do_cancel || !open {
        tab.pin_modal = None;
        tab.pin_edit_index = None;
        tab.pin_comment.clear();
    }
}

const INSPECTOR_MIN_SIZE: egui::Vec2 = egui::Vec2::new(520.0, 220.0);
const INSPECTOR_HORIZONTAL_GAP: f32 = 4.0;
const INSPECTOR_VERTICAL_GAP: f32 = 12.0;

/// Persistent structured-data inspector, drawn as an in-view foreground
/// overlay so it stays visually attached to the cue that opened it.
pub fn embedded_data_inspector_ui(ui: &mut egui::Ui, tab: &mut LogTab, theme: &Theme) {
    let Some(detection) = tab.embedded_inspector.take() else {
        return;
    };
    let anchor = tab
        .embedded_inspector_anchor
        .unwrap_or_else(|| ui.ctx().content_rect().center());
    let screen = ui.ctx().content_rect();
    let default_size = inspector_default_size(&detection, tab.embedded_inspector_mode, screen);
    let panel_pos = inspector_position(anchor, screen, default_size);
    let mut close = false;
    let area_response = egui::Window::new("embedded_json_inspector")
        .id(egui::Id::new("embedded_json_inspector"))
        .order(egui::Order::Foreground)
        .title_bar(false)
        .collapsible(false)
        .resizable(true)
        .movable(false)
        .fixed_pos(panel_pos)
        .default_size(default_size)
        .min_size(INSPECTOR_MIN_SIZE)
        .max_size(inspector_max_size(screen))
        .frame(
            egui::Frame::new()
                .fill(scaled_alpha(theme.surface, 0.98))
                .stroke(Stroke::new(1.0, scaled_alpha(theme.accent, 0.65)))
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(egui::Margin::same(10)),
        )
        .show(ui.ctx(), |ui| {
            ui.horizontal(|ui| {
                let presentation = presentation(detection.detector_id);
                ui.label(RichText::new(presentation.title).strong());
                ui.label(detection.summary());
                ui.separator();
                ui.label(format!(
                    "lines {}–{}",
                    detection.span.start.line + 1,
                    detection.span.end.line + 1
                ));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icons::image_button(ui, Icon::Close, egui::vec2(22.0, 22.0), theme.text)
                        .on_hover_text("Close inspector")
                        .clicked()
                    {
                        close = true;
                    }
                });
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let presentation = presentation(detection.detector_id);
                let primary_mode = inspector_mode_for(detection.detector_id);
                ui.selectable_value(
                    &mut tab.embedded_inspector_mode,
                    primary_mode,
                    presentation.primary_tab,
                );
                if !matches!(
                    primary_mode,
                    EmbeddedInspectorMode::Frames | EmbeddedInspectorMode::Summary
                ) {
                    ui.selectable_value(
                        &mut tab.embedded_inspector_mode,
                        EmbeddedInspectorMode::Pretty,
                        "Pretty",
                    );
                }
                ui.selectable_value(
                    &mut tab.embedded_inspector_mode,
                    EmbeddedInspectorMode::Raw,
                    "Raw",
                );
                ui.separator();
                if presentation.explicit_decode && ui.button("Decode preview").clicked() {
                    tab.embedded_inspector_mode = EmbeddedInspectorMode::Decoded;
                }
                if ui.button("Copy raw").clicked() {
                    ui.ctx().copy_text(detection.raw.clone());
                    tab.pending_toast = Some("Copied raw embedded data".to_string());
                }
                if ui.button("Copy pretty").clicked() {
                    ui.ctx().copy_text(detection.pretty.clone());
                    tab.pending_toast = Some("Copied formatted embedded data".to_string());
                }
            });
            ui.separator();
            let content_height = (ui.available_height() - 4.0).max(80.0);
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .max_height(content_height)
                .show(ui, |ui| match tab.embedded_inspector_mode {
                    EmbeddedInspectorMode::Tree
                    | EmbeddedInspectorMode::Frames
                    | EmbeddedInspectorMode::Summary => {
                        render_data_node(ui, "$", &detection.data, "root", 0, theme);
                    }
                    EmbeddedInspectorMode::Pretty => {
                        ui.add(
                            egui::Label::new(RichText::new(&detection.pretty).monospace())
                                .selectable(true),
                        );
                    }
                    EmbeddedInspectorMode::Raw => {
                        ui.add(
                            egui::Label::new(RichText::new(&detection.raw).monospace())
                                .selectable(true),
                        );
                    }
                    EmbeddedInspectorMode::Decoded => {
                        render_decoded_preview(ui, &detection.raw, theme);
                    }
                });
        });
    let escape = ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    let outside_click = ui.input(|input| {
        input
            .pointer
            .any_click()
            .then(|| input.pointer.interact_pos())
            .flatten()
    });
    let window_rect = area_response.map(|response| response.response.rect);
    let dismissed = should_dismiss_inspector(escape, outside_click, window_rect);
    if close || dismissed {
        tab.embedded_inspector_anchor = None;
    } else {
        tab.embedded_inspector = Some(detection);
    }
}

fn inspector_mode_for(detector_id: &str) -> EmbeddedInspectorMode {
    match presentation(detector_id).primary_tab {
        "Frames" => EmbeddedInspectorMode::Frames,
        "Summary" => EmbeddedInspectorMode::Summary,
        _ => EmbeddedInspectorMode::Tree,
    }
}

fn inspector_mode_for_open(
    current: EmbeddedInspectorMode,
    detector_mode: EmbeddedInspectorMode,
) -> EmbeddedInspectorMode {
    if matches!(
        detector_mode,
        EmbeddedInspectorMode::Frames | EmbeddedInspectorMode::Summary
    ) {
        detector_mode
    } else if matches!(
        current,
        EmbeddedInspectorMode::Frames
            | EmbeddedInspectorMode::Summary
            | EmbeddedInspectorMode::Decoded
    ) {
        EmbeddedInspectorMode::Pretty
    } else {
        current
    }
}

/// Decoding is deliberately only reachable from an explicit inspector action.
/// It is byte-bounded by the detector's own candidate limit and never verifies
/// JWT signatures or interprets binary code.
fn render_decoded_preview(ui: &mut egui::Ui, raw: &str, theme: &Theme) {
    let decoded = decode_embedded_preview(raw)
        .unwrap_or_else(|message| format!("Unable to decode preview: {message}"));
    ui.add(
        egui::Label::new(RichText::new(decoded).monospace().color(theme.log_text)).selectable(true),
    );
}

fn decode_embedded_preview(raw: &str) -> Result<String, &'static str> {
    let token = raw.trim();
    if token.starts_with("-----BEGIN ") {
        return Ok("PEM envelope detected. Certificate decoding is intentionally metadata-only in this build.".to_owned());
    }
    if token.split('.').count() == 3 {
        let payload = token.split('.').nth(1).unwrap_or_default();
        return decode_base64(payload, true);
    }
    if let Some(hex) = token.strip_prefix("0x").or(Some(token)).filter(|value| {
        value.len() >= 16
            && value.len() % 2 == 0
            && value.bytes().all(|byte| byte.is_ascii_hexdigit())
    }) {
        let bytes = (0..hex.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).map_err(|_| "invalid hex"))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(String::from_utf8_lossy(&bytes).into_owned());
    }
    decode_base64(token, false)
}

fn decode_base64(value: &str, url: bool) -> Result<String, &'static str> {
    let mut out = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0u8;
    for byte in value.bytes().filter(|byte| *byte != b'=') {
        let six = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' if !url => 62,
            b'/' if !url => 63,
            b'-' if url => 62,
            b'_' if url => 63,
            _ => return Err("invalid base64"),
        };
        buffer = (buffer << 6) | u32::from(six);
        bits += 6;
        while bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    String::from_utf8(out).map_err(|_| "decoded data is binary")
}

/// Keep the first presentation useful without forcing every payload into the
/// old fixed-size box. The Window remembers a user's later resize in egui's
/// persistent area state.
fn inspector_default_size(
    detection: &Detection,
    mode: EmbeddedInspectorMode,
    screen: Rect,
) -> egui::Vec2 {
    let text = match mode {
        EmbeddedInspectorMode::Raw => &detection.raw,
        _ => &detection.pretty,
    };
    let line_count = text.lines().count().max(1) as f32;
    let longest_line = text.lines().map(str::len).max().unwrap_or(1) as f32;
    let estimated_width = 360.0 + (longest_line * 7.2).min(360.0);
    let estimated_height = 150.0 + (line_count * 18.0).min(520.0);
    let max_size = inspector_max_size(screen);

    egui::vec2(
        estimated_width.clamp(INSPECTOR_MIN_SIZE.x, max_size.x),
        estimated_height.clamp(INSPECTOR_MIN_SIZE.y, max_size.y),
    )
}

fn inspector_max_size(screen: Rect) -> egui::Vec2 {
    egui::vec2(
        (screen.width() - INSPECTOR_HORIZONTAL_GAP * 2.0).max(INSPECTOR_MIN_SIZE.x),
        (screen.height() - INSPECTOR_VERTICAL_GAP * 2.0).max(INSPECTOR_MIN_SIZE.y),
    )
}

fn inspector_position(anchor: Pos2, screen: Rect, size: egui::Vec2) -> Pos2 {
    let right_space = screen.right() - anchor.x;
    let x = if right_space >= size.x + INSPECTOR_HORIZONTAL_GAP {
        anchor.x + INSPECTOR_HORIZONTAL_GAP
    } else {
        anchor.x - size.x - INSPECTOR_HORIZONTAL_GAP
    };

    let below_space = screen.bottom() - anchor.y;
    let y = if below_space >= size.y + INSPECTOR_VERTICAL_GAP {
        anchor.y + INSPECTOR_VERTICAL_GAP
    } else {
        anchor.y - size.y - INSPECTOR_VERTICAL_GAP
    };

    Pos2::new(
        x.clamp(
            screen.left() + INSPECTOR_HORIZONTAL_GAP,
            screen.right() - size.x - INSPECTOR_HORIZONTAL_GAP,
        ),
        y.clamp(
            screen.top() + INSPECTOR_VERTICAL_GAP,
            screen.bottom() - size.y - INSPECTOR_VERTICAL_GAP,
        ),
    )
}

fn should_dismiss_inspector(escape: bool, click: Option<Pos2>, window_rect: Option<Rect>) -> bool {
    escape || click.is_some_and(|position| window_rect.is_some_and(|rect| !rect.contains(position)))
}

fn render_data_node(
    ui: &mut egui::Ui,
    label: &str,
    node: &DataNode,
    path: &str,
    depth: usize,
    theme: &Theme,
) {
    match node {
        DataNode::Object(entries) => {
            egui::CollapsingHeader::new(format!("{label}  {{ {} fields }}", entries.len()))
                .id_salt(path)
                .default_open(depth < 2)
                .show(ui, |ui| {
                    for (index, (key, value)) in entries.iter().enumerate() {
                        render_data_node(
                            ui,
                            key,
                            value,
                            &format!("{path}.object.{index}"),
                            depth + 1,
                            theme,
                        );
                    }
                });
        }
        DataNode::Array(items) => {
            egui::CollapsingHeader::new(format!("{label}  [ {} items ]", items.len()))
                .id_salt(path)
                .default_open(depth < 2)
                .show(ui, |ui| {
                    for (index, value) in items.iter().enumerate() {
                        render_data_node(
                            ui,
                            &format!("[{index}]"),
                            value,
                            &format!("{path}.array.{index}"),
                            depth + 1,
                            theme,
                        );
                    }
                });
        }
        DataNode::String(value) => {
            let display = format!("\"{}\"", preview(value));
            let copy = serde_json::to_string(value).unwrap_or_else(|_| display.clone());
            data_leaf(ui, label, &display, &copy, theme);
        }
        DataNode::Number(value) => data_leaf(ui, label, value, value, theme),
        DataNode::Bool(value) => {
            let value = value.to_string();
            data_leaf(ui, label, &value, &value, theme);
        }
        DataNode::Null => data_leaf(ui, label, "null", "null", theme),
    }
}

fn data_leaf(ui: &mut egui::Ui, label: &str, display: &str, copy: &str, theme: &Theme) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).strong());
        ui.label(RichText::new(display).monospace().color(theme.log_text));
        if ui.small_button("Copy").clicked() {
            ui.ctx().copy_text(copy.to_string());
        }
    });
}

fn preview(value: &str) -> Cow<'_, str> {
    const LIMIT: usize = 240;
    if value.len() <= LIMIT {
        return Cow::Borrowed(value);
    }
    let mut end = LIMIT;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    Cow::Owned(format!("{}…", &value[..end]))
}
/// Save the current pin modal range as a PinEntry and close the modal.
/// If `tab.pin_edit_index` is set, the matching entry is updated in place
/// instead of pushing a brand-new pin.
#[inline]
fn save_pin(tab: &mut LogTab, range: (usize, usize)) {
    let (start, end) = range;
    // When visible_lines is active (filtered view), only include lines that
    // are actually visible, since the user's selection spans virtual indices.
    let line_numbers: Vec<usize> = match &tab.visible_lines {
        Some(vis) => visible_lines_in_range(vis, start, end).to_vec(),
        None => (start..=end).collect(),
    };
    let start_ts = tab.doc.ts_at_opt(start).unwrap_or(-1);
    let end_ts = tab.doc.ts_at_opt(end).unwrap_or(-1);
    let comment = tab.pin_comment.trim().to_string();

    if let Some(idx) = tab.pin_edit_index {
        // Editing an existing pin: replace its content in place.
        if idx < tab.pins.len() {
            let p = &mut tab.pins[idx];
            p.start_line = start;
            p.line_numbers = line_numbers;
            p.start_ts = start_ts;
            p.end_ts = end_ts;
            p.comment = comment;
        }
    } else {
        tab.pins.push(PinEntry {
            start_line: start,
            line_numbers,
            start_ts,
            end_ts,
            comment,
        });
    }
    tab.pin_modal = None;
    tab.pin_edit_index = None;
    tab.pin_comment.clear();
    tab.bottom_panel_open = true;
}

fn navigate_vertical(tab: &mut LogTab, next: bool) {
    if tab.find_matches.is_empty() {
        tab.select_adjacent_line(next);
    } else if next {
        tab.find_next();
    } else {
        tab.find_prev();
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Font-size buttons, trim indicator, and "lines visible" label.
fn show_toolbar(ui: &mut egui::Ui, tab: &mut LogTab, theme: &Theme, _max_visible_lines: usize) {
    ui.horizontal(|ui| {
        if ui
            .button("A-")
            .on_hover_text("Decrease text size")
            .clicked()
        {
            tab.log_font_size = (tab.log_font_size - 1.0).max(8.0);
        }
        if ui
            .button("A+")
            .on_hover_text("Increase text size")
            .clicked()
        {
            tab.log_font_size = (tab.log_font_size + 1.0).min(24.0);
        }
        ui.label(
            RichText::new(format!("{:.0}px", tab.log_font_size))
                .monospace()
                .color(theme.text_muted),
        );
        ui.add_space(8.0);

        // Trim indicator + reset button
        if tab.doc.is_trimmed() {
            let total = tab.doc.total_lines_untrimmed();
            let current = tab.doc.total_lines();
            let ctx = ui.ctx().clone();
            ui.add(icons::icon_image(&ctx, Icon::Trim, 12.0, theme.warning));
            ui.label(
                RichText::new(format!("{} / {} lines", current, total))
                    .color(theme.warning)
                    .size(11.0),
            );
            if ui
                .button("Reset")
                .on_hover_text("Reset trim to show all lines")
                .clicked()
            {
                tab.handle_trim_reset();
            }
        }

        // Search controls sit left-aligned, right after the status text, separated
        // by a separator (not flushed to the right of the toolbar).
        ui.separator();
        ui.add_space(8.0);
        show_search_ui(ui, tab, theme);
    });
}

/// Render the find/search UI in the toolbar.
fn show_search_ui(ui: &mut egui::Ui, tab: &mut LogTab, theme: &Theme) {
    let search_active = !tab.find_query.is_empty();

    ui.horizontal(|ui| {
        let output = egui::TextEdit::singleline(&mut tab.find_input)
            .id(egui::Id::new("log_find_input"))
            .hint_text("search log + Enter")
            .desired_width(200.0)
            .show(ui);
        let resp_id = output.response.id;
        let input_resp = output.response;

        // Cmd/Ctrl+F focus: select the existing text and park the caret at the
        // end, so typing replaces the selection and Left-arrow collapses it to
        // append/extend the current query.
        if std::mem::take(&mut tab.search_focus_requested) {
            input_resp.request_focus();
            let len = tab.find_input.chars().count();
            if len > 0 {
                let range = egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(len),
                );
                let mut state = output.state;
                state.cursor.set_char_range(Some(range));
                state.store(ui.ctx(), resp_id);
            }
        }

        // Short 0.5s highlight "pulse" on the search box border when it gained
        // focus via Cmd/Ctrl+F.
        if let Some(at) = tab.search_focus_anim {
            let duration = Duration::from_millis(500);
            let elapsed = at.elapsed();
            if elapsed < duration {
                let t = elapsed.as_secs_f32() / duration.as_secs_f32();
                let alpha = ((1.0 - t) * 150.0) as u8;
                let color = Color32::from_rgba_unmultiplied(
                    theme.accent.r(),
                    theme.accent.g(),
                    theme.accent.b(),
                    alpha,
                );
                ui.painter().rect_stroke(
                    input_resp.rect.expand(1.0),
                    egui::CornerRadius::same(4),
                    Stroke::new(1.5_f32, color),
                    StrokeKind::Middle,
                );
                ui.ctx().request_repaint();
            } else {
                tab.search_focus_anim = None;
            }
        }

        if input_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let trimmed = tab.find_input.trim();
            if trimmed != tab.find_query {
                tab.start_find(trimmed.to_string());
            } else {
                tab.find_next();
            }
        }

        let case_sensitive = ui
            .add(
                egui::Button::new(RichText::new("Aa").monospace())
                    .selected(tab.find_case_sensitive),
            )
            .on_hover_text("Match case");
        if case_sensitive.clicked() {
            tab.find_case_sensitive = !tab.find_case_sensitive;
            if !tab.find_input.trim().is_empty() {
                tab.start_find(tab.find_input.clone());
            }
        }

        if !tab.find_matches.is_empty() {
            let pos = tab.find_pos.unwrap_or(0);
            ui.label(
                RichText::new(format!("{} / {}", pos + 1, tab.find_matches.len()))
                    .size(11.0)
                    .color(theme.text_muted),
            );
        } else if search_active {
            ui.label(RichText::new("no matches").size(11.0).color(theme.warning));
        }

        let has_matches = !tab.find_matches.is_empty();
        let nav_color = if has_matches {
            theme.text
        } else {
            theme.text_muted
        };
        if ui
            .add_enabled(
                has_matches,
                egui::Button::new(icons::icon_image(ui.ctx(), Icon::ArrowUp, 13.0, nav_color)),
            )
            .on_hover_text("Previous match")
            .clicked()
        {
            tab.find_prev();
        }

        if ui
            .add_enabled(
                has_matches,
                egui::Button::new(icons::icon_image(
                    ui.ctx(),
                    Icon::ArrowDown,
                    13.0,
                    nav_color,
                )),
            )
            .on_hover_text("Next match")
            .clicked()
        {
            tab.find_next();
        }

        if ui
            .add(egui::Button::new(icons::icon_image(
                ui.ctx(),
                Icon::Close,
                13.0,
                theme.text,
            )))
            .on_hover_text("Clear search")
            .clicked()
        {
            tab.clear_find();
            tab.find_input.clear();
        }

        // Once a search completes, offer to promote it into a timeline filter —
        // but only if the searched text isn't already one of the filters.
        let can_add_filter = !tab.find_query.is_empty()
            && !tab.find_matches.is_empty()
            && tab.filters.len() < MAX_FILTERS
            && !tab.filters.iter().any(|f| f.text == tab.find_query);
        if can_add_filter {
            if ui
                .add(egui::Button::image_and_text(
                    icons::icon_image(ui.ctx(), Icon::Enter, 13.0, theme.text),
                    "Add Filter",
                ))
                .on_hover_text(format!("Add '{}' as a timeline filter", tab.find_query))
                .clicked()
            {
                let color = theme.filter_colors[tab.filters.len() % theme.filter_colors.len()];
                let query = tab.find_query.clone();
                tab.push_filter(&query, color);
            }
        }

        if tab.find_rx.is_some() {
            ui.spinner();
        }
    });
}

fn keyword_at(text: &str, char_idx: usize) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    let char_count = text.chars().count();
    if char_idx >= char_count {
        return None;
    }
    let clicked_char = text.chars().nth(char_idx)?;
    if is_word_delimiter(clicked_char) {
        return None;
    }

    let mut start = char_idx;
    while start > 0 {
        let c = text.chars().nth(start - 1)?;
        if is_word_delimiter(c) {
            break;
        }
        start -= 1;
    }
    let mut end = char_idx;
    while end < char_count {
        let c = text.chars().nth(end)?;
        if is_word_delimiter(c) {
            break;
        }
        end += 1;
    }
    let word: String = text.chars().skip(start).take(end - start).collect();
    if word.is_empty() || word.chars().all(is_word_delimiter) {
        None
    } else {
        Some(word)
    }
}

fn is_word_delimiter(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '-' | '[' | ']' | '{' | '}' | '(' | ')' | ',' | '"' | '\''
        )
}

/// Compute the deterministic scroll offset for a pending scroll-to-line,
/// centering the target line in the viewport. Returns `None` when no
/// pending scroll is needed.
fn compute_pending_scroll_offset(
    tab: &LogTab,
    pending: Option<usize>,
    row_height: f32,
    avail_height: f32,
    total_visible: usize,
) -> Option<f32> {
    let line = pending?;
    let vis_idx = match &tab.visible_lines {
        Some(ref vis) => match vis.binary_search(&line) {
            Ok(idx) => idx,
            Err(idx) => idx.min(vis.len().saturating_sub(1)),
        },
        None => line.min(total_visible.saturating_sub(1)),
    };
    let center_offset = vis_idx as f32 * row_height - (avail_height - row_height) * 0.5;
    Some(center_offset.max(0.0))
}

/// Compute a top-aligned scroll offset that places the preserved anchor line
/// at the top of the viewport. Used after a lane-filter change to keep the
/// previously visible log content in the window.
fn compute_preserve_anchor_offset(
    tab: &LogTab,
    anchor: usize,
    row_height: f32,
    total_visible: usize,
) -> Option<f32> {
    let vis_idx = match &tab.visible_lines {
        Some(ref vis) => match vis.binary_search(&anchor) {
            Ok(idx) => idx,
            Err(idx) => idx.min(vis.len().saturating_sub(1)),
        },
        None => anchor.min(total_visible.saturating_sub(1)),
    };
    Some(vis_idx as f32 * row_height)
}

/// Calculate which real log line index is under the pointer, if any.
fn row_under_pointer(
    ui: &egui::Ui,
    inner_rect: Rect,
    scroll_offset: f32,
    row_height: f32,
    total_visible: usize,
    visible_lines: &Option<Arc<Vec<usize>>>,
) -> Option<usize> {
    let pointer = ui.input(|i| i.pointer.latest_pos())?;
    if pointer.x < inner_rect.left()
        || pointer.x > inner_rect.right()
        || pointer.y < inner_rect.top()
        || pointer.y > inner_rect.bottom()
    {
        return None;
    }
    let relative_y = pointer.y - inner_rect.top() + scroll_offset;
    let virtual_idx = (relative_y / row_height).floor() as usize;
    let virtual_idx = virtual_idx.min(total_visible.saturating_sub(1));
    match visible_lines {
        Some(vis) => vis.get(virtual_idx).copied(),
        None => Some(virtual_idx),
    }
}

/// The filtered visible-line list is sorted by real line number. Range
/// selection and pinning should therefore use two binary searches instead of
/// walking every visible line each frame.
fn visible_lines_in_range(lines: &[usize], start: usize, end: usize) -> &[usize] {
    let first = lines.partition_point(|&line| line < start);
    let last = lines.partition_point(|&line| line <= end);
    &lines[first..last]
}

/// Render a single log row, returning an action to be applied by the caller.
/// The line number is shown as a separate non-interactive label, followed by
/// selectable log content with filter highlighting.
fn render_row(
    ui: &mut egui::Ui,
    doc: &LogDocument,
    highlights: &Highlights,
    idx: usize,
    selected: bool,
    font_id: FontId,
    theme: &Theme,
    row_height: f32,
    selection_range: Option<(usize, usize)>,
    char_width: f32,
    gutter_width: f32,
) -> Option<RowAction> {
    let mut action: Option<RowAction> = None;

    let in_selection = selection_range.is_some_and(|(lo, hi)| idx >= lo && idx <= hi);
    let bg = if selected {
        theme.selection_bg
    } else if in_selection {
        theme.selection_range_bg
    } else {
        Color32::TRANSPARENT
    };

    // Build the log content job (no line number, no color marker).
    let job = line_job(doc, highlights, idx, selected, font_id.clone(), theme);

    // Use a horizontal layout: line number (non-interactive) + selectable log content.
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), row_height),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            // Line number gutter — non-interactive, not selectable during drag.
            ui.allocate_ui_with_layout(
                egui::vec2(gutter_width, row_height),
                egui::Layout::left_to_right(egui::Align::Center),
                |gutter_ui| {
                    let gutter_rect = gutter_ui.max_rect();
                    let gutter_fill = if bg == Color32::TRANSPARENT {
                        theme.gutter_bg
                    } else {
                        bg
                    };
                    gutter_ui.painter().rect_filled(
                        gutter_rect,
                        egui::CornerRadius::ZERO,
                        gutter_fill,
                    );

                    let line_height = gutter_ui
                        .ctx()
                        .fonts_mut(|fonts| fonts.row_height(&font_id));
                    let top_padding =
                        ((row_height - line_height) * 0.5 + GUTTER_VERTICAL_OFFSET).max(0.0);
                    gutter_ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                        ui.add_space(top_padding);
                        let line_num_fmt = egui::text::TextFormat {
                            font_id: font_id.clone(),
                            color: theme.gutter,
                            ..Default::default()
                        };
                        let mut line_num_job = egui::text::LayoutJob::default();
                        line_num_job.append(
                            &format!("{:>width$}: ", idx + 1, width = GUTTER_DIGITS),
                            0.0,
                            line_num_fmt,
                        );
                        ui.add(egui::Label::new(line_num_job).selectable(false));
                    });
                },
            );

            let original_line = doc.trim_start + idx;
            let row_detection = highlights.embedded.and_then(|detections| {
                detections
                    .iter()
                    .find(|detection| detection.span.start.line == original_line)
            });

            // Log content label — selectable and clickable for row actions. `line_job` already
            // bakes the correct per-section background (selection tint on
            // non-highlighted spans, search/keyword highlight colours on
            // matches); do NOT overwrite it, or highlights are erased.
            let content_job = job;
            let content_resp = ui.add(log_content_label(content_job));

            // Paint a tiny overlay at the exact opening token. It consumes no
            // layout width, so embedded-data results never move log text.
            if let Some(detection) = row_detection {
                let presentation = presentation(detection.detector_id);
                if let Some(prefix) =
                    source_prefix_at(doc.line_bytes(idx), detection.span.start.byte)
                {
                    let prefix_width = ui.ctx().fonts_mut(|fonts| {
                        fonts
                            .layout_no_wrap(prefix.to_owned(), font_id.clone(), theme.log_text)
                            .size()
                            .x
                    });
                    let badge_font_size = (font_id.size * 0.46).clamp(4.5, 11.0);
                    let badge_font = FontId::proportional(badge_font_size);
                    let label_width = ui.ctx().fonts_mut(|fonts| {
                        fonts
                            .layout_no_wrap(
                                presentation.badge.to_string(),
                                badge_font.clone(),
                                theme.embedded_data,
                            )
                            .size()
                            .x
                    });
                    let (badge_size, vertical_lift) =
                        embedded_badge_geometry(font_id.size, label_width);
                    let anchor_x = content_resp.rect.left() + prefix_width;
                    let badge_y =
                        (content_resp.rect.top() - vertical_lift).max(ui.clip_rect().top());
                    let badge_rect = Rect::from_min_size(Pos2::new(anchor_x, badge_y), badge_size);
                    let badge_id = ui.make_persistent_id((
                        "embedded_data_overlay",
                        detection.detector_id,
                        detection.span.start.line,
                        detection.span.start.byte,
                    ));
                    let response = ui
                        .interact(badge_rect.expand(2.0), badge_id, egui::Sense::click())
                        .on_hover_ui(|ui| {
                            ui.label(
                                RichText::new(presentation.title)
                                    .strong()
                                    .color(theme.embedded_data),
                            );
                            ui.label(detection.summary());
                            ui.label(format!(
                                "Lines {}–{}",
                                detection.span.start.line + 1,
                                detection.span.end.line + 1
                            ));
                            ui.label(
                                RichText::new("Click to inspect and copy")
                                    .small()
                                    .color(theme.text_muted),
                            );
                        });
                    let painter = ui.painter();
                    let fill = scaled_alpha(theme.embedded_data_bg, 0.62);
                    let stroke_color = scaled_alpha(theme.embedded_data, 0.52);
                    let text_color = scaled_alpha(theme.embedded_data, 0.72);
                    let corner = (badge_size.y * 0.24).round().clamp(1.0, 4.0) as u8;
                    painter.rect_filled(badge_rect, egui::CornerRadius::same(corner), fill);
                    painter.rect_stroke(
                        badge_rect,
                        egui::CornerRadius::same(corner),
                        Stroke::new((font_id.size * 0.045).clamp(0.35, 0.85), stroke_color),
                        StrokeKind::Inside,
                    );
                    painter.text(
                        badge_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        presentation.badge,
                        badge_font,
                        text_color,
                    );
                    if response.clicked() {
                        // Keep the inspector tied to the line-number gutter,
                        // so it opens beside it or flips above/below it when
                        // the viewport edge leaves less room.
                        action = Some(RowAction::OpenData(
                            detection.clone(),
                            Pos2::new(content_resp.rect.left(), content_resp.rect.center().y),
                        ));
                    }
                }
            }

            // Double-click: keyword highlight
            if content_resp.double_clicked() {
                let rel_x = ui
                    .input(|i| i.pointer.latest_pos())
                    .map(|p| p.x - content_resp.rect.left())
                    .unwrap_or(0.0);
                let ci = (rel_x / char_width).max(0.0) as usize;
                if let Some(kw) = keyword_at(&doc.line(idx), ci) {
                    action = Some(RowAction::Keyword(kw));
                }
            } else if content_resp.clicked() {
                action = Some(RowAction::Select);
            }

            // Right-click: context menu (on the whole row area)
            content_resp.context_menu(|ui| {
                ui.set_min_width(160.0);
                if ui.button("Pin").clicked() {
                    action = Some(RowAction::Pin);
                    ui.close();
                }
                ui.separator();
                if ui
                    .button("Trim top")
                    .on_hover_text("Remove all lines before this one")
                    .clicked()
                {
                    action = Some(RowAction::TrimLeft);
                    ui.close();
                }
                if ui
                    .button("Trim bottom")
                    .on_hover_text("Remove all lines after this one")
                    .clicked()
                {
                    action = Some(RowAction::TrimRight);
                    ui.close();
                }
            });

            if content_resp.hovered() {
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Text);
            }
        },
    )
    .inner;

    action
}

/// Keep native egui text selection enabled for log content. Whole-line drag
/// selection remains handled by `show` for pinning selected rows.
fn log_content_label(job: egui::text::LayoutJob) -> egui::Label {
    // Virtual rows have a fixed height, so log content must never wrap into
    // the next row. The enclosing scroll area clips horizontally.
    egui::Label::new(job).selectable(true).extend()
}

fn source_prefix_at(bytes: &[u8], byte_offset: usize) -> Option<&str> {
    std::str::from_utf8(bytes.get(..byte_offset)?).ok()
}

fn line_gutter_width(char_width: f32) -> f32 {
    char_width * (GUTTER_DIGITS as f32 + 2.0) + GUTTER_PADDING
}

fn embedded_badge_geometry(log_font_size: f32, label_width: f32) -> (egui::Vec2, f32) {
    let height = (log_font_size * 0.56).clamp(6.0, 13.5);
    let horizontal_padding = (log_font_size * 0.16).clamp(1.5, 4.0);
    let width = label_width + horizontal_padding * 2.0;
    let vertical_lift = height * 0.76;
    (egui::vec2(width, height), vertical_lift)
}

fn scaled_alpha(color: Color32, factor: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        color.r(),
        color.g(),
        color.b(),
        (color.a() as f32 * factor).round() as u8,
    )
}

/// Apply the deferred pin / trim actions from the context menu.
/// Single-line right-click "📌 Pin" opens the pin modal.
fn apply_context_actions(
    tab: &mut LogTab,
    context_pin: Option<usize>,
    context_trim: Option<TrimAction>,
) {
    if let Some(line) = context_pin {
        tab.pin_modal = Some((line, line));
        tab.pin_comment.clear();
        tab.bottom_panel_open = true;
    }
    if let Some(action) = context_trim {
        tab.handle_trim(action);
    }
}

/// Update `tab.viewport_range` for the timeline shadow, using the exact
/// rendered range from `show_rows`. If a pending scroll was just processed,
/// force the range to include the target line so the shadow immediately
/// covers the selection marker.
fn update_viewport_range(
    tab: &mut LogTab,
    rendered_range: &Cell<Option<(usize, usize)>>,
    pending: Option<usize>,
) {
    let mut forced_range: Option<(usize, usize)> = None;
    if pending.is_some() {
        if let Some(line) = tab.context_line {
            forced_range = Some((line, line));
        }
    }
    if let Some((first_virtual, last_virtual)) = rendered_range.get() {
        let map_to_real = |vi: usize| -> usize {
            match &tab.visible_lines {
                Some(vis) => {
                    if vi < vis.len() {
                        vis[vi]
                    } else {
                        vis.last().copied().unwrap_or(0)
                    }
                }
                None => vi,
            }
        };
        let first_real = map_to_real(first_virtual);
        let last_real = map_to_real(last_virtual);
        let merged = match (forced_range, first_real <= last_real) {
            (Some((f, l)), true) => Some((first_real.min(f), last_real.max(l))),
            (Some(range), false) => Some(range),
            (None, true) => Some((first_real, last_real)),
            (None, false) => None,
        };
        if let Some((fr, lr)) = merged {
            tab.viewport_range = Some((fr, lr));
        }
    } else if let Some(range) = forced_range {
        tab.viewport_range = Some(range);
    }

    // If the visible range has scrolled fully out of the current timeline view,
    // recenter the timeline zoom window on the shadow so the user never loses
    // their position (timeline "zoom slider" stays in sync with log scroll).
    tab.ensure_viewport_visible();
}

#[cfg(test)]
mod tests {
    use super::highlight::{display_text, MAX_DISPLAY_BYTES};
    use super::*;
    use crate::ui::app::model::Filter;

    fn write_temp(content: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "logotomy_log_view_test_{}_{}.log",
            std::process::id(),
            n
        ));
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn save_pin_edits_existing_entry_in_place() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z INFO alpha\n\
             2026-07-19T10:00:01.000Z WARN beta\n",
        );
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.pins.push(PinEntry {
            start_line: 0,
            line_numbers: vec![0, 1],
            start_ts: 0,
            end_ts: 1,
            comment: "old comment".into(),
        });

        // Simulate the pin-viewer edit flow: pre-fill the comment + flags, then save.
        tab.pin_comment = "updated comment".into();
        tab.pin_edit_index = Some(0);
        save_pin(&mut tab, (0, 1));

        assert_eq!(tab.pins.len(), 1, "editing must not add a second pin");
        assert_eq!(tab.pins[0].comment, "updated comment");
        assert_eq!(tab.pins[0].line_numbers, vec![0, 1]);
        assert!(tab.pin_modal.is_none());
        assert!(tab.pin_edit_index.is_none());
        assert!(tab.pin_comment.is_empty());

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn save_pin_without_edit_flag_creates_a_new_pin() {
        let path = write_temp("2026-07-19T10:00:00.000Z INFO alpha\n");
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.pin_comment = "fresh comment".into();
        save_pin(&mut tab, (0, 0));

        assert_eq!(tab.pins.len(), 1);
        assert_eq!(tab.pins[0].line_numbers, vec![0]);

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn vertical_navigation_prefers_search_occurrences_then_visible_lines() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z alpha\n\
             2026-07-19T10:00:01.000Z beta\n\
             2026-07-19T10:00:02.000Z gamma\n",
        );
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);

        tab.context_line = Some(1);
        navigate_vertical(&mut tab, false);
        assert_eq!(tab.context_line, Some(0));

        tab.find_matches = vec![0, 2];
        tab.find_pos = Some(0);
        navigate_vertical(&mut tab, true);
        assert_eq!(tab.context_line, Some(2));
        assert_eq!(tab.find_pos, Some(1));
        navigate_vertical(&mut tab, false);
        assert_eq!(tab.context_line, Some(0));
        assert_eq!(tab.find_pos, Some(0));

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn keyword_at_mid_word() {
        assert_eq!(keyword_at("hello world", 2), Some("hello".to_string()));
    }

    #[test]
    fn keyword_at_on_delimiter_returns_none() {
        assert_eq!(keyword_at("hello world", 5), None);
    }

    #[test]
    fn keyword_at_bracketed_token() {
        assert_eq!(
            keyword_at("[LogEntry] foo", 1),
            Some("LogEntry".to_string())
        );
    }

    #[test]
    fn keyword_at_past_eol_returns_none() {
        assert_eq!(keyword_at("hi", 5), None);
    }

    #[test]
    fn display_text_truncates_at_a_utf8_boundary() {
        let line = format!("{}é trailing", "a".repeat(MAX_DISPLAY_BYTES - 1));
        let shown = display_text(&line);

        assert!(shown.contains("truncated"));
        assert!(shown.starts_with(&"a".repeat(MAX_DISPLAY_BYTES - 1)));
        assert!(!shown.contains("é trailing"));
    }

    #[test]
    fn source_prefix_uses_exact_utf8_byte_position() {
        let line = "INFO ঢাকা payload={\"ok\":true}";
        let opening = line.find('{').unwrap();
        assert_eq!(
            source_prefix_at(line.as_bytes(), opening),
            Some("INFO ঢাকা payload=")
        );
        assert_eq!(
            source_prefix_at(line.as_bytes(), opening + 1),
            Some("INFO ঢাকা payload={")
        );
        // A raw source byte offset inside a multibyte character is not safe to
        // project into the rendered text; the underline remains the fallback.
        let inside_unicode = line.find('ঢ').unwrap() + 1;
        assert_eq!(source_prefix_at(line.as_bytes(), inside_unicode), None);
    }

    #[test]
    fn embedded_badge_scales_with_log_font() {
        let (small, small_lift) = embedded_badge_geometry(8.0, 9.0);
        let (large, large_lift) = embedded_badge_geometry(24.0, 25.0);
        assert!(large.x > small.x);
        assert!(large.y > small.y);
        assert!(large_lift > small_lift);
        assert!(small.y <= 8.0, "small cue should stay between compact rows");
        assert!(
            large.y < 24.0,
            "large cue must remain smaller than log text"
        );
        assert!(scaled_alpha(Color32::from_rgba_unmultiplied(1, 2, 3, 100), 0.5).a() == 50);
    }

    #[test]
    fn line_number_gutter_reserves_more_than_the_previous_width() {
        let width = line_gutter_width(8.0);
        assert!(width > 8.0 * 9.0);
        assert_eq!(line_gutter_width(8.0), 100.0);
    }

    #[test]
    fn inspector_chooses_available_side_and_vertical_space() {
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let size = egui::vec2(620.0, 480.0);
        assert_eq!(
            inspector_position(Pos2::new(100.0, 100.0), screen, size),
            Pos2::new(104.0, 112.0)
        );
        assert_eq!(
            inspector_position(Pos2::new(1100.0, 740.0), screen, size),
            Pos2::new(476.0, 248.0)
        );
    }

    #[test]
    fn inspector_keeps_user_tab_for_next_payload() {
        assert_eq!(
            EmbeddedInspectorMode::default(),
            EmbeddedInspectorMode::Pretty
        );
        assert_eq!(
            inspector_mode_for_open(EmbeddedInspectorMode::Tree, EmbeddedInspectorMode::Tree),
            EmbeddedInspectorMode::Tree
        );
        assert_eq!(
            inspector_mode_for_open(EmbeddedInspectorMode::Raw, EmbeddedInspectorMode::Tree),
            EmbeddedInspectorMode::Raw
        );
        assert_eq!(
            inspector_mode_for_open(EmbeddedInspectorMode::Pretty, EmbeddedInspectorMode::Frames),
            EmbeddedInspectorMode::Frames
        );
        assert_eq!(
            inspector_mode_for_open(EmbeddedInspectorMode::Decoded, EmbeddedInspectorMode::Tree),
            EmbeddedInspectorMode::Pretty
        );
    }

    #[test]
    fn inspector_dismisses_on_escape_or_outside_click() {
        let rect = Rect::from_min_size(Pos2::new(10.0, 10.0), egui::vec2(100.0, 80.0));
        assert!(should_dismiss_inspector(true, None, Some(rect)));
        assert!(should_dismiss_inspector(
            false,
            Some(Pos2::new(5.0, 5.0)),
            Some(rect)
        ));
        assert!(!should_dismiss_inspector(
            false,
            Some(Pos2::new(20.0, 20.0)),
            Some(rect)
        ));
        assert!(!should_dismiss_inspector(false, None, Some(rect)));
    }

    #[test]
    fn visible_lines_range_uses_sorted_bounds() {
        let lines = [1, 3, 7, 10, 11, 20];
        assert_eq!(visible_lines_in_range(&lines, 3, 11), &[3, 7, 10, 11]);
        assert!(visible_lines_in_range(&lines, 12, 19).is_empty());
    }

    #[test]
    fn log_content_label_supports_native_text_drag_selection() {
        egui::__run_test_ui(|ui| {
            let response = ui.add(log_content_label(egui::text::LayoutJob::single_section(
                "select me".to_owned(),
                egui::text::TextFormat::default(),
            )));
            assert!(response.sense.senses_drag());
            assert!(response.sense.senses_click());
        });
    }

    #[test]
    fn line_job_preserves_search_and_keyword_highlight_backgrounds() {
        let path = write_temp("2026-07-19T10:00:00.000Z INFO error starting\n");
        let doc = LogDocument::open(&path).unwrap();
        let theme = Theme::dark();
        let font_id = crate::ui::fonts::log_font(12.0);

        // Search match must carry the search highlight background.
        let search_ac = logotomy::core::search::build_find_automaton("error", true);
        let hl = Highlights {
            filters: &[],
            filter_ac: None,
            search_ac: search_ac.as_ref(),
            keyword_ac: None,
            embedded: None,
        };
        let job = line_job(&doc, &hl, 0, false, font_id.clone(), &theme);
        assert!(
            job.sections
                .iter()
                .any(|s| s.format.background == theme.search_highlight_bg),
            "search match must carry the search highlight background"
        );

        // Keyword match must carry the keyword highlight background.
        let keyword_ac = logotomy::core::search::build_find_automaton("error", true);
        let hl = Highlights {
            filters: &[],
            filter_ac: None,
            search_ac: None,
            keyword_ac: keyword_ac.as_ref(),
            embedded: None,
        };
        let job = line_job(&doc, &hl, 0, false, font_id, &theme);
        assert!(
            job.sections
                .iter()
                .any(|s| s.format.background == theme.keyword_highlight_bg),
            "keyword match must carry the keyword highlight background"
        );

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn higher_priority_highlight_wins_over_filter() {
        let path = write_temp("error\n");
        let doc = LogDocument::open(&path).unwrap();
        let theme = Theme::dark();
        let font_id = crate::ui::fonts::log_font(12.0);
        let filters = [Filter {
            text: "error".to_owned(),
            color: Color32::RED,
        }];
        let filter_ac = logotomy::core::search::build_automaton(&["error".to_owned()]);
        let search_ac = logotomy::core::search::build_find_automaton("error", true);
        let hl = Highlights {
            filters: &filters,
            filter_ac: filter_ac.as_ref(),
            search_ac: search_ac.as_ref(),
            keyword_ac: None,
            embedded: None,
        };

        let job = line_job(&doc, &hl, 0, false, font_id, &theme);
        assert!(job
            .sections
            .iter()
            .any(|s| s.format.background == theme.search_highlight_bg));
        assert!(!job.sections.iter().any(|s| s.format.background
            == Color32::from_rgba_unmultiplied(
                Color32::RED.r(),
                Color32::RED.g(),
                Color32::RED.b(),
                51
            )));

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn embedded_json_underline_coexists_with_search_background() {
        use logotomy::core::embedded_data::{AnalysisLimits, EmbeddedDataEngine};
        use std::sync::atomic::AtomicBool;

        let path = write_temp("INFO payload={\"error\":true}\n");
        let doc = LogDocument::open(&path).unwrap();
        let detections = EmbeddedDataEngine::default().analyze_original_lines(
            &doc,
            0..1,
            AnalysisLimits::default(),
            &AtomicBool::new(false),
        );
        let search_ac = logotomy::core::search::build_find_automaton("error", true);
        let highlights = Highlights {
            filters: &[],
            filter_ac: None,
            search_ac: search_ac.as_ref(),
            keyword_ac: None,
            embedded: Some(&detections),
        };
        let theme = Theme::dark();
        let job = line_job(
            &doc,
            &highlights,
            0,
            false,
            crate::ui::fonts::log_font(12.0),
            &theme,
        );
        assert!(job.sections.iter().any(|section| {
            section.format.background == theme.search_highlight_bg
                && section.format.underline.width > 0.0
        }));

        std::fs::remove_file(path).ok();
    }
}
