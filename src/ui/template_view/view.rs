use super::model::TemplateSort;
use crate::ui::app::model::LogTab;
use crate::ui::icons::{self, Icon};
use crate::ui::theme::Theme;
use eframe::egui;

/// Render the Templates dock tab. The data/cache model will live alongside
/// this view; keeping this entry point independent lets the same UI render in
/// a dock leaf and in a popped-out viewport.
pub fn show(ui: &mut egui::Ui, tab: &mut LogTab, theme: &Theme) {
    tab.template_browser.refresh(&tab.doc);
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = icons::ACTION_HEIGHT;
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.add(icons::icon_image(
            ui.ctx(),
            Icon::Search,
            14.0,
            theme.text_muted,
        ))
        .on_hover_text("Search templates");
        ui.add_sized(
            egui::vec2(190.0, icons::ACTION_HEIGHT),
            egui::TextEdit::singleline(&mut tab.template_browser.query)
                .hint_text("Search pattern or T-ID…"),
        );
        let before = tab.template_browser.sort;
        egui::ComboBox::from_id_salt("template_sort")
            .selected_text(tab.template_browser.sort.label())
            .show_ui(ui, |ui| {
                for sort in TemplateSort::ALL {
                    ui.selectable_value(&mut tab.template_browser.sort, sort, sort.label());
                }
            });
        if before != tab.template_browser.sort {
            tab.template_browser.resort(&tab.doc);
        }
        if icons::icon_action_button(
            ui,
            if tab.template_browser.descending {
                Icon::ArrowDown
            } else {
                Icon::ArrowUp
            },
            theme.text,
            "Reverse template sort order",
        )
        .clicked()
        {
            tab.template_browser.descending = !tab.template_browser.descending;
            tab.template_browser.resort(&tab.doc);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icons::action_button(
                ui,
                Icon::Export,
                "JSON",
                theme.text_muted,
                "Export template counts as JSON",
            )
            .clicked()
            {
                save_templates(
                    tab,
                    "template-counts.json",
                    templates_json(tab),
                    "Template counts exported",
                );
            }
            if icons::action_button(
                ui,
                Icon::Export,
                "CSV",
                theme.text_muted,
                "Export template counts as CSV",
            )
            .clicked()
            {
                save_templates(
                    tab,
                    "template-counts.csv",
                    templates_csv(tab),
                    "Template counts exported",
                );
            }
            ui.label(
                egui::RichText::new("Export")
                    .small()
                    .color(theme.text_muted),
            );
        });
    });
    let order = tab.template_browser.visible_order(&tab.doc);
    if order.is_empty() {
        ui.add_space(12.0);
        ui.label(
            egui::RichText::new(if tab.template_browser.query.trim().is_empty() {
                "No templates were mined from this log."
            } else {
                "No templates match this search. Try a pattern or Template ID."
            })
            .color(theme.text_muted),
        );
        return;
    }
    handle_template_keys(ui, tab, &order);
    let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, row_height, order.len(), |ui, range| {
            for &row_index in &order[range] {
                let (ti, rare, late, bursty) = {
                    let row = &tab.template_browser.rows[row_index];
                    (row.template_index, row.rare, row.late, row.bursty)
                };
                let t = &tab.doc.templates[ti];
                let (count, id, example_line, pattern) =
                    (t.count, t.id, t.example_line, t.pattern.clone());
                let anomalies = [
                    rare.then_some("rare"),
                    late.then_some("late"),
                    bursty.then_some("bursty"),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(", ");
                let label = format!(
                    "×{:<7} T{:<4} {}{}",
                    count,
                    id,
                    pattern,
                    if anomalies.is_empty() {
                        String::new()
                    } else {
                        format!("  [{anomalies}]")
                    }
                );
                let row_rect = ui.available_rect_before_wrap();
                let row_rect = egui::Rect::from_min_size(
                    row_rect.min,
                    egui::vec2(row_rect.width(), row_height),
                );
                let selected = tab.template_browser.selected_id == Some(id);
                let resp = ui.interact(
                    row_rect,
                    ui.id().with(("template_row", id)),
                    egui::Sense::click(),
                );
                if selected {
                    ui.painter()
                        .rect_filled(row_rect, 2.0, ui.visuals().selection.bg_fill);
                } else if resp.hovered() {
                    ui.painter().rect_filled(
                        row_rect,
                        2.0,
                        ui.visuals().widgets.hovered.weak_bg_fill,
                    );
                }
                ui.painter().text(
                    row_rect.left_center() + egui::vec2(4.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    &label,
                    egui::FontId::monospace(11.0),
                    ui.visuals().text_color(),
                );
                ui.advance_cursor_after_rect(row_rect);
                if resp.clicked() {
                    select_template(tab, id, example_line);
                }
                if resp.hovered() {
                    ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                }
                if selected {
                    selected_row_commands(ui, tab, theme, id, &pattern, &label, row_rect);
                }
            }
        });
}

