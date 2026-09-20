//! Log-format editor. Preview work is debounced and runs off the paint path.

mod completion;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{Datelike, Local};
use crossbeam_channel::Receiver;
use eframe::egui;
use egui::RichText;
use haystack::core::record::{
    preview_profile, save_presets, CompiledProfile, CustomFieldDefinition, CustomFieldKind,
    PreviewResult, ProfileSource, RecordProfile, TimestampSelection, PREVIEW_MAX_BYTES,
    RECORD_PROFILE_SCHEMA_VERSION,
};
use haystack::core::settings::Settings;
use haystack::core::time::{CustomDateFormat, CustomTimeFormat, TIME_FORMATS};

use crate::ui::app::model::HaystackApp;

struct PreviewUpdate {
    revision: u64,
    format_valid: Result<(), String>,
    preview: Option<Result<PreviewResult, String>>,
}

pub struct RecordEditor {
    pub open: bool,
    profile: RecordProfile,
    sample: String,
    changed_at: Option<Instant>,
    preview_rx: Option<Receiver<PreviewUpdate>>,
    preview_cancel: Option<Arc<AtomicBool>>,
    preview: Option<Result<PreviewResult, String>>,
    preview_revision: u64,
    preview_result_revision: Option<u64>,
    format_valid: Option<Result<(), String>>,
    message: Option<String>,
    completion_open: bool,
    completion_visible: bool,
    completion_selected: usize,
    apply_target: Option<PathBuf>,
}

impl Default for RecordEditor {
    fn default() -> Self {
        Self {
            open: false,
            profile: RecordProfile::refined_inline(
                format!("user:{:016x}", rand::random::<u64>()),
                "",
                "{time} {log}",
            ),
            sample: "2026-07-15 22:26:39.907481+0300 MyApp[12345:9] <FAULT> CameraService.swift:295 EXC_CRASH (SIGKILL) — jetsam killed process after memory limit exceeded (11743MB)\n".into(),
            changed_at: Some(Instant::now()),
            preview_rx: None,
            preview_cancel: None,
            preview: None,
            preview_revision: 0,
            preview_result_revision: None,
            format_valid: None,
            message: None,
            completion_open: false,
            completion_visible: false,
            completion_selected: 0,
            apply_target: None,
        }
    }
}

impl RecordEditor {
    fn changed(&mut self) {
        if let Some(cancel) = self.preview_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.preview_rx = None;
        self.preview_revision = self.preview_revision.wrapping_add(1);
        self.format_valid = None;
        self.changed_at = Some(Instant::now());
    }

    fn preview_is_stale(&self) -> bool {
        self.changed_at.is_some()
            || self.preview_rx.is_some()
            || self.preview_result_revision != Some(self.preview_revision)
    }

    pub fn dismiss_completion(&mut self) -> bool {
        std::mem::take(&mut self.completion_open)
    }

    pub(crate) fn select_profile(&mut self, profile: RecordProfile) {
        self.profile = profile;
        self.message = None;
        self.changed();
    }

    pub(crate) fn set_sample(&mut self, sample: String) {
        self.sample = sample;
        self.changed();
    }

    /// Apply remains bound to the file that opened this editor. Changing tabs
    /// must never redirect an in-progress draft to a different investigation.
    pub(crate) fn bind_apply_target(&mut self, path: Option<PathBuf>) {
        self.apply_target = path;
    }

    fn apply_target_is_active(&self, app: &HaystackApp) -> bool {
        self.apply_target.as_ref().is_some_and(|path| {
            app.active
                .and_then(|index| app.tabs.get(index))
                .is_some_and(|tab| &tab.doc.path == path)
        })
    }

    fn legacy_conversion(&self) -> Result<RecordProfile, String> {
        if self.profile.schema_version != RECORD_PROFILE_SCHEMA_VERSION {
            return Err("This format does not use the current log-format grammar.".into());
        }
        if !self.profile.custom_fields.is_empty()
            || !matches!(self.profile.source, ProfileSource::Template { .. })
        {
            return Err("Advanced regex and custom-field formats stay legacy because a safe schema-3 conversion is not available.".into());
        }
        if matches!(&self.profile.source, ProfileSource::Template { layout } if layout.contains("{ignore}"))
        {
            return Err("This legacy format captures a field named ignore. Keep it legacy so conversion cannot discard that field.".into());
        }
        let mut converted = self.profile.clone();
        converted.id = format!("user:{:016x}", rand::random::<u64>());
        converted.name = format!("{} (schema 3)", converted.name);
        converted.revision = 0;
        converted.schema_version = RECORD_PROFILE_SCHEMA_VERSION;
        Ok(converted)
    }

