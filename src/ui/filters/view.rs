//! Filter bookmark strip: add/remove filters, see live match counts.
//! Matches land on the timeline as colored markers once the background
//! scan finishes (a spinner shows while it's chewing).
//!
//! `add_filter_ui` renders a compact one-line input meant to go in the
//! toolbar. `show` renders the filter chip row.

use eframe::egui;

use crate::ui::app::model::{LogTab, MAX_FILTERS};
use crate::ui::icons::{self, Icon};
use crate::ui::theme::Theme;

/// Compact one-line Add Filter section — call this from the toolbar.
/// The input is disabled once the filter cap (MAX_FILTERS) is reached.
pub fn add_filter_ui(ui: &mut egui::Ui, tab: &mut LogTab, theme: &Theme) {
    let at_cap = tab.filters.len() >= MAX_FILTERS;
    ui.horizontal_top(|ui| {
        let resp = ui
            .add_enabled_ui(!at_cap, |ui| {
                ui.add_sized(
                    egui::vec2(225.0, 0.0),
                    egui::TextEdit::singleline(&mut tab.filter_input)
                        .hint_text(if at_cap {
                            "Max 20 filters"
                        } else {
                            "filter + Enter"
                        })
                        .desired_width(225.0),
                )
            })
            .inner;
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let add_clicked = ui
            .add_enabled(
                !at_cap,
                egui::Button::image_and_text(
                    icons::icon_image(ui.ctx(), Icon::Enter, 13.0, theme.text),
                    "Add",
                ),
            )
            .clicked();
        if (enter || add_clicked) && !tab.filter_input.trim().is_empty() {
            let text = tab.filter_input.trim().to_string();
            let color = theme.filter_colors[tab.filters.len() % theme.filter_colors.len()];
            tab.push_filter(&text, color);
            tab.filter_input.clear();
            resp.request_focus();
        }
        if tab.search_rx.is_some() || tab.visible_rx.is_some() {
            ui.spinner();
        }
    });
}
