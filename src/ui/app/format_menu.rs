//! Current-file format and saved profile switching from the top bar.

use eframe::egui;
use egui::RichText;
use haystack::core::record::{ProfileSource, RecordProfile};

use super::HaystackApp;
use crate::ui::app::overlay;
use crate::ui::record_format::RecordEditor;

enum Action {
    EditCurrent,
    New,
    Select(RecordProfile),
    AutoDetect,
    CancelPending,
}

fn syntax(profile: &RecordProfile) -> &str {
    match &profile.source {
        ProfileSource::Template { layout } => layout,
        ProfileSource::AdvancedRegex { pattern } => pattern,
        ProfileSource::BuiltIn { adapter } => adapter,
    }
}

pub(super) fn show(app: &mut HaystackApp, ctx: &egui::Context) {
    if !app.show_format_dropdown {
        return;
    }
    let Some(button_rect) = app.format_button_rect else {
        return;
    };
    let (current_profile, current_file, current_sample, auto_description, has_current, match_rate) = {
        let current = app.active.and_then(|index| app.tabs.get(index));
        let profile = current.and_then(|tab| tab.doc.record_profile()).cloned();
        let file = current.map(|tab| tab.doc.file_name.clone());
        let sample = current.map(|tab| {
            let line = tab.context_line.unwrap_or(0);
            let first = tab
                .doc
                .record_range_containing(line)
                .map_or(line, |range| range.start);
            tab.doc.line_untrimmed(first).into_owned()
        });
        let description = current.map(|tab| format!("Auto-detect · {}", tab.doc.format_name()));
        let match_rate = current.and_then(|tab| tab.doc.record_profile_match_rate());
        (
            profile,
            file,
            sample,
            description,
            current.is_some(),
            match_rate,
        )
    };
    let presets = app.record_presets.clone();
    let available = ctx.content_rect();
    let width = 440.0_f32.min(available.width() - 24.0).max(260.0);
    let left = button_rect
        .left()
        .min((available.right() - width - 12.0).max(12.0));
    let mut action = None;
    let popup = overlay::popover(
        ctx,
        "format_dropdown",
        egui::pos2(left, button_rect.bottom()),
        |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 5.0;
            ui.label(RichText::new(current_file.as_deref().unwrap_or("No log open")).size(13.0));
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(current_profile.as_ref().map_or_else(
                        || {
                            auto_description
                                .as_deref()
                                .unwrap_or("Choose or create a format")
                        },
                        |profile| profile.name.as_str(),
                    ))
                    .strong()
                    .size(14.0),
                );
                if has_current && ui.button("Edit").clicked() {
                    action = Some(Action::EditCurrent);
                }
                if ui.button("New").clicked() {
                    action = Some(Action::New);
                }
            });
            if let Some(profile) = &current_profile {
                let applied_syntax = syntax(profile);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(applied_syntax).monospace().size(13.0));
                    if ui.button("Copy").clicked() {
                        ctx.copy_text(applied_syntax.to_owned());
                    }
                });
                if let Some(saved) = presets.iter().find(|saved| saved.id == profile.id) {
                    if saved != profile {
                        ui.label(
                            RichText::new("This file uses a different saved version.").size(13.0),
                        );
                    }
                } else {
                    ui.label(RichText::new("File-only format").size(13.0));
                }
                if let Some((matched, total)) =
                    match_rate.filter(|(matched, total)| *total > 0 && matched * 100 < total * 90)
                {
                    let percent = matched as f64 * 100.0 / total as f64;
                    ui.colored_label(
                            app.theme.warning,
                            RichText::new(format!("⚠ Format matched {matched} of {total} log lines ({percent:.0}%). Check and update Format.")).size(13.0),
                        );
                }
            }
            if app.format_change_pending_for_active() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Applying format…").size(13.0));
                    if ui.button("Cancel").clicked() {
                        action = Some(Action::CancelPending);
                    }
                });
            }
            ui.separator();
            ui.add(
                egui::TextEdit::singleline(&mut app.format_search)
                    .hint_text("Search saved formats…")
                    .font(egui::FontId::proportional(13.0))
                    .desired_width(f32::INFINITY),
            );
            if let Some(error) = &app.record_presets_error {
                ui.colored_label(
                    app.theme.warning,
                    RichText::new(format!("Could not load saved formats: {error}")).size(13.0),
                );
            } else if presets.is_empty() {
                ui.label(RichText::new("No saved formats yet").size(13.0));
            } else {
                let needle = app.format_search.trim().to_lowercase();
                let matches = presets
                    .iter()
                    .filter(|profile| {
                        needle.is_empty()
                            || profile.name.to_lowercase().contains(&needle)
                            || syntax(profile).to_lowercase().contains(&needle)
                    })
                    .collect::<Vec<_>>();
                if matches.is_empty() {
                    ui.label(RichText::new("No formats match this search").size(13.0));
                    if ui.button("Clear search").clicked() {
                        app.format_search.clear();
                    }
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(260.0)
                        .show(ui, |ui| {
                            for profile in matches {
                                let applied = current_profile
                                    .as_ref()
                                    .is_some_and(|current| current == profile);
                                let newer = current_profile.as_ref().is_some_and(|current| {
                                    current.id == profile.id && current != profile
                                });
                                let label = format!(
                                    "{}{}{}",
                                    if applied { "✓ " } else { "  " },
                                    profile.name,
                                    if newer { " · Newer version" } else { "" }
                                );
                                if ui
                                    .selectable_label(applied, RichText::new(label).size(13.0))
                                    .on_hover_text(syntax(profile))
                                    .clicked()
                                {
                                    action = Some(Action::Select(profile.clone()));
                                }
                            }
                        });
                }
            }
            if has_current {
                ui.separator();
                if ui
                    .selectable_label(
                        current_profile.is_none(),
                        RichText::new("Auto-detect").size(13.0),
                    )
                    .clicked()
                {
                    action = Some(Action::AutoDetect);
                }
            }
            if let Some(error) = &app.format_error {
                ui.colored_label(app.theme.warning, RichText::new(error).size(13.0));
            }
        },
    );
    if let Some(action) = action {
        app.format_error = None;
        match action {
            Action::EditCurrent => {
                app.record_editor = RecordEditor::default();
                if let Some(profile) = current_profile {
                    app.record_editor.select_profile(profile);
                }
                if let Some(sample) = current_sample {
                    app.record_editor.set_sample(sample);
                }
                app.record_editor.bind_apply_target(
                    app.active
                        .and_then(|index| app.tabs.get(index))
                        .map(|tab| tab.doc.path.clone()),
                );
                app.record_editor.open = true;
                app.show_format_dropdown = false;
            }
            Action::New => {
                app.record_editor = RecordEditor::default();
                if let Some(sample) = current_sample {
                    app.record_editor.set_sample(sample);
                }
                app.record_editor.bind_apply_target(
                    app.active
                        .and_then(|index| app.tabs.get(index))
                        .map(|tab| tab.doc.path.clone()),
                );
                app.record_editor.open = true;
                app.show_format_dropdown = false;
            }
            Action::Select(profile) => {
                if app.active.is_none() {
                    app.record_editor = RecordEditor::default();
                    app.record_editor.select_profile(profile);
                    app.record_editor.open = true;
                    app.show_format_dropdown = false;
                } else if current_profile.as_ref() == Some(&profile) {
                    app.show_format_dropdown = false;
                } else {
                    match app.apply_record_profile(profile) {
                        Ok(()) => app.show_format_dropdown = false,
                        Err(error) => app.format_error = Some(error),
                    }
                }
            }
            Action::AutoDetect => match app.apply_auto_record_profile() {
                Ok(()) => app.show_format_dropdown = false,
                Err(error) => app.format_error = Some(error),
            },
            Action::CancelPending => app.cancel_record_reparse(),
        }
    }
    if popup.should_close() {
        app.show_format_dropdown = false;
    }
}
