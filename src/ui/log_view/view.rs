//! Central log view: virtualized with `show_rows`, so a 5M-line file renders
//! the same ~40 visible rows per frame as a 40-line file. Each row shows the
//! line number, the Drain template ID, and filter-highlighted text.
//!
//! Also provides a right-click context menu (pin / add analysis).

use std::borrow::Cow;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Color32, FontId, Pos2, Rect, RichText, Stroke, StrokeKind};

use haystack::core::document::LogDocument;
use haystack::core::embedded_data::{DataNode, Detection, SourcePos, SourceSpan};
use haystack::core::time::format_ms;

use super::embedded::presentation;
use super::{analysis_popup, annotation_popup, occurrence_overlay};
use crate::ui::app::model::{
    AnnotationHoverKey, EmbeddedInspectorMode, LogTab, PinEntry, TrimAction, MAX_FILTERS,
};
use crate::ui::icons::{self, Icon};
use crate::ui::theme::Theme;
use crate::ui::util::error_bubble::{error_bubble, BubbleAlign};
use crate::ui::util::suggestion_row;

#[path = "highlight.rs"]
mod highlight;
pub use highlight::{line_job, line_job_for_mode, Highlights};

/// Extra breathing room after the line number and separator.
const GUTTER_PADDING: f32 = 12.0;
/// Small baseline adjustment so the gutter sits closer to source text.
const GUTTER_VERTICAL_OFFSET: f32 = 1.0;
/// Minimum pointer displacement (px) to distinguish a drag from a click.
const DRAG_THRESHOLD: f32 = 3.0;
/// A nearby navigation target should require only enough scrolling to reveal it.
const NEAR_SELECTION_SCROLL_LINES: usize = 5;
/// Keep a one-row visual safety margin when user scrolling reselects an edge
/// row inside the viewport.
const SELECTION_VIEWPORT_MARGIN_LINES: usize = 1;
/// Keep a larger two-row margin when navigation scrolls to a selected line;
/// this prevents partial row rendering from making the selection look clipped.
const SELECTION_SCROLL_MARGIN_LINES: usize = 2;

/// Action returned from a single row render, to be applied after the
/// scroll-area closure so we avoid borrow conflicts with `tab`.
enum RowAction {
    Select,
    OpenOccurrenceOverlay,
    Pin,
    CopyFull,
    CopyWithoutHeader,
    CopyWithLineNumber,
    OpenFullLine,
    TrimRight,
    TrimLeft,
    Keyword(String),
    OpenEmbedded(Detection),
    SearchField(haystack::core::field_query::FieldQuery),
    FilterField(haystack::core::field_query::FieldQuery),
}

#[derive(Default)]
struct RowRenderResult {
    action: Option<RowAction>,
    hovered: Option<annotation_popup::Candidate>,
    anchor_rect: Option<Rect>,
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
    let mut context_pin: Option<(usize, Rect)> = None;
    let mut context_trim: Option<TrimAction> = None;
    let mut context_copy: Option<(usize, RowAction)> = None;
    let mut context_field_action: Option<RowAction> = None;
    let mut context_embedded: Option<Detection> = None;
    let mut open_full_line: Option<usize> = None;
    let mut hovered_annotation: Option<annotation_popup::Candidate> = None;
    // Keep a selection-originated highlight authoritative for the whole
    // analysis editor lifetime, even if another transient selection field is
    // cleared while the popup handles an action.
    let popup_selection_range = tab
        .analysis_popup
        .filter(|state| state.from_selection)
        .map(|state| state.range);
    let selection_range = popup_selection_range.or(tab.selection_range);
    let pending_selection_before = tab.pending_selection;
    let selection_anchor_range = tab.pending_selection.or(selection_range);
    let mut selection_anchor_rect: Option<Rect> = None;
    let mut suppress_select: bool = false;
    let occurrence_overlay_open = tab.occurrence_overlay.is_some();

    let total_visible = match &tab.visible_lines {
        Some(vis) => vis.len(),
        None => n,
    };
    let available = ui.available_size();
    // The previous layout gives us the real viewport height, including space
    // consumed by a horizontal scrollbar. Use the current panel height as a
    // conservative first-frame estimate and refresh it after layout below.
    let estimated_viewport_height = tab
        .log_viewport_height
        .map(|height| height.min(available.y))
        .unwrap_or(available.y);
    // Take the pending scroll before entering the closure to avoid borrow conflicts.
    let pending = tab.pending_scroll.take();
    let restored_scroll = tab.pending_scroll_restore.take();
    // One-shot preserve-anchor set by a filter change; top-aligns the viewport.
    let preserve_anchor = tab.preserve_anchor.take();

    // ---- scroll area setup ----
    let char_width = ui.ctx().fonts_mut(|f| f.glyph_width(&font_id, ' '));
    let gutter_width = line_gutter_width(char_width, n);
    let wrap_offsets = (tab.log_line_display_mode
        == haystack::core::settings::LogLineDisplayMode::Wrap)
        .then(|| {
            wrap_offsets_for(
                tab,
                (available.x - gutter_width).max(char_width),
                char_width,
                row_height,
            )
        });
    let mut scroll_area = if tab.log_line_display_mode
        == haystack::core::settings::LogLineDisplayMode::HorizontalScroll
    {
        egui::ScrollArea::both()
    } else {
        egui::ScrollArea::vertical()
    }
    .auto_shrink([false, false])
    .id_salt(("log_scroll", tab.focused_log_view_id));

