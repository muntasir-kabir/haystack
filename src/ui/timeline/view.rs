//! The timeline strip: whole-file line density (gray histogram) with one
//! colored lane per filter underneath. Single filter occurrences are shown at
//! their exact position; aggregated buckets join into a continuous density
//! strip. Zoom with scroll wheel, pan with drag, brush
//! select a range with shift+drag. A minimap below shows the full-file
//! overview with the current zoom window highlighted.
//!
//! Click any occurrence mark → jump the Log View to that real line.

use std::time::Duration;

use eframe::egui;
use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use haystack::core::document::LogDocument;
use haystack::core::time::format_ms;
use haystack::core::timeline::TimelineDomain;

use crate::ui::app::model::{LogTab, TimelineDisplayMode};
use crate::ui::filters as filter_strip;
use crate::ui::icons::{self, Icon};
use crate::ui::theme::Theme;

const HISTO_HEIGHT: f32 = 68.0;
const LANE_HEIGHT: f32 = 14.0;
const MAX_LANES: usize = 20;
// Occurrence/bucket dimensions are intentionally centralized here so the lane
// scale can be customized without touching rendering logic.
const OCCURRENCE_BUCKET_WIDTH: f32 = 8.0;
const SINGLE_OCCURRENCE_WIDTH: f32 = 3.0;
const SINGLE_OCCURRENCE_HEIGHT: f32 = 7.0;
const SMALL_BUCKET_HEIGHT: f32 = 7.0;
const MEDIUM_BUCKET_HEIGHT: f32 = 9.0;
const DENSE_BUCKET_HEIGHT: f32 = 11.0;

// Count thresholds for the three aggregated bucket tiers.
const SMALL_BUCKET_MAX_OCCURRENCES: u32 = 4;
const MEDIUM_BUCKET_MAX_OCCURRENCES: u32 = 16;
const MINIMAP_HEIGHT: f32 = 12.0;
/// Vertical placement of the minimap after the gesture hint row was removed.
const MINIMAP_AXIS_OFFSET: f32 = 16.0;
/// Zoom factor per scroll tick.
const ZOOM_FACTOR: f64 = 1.18;
/// Width of the left column for filter labels + visibility/delete controls.
const LABEL_WIDTH: f32 = 216.0;
/// Left padding from the label column edge to the eye icon.
const EYE_LEFT_PAD: f32 = 2.0;
/// Margin from the label column's right edge (and the lane content) to the
/// trailing trash icon, so the label never crowds the lane.
const LABEL_LANE_PAD: f32 = 3.0;
/// Height of the header row (filter controls). Included in `panel_height` so
/// the fixed top panel is tall enough for controls + body + minimap.
const HEADER_HEIGHT: f32 = 22.0;

fn minimap_height() -> f32 {
    8.0 + MINIMAP_HEIGHT
}

fn minimap_color(theme: &Theme) -> Color32 {
    // The overview minimap always uses the untouched theme-aware default.
    Color32::from_rgba_unmultiplied(theme.axis.r(), theme.axis.g(), theme.axis.b(), 255)
}

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
    let has_pin_markers = tab
        .pins
        .iter()
        .any(|pin| pin.visible_bounds(tab.doc.total_lines()).is_some());
    // The logically-empty Everything Else lane doubles as the Pin marker
    // lane. Keep it available for pins even before the user adds a filter.
    let has_lanes = has_filters || has_pin_markers;
    let total_lanes = if has_lanes { n_filter_lanes + 1 } else { 0 };
    let lanes_height = total_lanes as f32 * LANE_HEIGHT;
    let content_height = HISTO_HEIGHT.max(lanes_height);

    HEADER_HEIGHT
        + content_height
        + (if has_lanes { 4.0 } else { 0.0 }) // gap after histo to axis labels
        + 18.0 // axis labels row
        + minimap_height()
}

