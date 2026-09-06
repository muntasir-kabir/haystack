//! Full-width, text-first rows for lightweight suggestion popups.

use eframe::egui;

/// A clickable dropdown row whose text is always left-aligned. Unlike a
/// button, the row has no inactive chrome; hover is the only emphasis.
pub fn show(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let size = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(3),
            ui.visuals().widgets.hovered.weak_bg_fill,
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().text(
        rect.left_center() + egui::vec2(6.0, 0.0),
        egui::Align2::LEFT_CENTER,
        text,
        egui::TextStyle::Button.resolve(ui.style()),
        ui.visuals().text_color(),
    );
    response
}