    if let Some(offsets) = wrap_offsets.as_ref() {
        let target = pending
            .or_else(|| restored_scroll.map(|(anchor, _)| anchor))
            .or(preserve_anchor);
        let offset = if pending.is_some() {
            compute_pending_wrap_scroll_offset(
                tab,
                pending,
                offsets,
                estimated_viewport_height,
                row_height,
                total_visible,
            )
        } else {
            target.and_then(|line| wrap_scroll_offset(tab, offsets, line))
        };
        if let Some(offset) = offset {
            scroll_area = scroll_area.vertical_scroll_offset(offset);
        }
    } else if let Some(offset) = compute_pending_scroll_offset(
        tab,
        pending,
        row_height,
        estimated_viewport_height,
        total_visible,
    ) {
        scroll_area = scroll_area.vertical_scroll_offset(offset);
    } else if let Some((anchor, fraction)) = restored_scroll {
        if let Some(offset) =
            compute_restore_scroll_offset(tab, anchor, fraction, row_height, total_visible)
        {
            scroll_area = scroll_area.vertical_scroll_offset(offset);
        }
    } else if let Some(anchor) = preserve_anchor {
        // No pending scroll (timeline occurrence click etc.), so honor a filter-change
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
            let output = if let Some(offsets) = wrap_offsets.as_ref() {
                scroll_area.show_viewport(ui, |ui, viewport| {
                    ui.set_height(*offsets.last().unwrap_or(&0.0));
                    let start = offsets
                        .partition_point(|offset| *offset <= viewport.min.y)
                        .saturating_sub(1)
                        .min(total_visible);
                    let end = offsets
                        .partition_point(|offset| *offset < viewport.max.y + row_height)
                        .min(total_visible);
                    let rect = Rect::from_x_y_ranges(
                        ui.max_rect().x_range(),
                        (ui.max_rect().top() + offsets[start])
                            ..=(ui.max_rect().top() + offsets[end]),
                    );
                    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                        ui.skip_ahead_auto_ids(start);
                        for vi in start..end {
                            let i = match &tab.visible_lines {
                                Some(vis) => vis[vi] as usize,
                                None => vi,
                            };
                            let selected = tab.context_line == Some(i);
                            let rendered = render_row(
                                ui,
                                &tab.doc,
                                &Highlights::from_tab(tab),
                                i,
                                selected,
                                font_id.clone(),
                                theme,
                                offsets[vi + 1] - offsets[vi],
                                selection_range,
                                char_width,
                                gutter_width,
                                tab.log_line_display_mode,
                            );
                            if hovered_annotation.is_none() {
                                hovered_annotation = rendered.hovered;
                            }
                            if let (Some((lo, hi)), Some(anchor_rect)) =
                                (selection_anchor_range, rendered.anchor_rect)
                            {
                                if (lo..=hi).contains(&i) {
                                    selection_anchor_rect = Some(match selection_anchor_rect {
                                        Some(current) => current.union(anchor_rect),
                                        None => anchor_rect,
                                    });
                                }
                            }
                            let is_select = matches!(rendered.action, Some(RowAction::Select));
                            match rendered.action {
                                Some(RowAction::Select) if !suppress_select => {
                                    tab.set_keyword_highlight(None);
                                    tab.context_line = Some(i);
                                    tab.ensure_visible();
                                }
                                Some(RowAction::OpenOccurrenceOverlay) => {
                                    tab.set_keyword_highlight(None);
                                    tab.context_line = Some(i);
                                    tab.ensure_visible();
                                    tab.open_occurrence_overlay(i);
                                }
                                Some(RowAction::Pin) => {
                                    if let Some(anchor_rect) = rendered.anchor_rect {
                                        context_pin = Some((i, anchor_rect));
                                    }
                                }
                                Some(
                                    action @ (RowAction::CopyFull
                                    | RowAction::CopyWithoutHeader
                                    | RowAction::CopyWithLineNumber),
                                ) => context_copy = Some((i, action)),
                                Some(RowAction::OpenFullLine) => open_full_line = Some(i),
                                Some(RowAction::TrimRight) => {
                                    context_trim = Some(TrimAction::TrimRight(i))
                                }
                                Some(RowAction::TrimLeft) => {
                                    context_trim = Some(TrimAction::TrimLeft(i))
                                }
                                Some(RowAction::Keyword(kw)) => tab.set_keyword_highlight(Some(kw)),
                                Some(RowAction::OpenEmbedded(detection)) => {
                                    context_embedded = Some(detection)
                                }
                                Some(
                                    action
                                    @ (RowAction::SearchField(_) | RowAction::FilterField(_)),
                                ) => context_field_action = Some(action),
                                _ => {}
                            }
                            if is_select && tab.drag_selecting {
                                suppress_select = true;
                            }
                        }
                    })
                    .inner
                })
            } else {
                scroll_area.show_rows(ui, row_height, total_visible, |ui, range| {
                    for vi in range {
                        let i = match &tab.visible_lines {
                            Some(vis) => vis[vi] as usize,
                            None => vi,
                        };
                        let selected = tab.context_line == Some(i);

                        let rendered = render_row(
                            ui,
                            &tab.doc,
                            &Highlights::from_tab(tab),
                            i,
                            selected,
                            font_id.clone(),
                            theme,
                            row_height,
                            selection_range,
                            char_width,
                            gutter_width,
                            tab.log_line_display_mode,
                        );

                        if hovered_annotation.is_none() {
                            hovered_annotation = rendered.hovered;
                        }
                        if let (Some((lo, hi)), Some(anchor_rect)) =
                            (selection_anchor_range, rendered.anchor_rect)
                        {
                            if (lo..=hi).contains(&i) {
                                selection_anchor_rect = Some(match selection_anchor_rect {
                                    Some(current) => current.union(anchor_rect),
                                    None => anchor_rect,
                                });
                            }
                        }
                        let is_select = matches!(rendered.action, Some(RowAction::Select));
                        match rendered.action {
                            Some(RowAction::Select) if !suppress_select => {
                                tab.set_keyword_highlight(None);
                                tab.context_line = Some(i);
                                tab.ensure_visible();
                            }
                            Some(RowAction::OpenOccurrenceOverlay) => {
                                tab.set_keyword_highlight(None);
                                tab.context_line = Some(i);
                                tab.ensure_visible();
                                tab.open_occurrence_overlay(i);
                            }
                            Some(RowAction::Pin) => {
                                if let Some(anchor_rect) = rendered.anchor_rect {
                                    context_pin = Some((i, anchor_rect));
                                }
                            }
                            Some(
                                action @ (RowAction::CopyFull
                                | RowAction::CopyWithoutHeader
                                | RowAction::CopyWithLineNumber),
                            ) => {
                                context_copy = Some((i, action));
                            }
                            Some(RowAction::OpenFullLine) => open_full_line = Some(i),
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
                            Some(RowAction::OpenEmbedded(detection)) => {
                                context_embedded = Some(detection)
                            }
                            Some(
                                action @ (RowAction::SearchField(_) | RowAction::FilterField(_)),
                            ) => context_field_action = Some(action),
                            _ => {}
                        }

                        // If this row was a click but we later determine it was actually a drag,
                        // suppress the select action. For simplicity, we track whether any row
                        // received a click this frame and suppress on the next frame if drag was detected.
                        if is_select && tab.drag_selecting {
                            suppress_select = true;
                        }
                    }
                })
            };
            output
        },
    );
    let output = inner_resp.inner;
    let virtual_top = wrap_offsets
        .as_ref()
        .map(|offsets| {
            offsets
                .partition_point(|offset| *offset <= output.state.offset.y)
                .saturating_sub(1)
                .min(total_visible.saturating_sub(1))
        })
        .unwrap_or_else(|| (output.state.offset.y / row_height).floor() as usize);
    let top_line = tab
        .visible_lines
        .as_ref()
        .and_then(|lines| lines.get(virtual_top).copied().map(|line| line as usize))
        .or_else(|| (virtual_top < n).then_some(virtual_top));
    if let Some(top_line) = top_line {
        tab.scroll_top_line = Some(top_line);
        tab.scroll_fraction = wrap_offsets
            .as_ref()
            .and_then(|offsets| {
                offsets.get(virtual_top..=virtual_top + 1).map(|pair| {
                    ((output.state.offset.y - pair[0]) / (pair[1] - pair[0]).max(1.0))
                        .clamp(0.0, 1.0)
                })
            })
            .unwrap_or_else(|| {
                ((output.state.offset.y / row_height) - virtual_top as f32).clamp(0.0, 1.0)
            });
    }
    tab.log_viewport_height = Some(output.inner_rect.height().max(0.0));

    // Apply deferred context menu actions.
    apply_context_actions(tab, context_pin, context_trim, context_copy, ui.ctx());
    if let Some(detection) = context_embedded {
        tab.embedded_inspector = Some(detection);
    }
    if let Some(action) = context_field_action {
        match action {
            RowAction::SearchField(query) => {
                tab.find_template_id_mode = false;
                tab.find_regex = false;
                tab.find_field_mode = true;
                tab.find_case_sensitive = query.case_sensitive;
                tab.find_input = query.expression();
                tab.start_find(tab.find_input.clone());
                tab.trigger_search_focus();
            }
            RowAction::FilterField(query) => {
                let color = theme.filter_colors[tab.filters.len() % theme.filter_colors.len()];
                if let Err(error) = tab.push_field_filter(query, color) {
                    tab.pending_toast = Some(error);
                }
            }
            _ => {}
        }
    }
    if let Some(line) = open_full_line {
        tab.full_line_inspector = Some(line);
    }
    // Reserve Escape for the active analysis bubble before the independent
    // annotation hover lifecycle gets a chance to consume it.
    let analysis_popup_escape = tab.analysis_popup.is_some()
        && ui
            .ctx()
            .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    let suppress_annotation_hover = tab.drag_selecting
        || tab.pending_selection.is_some()
        || ui
            .input(|input| input.pointer.any_down() || input.smooth_scroll_delta.length_sq() > 0.0);
    annotation_popup::update(
        tab,
        hovered_annotation,
        suppress_annotation_hover,
        ui.ctx(),
        Instant::now(),
    );
    annotation_popup::show(ui, tab, theme);

    // ---- compute viewport_range for timeline shadow ----
    let fully_visible_range = fully_visible_virtual_range(
        output.state.offset.y,
        output.inner_rect.height(),
        row_height,
        wrap_offsets.as_ref().map(|offsets| offsets.as_slice()),
        total_visible,
    );
    update_viewport_range(tab, fully_visible_range, pending);
    if let Some(target_line) = pending {
        let target_virtual = virtual_index_for_line(tab, target_line, total_visible);
        let target_is_fully_visible = target_virtual
            .zip(fully_visible_range)
            .is_some_and(|(target, (first, last))| (first..=last).contains(&target));
        let target_is_taller_than_viewport = wrap_offsets
            .as_deref()
            .and_then(|offsets| target_virtual.map(|target| (offsets, target)))
            .and_then(|(offsets, target)| Some(offsets.get(target + 1)? - offsets.get(target)?))
            .is_some_and(|height| height > output.inner_rect.height());

        if !target_is_fully_visible && target_is_taller_than_viewport {
            // A wrapped logical line taller than the viewport cannot ever be
            // fully shown. Select the nearest line that can be fully shown.
            if let Some((first, last)) = fully_visible_range {
                let replacement = target_virtual.unwrap_or(first).clamp(first, last);
                let replacement = tab
                    .visible_lines
                    .as_ref()
                    .and_then(|visible| visible.get(replacement).copied())
                    .map(|line| line as usize)
                    .unwrap_or(replacement);
                if tab.context_line != Some(replacement) {
                    tab.context_line = Some(replacement);
                    tab.sync_navigation_positions(replacement);
                }
            }
        } else if !target_is_fully_visible && fully_visible_range.is_some() {
            // The pre-layout estimate can be larger than the actual viewport
            // (notably when Horizontal Scroll adds its bottom scrollbar).
            // Re-run navigation once with the measured height.
            tab.pending_scroll = Some(target_line);
            ui.ctx().request_repaint();
        }
    }
    // Once the current viewport has been laid out, keep the selection stable
    // while it remains visible. If it is outside the viewport (most commonly
    // after a manual scroll), move it to the nearest visible edge instead of
    // leaving the UI with no selected row in view.
    if pending.is_none()
        && restored_scroll.is_none()
        && preserve_anchor.is_none()
        && reconcile_selection_after_user_scroll(tab)
    {
        ui.ctx().request_repaint();
    }
    if tab.schedule_embedded_scan() {
        ui.ctx().request_repaint_after(Duration::from_millis(60));
    }

    // ---- arrow-key find navigation ----
    let background_input_blocked = crate::ui::app::overlay::background_input_blocked(ui.ctx());
    if !background_input_blocked
        && !occurrence_overlay_open
        && tab.pin_modal.is_none()
        && tab.pending_selection.is_none()
    {
        let search_has_focus = tab.find_rx.is_some() || !tab.find_query.is_empty();
        // Arrow keys belong to whichever text editor owns keyboard input —
        // not only the Log View find box. This includes Add Filter, notes,
        // custom-date fields, and detached text editors.
        let text_edit_focused = ui.ctx().egui_wants_keyboard_input();
        ui.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft)
                && !text_edit_focused
                && (tab.selected_lane.is_some() || !search_has_focus)
            {
                if tab.selected_lane.is_some() {
                    tab.select_lane_previous();
                } else {
                    tab.find_prev();
                }
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight)
                && !text_edit_focused
                && (tab.selected_lane.is_some() || !search_has_focus)
            {
                if tab.selected_lane.is_some() {
                    tab.select_lane_next();
                } else {
                    tab.find_next();
                }
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) && !text_edit_focused {
                navigate_vertical(tab, false);
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) && !text_edit_focused {
                navigate_vertical(tab, true);
            }
            if should_advance_find_on_enter(
                search_has_focus,
                !tab.find_matches.is_empty(),
                text_edit_focused,
                i.modifiers == egui::Modifiers::NONE && i.key_pressed(egui::Key::Enter),
            ) {
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
                tab.find_next();
            }
        });
    }

    // ---- Esc peels one layer at a time ----
    if !background_input_blocked
        && !occurrence_overlay_open
        && tab.pin_modal.is_none()
        && tab.pending_selection.is_none()
        && ui.input(|i| i.key_pressed(egui::Key::Escape))
    {
        if tab.find_rx.is_some() || !tab.find_query.is_empty() {
            tab.clear_find();
            tab.find_input.clear();
        } else if tab.keyword_highlight.is_some() {
            tab.set_keyword_highlight(None);
        }
    }

    // ---- pointer-driven drag selection ----
    let pointer = ui.input(|i| i.pointer.clone());
    let inner_rect = output.inner_rect;

    if background_input_blocked {
        tab.drag_selecting = false;
        tab.drag_start_pos = None;
        tab.drag_start_line = None;
        tab.drag_current_line = None;
    } else if pointer.primary_pressed()
        && !occurrence_overlay_open
        && !ui.input(|i| i.modifiers.shift)
    {
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
                        wrap_offsets.as_ref().map(|offsets| offsets.as_slice()),
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
            wrap_offsets.as_ref().map(|offsets| offsets.as_slice()),
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

    // ---- analysis bubble for a drag-selected range ----
    if tab.analysis_popup.is_none() {
        if let Some(range) = tab.pending_selection {
            if pending_selection_before == Some(range) {
                let fallback = tab
                    .selection_popup_pos
                    .map(|position| Rect::from_center_size(position, egui::vec2(1.0, 1.0)))
                    .unwrap_or_else(|| {
                        Rect::from_center_size(
                            ui.input(|input| input.pointer.latest_pos().unwrap_or_default()),
                            egui::vec2(1.0, 1.0),
                        )
                    });
                let cursor = tab.selection_popup_pos.unwrap_or_else(|| fallback.center());
                analysis_popup::open_actions(
                    tab,
                    range,
                    cursor,
                    selection_anchor_rect.unwrap_or(fallback),
                );
            } else {
                // Let the selected rows lay out once with the final range so
                // the bubble can be positioned above/below all selected text.
                ui.ctx().request_repaint();
            }
        }
    }

    if let Some(action) = analysis_popup::show(ui, tab, theme, analysis_popup_escape) {
        match action {
            analysis_popup::Action::Save { range, text } => {
                tab.pin_comment = text;
                save_pin(tab, range);
                analysis_popup::clear_after_save(tab);
            }
            analysis_popup::Action::Cancel => analysis_popup::dismiss(tab),
        }
    }

    occurrence_overlay::show(ui, tab, theme);
}

/// Render the pin editing modal (comment + log preview). Called from the app
/// level so it works regardless of which dock tab or detached viewport is
/// focused. New pins from Log View use `analysis_popup`; this remains the
/// detailed editor for existing Pin-panel entries.
pub fn pin_modal_ui(ui: &mut egui::Ui, tab: &mut LogTab, theme: &Theme) {
    let Some(range) = tab.pin_modal else { return };
    let (start, end) = range;
    // Compute the actual visible lines within the range (respects text filters).
    let visible_in_range: Vec<usize> = match &tab.visible_lines {
        Some(vis) => visible_lines_in_range(vis, start, end)
            .iter()
            .map(|&line| line as usize)
            .collect(),
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

    let mut do_save = false;
    let mut do_cancel = false;
    crate::ui::app::overlay::modal(
        ui.ctx(),
        "pin_editor_modal",
        title,
        egui::vec2(500.0, 400.0),
        |ui| {
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
                if icons::action_button(
                    ui,
                    Icon::Save,
                    "Save pin",
                    theme.text,
                    "Save this pinned evidence",
                )
                .clicked()
                {
                    do_save = true;
                }
                if icons::action_button(
                    ui,
                    Icon::Close,
                    "Cancel",
                    theme.text,
                    "Cancel without saving",
                )
                .clicked()
                {
                    do_cancel = true;
                }
            });
        },
    );

    if do_save {
        save_pin(tab, range);
    } else if do_cancel {
        tab.pin_modal = None;
        tab.pin_edit_index = None;
        tab.pin_comment.clear();
    }
}

const INSPECTOR_MIN_SIZE: egui::Vec2 = egui::Vec2::new(520.0, 220.0);
const INSPECTOR_HORIZONTAL_GAP: f32 = 4.0;
const INSPECTOR_VERTICAL_GAP: f32 = 12.0;

/// Full, untruncated source for one Log View row. This is deliberately a
/// plain text inspector (rather than the structured-payload inspector below)
/// so it works for every log format and preserves the exact mmap-backed line.
pub fn full_line_inspector_ui(ui: &mut egui::Ui, tab: &mut LogTab, theme: &Theme) {
    let Some(line) = tab.full_line_inspector else {
        return;
    };
    if line >= tab.doc.total_lines() {
        tab.full_line_inspector = None;
        return;
    }
    let source = tab.doc.line(line).into_owned();
    crate::ui::app::overlay::modal(
        ui.ctx(),
        (
            "full_line_inspector",
            tab.focused_log_view_id,
            tab.doc.trim_start + line,
        ),
        format!("Full line {}", tab.doc.trim_start + line + 1),
        egui::vec2(760.0, 380.0),
        |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{} bytes", source.len()))
                        .monospace()
                        .color(theme.text_muted),
                );
                if icons::action_button(
                    ui,
                    Icon::Copy,
                    "Copy full line",
                    theme.text,
                    "Copy the complete, untruncated source line",
                )
                .clicked()
                {
                    ui.ctx().copy_text(source.clone());
                    tab.pending_toast = Some("Full line copied".to_string());
                }
            });
            ui.separator();
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(&source).family(crate::ui::fonts::log_font_family()),
                        )
                        .selectable(true)
                        .extend(),
                    );
                });
        },
    );
    if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        tab.full_line_inspector = None;
    }
}

