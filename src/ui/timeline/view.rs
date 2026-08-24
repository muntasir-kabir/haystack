//! The timeline strip: whole-file line density (gray histogram) with one
//! colored lane per filter underneath. Filter occurrences are shown as
//! selectable ◆ diamonds. Zoom with scroll wheel, pan with drag, brush
//! select a range with shift+drag. A minimap below shows the full-file
//! overview with the current zoom window highlighted.
//!
//! Click any diamond → context panel shows that line ± radius with line
//! numbers and original log text.

use std::time::Duration;

use eframe::egui;
use egui::{Color32, Pos2, Rect, RichText, Sense, Shape, Stroke, Vec2};

use logotomy::core::document::LogDocument;
use logotomy::core::time::format_ms;
use logotomy::core::timeline::{Timeline, TimelineDomain};

use crate::ui::app::model::LogTab;
use crate::ui::filters as filter_strip;
use crate::ui::icons::{self, Icon};
use crate::ui::theme::Theme;

const HISTO_HEIGHT: f32 = 68.0;
const LANE_HEIGHT: f32 = 14.0;
const MAX_LANES: usize = 20;
/// When the visible filter points in a lane exceed this threshold,
/// fall back to bucket bars instead of individual diamonds.
const DIAMOND_LIMIT: usize = 500;
const MINIMAP_HEIGHT: f32 = 12.0;
/// Zoom factor per scroll tick.
const ZOOM_FACTOR: f64 = 1.18;
/// Minimum zoom span as fraction of total span.
const MIN_ZOOM_FRAC: f64 = 0.0005;
/// Width of the left column for filter labels + visibility/delete controls.
const LABEL_WIDTH: f32 = 216.0;
/// Left padding from the label column edge to the eye icon.
const EYE_LEFT_PAD: f32 = 2.0;
/// Margin from the label column's right edge (and the lane content) to the
/// trailing trash icon, so the label never crowds the lane.
const LABEL_LANE_PAD: f32 = 3.0;
/// Width of the right column for pan/zoom hint icons.
const ICON_WIDTH: f32 = 18.0;
/// Height of the header row ("Timeline" label + filter controls). Included in
/// `panel_height` so the fixed top panel is tall enough for header + body +
/// minimap.
const HEADER_HEIGHT: f32 = 30.0;

/// Compute the total height of the timeline panel for the given tab.
/// Used by the fixed top panel so the whole timeline (header, histogram,
/// all filter lanes, axis labels, and minimap) is always fully visible.
pub fn panel_height(tab: &LogTab) -> f32 {
    let n_filter_lanes = tab
        .timeline
        .filter_buckets
        .len()
        .min(MAX_LANES)
        .min(tab.filters.len());
    let has_filters = !tab.filters.is_empty();
    let total_lanes = if has_filters { n_filter_lanes + 1 } else { 0 };
    let lanes_height = total_lanes as f32 * LANE_HEIGHT;
    let content_height = HISTO_HEIGHT.max(lanes_height);

    HEADER_HEIGHT
        + content_height
        + (if has_filters { 10.0 } else { 0.0 }) // gap after histo
        + 20.0 // axis labels row (tick labels + duration labels + hint text)
        + 8.0   // gap
        + MINIMAP_HEIGHT
}

