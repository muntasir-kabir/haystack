//! Shared Field-mode completion for Search and Filter.

use eframe::egui;
use egui::RichText;
use haystack::core::document::LogDocument;
use haystack::core::field_query::{FieldQuery, FieldValueSuggestions};

pub(crate) fn is_field_criteria_kind(kind: &str) -> bool {
    kind != "message"
}

pub fn show(
    ui: &mut egui::Ui,
    id: &'static str,
    doc: &LogDocument,
    input: &str,
    suggestions: Option<&FieldValueSuggestions>,
    error: Option<&str>,
    anchor: egui::Rect,
) -> Option<String> {
    // Keep Escape dismissed until the expression changes. Otherwise the
    // focused text box would reopen the popup on the very next frame.
    let dismissed_id = egui::Id::new(id).with("dismissed_input");
    if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
        ui.ctx()
            .data_mut(|data| data.insert_temp(dismissed_id, input.to_owned()));
        return None;
    }
    let reopen = ui.input(|input| input.modifiers.command && input.key_pressed(egui::Key::Space));
    if reopen {
        ui.ctx()
            .data_mut(|data| data.remove::<String>(dismissed_id));
    }
    if ui
        .ctx()
        .data(|data| data.get_temp::<String>(dismissed_id))
        .as_deref()
        == Some(input)
    {
        return None;
    }
    let mut selected = None;
    let available = ui.ctx().content_rect();
    let width = 440.0_f32.min((available.width() - 24.0).max(220.0));
    let left = anchor
        .left()
        .min((available.right() - width - 12.0).max(12.0));
    let top = if available.bottom() - anchor.bottom() < 230.0 {
        (anchor.top() - 230.0).max(available.top() + 8.0)
    } else {
        anchor.bottom()
    };
    egui::Area::new(egui::Id::new(id))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(left, top))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(anchor.width().min(width).max(220.0));
                ui.set_max_width(width);
                egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                    if let Some(error) = error {
                        ui.colored_label(ui.visuals().error_fg_color, error);
                        ui.separator();
                    }
                    if let Some((field, _)) = FieldQuery::completion_context(input) {
                        ui.label(RichText::new(format!("Values for {field}")).size(14.0));
                        if doc.record_field_type(field).is_none() {
                            ui.label("Choose a field from the applied log format.");
                        } else if doc.record_field_type(field) == Some("message") {
                            ui.label("Choose a captured header field; use Text or Regex to search log messages.");
                        } else if let Some(suggestions) = suggestions {
                            for entry in &suggestions.values {
                                let display = if entry.value.chars().count() > 72 {
                                    format!("{}…", entry.value.chars().take(72).collect::<String>())
                                } else { entry.value.clone() };
                                if ui.button(format!("{display}  ·  {}", entry.count))
                                    .on_hover_text(&entry.value).clicked()
                                {
                                    let operator = [">=", "<=", "!=", "=", ">", "<", "contains", "matches"]
                                        .into_iter()
                                        .find(|operator| input.trim_start()[field.len()..].trim_start().starts_with(operator))
                                        .unwrap_or("=");
                                    let quoted = serde_json::to_string(&entry.value).unwrap_or_default();
                                    selected = Some(format!("{field} {operator} {quoted}"));
                                }
                            }
                            if suggestions.values.is_empty() { ui.label("No captured values match this prefix."); }
                            if !suggestions.complete {
                                ui.label(RichText::new(format!(
                                    "Suggestions limited after {} records; search results are not limited.",
                                    suggestions.scanned_records,
                                )).size(14.0));
                            }
                        } else {
                            ui.spinner();
                            ui.label("Checking captured values…");
                            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                        }
                    } else {
                        let typed = input.trim();
                        let names: Vec<_> = doc
                            .record_field_schema()
                            .into_iter()
                            .filter(|(_, kind)| is_field_criteria_kind(kind))
                            .collect();
                        if names.is_empty() {
                            ui.label("This log format has no captured header fields.");
                        } else if names.iter().any(|(name, _)| *name == typed) && input.ends_with(' ') {
                            ui.label(RichText::new("Choose an operator").size(14.0));
                            let kind = doc.record_field_type(typed).unwrap_or("token");
                            let operators = if matches!(kind, "number" | "integer" | "timestamp") {
                                &["=", "!=", ">=", "<=", "exists", "missing"][..]
                            } else {
                                &["=", "!=", "contains", "matches", "exists", "missing"][..]
                            };
                            for operator in operators {
                                if ui.button(*operator).clicked() { selected = Some(format!("{typed} {operator} ")); }
                            }
                        } else {
                            ui.label(RichText::new("Fields in this log format").size(14.0));
                            for (name, kind) in names {
                                if name.starts_with(typed) && ui.button(format!("{name}  ·  {kind}")).clicked() {
                                    selected = Some(format!("{name} = "));
                                }
                            }
                        }
                    }
                });
            });
        });
    selected
}

#[cfg(test)]
mod tests {
    use super::is_field_criteria_kind;

    #[test]
    fn message_capture_is_not_a_field_criteria_option() {
        assert!(!is_field_criteria_kind("message"));
        for kind in ["token", "text", "path", "number", "timestamp"] {
            assert!(is_field_criteria_kind(kind));
        }
    }
}