/// Persistent structured-data inspector with modal ownership of application
/// input so selecting or copying its contents cannot affect the Log View.
pub fn embedded_data_inspector_ui(ui: &mut egui::Ui, tab: &mut LogTab, theme: &Theme) {
    let Some(detection) = tab.embedded_inspector.take() else {
        return;
    };
    let screen = ui.ctx().content_rect();
    let default_size = inspector_default_size(&detection, tab.embedded_inspector_mode, screen);
    let mut close = false;
    let title = presentation(detection.detector_id).title;
    let modal_response = crate::ui::app::overlay::modal(
        ui.ctx(),
        ("embedded_data_inspector_modal", tab.focused_log_view_id),
        title,
        default_size
            .max(INSPECTOR_MIN_SIZE)
            .min(inspector_max_size(screen)),
        |ui| {
            ui.horizontal(|ui| {
                ui.label(detection.summary());
                ui.separator();
                ui.label(format!(
                    "lines {}–{}",
                    detection.span.start.line + 1,
                    detection.span.end.line + 1
                ));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icons::icon_action_button(ui, Icon::Close, theme.text, "Close inspector")
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
                if presentation.explicit_decode
                    && icons::action_button(
                        ui,
                        Icon::Analysis,
                        "Decode preview",
                        theme.text,
                        "Decode and preview this embedded value",
                    )
                    .clicked()
                {
                    tab.embedded_inspector_mode = EmbeddedInspectorMode::Decoded;
                }
                if icons::action_button(
                    ui,
                    Icon::Copy,
                    "Copy raw",
                    theme.text,
                    "Copy the exact embedded source",
                )
                .clicked()
                {
                    ui.ctx().copy_text(detection.raw.clone());
                    tab.pending_toast = Some("Copied raw embedded data".to_string());
                }
                if icons::action_button(
                    ui,
                    Icon::Copy,
                    "Copy formatted",
                    theme.text,
                    "Copy the formatted embedded value",
                )
                .clicked()
                {
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
                            egui::Label::new(
                                RichText::new(&detection.pretty)
                                    .family(crate::ui::fonts::log_font_family()),
                            )
                            .selectable(true),
                        );
                    }
                    EmbeddedInspectorMode::Raw => {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&detection.raw)
                                    .family(crate::ui::fonts::log_font_family()),
                            )
                            .selectable(true),
                        );
                    }
                    EmbeddedInspectorMode::Decoded => {
                        render_decoded_preview(ui, detection.detector_id, &detection.raw, theme);
                    }
                });
        },
    );
    let escape = ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    let dismissed = escape || modal_response.backdrop_response.clicked();
    if close || dismissed {
        tab.embedded_inspector_anchor = None;
    } else {
        tab.embedded_inspector = Some(detection);
    }
}

pub(super) fn inspector_mode_for(detector_id: &str) -> EmbeddedInspectorMode {
    match presentation(detector_id).primary_tab {
        "Frames" => EmbeddedInspectorMode::Frames,
        "Summary" => EmbeddedInspectorMode::Summary,
        _ => EmbeddedInspectorMode::Tree,
    }
}

pub(super) fn inspector_mode_for_open(
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
fn render_decoded_preview(ui: &mut egui::Ui, detector_id: &str, raw: &str, theme: &Theme) {
    let decoded = decode_embedded_preview(detector_id, raw)
        .unwrap_or_else(|message| format!("Unable to decode preview: {message}"));
    ui.add(
        egui::Label::new(
            RichText::new(decoded)
                .family(crate::ui::fonts::log_font_family())
                .color(theme.log_text),
        )
        .selectable(true),
    );
}

fn decode_embedded_preview(detector_id: &str, raw: &str) -> Result<String, &'static str> {
    let token = raw.trim();
    if detector_id == "binary-plist" {
        return decode_binary_plist_preview(token);
    }
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
        let bytes = decode_hex(hex)?;
        return Ok(String::from_utf8_lossy(&bytes).into_owned());
    }
    decode_base64(token, false)
}

fn decode_binary_plist_preview(token: &str) -> Result<String, &'static str> {
    let (transport, bytes) = if token.starts_with("bplist00") {
        ("literal", token.as_bytes().to_vec())
    } else if let Some(hex) = token.strip_prefix("0x").or(Some(token)).filter(|value| {
        value.len() >= 16
            && value.len() % 2 == 0
            && value.bytes().all(|byte| byte.is_ascii_hexdigit())
    }) {
        ("hex", decode_hex(hex)?)
    } else {
        ("base64", decode_base64_bytes(token, false)?)
    };
    if !bytes.starts_with(b"bplist00") {
        return Err("binary plist magic is missing");
    }
    Ok(format!(
        "Binary property list\nMagic: bplist00\nVersion: 00\nTransport: {transport}\nDecoded bytes: {}\n\nTransport decoded safely. Binary object-table parsing is intentionally not automatic.",
        bytes.len()
    ))
}

fn decode_hex(value: &str) -> Result<Vec<u8>, &'static str> {
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).map_err(|_| "invalid hex"))
        .collect()
}

fn decode_base64(value: &str, url: bool) -> Result<String, &'static str> {
    String::from_utf8(decode_base64_bytes(value, url)?).map_err(|_| "decoded data is binary")
}

fn decode_base64_bytes(value: &str, url: bool) -> Result<Vec<u8>, &'static str> {
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
    Ok(out)
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
        ui.label(
            RichText::new(display)
                .family(crate::ui::fonts::log_font_family())
                .color(theme.log_text),
        );
        if icons::icon_action_button(ui, Icon::Copy, theme.text, "Copy this value").clicked() {
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
        Some(vis) => visible_lines_in_range(vis, start, end)
            .iter()
            .map(|&line| line as usize)
            .collect(),
        None => (start..=end).collect(),
    };
    let comment = tab.pin_comment.trim().to_string();
    let Some(pin) = PinEntry::anchored(&tab.doc, line_numbers, comment) else {
        tab.pending_toast = Some("No visible log lines to pin".to_string());
        return;
    };

    if let Some(idx) = tab.pin_edit_index {
        // Editing an existing pin: replace its content in place.
        if idx < tab.pins.len() {
            tab.pins[idx] = pin;
        }
    } else {
        tab.pins.push(pin);
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

/// Viewer controls use a local compact metric so the text editor, menus, and
/// icon actions share one baseline without changing the spacing of dialogs.
const TOOLBAR_HEIGHT: f32 = 24.0;

/// Inline font-size controls, trim indicator, and search controls.
fn show_toolbar(ui: &mut egui::Ui, tab: &mut LogTab, theme: &Theme, _max_visible_lines: usize) {
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = TOOLBAR_HEIGHT;
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 2.0);
        ui.spacing_mut().button_padding = egui::vec2(6.0, 3.0);
        ui.horizontal(|ui| {
            
            if ui
                .add_sized(
                    egui::vec2(28.0, TOOLBAR_HEIGHT),
                    egui::Button::new("A−").frame_when_inactive(false),
                )
                .on_hover_text("Decrease log text size")
                .clicked()
            {
                tab.log_font_size = (tab.log_font_size - 1.0).max(8.0);
            }
            
            if ui
                .add_sized(
                    egui::vec2(28.0, TOOLBAR_HEIGHT),
                    egui::Button::new("A+").frame_when_inactive(false),
                )
                .on_hover_text("Increase log text size")
                .clicked()
            {
                tab.log_font_size = (tab.log_font_size + 1.0).min(24.0);
            }

            ui.label(
                RichText::new(format!("{:.0}px", tab.log_font_size))
                    .monospace()
                    .color(theme.text_muted),
            );

            if icons::action_button(
                ui,
                Icon::Reset,
                "Reset text size",
                theme.text,
                "Reset this Log View's text size to 12 px",
            )
            .clicked()
            {
                tab.log_font_size = 12.0;
            }

            ui.separator();

            let mut mode = tab.log_line_display_mode;
            let mode_response =
                egui::ComboBox::from_id_salt(("log_line_mode", tab.focused_log_view_id))
                    .selected_text(mode.label())
                    .show_ui(ui, |ui| {
                        for candidate in haystack::core::settings::LogLineDisplayMode::ALL {
                            ui.selectable_value(&mut mode, candidate, candidate.label())
                                .on_hover_text(candidate.description());
                        }
                    })
                    .response;
            mode_response.on_hover_text(mode.description());
            if mode != tab.log_line_display_mode {
                tab.log_line_display_mode = mode;
                tab.wrap_layout = None;
            }

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
                if icons::action_button(
                    ui,
                    Icon::Reset,
                    "Reset trim",
                    theme.text,
                    "Reset trim to show all lines",
                )
                .clicked()
                {
                    tab.handle_trim_reset();
                }
            }

            // Search controls sit left-aligned after viewing controls; Export stays
            // at the far edge without creating a second toolbar row.
            ui.separator();
            ui.add_space(4.0);
            show_search_ui(ui, tab, theme);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                show_export_menu(ui, tab, theme);
            });
        });
    });
}

/// One right-anchored entry point keeps all Log View export scopes discoverable.
fn show_export_menu(ui: &mut egui::Ui, tab: &mut LogTab, theme: &Theme) {
    let button = egui::Button::image_and_text(
        icons::icon_image(ui.ctx(), Icon::Export, 14.0, theme.text),
        "Export",
    )
    .frame_when_inactive(false)
    .min_size(egui::vec2(0.0, TOOLBAR_HEIGHT));
    let (response, _) = egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
        ui.label(RichText::new("Export scope").small().color(theme.text_muted));
        if let Some((start, end)) = tab.pending_selection.or(tab.selection_range) {
            if icons::action_button(
                ui,
                Icon::Export,
                "Selected rows",
                theme.text,
                "Export the selected rows as text",
            )
            .clicked()
            {
                save_text_file(
                    tab,
                    "selected-log.txt",
                    lines_text(tab, start, end, false, false),
                    "Selected rows exported",
                );
                ui.close();
            }
        } else {
            icons::action_button_enabled(
                ui,
                false,
                Icon::Export,
                "Selected rows",
                theme.text,
                "Drag across rows to select an export range",
            );
        }
        if icons::action_button(
            ui,
            Icon::Export,
            "Current view rows",
            theme.text,
            "Export all rows currently in this view; when filters are active, this is the filtered view",
        )
        .clicked()
        {
            let last = tab.doc.total_lines().saturating_sub(1);
            let text = match &tab.visible_lines {
                Some(lines) => lines
                    .iter()
                    .map(|&line| {
                        let line = line as usize;
                        lines_text(tab, line, line, false, false)
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                None => lines_text(tab, 0, last, false, false),
            };
            save_text_file(tab, "current-view.txt", text, "Current view rows exported");
            ui.close();
        }
        if tab.timeline_zoom.is_some() {
            if icons::action_button(
                ui,
                Icon::Export,
                "Current timeline range",
                theme.text,
                "Export rows in the current timeline range",
            )
            .clicked()
            {
                save_text_file(
                    tab,
                    "timeline-range.txt",
                    timeline_range_text(tab),
                    "Timeline range exported",
                );
                ui.close();
            }
        } else {
            icons::action_button_enabled(
                ui,
                false,
                Icon::Export,
                "Current timeline range",
                theme.text,
                "Zoom or brush a timeline range first",
            );
        }
    });
    response
        .on_hover_text("Export selected rows, the current view, or the current timeline range.");
}

/// Return cached estimated wrapped-row offsets. The calculation reads each
/// source line once when width/font/filter membership changes, never while the
/// user scrolls; the ScrollArea then lays out only rows intersecting its
/// viewport.
fn wrap_offsets_for(
    tab: &mut LogTab,
    content_width: f32,
    char_width: f32,
    row_height: f32,
) -> Arc<Vec<f32>> {
    let visible_len = tab
        .visible_lines
        .as_ref()
        .map_or_else(|| tab.doc.total_lines(), |visible| visible.len());
    let first_line = tab
        .visible_lines
        .as_ref()
        .and_then(|visible| visible.first().copied().map(|line| line as usize))
        .or_else(|| (visible_len > 0).then_some(0));
    let last_line = tab
        .visible_lines
        .as_ref()
        .and_then(|visible| visible.last().copied().map(|line| line as usize))
        .or_else(|| visible_len.checked_sub(1));
    let visible_identity = tab
        .visible_lines
        .as_ref()
        .map_or(0, |visible| Arc::as_ptr(visible) as usize);
    if let Some(layout) = &tab.wrap_layout {
        if (layout.content_width - content_width).abs() < 1.0
            && (layout.font_size - tab.log_font_size).abs() < f32::EPSILON
            && layout.visible_len == visible_len
            && layout.visible_identity == visible_identity
            && layout.first_line == first_line
            && layout.last_line == last_line
        {
            return Arc::clone(&layout.offsets);
        }
    }

    // Space Mono is monospace. Using byte length makes this a conservative
    // estimate for non-ASCII source, avoiding visual-row overlap while keeping
    // the cache inexpensive to build.
    let columns = (content_width / char_width.max(1.0)).floor().max(1.0);
    let mut offsets = Vec::with_capacity(visible_len.saturating_add(1));
    offsets.push(0.0);
    let mut total = 0.0;
    for virtual_index in 0..visible_len {
        let line = tab
            .visible_lines
            .as_ref()
            .map_or(virtual_index, |visible| visible[virtual_index] as usize);
        let rows = (tab.doc.line_bytes(line).len() as f32 / columns)
            .ceil()
            .max(1.0);
        total += rows * row_height;
        offsets.push(total);
    }
    let offsets = Arc::new(offsets);
    tab.wrap_layout = Some(crate::ui::app::model::WrapLayout {
        content_width,
        font_size: tab.log_font_size,
        visible_len,
        visible_identity,
        first_line,
        last_line,
        offsets: Arc::clone(&offsets),
    });
    offsets
}

fn wrap_scroll_offset(tab: &LogTab, offsets: &[f32], line: usize) -> Option<f32> {
    let virtual_index = virtual_index_for_line(tab, line, offsets.len().saturating_sub(1))?;
    offsets.get(virtual_index).copied()
}