pub fn show(ui: &mut egui::Ui, tab: &mut LogTab, theme: &Theme) {
    ui.horizontal_top(|ui| {
        ui.label(RichText::new("Timeline").strong());
        ui.add_space(8.0);

        if !tab.filters.is_empty() {
            // Show / hide all filter lanes at once (Everything Else is never touched).
            let all_active = tab.lane_active.iter().all(|&a| a);
            let toggle_txt = if all_active {
                "Disable All"
            } else {
                "Enable All"
            };
            if ui
                .add(egui::Button::image_and_text(
                    icons::icon_image(
                        ui.ctx(),
                        if all_active {
                            Icon::Invisible
                        } else {
                            Icon::Visible
                        },
                        13.0,
                        theme.text,
                    ),
                    toggle_txt,
                ))
                .on_hover_text(if all_active {
                    "Disable every filter lane at once (Everything Else stays as-is)"
                } else {
                    "Enable every filter lane at once"
                })
                .clicked()
            {
                tab.toggle_all_lanes();
            }

            ui.add_space(6.0);
            // Clear all filters (asks for confirmation in app/view.rs).
            if ui
                .add(egui::Button::image_and_text(
                    icons::icon_image(ui.ctx(), Icon::Remove, 13.0, theme.text),
                    "Delete All",
                ))
                .on_hover_text("Remove every filter behind a confirmation popup")
                .clicked()
            {
                tab.pending_clear_filters = true;
            }

            // Keep the separator to header height. A bare separator in this
            // top-aligned horizontal layout expands to the full panel height,
            // which would push the timeline body below the visible panel.
            ui.add_sized(
                egui::vec2(1.0, HEADER_HEIGHT - 2.0),
                egui::Separator::default(),
            );
        }

        filter_strip::add_filter_ui(ui, tab, theme);
        if tab.selected_lane.is_some() {
            // Keep this navigation hint inside the fixed header row. A bare
            // right-to-left layout inherits the full panel height and centers
            // its contents over the lanes, making the header look like it is
            // part of the timeline body and stealing lane hit-testing space.
            let header_width = ui.available_width().max(0.0);
            let mut unselect = false;
            ui.allocate_ui_with_layout(
                egui::vec2(header_width, HEADER_HEIGHT),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    if ui.button("Unselect").clicked() {
                        unselect = true;
                    }
                    ui.add_space(8.0);
                    ui.add(icons::icon_image(
                        ui.ctx(),
                        Icon::ArrowRight,
                        12.0,
                        theme.text_muted,
                    ))
                    .on_hover_text("Next filter occurrence (Right Arrow)");
                    ui.add_space(7.0);
                    ui.add(icons::icon_image(
                        ui.ctx(),
                        Icon::ArrowLeft,
                        12.0,
                        theme.text_muted,
                    ))
                    .on_hover_text("Previous filter occurrence (Left Arrow)");
                    ui.add_space(5.0);
                    ui.label(RichText::new("Use").small().color(theme.text_muted));
                    if let Some(text) = occurrence_navigation_text(tab) {
                        ui.add_space(10.0);
                        show_occurrence_navigation(ui, &text, theme);
                    }
                },
            );
            if unselect {
                tab.selected_lane = None;
                tab.selected_diamond = None;
            }
        }
    });
    // Everything Else lane only shown when filters exist.
    let n_filter_lanes = tab
        .timeline
        .filter_buckets
        .len()
        .min(MAX_LANES)
        .min(tab.filters.len());
    let has_filters = !tab.filters.is_empty();
    let total_lanes = if has_filters { n_filter_lanes + 1 } else { 0 };
    let lanes_height = total_lanes as f32 * LANE_HEIGHT;
    let content_height = HISTO_HEIGHT.max(lanes_height);

    // egui inserts vertical item spacing between the header widget and this
    // body allocation. Remove that inter-widget spacing for the boundary only;
    // the timeline's internal axis/minimap spacing remains unchanged.
    let previous_item_spacing_y = ui.spacing().item_spacing.y;
    ui.spacing_mut().item_spacing.y = 0.0;
    let height = panel_height(tab) - HEADER_HEIGHT;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click_and_drag(),
    );
    ui.spacing_mut().item_spacing.y = previous_item_spacing_y;
    if !ui.is_rect_visible(rect) {
        return;
    }
    // ---- determine domain span ----
    let (full_start, full_end) = domain_span(&tab.timeline.domain, tab.doc.total_lines());
    let (view_start, view_end) = effective_zoom(&tab.timeline_zoom, full_start, full_end);
    let zoomed = tab.timeline_zoom.is_some();

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, egui::CornerRadius::same(4), theme.surface);

    // ---- layout sub-rects ----
    // The main content area spans either from rect.min.x+4 (no filters) or
    // after the label column (with filters).
    let content_left = if has_filters {
        rect.min.x + 4.0 + LABEL_WIDTH + 2.0
    } else {
        rect.min.x + 4.0
    };
    let icon_right = rect.max.x - 4.0;
    let icon_left = icon_right - ICON_WIDTH;
    // Histo + lanes content rect
    let hist = Rect::from_min_max(
        Pos2::new(content_left, rect.min.y),
        Pos2::new(
            if has_filters {
                icon_left - 2.0
            } else {
                icon_left
            },
            rect.min.y + content_height,
        ),
    );
    let lanes_bottom = hist.bottom();
    let axis_top = lanes_bottom + 6.0;
    let minimap = Rect::from_min_max(
        Pos2::new(hist.left(), axis_top + 18.0),
        Pos2::new(hist.right(), axis_top + 18.0 + MINIMAP_HEIGHT),
    );

    // ---- helpers: x-value ↔ pixel ----
    let view_span = (view_end - view_start).max(1);
    let x_to_px = |v: i64| -> f32 {
        let frac = ((v - view_start) as f64 / view_span as f64).clamp(0.0, 1.0) as f32;
        hist.left() + frac * hist.width()
    };
    let px_to_x = |px: f32| -> i64 {
        let frac = ((px - hist.left()) as f64 / hist.width() as f64).clamp(0.0, 1.0);
        view_start + (frac * view_span as f64) as i64
    };

    // ---- density histogram (full-height background) ----
    let density_bins = aggregate_timeline_bins(
        &tab.timeline,
        &tab.timeline.density,
        view_start,
        view_end,
        hist.width().ceil() as usize,
    );
    let max_d = density_bins.iter().copied().max().unwrap_or(1).max(1) as f32;
    let bar_color = theme.histogram;

    // One bar per screen column instead of one per stored timeline bucket.
    // This substantially reduces egui shape/tessellation work when scrolling.
    for (i, &c) in density_bins.iter().enumerate() {
        if c == 0 {
            continue;
        }
        let h = (c as f32 / max_d) * HISTO_HEIGHT;
        let x0 = hist.left() + i as f32 * hist.width() / density_bins.len() as f32;
        let x1 = hist.left() + (i + 1) as f32 * hist.width() / density_bins.len() as f32;
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(x0, hist.bottom() - h),
                Pos2::new(x1, hist.bottom()),
            ),
            egui::CornerRadius::ZERO,
            bar_color,
        );
    }

    // ---- lane row offset helper ----
    let lane_y = |lane_index: usize| -> f32 { hist.top() + lane_index as f32 * LANE_HEIGHT };

    // Deferred toggle flags (to avoid mutable borrow conflict during iteration).
    let mut toggle_ee: Option<bool> = None;
    let mut toggle_kw: Option<(usize, bool)> = None;
    let mut select_lane: Option<usize> = None;
    let mut selected_occurrence = false;
    // Lane hit targets can consume the pointer event before the outer timeline
    // response sees it. Keep the position so an empty lane click still moves
    // the log view.
    let mut lane_click_pos: Option<Pos2> = None;
    let mut lane_click_lane: Option<usize> = None;
    let mut sync_line: Option<usize> = None;
    // Deferred ensure_visible (diamond click sets context_line within an immutably-borrowed loop).
    let mut ensure_line: Option<usize> = None;

    if has_filters {
        // ---- label column ----
        let label_col = Rect::from_min_max(
            Pos2::new(rect.min.x + 4.0, rect.min.y),
            Pos2::new(rect.min.x + 4.0 + LABEL_WIDTH, hist.bottom()),
        );

        // ---- Everything Else lane (first lane, index 0) ----
        let ee_y = lane_y(0);
        // Whole-lane hover highlight (label column + lane content) so the
        // whole row reads as one controllable unit while hovering.
        let ee_lane_rect = Rect::from_min_max(
            Pos2::new(label_col.left(), ee_y),
            Pos2::new(hist.right(), ee_y + LANE_HEIGHT),
        );
        if ui.rect_contains_pointer(ee_lane_rect) {
            let hb = theme.text;
            painter.rect_filled(
                ee_lane_rect,
                egui::CornerRadius::same(3),
                Color32::from_rgba_unmultiplied(hb.r(), hb.g(), hb.b(), 18),
            );
            painter.rect_stroke(
                ee_lane_rect,
                egui::CornerRadius::same(3),
                Stroke::new(
                    1.0_f32,
                    Color32::from_rgba_unmultiplied(hb.r(), hb.g(), hb.b(), 80),
                ),
                egui::StrokeKind::Middle,
            );
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
        }
        // Marker + label in the left column
        let ee_label_rect = Rect::from_min_max(
            Pos2::new(label_col.left(), ee_y),
            Pos2::new(label_col.right(), ee_y + LANE_HEIGHT),
        );
        let ee_label_id = ui.id().with("ee_checkbox");
        let ee_check_resp = ui.interact(ee_label_rect, ee_label_id, Sense::click());
        if ee_check_resp.clicked() {
            toggle_ee = Some(!tab.everything_else_active);
        }
        // Native visible/invisible toggle button
        let ee_marker_pos = Pos2::new(
            label_col.left() + EYE_LEFT_PAD + 6.0,
            ee_y + LANE_HEIGHT / 2.0,
        );
        let ee_marker_color = if tab.everything_else_active {
            theme.text
        } else {
            theme.text_muted
        };
        let ee_icon = if tab.everything_else_active {
            Icon::Visible
        } else {
            Icon::Invisible
        };
        let ee_eye_rect = Rect::from_center_size(ee_marker_pos, Vec2::new(16.0, 14.0));
        if icons::icon_button_at(ui, ee_eye_rect, ee_icon, ee_marker_color).clicked() {
            toggle_ee = Some(!tab.everything_else_active);
        }
        // Label text, centered horizontally in the space after the eye icon
        let ee_text_left = label_col.left() + EYE_LEFT_PAD + 12.0;
        let ee_text_right = label_col.right() - LABEL_LANE_PAD;
        let ee_text = fit_text_to_width(
            ui,
            "Everything Else",
            egui::FontId::monospace(12.0),
            (ee_text_right - ee_text_left).max(0.0),
        );
        let ee_label_pos = Pos2::new(
            (ee_text_left + ee_text_right) / 2.0,
            ee_y + LANE_HEIGHT / 2.0,
        );
        painter.text(
            ee_label_pos,
            egui::Align2::CENTER_CENTER,
            ee_text,
            egui::FontId::monospace(12.0),
            if tab.everything_else_active {
                theme.text
            } else {
                theme.text_muted
            },
        );
        // No density line, no diamonds for Everything Else lane.
    }

    // ---- filter lanes (index 1..total_lanes) ----
    if has_filters {
        let label_col = Rect::from_min_max(
            Pos2::new(rect.min.x + 4.0, rect.min.y),
            Pos2::new(rect.min.x + 4.0 + LABEL_WIDTH, hist.bottom()),
        );

        for (ki, _kb) in tab
            .timeline
            .filter_buckets
            .iter()
            .enumerate()
            .take(n_filter_lanes)
        {
            let li = ki + 1; // lane index (offset by 1 for Everything Else)
            let y = lane_y(li);
            let color = tab.filters[ki].color;
            let is_active = tab.lane_active.get(ki).copied().unwrap_or(true);

            // Whole-lane hover highlight (label column + lane content) so the
            // whole row reads as one controllable unit while hovering.
            let lane_rect = Rect::from_min_max(
                Pos2::new(label_col.left(), y),
                Pos2::new(hist.right(), y + LANE_HEIGHT),
            );
            // Selection is deliberately limited to the lane content area so
            // clicking the eye/trash controls cannot also select or toggle the
            // lane through an overlapping hit target.
            let lane_select_rect = Rect::from_min_max(
                Pos2::new(hist.left(), y),
                Pos2::new(hist.right(), y + LANE_HEIGHT),
            );
            let lane_id = ui.id().with(("lane_select", ki));
            let lane_resp = ui.interact(lane_select_rect, lane_id, Sense::click());
            if lane_resp.clicked() && is_active {
                select_lane = Some(ki);
                lane_click_pos = lane_resp.interact_pointer_pos();
                lane_click_lane = Some(ki);
            }
            if ui.rect_contains_pointer(lane_rect) {
                painter.rect_filled(
                    lane_rect,
                    egui::CornerRadius::same(3),
                    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 20),
                );
                painter.rect_stroke(
                    lane_rect,
                    egui::CornerRadius::same(3),
                    Stroke::new(
                        1.0_f32,
                        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 90),
                    ),
                    egui::StrokeKind::Middle,
                );
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
            }
            if tab.selected_lane == Some(ki) {
                painter.rect_filled(
                    lane_rect,
                    egui::CornerRadius::same(3),
                    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 22),
                );
            }

            // Marker + label in label area
            let kw_label_rect = Rect::from_min_max(
                Pos2::new(label_col.left() + EYE_LEFT_PAD + 12.0, y),
                Pos2::new(label_col.right() - LABEL_LANE_PAD - 12.0, y + LANE_HEIGHT),
            );
            let kw_label_id = ui.id().with(("kw_lane", ki));
            let kw_check_resp = ui.interact(kw_label_rect, kw_label_id, Sense::click());
            if kw_check_resp.clicked() {
                select_lane = Some(ki);
            }
            // Native visible/invisible toggle button
            let kw_marker_pos =
                Pos2::new(label_col.left() + EYE_LEFT_PAD + 6.0, y + LANE_HEIGHT / 2.0);
            let kw_marker_color = if is_active { color } else { theme.text_muted };
            let kw_icon = if is_active {
                Icon::Visible
            } else {
                Icon::Invisible
            };
            let kw_eye_rect = Rect::from_center_size(kw_marker_pos, Vec2::new(16.0, 14.0));
            icons::paint_icon(
                ui.ctx(),
                &painter,
                kw_icon,
                kw_eye_rect.center(),
                12.0,
                kw_marker_color,
            );
            let eye_resp = ui.interact(kw_eye_rect, ui.id().with(("kw_eye", ki)), Sense::click());
            if eye_resp.clicked() {
                toggle_kw = Some((ki, !tab.lane_active[ki]));
            }

            // Label text (bold + bigger when active, normal when disabled)
            let text = &tab.filters[ki].text;
            // Trash (remove) button on the right side of the label row, inset so
            // it never crowds the lane content next to the label column.
            let trash_center_x = label_col.right() - LABEL_LANE_PAD - 6.0;
            let trash_rect = Rect::from_center_size(
                Pos2::new(trash_center_x, y + LANE_HEIGHT / 2.0),
                Vec2::new(16.0, 14.0),
            );
            icons::paint_icon(
                ui.ctx(),
                &painter,
                Icon::Remove,
                trash_rect.center(),
                12.0,
                theme.text_muted,
            );
            let trash_resp =
                ui.interact(trash_rect, ui.id().with(("kw_trash", ki)), Sense::click());
            if trash_resp.clicked() {
                tab.pending_filter_removal = Some(ki);
            }
            if trash_resp.hovered() {
                trash_resp.on_hover_text(format!("Remove '{}'", text));
            }

            // Label text fills the available interval between the eye and trash
            // icons, truncating only when the actual rendered width requires it.
            let text_left = label_col.left() + EYE_LEFT_PAD + 12.0;
            let text_right = label_col.right() - LABEL_LANE_PAD - 12.0;
            let label_center_x = (text_left + text_right) / 2.0;
            let label_font = if is_active {
                egui::FontId::proportional(12.0)
            } else {
                egui::FontId::monospace(11.0)
            };
            let short = fit_text_to_width(
                ui,
                text,
                label_font.clone(),
                (text_right - text_left).max(0.0),
            );
            let label_pos = Pos2::new(label_center_x, y + LANE_HEIGHT / 2.0);

            // New-filter notification: briefly glow the lane label when the
            // filter was just added (via the strip or the search "Add Filter").
            if let Some((hki, at)) = tab.filter_highlight {
                if hki == ki {
                    let duration = Duration::from_millis(300);
                    let elapsed = at.elapsed();
                    if elapsed < duration {
                        let t = elapsed.as_secs_f32() / duration.as_secs_f32();
                        let glow = (1.0 - t).clamp(0.0, 1.0);
                        let galley = ui.ctx().fonts_mut(|f| {
                            f.layout_no_wrap(short.clone(), label_font.clone(), color)
                        });
                        let size = galley.size();
                        let hl_rect = Rect::from_center_size(
                            label_pos,
                            Vec2::new(size.x + 14.0, size.y + 7.0),
                        );
                        let fill = Color32::from_rgba_unmultiplied(
                            color.r(),
                            color.g(),
                            color.b(),
                            (glow * 95.0) as u8,
                        );
                        painter.rect_filled(hl_rect, egui::CornerRadius::same(4), fill);
                        painter.rect_stroke(
                            hl_rect,
                            egui::CornerRadius::same(4),
                            Stroke::new(
                                1.0_f32,
                                Color32::from_rgba_unmultiplied(
                                    color.r(),
                                    color.g(),
                                    color.b(),
                                    (glow * 180.0) as u8,
                                ),
                            ),
                            egui::StrokeKind::Middle,
                        );
                        ui.ctx().request_repaint();
                    } else {
                        tab.filter_highlight = None;
                    }
                }
            }

            painter.text(
                label_pos,
                egui::Align2::CENTER_CENTER,
                &short,
                label_font,
                if is_active { color } else { theme.text_muted },
            );
            if tab.selected_lane == Some(ki) {
                painter.rect_stroke(
                    lane_rect,
                    egui::CornerRadius::same(3),
                    Stroke::new(
                        1.0,
                        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 180),
                    ),
                    egui::StrokeKind::Middle,
                );
            }
            if kw_check_resp.hovered() {
                // Tooltip with the FULL filter text (the label truncates at 14
                // chars) plus its total occurrence count, e.g. "error (334 occurrence)".
                let count = tab.matches.get(ki).map_or(0, |m| m.len());
                let plural = if count == 1 { "" } else { "s" };
                kw_check_resp.on_hover_text(format!("{} ({count} occurrence{plural})", text));
            }

            if !is_active {
                // Lane is disabled: skip density line and diamonds.
                continue;
            }

            // Draw a straight 1px horizontal line across the full lane width
            // in the filter's color.
            let line_y = y + LANE_HEIGHT / 2.0;
            painter.line_segment(
                [
                    Pos2::new(hist.left(), line_y),
                    Pos2::new(hist.right(), line_y),
                ],
                Stroke::new(1.0_f32, color),
            );
            let kb = &tab.timeline.filter_buckets[ki];

            // ---- filter occurrences: diamonds (sparse) or bucket bars (dense) ----
            let point_count = tab.timeline.point_count_in_range(ki, view_start, view_end);

            if point_count > 0 && point_count <= DIAMOND_LIMIT {
                // --- individual diamonds ---
                if let Some(pts) = tab.timeline.points_in_range(ki, view_start, view_end) {
                    for &(line_idx, xv) in pts {
                        let cx = x_to_px(xv);
                        let cy = y + LANE_HEIGHT / 2.0; // vertical center of the lane
                        let half = 5.0_f32;
                        let diamond = Shape::convex_polygon(
                            vec![
                                Pos2::new(cx, cy - half),
                                Pos2::new(cx + half, cy),
                                Pos2::new(cx, cy + half),
                                Pos2::new(cx - half, cy),
                            ],
                            color,
                            Stroke::new(
                                if tab.selected_diamond == Some((ki, line_idx as usize)) {
                                    1.5_f32
                                } else {
                                    0.0_f32
                                },
                                theme.diamond_stroke,
                            ),
                        );
                        painter.add(diamond);

                        // Invisible click target per diamond (bigger to match larger diamond).
                        let click_rect =
                            Rect::from_center_size(Pos2::new(cx, cy), Vec2::new(12.0, 12.0));
                        let click_id = ui.id().with(("diamond", ki, line_idx));
                        let click_resp = ui.interact(click_rect, click_id, Sense::click());
                        if click_resp.clicked() {
                            select_lane = Some(ki);
                            selected_occurrence = true;
                            tab.context_line = Some(line_idx as usize);
                            tab.selected_diamond = Some((ki, line_idx as usize));
                            tab.pending_scroll = Some(line_idx as usize);
                            sync_line = Some(line_idx as usize);
                            ensure_line = Some(line_idx as usize);
                        }
                        if click_resp.hovered() {
                            painter.add(Shape::convex_polygon(
                                vec![
                                    Pos2::new(cx, cy - half - 1.0),
                                    Pos2::new(cx + half + 1.0, cy),
                                    Pos2::new(cx, cy + half + 1.0),
                                    Pos2::new(cx - half - 1.0, cy),
                                ],
                                Color32::TRANSPARENT,
                                Stroke::new(1.5_f32, theme.diamond_hover),
                            ));
                            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                            let tip = format!("◆ {} — line {}", tab.filters[ki].text, line_idx + 1);
                            click_resp.on_hover_text(tip);
                        }
                    }
                }
            } else if point_count > DIAMOND_LIMIT {
                // --- too many points: fall back to bucket bars ---
                let bins = aggregate_timeline_bins(
                    &tab.timeline,
                    kb,
                    view_start,
                    view_end,
                    hist.width().ceil() as usize,
                );
                for (i, &c) in bins.iter().enumerate() {
                    if c == 0 {
                        continue;
                    }
                    let x0 = hist.left() + i as f32 * hist.width() / bins.len() as f32;
                    let x1 = hist.left() + (i + 1) as f32 * hist.width() / bins.len() as f32;
                    let center_y = y + LANE_HEIGHT / 2.0;
                    let h = (c as f32 / max_d).clamp(1.0, LANE_HEIGHT / 2.0 - 1.0);
                    painter.rect_filled(
                        Rect::from_min_max(
                            Pos2::new(x0.max(hist.left()), center_y - h),
                            Pos2::new(x1.min(hist.right()), center_y),
                        ),
                        egui::CornerRadius::ZERO,
                        color,
                    );
                }
            }
        }
    }

    // Apply deferred ensure_visible (after diamond click during immutable borrow).
    if let Some(_line) = ensure_line {
        tab.ensure_visible();
    }

    if let Some(ki) = select_lane {
        // Lane selection is non-mutating. If an overlapping/retained egui
        // response also reported the eye control, discard that toggle so a
        // lane click can never hide the timeline data it just selected.
        toggle_kw = None;
        if let Some(active) = tab.lane_active.get_mut(ki) {
            *active = true;
        }
        let occurrence = tab.selected_diamond;
        tab.select_lane(ki);
        if selected_occurrence {
            tab.selected_diamond = occurrence;
        }
    }
    if let Some(line) = sync_line {
        tab.sync_timeline_selection_to_line(line);
    }

    // Apply deferred lane toggles.
    if let Some(active) = toggle_ee {
        tab.everything_else_active = active;
        tab.rebuild_visible_lines_background();
    }
    if let Some((ki, active)) = toggle_kw {
        if ki < tab.lane_active.len() {
            tab.lane_active[ki] = active;
            if !active && tab.selected_lane == Some(ki) {
                tab.selected_lane = None;
                tab.selected_diamond = None;
            }
            // Check if this was the last active filter and everything else is off.
            let active_filter_count = tab.lane_active.iter().filter(|&&b| b).count();
            if active_filter_count == 0 && !tab.everything_else_active {
                tab.everything_else_active = true;
            }
            tab.rebuild_visible_lines_background();
        }
    }

    if has_filters && tab.timeline.filter_buckets.len() > MAX_LANES {
        painter.text(
            Pos2::new(hist.right() - 2.0, lanes_bottom),
            egui::Align2::RIGHT_TOP,
            format!("+{} more", tab.timeline.filter_buckets.len() - MAX_LANES),
            egui::FontId::monospace(7.5),
            theme.text_muted,
        );
    }

    // ---- viewport shadow: shaded band showing log view's current scroll range ----
    {
        let mut shadow_first = tab.viewport_range.map(|(f, _)| f);
        let mut shadow_last = tab.viewport_range.map(|(_, l)| l);

        // If a pending scroll exists (click just happened), expand shadow to cover
        // the context_line so the marker is always visually inside the shadow.
        if tab.pending_scroll.is_some() {
            if let Some(cl) = tab.context_line {
                let cur_first = shadow_first.unwrap_or(cl);
                let cur_last = shadow_last.unwrap_or(cl);
                shadow_first = Some(cur_first.min(cl));
                shadow_last = Some(cur_last.max(cl));
            }
        }

        if let Some((first_line, last_line)) = shadow_first.zip(shadow_last) {
            let v0 = x_of_line(&tab.doc, &tab.timeline.domain, first_line);
            let v1 = x_of_line(&tab.doc, &tab.timeline.domain, last_line);
            if v0 >= 0 && v1 >= 0 {
                let x0 = x_to_px(v0).max(hist.left());
                let x1 = x_to_px(v1).min(hist.right());
                if x1 > x0 {
                    let shadow_rect =
                        Rect::from_min_max(Pos2::new(x0, hist.top()), Pos2::new(x1, hist.bottom()));
                    painter.rect_filled(
                        shadow_rect,
                        egui::CornerRadius::same(2),
                        theme.viewport_shadow,
                    );
                    // Top/bottom edge highlight
                    painter.rect_stroke(
                        shadow_rect,
                        egui::CornerRadius::same(2),
                        Stroke::new(1.0_f32, theme.viewport_shadow_stroke),
                        egui::StrokeKind::Middle,
                    );
                }
            }
        }
    }

    // ---- selection marker (shadowed rectangle spanning histo + lanes) ----
    if let Some(line) = tab.context_line {
        let v = x_of_line(&tab.doc, &tab.timeline.domain, line);
        if v >= 0 && v >= view_start && v <= view_end {
            let x = x_to_px(v);
            let marker_rect = Rect::from_min_size(
                Pos2::new(x - 1.5, hist.top()),
                egui::vec2(3.0, lanes_bottom - hist.top()),
            );
            // Shadow (slightly offset, darker)
            let shadow_offset = egui::vec2(1.5, 1.5);
            painter.rect_filled(
                Rect::from_min_size(marker_rect.min + shadow_offset, marker_rect.size()),
                egui::CornerRadius::same(1),
                Color32::from_black_alpha(80),
            );
            // Main rectangle
            painter.rect_filled(
                marker_rect,
                egui::CornerRadius::same(1),
                theme.selection_line,
            );
        }
    }

    // ---- axis tick labels (smart shorthand) ----
    let n_ticks = 7_usize;
    let label_y = lanes_bottom + 4.0;
    let font_id = egui::FontId::monospace(10.0);
    let mut tick_xs: Vec<f32> = Vec::with_capacity(n_ticks);
    let mut tick_vs: Vec<i64> = Vec::with_capacity(n_ticks);

    // Determine tick positions and values.
    for i in 0..n_ticks {
        let frac = i as f64 / (n_ticks - 1) as f64;
        let v = view_start + (frac * view_span as f64) as i64;
        let x = hist.left() + frac as f32 * hist.width();
        tick_xs.push(x);
        tick_vs.push(v);
    }

    // Compute shorthand labels for Time domain.
    let labels: Vec<String> = match tab.timeline.domain {
        TimelineDomain::Time { .. } => {
            // Determine common prefix depth.
            let dt0 = chrono::DateTime::from_timestamp_millis(tick_vs[0]);
            let dt1 = chrono::DateTime::from_timestamp_millis(tick_vs[n_ticks - 1]);
            match (dt0, dt1) {
                (Some(d0), Some(d1)) => {
                    let same_date =
                        d0.format("%Y-%m-%d").to_string() == d1.format("%Y-%m-%d").to_string();
                    let same_hour =
                        same_date && d0.format("%H").to_string() == d1.format("%H").to_string();
                    tick_vs
                        .iter()
                        .map(|&v| {
                            if let Some(dt) = chrono::DateTime::from_timestamp_millis(v) {
                                if same_hour {
                                    dt.format("%M:%S%.3f").to_string()
                                } else if same_date {
                                    dt.format("%H:%M:%S%.3f").to_string()
                                } else {
                                    dt.format("%Y-%m-%d %H:%M:%S%.3f").to_string()
                                }
                            } else {
                                format_ms(v)
                            }
                        })
                        .collect()
                }
                _ => tick_vs.iter().map(|&v| format_ms(v)).collect(),
            }
        }
        TimelineDomain::Sequence => tick_vs.iter().map(|&v| format!("L{v}")).collect(),
    };

    // Draw tick marks and labels.
    for i in 0..n_ticks {
        let x = tick_xs[i];
        // Tick mark.
        painter.line_segment(
            [Pos2::new(x, label_y), Pos2::new(x, label_y + 6.0)],
            Stroke::new(1.0_f32, theme.axis),
        );
        // Label.
        let (align, offset) = (egui::Align2::CENTER_TOP, Vec2::new(0.0, 2.0));
        painter.text(
            Pos2::new(x, label_y) + offset,
            align,
            &labels[i],
            font_id.clone(),
            theme.axis,
        );
    }

    // ---- duration labels BETWEEN each pair of adjacent tick labels ----
    let dur_font = egui::FontId::monospace(7.5);
    for i in 1..n_ticks {
        let mid_x = (tick_xs[i - 1] + tick_xs[i]) / 2.0;
        let delta = tick_vs[i] - tick_vs[i - 1];
        if delta > 0 {
            let dur_str = match tab.timeline.domain {
                TimelineDomain::Time { .. } => format_duration_ms(delta),
                TimelineDomain::Sequence => format!("Δ {} lines", delta),
            };
            // Draw between the label rows
            let dur_y = label_y + 2.0;
            painter.text(
                Pos2::new(mid_x, dur_y),
                egui::Align2::CENTER_TOP,
                &dur_str,
                dur_font.clone(),
                theme.hint,
            );
        }
    }

    // ---- hint text ----
    painter.text(
        Pos2::new(hist.center().x, label_y + 14.0),
        egui::Align2::CENTER_TOP,
        if zoomed {
            "scroll to zoom · drag to pan · shift+drag to brush · double-click to reset"
        } else {
            "scroll to zoom · drag to pan · shift+drag to brush"
        },
        egui::FontId::proportional(8.0),
        theme.hint,
    );

    // ---- reset zoom button ----
    if zoomed {
        let reset_rect = Rect::from_min_size(
            hist.right_top() + Vec2::new(-28.0, 2.0),
            Vec2::new(26.0, 14.0),
        );
        let reset_resp = icons::icon_button_at(ui, reset_rect, Icon::Reset, theme.text_muted);
        if reset_resp.clicked() {
            tab.timeline_zoom = None;
            tab.selected_diamond = None;
        }
        if reset_resp.hovered() {
            reset_resp.on_hover_text("Reset zoom to full log");
        }
    }

    // ---- pan/zoom hint icons on the right ----
    if has_filters {
        let icon_col = Rect::from_min_max(
            Pos2::new(rect.max.x - 4.0 - ICON_WIDTH, rect.min.y + 4.0),
            Pos2::new(rect.max.x - 4.0, hist.bottom()),
        );
        let icon_center_x = icon_col.center().x;
        let icon_area_top = icon_col.top() + 4.0;
        let icon_spacing = 18.0;

        // Zoom icon (↕)
        let zoom_icon_rect = Rect::from_center_size(
            Pos2::new(icon_center_x, icon_area_top + icon_spacing * 0.5),
            Vec2::new(ICON_WIDTH, 14.0),
        );
        let zoom_icon_id = ui.id().with("zoom_icon");
        let zoom_icon_resp = ui.interact(zoom_icon_rect, zoom_icon_id, Sense::hover());
        let zoom_color = if zoom_icon_resp.hovered() {
            theme.text
        } else {
            theme.text_muted
        };
        icons::paint_icon(
            ui.ctx(),
            &painter,
            Icon::ArrowsVertical,
            zoom_icon_rect.center(),
            10.0,
            zoom_color,
        );
        if zoom_icon_resp.hovered() {
            zoom_icon_resp.on_hover_text("Scroll to zoom in/out");
        }

        // Pan icon (↔)
        let pan_icon_rect = Rect::from_center_size(
            Pos2::new(icon_center_x, icon_area_top + icon_spacing * 1.5),
            Vec2::new(ICON_WIDTH, 14.0),
        );
        let pan_icon_id = ui.id().with("pan_icon");
        let pan_icon_resp = ui.interact(pan_icon_rect, pan_icon_id, Sense::hover());
        let pan_color = if pan_icon_resp.hovered() {
            theme.text
        } else {
            theme.text_muted
        };
        icons::paint_icon(
            ui.ctx(),
            &painter,
            Icon::ArrowsHorizontal,
            pan_icon_rect.center(),
            10.0,
            pan_color,
        );
        if pan_icon_resp.hovered() {
            pan_icon_resp.on_hover_text("Drag to pan left/right");
        }
    }

    // ---- minimap ----
    painter.rect_filled(minimap, egui::CornerRadius::same(2), theme.minimap_bg);
    // Draw full-range density in one bar per minimap pixel.
    let full_span = (full_end - full_start).max(1);
    let minimap_bins = aggregate_timeline_bins(
        &tab.timeline,
        &tab.timeline.density,
        full_start,
        full_end,
        minimap.width().ceil() as usize,
    );
    let minimap_max = minimap_bins.iter().copied().max().unwrap_or(1).max(1) as f32;
    for (i, &c) in minimap_bins.iter().enumerate() {
        if c == 0 {
            continue;
        }
        let map_x = minimap.left() + i as f32 * minimap.width() / minimap_bins.len() as f32;
        let map_bw = minimap.width() / minimap_bins.len() as f32;
        let h_frac = (c as f32 / minimap_max).min(1.0);
        let map_h = h_frac * minimap.height();
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(map_x, minimap.bottom() - map_h),
                Pos2::new((map_x + map_bw).min(minimap.right()), minimap.bottom()),
            ),
            egui::CornerRadius::ZERO,
            theme.minimap_bar,
        );
    }
    // Draw zoom window highlight on minimap.
    {
        let frac_left =
            ((view_start - full_start) as f64 / full_span as f64).clamp(0.0, 1.0) as f32;
        let frac_right = ((view_end - full_start) as f64 / full_span as f64).clamp(0.0, 1.0) as f32;
        let win_left = minimap.left() + frac_left * minimap.width();
        let win_right = minimap.left() + frac_right * minimap.width();
        let win_rect = Rect::from_min_max(
            Pos2::new(win_left.max(minimap.left()), minimap.top()),
            Pos2::new(win_right.min(minimap.right()), minimap.bottom()),
        );
        if win_rect.width() > 1.0 {
            painter.rect_stroke(
                win_rect,
                egui::CornerRadius::same(1),
                Stroke::new(1.5_f32, theme.minimap_zoom),
                egui::StrokeKind::Middle,
            );
        }
        // Click on minimap → pan to that position.
        let minimap_resp = ui.interact(minimap, ui.id().with("minimap"), Sense::click());
        if minimap_resp.clicked() {
            if let Some(pos) = minimap_resp.interact_pointer_pos() {
                let frac = ((pos.x - minimap.left()) / minimap.width()).clamp(0.0, 1.0) as f64;
                let center = full_start + (frac * full_span as f64) as i64;
                let half = view_span / 2;
                tab.timeline_zoom = Some((
                    (center - half).max(full_start),
                    (center + half).min(full_end),
                ));
                tab.selected_diamond = None;
            }
        }
        if minimap_resp.hovered() {
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
        }
    }

    /*
    // ---- axis labels at minimap ends ----
    let minimap_label = |x: f32, text: &str| {
        painter.text(
            Pos2::new(x, minimap.bottom() + 2.0),
            egui::Align2::CENTER_TOP,
            text,
            egui::FontId::monospace(7.0),
            theme.text_muted,
        );
    };
    match tab.timeline.domain {
        TimelineDomain::Time { .. } => {
            minimap_label(minimap.left(), &format_ms(full_start));
            minimap_label(minimap.right(), &format_ms(full_end));
        }
        TimelineDomain::Sequence => {
            minimap_label(minimap.left(), "L1");
            minimap_label(minimap.right(), &format!("L{}", tab.doc.total_lines()));
        }
    }
    */

    // ---- zoom via scroll wheel ----
    if response.hovered() {
        let scroll_delta = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll_delta != 0.0 {
            let mouse_x = ui
                .input(|i| i.pointer.hover_pos())
                .map(|p| px_to_x(p.x))
                .unwrap_or((view_start + view_end) / 2);
            // Continuous zoom factor: powf works for both trackpad (small deltas,
            // many frames) and mouse wheel (large deltas, few notches).
            let factor = if scroll_delta > 0.0 {
                ZOOM_FACTOR.powf(scroll_delta.abs() as f64 / 60.0)
            } else {
                1.0 / ZOOM_FACTOR.powf(scroll_delta.abs() as f64 / 60.0)
            };
            let new_span = ((view_span as f64) / factor) as i64;
            let min_span = ((full_span as f64) * MIN_ZOOM_FRAC) as i64;
            let new_span = new_span.max(min_span).min(full_span);
            let ratio = (mouse_x - view_start) as f64 / view_span as f64;
            let new_start = mouse_x - (new_span as f64 * ratio) as i64;
            let new_start = new_start.max(full_start);
            let new_end = (new_start + new_span).min(full_end);
            let new_start = (new_end - new_span).max(full_start);
            if new_span >= full_span {
                tab.timeline_zoom = None;
            } else {
                tab.timeline_zoom = Some((new_start, new_end));
            }
            tab.selected_diamond = None;
        }
    }

    // ---- pan via drag (left button only) ----
    if !ui.input(|i| i.modifiers.shift) && response.dragged() {
        let dx_px = response.drag_delta().x;
        let dx_val = (dx_px as f64 / hist.width() as f64 * view_span as f64) as i64;
        let new_start = (view_start - dx_val).clamp(full_start, full_end - view_span);
        let new_end = new_start + view_span;
        if new_start == full_start && new_end == full_end {
            tab.timeline_zoom = None;
        } else {
            tab.timeline_zoom = Some((new_start, new_end));
        }
        tab.selected_diamond = None;
    }

    // ---- brush-select (shift+drag) ----
    if ui.input(|i| i.modifiers.shift) {
        if response.dragged() {
            let Some(drag_origin) = ui.input(|i| i.pointer.interact_pos()) else {
                return;
            };
            let Some(current) = ui.input(|i| i.pointer.interact_pos()) else {
                return;
            };
            let brush_left = drag_origin.x.min(current.x);
            let brush_right = drag_origin.x.max(current.x);
            let brush_rect = Rect::from_min_max(
                Pos2::new(brush_left, hist.top()),
                Pos2::new(brush_right, lanes_bottom),
            );
            painter.rect_filled(brush_rect, egui::CornerRadius::same(2), theme.brush_fill);
            painter.rect_stroke(
                brush_rect,
                egui::CornerRadius::same(2),
                Stroke::new(1.0_f32, theme.brush_stroke),
                egui::StrokeKind::Middle,
            );
        }
        if response.drag_stopped() {
            if let Some(start) = ui.input(|i| i.pointer.interact_pos()) {
                let drag_origin = response.interact_pointer_pos().unwrap_or(start);
                if let Some(current) = Some(start) {
                    let x1 = px_to_x(drag_origin.x.min(current.x));
                    let x2 = px_to_x(drag_origin.x.max(current.x));
                    if (x2 - x1).abs() > 100 {
                        tab.timeline_zoom = Some((x1.max(full_start), x2.min(full_end)));
                        tab.selected_diamond = None;
                    }
                }
            }
        }
    }

    // ---- double-click to reset zoom and snap to nearest match ----
    if response.double_clicked() {
        tab.timeline_zoom = None;
        tab.selected_diamond = None;
        if let Some(pos) = response.interact_pointer_pos() {
            let v = px_to_x(pos.x);
            let target = tab
                .timeline
                .nearest_match_line(v)
                .or_else(|| approx_line(&tab.doc, &tab.timeline.domain, v, full_start, full_end));
            tab.context_line = target;
            if target.is_some() {
                tab.pending_scroll = target;
            }
            tab.ensure_visible();
        }
    }

    // ---- click (no drag, no shift) → snap to nearest match ----
    if !ui.input(|i| i.modifiers.shift) {
        let click_pos = lane_click_pos.or_else(|| {
            response
                .clicked()
                .then(|| response.interact_pointer_pos())
                .flatten()
        });
        if let Some(pos) = click_pos {
            if !response.dragged() && !selected_occurrence {
                let v = px_to_x(pos.x);
                let target = approx_line(&tab.doc, &tab.timeline.domain, v, full_start, full_end);
                if let Some(line) = target {
                    tab.select_timeline_line(line, lane_click_lane);
                }
            }
        }
    }

    // TODO: fix timeline drag after showing this context menu
    // ---- right-click context menu (trim actions) ----
    // let mut context_trim: Option<TrimAction> = None;
    // response.context_menu(|ui| {
    //     // Determine the line at the right-click position.
    //     let click_line = ui.input(|i| i.pointer.interact_pos()).and_then(|pos| {
    //         let v = px_to_x(pos.x);
    //         tab.timeline
    //             .nearest_match_line(v)
    //             .or_else(|| approx_line(&tab.doc, &tab.timeline.domain, v, full_start, full_end))
    //     });
    //     if let Some(line) = click_line {
    //         // Select this line so the user sees which line will be trimmed.
    //         tab.context_line = Some(line);
    //         tab.selected_diamond = None;
    //         tab.pending_scroll = Some(line);
    //         tab.ensure_visible();
    //
    //         ui.set_min_width(160.0);
    //         if ui.button("↑ ✂️ Trim bottom").on_hover_text("Remove all lines after this one").clicked() {
    //             context_trim = Some(TrimAction::TrimRight(line));
    //             ui.close_menu();
    //         }
    //         if ui.button("↓ ✂️ Trim top").on_hover_text("Remove all lines before this one").clicked() {
    //             context_trim = Some(TrimAction::TrimLeft(line));
    //             ui.close_menu();
    //         }
    //     } else {
    //         ui.label("No line at this position");
    //     }
    // });
    // if let Some(action) = context_trim {
    //     tab.handle_trim(action);
    // }

    // ---- tooltip on hover when not dragging ----
    if !response.dragged() {
        response.on_hover_ui(|ui| {
            if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
                let v = px_to_x(pos.x);
                ui.label(RichText::new(v_caption(tab.timeline.domain, v)).strong());
                let b = tab.timeline.bucket_for(v, tab.doc.total_lines());
                ui.label(format!("{} lines in bucket", tab.timeline.density[b]));
                for (ki, kb) in tab
                    .timeline
                    .filter_buckets
                    .iter()
                    .enumerate()
                    .take(MAX_LANES.min(tab.filters.len()))
                {
                    if kb[b] > 0 {
                        ui.label(
                            RichText::new(format!("◆ {} ×{}", tab.filters[ki].text, kb[b]))
                                .color(tab.filters[ki].color),
                        );
                    }
                }
            }
        });
    }
}

