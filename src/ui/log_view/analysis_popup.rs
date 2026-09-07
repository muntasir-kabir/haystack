//! Movable analysis bubbles for line and range pinning.
//!
//! This is deliberately separate from `annotation_popup`: annotation callouts
//! are hover-driven source previews, while this popup owns the user's explicit
//! pin/analysis flow.

use eframe::egui;
use egui::{Color32, Pos2, Rect, Stroke};

use super::view::lines_text;
use crate::ui::app::model::{AnalysisPopupMode, AnalysisPopupState, LogTab};
use crate::ui::icons::{self, Icon};
use crate::ui::theme::Theme;

const POPUP_INSET: f32 = 8.0;
const POPUP_GAP: f32 = 8.0;
const ACTION_WIDTH: f32 = 280.0;
const EDITOR_WIDTH: f32 = 390.0;
const EDITOR_HEIGHT: f32 = 190.0;
const MIN_WIDTH: f32 = 220.0;
const MIN_HEIGHT: f32 = 54.0;
const MAX_HEIGHT: f32 = 520.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Save { range: (usize, usize), text: String },
    Cancel,
}

pub(crate) fn open_actions(
    tab: &mut LogTab,
    range: (usize, usize),
    cursor: Pos2,
    selected_rect: Rect,
) {
    let cursor_rect = Rect::from_center_size(cursor, egui::vec2(1.0, 1.0));
    open(
        tab,
        range,
        cursor_rect,
        selected_rect,
        AnalysisPopupMode::Actions,
        true,
    );
}

pub(crate) fn open_editor(
    tab: &mut LogTab,
    range: (usize, usize),
    anchor_rect: Rect,
    from_selection: bool,
) {
    open(
        tab,
        range,
        anchor_rect,
        anchor_rect,
        AnalysisPopupMode::Editor,
        from_selection,
    );
}

fn open(
    tab: &mut LogTab,
    range: (usize, usize),
    anchor_rect: Rect,
    selected_rect: Rect,
    mode: AnalysisPopupMode,
    from_selection: bool,
) {
    tab.next_analysis_popup_id = tab.next_analysis_popup_id.wrapping_add(1);
    tab.pin_comment.clear();
    tab.analysis_popup = Some(AnalysisPopupState {
        range,
        anchor_rect,
        selected_rect,
        mode,
        from_selection,
        id: tab.next_analysis_popup_id,
        ignore_outside_click: true,
    });
}

/// Switch an action bubble into the editor while preserving the selected-line
/// anchor. The mode-specific egui ID lets the larger editor recalculate its
/// initial top/bottom placement instead of expanding over the selection.
fn switch_to_editor(tab: &mut LogTab) {
    tab.pin_comment.clear();
    if let Some(state) = tab.analysis_popup.as_mut() {
        state.anchor_rect = state.selected_rect;
        state.mode = AnalysisPopupMode::Editor;
    }
}

pub(crate) fn dismiss(tab: &mut LogTab) {
    let from_selection = tab
        .analysis_popup
        .take()
        .is_some_and(|state| state.from_selection);
    tab.pin_comment.clear();
    if from_selection {
        clear_selection(tab);
    }
}

pub(crate) fn clear_after_save(tab: &mut LogTab) {
    let from_selection = tab
        .analysis_popup
        .take()
        .is_some_and(|state| state.from_selection);
    tab.pin_comment.clear();
    if from_selection {
        clear_selection(tab);
    }
}

