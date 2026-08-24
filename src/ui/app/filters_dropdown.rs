use eframe::egui;
use egui::RichText;

use crate::ui::app::model::LogotomyApp;

impl LogotomyApp {
    pub(super) fn show_filters_dropdown(&mut self, ui: &mut egui::Ui) {
        if let Some(button_rect) = self.filter_button_rect {
            let popup_id = egui::Id::new("filters_popup");
            let area = egui::Area::new(popup_id)
                .current_pos(button_rect.left_bottom())
                .order(egui::Order::Foreground)
                .fixed_pos(button_rect.left_bottom());
            let area_resp = area.show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(260.0);
                    ui.set_max_width(360.0);
                    ui.label(RichText::new("Saved filters").strong().size(14.0));
                    ui.separator();

                    let save_name = self
                        .active
                        .and_then(|i| self.tabs.get(i).and_then(|t| t.applied_filter.clone()));
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            if let Some(ref name) = save_name {
                                self.save_filter(name);
                            } else {
                                self.show_new_filter_popup = true;
                            }
                            self.show_filter_dropdown = false;
                        }
                        if ui.button("Save As...").clicked() {
                            self.show_new_filter_popup = true;
                            self.show_filter_dropdown = false;
                        }
                    });
                    ui.separator();

                    egui::ScrollArea::vertical()
                        .max_height(280.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if self.available_filters.is_empty() {
                                ui.label(
                                    RichText::new("No saved filters yet.")
                                        .small()
                                        .color(self.theme.text_muted),
                                );
                            }
                            for filter_name in &self.available_filters.clone() {
                                let is_applied = self.active.map_or(false, |i| {
                                    self.tabs[i].applied_filter.as_deref() == Some(filter_name)
                                });
                                // Keep the row identity tied to the saved filter name. Applying
                                // a filter mutates the main UI while this popup is being drawn;
                                // relying on auto IDs then makes egui see a different widget at
                                // the same row rectangle on the next pass.
                                ui.push_id(super::saved_filter_row_id(filter_name), |ui| {
                                    ui.horizontal(|ui| {
                                        if ui.selectable_label(is_applied, filter_name).clicked() {
                                            self.apply_filter(filter_name);
                                            self.show_filter_dropdown = false;
                                        }
                                        if ui
                                            .button("Edit")
                                            .on_hover_text("Rename filter")
                                            .clicked()
                                        {
                                            self.rename_filter_target = filter_name.clone();
                                            self.rename_filter_new_name = filter_name.clone();
                                            self.show_rename_filter_popup = true;
                                            self.show_filter_dropdown = false;
                                        }
                                        let is_default = self.settings.default_filter.as_deref()
                                            == Some(filter_name);
                                        let star_icon = if is_default { "★" } else { "☆" };
                                        if ui
                                            .button(star_icon)
                                            .on_hover_text("Set as default filter")
                                            .clicked()
                                        {
                                            if is_default {
                                                self.settings.default_filter = None;
                                            } else {
                                                self.settings.default_filter =
                                                    Some(filter_name.clone());
                                            }
                                            self.settings.save();
                                        }
                                    });
                                });
                            }
                        });
                });
            });
            // Close on click outside
            if ui.input(|i| i.pointer.any_click()) {
                if let Some(click_pos) = ui.input(|i| i.pointer.interact_pos()) {
                    let on_button = button_rect.contains(click_pos);
                    let on_popup = area_resp.response.rect.contains(click_pos);
                    if !on_button && !on_popup {
                        self.show_filter_dropdown = false;
                    }
                }
            }
        }
    }
}