    fn poll_preview(&mut self, custom_definitions: &[CustomDateFormat]) {
        if let Some(rx) = self.preview_rx.as_ref() {
            match rx.try_recv() {
                Ok(update) if update.revision == self.preview_revision => {
                    self.format_valid = Some(update.format_valid);
                    if let Some(result) = update.preview {
                        self.preview = Some(result);
                        self.preview_result_revision = Some(update.revision);
                    }
                    self.preview_rx = None;
                    self.preview_cancel = None;
                }
                Ok(_) => {
                    self.preview_rx = None;
                    self.preview_cancel = None;
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.format_valid = Some(Err("preview worker stopped".into()));
                    self.preview_rx = None;
                    self.preview_cancel = None;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {}
            }
        }
        if self.preview_rx.is_some()
            || !self
                .changed_at
                .is_some_and(|when| when.elapsed() >= Duration::from_millis(200))
        {
            return;
        }
        self.changed_at = None;
        let revision = self.preview_revision;
        let mut draft = self.profile.clone();
        if draft.name.trim().is_empty() {
            draft.name = "Preview".into();
        }
        let mut prefix_end = self.sample.len().min(PREVIEW_MAX_BYTES + 1);
        while !self.sample.is_char_boundary(prefix_end) {
            prefix_end -= 1;
        }
        let sample = self.sample[..prefix_end].to_string();
        let custom_definitions = custom_definitions.to_vec();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (tx, rx) = crossbeam_channel::bounded(1);
        crate::ui::worker_pool::spawn(move || {
            let update = match CompiledProfile::compile(draft) {
                Ok(compiled) => {
                    let custom: Vec<CustomTimeFormat> = custom_definitions
                        .iter()
                        .filter_map(|definition| definition.compile().ok())
                        .collect();
                    PreviewUpdate {
                        revision,
                        format_valid: Ok(()),
                        preview: Some(preview_profile(&compiled, &sample, &custom, &worker_cancel)),
                    }
                }
                Err(error) => PreviewUpdate {
                    revision,
                    format_valid: Err(error.to_string()),
                    preview: None,
                },
            };
            let _ = tx.send(update);
        });
        self.preview_rx = Some(rx);
        self.preview_cancel = Some(cancel);
    }
}

enum Action {
    Save,
    Apply,
    SaveApply,
}

#[derive(Clone, Copy)]
enum CompletionKey {
    Previous,
    Next,
    Accept,
}

pub fn show_record_format_popup(app: &mut HaystackApp, ctx: &egui::Context) {
    if !app.record_editor.open {
        return;
    }
    let mut editor = std::mem::take(&mut app.record_editor);
    editor.poll_preview(&app.custom_date_formats);
    let mut changed = false;
    let mut action = None;
    let mut valid = false;
    let title = if editor.profile.name.is_empty() {
        "New log format"
    } else {
        "Edit log format"
    };
    let viewport = ctx.content_rect().size();
    egui::Modal::new(egui::Id::new("record_format_modal")).show(ctx, |ui| {
        egui::Resize::default()
            .id_salt("record_format_resize")
            .default_size(egui::vec2(900.0, 660.0))
            .min_size(egui::vec2(520.0, 360.0))
            .max_size(egui::vec2(
                (viewport.x - 32.0).max(320.0),
                (viewport.y - 32.0).max(240.0),
            ))
            .show(ui, |ui| {
            ui.heading(title);
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.label(
                RichText::new("Describe a log header. Check the extracted values before applying.")
                    .size(13.0),
            );
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt(("record_format_content", &editor.profile.id))
                .max_height((ctx.content_rect().height() - 150.0).max(200.0))
                .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_width(72.0);
                ui.label(
                    RichText::new(if editor.profile.name.trim().is_empty() { "Name *" } else { "Name" })
                        .strong()
                        .size(14.0),
                );
                let name = ui.add(egui::TextEdit::singleline(&mut editor.profile.name)
                    .hint_text("e.g. MyApp console")
                    .desired_width(f32::INFINITY)
                    .font(egui::FontId::proportional(13.0)));
                if name.changed() {
                    editor.message = None;
                }
            });
            if editor.profile.name.trim().is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(72.0);
                    ui.colored_label(app.theme.warning, RichText::new("Name is required before saving.").size(12.0));
                });
            }
            ui.horizontal(|ui| {
                ui.add_space(72.0);
                egui::CollapsingHeader::new(RichText::new("Template rules & syntax").size(13.0))
                    .id_salt(("record_template_rules", &editor.profile.id))
                    .default_open(false)
                    .show(ui, |ui| render_template_rules(ui));
            });
            ui.add_space(4.0);
            ui.horizontal_top(|ui| {
                ui.set_min_width(72.0);
                ui.label(RichText::new("Template").strong().size(14.0));
            if let ProfileSource::Template { layout } = &mut editor.profile.source {
                let completion_key = if editor.completion_visible {
                    ui.input_mut(|input| {
                        if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                            Some(CompletionKey::Next)
                        } else if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                            Some(CompletionKey::Previous)
                        } else if input.consume_key(egui::Modifiers::NONE, egui::Key::Tab)
                            || input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                        {
                            Some(CompletionKey::Accept)
                        } else {
                            None
                        }
                    })
                } else {
                    None
                };
                let mut template_layouter = |ui: &egui::Ui,
                                             buffer: &dyn egui::TextBuffer,
                                             wrap_width: f32| {
                    let mut job = template_layout_job(buffer.as_str(), &app.theme);
                    job.wrap.max_width = wrap_width;
                    ui.fonts_mut(|fonts| fonts.layout_job(job))
                };
                let output = egui::TextEdit::multiline(layout)
                    .desired_rows(1).desired_width(f32::INFINITY)
                    .font(egui::FontId::monospace(13.0))
                    .return_key(None)
                    .lock_focus(false)
                    .layouter(&mut template_layouter)
                    .show(ui);
                changed |= output.response.changed();
                if output.response.has_focus()
                    && ui.input_mut(|input| {
                        input.consume_key(egui::Modifiers::COMMAND, egui::Key::Space)
                    })
                {
                    editor.completion_open = true;
                    editor.completion_selected = 0;
                }
                if output.response.changed() {
                    editor.completion_open = true;
                    editor.completion_selected = 0;
                }
                if output.response.has_focus() && editor.completion_open {
                    if let Some(cursor) = output.cursor_range.map(|range| range.primary.index.0) {
                        if let Some(completion) = completion::at_cursor(layout, cursor) {
                            // `{ignore}` is schema-3 syntax. Do not offer a completion that
                            // the legacy profile compiler would reject.
                            let mut selected = None;
                            if !completion.choices.is_empty() {
                                editor.completion_visible = true;
                                match completion_key {
                                    Some(CompletionKey::Next) => {
                                        editor.completion_selected =
                                            (editor.completion_selected + 1) % completion.choices.len();
                                    }
                                    Some(CompletionKey::Previous) => {
                                        editor.completion_selected = (editor.completion_selected
                                            + completion.choices.len() - 1)
                                            % completion.choices.len();
                                    }
                                    Some(CompletionKey::Accept) => {
                                        selected = Some(
                                            editor
                                                .completion_selected
                                                .min(completion.choices.len() - 1),
                                        );
                                    }
                                    None => {}
                                }
                                let cursor = output.cursor_range.unwrap().primary;
                                let caret = output.galley_pos
                                    + output.galley.pos_from_cursor(cursor).min.to_vec2();
                                let viewport = ctx.content_rect();
                                let popup_pos = egui::pos2(
                                    caret.x.min(viewport.right() - 240.0).max(viewport.left() + 8.0),
                                    (caret.y + 20.0).min(viewport.bottom() - 180.0).max(viewport.top() + 8.0),
                                );
                                let clicked = egui::Area::new(egui::Id::new((
                                    "record_template_completion",
                                    &editor.profile.id,
                                )))
                                .order(egui::Order::Foreground)
                                .fixed_pos(popup_pos)
                                .show(ctx, |ui| {
                                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                                        ui.set_width(232.0);
                                        for (index, choice) in completion.choices.iter().take(6).enumerate() {
                                            if ui
                                                .selectable_label(
                                                    index == editor.completion_selected,
                                                    RichText::new(*choice).monospace().size(13.0),
                                                )
                                                .clicked()
                                            {
                                                selected = Some(index);
                                            }
                                        }
                                        ui.label(RichText::new("Enter or Tab to insert · Esc to close").size(12.0));
                                    });
                                });
                                let clicked_elsewhere = ctx.input(|input| {
                                    input.pointer.any_click()
                                        && input.pointer.interact_pos().is_some_and(|point| {
                                            !output.response.rect.contains(point)
                                                && !clicked.response.rect.contains(point)
                                        })
                                });
                                if clicked_elsewhere {
                                    editor.completion_open = false;
                                    editor.completion_visible = false;
                                }
                                if let Some(index) = selected {
                                    let new_cursor = completion::insert(layout, &completion, completion.choices[index]);
                                    let mut state = output.state;
                                    state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(new_cursor))));
                                    state.store(ui.ctx(), output.response.id);
                                    output.response.request_focus();
                                    editor.completion_open = false;
                                    editor.completion_visible = false;
                                    changed = true;
                                }
                            } else {
                                editor.completion_visible = false;
                            }
                        } else {
                            editor.completion_open = false;
                            editor.completion_visible = false;
                        }
                    }
                } else {
                    editor.completion_visible = false;
                }
            }
            });
            if let ProfileSource::Template { layout } = &editor.profile.source {
                match editor.format_valid.as_ref() {
                    Some(Ok(_)) => {
                        valid = !editor.profile.name.trim().is_empty();
                        ui.label(RichText::new("Template valid").size(13.0))
                    }
                    Some(Err(error)) => ui.colored_label(app.theme.warning, RichText::new(error).size(13.0)),
                    None => ui.label(RichText::new("Checking template…").size(13.0)),
                };
                if layout.is_empty() {
                    ui.label(RichText::new("Add {time} and {log}; keep {log} last.").size(13.0));
                }
            }
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Sample log").strong().size(14.0));
                if ui.add_enabled(app.active.is_some(), egui::Button::new("Use current log")).clicked() {
                    if let Some(tab) = app.active.and_then(|index| app.tabs.get(index)) {
                        let line = tab.context_line.unwrap_or(0);
                        let first = tab.doc.record_range_containing(line).map_or(line, |range| range.start);
                        editor.sample = tab.doc.line_untrimmed(first).into_owned();
                        editor.changed();
                    }
                }
                ui.menu_button("Examples", |ui| {
                    for example in EXAMPLES {
                        if ui.button(example.label).clicked() {
                            apply_example(&mut editor, example);
                            ui.close();
                        }
                    }
                });
            });
            changed |= ui.add(egui::TextEdit::multiline(&mut editor.sample)
                .desired_rows(3).desired_width(f32::INFINITY)
                .font(egui::FontId::monospace(13.0))).changed();
            ui.add_space(4.0);
            ui.label(RichText::new("Parsing preview").strong().size(14.0));
            if changed { editor.changed(); }
            if editor.preview_is_stale() {
                ui.label(RichText::new("Updating sample…").size(13.0));
                ctx.request_repaint_after(Duration::from_millis(100));
            }
            match editor.preview.as_ref() {
                Some(Ok(preview)) => render_preview(ui, preview, app.theme.warning),
                Some(Err(error)) => { ui.colored_label(app.theme.warning, RichText::new(error).size(13.0)); }
                None => {}
            }
            if let Some(message) = &editor.message {
                ui.colored_label(app.theme.warning, RichText::new(message).size(13.0));
            }
            ui.add_space(6.0);
            ui.label(RichText::new("Advanced").strong().size(14.0));
            egui::CollapsingHeader::new("Advanced options")
                .default_open(false)
                .show(ui, |ui| {
            if false {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Legacy schema 2 format").size(13.0));
                    if ui.button("Create schema-3 copy").clicked() {
                        match editor.legacy_conversion() {
                            Ok(converted) => {
                                editor.select_profile(converted);
                                editor.message = Some("Created a schema-3 draft. The original format is unchanged until you save this copy.".into());
                            }
                            Err(error) => editor.message = Some(error),
                        }
                    }
                });
                ui.small("Conversion is explicit. Formats with advanced rules, custom fields, or a captured legacy {ignore} stay unchanged.");
                ui.separator();
            }
            ui.horizontal(|ui| {
                ui.label("Preset");
                egui::ComboBox::from_id_salt("record_profile_preset")
                    .selected_text(&editor.profile.name)
                    .show_ui(ui, |ui| {
                        for preset in &app.record_presets {
                            if ui.selectable_label(editor.profile.id == preset.id, &preset.name).clicked() {
                                editor.select_profile(preset.clone());
                            }
                        }
                        for legacy in &app.custom_date_formats {
                            let mut profile = RecordProfile::text(
                                format!("legacy-date:{}", legacy.name),
                                format!("{} (legacy date; timestamp-first)", legacy.name),
                                "{time} {log}",
                            );
                            profile.timestamp = TimestampSelection::Custom(legacy.name.clone());
                            if ui.selectable_label(editor.profile.id == profile.id, &profile.name).clicked() {
                                editor.select_profile(profile);
                                editor.message = Some("Legacy date recognizers are wrapped in a timestamp-first header. If the old regex relied on a prefix, add that prefix to this layout and verify the preview before applying.".into());
                            }
                        }
                    });
                if ui.button("New").clicked() {
                    editor.select_profile(RecordEditor::default().profile);
                }
            });
            if let Some(error) = &app.record_presets_error {
                ui.colored_label(app.theme.warning, format!("Presets could not be loaded: {error}"));
            }
            if false {
            ui.horizontal(|ui| {
                ui.label("Header rule");
                let mut advanced = matches!(editor.profile.source, ProfileSource::AdvancedRegex { .. });
                egui::ComboBox::from_id_salt("record_header_rule")
                    .selected_text(if advanced { "Advanced regex" } else { "Log format template" })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut advanced, false, "Log format template");
                        ui.selectable_value(&mut advanced, true, "Advanced regex");
                    });
                if advanced != matches!(editor.profile.source, ProfileSource::AdvancedRegex { .. }) {
                    editor.profile.source = if advanced {
                        ProfileSource::AdvancedRegex { pattern: r"\A(?P<timestamp>\d{4}-\d{2}-\d{2} [^ ]+)[ \t]".into() }
                    } else {
                        ProfileSource::Template { layout: "{time} {log}".into() }
                    };
                    changed = true;
                }
            });
            match &mut editor.profile.source {
                ProfileSource::Template { .. } => {}
                ProfileSource::AdvancedRegex { pattern } => {
                    changed |= ui.add(egui::TextEdit::multiline(pattern).desired_rows(2).desired_width(f32::INFINITY)).changed();
                    ui.small("Anchor at byte zero with \\A and use a named timestamp capture. Work is capped at 4 KiB per header.");
                }
                ProfileSource::BuiltIn { .. } => {}
            }
            ui.horizontal(|ui| {
                ui.label("Timestamp format");
                egui::ComboBox::from_id_salt("record_timestamp_format")
                    .selected_text(timestamp_label(&editor.profile.timestamp))
                    .show_ui(ui, |ui| {
                        changed |= ui.selectable_value(&mut editor.profile.timestamp, TimestampSelection::Auto, "Auto").changed();
                        changed |= ui.selectable_value(&mut editor.profile.timestamp, TimestampSelection::None, "None (timeless)").changed();
                        for format in TIME_FORMATS {
                            let name = format.name().to_string();
                            changed |= ui.selectable_value(&mut editor.profile.timestamp, TimestampSelection::BuiltIn(name.clone()), name).changed();
                        }
                        for definition in &app.custom_date_formats {
                            changed |= ui.selectable_value(&mut editor.profile.timestamp, TimestampSelection::Custom(definition.name.clone()), format!("Custom: {}", definition.name)).changed();
                        }
                    });
            });
            ui.horizontal(|ui| {
                let mut override_year = editor.profile.yearless_year.is_some();
                if ui.checkbox(&mut override_year, "Override year for yearless dates").changed() {
                    editor.profile.yearless_year = override_year.then(|| Local::now().year());
                    changed = true;
                }
                if let Some(year) = editor.profile.yearless_year.as_mut() {
                    changed |= ui.add(egui::DragValue::new(year).range(1..=9999)).changed();
                }
            });
            ui.collapsing("Custom fields and prefix", |ui| {
                changed |= ui.text_edit_singleline(&mut editor.profile.prefix.fixed_indentation).changed();
                ui.small("Fixed indentation required before each header (leave blank for none).");
                let mut remove = None;
                for (index, field) in editor.profile.custom_fields.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        changed |= ui.text_edit_singleline(&mut field.name).changed();
                        egui::ComboBox::from_id_salt(("record_field_kind", index))
                            .selected_text(field_kind_label(field.kind))
                            .show_ui(ui, |ui| {
                                for kind in [CustomFieldKind::Token, CustomFieldKind::Integer, CustomFieldKind::DelimitedText] {
                                    changed |= ui.selectable_value(&mut field.kind, kind, field_kind_label(kind)).changed();
                                }
                            });
                        let validator = field.validator.get_or_insert_with(String::new);
                        changed |= ui.add(egui::TextEdit::singleline(validator).hint_text("optional validator regex")).changed();
                        if ui.button("Remove").clicked() { remove = Some(index); }
                    });
                }
                if let Some(index) = remove {
                    editor.profile.custom_fields.remove(index);
                    changed = true;
                }
                if ui.button("Add custom field").clicked() {
                    editor.profile.custom_fields.push(CustomFieldDefinition { name: "request_id".into(), kind: CustomFieldKind::Token, validator: None });
                    changed = true;
                }
            });
            } else {
                ui.label(RichText::new("Timestamp detection: Auto. Use a custom date recognizer in Settings if the timestamp shape is not supported.").size(15.0));
            }
                });
                });
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if ui.button("Cancel").clicked() { editor.open = false; }
                if ui.add_enabled(valid && app.record_presets_error.is_none(), egui::Button::new("Save")).clicked() {
                    action = Some(Action::Save);
                }
                let can_apply = valid && editor.apply_target_is_active(app);
                if ui.add_enabled(can_apply, egui::Button::new("Apply to this file")).clicked() {
                    action = Some(Action::Apply);
                }
                if ui.add_enabled(can_apply && app.record_presets_error.is_none(), egui::Button::new("Save & apply")).clicked() {
                    action = Some(Action::SaveApply);
                }
                if valid && !editor.apply_target_is_active(app) {
                    ui.label(RichText::new("Apply is unavailable because the file that opened this draft is no longer active.").size(12.0));
                }
            });
        });
    });
    if let Some(action) = action {
        let result = CompiledProfile::compile(editor.profile.clone())
            .map_err(|error| error.to_string())
            .and_then(|_| match action {
                Action::Save => save_record_preset(app, &mut editor),
                Action::Apply => apply_to_editor_target(app, &editor).map(|()| {
                    editor.open = false;
                }),
                Action::SaveApply => save_record_preset(app, &mut editor)
                    .and_then(|()| apply_to_editor_target(app, &editor))
                    .map(|()| editor.open = false),
            });
        if let Err(error) = result {
            editor.message = Some(error);
        }
    }
    app.record_editor = editor;
}