fn clear_selection(tab: &mut LogTab) {
    tab.selection_range = None;
    tab.pending_selection = None;
    tab.drag_start_pos = None;
    tab.drag_start_line = None;
    tab.drag_current_line = None;
    tab.selection_popup_pos = None;
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    tab: &mut LogTab,
    theme: &Theme,
    escape_pressed: bool,
) -> Option<Action> {
    let state = tab.analysis_popup?;
    let screen = ui.ctx().content_rect();
    let default_size = popup_size(state.mode, screen);
    let default_pos = popup_position(state.anchor_rect, screen, default_size);
    let popup_mode = match state.mode {
        AnalysisPopupMode::Actions => "actions",
        AnalysisPopupMode::Editor => "editor",
    };
    let popup_id = egui::Id::new(("analysis_popup", state.id, popup_mode));

    let mut switch_editor = false;
    let mut save = false;
    let mut cancel = false;
    let mut copy: Option<bool> = None;

    let area = egui::Window::new("analysis_popup")
        .id(popup_id)
        .order(egui::Order::Foreground)
        .title_bar(false)
        .collapsible(false)
        .movable(true)
        .resizable(true)
        .default_pos(default_pos)
        .default_size(default_size)
        .min_size(egui::vec2(MIN_WIDTH, MIN_HEIGHT))
        .max_size(egui::vec2(
            (screen.width() - POPUP_INSET * 2.0).max(MIN_WIDTH),
            (screen.height() - POPUP_INSET * 2.0)
                .min(MAX_HEIGHT)
                .max(MIN_HEIGHT),
        ))
        .constrain_to(screen.shrink(POPUP_INSET))
        .frame(
            egui::Frame::new()
                .fill(scaled_alpha(theme.surface, 0.96))
                .stroke(Stroke::new(1.0, scaled_alpha(theme.accent, 0.72)))
                .corner_radius(egui::CornerRadius::same(7))
                .inner_margin(egui::Margin::same(10)),
        )
        .show(ui.ctx(), |ui| match state.mode {
            AnalysisPopupMode::Actions => {
                ui.horizontal(|ui| {
                    if icons::action_button(
                        ui,
                        Icon::Pin,
                        "Pin",
                        theme.text,
                        "Pin these rows and add an optional analysis",
                    )
                    .clicked()
                    {
                        switch_editor = true;
                    }
                    if icons::action_button(
                        ui,
                        Icon::Copy,
                        "Copy",
                        theme.text,
                        "Copy the selected log text",
                    )
                    .clicked()
                    {
                        copy = Some(false);
                    }
                    if icons::action_button(
                        ui,
                        Icon::Copy,
                        "Copy with lines",
                        theme.text,
                        "Copy the selected log text with line numbers",
                    )
                    .clicked()
                    {
                        copy = Some(true);
                    }
                    if icons::icon_action_button(ui, Icon::Close, theme.text, "Cancel selection")
                        .clicked()
                    {
                        cancel = true;
                    }
                });
            }
            AnalysisPopupMode::Editor => {
                let response = ui.add_sized(
                    egui::vec2(
                        ui.available_width(),
                        ui.available_height().max(EDITOR_HEIGHT - 28.0),
                    ),
                    egui::TextEdit::multiline(&mut tab.pin_comment)
                        .hint_text("Add analysis/info or ↵ to save")
                        .desired_width(f32::INFINITY),
                );
                response.request_focus();
                let enter_pressed =
                    ui.input(|input| input.key_pressed(egui::Key::Enter) && !input.modifiers.shift);
                let escape_pressed = ui.input(|input| input.key_pressed(egui::Key::Escape));
                if response.has_focus() && enter_pressed {
                    save = true;
                }
                if response.has_focus() && escape_pressed {
                    cancel = true;
                }
            }
        });

    let Some(area) = area else {
        return None;
    };
    let bubble_rect = area.response.rect;

    draw_arrow(
        ui.ctx(),
        area.response.layer_id,
        state.anchor_rect,
        bubble_rect,
        theme,
    );

    if let Some(copy_with_line_numbers) = copy {
        let (start, end) = state.range;
        ui.ctx()
            .copy_text(lines_text(tab, start, end, copy_with_line_numbers, false));
        tab.pending_toast = Some(if copy_with_line_numbers {
            "Selected lines copied".to_owned()
        } else {
            "Selected text copied".to_owned()
        });
        // Copy is a terminal action for the selection bubble. Clear both the
        // popup and its transient selected range so the overlay does not
        // remain over the log after the clipboard action completes.
        dismiss(tab);
        return None;
    }

    if switch_editor {
        switch_to_editor(tab);
    }

    let outside_click = ui.ctx().input(|input| {
        input
            .pointer
            .any_click()
            .then(|| input.pointer.interact_pos())
            .flatten()
    });
    let should_cancel = cancel
        || escape_pressed
        || (outside_click.is_some_and(|position| !bubble_rect.contains(position))
            && !state.ignore_outside_click);

    if should_cancel {
        return Some(Action::Cancel);
    }
    if save {
        return Some(Action::Save {
            range: state.range,
            text: tab.pin_comment.clone(),
        });
    }

    if let Some(current) = tab.analysis_popup.as_mut() {
        current.ignore_outside_click = false;
    }
    None
}