// ---- helpers ----

/// Keep a lane label inside the space reserved between its controls.
///
/// The old fixed character limit made the label column look underused after
/// it was widened, and slicing at a byte offset could also split UTF-8 text.
fn fit_text_to_width(ui: &egui::Ui, text: &str, font: egui::FontId, max_width: f32) -> String {
    let full = ui
        .ctx()
        .fonts_mut(|fonts| fonts.layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE));
    if full.size().x <= max_width {
        return text.to_owned();
    }

    let mut fitted = String::new();
    for ch in text.chars() {
        let candidate = format!("{fitted}{ch}…");
        let width = ui.ctx().fonts_mut(|fonts| {
            fonts
                .layout_no_wrap(candidate.clone(), font.clone(), Color32::WHITE)
                .size()
                .x
        });
        if width > max_width {
            break;
        }
        fitted.push(ch);
    }
    format!("{fitted}…")
}

/// Format a duration delta in ms to a human-readable string.
fn format_duration_ms(ms: i64) -> String {
    if ms >= 3600000 {
        format!("Δ {}h {}m", ms / 3600000, (ms % 3600000) / 60000)
    } else if ms >= 60000 {
        format!("Δ {}m {}s", ms / 60000, (ms % 60000) / 1000)
    } else if ms >= 1000 {
        format!("Δ {:.1}s", ms as f64 / 1000.0)
    } else {
        format!("Δ {}ms", ms)
    }
}