fn apply_to_editor_target(app: &mut HaystackApp, editor: &RecordEditor) -> Result<(), String> {
    if !editor.apply_target_is_active(app) {
        return Err("The file that opened this draft is no longer active. Save the format, then open it from that file's Format menu to apply.".into());
    }
    app.apply_record_profile(editor.profile.clone())
}

fn save_record_preset(app: &mut HaystackApp, editor: &mut RecordEditor) -> Result<(), String> {
    let mut profile = editor.profile.clone();
    let mut presets = app.record_presets.clone();
    if presets
        .iter()
        .any(|item| item.id != profile.id && item.name.eq_ignore_ascii_case(&profile.name))
    {
        return Err("A saved format has this name. Choose another name or edit it.".into());
    }
    if let Some(index) = presets.iter().position(|item| item.id == profile.id) {
        profile.revision = presets[index].revision.saturating_add(1);
        presets[index] = profile.clone();
    } else {
        presets.push(profile.clone());
    }
    save_presets(&Settings::record_profiles_path(), &presets)?;
    app.record_presets = presets;
    editor.profile = profile;
    editor.message =
        Some("Saved. This file keeps its current format until you apply the new one.".into());
    Ok(())
}

fn render_template_rules(ui: &mut egui::Ui) {
    for (syntax, meaning) in [
        ("{time}", "Timestamp. Required once."),
        (
            "{log}",
            "Message and continuation lines. Required and last.",
        ),
        ("{name}", "Any name is captured as a token by default."),
        (
            "{ignore:number}",
            "Match a header value without saving a field.",
        ),
        ("{count:number}", "A signed or decimal number."),
        (
            "{source:path}",
            "A Windows/Unix path, filename, or HTTP(S) URL.",
        ),
        (
            "{description:text}",
            "Text with spaces up to a clear separator.",
        ),
        ("{{ and }}", "Literal braces."),
    ] {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(syntax).monospace().size(13.0));
            ui.label(RichText::new(meaning).size(13.0));
        });
    }
    ui.label(
        RichText::new(
            "Captured names must be unique. Separate fields with literal punctuation or spaces.",
        )
        .size(13.0),
    );
}