fn popup_size(mode: AnalysisPopupMode, screen: Rect) -> egui::Vec2 {
    let max_width = (screen.width() - POPUP_INSET * 2.0).max(MIN_WIDTH);
    match mode {
        AnalysisPopupMode::Actions => egui::vec2(ACTION_WIDTH.min(max_width), 50.0),
        AnalysisPopupMode::Editor => egui::vec2(EDITOR_WIDTH.min(max_width), EDITOR_HEIGHT),
    }
}

fn popup_position(anchor: Rect, screen: Rect, size: egui::Vec2) -> Pos2 {
    let min_x = screen.left() + POPUP_INSET;
    let max_x = (screen.right() - size.x - POPUP_INSET).max(min_x);
    let x = (anchor.center().x - size.x * 0.5).clamp(min_x, max_x);

    let space_above = anchor.top() - screen.top() - POPUP_INSET;
    let space_below = screen.bottom() - anchor.bottom() - POPUP_INSET;
    let above = space_above >= size.y || space_above > space_below;
    let preferred_y = if above {
        anchor.top() - size.y - POPUP_GAP
    } else {
        anchor.bottom() + POPUP_GAP
    };
    let min_y = screen.top() + POPUP_INSET;
    let max_y = (screen.bottom() - size.y - POPUP_INSET).max(min_y);
    Pos2::new(x, preferred_y.clamp(min_y, max_y))
}

fn draw_arrow(
    ctx: &egui::Context,
    layer_id: egui::LayerId,
    anchor: Rect,
    bubble: Rect,
    theme: &Theme,
) {
    let above = bubble.center().y < anchor.center().y;
    let mut tip = if above {
        anchor.center_top()
    } else {
        anchor.center_bottom()
    };
    // A very large/resized bubble can be clamped over its source when neither
    // side has enough room. Never draw the arrow tip inside the bubble: keep
    // it just beyond the relevant edge so the arrow remains visible and
    // communicates the source direction.
    if bubble.contains(tip) {
        tip = if above {
            Pos2::new(tip.x, bubble.top() - POPUP_GAP)
        } else {
            Pos2::new(tip.x, bubble.bottom() + POPUP_GAP)
        };
    }
    let edge_y = if above { bubble.bottom() } else { bubble.top() };
    let edge_x = tip.x.clamp(bubble.left() + 10.0, bubble.right() - 10.0);
    let half_width = 6.0;
    let points = vec![
        Pos2::new(edge_x - half_width, edge_y),
        Pos2::new(edge_x + half_width, edge_y),
        tip,
    ];
    ctx.layer_painter(layer_id).add(egui::Shape::convex_polygon(
        points,
        scaled_alpha(theme.surface, 0.96),
        Stroke::new(1.0, scaled_alpha(theme.accent, 0.72)),
    ));
}