fn timeline_range_text(tab: &LogTab) -> String {
    let Some((start, end)) = tab.timeline_zoom else {
        return String::new();
    };
    let lines: Vec<usize> = match tab.timeline.domain {
        haystack::core::timeline::TimelineDomain::Sequence => {
            let start = start.max(0) as usize;
            let end = end.max(0) as usize;
            (start..=end.min(tab.doc.total_lines().saturating_sub(1))).collect()
        }
        haystack::core::timeline::TimelineDomain::Time { .. } => (0..tab.doc.total_lines())
            .filter(|&line| {
                tab.doc
                    .ts_at_opt(line)
                    .is_some_and(|ts| ts >= start && ts <= end)
            })
            .collect(),
    };
    lines
        .into_iter()
        .map(|line| lines_text(tab, line, line, false, false))
        .collect::<Vec<_>>()
        .join("\n")
}

fn save_text_file(tab: &mut LogTab, suggested_name: &str, text: String, success: &str) {
    let Some(path) = rfd::FileDialog::new()
        .set_file_name(suggested_name)
        .save_file()
    else {
        return;
    };
    match std::fs::write(path, text) {
        Ok(()) => tab.pending_toast = Some(success.to_string()),
        Err(error) => tab.pending_toast = Some(format!("Export failed: {error}")),
    }
}

/// Render the find/search UI in the toolbar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FindEnterAction {
    Start,
    Next,
}

fn find_enter_action(
    input_has_focus: bool,
    input_lost_focus: bool,
    enter_pressed: bool,
    can_execute: bool,
    query_changed: bool,
    mode_changed: bool,
) -> Option<FindEnterAction> {
    if !enter_pressed || !can_execute || !(input_has_focus || input_lost_focus) {
        return None;
    }
    if query_changed || mode_changed {
        Some(FindEnterAction::Start)
    } else {
        Some(FindEnterAction::Next)
    }
}

fn should_advance_find_on_enter(
    search_active: bool,
    has_matches: bool,
    text_edit_focused: bool,
    enter_pressed: bool,
) -> bool {
    search_active && has_matches && !text_edit_focused && enter_pressed
}

fn has_search_content(find_input: &str, find_query: &str, search_in_flight: bool) -> bool {
    search_in_flight || !find_input.trim().is_empty() || !find_query.is_empty()
}

fn show_search_ui(ui: &mut egui::Ui, tab: &mut LogTab, theme: &Theme) {
    const VALIDATION_DEBOUNCE: Duration = Duration::from_millis(150);
    let search_active = tab.find_rx.is_some() || !tab.find_query.is_empty();
    let search_content =
        has_search_content(&tab.find_input, &tab.find_query, tab.find_rx.is_some());
    if !tab.find_regex && !tab.find_template_id_mode && !tab.find_field_mode {
        tab.find_validate_at = None;
        tab.find_error = None;
        tab.find_error_dismissed = false;
    } else if tab
        .find_validate_at
        .is_some_and(|when| Instant::now() >= when)
    {
        let input = tab.find_input.trim();
        tab.find_error = if input.is_empty() {
            None
        } else if tab.find_field_mode {
            haystack::core::field_query::FieldQuery::parse(input, tab.find_case_sensitive)
                .and_then(|query| query.bind_to_doc(&tab.doc))
                .err()
        } else if tab.find_template_id_mode {
            haystack::core::search::parse_template_id(input).err()
        } else {
            haystack::core::search::validate_regex(input, tab.find_case_sensitive).err()
        };
        tab.find_validate_at = None;
        tab.find_error_dismissed = false;
    }
    let mut input_rect = None;

    ui.horizontal(|ui| {
        ui.add(icons::icon_image(
            ui.ctx(),
            Icon::Search,
            14.0,
            theme.text_muted,
        ))
        .on_hover_text("Find text, expressions, templates, or fields in this view");

        let hint_text = if tab.find_template_id_mode {
            "42, T42, or T{42}"
        } else if tab.find_field_mode {
            "b >= 9 or loglevel = \"FAULT\""
        } else {
            "text or expression"
        };
        let find_input_id = tab.find_input_widget_id();
        let output = egui::TextEdit::singleline(&mut tab.find_input)
            .id(find_input_id)
            .hint_text(hint_text)
            .desired_width(200.0)
            .min_size(egui::vec2(200.0, icons::ACTION_HEIGHT))
            .show(ui);
        let resp_id = output.response.id;
        let input_resp = output.response;
        input_rect = Some(input_resp.rect);

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

        if input_resp.has_focus() && tab.find_input.trim().is_empty() {
            tab.search_suggestions_open = true;
        }
        if input_resp.changed() && !tab.find_input.trim().is_empty() {
            tab.search_suggestions_open = false;
        }
        if input_resp.changed()
            && (tab.find_regex || tab.find_template_id_mode || tab.find_field_mode)
        {
            tab.find_validate_at = Some(Instant::now() + VALIDATION_DEBOUNCE);
            tab.find_error = None;
            tab.find_error_dismissed = false;
        }

        // Keep this Area alive while a suggestion receives its click. Text
        // editors release focus before sibling widgets are evaluated, so
        // tying visibility directly to `has_focus` loses the click.
        if tab.search_suggestions_open
            && tab.find_input.trim().is_empty()
            && ((!tab.find_field_mode && !tab.search_history.is_empty())
                || (tab.doc.record_profile().is_some() && !tab.field_search_history.is_empty()))
            && !tab.find_regex
            && !tab.find_template_id_mode
        {
            let mut chosen = None;
            let mut chosen_field = None;
            let suggestion_area = egui::Area::new(egui::Id::new((
                "log_find_recent_suggestions",
                tab.focused_log_view_id,
            )))
                .order(egui::Order::Foreground)
                .fixed_pos(input_resp.rect.left_bottom())
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_min_width(input_resp.rect.width());
                        ui.set_max_width(input_resp.rect.width());
                        ui.label(RichText::new("Recent searches").weak());
                        if !tab.find_field_mode {
                            for query in &tab.search_history {
                                if suggestion_row::show(ui, query)
                                    .on_hover_text("Run this recent text search")
                                    .clicked()
                                {
                                    chosen = Some(query.clone());
                                }
                            }
                        }
                        if tab.doc.record_profile().is_some() {
                            for query in &tab.field_search_history {
                                if suggestion_row::show(ui, &format!("Field · {}", query.expression()))
                                    .on_hover_text("Run this typed field search")
                                    .clicked()
                                {
                                    chosen_field = Some(query.clone());
                                }
                            }
                        }
                    });
                });
            if let Some(query) = chosen_field {
                tab.find_case_sensitive = query.case_sensitive;
                tab.find_regex = false;
                tab.find_template_id_mode = false;
                tab.find_field_mode = true;
                tab.find_input = query.expression();
                tab.search_suggestions_open = false;
                tab.start_find(tab.find_input.clone());
            } else if let Some(query) = chosen {
                tab.find_input = query.clone();
                tab.search_suggestions_open = false;
                tab.start_find(query);
            } else if ui.input(|input| input.key_pressed(egui::Key::Escape))
                || (ui.input(|input| input.pointer.any_click())
                    && !input_resp.has_focus()
                    && !suggestion_area.response.hovered())
            {
                tab.search_suggestions_open = false;
            }
        }

        let mode_label = if tab.find_template_id_mode {
            "Template ID"
        } else if tab.find_field_mode {
            "Field"
        } else if tab.find_regex {
            "Regex"
        } else if tab.find_case_sensitive {
            "Text (case-sensitive)"
        } else {
            "Text (ignore case)"
        };
        let original_mode = if tab.find_template_id_mode {
            3_u8
        } else if tab.find_field_mode {
            4_u8
        } else if tab.find_regex {
            2
        } else if tab.find_case_sensitive {
            0
        } else {
            1
        };
        let mut mode = original_mode;
        egui::ComboBox::from_id_salt(("log_find_match_mode", tab.focused_log_view_id))
            .selected_text(mode_label)
            .width(150.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut mode, 0, "Text (case-sensitive)")
                    .on_hover_text("Plain text; match case exactly.");
                ui.selectable_value(&mut mode, 1, "Text (ignore case)")
                    .on_hover_text("Plain text; match ASCII case-insensitively.");
                ui.selectable_value(&mut mode, 2, "Regex")
                    .on_hover_text("Rust regular expression; case follows the selected text mode.");
                ui.selectable_value(&mut mode, 3, "Template ID")
                    .on_hover_text("Mined Drain template ID. Enter digits, Tdigits, or T{digits}.");
                ui.selectable_value(&mut mode, 4, "Field")
                    .on_hover_text("Query a captured field, e.g. b >= 9 or loglevel = \"FAULT\".");
            });
        let mode_changed = mode != original_mode;
        if mode_changed {
            match mode {
                0 => {
                    tab.find_regex = false;
                    tab.find_template_id_mode = false;
                    tab.find_field_mode = false;
                    tab.find_case_sensitive = true;
                }
                1 => {
                    tab.find_regex = false;
                    tab.find_template_id_mode = false;
                    tab.find_field_mode = false;
                    tab.find_case_sensitive = false;
                }
                2 => {
                    tab.find_regex = true;
                    tab.find_template_id_mode = false;
                    tab.find_field_mode = false;
                }
                3 => {
                    tab.find_regex = false;
                    tab.find_template_id_mode = true;
                    tab.find_field_mode = false;
                    tab.search_suggestions_open = false;
                }
                _ => {
                    tab.find_regex = false;
                    tab.find_template_id_mode = false;
                    tab.find_field_mode = true;
                    tab.search_suggestions_open = false;
                }
            }
            if tab.find_regex || tab.find_template_id_mode || tab.find_field_mode {
                tab.find_validate_at = Some(Instant::now() + VALIDATION_DEBOUNCE);
                tab.find_error = None;
                tab.find_error_dismissed = false;
                ui.ctx().request_repaint_after(VALIDATION_DEBOUNCE);
            } else {
                tab.find_validate_at = None;
                tab.find_error = None;
            }
        }
        let field_case_changed = tab.find_field_mode
            && ui.checkbox(&mut tab.find_case_sensitive, "Case-sensitive")
                .on_hover_text("Match captured text/path values with exact case; turn off for case-insensitive matching.")
                .clicked();
        if field_case_changed {
            tab.find_validate_at = Some(Instant::now() + VALIDATION_DEBOUNCE);
            ui.ctx().request_repaint_after(VALIDATION_DEBOUNCE);
        }

        let validation_pending = tab.find_validate_at.is_some();
        let can_execute =
            !tab.find_input.trim().is_empty() && !validation_pending && tab.find_error.is_none();
        let trimmed = tab.find_input.trim();
        match find_enter_action(
            input_resp.has_focus(),
            input_resp.lost_focus(),
            ui.input(|i| i.key_pressed(egui::Key::Enter)),
            can_execute,
            trimmed != tab.find_query,
            mode_changed || field_case_changed,
        ) {
            Some(FindEnterAction::Start) => tab.start_find(trimmed.to_string()),
            Some(FindEnterAction::Next) => tab.find_next(),
            None => {}
        }

        let find_status = if tab.find_validate_at.is_some() {
            Some(("Checking…", theme.text_muted, "Validating the current expression."))
        } else if let Some(error) = tab.find_error.as_deref() {
            Some(("Invalid expression", theme.warning, error))
        } else if tab.find_rx.is_some() {
            Some(("Searching…", theme.text_muted, "Searching this view in the background."))
        } else if !tab.find_matches.is_empty() {
            None
        } else if search_active {
            Some(("No matches", theme.warning, "The completed query found no matches."))
        } else {
            None
        };
        if let Some((label, color, tooltip)) = find_status {
            ui.label(RichText::new(label).size(11.0).color(color))
                .on_hover_text(tooltip);
        } else if !tab.find_matches.is_empty() {
            let pos = tab.find_pos.unwrap_or(0);
            let count = if tab.find_active_spec.as_ref().is_some_and(|spec| spec.field_query.is_some()) {
                format!(
                    "{} / {} rows · {} records",
                    pos + 1,
                    tab.find_matches.len(),
                    tab.find_record_count
                )
            } else {
                format!("{} / {}", pos + 1, tab.find_matches.len())
            };
            ui.label(RichText::new(count).size(11.0).color(theme.text_muted));
        }

        let has_matches = !tab.find_matches.is_empty();
        let nav_color = if has_matches {
            theme.text
        } else {
            theme.text_muted
        };
        if ui
            .add_enabled_ui(has_matches, |ui| {
                icons::icon_action_button(ui, Icon::ArrowUp, nav_color, "Previous match (Shift+F3)")
            })
            .inner
            .clicked()
        {
            tab.find_prev();
        }

        if ui
            .add_enabled_ui(has_matches, |ui| {
                icons::icon_action_button(ui, Icon::ArrowDown, nav_color, "Next match (F3)")
            })
            .inner
            .clicked()
        {
            tab.find_next();
        }

        if search_content
            && icons::icon_action_button(ui, Icon::Close, theme.text, "Clear search (Esc)")
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
            && !tab.filters.iter().any(|f| f.text == tab.find_query)
            && !tab.find_active_spec.as_ref().and_then(|spec| spec.field_query.as_ref())
                .is_some_and(|query| tab.filter_field_queries.iter().flatten().any(|existing| existing == query));
        if can_add_filter {
            if ui
                .add(
                    egui::Button::image_and_text(
                        icons::icon_image(ui.ctx(), Icon::Filter, 14.0, theme.text),
                        "Add filter",
                    )
                    .min_size(egui::vec2(0.0, icons::ACTION_HEIGHT)),
                )
                .on_hover_text(format!("Add '{}' as a timeline filter", tab.find_query))
                .clicked()
            {
                let color = theme.filter_colors[tab.filters.len() % theme.filter_colors.len()];
                let Some(spec) = tab.find_active_spec.clone() else {
                    return;
                };
                if let Some(query) = spec.field_query {
                    let _ = tab.push_field_filter(query, color);
                } else if let Some(template_id) = spec.template_id {
                    tab.push_template_filter(template_id, color);
                } else {
                    tab.push_filter_with_options(
                        &spec.text,
                        color,
                        spec.case_sensitive,
                        spec.regex,
                    );
                }
            }
        }

        if tab.find_rx.is_some() {
            ui.spinner();
        }
    });

    if tab.find_field_mode
        && !(tab.search_suggestions_open
            && tab.find_input.trim().is_empty()
            && !tab.field_search_history.is_empty())
    {
        if let Some(anchor) = input_rect {
            if ui.memory(|memory| memory.has_focus(tab.find_input_widget_id())) {
                let current = tab.find_input.clone();
                tab.refresh_field_suggestions(&current);
                if let Some(completed) = crate::ui::field_query_ui::show(
                    ui,
                    "search_field_completion",
                    &tab.doc,
                    &current,
                    tab.field_suggestions.as_ref(),
                    tab.find_error.as_deref(),
                    anchor,
                ) {
                    tab.find_input = completed;
                    tab.find_validate_at = Some(Instant::now() + VALIDATION_DEBOUNCE);
                    ui.ctx().request_repaint_after(VALIDATION_DEBOUNCE);
                }
            }
        }
    }

    if tab.find_validate_at.is_some() {
        ui.ctx().request_repaint_after(VALIDATION_DEBOUNCE);
    } else if !(tab.find_field_mode
        && ui.memory(|memory| memory.has_focus(tab.find_input_widget_id())))
    {
        if let (Some(error), Some(anchor)) = (&tab.find_error, input_rect) {
            let mut bubble_open = !tab.find_error_dismissed;
            error_bubble(
                ui.ctx(),
                ("log_find_input_error", tab.focused_log_view_id),
                anchor,
                BubbleAlign::Below,
                error,
                &mut bubble_open,
            );
            tab.find_error_dismissed = !bubble_open;
        }
    }
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