fn format_human_duration_ms(ms: i64) -> String {
    let ms = ms.max(0);
    if ms >= 3600000 {
        let hours = ms / 3600000;
        let minutes = (ms % 3600000) / 60000;
        if minutes == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h {minutes}m")
        }
    } else if ms >= 60000 {
        let minutes = ms / 60000;
        let seconds = (ms % 60000) / 1000;
        if seconds == 0 {
            format!("{minutes}m")
        } else {
            format!("{minutes}m {seconds}sec")
        }
    } else if ms >= 1000 {
        format!("{:.1}sec", ms as f64 / 1000.0)
    } else {
        format!("{ms}ms")
    }
}

fn occurrence_navigation_text(tab: &LogTab) -> Option<String> {
    let (lane, line) = tab.selected_diamond?;
    let matches = tab.matches.get(lane)?;
    let position = matches
        .iter()
        .position(|&match_line| match_line as usize == line)?;
    let mut text = format!("Occurrence {} / {}", position + 1, matches.len());

    if position > 0 {
        let previous = matches[position - 1] as usize;
        let delta = match tab.timeline.domain {
            TimelineDomain::Time { .. } => tab
                .doc
                .ts_at_opt(line)
                .zip(tab.doc.ts_at_opt(previous))
                .map(|(current, previous)| format_human_duration_ms(current - previous))
                .unwrap_or_else(|| format!("{} lines", line - previous)),
            TimelineDomain::Sequence => format!("{} lines", line - previous),
        };
        text.push_str(&format!(" | prev {delta} ago"));
    }
    if position + 1 < matches.len() {
        let next = matches[position + 1] as usize;
        let delta = match tab.timeline.domain {
            TimelineDomain::Time { .. } => tab
                .doc
                .ts_at_opt(next)
                .zip(tab.doc.ts_at_opt(line))
                .map(|(next, current)| format_human_duration_ms(next - current))
                .unwrap_or_else(|| format!("{} lines", next - line)),
            TimelineDomain::Sequence => format!("{} lines", next - line),
        };
        text.push_str(&format!(" | next {delta} after"));
    }
    Some(text)
}