pub fn show(ui: &mut egui::Ui, tab: &mut LogTab, theme: &Theme) {
    let zoomed = tab.timeline_zoom.is_some();

    let mut requested_mode = tab.timeline_display_mode;
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = icons::ACTION_HEIGHT;
        ui.spacing_mut().button_padding = egui::vec2(6.0, 3.0);
        filter_strip::add_filter_ui(ui, tab, theme);

        ui.separator();

        egui::Frame::new()
            .fill(theme.raised_surface)
            .stroke(Stroke::new(1.0, theme.border))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(4, 0))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for mode in TimelineDisplayMode::ALL {
                    let enabled = mode == TimelineDisplayMode::Line || tab.doc.time_range.is_some();
                    let response = ui.add_enabled(
                        enabled,
                        egui::Button::new(RichText::new(mode.label()).small())
                            .selected(requested_mode == mode),
                    );
                    if response.clicked() {
                        requested_mode = mode;
                    }
                    if !enabled {
                        response.on_disabled_hover_text("No valid log timestamps were detected");
                    }
                }
            });

        ui.separator();

        ui.spacing_mut().item_spacing.x = 8.0;

        if zoomed
            && ui
                .button(RichText::new("Reset zoom").small())
                .on_hover_text("Return to the full source range")
                .clicked()
        {
            tab.timeline_zoom = None;
        }

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
                    if icons::action_button(
                        ui,
                        Icon::Close,
                        "Clear selection",
                        theme.text,
                        "Clear the selected filter lane",
                    )
                    .clicked()
                    {
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
                    ui.label(RichText::new("Navigate").small().color(theme.text_muted))
                        .on_hover_text(
                            "Use Left and Right arrows to move through occurrences in the selected lane",
                        );
                    if let Some(text) = occurrence_navigation_text(tab) {
                        ui.add_space(10.0);
                        let changed = tab
                            .occurrence_navigation_animation
                            .as_ref()
                            .is_none_or(|(last_text, _)| last_text != &text);
                        if changed {
                            tab.occurrence_navigation_animation =
                                Some((text.clone(), std::time::Instant::now()));
                        }
                        let elapsed = tab
                            .occurrence_navigation_animation
                            .as_ref()
                            .map(|(_, at)| at.elapsed());
                        show_occurrence_navigation(ui, &text, theme, elapsed);
                    } else {
                        tab.occurrence_navigation_animation = None;
                    }
                },
            );
            if unselect {
                tab.selected_lane = None;
            }
        }
    });

    // Eliminate spacing between header and content immediately after header closes
    ui.spacing_mut().item_spacing.y = 0.0;

    if requested_mode != tab.timeline_display_mode {
        tab.set_timeline_display_mode(requested_mode);
    }
    let (full_start, full_end) = domain_span(&tab.timeline.domain, tab.doc.total_lines());
    let full_span = (full_end - full_start).max(1);
    let (view_start, view_end) = effective_zoom(&tab.timeline_zoom, full_start, full_end);
    // Everything Else lane only shown when filters exist.
    let n_filter_lanes = tab
        .timeline
        .filter_buckets
        .len()
        .min(MAX_LANES)
        .min(tab.filters.len());
    let has_filters = !tab.filters.is_empty();
    let has_pin_markers = tab
        .pins
        .iter()
        .any(|pin| pin.visible_bounds(tab.doc.total_lines()).is_some());
    let has_lanes = has_filters || has_pin_markers;
    let total_lanes = if has_lanes { n_filter_lanes + 1 } else { 0 };
    let lanes_height = total_lanes as f32 * LANE_HEIGHT;
    let content_height = HISTO_HEIGHT.max(lanes_height);

    let height = panel_height(tab) - HEADER_HEIGHT;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click_and_drag(),
    );
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, egui::CornerRadius::same(4), theme.log_surface);

    // ---- layout sub-rects ----
    // The main content area spans either from rect.min.x+4 (no filters) or
    // after the label column (with filters).
    let content_left = if has_lanes {
        rect.min.x + 4.0 + LABEL_WIDTH + 2.0
    } else {
        rect.min.x + 4.0
    };
    // Histo + lanes content rect
    let hist = Rect::from_min_max(
        Pos2::new(content_left, rect.min.y),
        Pos2::new(rect.max.x - 4.0, rect.min.y + content_height),
    );
    let lanes_bottom = hist.bottom();
    let axis_top = lanes_bottom + 6.0;
    let minimap = minimap_rect(hist, axis_top);

    // Bulk filter actions live in the otherwise-unused lower part of the
    // label column, keeping the title/input row quiet and aligned.
    if has_filters {
        let actions_rect = Rect::from_min_max(
            Pos2::new(rect.left() + 6.0, lanes_bottom + 5.0),
            Pos2::new(
                content_left - 6.0,
                lanes_bottom + 5.0 + icons::ACTION_HEIGHT,
            ),
        );
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(actions_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                ui.spacing_mut().interact_size.y = icons::ACTION_HEIGHT;
                ui.spacing_mut().item_spacing.x = 3.0;
                let all_active = tab.lane_active.iter().all(|&active| active);
                if icons::action_button(
                    ui,
                    if all_active {
                        Icon::Invisible
                    } else {
                        Icon::Visible
                    },
                    if all_active {
                        "Disable all"
                    } else {
                        "Enable all"
                    },
                    theme.text_muted,
                    if all_active {
                        "Disable every filter lane at once (Everything Else stays as-is)"
                    } else {
                        "Enable every filter lane at once"
                    },
                )
                .clicked()
                {
                    tab.toggle_all_lanes();
                }
                if icons::action_button(
                    ui,
                    Icon::Remove,
                    "Clear filters",
                    theme.text_muted,
                    "Remove every filter after confirmation",
                )
                .clicked()
                {
                    tab.pending_clear_filters = true;
                }
            },
        );
    }

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
    let density_bins = tab.timeline.resolve_density_bins(
        &tab.doc,
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
    let mut clicked_occurrence = false;
    let mut clicked_pin_marker = false;
    let mut navigate_to_pin: Option<usize> = None;
    // Lane hit targets can consume the pointer event before the outer timeline
    // response sees it. Keep the position so an empty lane click still moves
    // the log view.
    let mut lane_click_pos: Option<Pos2> = None;
    let mut lane_click_lane: Option<usize> = None;
    // Deferred ensure_visible (occurrence clicks happen inside an immutably-borrowed loop).
    let mut ensure_line: Option<usize> = None;

    if has_lanes {
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
            Pos2::new(label_col.left() + EYE_LEFT_PAD + 12.0, ee_y),
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
        let ee_marker_color = theme.text;
        let ee_icon = if tab.everything_else_active {
            Icon::Visible
        } else {
            Icon::Invisible
        };
        let ee_eye_rect = Rect::from_center_size(ee_marker_pos, Vec2::new(16.0, 14.0));
        icons::paint_icon(
            ui.ctx(),
            &painter,
            ee_icon,
            ee_eye_rect.center(),
            12.0,
            ee_marker_color,
        );
        let ee_eye_resp = ui.interact(ee_eye_rect, ui.id().with("ee_eye"), Sense::click());
        if ee_eye_resp.clicked() {
            toggle_ee = Some(!tab.everything_else_active);
        }
        if ee_eye_resp.hovered() || ee_eye_resp.has_focus() {
            painter.rect_stroke(
                ee_eye_rect.expand(2.0),
                egui::CornerRadius::same(2),
                Stroke::new(1.0, theme.focus_ring),
                egui::StrokeKind::Middle,
            );
            ee_eye_resp.on_hover_text(if tab.everything_else_active {
                "Hide Everything Else"
            } else {
                "Show Everything Else"
            });
        }
        // Label text, centered horizontally in the space after the eye icon
        let ee_text_left = label_col.left() + EYE_LEFT_PAD + 28.0;
        let ee_text_right = label_col.right() - LABEL_LANE_PAD - 12.0;
        let ee_text = fit_text_to_width(
            ui,
            "Everything Else",
            egui::FontId::monospace(12.0),
            (ee_text_right - ee_text_left).max(0.0),
        );
        let ee_label_pos = Pos2::new(ee_text_left, ee_y + LANE_HEIGHT / 2.0);
        painter.rect_filled(
            Rect::from_center_size(
                Pos2::new(
                    label_col.left() + EYE_LEFT_PAD + 20.0,
                    ee_y + LANE_HEIGHT / 2.0,
                ),
                Vec2::splat(6.0),
            ),
            egui::CornerRadius::same(1),
            theme.text,
        );
        painter.text(
            ee_label_pos,
            egui::Align2::LEFT_CENTER,
            ee_text,
            egui::FontId::monospace(12.0),
            if tab.everything_else_active {
                theme.text
            } else {
                theme.text_muted
            },
        );
        // No density line or occurrence buckets for Everything Else lane.
        // Its otherwise empty visual space is reserved for pinned evidence.
        let marker_y = ee_y + LANE_HEIGHT / 2.0;
        for (pin_index, pin) in tab.pins.iter().enumerate() {
            let Some((start_line, end_line)) = pin.visible_bounds(tab.doc.total_lines()) else {
                continue;
            };
            for (endpoint, line) in [("start", start_line), ("end", end_line)] {
                // A one-line pin has one marker, not two overlapping copies.
                if endpoint == "end" && end_line == start_line {
                    continue;
                }
                let value = x_of_line(&tab.doc, &tab.timeline.domain, line);
                if value < view_start || value > view_end {
                    continue;
                }
                let center = Pos2::new(x_to_px(value), marker_y);
                let marker_rect = Rect::from_center_size(center, Vec2::splat(12.0));
                let selected = tab.selected_pin == Some(pin_index);
                icons::paint_icon(
                    ui.ctx(),
                    &painter,
                    Icon::Pin,
                    center,
                    11.0,
                    if selected {
                        theme.accent
                    } else {
                        theme.analysis_text
                    },
                );
                let response = ui.interact(
                    marker_rect.expand(3.0),
                    ui.id().with(("pin_marker", pin_index, endpoint)),
                    Sense::click(),
                );
                if response.clicked() {
                    clicked_pin_marker = true;
                    navigate_to_pin = Some(pin_index);
                }
                if response.hovered() {
                    painter.rect_stroke(
                        marker_rect.expand(1.0),
                        egui::CornerRadius::same(2),
                        Stroke::new(1.0, theme.occurrence_hover),
                        egui::StrokeKind::Middle,
                    );
                    ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                    response.on_hover_text(pin_marker_tooltip(pin, start_line, end_line));
                }
            }
        }
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
            if lane_resp.clicked() {
                lane_click_pos = lane_resp.interact_pointer_pos();
                lane_click_lane = Some(ki);
                if is_active {
                    select_lane = Some(ki);
                }
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
                toggle_kw = Some((ki, !is_active));
            }
            // Native visible/invisible toggle button
            let kw_marker_pos =
                Pos2::new(label_col.left() + EYE_LEFT_PAD + 6.0, y + LANE_HEIGHT / 2.0);
            let kw_marker_color = theme.text;
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

            // Label text stays neutral: the swatch carries categorical lane
            // identity, while the label remains readable in both themes.
            let text = &tab.filters[ki].text;
            // Trash (remove) button on the right side of the label row, inset so
            // it never crowds the lane content next to the label column.
            let trash_center_x = label_col.right() - LABEL_LANE_PAD - 6.0;
            let trash_rect = Rect::from_center_size(
                Pos2::new(trash_center_x, y + LANE_HEIGHT / 2.0),
                Vec2::new(16.0, 14.0),
            );
            let trash_resp =
                ui.interact(trash_rect, ui.id().with(("kw_trash", ki)), Sense::click());
            if trash_resp.clicked() {
                tab.pending_filter_removal = Some(ki);
            }
            let trash_focused = trash_resp.hovered() || trash_resp.has_focus();
            let trash_color = theme.text;
            icons::paint_icon(
                ui.ctx(),
                &painter,
                Icon::Remove,
                trash_rect.center(),
                12.0,
                trash_color,
            );
            if trash_focused {
                painter.rect_stroke(
                    trash_rect.expand(2.0),
                    egui::CornerRadius::same(2),
                    Stroke::new(1.0, theme.focus_ring),
                    egui::StrokeKind::Middle,
                );
                trash_resp.on_hover_text(format!("Remove '{}'", text));
            }

            // Label text fills the available interval between the eye and trash
            // icons, truncating only when the actual rendered width requires it.
            let swatch_center = Pos2::new(
                label_col.left() + EYE_LEFT_PAD + 20.0,
                y + LANE_HEIGHT / 2.0,
            );
            painter.rect_filled(
                Rect::from_center_size(swatch_center, Vec2::splat(6.0)),
                egui::CornerRadius::same(1),
                if is_active { color } else { theme.text_muted },
            );
            let text_left = label_col.left() + EYE_LEFT_PAD + 28.0;
            let text_right = label_col.right() - LABEL_LANE_PAD - 12.0;
            let label_font = egui::FontId::proportional(12.0);
            let short = fit_text_to_width(
                ui,
                text,
                label_font.clone(),
                (text_right - text_left).max(0.0),
            );
            let label_pos = Pos2::new(text_left, y + LANE_HEIGHT / 2.0);

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
                            f.layout_no_wrap(short.clone(), label_font.clone(), theme.text)
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
                egui::Align2::LEFT_CENTER,
                &short,
                label_font,
                if is_active {
                    theme.text
                } else {
                    theme.text_muted
                },
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
                // Tooltip with the FULL filter text (the label may truncate to
                // fit) plus its total occurrence count.
                let count = tab.matches.get(ki).map_or(0, |m| m.len());
                let plural = if count == 1 { "" } else { "s" };
                let detail = if let Some(query) =
                    tab.filter_field_queries.get(ki).and_then(Option::as_ref)
                {
                    match query.compile(&tab.doc) {
                        Ok(_) => format!("{count} physical row{plural} in matching records"),
                        Err(error) => format!("Disabled for review: {error}"),
                    }
                } else {
                    format!("{count} occurrence{plural}")
                };
                kw_check_resp.on_hover_text(format!("{text} ({detail})"));
            }

            // Keep a very quiet baseline for orientation while disabled. The
            // occurrences themselves carry the lane's categorical colour.
            let line_y = y + LANE_HEIGHT / 2.0;
            if !is_active {
                painter.line_segment(
                    [
                        Pos2::new(hist.left(), line_y),
                        Pos2::new(hist.right(), line_y),
                    ],
                    Stroke::new(
                        1.0_f32,
                        Color32::from_rgba_unmultiplied(
                            theme.text_muted.r(),
                            theme.text_muted.g(),
                            theme.text_muted.b(),
                            55,
                        ),
                    ),
                );
                continue;
            }

            // The baseline is only an orientation cue. Full-strength colour is
            // reserved for actual occurrence markers and density buckets.
            painter.line_segment(
                [
                    Pos2::new(hist.left(), line_y),
                    Pos2::new(hist.right(), line_y),
                ],
                Stroke::new(
                    1.0_f32,
                    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 62),
                ),
            );
            // ---- exact adaptive filter occurrences ----
            // Resolve by marker footprint, not by the fixed whole-file buckets.
            // Zooming therefore splits close points as soon as the screen can
            // distinguish them, while every non-empty cell remains visible.
            let marker_columns = (hist.width() / OCCURRENCE_BUCKET_WIDTH).ceil().max(1.0) as usize;
            let resolved = tab.timeline.resolve_filter_bins(
                &tab.doc,
                ki,
                view_start,
                view_end,
                marker_columns,
            );
            for (bin_index, bin) in resolved.iter().enumerate() {
                if bin.count == 0 {
                    continue;
                }
                let cell_x0 =
                    hist.left() + bin_index as f32 * hist.width() / resolved.len().max(1) as f32;
                let cell_x1 = hist.left()
                    + (bin_index + 1) as f32 * hist.width() / resolved.len().max(1) as f32;
                let cy = y + LANE_HEIGHT / 2.0;

                if let Some((line_idx, xv)) = bin.sole_point {
                    let cx = x_to_px(xv);
                    let occurrence_rect = Rect::from_center_size(
                        Pos2::new(cx, cy),
                        Vec2::new(SINGLE_OCCURRENCE_WIDTH, SINGLE_OCCURRENCE_HEIGHT),
                    );
                    painter.rect_filled(occurrence_rect, egui::CornerRadius::same(1), color);

                    // Comfortable invisible hit target around the exact
                    // 2px-wide occurrence marker.
                    let click_rect =
                        Rect::from_center_size(Pos2::new(cx, cy), Vec2::new(8.0, 10.0));
                    let click_id = ui.id().with(("occurrence", ki, line_idx));
                    let click_resp = ui.interact(click_rect, click_id, Sense::click());
                    if click_resp.clicked() {
                        select_lane = Some(ki);
                        clicked_occurrence = true;
                        ensure_line = Some(line_idx as usize);
                    }
                    if click_resp.hovered() {
                        painter.rect_stroke(
                            occurrence_rect.expand(1.0),
                            egui::CornerRadius::same(1),
                            Stroke::new(1.5_f32, theme.occurrence_hover),
                            egui::StrokeKind::Middle,
                        );
                        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                        let tip = format!("{} — line {}", tab.filters[ki].text, line_idx + 1);
                        click_resp.on_hover_text(tip);
                    }
                } else {
                    let (height, alpha) = cluster_style(bin.count);
                    // Cluster width represents the resolved timeline bucket,
                    // while height/opacity represents its small/medium/large
                    // occurrence tier.
                    let left = cell_x0;
                    let right = cell_x1.max(left + 1.0);
                    let cluster_color =
                        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha);
                    let cluster_rect = Rect::from_min_max(
                        Pos2::new(left, cy - height / 2.0),
                        Pos2::new(right.max(left + 1.0), cy + height / 2.0),
                    );
                    painter.rect_filled(cluster_rect, egui::CornerRadius::ZERO, cluster_color);
                    let hit_rect = Rect::from_min_max(
                        Pos2::new(cell_x0, y),
                        Pos2::new(cell_x1, y + LANE_HEIGHT),
                    );
                    let cluster_resp = ui.interact(
                        hit_rect,
                        ui.id().with(("cluster", ki, bin_index)),
                        Sense::click(),
                    );
                    if cluster_resp.clicked() {
                        if let Some(pos) = cluster_resp.interact_pointer_pos() {
                            let xv = px_to_x(pos.x);
                            if let Some(line) =
                                tab.timeline.nearest_match_line_in_filter(&tab.doc, ki, xv)
                            {
                                select_lane = Some(ki);
                                clicked_occurrence = true;
                                ensure_line = Some(line);
                            }
                        }
                    }
                    if cluster_resp.hovered() {
                        painter.rect_stroke(
                            cluster_rect.expand(1.0),
                            egui::CornerRadius::ZERO,
                            Stroke::new(1.0, theme.occurrence_hover),
                            egui::StrokeKind::Middle,
                        );
                        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                        cluster_resp.on_hover_text(format!(
                            "{} — {} occurrences (boundary lines {} / {})",
                            tab.filters[ki].text,
                            bin.count,
                            bin.first_line + 1,
                            bin.last_line + 1,
                        ));
                    }
                }
            }
        }
    }

    // Apply deferred ensure_visible after occurrence clicks.
    if let Some(line) = ensure_line {
        tab.context_line = Some(line);
        tab.pending_scroll = Some(line);
        tab.ensure_visible();
    }

    if let Some(ki) = select_lane {
        // Lane selection is non-mutating. If an overlapping/retained egui
        // response also reported the eye control, discard that toggle so a
        // lane click can never hide the timeline data it just selected.
        toggle_kw = None;
        tab.set_lane_active(ki, true);
        tab.select_lane(ki);
    }

    if let Some(pin_index) = navigate_to_pin {
        tab.navigate_to_pin(pin_index);
    }

    // Apply deferred lane toggles.
    if let Some(active) = toggle_ee {
        tab.set_everything_else_active(active);
    }
    if let Some((ki, active)) = toggle_kw {
        if ki < tab.filters.len() {
            if !active && tab.selected_lane == Some(ki) {
                tab.selected_lane = None;
            }
            tab.set_lane_active(ki, active);
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
            let (v0, v1) =
                axis_bounds_for_line_range(tab, first_line, last_line).unwrap_or((-1, -1));
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
    let max_ticks_for_width = (hist.width() / 160.0).floor().clamp(2.0, 5.0) as usize;
    let domain_units = view_end.saturating_sub(view_start).saturating_add(1);
    let n_ticks = max_ticks_for_width.min(domain_units.clamp(1, 7) as usize);
    let label_y = lanes_bottom + 4.0;
    let font_id = egui::FontId::monospace(12.0);
    let mut tick_xs: Vec<f32> = Vec::with_capacity(n_ticks);
    let mut tick_vs: Vec<i64> = Vec::with_capacity(n_ticks);

    // Determine tick positions and values.
    for i in 0..n_ticks {
        let frac = if n_ticks == 1 {
            0.0
        } else {
            i as f64 / (n_ticks - 1) as f64
        };
        let v = view_start + (frac * view_span as f64) as i64;
        let x = hist.left() + frac as f32 * hist.width();
        tick_xs.push(x);
        tick_vs.push(v);
    }

    let tick_times: Vec<Option<i64>> = tick_vs
        .iter()
        .map(|&value| axis_timestamp(tab, value))
        .collect();
    // Time labels retain source-line spacing in Time mode and use elapsed
    // clock spacing in Real Time mode.
    let labels: Vec<String> = if tab.timeline_display_mode.shows_time_labels() {
        let first = tick_times.iter().flatten().next().copied();
        let last = tick_times.iter().flatten().next_back().copied();
        match first.zip(last) {
            Some((first, last)) => {
                let dt0 = chrono::DateTime::from_timestamp_millis(first);
                let dt1 = chrono::DateTime::from_timestamp_millis(last);
                match (dt0, dt1) {
                    (Some(d0), Some(d1)) => {
                        let same_date =
                            d0.format("%Y-%m-%d").to_string() == d1.format("%Y-%m-%d").to_string();
                        let same_hour =
                            same_date && d0.format("%H").to_string() == d1.format("%H").to_string();
                        tick_times
                            .iter()
                            .map(|value| {
                                if let Some(dt) =
                                    value.and_then(chrono::DateTime::from_timestamp_millis)
                                {
                                    if same_hour {
                                        dt.format("%M:%S%.3f").to_string()
                                    } else if same_date {
                                        dt.format("%H:%M:%S%.3f").to_string()
                                    } else {
                                        dt.format("%Y-%m-%d %H:%M:%S%.3f").to_string()
                                    }
                                } else {
                                    value.map_or_else(|| "Unknown".to_string(), format_ms)
                                }
                            })
                            .collect()
                    }
                    _ => tick_times
                        .iter()
                        .map(|value| value.map_or_else(|| "Unknown".to_string(), format_ms))
                        .collect(),
                }
            }
            None => vec!["Unknown".to_string(); tick_vs.len()],
        }
    } else {
        tick_vs.iter().map(|&v| format!("L{}", v + 1)).collect()
    };

    // Draw tick marks and labels.
    for i in 0..n_ticks {
        let x = tick_xs[i];
        // Tick mark.
        painter.line_segment(
            [Pos2::new(x, label_y), Pos2::new(x, label_y + 6.0)],
            Stroke::new(1.0_f32, theme.text),
        );
        // Label. Use an egui label here so the time/line captions can be bold.
        let galley = ui.ctx().fonts_mut(|fonts| {
            fonts.layout_no_wrap(labels[i].clone(), font_id.clone(), theme.text)
        });
        let label_rect = Rect::from_center_size(
            Pos2::new(x, label_y + 2.0 + galley.size().y / 2.0),
            galley.size(),
        );
        let tick_response = ui.put(
            label_rect,
            egui::Label::new(
                RichText::new(&labels[i])
                    .monospace()
                    .size(11.0)
                    .strong()
                    .color(theme.text),
            ),
        );
        if tab.timeline_display_mode == TimelineDisplayMode::Line {
            tick_response.on_hover_text(
                "L = physical source line; positions follow file order, not elapsed time",
            );
        } else if let Some(timestamp) = tick_times[i] {
            tick_response.on_hover_text(format!(
                "{} · source line L{}",
                format_ms(timestamp),
                tab.timeline.nearest_line(&tab.doc, tick_vs[i]).unwrap_or(0) + 1
            ));
        }
    }

    // ---- restrained duration labels between selected tick pairs ----
    let dur_font = egui::FontId::monospace(9.5);
    for i in 1..n_ticks {
        let mid_x = (tick_xs[i - 1] + tick_xs[i]) / 2.0;
        let delta = if tab.timeline_display_mode.shows_time_labels() {
            tick_times[i]
                .zip(tick_times[i - 1])
                .map(|(next, previous)| next - previous)
        } else {
            Some(tick_vs[i] - tick_vs[i - 1])
        };
        if let Some(delta) = delta {
            let dur_str = if tab.timeline_display_mode.shows_time_labels() {
                format_duration_ms(delta)
            } else {
                format!("Δ {} lines", delta)
            };
            // Draw between the label rows
            let dur_y = label_y + 2.0;
            painter.text(
                Pos2::new(mid_x, dur_y),
                egui::Align2::CENTER_TOP,
                &dur_str,
                dur_font.clone(),
                theme.text_muted,
            );
        }
    }

    // ---- minimap ----
    painter.rect_filled(minimap, egui::CornerRadius::same(2), theme.minimap_bg);
    // Draw full-range density in one bar per minimap pixel.
    let minimap_bins = tab.timeline.resolve_density_bins(
        &tab.doc,
        full_start,
        full_end,
        minimap.width().ceil() as usize,
    );
    let minimap_max = minimap_bins.iter().copied().max().unwrap_or(1).max(1) as f32;
    let minimap_color = minimap_color(theme);
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
            minimap_color,
        );
    }
    // Draw zoom window highlight on minimap.
    let frac_left = ((view_start - full_start) as f64 / full_span as f64).clamp(0.0, 1.0) as f32;
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
            let window = centered_window(center, view_span, full_start, full_end);
            tab.timeline_zoom = (window != (full_start, full_end)).then_some(window);
        }
    }
    if minimap_resp.hovered() {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
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
            let min_span = 1_i64;
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
        }
    }

    // ---- pan via drag (left button only) ----
    if !ui.input(|i| i.modifiers.shift) && response.dragged() {
        // Pointer delta is frame-local. Applying egui's cumulative drag_delta
        // to the already-shifted viewport compounds the movement every frame.
        let dx_px = ui.input(|i| i.pointer.delta().x);
        let dx_val = (dx_px as f64 / hist.width() as f64 * view_span as f64) as i64;
        let new_start = (view_start - dx_val).clamp(full_start, full_end - view_span);
        let new_end = new_start + view_span;
        if new_start == full_start && new_end == full_end {
            tab.timeline_zoom = None;
        } else {
            tab.timeline_zoom = Some((new_start, new_end));
        }
    }

    // ---- brush-select (shift+drag) ----
    let shift_down = ui.input(|i| i.modifiers.shift);
    if shift_down && response.drag_started() {
        tab.timeline_brush_start = response.interact_pointer_pos().map(|pos| pos.x);
    }
    if let Some(drag_origin_x) = tab.timeline_brush_start {
        let current_x = ui
            .input(|i| i.pointer.hover_pos())
            .map_or(drag_origin_x, |pos| pos.x);
        if response.dragged() {
            let brush_left = drag_origin_x
                .min(current_x)
                .clamp(hist.left(), hist.right());
            let brush_right = drag_origin_x
                .max(current_x)
                .clamp(hist.left(), hist.right());
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
            tab.timeline_brush_start = None;
            if (current_x - drag_origin_x).abs() >= 4.0 {
                let x1 = px_to_x(drag_origin_x.min(current_x));
                let x2 = px_to_x(drag_origin_x.max(current_x));
                if x2 > x1 {
                    tab.timeline_zoom = Some((x1.max(full_start), x2.min(full_end)));
                }
            }
        }
    }

    // ---- double-click to reset zoom and snap to nearest match ----
    if response.double_clicked() {
        tab.timeline_zoom = None;
        if let Some(pos) = response.interact_pointer_pos() {
            let v = px_to_x(pos.x);
            let target = tab
                .timeline
                .nearest_match_line(&tab.doc, v)
                .or_else(|| tab.timeline.nearest_line(&tab.doc, v));
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
            if !response.dragged() && !clicked_occurrence && !clicked_pin_marker {
                let v = px_to_x(pos.x);
                let target = tab.timeline.nearest_line(&tab.doc, v);
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
                ui.label(RichText::new(v_caption(tab, v)).strong());
                let density_column = (((pos.x - hist.left()) / hist.width())
                    * density_bins.len() as f32)
                    .floor()
                    .clamp(0.0, density_bins.len().saturating_sub(1) as f32)
                    as usize;
                ui.label(format!(
                    "{} lines in visible column",
                    density_bins[density_column]
                ));
                let marker_half_span = ((view_span as f64 * OCCURRENCE_BUCKET_WIDTH as f64
                    / hist.width().max(1.0) as f64)
                    / 2.0)
                    .ceil() as i64;
                for ki in 0..tab
                    .timeline
                    .filter_lines
                    .len()
                    .min(MAX_LANES.min(tab.filters.len()))
                {
                    let count = tab.timeline.point_count_in_range(
                        &tab.doc,
                        ki,
                        v.saturating_sub(marker_half_span),
                        v.saturating_add(marker_half_span),
                    );
                    if count > 0 {
                        ui.label(
                            RichText::new(format!("▌ {} ×{}", tab.filters[ki].text, count))
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
    format!("Δ {}", format_human_duration_ms(ms))
}

fn axis_timestamp(tab: &LogTab, value: i64) -> Option<i64> {
    match tab.timeline_display_mode {
        TimelineDisplayMode::Line => None,
        TimelineDisplayMode::Time => usize::try_from(value)
            .ok()
            .and_then(|line| tab.doc.ts_at_opt(line))
            .filter(|timestamp| *timestamp >= 0),
        TimelineDisplayMode::RealTime => Some(value),
    }
}

fn unsigned_human_duration_ms(ms: u64) -> String {
    if ms >= 86_400_000 {
        let days = ms / 86_400_000;
        let hours = (ms % 86_400_000) / 3_600_000;
        if hours == 0 {
            format!("{days}d")
        } else {
            format!("{days}d {hours}h")
        }
    } else if ms >= 3_600_000 {
        let hours = ms / 3_600_000;
        let minutes = (ms % 3_600_000) / 60_000;
        if minutes == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h {minutes}m")
        }
    } else if ms >= 60_000 {
        let minutes = ms / 60_000;
        let seconds = (ms % 60_000) / 1000;
        if seconds == 0 {
            format!("{minutes}m")
        } else {
            format!("{minutes}m {seconds}s")
        }
    } else if ms >= 1000 {
        let seconds = ms / 1000;
        let remainder = ms % 1000;
        if remainder == 0 {
            format!("{seconds}s")
        } else {
            format!("{:.1}s", ms as f64 / 1000.0)
        }
    } else {
        format!("{ms}ms")
    }
}

/// Visual tiers for collision cells. Exact counts remain available in the
/// tooltip; the tiers make small, medium, and dense groups distinguishable at
/// a glance without letting a two-point cluster disappear into the baseline.
fn cluster_style(count: u32) -> (f32, u8) {
    match count {
        0 | 1 => (0.0, 0),
        count if count <= SMALL_BUCKET_MAX_OCCURRENCES => (SMALL_BUCKET_HEIGHT, 170),
        count if count <= MEDIUM_BUCKET_MAX_OCCURRENCES => (MEDIUM_BUCKET_HEIGHT, 210),
        _ => (DENSE_BUCKET_HEIGHT, 245),
    }
}

fn minimap_rect(hist: Rect, axis_top: f32) -> Rect {
    let top = axis_top + MINIMAP_AXIS_OFFSET;
    Rect::from_min_max(
        Pos2::new(hist.left(), top),
        Pos2::new(hist.right(), top + MINIMAP_HEIGHT),
    )
}

fn format_human_duration_ms(ms: i64) -> String {
    let value = unsigned_human_duration_ms(ms.unsigned_abs());
    if ms < 0 {
        format!("−{value}")
    } else {
        value
    }
}

fn occurrence_navigation_text(tab: &LogTab) -> Option<String> {
    let (lane, line) = tab.selected_occurrence()?;
    let matches = tab.matches.get(lane)?;
    let position = matches
        .iter()
        .position(|&match_line| match_line as usize == line)?;
    let previous = if position > 0 {
        let previous = matches[position - 1] as usize;
        let delta = if tab.timeline_display_mode.shows_time_labels() {
            tab.doc
                .ts_at_opt(line)
                .zip(tab.doc.ts_at_opt(previous))
                .filter(|(current, previous)| *current >= 0 && *previous >= 0)
                .map(|(current, previous)| format_human_duration_ms(current - previous))
                .unwrap_or_else(|| format!("{} lines", line - previous))
        } else {
            format!("{} lines", line - previous)
        };
        Some(delta)
    } else {
        None
    };
    let next = if position + 1 < matches.len() {
        let next = matches[position + 1] as usize;
        let delta = if tab.timeline_display_mode.shows_time_labels() {
            tab.doc
                .ts_at_opt(next)
                .zip(tab.doc.ts_at_opt(line))
                .filter(|(next, current)| *next >= 0 && *current >= 0)
                .map(|(next, current)| format_human_duration_ms(next - current))
                .unwrap_or_else(|| format!("{} lines", next - line))
        } else {
            format!("{} lines", next - line)
        };
        Some(delta)
    } else {
        None
    };

    Some(format_occurrence_navigation_text(
        position + 1,
        matches.len(),
        next.as_deref(),
        previous.as_deref(),
    ))
}

fn format_occurrence_navigation_text(
    position: usize,
    total: usize,
    next: Option<&str>,
    previous: Option<&str>,
) -> String {
    format!(
        "OCCURRENCE: {position}/{total}  |  PREV: {} ago  |  NEXT: after {}",
        previous.unwrap_or("—"),
        next.unwrap_or("—")
    )
}

fn show_occurrence_navigation(
    ui: &mut egui::Ui,
    text: &str,
    theme: &Theme,
    elapsed: Option<Duration>,
) {
    let progress = elapsed
        .map(|elapsed| (elapsed.as_secs_f32() / 0.1).clamp(0.0, 1.0))
        .unwrap_or(1.0);
    if progress < 1.0 {
        ui.ctx().request_repaint();
    }
    let offset = (1.0 - progress) * 3.0;
    let animated_color = |color: Color32| {
        Color32::from_rgba_unmultiplied(
            color.r(),
            color.g(),
            color.b(),
            (color.a() as f32 * (0.65 + progress * 0.35)) as u8,
        )
    };
    ui.add_space(offset);
    ui.label(
        RichText::new(text)
            .size(12.0)
            .color(animated_color(theme.text_muted)),
    );
}

fn domain_span(domain: &TimelineDomain, total_lines: usize) -> (i64, i64) {
    match domain {
        TimelineDomain::Time { start_ms, end_ms } => (*start_ms, *end_ms),
        TimelineDomain::Sequence => (0, total_lines.saturating_sub(1) as i64),
    }
}

fn effective_zoom(zoom: &Option<(i64, i64)>, full_start: i64, full_end: i64) -> (i64, i64) {
    match zoom {
        Some((s, e)) => ((*s).max(full_start), (*e).min(full_end)),
        None => (full_start, full_end),
    }
}

/// Center a fixed-span inclusive viewport while preserving its size at both
/// domain boundaries.
fn centered_window(center: i64, span: i64, full_start: i64, full_end: i64) -> (i64, i64) {
    let span = span.max(0).min(full_end.saturating_sub(full_start));
    let start = center
        .saturating_sub(span / 2)
        .clamp(full_start, full_end.saturating_sub(span));
    (start, start.saturating_add(span).min(full_end))
}

fn x_of_line(doc: &LogDocument, domain: &TimelineDomain, line: usize) -> i64 {
    match domain {
        TimelineDomain::Time { .. } => doc.ts_at(line),
        TimelineDomain::Sequence => line as i64,
    }
}

fn axis_bounds_for_line_range(
    tab: &LogTab,
    first_line: usize,
    last_line: usize,
) -> Option<(i64, i64)> {
    if !tab.timeline_display_mode.uses_real_time_coordinates() {
        return Some((first_line as i64, last_line as i64));
    }
    let end = last_line.min(tab.doc.total_lines().saturating_sub(1));
    let mut min = i64::MAX;
    let mut max = i64::MIN;
    for line in first_line.min(end)..=end {
        let timestamp = tab.doc.ts_at(line);
        if timestamp >= 0 {
            min = min.min(timestamp);
            max = max.max(timestamp);
        }
    }
    (min <= max).then_some((min, max))
}

/// Text shown by a Timeline pin marker. The user analysis stays first so the
/// tooltip answers the investigation question before showing navigation detail.
fn pin_marker_tooltip(
    pin: &crate::ui::app::model::PinEntry,
    start_line: usize,
    end_line: usize,
) -> String {
    let analysis = if pin.comment.is_empty() {
        "No analysis added."
    } else {
        &pin.comment
    };
    if start_line == end_line {
        format!("{analysis}\nPinned line {}", start_line + 1)
    } else {
        format!(
            "{analysis}\nPinned lines {}–{}",
            start_line + 1,
            end_line + 1
        )
    }
}

fn v_caption(tab: &LogTab, v: i64) -> String {
    match tab.timeline_display_mode {
        TimelineDisplayMode::Line => format!("source line L{}", v + 1),
        TimelineDisplayMode::Time => axis_timestamp(tab, v)
            .map(format_ms)
            .unwrap_or_else(|| "Unknown log time".to_string()),
        TimelineDisplayMode::RealTime => format_ms(v),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_duration_formats_occurrence_deltas() {
        assert_eq!(format_human_duration_ms(3_900_000), "1h 5m");
        assert_eq!(format_human_duration_ms(1_500), "1.5s");
        assert_eq!(format_human_duration_ms(200), "200ms");
        assert_eq!(format_human_duration_ms(165_600_000), "1d 22h");
        assert_eq!(format_human_duration_ms(-5_000), "−5s");
    }

    #[test]
    fn occurrence_navigation_uses_one_fixed_label_template() {
        assert_eq!(
            format_occurrence_navigation_text(2, 8, Some("200ms"), Some("1.5sec")),
            "OCCURRENCE: 2/8  |  PREV: 1.5sec ago  |  NEXT: after 200ms"
        );
        assert_eq!(
            format_occurrence_navigation_text(1, 1, None, None),
            "OCCURRENCE: 1/1  |  PREV: — ago  |  NEXT: after —"
        );
    }

    #[test]
    fn cluster_styles_distinguish_small_medium_and_dense_groups() {
        assert_eq!(cluster_style(1), (0.0, 0));
        assert_eq!(cluster_style(2), (SMALL_BUCKET_HEIGHT, 170));
        assert_eq!(
            cluster_style(SMALL_BUCKET_MAX_OCCURRENCES),
            (SMALL_BUCKET_HEIGHT, 170)
        );
        assert_eq!(
            cluster_style(SMALL_BUCKET_MAX_OCCURRENCES + 1),
            (MEDIUM_BUCKET_HEIGHT, 210)
        );
        assert_eq!(
            cluster_style(MEDIUM_BUCKET_MAX_OCCURRENCES),
            (MEDIUM_BUCKET_HEIGHT, 210)
        );
        assert_eq!(
            cluster_style(MEDIUM_BUCKET_MAX_OCCURRENCES + 1),
            (DENSE_BUCKET_HEIGHT, 245)
        );
        assert_eq!(
            (SINGLE_OCCURRENCE_WIDTH, SINGLE_OCCURRENCE_HEIGHT),
            (3.0, 7.0)
        );
        assert_eq!(SMALL_BUCKET_HEIGHT, SINGLE_OCCURRENCE_HEIGHT);
        assert_eq!(MEDIUM_BUCKET_HEIGHT, 9.0);
        assert_eq!(DENSE_BUCKET_HEIGHT, 11.0);
    }

    #[test]
    fn minimap_uses_the_compacted_axis_offset() {
        let hist = Rect::from_min_size(Pos2::new(10.0, 100.0), Vec2::new(200.0, 68.0));
        let minimap = minimap_rect(hist, hist.bottom() + 6.0);

        assert_eq!(minimap.top(), hist.bottom() + 6.0 + 16.0);
        assert_eq!(minimap.bottom(), minimap.top() + MINIMAP_HEIGHT);
        assert_eq!(minimap.left(), hist.left());
        assert_eq!(minimap.right(), hist.right());
    }

    #[test]
    fn minimap_is_always_visible_with_default_height() {
        assert_eq!(minimap_height(), 8.0 + MINIMAP_HEIGHT);
    }

    #[test]
    fn minimap_default_color_follows_the_active_theme() {
        assert_eq!(
            minimap_color(&Theme::light()),
            Color32::from_rgba_unmultiplied(100, 100, 100, 255)
        );
        assert_eq!(
            minimap_color(&Theme::dark()),
            Color32::from_rgba_unmultiplied(130, 130, 130, 255)
        );
    }

    #[test]
    fn centered_window_preserves_span_at_boundaries() {
        assert_eq!(centered_window(0, 20, 0, 100), (0, 20));
        assert_eq!(centered_window(100, 20, 0, 100), (80, 100));
        assert_eq!(centered_window(50, 20, 0, 100), (40, 60));
        assert_eq!(centered_window(50, 200, 0, 100), (0, 100));
    }

    #[test]
    fn pin_marker_tooltip_leads_with_user_analysis_and_range() {
        let pin = crate::ui::app::model::PinEntry {
            start_line: 4,
            line_numbers: vec![4, 7],
            start_ts: 10,
            end_ts: 20,
            comment: "Timeout begins here".into(),
            unanchored: false,
        };
        assert_eq!(
            pin_marker_tooltip(&pin, 4, 7),
            "Timeout begins here\nPinned lines 5–8"
        );
    }
}