fn handle_template_keys(ui: &mut egui::Ui, tab: &mut LogTab, order: &[usize]) {
    if tab.template_browser.selected_id.is_none() || ui.ctx().egui_wants_keyboard_input() {
        return;
    }
    let mut previous_occurrence = false;
    let mut next_occurrence = false;
    let mut vertical: Option<bool> = None;
    ui.input_mut(|input| {
        previous_occurrence = input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft);
        next_occurrence = input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight);
        vertical = input
            .consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)
            .then_some(false)
            .or_else(|| {
                input
                    .consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)
                    .then_some(true)
            });
    });
    if previous_occurrence || next_occurrence {
        if let Some(id) = tab.template_browser.selected_id {
            navigate_selected_occurrence(tab, id, next_occurrence);
        }
    }
    if let Some(next) = vertical {
        let Some(id) = tab.template_browser.selected_id else {
            return;
        };
        let Some(position) = order.iter().position(|&row_index| {
            tab.doc.templates[tab.template_browser.rows[row_index].template_index].id == id
        }) else {
            return;
        };
        let new_position = if next {
            (position + 1).min(order.len().saturating_sub(1))
        } else {
            position.saturating_sub(1)
        };
        if let Some(&row_index) = order.get(new_position) {
            let selected =
                tab.doc.templates[tab.template_browser.rows[row_index].template_index].id;
            let example_line =
                tab.doc.templates[tab.template_browser.rows[row_index].template_index].example_line;
            select_template(tab, selected, example_line);
        }
    }
}

fn select_template(tab: &mut LogTab, id: u32, fallback: usize) {
    let occurrences = tab
        .template_browser
        .row_for_id(&tab.doc, id)
        .map(|row| row.occurrences.clone())
        .unwrap_or_default();
    tab.template_browser.selected_id = Some(id);
    let midpoint = tab
        .viewport_range
        .map(|(first, last)| first + (last - first) / 2)
        .or(tab.context_line)
        .unwrap_or(fallback);
    let target = nearest_occurrence(&occurrences, midpoint).unwrap_or(fallback);
    tab.select_and_scroll_to(target);
}

fn navigate_selected_occurrence(tab: &mut LogTab, id: u32, next: bool) {
    let Some(lines) = tab
        .template_browser
        .row_for_id(&tab.doc, id)
        .map(|row| row.occurrences.clone())
    else {
        return;
    };
    if lines.is_empty() {
        return;
    }
    let current = tab.context_line.unwrap_or(lines[0]);
    let pos = lines.partition_point(|&line| {
        if next {
            line <= current
        } else {
            line < current
        }
    });
    let target = if next {
        lines.get(pos).copied().unwrap_or(lines[0])
    } else {
        lines
            .get(pos.saturating_sub(1))
            .copied()
            .unwrap_or(*lines.last().unwrap())
    };
    tab.select_and_scroll_to(target);
    tab.template_browser.selected_id = Some(id);
}

fn nearest_occurrence(lines: &[usize], target: usize) -> Option<usize> {
    let pos = lines.partition_point(|&line| line < target);
    match (
        pos.checked_sub(1).and_then(|index| lines.get(index)),
        lines.get(pos),
    ) {
        (Some(&before), Some(&after)) => Some(if target - before <= after - target {
            before
        } else {
            after
        }),
        (Some(&before), None) => Some(before),
        (None, Some(&after)) => Some(after),
        (None, None) => None,
    }
}