fn show_occurrence_navigation(ui: &mut egui::Ui, text: &str, theme: &Theme) {
    let normal = |text: &str| RichText::new(text).size(11.0).color(theme.text_muted);
    let emphasized = |text: &str| {
        RichText::new(text)
            .size(11.0)
            .strong()
            .color(theme.text_muted)
    };

    ui.horizontal(|ui| {
        let mut sections = text.split(" | ");
        if let Some(occurrence) = sections.next() {
            if let Some(value) = occurrence.strip_prefix("Occurrence ") {
                ui.label(normal("Occurrence "));
                ui.label(emphasized(value));
            } else {
                ui.label(normal(occurrence));
            }
        }
        for section in sections {
            ui.label(normal(" | "));
            if let Some((prefix, value)) = section.split_once(' ') {
                ui.label(normal(&format!("{prefix} ")));
                ui.label(emphasized(value));
            } else {
                ui.label(normal(section));
            }
        }
    });
}

fn domain_span(domain: &TimelineDomain, total_lines: usize) -> (i64, i64) {
    match domain {
        TimelineDomain::Time { start_ms, end_ms } => (*start_ms, *end_ms),
        TimelineDomain::Sequence => (1, total_lines as i64),
    }
}

fn effective_zoom(zoom: &Option<(i64, i64)>, full_start: i64, full_end: i64) -> (i64, i64) {
    match zoom {
        Some((s, e)) => ((*s).max(full_start), (*e).min(full_end)),
        None => (full_start, full_end),
    }
}