/// Resolve a real line number to the virtual row used by the Log View.
/// Filtered views use the nearest row when a programmatic jump targets a
/// hidden line; unfiltered views map directly to the source line.
fn virtual_index_for_line(tab: &LogTab, line: usize, total_visible: usize) -> Option<usize> {
    if total_visible == 0 {
        return None;
    }
    match &tab.visible_lines {
        Some(visible) => {
            let index = visible
                .binary_search(&(line as u32))
                .unwrap_or_else(|index| index.min(visible.len().saturating_sub(1)));
            Some(index)
        }
        None => Some(line.min(total_visible.saturating_sub(1))),
    }
}

/// Convert the real-line viewport shadow back to virtual row indices.
fn viewport_virtual_range(tab: &LogTab, total_visible: usize) -> Option<(usize, usize)> {
    let (first, last) = tab.viewport_range?;
    if total_visible == 0 {
        return None;
    }
    match &tab.visible_lines {
        Some(visible) => {
            let first_virtual = visible.partition_point(|&line| (line as usize) < first);
            let end_virtual = visible.partition_point(|&line| (line as usize) <= last);
            (first_virtual < end_virtual).then_some((first_virtual, end_virtual - 1))
        }
        None => Some((first.min(total_visible - 1), last.min(total_visible - 1))),
    }
}

fn max_scroll_offset(total_visible: usize, row_height: f32, avail_height: f32) -> f32 {
    (total_visible as f32 * row_height - avail_height.max(0.0)).max(0.0)
}

/// Compute the deterministic scroll offset for a pending selection.
///
/// A target already in the viewport does not move it. Nearby targets are
/// revealed with the smallest possible scroll, while distant targets are
/// centered for orientation.
fn compute_pending_scroll_offset(
    tab: &LogTab,
    pending: Option<usize>,
    row_height: f32,
    avail_height: f32,
    total_visible: usize,
) -> Option<f32> {
    let line = pending?;
    let target = virtual_index_for_line(tab, line, total_visible)?;
    let max_offset = max_scroll_offset(total_visible, row_height, avail_height);

    // Prefer the persisted physical offset when available. `viewport_range`
    // intentionally includes rows that only partially intersect the viewport,
    // which can otherwise make a selected row look visible while its text is
    // clipped at the top or bottom edge.
    if let Some(current_offset) = current_fixed_scroll_offset(tab, row_height, total_visible) {
        let viewport_height = avail_height.max(row_height);
        let target_start = target as f32 * row_height;
        let target_end = target_start + row_height;
        let viewport_end = current_offset + viewport_height;
        if target_start >= current_offset && target_end <= viewport_end {
            return None;
        }

        let target_above = target_start < current_offset;
        let distance = if target_above {
            ((current_offset - target_end) / row_height.max(1.0)).ceil() as usize
        } else {
            ((target_start - viewport_end) / row_height.max(1.0)).ceil() as usize
        };
        if distance <= NEAR_SELECTION_SCROLL_LINES {
            let margin = SELECTION_SCROLL_MARGIN_LINES as f32 * row_height;
            let safe_edge_offset = if target_above {
                target_start - margin
            } else {
                target_end - viewport_height + margin
            };
            return Some(safe_edge_offset.clamp(0.0, max_offset));
        }

        let center_offset = target_start - (viewport_height - row_height) * 0.5;
        return Some(center_offset.clamp(0.0, max_offset));
    }

    let Some((first, last)) = viewport_virtual_range(tab, total_visible) else {
        let center_offset =
            target as f32 * row_height - (avail_height.max(row_height) - row_height) * 0.5;
        return Some(center_offset.clamp(0.0, max_offset));
    };

    if (first..=last).contains(&target) {
        return None;
    }

    let (distance, edge_offset) = if target < first {
        (first - target, target as f32 * row_height)
    } else {
        (
            target - last,
            target as f32 * row_height - (avail_height.max(row_height) - row_height),
        )
    };
    if distance <= NEAR_SELECTION_SCROLL_LINES {
        let margin = SELECTION_SCROLL_MARGIN_LINES as f32 * row_height;
        let safe_edge_offset = if target < first {
            edge_offset - margin
        } else {
            edge_offset + margin
        };
        return Some(safe_edge_offset.clamp(0.0, max_offset));
    }

    let center_offset =
        target as f32 * row_height - (avail_height.max(row_height) - row_height) * 0.5;
    Some(center_offset.clamp(0.0, max_offset))
}

/// Return the current fixed-row scroll offset from the logical anchor stored
/// after the previous Log View layout pass.
fn current_fixed_scroll_offset(tab: &LogTab, row_height: f32, total_visible: usize) -> Option<f32> {
    let line = tab.scroll_top_line?;
    let index = virtual_index_for_line(tab, line, total_visible)?;
    Some((index as f32 + tab.scroll_fraction.clamp(0.0, 1.0)) * row_height)
}

/// Return the current content offset for a wrapped Log View from its persisted
/// logical line/fraction state.
fn current_wrap_scroll_offset(tab: &LogTab, offsets: &[f32]) -> Option<f32> {
    let line = tab.scroll_top_line?;
    let index = virtual_index_for_line(tab, line, offsets.len().saturating_sub(1))?;
    let start = *offsets.get(index)?;
    let end = *offsets.get(index + 1).unwrap_or(&start);
    Some(start + (end - start) * tab.scroll_fraction.clamp(0.0, 1.0))
}

/// Wrapped-row equivalent of `compute_pending_scroll_offset`.
fn compute_pending_wrap_scroll_offset(
    tab: &LogTab,
    pending: Option<usize>,
    offsets: &[f32],
    avail_height: f32,
    row_height: f32,
    total_visible: usize,
) -> Option<f32> {
    let line = pending?;
    let target = virtual_index_for_line(tab, line, total_visible)?;
    let target_start = *offsets.get(target)?;
    let target_end = *offsets.get(target + 1).unwrap_or(&target_start);
    let max_offset = (offsets.last().copied().unwrap_or(0.0) - avail_height.max(0.0)).max(0.0);

    let Some(current_offset) = current_wrap_scroll_offset(tab, offsets) else {
        let center = (target_start + target_end) * 0.5 - avail_height.max(row_height) * 0.5;
        return Some(center.clamp(0.0, max_offset));
    };
    let viewport_height = avail_height.max(row_height);
    let viewport_end = current_offset + viewport_height;
    if target_start >= current_offset && target_end <= viewport_end {
        return None;
    }

    let target_above = target_start < current_offset;
    let (distance, edge_offset) = if target_above {
        (
            ((current_offset - target_end) / row_height.max(1.0)).ceil() as usize,
            target_start,
        )
    } else {
        let target_height = target_end - target_start;
        (
            ((target_start - viewport_end) / row_height.max(1.0)).ceil() as usize,
            if target_height <= avail_height.max(0.0) {
                target_end - viewport_height
            } else {
                target_start
            },
        )
    };
    if distance <= NEAR_SELECTION_SCROLL_LINES {
        let margin = SELECTION_SCROLL_MARGIN_LINES as f32 * row_height;
        let safe_edge_offset = if target_above {
            edge_offset - margin
        } else {
            edge_offset + margin
        };
        return Some(safe_edge_offset.clamp(0.0, max_offset));
    }

    let center = (target_start + target_end) * 0.5 - avail_height.max(row_height) * 0.5;
    Some(center.clamp(0.0, max_offset))
}