fn template_layout_job(source: &str, theme: &crate::ui::theme::Theme) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let format = |color| egui::text::TextFormat {
        font_id: egui::FontId::monospace(13.0),
        color,
        ..Default::default()
    };
    let mut literal_start = 0;
    let mut cursor = 0;
    while let Some(relative_open) = source[cursor..].find('{') {
        let open = cursor + relative_open;
        if literal_start < open {
            job.append(&source[literal_start..open], 0.0, format(theme.text));
        }
        let body_start = open + 1;
        let close = source[body_start..]
            .find('}')
            .map(|offset| body_start + offset);
        job.append("{", 0.0, format(theme.text_muted));
        let Some(close) = close else {
            job.append(&source[body_start..], 0.0, format(theme.warning));
            return job;
        };
        let body = &source[body_start..close];
        if let Some(colon) = body.find(':') {
            job.append(&body[..colon], 0.0, format(theme.accent));
            job.append(":", 0.0, format(theme.text_muted));
            let type_color = match &body[colon + 1..] {
                "token" | "number" | "path" | "text" => theme.filter_colors[3],
                _ => theme.warning,
            };
            job.append(&body[colon + 1..], 0.0, format(type_color));
        } else {
            job.append(body, 0.0, format(theme.accent));
        }
        job.append("}", 0.0, format(theme.text_muted));
        cursor = close + 1;
        literal_start = cursor;
    }
    if literal_start < source.len() {
        job.append(&source[literal_start..], 0.0, format(theme.text));
    }
    job
}