/// Aggregate stored timeline buckets into screen columns for a domain range.
/// The original buckets remain unchanged for hover details and navigation.
fn aggregate_timeline_bins(
    timeline: &Timeline,
    counts: &[u32],
    start: i64,
    end: i64,
    columns: usize,
) -> Vec<u32> {
    let columns = columns.max(1).min(counts.len().max(1));
    let mut out = vec![0u32; columns];
    let span = (end - start).max(1);
    for (bucket, &count) in counts.iter().enumerate() {
        if count == 0 {
            continue;
        }
        let x = timeline.bucket_center(bucket);
        if x < start || x > end {
            continue;
        }
        let col = (((x - start).clamp(0, span) * columns as i64) / (span + 1)) as usize;
        out[col.min(columns - 1)] = out[col.min(columns - 1)].saturating_add(count);
    }
    out
}

fn x_of_line(doc: &LogDocument, domain: &TimelineDomain, line: usize) -> i64 {
    match domain {
        TimelineDomain::Time { .. } => doc.ts_at(line),
        // Sequence domain uses 1-based line numbering to match domain_span (1..total_lines).
        TimelineDomain::Sequence => line as i64 + 1,
    }
}

fn v_caption(domain: TimelineDomain, v: i64) -> String {
    match domain {
        TimelineDomain::Time { .. } => format_ms(v),
        TimelineDomain::Sequence => format!("~line {v}"),
    }
}