fn compute_restore_scroll_offset(
    tab: &LogTab,
    anchor: usize,
    fraction: f32,
    row_height: f32,
    total_visible: usize,
) -> Option<f32> {
    let virtual_idx = match &tab.visible_lines {
        Some(lines) => lines
            .binary_search(&(anchor as u32))
            .unwrap_or_else(|idx| idx.min(lines.len().saturating_sub(1))),
        None => anchor.min(total_visible.saturating_sub(1)),
    };
    Some((virtual_idx as f32 + fraction.clamp(0.0, 1.0)) * row_height)
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
        Some(ref vis) => match vis.binary_search(&(anchor as u32)) {
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
    wrap_offsets: Option<&[f32]>,
    total_visible: usize,
    visible_lines: &Option<Arc<Vec<u32>>>,
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
    let virtual_idx = wrap_offsets
        .map(|offsets| {
            offsets
                .partition_point(|offset| *offset <= relative_y)
                .saturating_sub(1)
        })
        .unwrap_or_else(|| (relative_y / row_height).floor() as usize);
    let virtual_idx = virtual_idx.min(total_visible.saturating_sub(1));
    match visible_lines {
        Some(vis) => vis.get(virtual_idx).copied().map(|line| line as usize),
        None => Some(virtual_idx),
    }
}

/// The filtered visible-line list is sorted by real line number. Range
/// selection and pinning should therefore use two binary searches instead of
/// walking every visible line each frame.
fn visible_lines_in_range(lines: &[u32], start: usize, end: usize) -> &[u32] {
    let first = lines.partition_point(|&line| (line as usize) < start);
    let last = lines.partition_point(|&line| (line as usize) <= end);
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
    display_mode: haystack::core::settings::LogLineDisplayMode,
) -> RowRenderResult {
    let mut action: Option<RowAction> = None;
    let mut hovered: Option<annotation_popup::Candidate> = None;
    let mut anchor_rect: Option<Rect> = None;

    let in_selection = selection_range.is_some_and(|(lo, hi)| idx >= lo && idx <= hi);
    let selected_bg = if ui.ctx().input(|input| input.focused) {
        theme.selection_focused
    } else {
        theme.selection_unfocused
    };
    let bg = if selected {
        selected_bg
    } else if in_selection {
        theme.selection_range_bg
    } else {
        Color32::TRANSPARENT
    };

    // Paint the selection across the complete row. The small left marker keeps
    // the selected source line discoverable without turning the gutter into a
    // second, competing highlight surface.
    let row_rect = Rect::from_min_size(
        ui.cursor().min,
        egui::vec2(ui.available_width(), row_height),
    );
    if bg != Color32::TRANSPARENT {
        ui.painter()
            .rect_filled(row_rect, egui::CornerRadius::ZERO, bg);
        if selected {
            ui.painter().rect_filled(
                Rect::from_min_max(
                    row_rect.left_top(),
                    Pos2::new(row_rect.left() + 3.0, row_rect.bottom()),
                ),
                egui::CornerRadius::ZERO,
                theme.accent,
            );
        }
    }

    // Build the log content job (no line number, no color marker).
    let job = line_job_for_mode(
        doc,
        highlights,
        idx,
        selected,
        font_id.clone(),
        theme,
        display_mode,
    );

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
                            &format!(
                                "{:>width$}: ",
                                idx + 1,
                                width = doc.total_lines().max(1).to_string().len()
                            ),
                            0.0,
                            line_num_fmt,
                        );
                        ui.add(egui::Label::new(line_num_job).selectable(false));
                    });
                },
            );

            let original_line = doc.trim_start + idx;
            let detections = highlights.embedded.unwrap_or_default();

            // Log content label — selectable and clickable for row actions. `line_job` already
            // bakes the correct per-section background (selection tint on
            // non-highlighted spans, search/keyword highlight colours on
            // matches); do NOT overwrite it, or highlights are erased.
            let content_job = job;
            let content_resp = ui.add(log_content_label(
                content_job,
                display_mode == haystack::core::settings::LogLineDisplayMode::Wrap,
            ));
            anchor_rect = Some(content_resp.rect);

            if display_mode == haystack::core::settings::LogLineDisplayMode::Truncate
                && doc.line(idx).len() > highlight::MAX_DISPLAY_BYTES
            {
                let preview_len = highlight::display_source_len(&doc.line(idx));
                let beyond_match = highlights.search_matcher.is_some_and(|matcher| {
                    search_match_beyond_preview(doc.line(idx).as_ref(), preview_len, matcher)
                });
                let label = if beyond_match { "… match" } else { "…" };
                let badge = egui::Button::new(
                    RichText::new(label)
                        .monospace()
                        .size((font_id.size * 0.78).max(8.0))
                        .color(if beyond_match {
                            theme.warning
                        } else {
                            theme.text_muted
                        }),
                )
                .small();
                let badge_response = ui.put(
                    Rect::from_min_size(
                        Pos2::new(content_resp.rect.right() - 52.0, content_resp.rect.top()),
                        egui::vec2(52.0, row_height),
                    ),
                    badge,
                )
                .on_hover_text(if beyond_match {
                    "A search match continues beyond the rendered preview. Open the complete line."
                } else {
                    "This line is previewed. Open the complete line."
                });
                if badge_response.clicked() {
                    action = Some(RowAction::OpenFullLine);
                }
            }

            let visible_source_len = highlight::display_source_len(&doc.line(idx));
            if let Some((_, range)) = doc.explicit_timestamp_at(idx) {
                let displayed_range =
                    range.start.min(visible_source_len)..range.end.min(visible_source_len);
                if displayed_range.start < displayed_range.end {
                    if let Some(source_rect) = source_range_rect(
                        ui,
                        doc.line_bytes(idx),
                        displayed_range,
                        content_resp.rect,
                        &font_id,
                        theme.log_text,
                    ) {
                        let span = SourceSpan {
                            start: SourcePos {
                                line: original_line,
                                byte: range.start,
                            },
                            end: SourcePos {
                                line: original_line,
                                byte: range.end,
                            },
                        };
                        let response = ui.interact(
                            source_rect.expand2(egui::vec2(1.0, 2.0)),
                            ui.make_persistent_id((
                                "timestamp_source_hover",
                                original_line,
                                range.start,
                            )),
                            egui::Sense::hover(),
                        );
                        paint_annotation_cue(ui, source_rect, theme.timestamp, response.hovered());
                        if response.hovered() {
                            hovered = Some(annotation_popup::Candidate {
                                key: AnnotationHoverKey::Timestamp { span },
                                source_rect,
                            });
                        }
                    }
                }
            }

            for detection in detections {
                let key = AnnotationHoverKey::for_detection(detection);
                for (range_index, range) in
                    highlight::source_ranges_for_line(detection, original_line, visible_source_len)
                        .into_iter()
                        .enumerate()
                {
                    let Some(source_rect) = source_range_rect(
                        ui,
                        doc.line_bytes(idx),
                        range,
                        content_resp.rect,
                        &font_id,
                        theme.log_text,
                    ) else {
                        continue;
                    };
                    let hover_id = ui.make_persistent_id((
                        "embedded_source_hover",
                        detection.detector_id,
                        detection.span.start.line,
                        detection.span.start.byte,
                        range_index,
                    ));
                    let response = ui.interact(
                        source_rect.expand2(egui::vec2(1.0, 2.0)),
                        hover_id,
                        egui::Sense::hover(),
                    );
                    paint_annotation_cue(ui, source_rect, theme.embedded_data, response.hovered());
                    if response.hovered() && hovered.is_none() {
                        hovered = Some(annotation_popup::Candidate { key, source_rect });
                    }
                }
            }

            // Shift-click is a read-focused occurrence context. It takes
            // priority over double-click keyword highlighting.
            if content_resp.clicked() && ui.input(|input| input.modifiers.shift) {
                action = Some(RowAction::OpenOccurrenceOverlay);
            // Double-click: keyword highlight
            } else if content_resp.double_clicked() {
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
                ui.set_min_width(210.0);
                if icons::action_button(ui, Icon::Pin, "Pin", theme.text, "Pin this log line")
                    .clicked()
                {
                    action = Some(RowAction::Pin);
                    ui.close();
                }
                ui.separator();
                if let Some(detection) = detections
                    .iter()
                    .find(|detection| detection.span.includes_line(original_line))
                {
                    if icons::action_button(
                        ui,
                        Icon::Analysis,
                        "Inspect structured data",
                        theme.text,
                        "Open the structured-data inspector for this row",
                    )
                    .clicked()
                    {
                        action = Some(RowAction::OpenEmbedded(detection.clone()));
                        ui.close();
                    }
                    ui.separator();
                }
                if doc.record_profile().is_some() {
                    ui.menu_button("Captured fields", |ui| {
                        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                            let original = doc.trim_start + idx;
                            for (field, kind) in doc.record_field_schema() {
                                let Some(span) = doc.record_field_span(original, field) else { continue };
                                let Some(bytes) = doc.source_bytes(span) else { continue };
                                let preview: String = String::from_utf8_lossy(&bytes[..bytes.len().min(256)])
                                    .chars().take(48).collect();
                                ui.menu_button(format!("{field}: {preview}"), |ui| {
                                    if ui.button("Copy value").clicked() {
                                        ui.ctx().copy_text(String::from_utf8_lossy(bytes).into_owned());
                                        ui.close();
                                    }
                                    if !crate::ui::field_query_ui::is_field_criteria_kind(kind) {
                                        ui.label("Use Text or Regex to search the log message.");
                                    } else if bytes.len() <= 4096 {
                                        let value = String::from_utf8_lossy(bytes).into_owned();
                                        let (operator, value) = if kind == "timestamp" {
                                            let zoned = chrono::DateTime::parse_from_rfc3339(&value)
                                                .or_else(|_| chrono::DateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S%.f%z"));
                                            if zoned.is_ok() {
                                                (haystack::core::field_query::FieldOperator::Equal, value)
                                            } else {
                                                doc.explicit_time_untrimmed(original)
                                                    .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
                                                    .map_or((haystack::core::field_query::FieldOperator::Exists, String::new()),
                                                        |time| (haystack::core::field_query::FieldOperator::Equal, time.to_rfc3339()))
                                            }
                                        } else {
                                            (haystack::core::field_query::FieldOperator::Equal, value)
                                        };
                                        let query = haystack::core::field_query::FieldQuery {
                                            field: field.to_owned(),
                                            operator,
                                            value,
                                            case_sensitive: true,
                                            field_type: Some(kind.to_owned()),
                                        };
                                        if ui.button("Search this field value").clicked() {
                                            action = Some(RowAction::SearchField(query.clone()));
                                            ui.close();
                                        }
                                        if ui.button("Add field filter").clicked() {
                                            action = Some(RowAction::FilterField(query));
                                            ui.close();
                                        }
                                    } else {
                                        ui.label("Value is too long for a one-click field query.");
                                    }
                                });
                            }
                        });
                    });
                    ui.separator();
                }
                if icons::action_button(
                    ui,
                    Icon::ExternalWindow,
                    "Open full line",
                    theme.text,
                    "Inspect the complete, untruncated line",
                )
                .clicked()
                {
                    action = Some(RowAction::OpenFullLine);
                    ui.close();
                }
                ui.separator();
                if icons::action_button(
                    ui,
                    Icon::Copy,
                    "Copy full line",
                    theme.text,
                    "Copy this complete log line",
                )
                .clicked()
                {
                    action = Some(RowAction::CopyFull);
                    ui.close();
                }
                if icons::action_button(
                    ui,
                    Icon::Copy,
                    "Copy without timestamp/header",
                    theme.text,
                    "Copy only the log message body",
                )
                .clicked()
                {
                    action = Some(RowAction::CopyWithoutHeader);
                    ui.close();
                }
                if icons::action_button(
                    ui,
                    Icon::Copy,
                    "Copy with line number",
                    theme.text,
                    "Copy this line with its source line number",
                )
                .clicked()
                {
                    action = Some(RowAction::CopyWithLineNumber);
                    ui.close();
                }
                ui.separator();
                if icons::action_button(
                    ui,
                    Icon::Trim,
                    "Trim top",
                    theme.text,
                    "Remove all lines before this one",
                )
                .clicked()
                {
                    action = Some(RowAction::TrimLeft);
                    ui.close();
                }
                if icons::action_button(
                    ui,
                    Icon::Trim,
                    "Trim bottom",
                    theme.text,
                    "Remove all lines after this one",
                )
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

    RowRenderResult {
        action,
        hovered,
        anchor_rect,
    }
}

/// Keep native egui text selection enabled for log content. Whole-line drag
/// selection remains handled by `show` for pinning selected rows.
fn log_content_label(job: egui::text::LayoutJob, wrap: bool) -> egui::Label {
    if wrap {
        egui::Label::new(job).selectable(true).wrap()
    } else {
        // Virtual rows have a fixed height, so log content must never wrap into
        // the next row. The enclosing scroll area clips horizontally.
        egui::Label::new(job).selectable(true).extend()
    }
}

/// Whether a full-line search hit would be hidden by a truncated preview.
fn search_match_beyond_preview(
    line: &str,
    preview_len: usize,
    matcher: &haystack::core::search::FilterHighlighter,
) -> bool {
    matcher
        .spans(line)
        .into_iter()
        .any(|(_, matched)| matched.end > preview_len)
}

fn source_prefix_at(bytes: &[u8], byte_offset: usize) -> Option<&str> {
    std::str::from_utf8(bytes.get(..byte_offset)?).ok()
}

fn source_range_rect(
    ui: &egui::Ui,
    bytes: &[u8],
    range: std::ops::Range<usize>,
    content_rect: Rect,
    font_id: &FontId,
    color: Color32,
) -> Option<Rect> {
    let start = source_prefix_at(bytes, range.start)?;
    let end = source_prefix_at(bytes, range.end)?;
    let (start_width, end_width) = ui.ctx().fonts_mut(|fonts| {
        (
            fonts
                .layout_no_wrap(start.to_owned(), font_id.clone(), color)
                .size()
                .x,
            fonts
                .layout_no_wrap(end.to_owned(), font_id.clone(), color)
                .size()
                .x,
        )
    });
    if end_width <= start_width {
        return None;
    }
    let rect = Rect::from_min_max(
        Pos2::new(content_rect.left() + start_width, content_rect.top()),
        Pos2::new(content_rect.left() + end_width, content_rect.bottom()),
    )
    .intersect(ui.clip_rect());
    (rect.width() > 0.0 && rect.height() > 0.0).then_some(rect)
}

fn line_gutter_width(char_width: f32, total_lines: usize) -> f32 {
    let digits = total_lines.max(1).to_string().len();
    char_width * (digits as f32 + 2.0) + GUTTER_PADDING
}

fn paint_annotation_cue(ui: &egui::Ui, rect: Rect, color: Color32, emphasized: bool) {
    let color = Color32::from_rgba_unmultiplied(
        color.r(),
        color.g(),
        color.b(),
        if emphasized { 220 } else { 135 },
    );
    let y = rect.bottom() - if emphasized { 1.0 } else { 1.5 };
    let radius = if emphasized { 1.0 } else { 0.7 };
    let mut x = rect.left() + 1.0;
    while x < rect.right() {
        ui.painter().circle_filled(Pos2::new(x, y), radius, color);
        x += if emphasized { 3.0 } else { 4.0 };
    }
}

/// Apply the deferred pin / trim actions from the context menu.
/// Single-line right-click "📌 Pin" opens the analysis bubble.
fn apply_context_actions(
    tab: &mut LogTab,
    context_pin: Option<(usize, Rect)>,
    context_trim: Option<TrimAction>,
    context_copy: Option<(usize, RowAction)>,
    ctx: &egui::Context,
) {
    if let Some((line, anchor_rect)) = context_pin {
        analysis_popup::open_editor(tab, (line, line), anchor_rect, false);
        tab.bottom_panel_open = true;
    }
    if let Some(action) = context_trim {
        tab.handle_trim(action);
    }
    if let Some((line, action)) = context_copy {
        let (numbers, without_header) = match action {
            RowAction::CopyFull => (false, false),
            RowAction::CopyWithoutHeader => (false, true),
            RowAction::CopyWithLineNumber => (true, false),
            _ => return,
        };
        ctx.copy_text(lines_text(tab, line, line, numbers, without_header));
        tab.pending_toast = Some("Line copied".to_string());
    }
}