fn timestamp_label(selection: &TimestampSelection) -> String {
    match selection {
        TimestampSelection::Auto => "Auto".into(),
        TimestampSelection::None => "None (timeless)".into(),
        TimestampSelection::BuiltIn(name) => name.clone(),
        TimestampSelection::Custom(name) => format!("Custom: {name}"),
    }
}

fn field_kind_label(kind: CustomFieldKind) -> &'static str {
    match kind {
        CustomFieldKind::Token => "Token",
        CustomFieldKind::Integer => "Integer",
        CustomFieldKind::DelimitedText => "Delimited text",
    }
}

struct FormatExample {
    label: &'static str,
    template: &'static str,
    sample: &'static str,
}

const EXAMPLES: &[FormatExample] = &[
    FormatExample {
        label: "Basic timestamp and message",
        template: "{time} {log}",
        sample: "2026-07-15 22:26:39.907481+0300 Started server",
    },
    FormatExample {
        label: "Named fields",
        template: "{time} [{module}] <{level}> {log}",
        sample: "2026-07-15 22:26:39.907481+0300 [network] <ERROR> Connection failed",
    },
    FormatExample {
        label: "Ignore header values",
        template: "{time} MyApp[{thread:number}:{ignore:number}] {log}",
        sample: "2026-07-15 22:26:39.907481+0300 MyApp[12345:9] Started server",
    },
    FormatExample {
        label: "Quoted path",
        template: "{time} \"{source:path}\" {log}",
        sample:
            "2026-07-15 22:26:39.907481+0300 \"/srv/My App/config.json\" Reloaded configuration",
    },
];