/// With no filter matches to snap to, approximate a line index for the click.
fn approx_line(
    doc: &LogDocument,
    domain: &TimelineDomain,
    v: i64,
    view_start: i64,
    view_end: i64,
) -> Option<usize> {
    let n = doc.total_lines();
    if n == 0 {
        return None;
    }
    match domain {
        TimelineDomain::Time { .. } => {
            for i in 0..n {
                let t = doc.ts_at(i);
                if t >= v && t >= 0 {
                    return Some(i);
                }
            }
            for i in (0..n).rev() {
                let t = doc.ts_at(i);
                if t >= 0 && t <= v {
                    return Some(i);
                }
            }
            Some(n - 1)
        }
        TimelineDomain::Sequence => {
            let frac =
                ((v - view_start) as f64 / (view_end - view_start).max(1) as f64).clamp(0.0, 1.0);
            Some((frac * (n - 1) as f64) as usize)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_bins_preserves_visible_count() {
        let timeline = Timeline {
            domain: TimelineDomain::Sequence,
            n_buckets: 8,
            density: vec![1, 2, 3, 4, 5, 6, 7, 8],
            filter_buckets: Vec::new(),
            filter_points: Vec::new(),
            max_density: 8,
        };
        let bins = aggregate_timeline_bins(&timeline, &timeline.density, 0, 7, 3);
        assert_eq!(bins.len(), 3);
        assert_eq!(bins.iter().sum::<u32>(), 36);
    }

    #[test]
    fn aggregate_bins_ignores_values_outside_range() {
        let timeline = Timeline {
            domain: TimelineDomain::Sequence,
            n_buckets: 4,
            density: vec![1, 2, 3, 4],
            filter_buckets: Vec::new(),
            filter_points: Vec::new(),
            max_density: 4,
        };
        let bins = aggregate_timeline_bins(&timeline, &timeline.density, 1, 2, 8);
        assert_eq!(bins.iter().sum::<u32>(), 5);
    }

    #[test]
    fn human_duration_formats_occurrence_deltas() {
        assert_eq!(format_human_duration_ms(3_900_000), "1h 5m");
        assert_eq!(format_human_duration_ms(1_500), "1.5sec");
        assert_eq!(format_human_duration_ms(200), "200ms");
    }
}