/// Clipboard-friendly rows. `start`/`end` are trim-relative, inclusive.
pub(crate) fn lines_text(
    tab: &LogTab,
    start: usize,
    end: usize,
    with_line_numbers: bool,
    without_timestamp: bool,
) -> String {
    let mut out = String::new();
    for line in start..=end.min(tab.doc.total_lines().saturating_sub(1)) {
        if !out.is_empty() {
            out.push('\n');
        }
        if with_line_numbers {
            out.push_str(&(tab.doc.trim_start + line + 1).to_string());
            out.push_str(": ");
        }
        let source = tab.doc.line(line);
        let text = if without_timestamp {
            tab.doc
                .explicit_timestamp_at(line)
                .map_or(source.as_ref(), |(_, range)| {
                    source[range.end.min(source.len())..].trim_start()
                })
        } else {
            source.as_ref()
        };
        out.push_str(text);
    }
    out
}

/// Return the virtual rows whose complete geometry fits inside the actual
/// scroll viewport. Virtualization deliberately renders extra rows around the
/// viewport, so its render range must never be used as a selectable range.
fn fully_visible_virtual_range(
    scroll_offset: f32,
    viewport_height: f32,
    row_height: f32,
    wrap_offsets: Option<&[f32]>,
    total_visible: usize,
) -> Option<(usize, usize)> {
    if total_visible == 0 || row_height <= 0.0 || viewport_height <= 0.0 {
        return None;
    }
    let top = scroll_offset.max(0.0);
    let bottom = top + viewport_height;
    // Only absorb floating-point noise; a tenth of a pixel must still count
    // as clipped rather than being rounded into the selectable range.
    const EPSILON: f32 = 0.001;

    if let Some(offsets) = wrap_offsets {
        if offsets.len() < total_visible + 1 {
            return None;
        }
        let first = offsets
            .partition_point(|offset| *offset < top - EPSILON)
            .min(total_visible);
        let end = offsets
            .partition_point(|offset| *offset <= bottom + EPSILON)
            .saturating_sub(1)
            .min(total_visible);
        return (first < end).then_some((first, end - 1));
    }

    let first = ((top / row_height) - EPSILON).ceil().max(0.0) as usize;
    let end = ((bottom / row_height) + EPSILON).floor().max(0.0) as usize;
    let first = first.min(total_visible);
    let end = end.min(total_visible);
    (first < end).then_some((first, end - 1))
}

/// Update `tab.viewport_range` for the timeline shadow using only completely
/// visible rows. The rendered range is intentionally larger because
/// virtualization overscans at both edges.
fn update_viewport_range(
    tab: &mut LogTab,
    fully_visible_range: Option<(usize, usize)>,
    pending: Option<usize>,
) {
    let previous_range = tab.viewport_range;
    tab.viewport_range =
        fully_visible_range.and_then(|(first_virtual, last_virtual)| match &tab.visible_lines {
            Some(visible) => Some((
                *visible.get(first_virtual)? as usize,
                *visible.get(last_virtual)? as usize,
            )),
            None => Some((first_virtual, last_virtual)),
        });

    // Follow an actual Log View movement or an explicit navigation request.
    // Calling this unconditionally made a manual timeline pan snap back on the
    // next frame even though the log viewport itself had not moved.
    if pending.is_some() || tab.viewport_range != previous_range {
        tab.ensure_viewport_visible();
    }
}

/// If the user manually scrolls away from the selected row, keep the
/// selection attached to the nearest visible row. This deliberately does not
/// create a new scroll request: the user's viewport is authoritative.
fn reconcile_selection_after_user_scroll(tab: &mut LogTab) -> bool {
    let Some(selected) = tab.context_line else {
        return false;
    };
    let Some((first, last)) = tab.viewport_range else {
        return false;
    };

    let replacement = match &tab.visible_lines {
        Some(visible) => {
            let first_virtual = visible.partition_point(|&line| (line as usize) < first);
            let end_virtual = visible.partition_point(|&line| (line as usize) <= last);
            if first_virtual >= end_virtual {
                return false;
            }
            match visible.binary_search(&(selected as u32)) {
                Ok(selected_virtual)
                    if (first_virtual..end_virtual).contains(&selected_virtual) =>
                {
                    None
                }
                Ok(selected_virtual) if selected_virtual < first_virtual => {
                    let index =
                        (first_virtual + SELECTION_VIEWPORT_MARGIN_LINES).min(end_virtual - 1);
                    Some(visible[index] as usize)
                }
                Ok(_) => {
                    let index = (end_virtual - 1).saturating_sub(SELECTION_VIEWPORT_MARGIN_LINES);
                    Some(visible[index.max(first_virtual)] as usize)
                }
                Err(insertion) => {
                    let lower = insertion.saturating_sub(1).max(first_virtual);
                    let upper = insertion.min(end_virtual - 1);
                    let lower_line = visible[lower] as usize;
                    let upper_line = visible[upper] as usize;
                    let nearest = if selected.abs_diff(lower_line) <= selected.abs_diff(upper_line)
                    {
                        lower
                    } else {
                        upper
                    };
                    let nearest = if nearest == first_virtual {
                        (nearest + SELECTION_VIEWPORT_MARGIN_LINES).min(end_virtual - 1)
                    } else if nearest == end_virtual - 1 {
                        nearest
                            .saturating_sub(SELECTION_VIEWPORT_MARGIN_LINES)
                            .max(first_virtual)
                    } else {
                        nearest
                    };
                    Some(visible[nearest] as usize)
                }
            }
        }
        None => {
            if (first..=last).contains(&selected) {
                None
            } else if selected < first {
                Some((first + SELECTION_VIEWPORT_MARGIN_LINES).min(last))
            } else {
                Some(
                    last.saturating_sub(SELECTION_VIEWPORT_MARGIN_LINES)
                        .max(first),
                )
            }
        }
    };

    let Some(replacement) = replacement else {
        return false;
    };
    if replacement == selected {
        return false;
    }
    tab.context_line = Some(replacement);
    tab.sync_navigation_positions(replacement);
    true
}

#[cfg(test)]
mod tests {
    use super::highlight::{display_text, MAX_DISPLAY_BYTES};
    use super::*;
    use crate::ui::app::model::Filter;

    #[test]
    fn log_scroll_identity_is_distinct_for_each_log_view() {
        use crate::ui::app::model::LogViewId;

        assert_ne!(
            egui::Id::new(("log_scroll", LogViewId(1))),
            egui::Id::new(("log_scroll", LogViewId(2)))
        );
    }

    #[test]
    fn focused_search_enter_advances_an_existing_query() {
        assert_eq!(
            find_enter_action(true, false, true, true, false, false),
            Some(FindEnterAction::Next)
        );
        assert_eq!(
            find_enter_action(true, false, true, true, true, false),
            Some(FindEnterAction::Start)
        );
        assert_eq!(
            find_enter_action(false, false, true, true, false, false),
            None
        );
    }

    #[test]
    fn clear_search_is_visible_only_when_search_has_content() {
        assert!(!has_search_content("", "", false));
        assert!(has_search_content("error", "", false));
        assert!(has_search_content("", "error", false));
        assert!(has_search_content("", "", true));
    }

    #[test]
    fn enter_advances_an_active_search_from_the_selected_log_view() {
        assert!(should_advance_find_on_enter(true, true, false, true));
        assert!(!should_advance_find_on_enter(true, false, false, true));
        assert!(!should_advance_find_on_enter(true, true, true, true));
        assert!(!should_advance_find_on_enter(false, true, false, true));
        assert!(!should_advance_find_on_enter(true, true, false, false));
    }

    fn write_temp(content: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "haystack_log_view_test_{}_{}.log",
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
            unanchored: false,
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
    fn save_pin_uses_first_and_last_visible_selected_rows_for_timestamps() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z zero\n\
             2026-07-19T10:00:01.000Z one\n\
             2026-07-19T10:00:02.000Z two\n\
             2026-07-19T10:00:03.000Z three\n",
        );
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.visible_lines = Some(Arc::new(vec![1, 3]));

        // The selection range includes hidden rows 0 and 2. The pin must use
        // the two actual selected rows for its timeline endpoints.
        save_pin(&mut tab, (0, 3));