fn selected_row_commands(
    ui: &mut egui::Ui,
    tab: &mut LogTab,
    theme: &Theme,
    id: u32,
    pattern: &str,
    label: &str,
    row_rect: egui::Rect,
) {
    const BUTTONS: usize = 3;
    const GAP: f32 = 3.0;
    const PADDING: f32 = 5.0;
    let button_height = row_rect.height() * 1.5;
    // A slightly wider hit target gives the SVG controls breathing room
    // without making the selected row feel like a toolbar.
    let button_width = button_height * 1.18;
    let palette_width = PADDING * 2.0 + button_width * BUTTONS as f32 + GAP * (BUTTONS - 1) as f32;
    let text_width = ui
        .painter()
        .layout_no_wrap(
            label.to_owned(),
            egui::FontId::monospace(11.0),
            ui.visuals().text_color(),
        )
        .size()
        .x;
    let desired_left = row_rect.left() + 4.0 + text_width + 8.0;
    let palette_left = desired_left
        .min(row_rect.right() - palette_width)
        .max(row_rect.left());
    let palette = egui::Rect::from_min_size(
        egui::pos2(palette_left, row_rect.center().y - button_height / 2.0),
        egui::vec2(palette_width, button_height),
    );
    // `surface` is deliberately opaque in both themes, so the command strip
    // stays legible over long template text in light and dark mode.
    ui.painter().rect_filled(palette, 4.0, theme.surface);
    let actions = [
        (Icon::Search, "Search this Template ID in Log"),
        (Icon::Filter, "Add Template ID filter"),
        (Icon::Copy, "Copy pattern"),
    ];
    for (index, (icon, hint)) in actions.iter().enumerate() {
        let rect = egui::Rect::from_min_size(
            egui::pos2(
                palette.left() + PADDING + index as f32 * (button_width + GAP),
                palette.top() + PADDING,
            ),
            egui::vec2(button_width - PADDING * 2.0, button_height - PADDING * 2.0),
        );
        let response = icons::icon_button_at(ui, rect, *icon, theme.text);
        if response.hovered() {
            response.clone().on_hover_text(*hint);
        }
        if !response.clicked() {
            continue;
        }
        match index {
            0 => tab.start_template_id_search(id),
            1 => {
                let color = theme.filter_colors[tab.filters.len() % theme.filter_colors.len()];
                if tab.push_template_filter(id, color).is_none() {
                    tab.pending_toast = Some("A maximum of 20 filters can be added.".into());
                }
            }
            _ => {
                ui.ctx().copy_text(pattern.to_owned());
                tab.pending_toast = Some("Template pattern copied".into());
            }
        }
    }
}

fn save_templates(tab: &mut LogTab, name: &str, contents: String, success: &str) {
    let Some(path) = rfd::FileDialog::new().set_file_name(name).save_file() else {
        return;
    };
    match std::fs::write(path, contents) {
        Ok(()) => tab.pending_toast = Some(success.into()),
        Err(error) => tab.pending_toast = Some(format!("Export failed: {error}")),
    }
}

fn templates_csv(tab: &LogTab) -> String {
    let mut out = "template_id,count,example_line,pattern\n".to_string();
    for t in &tab.doc.templates {
        out.push_str(&format!(
            "{},{},{},\"{}\"\n",
            t.id,
            t.count,
            tab.doc.trim_start + t.example_line + 1,
            t.pattern.replace('"', "\"\"")
        ));
    }
    out
}

fn templates_json(tab: &LogTab) -> String {
    let rows: Vec<serde_json::Value> = tab.doc.templates.iter().map(|t| serde_json::json!({
        "template_id": t.id, "count": t.count, "example_line": tab.doc.trim_start + t.example_line + 1, "pattern": t.pattern,
    })).collect();
    serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".into())
}

#[cfg(test)]
mod tests {
    use super::nearest_occurrence;

    #[test]
    fn nearest_occurrence_prefers_the_closest_template_line() {
        let lines = [10, 40, 90];
        assert_eq!(nearest_occurrence(&lines, 0), Some(10));
        assert_eq!(nearest_occurrence(&lines, 36), Some(40));
        assert_eq!(nearest_occurrence(&lines, 70), Some(90));
        assert_eq!(nearest_occurrence(&lines, 200), Some(90));
    }
}