fn apply_example(editor: &mut RecordEditor, example: &FormatExample) {
    editor.sample = example.sample.into();
    editor.profile.source = ProfileSource::Template {
        layout: example.template.into(),
    };
    editor.profile.schema_version = RECORD_PROFILE_SCHEMA_VERSION;
    editor.changed();
}

fn render_preview(ui: &mut egui::Ui, preview: &PreviewResult, warning: egui::Color32) {
    let matched = preview
        .lines
        .iter()
        .filter(|line| line.record_start)
        .count();
    if matched == 0 {
        ui.colored_label(
            warning,
            RichText::new("No sample records matched. Check literal separators and field types.")
                .size(15.0),
        );
    } else {
        let continuations = preview.lines.len().saturating_sub(matched);
        ui.label(
            RichText::new(format!(
                "Sample matched · {matched} record{} · {continuations} continuation{}",
                if matched == 1 { "" } else { "s" },
                if continuations == 1 { "" } else { "s" },
            ))
            .size(15.0),
        );
        if let Some(line) = preview.lines.iter().find(|line| line.record_start) {
            egui::Grid::new("record_preview_fields")
                .striped(true)
                .show(ui, |ui| {
                    for heading in ["Field", "Type", "Value"] {
                        ui.label(RichText::new(heading).strong().size(15.0));
                    }
                    ui.end_row();
                    for (name, value, _) in &line.fields {
                        let kind = preview
                            .field_types
                            .iter()
                            .find(|(field, _)| field == name)
                            .map_or("token", |(_, kind)| *kind);
                        ui.label(RichText::new(name).size(15.0));
                        ui.label(RichText::new(kind).size(15.0));
                        ui.label(
                            RichText::new(value.chars().take(64).collect::<String>()).size(15.0),
                        )
                        .on_hover_text(value);
                        ui.end_row();
                    }
                });
        }
    }
    if preview.truncated {
        ui.colored_label(
            warning,
            RichText::new("Showing only the first 200 lines or 256 KiB of this sample.").size(15.0),
        );
    }
    ui.collapsing("Diagnostics", |ui| {
        ui.label(format!(
            "{} lines / {} bytes · timestamp: {} · confidence: {:?}",
            preview.lines.len(),
            preview.sampled_bytes,
            preview
                .detection
                .time_format
                .as_deref()
                .unwrap_or("unknown"),
            preview.detection.confidence
        ));
        if preview.ignored_matches > 0 {
            ui.label(format!(
                "{} ignored header value{} matched. They are used only to recognize headers.",
                preview.ignored_matches,
                if preview.ignored_matches == 1 {
                    ""
                } else {
                    "s"
                },
            ));
        }
        egui::ScrollArea::vertical()
            .max_height(190.0)
            .show(ui, |ui| {
                let mut seen_record = false;
                for line in &preview.lines {
                    let classification = if line.record_start {
                        seen_record = true;
                        "Header"
                    } else if !seen_record {
                        "Unassigned"
                    } else {
                        "Continuation"
                    };
                    let snippet: String = line.text.chars().take(160).collect();
                    ui.label(format!(
                        "L{} · {classification} · {:?} · {snippet}",
                        line.source_line, line.provenance
                    ));
                    if !line.fields.is_empty() {
                        ui.label(
                            line.fields
                                .iter()
                                .map(|(name, value, span)| {
                                    format!("{name}={value:?}@{}..{}", span.start, span.end)
                                })
                                .collect::<Vec<_>>()
                                .join("  "),
                        );
                    }
                }
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_revision_invalidates_validation_without_discarding_a_previous_preview() {
        let mut editor = RecordEditor::default();
        editor.preview_result_revision = Some(0);
        editor.format_valid = Some(Ok(()));
        editor.changed();
        assert_eq!(editor.preview_revision, 1);
        assert!(editor.format_valid.is_none());
        assert!(editor.preview_is_stale());
        assert_eq!(editor.preview_result_revision, Some(0));
    }

    #[test]
    fn stale_worker_result_does_not_replace_current_validation() {
        let mut editor = RecordEditor::default();
        editor.preview_revision = 2;
        editor.format_valid = Some(Ok(()));
        let (tx, rx) = crossbeam_channel::bounded(1);
        tx.send(PreviewUpdate {
            revision: 1,
            format_valid: Err("stale".into()),
            preview: None,
        })
        .unwrap();
        editor.preview_rx = Some(rx);
        editor.poll_preview(&[]);
        assert!(matches!(editor.format_valid, Some(Ok(()))));
        assert!(editor.preview_rx.is_none());
    }

    #[test]
    fn applying_an_example_preserves_the_user_entered_name() {
        let mut editor = RecordEditor::default();
        editor.profile.name = "Production console".into();
        apply_example(&mut editor, &EXAMPLES[2]);
        assert_eq!(editor.profile.name, "Production console");
        assert!(editor.sample.contains("MyApp[12345:9]"));
        assert!(matches!(
            editor.profile.source,
            ProfileSource::Template { .. }
        ));
    }

    #[test]
    fn legacy_conversion_creates_a_new_schema_three_draft() {
        let mut editor = RecordEditor::default();
        editor.select_profile(RecordProfile::inline(
            "legacy:convertible",
            "Old console",
            "{time} [{worker}] {log}",
        ));
        let converted = editor.legacy_conversion().unwrap();
        assert_eq!(converted.schema_version, RECORD_PROFILE_SCHEMA_VERSION);
        assert_ne!(converted.id, editor.profile.id);
        assert_eq!(converted.name, "Old console (schema 3)");
    }

    #[test]
    fn legacy_conversion_keeps_captured_ignore_legacy() {
        let mut editor = RecordEditor::default();
        editor.select_profile(RecordProfile::inline(
            "legacy:ignore",
            "Old ignore",
            "{time} [{ignore}] {log}",
        ));
        assert!(editor.legacy_conversion().is_err());
    }
}
