//! Shared interaction ownership for app-level dialogs and popovers.
//!
//! egui lays widgets out in call order. Several Haystack views also read raw
//! input directly, so a foreground window alone cannot stop an earlier view
//! from reacting to the same event. The app records that an interactive
//! surface owns the frame before drawing the base views; raw-input handlers
//! consult this marker before consuming keyboard or pointer input.

use eframe::egui;

use super::model::HaystackApp;

const BACKGROUND_INPUT_BLOCKED: &str = "haystack_background_input_blocked";

/// Returns whether any interactive foreground surface owns application input.
///
/// This intentionally includes both modal dialogs and transient popovers.
/// Popovers dismiss on an outside click, but that click must not fall through
/// to a log row or another base-view control.
pub(super) fn interactive_surface_open(app: &HaystackApp, ctx: &egui::Context) -> bool {
    let focused = ctx.memory(|memory| memory.focused());
    let tab_surface_open = app
        .active
        .and_then(|index| app.tabs.get(index))
        .is_some_and(|tab| {
            tab.pending_filter_removal.is_some()
                || tab.pending_clear_filters
                || tab.pin_modal.is_some()
                || tab.filter_suggestions_open
                || (tab.filter_input_field_mode
                    && focused == Some(egui::Id::new("timeline_filter_input")))
                || tab.log_views.values().any(|view| {
                    view.full_line_inspector.is_some()
                        || view.embedded_inspector.is_some()
                        || view.analysis_popup.is_some()
                        || view.occurrence_overlay.is_some()
                        || view.search_suggestions_open
                        || (view.find_field_mode
                            && focused == Some(egui::Id::new(("log_find_input", view.id))))
                        || view
                            .annotation_hover
                            .as_ref()
                            .is_some_and(|state| state.bubble_rect.is_some())
                })
        });

    app.record_editor.open
        || app.show_custom_date_popup
        || app.show_integrate_popup
        || app.show_command_palette
        || app.show_goto_popup
        || app.show_cheat_sheet
        || app.show_new_filter_popup
        || app.show_rename_filter_popup
        || app.mcp_error_popup.is_some()
        || app.pending_restore_tab.is_some()
        || app.pending_sidecar_recovery.is_some()
        || app.close_save_error.is_some()
        || app.zip_imports.has_interactive_surface()
        || app.show_settings_popup
        || app.show_ai_assistant_popup
        || app.recent_show_dropdown
        || app.show_filter_dropdown
        || app.views_show_dropdown
        || app.show_format_dropdown
        || tab_surface_open
}

pub(super) fn mark_background_input(ctx: &egui::Context, blocked: bool) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new(BACKGROUND_INPUT_BLOCKED), blocked));
}

pub(crate) fn background_input_blocked(ctx: &egui::Context) -> bool {
    ctx.data(|data| {
        data.get_temp::<bool>(egui::Id::new(BACKGROUND_INPUT_BLOCKED))
            .unwrap_or(false)
    })
}

/// Show a centered, resizable dialog with a backdrop that consumes all input
/// outside the dialog. Dismissal policy stays with the caller so editors do
/// not lose drafts on an accidental backdrop click.
pub(crate) fn modal(
    ctx: &egui::Context,
    id: impl std::hash::Hash + std::fmt::Debug,
    title: impl Into<egui::RichText>,
    default_size: egui::Vec2,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> egui::ModalResponse<()> {
    modal_impl(ctx, id, title, default_size, true, add_contents)
}

/// Show a centered, resizable dialog without the built-in title row.
/// Callers can provide their own title bar when they need custom actions.
pub(crate) fn modal_without_title(
    ctx: &egui::Context,
    id: impl std::hash::Hash + std::fmt::Debug,
    default_size: egui::Vec2,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> egui::ModalResponse<()> {
    modal_impl(ctx, id, "", default_size, false, add_contents)
}

fn modal_impl(
    ctx: &egui::Context,
    id: impl std::hash::Hash + std::fmt::Debug,
    title: impl Into<egui::RichText>,
    default_size: egui::Vec2,
    show_title: bool,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> egui::ModalResponse<()> {
    let viewport = ctx.content_rect().size();
    egui::Modal::new(egui::Id::new(id)).show(ctx, |ui| {
        egui::Resize::default()
            .default_size(default_size)
            .min_size(egui::vec2(280.0, 120.0))
            .max_size(egui::vec2(
                (viewport.x - 32.0).max(280.0),
                (viewport.y - 32.0).max(120.0),
            ))
            .show(ui, |ui| {
                if show_title {
                    ui.heading(title);
                    ui.add_space(4.0);
                }
                add_contents(ui);
            });
    })
}

/// Show an anchored transient surface over a transparent input-consuming
/// backdrop. Clicking the backdrop closes the popover without activating the
/// control or log row beneath it.
pub(crate) fn popover(
    ctx: &egui::Context,
    id: impl std::hash::Hash + std::fmt::Debug,
    position: egui::Pos2,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> egui::ModalResponse<()> {
    let id = egui::Id::new(id);
    let screen = ctx.content_rect();
    let area = egui::Modal::default_area(id)
        .anchor(egui::Align2::LEFT_TOP, egui::Vec2::ZERO)
        .fixed_pos(screen.min);
    egui::Modal::new(id)
        .area(area)
        .backdrop_color(egui::Color32::TRANSPARENT)
        .frame(egui::Frame::NONE)
        .show(ctx, |ui| {
            let mut popup_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(egui::Rect::from_min_size(position, egui::Vec2::ZERO)),
            );
            egui::Frame::popup(ui.style()).show(&mut popup_ui, add_contents);
        })
}

/// Register a transparent modal layer behind a custom foreground window.
/// This is used by the movable/resizable analysis bubble, whose geometry is
/// intentionally richer than an anchored menu.
pub(crate) fn input_shield(
    ctx: &egui::Context,
    id: impl std::hash::Hash + std::fmt::Debug,
) -> egui::ModalResponse<()> {
    egui::Modal::new(egui::Id::new(id))
        .backdrop_color(egui::Color32::TRANSPARENT)
        .frame(egui::Frame::NONE)
        .show(ctx, |_| {})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_input_marker_round_trips() {
        let ctx = egui::Context::default();
        assert!(!background_input_blocked(&ctx));
        mark_background_input(&ctx, true);
        assert!(background_input_blocked(&ctx));
        mark_background_input(&ctx, false);
        assert!(!background_input_blocked(&ctx));
    }

    #[test]
    fn popover_backdrop_covers_the_viewport_and_consumes_pointer_input() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 400.0));
        let draw = |ctx: &egui::Context| {
            popover(ctx, "test_popover", egui::pos2(200.0, 100.0), |ui| {
                ui.set_min_size(egui::vec2(160.0, 80.0));
                ui.label("popover");
            })
        };

        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ui| {
                draw(ui.ctx());
            },
        );

        let mut backdrop_rect = egui::Rect::NOTHING;
        let mut backdrop_sense = egui::Sense::hover();
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen),
                events: vec![egui::Event::PointerMoved(egui::pos2(20.0, 20.0))],
                ..Default::default()
            },
            |ui| {
                let response = draw(ui.ctx()).backdrop_response;
                backdrop_rect = response.rect;
                backdrop_sense = response.sense;
            },
        );

        assert_eq!(backdrop_rect, screen);
        assert!(backdrop_sense.senses_click());
        assert!(backdrop_sense.senses_drag());
    }
}