        let pin = &tab.pins[0];
        assert_eq!(pin.line_numbers, vec![1, 3]);
        assert_eq!(pin.start_line, 1);
        assert_eq!(pin.start_ts, tab.doc.ts_at(1));
        assert_eq!(pin.end_ts, tab.doc.ts_at(3));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn save_pin_without_comment_still_creates_an_analysis_pin() {
        let path = write_temp("2026-07-19T10:00:00.000Z INFO alpha\n");
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        save_pin(&mut tab, (0, 0));

        assert_eq!(tab.pins.len(), 1);
        assert!(tab.pins[0].comment.is_empty());
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
    fn pending_selection_scrolls_only_as_far_as_needed() {
        let content = (0..20)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        let path = write_temp(&content);
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.viewport_range = Some((10, 14));

        // The selected line is already visible, so navigation must not move
        // the viewport at all.
        assert_eq!(
            compute_pending_scroll_offset(&tab, Some(12), 10.0, 50.0, 20),
            None
        );
        // A rendered edge row can still be clipped when the viewport starts
        // part-way through it; nudge it fully into view with two rows of air.
        tab.scroll_top_line = Some(10);
        tab.scroll_fraction = 0.5;
        assert_eq!(
            compute_pending_scroll_offset(&tab, Some(10), 10.0, 50.0, 20),
            Some(80.0)
        );
        tab.scroll_fraction = 0.0;
        // Five rows away uses the nearest edge, with two safety rows inside
        // the viewport so the target is not clipped.
        assert_eq!(
            compute_pending_scroll_offset(&tab, Some(5), 10.0, 50.0, 20),
            Some(30.0)
        );
        assert_eq!(
            compute_pending_scroll_offset(&tab, Some(15), 10.0, 50.0, 20),
            Some(130.0)
        );
        // Six rows away uses the center policy.
        assert_eq!(
            compute_pending_scroll_offset(&tab, Some(4), 10.0, 50.0, 20),
            Some(20.0)
        );

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn pending_selection_scrolls_safely_in_truncate_and_horizontal_modes() {
        let content = (0..20)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        let path = write_temp(&content);
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.viewport_range = Some((10, 14));
        tab.scroll_top_line = Some(10);
        tab.scroll_fraction = 0.0;

        for mode in [
            haystack::core::settings::LogLineDisplayMode::Truncate,
            haystack::core::settings::LogLineDisplayMode::HorizontalScroll,
        ] {
            tab.log_line_display_mode = mode;
            // Both modes use one fixed-height row per source line. The target
            // must land two rows inside the bottom edge, not on the clipped
            // virtualization boundary.
            assert_eq!(
                compute_pending_scroll_offset(&tab, Some(15), 10.0, 50.0, 20),
                Some(130.0),
                "unexpected scroll policy for {mode:?}"
            );
            // The top row is only half visible here. It must also be
            // repositioned instead of being accepted as a selectable row.
            tab.scroll_fraction = 0.5;
            assert_eq!(
                compute_pending_scroll_offset(&tab, Some(10), 10.0, 50.0, 20),
                Some(80.0),
                "clipped top row was accepted for {mode:?}"
            );
            tab.scroll_fraction = 0.0;
        }

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn fully_visible_rows_exclude_clipped_edges_in_fixed_modes() {
        for mode in [
            haystack::core::settings::LogLineDisplayMode::Truncate,
            haystack::core::settings::LogLineDisplayMode::HorizontalScroll,
        ] {
            assert_eq!(
                fully_visible_virtual_range(100.0, 50.0, 10.0, None, 20),
                Some((10, 14)),
                "unexpected full-row range for {mode:?}"
            );
            assert_eq!(
                fully_visible_virtual_range(105.0, 50.0, 10.0, None, 20),
                Some((11, 14)),
                "partially visible top/bottom rows were accepted for {mode:?}"
            );
            assert_eq!(
                fully_visible_virtual_range(100.0, 49.9, 10.0, None, 20),
                Some((10, 13)),
                "partially visible bottom row was accepted for {mode:?}"
            );
        }
    }

    #[test]
    fn show_rows_overscan_is_not_part_of_the_selectable_range() {
        let rendered = std::cell::Cell::new(None);
        egui::__run_test_ui(|ui| {
            let output = egui::ScrollArea::vertical().max_height(50.0).show_rows(
                ui,
                13.0,
                20,
                |_ui, range| {
                    rendered.set(Some((range.start, range.end)));
                },
            );
            let (rendered_start, rendered_end) = rendered.get().expect("rows were rendered");
            let full = fully_visible_virtual_range(
                output.state.offset.y,
                output.inner_rect.height(),
                13.0,
                None,
                20,
            )
            .expect("at least one complete row should fit");

            assert_eq!(rendered_start, 0);
            assert!(
                rendered_end > full.1 + 1,
                "rendered=({rendered_start}, {rendered_end}), full={full:?}, height={}",
                output.inner_rect.height()
            );
            assert_eq!(full, (0, 3));
        });
    }

    #[test]
    fn wrapped_selection_scroll_uses_visual_row_distance() {
        let content = (0..20)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        let path = write_temp(&content);
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.log_line_display_mode = haystack::core::settings::LogLineDisplayMode::Wrap;
        tab.viewport_range = Some((10, 14));
        tab.scroll_top_line = Some(10);
        tab.scroll_fraction = 0.0;
        let offsets = (0..=20).map(|line| line as f32 * 10.0).collect::<Vec<_>>();

        assert_eq!(
            compute_pending_wrap_scroll_offset(&tab, Some(12), &offsets, 50.0, 10.0, 20),
            None
        );
        assert_eq!(
            compute_pending_wrap_scroll_offset(&tab, Some(9), &offsets, 50.0, 10.0, 20),
            Some(70.0)
        );
        assert_eq!(
            compute_pending_wrap_scroll_offset(&tab, Some(3), &offsets, 50.0, 10.0, 20),
            Some(10.0)
        );

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn wrapped_selection_scroll_keeps_the_complete_visual_line_in_view() {
        let content = (0..20)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        let path = write_temp(&content);
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.log_line_display_mode = haystack::core::settings::LogLineDisplayMode::Wrap;
        tab.scroll_top_line = Some(10);
        tab.scroll_fraction = 0.0;
        // Lines 10..14 occupy 50 visual rows, while the selected wrapped
        // record occupies two rows. It must not be placed against the bottom
        // edge where its final visual row would be clipped.
        let offsets = (0..=20)
            .map(|line| line as f32 * 10.0 + if line >= 16 { 10.0 } else { 0.0 })
            .collect::<Vec<_>>();
        assert_eq!(
            compute_pending_wrap_scroll_offset(&tab, Some(15), &offsets, 50.0, 10.0, 20),
            Some(140.0)
        );

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn fully_visible_wrap_rows_require_the_complete_logical_line() {
        let offsets = vec![0.0, 10.0, 30.0, 40.0, 50.0, 60.0];

        // The first logical line is clipped above; lines 1 and 2 fit exactly.
        assert_eq!(
            fully_visible_virtual_range(5.0, 35.0, 10.0, Some(&offsets), 5),
            Some((1, 2))
        );
        // The second logical line ends below the viewport and is not
        // selectable even though virtualization would render it.
        assert_eq!(
            fully_visible_virtual_range(10.0, 29.9, 10.0, Some(&offsets), 5),
            Some((1, 1))
        );
    }

    #[test]
    fn manual_scroll_reselection_is_covered_for_all_long_line_modes() {
        let content = (0..20)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        let path = write_temp(&content);
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.viewport_range = Some((5, 9));

        for mode in [
            haystack::core::settings::LogLineDisplayMode::Truncate,
            haystack::core::settings::LogLineDisplayMode::HorizontalScroll,
            haystack::core::settings::LogLineDisplayMode::Wrap,
        ] {
            tab.log_line_display_mode = mode;
            tab.context_line = Some(12);
            assert!(reconcile_selection_after_user_scroll(&mut tab));
            assert_eq!(
                tab.context_line,
                Some(8),
                "manual scroll selected a clipped row for {mode:?}"
            );
        }

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn user_scroll_reselects_only_when_selection_leaves_viewport() {
        let content = (0..20)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        let path = write_temp(&content);
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.viewport_range = Some((5, 9));

        tab.context_line = Some(7);
        assert!(!reconcile_selection_after_user_scroll(&mut tab));
        assert_eq!(tab.context_line, Some(7));

        tab.context_line = Some(3);
        assert!(reconcile_selection_after_user_scroll(&mut tab));
        assert_eq!(tab.context_line, Some(6));

        tab.context_line = Some(12);
        assert!(reconcile_selection_after_user_scroll(&mut tab));
        assert_eq!(tab.context_line, Some(8));

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn user_scroll_reselection_uses_visible_filtered_rows() {
        let content = (0..30)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        let path = write_temp(&content);
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.visible_lines = Some(Arc::new(vec![0, 10, 20]));
        tab.viewport_range = Some((10, 20));
        tab.context_line = Some(15);

        assert!(reconcile_selection_after_user_scroll(&mut tab));
        assert_eq!(tab.context_line, Some(20));

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn viewport_shadow_maps_only_fully_visible_filtered_rows() {
        let content = (0..30)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        let path = write_temp(&content);
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.visible_lines = Some(Arc::new(vec![0, 10, 20]));

        // The virtual rows 1 and 2 are fully visible; the mapping used by the
        // timeline must expose their real source lines, not an overscanned row.
        let full = fully_visible_virtual_range(10.0, 20.0, 10.0, None, 3);
        assert_eq!(full, Some((1, 2)));
        update_viewport_range(&mut tab, full, None);
        assert_eq!(tab.viewport_range, Some((10, 20)));

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
    fn line_number_gutter_reserves_more_than_the_previous_width() {
        let width = line_gutter_width(8.0, 999_999_999);
        assert!(width > 8.0 * 9.0);
        assert_eq!(line_gutter_width(8.0, 999_999_999), 100.0);
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
    fn binary_plist_preview_decodes_transport_only_after_explicit_action() {
        let preview = decode_embedded_preview("binary-plist", "62706c6973743030deadbeef").unwrap();
        assert!(preview.contains("Magic: bplist00"));
        assert!(preview.contains("Transport: hex"));
        assert!(preview.contains("Decoded bytes: 12"));
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
            let response = ui.add(log_content_label(
                egui::text::LayoutJob::single_section(
                    "select me".to_owned(),
                    egui::text::TextFormat::default(),
                ),
                false,
            ));
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
        let search_matcher = haystack::core::search::build_filter_highlighter(&[
            haystack::core::search::FilterSpec::phrase("error"),
        ])
        .unwrap();
        let hl = Highlights {
            filters: &[],
            filter_matcher: None,
            search_matcher: Some(&search_matcher),
            search_template_id: None,
            search_field: None,
            search_rows: None,
            filter_fields: None,
            filter_rows: None,
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
        let keyword_ac = haystack::core::search::build_find_automaton("error", true);
        let hl = Highlights {
            filters: &[],
            filter_matcher: None,
            search_matcher: None,
            search_template_id: None,
            search_field: None,
            search_rows: None,
            filter_fields: None,
            filter_rows: None,
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
    fn field_find_highlights_the_exact_captured_value() {
        use haystack::core::record::{CompiledProfile, RecordProfile};
        use std::sync::Arc;

        let path = write_temp("2026-07-15 22:26:39.907481+0300 <FAULT> crash\n");
        let profile = CompiledProfile::compile(RecordProfile::inline(
            "test:highlight",
            "Highlight",
            "{time} <{loglevel}> {log}",
        ))
        .unwrap();
        let doc = LogDocument::open_with_record_profile(
            &path,
            haystack::core::document::ParsingConfig::default(),
            &[],
            Arc::clone(&profile),
            Some(haystack::core::time::TimeFormatKind::BuiltIn(
                &haystack::core::time::Iso,
            )),
        )
        .unwrap();
        let query = haystack::core::field_query::FieldQuery::parse("loglevel = \"FAULT\"", true)
            .unwrap()
            .bind_to_doc(&doc)
            .unwrap();
        let rows = [0u32];
        let highlights = Highlights {
            filters: &[],
            filter_matcher: None,
            search_matcher: None,
            search_template_id: None,
            search_field: Some(&query),
            search_rows: Some(&rows),
            filter_fields: None,
            filter_rows: None,
            keyword_ac: None,
            embedded: None,
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
                && &job.text[section.byte_range.start.0..section.byte_range.end.0] == "FAULT"
        }));
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
        let filter_matcher = haystack::core::search::build_filter_highlighter(&[
            haystack::core::search::FilterSpec::phrase("error"),
        ])
        .unwrap();
        let search_matcher = haystack::core::search::build_filter_highlighter(&[
            haystack::core::search::FilterSpec::phrase("error"),
        ])
        .unwrap();
        let hl = Highlights {
            filters: &filters,
            filter_matcher: Some(&filter_matcher),
            search_matcher: Some(&search_matcher),
            search_template_id: None,
            search_field: None,
            search_rows: None,
            filter_fields: None,
            filter_rows: None,
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
    fn embedded_json_keeps_search_background_without_persistent_underlines() {
        use haystack::core::embedded_data::{AnalysisLimits, EmbeddedDataEngine};
        use std::sync::atomic::AtomicBool;

        let path = write_temp("INFO payload={\"error\":true}\n");
        let doc = LogDocument::open(&path).unwrap();
        let detections = EmbeddedDataEngine::default().analyze_original_lines(
            &doc,
            0..1,
            AnalysisLimits::default(),
            &AtomicBool::new(false),
        );
        let search_matcher = haystack::core::search::build_filter_highlighter(&[
            haystack::core::search::FilterSpec::phrase("error"),
        ])
        .unwrap();
        let highlights = Highlights {
            filters: &[],
            filter_matcher: None,
            search_matcher: Some(&search_matcher),
            search_template_id: None,
            search_field: None,
            search_rows: None,
            filter_fields: None,
            filter_rows: None,
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
        assert!(job
            .sections
            .iter()
            .any(|section| { section.format.background == theme.search_highlight_bg }));
        assert!(job
            .sections
            .iter()
            .all(|section| section.format.underline.width == 0.0));

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn explicit_timestamp_has_no_persistent_underline() {
        let path = write_temp("2026-08-15T19:40:01.123Z INFO ready\n");
        let doc = LogDocument::open(&path).unwrap();
        let theme = Theme::dark();
        let job = line_job(
            &doc,
            &Highlights {
                filters: &[],
                filter_matcher: None,
                search_matcher: None,
                search_template_id: None,
                search_field: None,
                search_rows: None,
                filter_fields: None,
                filter_rows: None,
                keyword_ac: None,
                embedded: None,
            },
            0,
            false,
            crate::ui::fonts::log_font(12.0),
            &theme,
        );
        let timestamp = job
            .sections
            .iter()
            .find(|section| section.byte_range.start.0 == 0)
            .expect("timestamp section");
        assert_eq!(timestamp.format.underline.width, 0.0);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn exact_kv_source_spans_leave_field_gaps_unstyled() {
        use haystack::core::embedded_data::{AnalysisLimits, EmbeddedDataEngine};
        use std::sync::atomic::AtomicBool;

        let path = write_temp("INFO first=1 second=2\n");
        let doc = LogDocument::open(&path).unwrap();
        let detections = EmbeddedDataEngine::default().analyze_original_lines(
            &doc,
            0..1,
            AnalysisLimits::default(),
            &AtomicBool::new(false),
        );
        let highlights = Highlights {
            filters: &[],
            filter_matcher: None,
            search_matcher: None,
            search_template_id: None,
            search_field: None,
            search_rows: None,
            filter_fields: None,
            filter_rows: None,
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
        let gap = job.text.find("1 second").unwrap() + 1;
        let gap_section = job
            .sections
            .iter()
            .find(|section| section.byte_range.start.0 <= gap && section.byte_range.end.0 > gap)
            .expect("section containing the gap between fields");
        assert_eq!(gap_section.format.underline.width, 0.0);
        assert_eq!(detections[0].source_spans.len(), 2);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn copy_rows_can_strip_timestamp_and_include_source_line_numbers() {
        let path = write_temp("2026-07-19T10:00:00.000Z INFO alpha\n");
        let tab = LogTab::new(LogDocument::open(&path).unwrap());
        assert_eq!(lines_text(&tab, 0, 0, false, true), "INFO alpha");
        assert_eq!(
            lines_text(&tab, 0, 0, true, false),
            "1: 2026-07-19T10:00:00.000Z INFO alpha"
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn search_match_beyond_preview_distinguishes_hidden_and_visible_hits() {
        let matcher = haystack::core::search::build_filter_highlighter(&[
            haystack::core::search::FilterSpec::phrase("needle"),
        ])
        .unwrap();
        assert!(!search_match_beyond_preview(
            "needle then more",
            12,
            &matcher
        ));
        assert!(search_match_beyond_preview("prefix needle", 6, &matcher));
        // A match crossing the preview edge is not fully paintable either.
        assert!(search_match_beyond_preview("needle", 3, &matcher));
    }

    #[test]
    fn full_line_modes_do_not_apply_the_display_byte_cap() {
        let line = format!("{}needle", "x".repeat(highlight::MAX_DISPLAY_BYTES + 20));
        assert!(highlight::display_text(&line).contains("truncated"));
        assert_eq!(
            highlight::display_text_for_mode(
                &line,
                haystack::core::settings::LogLineDisplayMode::HorizontalScroll,
            ),
            line
        );
        assert_eq!(
            highlight::display_text_for_mode(
                &line,
                haystack::core::settings::LogLineDisplayMode::Wrap,
            ),
            line
        );
    }

    #[test]
    fn wrap_offsets_are_variable_height_and_reused_until_layout_changes() {
        let path = write_temp("short\nthis line needs multiple visual rows\n");
        let mut tab = LogTab::new(LogDocument::open(&path).unwrap());
        let first = wrap_offsets_for(&mut tab, 8.0, 1.0, 10.0);
        assert_eq!(first.len(), 3);
        assert_eq!(first[1], 10.0);
        assert!(first[2] > 10.0);
        let cached = wrap_offsets_for(&mut tab, 8.0, 1.0, 10.0);
        assert!(Arc::ptr_eq(&first, &cached));

        let resized = wrap_offsets_for(&mut tab, 16.0, 1.0, 10.0);
        assert!(!Arc::ptr_eq(&first, &resized));
        assert!(resized[2] < first[2]);
        std::fs::remove_file(path).ok();
    }
}
