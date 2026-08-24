//! Modal window for creating / managing user-defined custom date recognizers.
//!
//! Users paste a regex (with named groups `year month day hour min sec` and
//! optional `ms`, `ampm`), a sample log line to verify against, and a name.
//! The window live-verifies the regex against the sample and shows the parsed
//! components. On "Add", the format is saved to
//! `~/.logotomy/custom_date_format_list.json` and is used (alongside the
//! built-in families) when files are opened.

use eframe::egui;
use egui::{Color32, RichText};

use logotomy::core::settings::Settings;
use logotomy::core::time::CustomDateFormat;

use crate::ui::app::model::LogotomyApp;

/// Success green, tuned to read well on both light and dark surfaces.
fn success_color(bg: Color32) -> Color32 {
    if bg.r() > 128 {
        Color32::from_rgb(0, 140, 60)
    } else {
        Color32::from_rgb(110, 220, 120)
    }
}

/// Show the "Custom Date Recognizers" modal (drawn at the app level).
pub fn show_custom_date_popup(app: &mut LogotomyApp, ctx: &egui::Context) {
    if !app.show_custom_date_popup {
        return;
    }
    // Copy the palette colors up front so the closure can borrow `app` mutably.
    let (text_muted, warning, placeholder, bg) = (
        app.theme.text_muted,
        app.theme.warning,
        app.theme.placeholder,
        app.theme.bg,
    );

    let mut open = true;
    egui::Window::new("Custom Date Recognizers")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(620.0)
        .default_height(580.0)
        .show(ctx, |ui| {
            ui.label(
                RichText::new(
                    "Custom recognizers are tried together with the built-in date formats \
                     (ISO-8601, BSD syslog, Apache, …) whenever a log file is opened.",
                )
                .size(11.0)
                .color(text_muted),
            );

            // ---- Existing custom formats ----
            ui.add_space(8.0);
            ui.label(RichText::new("Saved custom formats").strong().size(13.0));
            let removed = saved_formats_ui(ui, app, text_muted);
            if let Some(i) = removed {
                app.custom_date_formats.remove(i);
                Settings::save_custom_date_formats(&app.custom_date_formats);
            }

            ui.separator();

            // ---- Add a new recognizer ----
            ui.label(RichText::new("Add a new recognizer").strong().size(13.0));

            let mut name = app.cd_name.clone();
            ui.horizontal(|ui| {
                ui.label("Name:");
                ui.add_sized(
                    [300.0, 22.0],
                    egui::TextEdit::singleline(&mut name).hint_text("e.g. iOS 12h AM/PM"),
                );
            });

            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "Regex (required named groups: year, month, day, hour, min, sec — \
                     optional: ms, ampm):",
                )
                .size(11.0)
                .color(text_muted),
            );
            let mut regex = app.cd_regex.clone();
            ui.add_sized(
                [ui.available_width(), 56.0],
                egui::TextEdit::multiline(&mut regex)
                    .font(egui::TextStyle::Monospace)
                    .hint_text(
                        r"(?P<year>\d{4})-(?P<month>\d{2})-(?P<day>\d{2}) (?P<hour>\d{1,2}):(?P<min>\d{2}):(?P<sec>\d{2})\.(?P<ms>\d{3}) (?P<ampm>[AP]M)",
                    ),
            );

            ui.add_space(4.0);
            ui.label(
                RichText::new("Sample log line (to verify the regex):")
                    .size(11.0)
                    .color(text_muted),
            );
            let mut sample = app.cd_sample.clone();
            ui.add_sized(
                [ui.available_width(), 34.0],
                egui::TextEdit::multiline(&mut sample)
                    .font(egui::TextStyle::Monospace)
                    .hint_text("2026-08-14 4:08:23.668 PM [com.apple.main-thread:18836] ..."),
            );

            app.cd_name = name;
            app.cd_regex = regex;
            app.cd_sample = sample;

            let def = CustomDateFormat {
                name: app.cd_name.trim().to_string(),
                regex: app.cd_regex.clone(),
            };

            // ---- Live verification ----
            ui.add_space(8.0);
            ui.label(
                RichText::new("Recognized date components (verification):")
                    .size(11.0)
                    .color(text_muted),
            );
            match def.preview(&app.cd_sample) {
                Ok(Some(comps)) => {
                    ui.label(RichText::new(comps.describe()).monospace().color(success_color(bg)));
                }
                Ok(None) => {
                    ui.label(RichText::new("Regex did not match the sample line.").monospace().color(warning));
                }
                Err(e) => {
                    ui.label(RichText::new(e).monospace().color(warning));
                }
            }
            ui.label(
                RichText::new(
                    "Required named groups: year, month, day, hour, min, sec. Optional: ms, ampm.",
                )
                .size(10.0)
                .color(placeholder),
            );

            ui.add_space(8.0);
            let can_add = !app.cd_name.trim().is_empty() && def.validate(&app.cd_sample).is_ok();
            if ui
                .add_enabled(can_add, egui::Button::new("Add custom date format"))
                .clicked()
            {
                app.custom_date_formats.push(def);
                Settings::save_custom_date_formats(&app.custom_date_formats);
                app.cd_name.clear();
                app.cd_regex.clear();
                app.cd_sample.clear();
            }

            ui.add_space(8.0);
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Apply now to the open file:")
                        .size(11.0)
                        .color(text_muted),
                );
                let has_active = app.active.is_some();
                if ui
                    .add_enabled(
                        has_active,
                        egui::Button::new("Re-scan active log"),
                    )
                    .on_hover_text(
                        "Re-opens the current file so the built-in + custom recognizers are \
                         re-run. Per-tab state (filters, pins, scroll) is reset.",
                    )
                    .clicked()
                {
                    app.reopen_active_with_custom();
                }
            });
        });
    if !open {
        app.show_custom_date_popup = false;
    }
}
/// Render the saved-formats list; returns the index to remove when its ✕ is clicked.
fn saved_formats_ui(
    ui: &mut egui::Ui,
    app: &mut LogotomyApp,
    text_muted: Color32,
) -> Option<usize> {
    if app.custom_date_formats.is_empty() {
        ui.label(
            RichText::new("None yet — add one below and it will be used on the next file open.")
                .italics()
                .color(text_muted),
        );
        return None;
    }
    let mut remove: Option<usize> = None;
    egui::ScrollArea::vertical()
        .max_height(150.0)
        .show(ui, |ui| {
            for (i, def) in app.custom_date_formats.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&def.name).strong().size(12.0));
                    if ui
                        .button("✕")
                        .on_hover_text("Delete this custom format")
                        .clicked()
                    {
                        remove = Some(i);
                    }
                });
                ui.label(
                    RichText::new(&def.regex)
                        .monospace()
                        .size(10.0)
                        .color(text_muted),
                );
                ui.separator();
            }
        });
    remove
}