fn scaled_alpha(color: Color32, factor: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        color.r(),
        color.g(),
        color.b(),
        (color.a() as f32 * factor).round() as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use logotomy::core::document::LogDocument;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_TEST_FILE: AtomicUsize = AtomicUsize::new(0);

    fn test_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "logotomy-analysis-popup-{}-{}.log",
            std::process::id(),
            NEXT_TEST_FILE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn test_tab() -> (LogTab, PathBuf) {
        let path = test_path();
        std::fs::write(&path, "first\nsecond\n").unwrap();
        (LogTab::new(LogDocument::open(&path).unwrap()), path)
    }

    #[test]
    fn test_fixture_path_is_windows_safe() {
        let path = test_path();
        let file_name = path.file_name().unwrap().to_str().unwrap();
        assert!(!file_name.contains(['<', '>', ':', '"', '/', '\\', '|', '?', '*']));
    }

    #[test]
    fn popup_position_prefers_below_when_space_allows() {
        let screen = Rect::from_min_size(Pos2::new(0.0, 0.0), egui::vec2(800.0, 600.0));
        let anchor = Rect::from_min_size(Pos2::new(360.0, 100.0), egui::vec2(1.0, 1.0));
        let position = popup_position(anchor, screen, egui::vec2(300.0, 100.0));
        assert!(position.y > anchor.bottom());
        assert!((position.x - 210.0).abs() < 1.0);
        assert!(!Rect::from_min_size(position, egui::vec2(300.0, 100.0)).contains(anchor.center()));
    }

    #[test]
    fn popup_position_clamps_to_screen_edges() {
        let screen = Rect::from_min_size(Pos2::new(0.0, 0.0), egui::vec2(320.0, 180.0));
        let anchor = Rect::from_min_size(Pos2::new(2.0, 2.0), egui::vec2(20.0, 18.0));
        let position = popup_position(anchor, screen, egui::vec2(280.0, 130.0));
        assert!(position.x >= POPUP_INSET);
        assert!(position.y >= POPUP_INSET);
    }

    #[test]
    fn dismissing_selection_popup_clears_the_transient_selection() {
        let (mut tab, path) = test_tab();
        tab.selection_range = Some((0, 1));
        tab.pending_selection = Some((0, 1));
        open_actions(
            &mut tab,
            (0, 1),
            Pos2::new(20.0, 20.0),
            Rect::from_min_size(Pos2::new(20.0, 20.0), egui::vec2(80.0, 36.0)),
        );

        assert_eq!(
            tab.analysis_popup.map(|state| state.mode),
            Some(AnalysisPopupMode::Actions)
        );
        dismiss(&mut tab);
        assert!(tab.analysis_popup.is_none());
        assert!(tab.selection_range.is_none());
        assert!(tab.pending_selection.is_none());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn selection_actions_use_cursor_anchor_and_retain_selected_bounds() {
        let (mut tab, path) = test_tab();
        let selected = Rect::from_min_size(Pos2::new(180.0, 160.0), egui::vec2(260.0, 36.0));
        let release = Pos2::new(410.0, 240.0);
        open_actions(&mut tab, (0, 1), release, selected);

        assert_eq!(
            tab.analysis_popup.map(|state| state.anchor_rect),
            Some(Rect::from_center_size(release, egui::vec2(1.0, 1.0)))
        );
        assert_eq!(
            tab.analysis_popup.map(|state| state.selected_rect),
            Some(selected)
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn switching_selection_actions_to_editor_uses_selected_bounds() {
        let (mut tab, path) = test_tab();
        let selected = Rect::from_min_size(Pos2::new(180.0, 160.0), egui::vec2(260.0, 36.0));
        let release = Pos2::new(410.0, 240.0);
        open_actions(&mut tab, (0, 1), release, selected);

        switch_to_editor(&mut tab);

        let state = tab.analysis_popup.unwrap();
        assert_eq!(state.mode, AnalysisPopupMode::Editor);
        assert_eq!(state.anchor_rect, selected);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn dismissing_context_popup_does_not_clear_existing_selection() {
        let (mut tab, path) = test_tab();
        tab.selection_range = Some((0, 1));
        open_editor(
            &mut tab,
            (0, 0),
            Rect::from_min_size(Pos2::new(20.0, 20.0), egui::vec2(80.0, 18.0)),
            false,
        );

        dismiss(&mut tab);
        assert!(tab.analysis_popup.is_none());
        assert_eq!(tab.selection_range, Some((0, 1)));
        std::fs::remove_file(path).ok();
    }
}
