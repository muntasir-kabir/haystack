//! Filter bookmark strip: add filters, inspect scan progress, and recall
//! recent entries. Advanced input options intentionally live beside Add so
//! there is one clear way to create a filter.

use eframe::egui;
use std::time::{Duration, Instant};

use crate::ui::app::model::Filter;
use crate::ui::app::model::{LogTab, MAX_FILTERS};
use crate::ui::icons::{self, Icon};
use crate::ui::theme::Theme;
use crate::ui::util::error_bubble::{error_bubble, BubbleAlign};
use crate::ui::util::suggestion_row;
use logotomy::core::search::{parse_template_id, validate_regex};

/// Compact Add Filter section for the timeline header.
pub fn add_filter_ui(ui: &mut egui::Ui, tab: &mut LogTab, theme: &Theme) {
    const REGEX_DEBOUNCE: Duration = Duration::from_millis(150);
    let at_cap = tab.filters.len() >= MAX_FILTERS;
    if !tab.filter_input_regex && !tab.filter_input_template_id {
        tab.filter_input_regex_validate_at = None;
        tab.filter_input_regex_error = None;
        tab.filter_input_regex_error_dismissed = false;
    } else if tab
        .filter_input_regex_validate_at
        .is_some_and(|when| Instant::now() >= when)
    {
        let input = tab.filter_input.trim();
        tab.filter_input_regex_error = if input.is_empty() {
            None
        } else if tab.filter_input_template_id {
            parse_template_id(input).err()
        } else {
            validate_regex(input, tab.filter_input_case_sensitive).err()
        };
        tab.filter_input_regex_validate_at = None;
        tab.filter_input_regex_error_dismissed = false;
    }

    let mut input_rect = None;
    let mut show_recent_suggestions = false;
    let mut input_has_focus = false;

    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = icons::ACTION_HEIGHT;
        ui.add(icons::icon_image(ui.ctx(), Icon::Filter, 14.0, theme.text_muted))
            .on_hover_text("Add a timeline filter");
        ui.label("Filter")
            .on_hover_text("Enter a phrase, regular expression, or Template ID.");
        let input = ui
            .add_enabled_ui(!at_cap, |ui| {
                ui.add_sized(
                    egui::vec2(225.0, icons::ACTION_HEIGHT),
                    egui::TextEdit::singleline(&mut tab.filter_input)
                        .hint_text(if at_cap {
                            "Max 20 filters"
                        } else if tab.filter_input_template_id {
                            "42, T42, or T{42} + Enter"
                        } else {
                            "filter + Enter"
                        })
                        .desired_width(225.0),
                )
            })
            .inner
            .on_hover_text("Type a phrase, regular expression, or Template ID. Press Enter or Add to create the filter.");
        input_rect = Some(input.rect);
        if input.has_focus() && tab.filter_input.trim().is_empty() && !at_cap && !tab.filter_input_template_id {
            tab.filter_suggestions_open = true;
        }
        input_has_focus = input.has_focus();
        if input.changed() && !tab.filter_input.trim().is_empty() {
            tab.filter_suggestions_open = false;
        }
        show_recent_suggestions = tab.filter_suggestions_open
            && tab.filter_input.trim().is_empty()
            && !at_cap
            && !tab.filter_input_template_id;
        if input.changed() && (tab.filter_input_regex || tab.filter_input_template_id) {
            tab.filter_input_regex_validate_at = Some(Instant::now() + REGEX_DEBOUNCE);
            tab.filter_input_regex_error = None;
            tab.filter_input_regex_error_dismissed = false;
        }
        let regex_validation_pending = tab.filter_input_regex_validate_at.is_some();
        let can_add = !at_cap
            && !tab.filter_input.trim().is_empty()
            && !regex_validation_pending
            && tab.filter_input_regex_error.is_none();
        let enter = input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let add = icons::action_button_enabled(
            ui,
            can_add,
            Icon::Add,
            "Add",
            theme.text,
            if at_cap {
                "A maximum of 20 filters can be added. Remove a filter to add another."
            } else if regex_validation_pending {
                "Checking the filter value…"
            } else if tab.filter_input_regex_error.is_some() {
                "Fix this filter value before adding it."
            } else {
                "Add this filter and start a background scan."
            },
        );
        let mode_label = if tab.filter_input_template_id {
            "Template ID"
        } else if tab.filter_input_regex {
            "Regex"
        } else if tab.filter_input_case_sensitive {
            "Text (Aa)"
        } else {
            "Text (Ab)"
        };
        // `selectable_value` needs a real mutable value. Binding it directly
        // to a tuple expression creates a temporary, so selections looked
        // clickable but were discarded at the end of the frame.
        let original_mode = if tab.filter_input_template_id {
            3_u8
        } else if tab.filter_input_regex {
            2_u8
        } else if tab.filter_input_case_sensitive {
            0
        } else {
            1
        };
        let mut mode = original_mode;
        egui::ComboBox::from_id_salt("filter_input_match_mode")
            .selected_text(mode_label)
            .width(108.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut mode, 0, "Text (Aa)")
                    .on_hover_text("Plain text; match upper- and lower-case letters exactly.")
                    ;
                ui.selectable_value(&mut mode, 1, "Text (Ab)")
                    .on_hover_text("Plain text; match ASCII upper- and lower-case letters alike.")
                    ;
                ui.selectable_value(&mut mode, 2, "Regex")
                    .on_hover_text("Rust regular expression; case follows the previous Text mode and is linear-time and size-limited.")
                    ;
                ui.selectable_value(&mut mode, 3, "Template ID")
                    .on_hover_text("Mined Drain template ID. Enter digits (for example 42), T42, or T{42}.");
            })
            .response
            .on_hover_text("Choose text, regular-expression, or Drain Template ID matching.");
        let mode_changed = mode != original_mode;
        if mode_changed {
            match mode {
                0 => {
                    tab.filter_input_regex = false;
                    tab.filter_input_template_id = false;
                    tab.filter_input_case_sensitive = true;
                }
                1 => {
                    tab.filter_input_regex = false;
                    tab.filter_input_template_id = false;
                    tab.filter_input_case_sensitive = false;
                }
                2 => {
                    tab.filter_input_regex = true;
                    tab.filter_input_template_id = false;
                }
                _ => {
                    tab.filter_input_regex = false;
                    tab.filter_input_template_id = true;
                    tab.filter_suggestions_open = false;
                }
            }
        }

        if (enter && can_add) || add.clicked() {
            let color = theme.filter_colors[tab.filters.len() % theme.filter_colors.len()];
            if tab.filter_input_template_id {
                let text = tab.filter_input.trim().to_string();
                tab.push_template_filter_input(&text, color);
            } else {
                let text = tab.filter_input.trim().to_string();
                tab.push_filter_with_options(
                    &text,
                    color,
                    tab.filter_input_case_sensitive,
                    tab.filter_input_regex,
                );
            }
            tab.filter_input.clear();
            input.request_focus();
        }
        if mode_changed {
            if tab.filter_input_regex || tab.filter_input_template_id {
                tab.filter_input_regex_validate_at = Some(Instant::now() + REGEX_DEBOUNCE);
                tab.filter_input_regex_error = None;
                tab.filter_input_regex_error_dismissed = false;
            }
            ui.ctx().request_repaint_after(REGEX_DEBOUNCE);
        }
        if tab.search_rx.is_some() || tab.visible_rx.is_some() {
            ui.spinner()
                .on_hover_text("A filter result or visible-line index is being rebuilt.");
        }
        if let Some(progress) = &tab.filter_scan_progress {
            use std::sync::atomic::Ordering;
            let scanned = progress.scanned_lines.load(Ordering::Relaxed);
            let total = progress.total_lines.load(Ordering::Relaxed);
            if total > 0 {
                ui.label(format!("{scanned}/{total}"))
                    .on_hover_text("Lines scanned so far / total lines in the active trim.");
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            }
            if icons::action_button(
                ui,
                Icon::Close,
                "Cancel",
                theme.text,
                "Stop this filter scan and keep the previous results",
            )
                .clicked()
            {
                if let Some((_, cancel)) = &tab.search_rx {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
        }
    });

    if tab.filter_input_regex_validate_at.is_some() {
        ui.ctx().request_repaint_after(REGEX_DEBOUNCE);
    } else if let (Some(error), Some(anchor)) = (&tab.filter_input_regex_error, input_rect) {
        let mut bubble_open = !tab.filter_input_regex_error_dismissed;
        error_bubble(
            ui.ctx(),
            "filter_input_regex_error",
            anchor,
            BubbleAlign::Below,
            error,
            &mut bubble_open,
        );
        tab.filter_input_regex_error_dismissed = !bubble_open;
    }

    // Suggestions deliberately live in a floating area instead of a nested
    // timeline row: the Timeline header has fixed height and must not push the
    // histogram out of view. Only offer entries not already installed.
    let suggestions = recent_filter_suggestions(&tab.filter_history, &tab.filters);
    if show_recent_suggestions && !suggestions.is_empty() {
        let anchor = input_rect.expect("filter input always has a rect");
        let mut chosen = None;
        let suggestion_area = egui::Area::new(egui::Id::new("timeline_filter_recent_suggestions"))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor.left_bottom())
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(anchor.width());
                    ui.set_max_width(anchor.width());
                    ui.label(egui::RichText::new("Recent filters").weak());
                    for entry in &suggestions {
                        if suggestion_row::show(ui, &entry.text)
                            .on_hover_text("Add this recent filter immediately")
                            .clicked()
                        {
                            chosen = Some(entry.clone());
                        }
                    }
                });
            });
        if let Some(entry) = chosen {
            let color = theme.filter_colors[tab.filters.len() % theme.filter_colors.len()];
            tab.push_filter_with_options(&entry.text, color, entry.case_sensitive, entry.regex);
            tab.filter_input.clear();
            tab.filter_suggestions_open = false;
        } else if ui.input(|input| input.key_pressed(egui::Key::Escape))
            || (ui.input(|input| input.pointer.any_click())
                && !input_has_focus
                && !suggestion_area.response.hovered())
        {
            tab.filter_suggestions_open = false;
        }
    }
}

fn recent_filter_suggestions(
    history: &[logotomy::core::settings::RecentFilter],
    active_filters: &[Filter],
) -> Vec<logotomy::core::settings::RecentFilter> {
    history
        .iter()
        .filter(|entry| {
            !active_filters
                .iter()
                .any(|filter| filter.text == entry.text)
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Color32;
    use logotomy::core::settings::RecentFilter;

    #[test]
    fn recent_filter_suggestions_omit_already_added_filters() {
        let history = vec![
            RecentFilter {
                text: "error".into(),
                case_sensitive: false,
                regex: false,
            },
            RecentFilter {
                text: "warning".into(),
                case_sensitive: true,
                regex: false,
            },
        ];
        let active = vec![Filter {
            text: "error".into(),
            color: Color32::RED,
        }];
        let suggestions = recent_filter_suggestions(&history, &active);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].text, "warning");
        assert!(suggestions[0].case_sensitive);
    }
}
