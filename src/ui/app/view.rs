use std::path::PathBuf;
use std::time::Duration;

use eframe::egui;
use egui::{RichText, Stroke};
use egui_dock::{DockArea, DockState};
use log::info;

use crate::ui::icons::{self, Icon};
use crate::ui::log_view;
use crate::ui::pin_viewer;
use crate::ui::template_view;
use crate::ui::theme::Theme;
use crate::ui::timeline;
use haystack::core::time::parse_time_param;
use haystack::core::time_query::nearest_record_time;

use super::model::*;
use crate::ui::custom_date;
use crate::ui::record_format;
use crate::ui::settings;

#[path = "filters_dropdown.rs"]
mod filters_dropdown;
#[path = "format_menu.rs"]
mod format_menu;
#[path = "key_listener.rs"]
mod key_listener;
use super::overlay;
use key_listener::{AppCommand, ALIASES, COMMANDS};

#[derive(Clone, Copy)]
enum TopPanelDropdown {
    Recent,
    Filters,
    More,
}

fn dropdown_visibility(selected: TopPanelDropdown, was_open: bool) -> (bool, bool, bool) {
    let open = !was_open;
    match selected {
        TopPanelDropdown::Recent => (open, false, false),
        TopPanelDropdown::Filters => (false, open, false),
        TopPanelDropdown::More => (false, false, open),
    }
}

fn compact_top_bar(available_width: f32) -> bool {
    available_width < 1100.0
}

fn open_file_shortcut() -> &'static str {
    if cfg!(target_os = "macos") {
        "Cmd+O"
    } else {
        "Ctrl+O"
    }
}

fn document_status_context(format: Option<&str>, status: &str) -> String {
    match format {
        Some(format) if !status.is_empty() => format!("{format} · {status}"),
        Some(format) => format.to_owned(),
        None => String::new(),
    }
}

fn should_request_repaint(
    any_search: bool,
    loaders_changed: bool,
    loaders_in_flight: bool,
    mcp_enabled: bool,
) -> bool {
    any_search || loaders_changed || loaders_in_flight || mcp_enabled
}

/// Save a renderer-native viewport capture when the tracked-screenshot
/// environment hook is active. This avoids OS screen-recording permissions and
/// keeps README captures deterministic across local and CI workflows.
fn consume_tracked_screenshot(ctx: &egui::Context) {
    let Ok(target) = std::env::var("HAYSTACK_SCREENSHOT_PATH") else {
        return;
    };
    for event in ctx.input(|input| input.events.clone()) {
        let egui::Event::Screenshot {
            user_data, image, ..
        } = event
        else {
            continue;
        };
        let matches_target = user_data
            .data
            .as_ref()
            .and_then(|data| data.downcast_ref::<PathBuf>())
            .is_some_and(|path| path == &PathBuf::from(&target));
        if !matches_target {
            continue;
        }
        let rgba = image
            .pixels
            .iter()
            .flat_map(|pixel| pixel.to_array())
            .collect::<Vec<_>>();
        if let Err(error) = image::save_buffer(
            &target,
            &rgba,
            image.size[0] as u32,
            image.size[1] as u32,
            image::ColorType::Rgba8,
        ) {
            log::error!("failed to save tracked screenshot to {target}: {error}");
        } else {
            info!("saved tracked screenshot to {target}");
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

fn request_tracked_screenshot(ctx: &egui::Context, content_ready: bool) {
    let Ok(target) = std::env::var("HAYSTACK_SCREENSHOT_PATH") else {
        return;
    };
    let requested_id = egui::Id::new("tracked_screenshot_requested");
    if ctx.data(|data| data.get_temp::<bool>(requested_id).unwrap_or(false)) || !content_ready {
        return;
    }

    let frame_id = egui::Id::new("tracked_screenshot_ready_frames");
    let ready_frames = ctx.data(|data| data.get_temp::<u8>(frame_id).unwrap_or(0));
    if ready_frames < 12 {
        ctx.data_mut(|data| data.insert_temp(frame_id, ready_frames + 1));
        ctx.request_repaint_after(Duration::from_millis(100));
        return;
    }

    ctx.data_mut(|data| data.insert_temp(requested_id, true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
        PathBuf::from(target),
    )));
}

impl Drop for HaystackApp {
    fn drop(&mut self) {
        if self.mcp_enabled {
            self.stop_mcp();
        }
        // Tracked screenshot runs are renderer-only previews. Avoid changing
        // the user's workspace, sidecars, recents, or persisted settings.
        if std::env::var_os("HAYSTACK_SCREENSHOT_PATH").is_some() {
            return;
        }
        self.flush_sidecars();
        self.save_workspace();
        self.settings.save();
        info!("settings saved on exit");
    }
}

impl HaystackApp {
    fn dismiss_app_overlay_on_escape(&mut self, ctx: &egui::Context) {
        let tab_confirmation_open = self
            .active
            .and_then(|index| self.tabs.get(index))
            .is_some_and(|tab| tab.pending_filter_removal.is_some() || tab.pending_clear_filters);
        // The occurrence overlay owns Escape unless an inspector is layered
        // above it; inspectors get first chance to close themselves.
        let occurrence_overlay_open = self
            .active
            .and_then(|index| self.tabs.get(index))
            .is_some_and(|tab| {
                tab.log_views.values().any(|view| {
                    view.occurrence_overlay.is_some() && view.embedded_inspector.is_none()
                })
            });
        let has_overlay = self.show_integrate_popup
            || self.record_editor.open
            || self.show_custom_date_popup
            || self.show_command_palette
            || self.show_goto_popup
            || self.show_cheat_sheet
            || self.show_new_filter_popup
            || self.show_rename_filter_popup
            || self.mcp_error_popup.is_some()
            || self.pending_restore_tab.is_some()
            || self.zip_imports.error.is_some()
            || self.close_save_error.is_some()
            || self.show_settings_popup
            || self.show_ai_assistant_popup
            || self.recent_show_dropdown
            || self.show_filter_dropdown
            || self.views_show_dropdown
            || self.show_format_dropdown
            || tab_confirmation_open
            || occurrence_overlay_open;
        if !has_overlay {
            if let Some(tab) = self.active.and_then(|index| self.tabs.get_mut(index)) {
                if tab.log_focus_mode
                    && ctx.input_mut(|input| {
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
                    })
                {
                    tab.log_focus_mode = false;
                }
            }
            return;
        }
        if !ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            return;
        }

        if self.show_integrate_popup {
            self.show_integrate_popup = false;
        } else if self.record_editor.open {
            if !self.record_editor.dismiss_completion() {
                self.record_editor.open = false;
            }
        } else if self.show_custom_date_popup {
            self.show_custom_date_popup = false;
        } else if self.show_command_palette {
            self.show_command_palette = false;
        } else if self.show_goto_popup {
            self.show_goto_popup = false;
            self.goto_error = None;
        } else if self.show_cheat_sheet {
            self.show_cheat_sheet = false;
        } else if self.show_new_filter_popup {
            self.show_new_filter_popup = false;
            self.new_filter_name.clear();
        } else if self.show_rename_filter_popup {
            self.show_rename_filter_popup = false;
        } else if self.mcp_error_popup.is_some() {
            self.mcp_error_popup = None;
        } else if self.pending_restore_tab.is_some() {
            self.pending_restore_tab = None;
        } else if self.zip_imports.error.is_some() {
            self.zip_imports.error = None;
        } else if self.close_save_error.is_some() {
            self.close_save_error = None;
        } else if self.show_settings_popup {
            self.show_settings_popup = false;
        } else if self.show_ai_assistant_popup {
            self.show_ai_assistant_popup = false;
        } else if self.recent_show_dropdown {
            self.recent_show_dropdown = false;
        } else if self.show_filter_dropdown {
            self.show_filter_dropdown = false;
        } else if self.views_show_dropdown {
            self.views_show_dropdown = false;
        } else if self.show_format_dropdown {
            self.show_format_dropdown = false;
        } else if let Some(tab) = self.active.and_then(|index| self.tabs.get_mut(index)) {
            if let Some(view) = tab
                .log_views
                .values_mut()
                .find(|view| view.occurrence_overlay.is_some() && view.embedded_inspector.is_none())
            {
                view.occurrence_overlay = None;
                view.annotation_hover = None;
                return;
            }
            tab.pending_filter_removal = None;
            tab.pending_clear_filters = false;
        }
    }

    fn open_file_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new().pick_file() {
            self.open_file(path);
        }
    }

    fn dispatch_command(&mut self, command: AppCommand) {
        match command {
            AppCommand::Open => self.open_file_dialog(),
            AppCommand::CloseTab => {
                if let Some(loader) = self.active_loader.and_then(|idx| self.loaders.get(idx)) {
                    loader
                        .cancel
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                } else if let Some(idx) = self.active {
                    self.request_close_tab(idx);
                }
            }
            AppCommand::NextTab => self.cycle_tabs(false),
            AppCommand::PreviousTab => self.cycle_tabs(true),
            AppCommand::FocusFind => {
                if let Some(tab) = self.active.and_then(|idx| self.tabs.get_mut(idx)) {
                    tab.trigger_search_focus();
                }
            }
            AppCommand::GoTo => {
                if self.active.is_some() {
                    self.show_goto_popup = true;
                    self.goto_input.clear();
                    self.goto_error = None;
                }
            }
            AppCommand::NextFind => {
                if let Some(tab) = self.active.and_then(|idx| self.tabs.get_mut(idx)) {
                    tab.find_next();
                }
            }
            AppCommand::PreviousFind => {
                if let Some(tab) = self.active.and_then(|idx| self.tabs.get_mut(idx)) {
                    tab.find_prev();
                }
            }
            AppCommand::ViewportStart
            | AppCommand::ViewportEnd
            | AppCommand::DocumentStart
            | AppCommand::DocumentEnd => {
                if let Some(tab) = self.active.and_then(|idx| self.tabs.get_mut(idx)) {
                    let last = matches!(command, AppCommand::ViewportEnd | AppCommand::DocumentEnd);
                    let target =
                        if matches!(command, AppCommand::ViewportStart | AppCommand::ViewportEnd) {
                            tab.viewport_boundary(last)
                                .or_else(|| tab.visible_boundary(last))
                        } else {
                            tab.visible_boundary(last)
                        };
                    if let Some(line) = target {
                        tab.select_and_scroll_to(line);
                    }
                }
            }
            AppCommand::ToggleLane => {
                if let Some(tab) = self.active.and_then(|idx| self.tabs.get_mut(idx)) {
                    tab.toggle_selected_lane();
                }
            }
            AppCommand::DeleteSelected => {
                if let Some(tab) = self.active.and_then(|idx| self.tabs.get_mut(idx)) {
                    if let Some(lane) = tab.selected_lane {
                        tab.pending_filter_removal = Some(lane);
                    } else if tab.selected_pin.is_some() {
                        tab.remove_selected_pin_with_undo();
                    }
                }
            }
            AppCommand::UndoDelete => {
                if let Some(tab) = self.active.and_then(|idx| self.tabs.get_mut(idx)) {
                    tab.undo_delete();
                }
            }
            AppCommand::ToggleLogFocus => {
                if let Some(tab) = self.active.and_then(|idx| self.tabs.get_mut(idx)) {
                    let focused = ViewTab::Log(tab.focused_log_view_id);
                    if !tab.is_view_detached(focused) {
                        tab.log_focus_mode = !tab.log_focus_mode;
                    }
                }
            }
            AppCommand::ShowHelp => self.show_cheat_sheet = true,
            AppCommand::ShowPalette => {
                self.show_command_palette = true;
                self.command_palette_query.clear();
            }
        }
    }

    fn submit_goto(&mut self) {
        let raw = self.goto_input.trim().to_owned();
        if raw.is_empty() {
            self.goto_error = Some("Enter a line number or timestamp.".to_string());
            return;
        }
        let line_input = raw
            .strip_prefix("line ")
            .or_else(|| raw.strip_prefix("l:"))
            .unwrap_or(&raw)
            .trim();
        let Some(tab) = self.active.and_then(|idx| self.tabs.get_mut(idx)) else {
            return;
        };
        if let Ok(line) = line_input.parse::<usize>() {
            if line == 0 {
                self.goto_error = Some("Line numbers start at 1.".to_string());
                return;
            }
            tab.select_and_scroll_to(line - 1);
            self.show_goto_popup = false;
            return;
        }
        let raw = raw.strip_prefix('@').unwrap_or(&raw);
        let Some(timestamp) = parse_time_param(raw) else {
            self.goto_error = Some("Use line 1200, RFC3339 time, or @epoch.".to_string());
            return;
        };
        let Some(line) = nearest_record_time(&tab.doc, timestamp) else {
            self.goto_error = Some("This log has no timestamps to navigate.".to_string());
            return;
        };
        tab.select_and_scroll_to(line);
        self.show_goto_popup = false;
    }

    fn command_palette_ui(&mut self, ctx: &egui::Context) {
        if !self.show_command_palette {
            return;
        }
        let mut execute = None;
        overlay::modal(
            ctx,
            "commands_modal",
            "Commands",
            egui::vec2(420.0, 420.0),
            |ui| {
                let input = ui
                    .horizontal(|ui| {
                        ui.add(icons::icon_image(
                            ui.ctx(),
                            Icon::Search,
                            16.0,
                            self.theme.text_muted,
                        ));
                        ui.add_sized(
                            [ui.available_width(), icons::ACTION_HEIGHT],
                            egui::TextEdit::singleline(&mut self.command_palette_query)
                                .id_source("command_palette_input")
                                .hint_text("Search commands…"),
                        )
                    })
                    .inner;
                input.request_focus();
                let query = self.command_palette_query.to_lowercase();
                ui.separator();
                let matches = COMMANDS.iter().filter(|spec| {
                    query.is_empty()
                        || spec.name.to_lowercase().contains(&query)
                        || spec.category.to_lowercase().contains(&query)
                });
                let mut match_count = 0;
                for spec in matches {
                    match_count += 1;
                    ui.horizontal(|ui| {
                        if ui.selectable_label(false, spec.name).clicked() {
                            execute = Some(spec.command);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if let Some(shortcut) = spec.shortcut {
                                ui.label(
                                    RichText::new(ctx.format_shortcut(&shortcut))
                                        .monospace()
                                        .small()
                                        .color(self.theme.text_muted),
                                );
                            }
                            ui.label(
                                RichText::new(spec.category)
                                    .small()
                                    .color(self.theme.text_muted),
                            );
                        });
                    });
                }
                if match_count == 0 {
                    ui.label("No commands match this search.");
                    ui.label(
                        RichText::new("Try a shorter action or category name.")
                            .small()
                            .color(self.theme.text_muted),
                    );
                }
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    execute = COMMANDS
                        .iter()
                        .find(|spec| {
                            query.is_empty()
                                || spec.name.to_lowercase().contains(&query)
                                || spec.category.to_lowercase().contains(&query)
                        })
                        .map(|spec| spec.command);
                }
            },
        );
        if let Some(command) = execute {
            self.show_command_palette = false;
            self.dispatch_command(command);
        }
    }

    fn views_dropdown_ui(&mut self, ui: &mut egui::Ui) {
        if !self.views_show_dropdown {
            return;
        }
        let Some(button_rect) = self.views_button_rect else {
            return;
        };

        let popup = overlay::popover(ui.ctx(), "views_popup", button_rect.left_bottom(), |ui| {
            ui.set_min_width(180.0);
            ui.label(RichText::new("More actions").strong().size(14.0));
            ui.separator();
            if icons::action_button(
                ui,
                Icon::History,
                "Recent files",
                self.theme.text,
                "Open a recently used file",
            )
            .clicked()
            {
                self.recent_button_rect = self.views_button_rect;
                self.recent_show_dropdown = true;
                self.views_show_dropdown = false;
            }
            if icons::action_button(
                ui,
                Icon::Filter,
                "Saved filters",
                self.theme.text,
                "Apply or save a filter set",
            )
            .clicked()
            {
                self.filter_button_rect = self.views_button_rect;
                self.show_filter_dropdown = true;
                self.views_show_dropdown = false;
            }
            if icons::action_button(
                ui,
                Icon::Commands,
                "Commands",
                self.theme.text,
                "Open the command palette (Cmd/Ctrl+Shift+P)",
            )
            .clicked()
            {
                self.show_command_palette = true;
                self.command_palette_query.clear();
                self.views_show_dropdown = false;
            }
            if let Some(tab) = self.active.and_then(|idx| self.tabs.get_mut(idx)) {
                let focused = ViewTab::Log(tab.focused_log_view_id);
                let label = if tab.log_focus_mode {
                    "Exit Focus Log"
                } else {
                    "Focus Log View"
                };
                if icons::action_button_enabled(
                    ui,
                    !tab.is_view_detached(focused),
                    Icon::Expand,
                    label,
                    self.theme.text,
                    "Hide or restore the timeline and other dock panes",
                )
                .clicked()
                {
                    tab.log_focus_mode = !tab.log_focus_mode;
                    self.views_show_dropdown = false;
                }
            }
        });
        if popup.should_close() {
            self.views_show_dropdown = false;
        }
    }

    fn ai_assistant_popup_ui(&mut self, ui: &mut egui::Ui) {
        if !self.show_ai_assistant_popup {
            return;
        }
        let Some(button_rect) = self.ai_assistant_button_rect else {
            return;
        };

        let popup = overlay::popover(
            ui.ctx(),
            "ai_assistant_popup",
            button_rect.right_bottom() - egui::vec2(330.0, 0.0),
            |ui| {
                ui.set_width(330.0);
                ui.horizontal(|ui| {
                    ui.add(icons::icon_image(
                        ui.ctx(),
                        Icon::Mcp,
                        16.0,
                        self.theme.text,
                    ));
                    ui.label(RichText::new("AI Assistant").strong().size(14.0));
                });
                ui.separator();

                let state = if self.mcp_enabled {
                    "Ready for a private GUI connection"
                } else if self.tabs.is_empty() {
                    "Open a log before starting the connection"
                } else {
                    "Connection is stopped"
                };
                ui.label(state);
                ui.label(
                    RichText::new(
                        "Connect a local coding agent to investigate the active log with you.",
                    )
                    .small()
                    .color(self.theme.text_muted),
                );
                ui.add_space(6.0);

                if self.mcp_enabled {
                    if let Some(instruction) = self.mcp_instruction() {
                        if icons::action_button(
                                ui,
                                Icon::Copy,
                                "Copy session instructions",
                                self.theme.text,
                                "Copy the temporary session ID and safe instructions for your coding agent",
                            )
                            .clicked()
                            {
                                ui.ctx().copy_text(instruction);
                                self.show_toast(
                                    "GUI session instructions copied to clipboard".to_string(),
                                );
                                self.show_ai_assistant_popup = false;
                            }
                    }
                    if icons::action_button(
                        ui,
                        Icon::Stop,
                        "Stop connection",
                        self.theme.text,
                        "Stop MCP and invalidate the temporary GUI session",
                    )
                    .clicked()
                    {
                        self.stop_mcp();
                        self.show_toast("AI Assistant connection stopped".to_string());
                        self.show_ai_assistant_popup = false;
                    }
                } else if icons::action_button_enabled(
                    ui,
                    !self.tabs.is_empty(),
                    Icon::Start,
                    "Start connection",
                    self.theme.text,
                    if self.tabs.is_empty() {
                        "Open a log file first"
                    } else {
                        "Start a private MCP connection for the active log"
                    },
                )
                .clicked()
                {
                    self.start_mcp();
                    if self.mcp_enabled {
                        self.show_toast(
                                "AI Assistant connection started. Copy the session instructions to connect."
                                    .to_string(),
                            );
                    }
                }

                ui.separator();
                if icons::action_button(
                    ui,
                    Icon::Integrate,
                    "Integration guide",
                    self.theme.text,
                    "Set up Haystack in Codex, Claude, or Cline",
                )
                .clicked()
                {
                    self.show_integrate_popup = true;
                    self.show_ai_assistant_popup = false;
                }
            },
        );
        if popup.should_close() {
            self.show_ai_assistant_popup = false;
        }
    }

    fn goto_ui(&mut self, ctx: &egui::Context) {
        if !self.show_goto_popup {
            return;
        }
        let mut submit = false;
        overlay::modal(
            ctx,
            "goto_modal",
            "Go to line or time",
            egui::vec2(420.0, 180.0),
            |ui| {
                ui.label("Line number, RFC3339 timestamp, or @epoch:");
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.goto_input)
                        .id_source("goto_input")
                        .desired_width(360.0)
                        .hint_text("1200  ·  2026-07-19T10:00:00Z  ·  @1784158530"),
                );
                response.request_focus();
                if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    submit = true;
                }
                if let Some(error) = &self.goto_error {
                    ui.label(RichText::new(error).color(self.theme.warning));
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if icons::action_button(
                        ui,
                        Icon::Close,
                        "Cancel",
                        self.theme.text,
                        "Close without navigating",
                    )
                    .clicked()
                    {
                        self.show_goto_popup = false;
                    }
                    if icons::primary_action_button(
                        ui,
                        Icon::Jump,
                        "Go",
                        self.theme.accent,
                        "Navigate to this line or time",
                    )
                    .clicked()
                    {
                        submit = true;
                    }
                });
            },
        );
        if submit {
            self.submit_goto();
        }
    }

    fn cheat_sheet_ui(&mut self, ctx: &egui::Context) {
        if !self.show_cheat_sheet {
            return;
        }
        overlay::modal(
            ctx,
            "shortcuts_modal",
            "Keyboard & Mouse Shortcuts",
            egui::vec2(520.0, 560.0),
            |ui| {
                let mut category = "";
                for spec in COMMANDS {
                    if spec.category != category {
                        category = spec.category;
                        ui.add_space(5.0);
                        ui.label(RichText::new(category).strong());
                    }
                    ui.horizontal(|ui| {
                        ui.label(spec.name);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let label = match spec.command {
                                AppCommand::GoTo => format!(
                                    "{} or {}",
                                    spec.shortcut
                                        .map(|s| ctx.format_shortcut(&s))
                                        .unwrap_or_default(),
                                    ALIASES
                                        .iter()
                                        .find(|(command, _)| *command == AppCommand::GoTo)
                                        .map(|(_, shortcut)| ctx.format_shortcut(shortcut))
                                        .unwrap_or_default()
                                ),
                                _ => spec
                                    .shortcut
                                    .map(|s| ctx.format_shortcut(&s))
                                    .unwrap_or_default(),
                            };
                            ui.label(
                                RichText::new(label)
                                    .monospace()
                                    .small()
                                    .color(self.theme.text_muted),
                            );
                        });
                    });
                }
                ui.separator();
                ui.label(RichText::new("Mouse").strong());
                for gesture in [
                    "Timeline: scroll to zoom, drag to pan, Shift-drag to brush, double-click to reset.",
                    "Timeline lanes: click a lane or occurrence to select it; click eye to toggle; click trash to remove.",
                    "Log: click a row to select; Shift-click for surrounding filter occurrences; right-click to pin; drag across rows to select a range.",
                    "Pins: click a card title to select it; use Remove or Delete to remove it.",
                ] { ui.label(RichText::new(gesture).small().color(self.theme.text_muted)); }
            },
        );
    }

    fn close_save_error_ui(&mut self, ctx: &egui::Context) {
        let Some(idx) = self.close_save_error else {
            return;
        };
        let waiting_for_restore = self
            .tabs
            .get(idx)
            .is_some_and(|tab| tab.pending_sidecar_restore.is_some());
        let mut retry = false;
        let mut discard = false;
        overlay::modal(
            ctx,
            "save_error_modal",
            "Could not save investigation state",
            egui::vec2(520.0, 220.0),
            |ui| {
                ui.label(if waiting_for_restore {
                    "This tab is waiting for a restored-investigation decision."
                } else {
                    "Haystack could not save this tab's filters, notes, and view state."
                });
                ui.label(
                    RichText::new(if waiting_for_restore {
                        "Keep the tab open to complete that decision, or discard the pending state and close."
                    } else {
                        "Keep the tab open, retry saving, or discard those unsaved changes."
                    })
                    .small()
                    .color(self.theme.text_muted),
                );
                ui.separator();
                ui.horizontal(|ui| {
                    if icons::action_button(
                        ui,
                        Icon::Close,
                        "Cancel",
                        self.theme.text,
                        "Keep the tab open",
                    )
                    .clicked()
                    {
                        self.close_save_error = None;
                    }
                    if !waiting_for_restore
                        && icons::primary_action_button(
                            ui,
                            Icon::Reset,
                            "Retry save",
                            self.theme.accent,
                            "Try saving the investigation state again",
                        )
                        .clicked()
                    {
                        retry = true;
                    }
                    if icons::action_button(
                        ui,
                        Icon::Remove,
                        "Discard and close",
                        self.theme.warning,
                        "Close this tab without saving its investigation state",
                    )
                    .clicked()
                    {
                        discard = true;
                    }
                });
            },
        );
        if retry {
            self.close_save_error = None;
            self.request_close_tab(idx);
        }
        if discard {
            self.close_save_error = None;
            self.close_tab(idx);
        }
    }

    fn sidecar_recovery_ui(&mut self, ctx: &egui::Context) {
        let Some(recovery) = self.pending_sidecar_recovery.as_ref() else {
            return;
        };
        let path = recovery.path.clone();
        let detail = recovery.detail.clone();
        let repaired_sidecar = recovery.repaired_sidecar.clone();
        let replace_after_load = recovery.replace_after_load;
        let is_profile = repaired_sidecar.is_some();
        let mut continue_load = false;
        let mut cancel = false;
        overlay::modal(
            ctx,
            "sidecar_recovery_modal",
            "Saved log settings need attention",
            egui::vec2(620.0, 260.0),
            |ui| {
                ui.label(format!(
                    "{} has saved settings this version cannot use.",
                    path.file_name().map_or_else(
                        || path.display().to_string(),
                        |name| name.to_string_lossy().into_owned()
                    )
                ));
                ui.add_space(4.0);
                ui.label(if is_profile {
                    "Continue loading with automatic format detection? The invalid saved format will be removed; filters, notes, and layout remain."
                } else if replace_after_load {
                    "Continue loading with a fresh investigation? The invalid saved state will be replaced only after this log opens successfully."
                } else {
                    "Continue loading without saved settings? The saved state will not be changed automatically."
                });
                ui.collapsing("Details", |ui| {
                    ui.monospace(format!(
                        "Sidecar: {}\n{detail}",
                        haystack::core::sidecar::path_for(&path).display()
                    ));
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    let label = if replace_after_load {
                        "Continue & repair"
                    } else {
                        "Continue without saved settings"
                    };
                    if ui.button(label).clicked() {
                        continue_load = true;
                    }
                });
            },
        );
        if cancel {
            self.pending_sidecar_recovery = None;
            self.status = "Opening log cancelled; saved settings were not changed.".into();
        } else if continue_load {
            self.pending_sidecar_recovery = None;
            self.recovery_sidecars
                .insert(path.clone(), (repaired_sidecar, replace_after_load));
            self.open_file(path);
        }
    }
}

/// Owns the mutable state for a single file tab and renders the UI for it.
struct TabViewer<'a> {
    tab: &'a mut LogTab,
    theme: &'a Theme,
    allow_popout: bool,
    /// Detached Log Views keep their return action in the tab header so it
    /// does not consume a second row above the reading surface.
    return_to_main: Option<&'a mut bool>,
    dock_container: DockContainer,
    drop_targets: &'a mut Vec<DockTargetVisual>,
}

#[derive(Clone, Copy)]
struct DockTargetVisual {
    target: DockDropTarget,
    hit_rect: egui::Rect,
    preview_rect: egui::Rect,
    label: &'static str,
}

impl<'a> egui_dock::TabViewer for TabViewer<'a> {
    type Tab = ViewTab;

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Self::Tab) {
        match tab {
            ViewTab::Timeline => timeline::show(ui, self.tab, self.theme),
            ViewTab::Log(id) => {
                let interacted = log_view_pointer_interaction(ui);
                show_log_view(ui, self.tab, self.theme, *id, interacted);
            }
            ViewTab::Pinned => pin_viewer::show(ui, self.tab, self.theme),
            ViewTab::Templates => template_view::show(ui, self.tab, self.theme),
        }
    }

    fn title(&mut self, tab: &mut Self::Tab) -> egui::WidgetText {
        let display_number = match tab {
            ViewTab::Log(id) => self.tab.log_views.get(id).map(|view| view.display_number),
            _ => None,
        };
        dock_tab_title(*tab, self.tab.log_view_count(), display_number).into()
    }

    fn id(&mut self, tab: &mut Self::Tab) -> egui::Id {
        egui::Id::new(("workspace_view", *tab))
    }

    fn is_closeable(&self, tab: &Self::Tab) -> bool {
        matches!(tab, ViewTab::Log(_)) && self.tab.log_view_count() > 1
    }

    fn on_close(&mut self, tab: &mut Self::Tab) -> egui_dock::tab_viewer::OnCloseResponse {
        match tab {
            ViewTab::Log(id) if self.tab.close_log_view(*id) => {
                egui_dock::tab_viewer::OnCloseResponse::Close
            }
            _ => egui_dock::tab_viewer::OnCloseResponse::Ignore,
        }
    }

    fn on_tab_button(&mut self, tab: &mut Self::Tab, response: &egui::Response) {
        let accessible_label = dock_tab_accessibility_label(
            *tab,
            self.tab.log_view_count(),
            match tab {
                ViewTab::Log(id) => self.tab.log_views.get(id).map(|view| view.display_number),
                _ => None,
            },
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                response.enabled(),
                accessible_label.clone(),
            )
        });
        if !matches!(tab, ViewTab::Timeline) {
            // `egui_dock` replaces the original tab response with a drag
            // proxy once its internal drag threshold is crossed. Arm our
            // cross-window bridge while the original tab owns the press,
            // before that proxy exists.
            if response.is_pointer_button_down_on() {
                let replace = self
                    .tab
                    .active_dock_drag
                    .as_ref()
                    .is_none_or(|drag| drag.tab != *tab || drag.source != self.dock_container);
                if replace {
                    self.tab.active_dock_drag =
                        Some(DockDragSession::new(*tab, self.dock_container));
                }
                // A native child viewport owns the pointer while the tab is
                // dragged. Repaint the main window so its cross-window drop
                // marker appears as soon as the pointer enters it.
                response.ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
            if response.clicked() {
                if let ViewTab::Log(id) = tab {
                    self.tab.focus_log_view(*id);
                }
                self.tab.active_dock_drag = None;
            }
            if self
                .tab
                .active_dock_drag
                .as_ref()
                .is_some_and(|drag| drag.active && drag.tab != *tab)
            {
                let midpoint = response.rect.center().x;
                for (after, hit_rect, x) in [
                    (
                        false,
                        egui::Rect::from_min_max(
                            response.rect.min,
                            egui::pos2(midpoint, response.rect.bottom()),
                        ),
                        response.rect.left(),
                    ),
                    (
                        true,
                        egui::Rect::from_min_max(
                            egui::pos2(midpoint, response.rect.top()),
                            response.rect.max,
                        ),
                        response.rect.right(),
                    ),
                ] {
                    self.drop_targets.push(DockTargetVisual {
                        target: DockDropTarget::TabInsert {
                            container: self.dock_container,
                            sibling: *tab,
                            after,
                        },
                        hit_rect,
                        preview_rect: egui::Rect::from_min_max(
                            egui::pos2(x - 2.0, response.rect.top()),
                            egui::pos2(x + 2.0, response.rect.bottom()),
                        ),
                        label: "Insert tab",
                    });
                }
            }
        }
        let action = if self.allow_popout {
            dock_tab_action_rects(*tab, response.rect, self.is_closeable(tab))
                .map(|rect| (rect, Icon::ExternalWindow))
        } else if self.return_to_main.is_some() && !matches!(tab, ViewTab::Timeline) {
            dock_tab_action_rects(*tab, response.rect, self.is_closeable(tab))
                .map(|rect| (rect, Icon::ArrowLeft))
        } else {
            None
        };
        let Some((action_rect, action_icon)) = action else {
            if response.hovered() {
                response.clone().on_hover_text("Switch workspace view");
            }
            return;
        };
        let pointer = response.ctx.pointer_hover_pos();
        let action_hovered = pointer.is_some_and(|p| action_rect.contains(p));
        let action_color = if action_hovered {
            self.theme.text
        } else {
            self.theme.text_muted
        };
        let painter = response.ctx.layer_painter(response.layer_id);
        icons::paint_icon(
            &response.ctx,
            &painter,
            action_icon,
            action_rect.center(),
            14.0,
            action_color,
        );
        if action_hovered {
            response.ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            let tooltip = if self.allow_popout {
                format!("Open {} in a separate window", dock_tab_name(*tab))
            } else {
                "Return this detached dock to the main window".to_owned()
            };
            response.clone().on_hover_text(tooltip);
            if response.clicked()
                && response
                    .interact_pointer_pos()
                    .is_some_and(|pointer| action_rect.contains(pointer))
            {
                if self.allow_popout {
                    self.tab.pending_detach = Some(*tab);
                } else if let Some(return_to_main) = self.return_to_main.as_deref_mut() {
                    *return_to_main = true;
                }
            }
        } else if response.hovered() {
            let tooltip = match tab {
                ViewTab::Log(_) if self.tab.log_view_count() == 1 => {
                    "Switch to this Log View. The final Log View stays open; use + to create another."
                }
                ViewTab::Log(_) => {
                    "Switch to this Log View. Timeline and navigation follow the focused Log View."
                }
                _ => "Switch workspace view",
            };
            response.clone().on_hover_text(tooltip);
        }
    }

    fn on_add(&mut self, path: egui_dock::NodePath) {
        self.tab.pending_add_log_view = Some(path);
    }
}

fn log_view_pointer_interaction(ui: &egui::Ui) -> bool {
    ui.rect_contains_pointer(ui.max_rect())
        && ui.input(|input| {
            input.pointer.any_pressed()
                || input.pointer.any_down()
                || input.smooth_scroll_delta != egui::Vec2::ZERO
        })
}

/// Draw the explicit, cross-native-window Log View drop marker. `egui_dock`
/// owns drag feedback inside one dock state; this bridge makes a dragged Log
/// tab visible and droppable in another native viewport as well.
fn show_cross_window_drop_targets(
    ui: &egui::Ui,
    tab: &mut LogTab,
    theme: &Theme,
    target_container: DockContainer,
    targets: &[DockTargetVisual],
) -> bool {
    let Some(drag) = tab.active_dock_drag.as_mut() else {
        return false;
    };
    let dragged_tab = drag.tab;
    if matches!(dragged_tab, ViewTab::Log(id) if !tab.log_views.contains_key(&id)) {
        tab.active_dock_drag = None;
        return false;
    }

    let viewport_origin = ui
        .ctx()
        .input(|input| input.viewport().inner_rect.map(|r| r.min));
    if let Some(origin) = viewport_origin {
        if let Some(pointer) = ui
            .ctx()
            .pointer_interact_pos()
            .or_else(|| ui.ctx().pointer_hover_pos())
        {
            drag.update_pointer(origin + pointer.to_vec2());
        }
        if drag.active {
            for visual in targets {
                if LogTab::dock_target_accepts(dragged_tab, visual.target)
                    && !dock_target_points_to_tab(visual.target, dragged_tab)
                {
                    drag.register_target(
                        visual.target,
                        visual.hit_rect.translate(origin.to_vec2()),
                    );
                }
            }
        }
    }

    let local_target = if drag.active && viewport_origin.is_none() {
        ui.ctx().pointer_hover_pos().and_then(|pointer| {
            targets
                .iter()
                .filter(|visual| {
                    LogTab::dock_target_accepts(dragged_tab, visual.target)
                        && !dock_target_points_to_tab(visual.target, dragged_tab)
                        && visual.hit_rect.contains(pointer)
                })
                .max_by_key(|visual| match visual.target {
                    DockDropTarget::TabInsert { .. } => 3,
                    DockDropTarget::SplitDetached { .. } => 2,
                    _ => 1,
                })
                .map(|visual| visual.target)
        })
    } else {
        None
    };
    let active_target = drag.hovered_target().or(local_target);
    let pointer_screen = drag.pointer_screen;
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new(("cross_window_log_drop", target_container)),
    ));
    if drag.active && drag.source == target_container {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        if let Some(pointer) = ui
            .ctx()
            .pointer_interact_pos()
            .or_else(|| ui.ctx().pointer_hover_pos())
        {
            let ghost = egui::Rect::from_min_size(
                pointer + egui::vec2(14.0, 14.0),
                egui::vec2(150.0, 30.0),
            );
            painter.rect_filled(ghost, 5.0, theme.raised_surface);
            painter.rect_stroke(
                ghost,
                5.0,
                Stroke::new(1.5, theme.selection_focused),
                egui::StrokeKind::Inside,
            );
            painter.text(
                ghost.left_center() + egui::vec2(10.0, 0.0),
                egui::Align2::LEFT_CENTER,
                if active_target.is_some() {
                    format!("Dock {}", dock_tab_name(dragged_tab))
                } else {
                    format!("New window · {}", dock_tab_name(dragged_tab))
                },
                egui::FontId::proportional(13.0),
                theme.text,
            );
        }
    }
    if drag.active {
        for visual in targets.iter().filter(|visual| {
            LogTab::dock_target_accepts(dragged_tab, visual.target)
                && !dock_target_points_to_tab(visual.target, dragged_tab)
        }) {
            let hovered = active_target == Some(visual.target);
            if hovered {
                painter.rect_filled(
                    visual.preview_rect,
                    5.0,
                    theme.selection_focused.gamma_multiply(0.16),
                );
            }
            painter.rect_stroke(
                visual.preview_rect,
                5.0,
                Stroke::new(if hovered { 2.0 } else { 1.0 }, theme.selection_focused),
                egui::StrokeKind::Inside,
            );
            if hovered && !matches!(visual.target, DockDropTarget::TabInsert { .. }) {
                painter.text(
                    visual.preview_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    visual.label,
                    egui::FontId::proportional(14.0),
                    theme.selection_focused,
                );
            }
        }
        if active_target.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
    }
    if drag.active && active_target.is_none() {
        let preview_size = egui::vec2(520.0, 360.0);
        let preview_screen = pointer_screen.map(|pointer| {
            egui::Rect::from_min_size(pointer - egui::vec2(80.0, 18.0), preview_size)
        });
        let preview_local = match (preview_screen, viewport_origin) {
            (Some(rect), Some(origin)) => Some(rect.translate(-origin.to_vec2())),
            (None, None) => ui.ctx().pointer_hover_pos().map(|pointer| {
                egui::Rect::from_min_size(pointer - egui::vec2(80.0, 18.0), preview_size)
            }),
            _ => None,
        };
        if let Some(preview) = preview_local {
            painter.rect_filled(preview, 7.0, theme.canvas.gamma_multiply(0.82));
            painter.rect_stroke(
                preview,
                7.0,
                Stroke::new(2.0, theme.selection_focused),
                egui::StrokeKind::Inside,
            );
            let title_bar = egui::Rect::from_min_max(
                preview.min,
                egui::pos2(preview.right(), preview.top() + 30.0),
            );
            painter.rect_filled(title_bar, 7.0, theme.raised_surface);
            painter.text(
                title_bar.left_center() + egui::vec2(10.0, 0.0),
                egui::Align2::LEFT_CENTER,
                format!("New window · {}", dock_tab_name(dragged_tab)),
                egui::FontId::proportional(13.0),
                theme.text,
            );
        }
    }
    let released = ui.input(|input| input.pointer.primary_released());
    if released {
        let destination = tab
            .active_dock_drag
            .as_ref()
            .and_then(DockDragSession::hovered_target)
            .or(local_target);
        let moved = if let Some(destination) = destination {
            tab.dock_view(dragged_tab, destination)
        } else if tab
            .active_dock_drag
            .as_ref()
            .is_some_and(|drag| drag.active)
        {
            let position = pointer_screen.map(|pointer| pointer - egui::vec2(80.0, 18.0));
            tab.detach_view_to_window(dragged_tab, position).is_some()
        } else {
            false
        };
        // Release always terminates the cross-window gesture, including when
        // it occurs outside every legal target.
        tab.active_dock_drag = None;
        if moved {
            ui.ctx().request_repaint();
        }
        return moved;
    }

    if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
        tab.active_dock_drag = None;
    }
    false
}

fn dock_target_points_to_tab(target: DockDropTarget, tab: ViewTab) -> bool {
    matches!(
        target,
        DockDropTarget::JoinDetached { sibling, .. }
            | DockDropTarget::SplitDetached { sibling, .. }
            | DockDropTarget::TabInsert { sibling, .. }
            if sibling == tab
    )
}

fn append_detached_leaf_drop_targets(
    targets: &mut Vec<DockTargetVisual>,
    state: &DockState<ViewTab>,
    window: DockWindowId,
    dragged_tab: Option<ViewTab>,
) {
    for (_leaf, node) in state.iter_leaves() {
        let Some(sibling) = node
            .tabs()
            .iter()
            .copied()
            .find(|candidate| Some(*candidate) != dragged_tab)
        else {
            continue;
        };
        let rect = node.rect().shrink(6.0);
        if rect.width() < 40.0 || rect.height() < 40.0 {
            continue;
        }
        let x1 = rect.left() + rect.width() * 0.25;
        let x2 = rect.right() - rect.width() * 0.25;
        let y1 = rect.top() + rect.height() * 0.25;
        let y2 = rect.bottom() - rect.height() * 0.25;
        let center = egui::Rect::from_min_max(egui::pos2(x1, y1), egui::pos2(x2, y2));
        targets.push(DockTargetVisual {
            target: DockDropTarget::JoinDetached { window, sibling },
            hit_rect: center,
            preview_rect: rect,
            label: "Join tabs",
        });
        for (split, hit_rect, preview_rect, label) in [
            (
                egui_dock::Split::Left,
                egui::Rect::from_min_max(rect.min, egui::pos2(x1, rect.bottom())),
                egui::Rect::from_min_max(rect.min, egui::pos2(rect.center().x, rect.bottom())),
                "Dock left",
            ),
            (
                egui_dock::Split::Right,
                egui::Rect::from_min_max(egui::pos2(x2, rect.top()), rect.max),
                egui::Rect::from_min_max(egui::pos2(rect.center().x, rect.top()), rect.max),
                "Dock right",
            ),
            (
                egui_dock::Split::Above,
                egui::Rect::from_min_max(egui::pos2(x1, rect.top()), egui::pos2(x2, y1)),
                egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), rect.center().y)),
                "Dock above",
            ),
            (
                egui_dock::Split::Below,
                egui::Rect::from_min_max(egui::pos2(x1, y2), egui::pos2(x2, rect.bottom())),
                egui::Rect::from_min_max(egui::pos2(rect.left(), rect.center().y), rect.max),
                "Dock below",
            ),
        ] {
            targets.push(DockTargetVisual {
                target: DockDropTarget::SplitDetached {
                    window,
                    sibling,
                    split,
                },
                hit_rect,
                preview_rect,
                label,
            });
        }
    }
}

/// Render through the temporary compatibility bridge without allowing paint
/// order to redefine the most recently focused Log View.
fn show_log_view(
    ui: &mut egui::Ui,
    tab: &mut LogTab,
    theme: &Theme,
    id: LogViewId,
    received_focus: bool,
) {
    if !tab.log_views.contains_key(&id) {
        return;
    }
    let previous = tab.focused_log_view_id;
    tab.focused_log_view_id = id;
    log_view::show(ui, tab, theme);
    tab.focused_log_view_id = previous;
    if received_focus {
        tab.focus_log_view(id);
    }
}

fn detached_viewport_id(path: &std::ffi::OsStr, view_tab: ViewTab) -> egui::ViewportId {
    egui::ViewportId::from_hash_of((path, view_tab))
}

fn dock_window_viewport_id(path: &std::ffi::OsStr, window: DockWindowId) -> egui::ViewportId {
    egui::ViewportId::from_hash_of((path, "dock_window", window))
}

fn dock_tab_name(tab: ViewTab) -> &'static str {
    match tab {
        ViewTab::Timeline => "Timeline",
        ViewTab::Log(_) => "Log",
        ViewTab::Pinned => "Pinned",
        ViewTab::Templates => "Templates",
    }
}

fn detached_view_title(
    file_name: &str,
    view_tab: ViewTab,
    log_view_count: usize,
    display_number: Option<u64>,
) -> String {
    let identity = match view_tab {
        ViewTab::Timeline => "Timeline".to_owned(),
        ViewTab::Log(_) if log_view_count > 1 => {
            format!("Log View {}", display_number.unwrap_or(1))
        }
        ViewTab::Log(_) => "Log View".to_owned(),
        ViewTab::Pinned => "Pinned".to_owned(),
        ViewTab::Templates => "Templates".to_owned(),
    };
    format!("{file_name} · {identity}")
}

fn dock_tab_title(tab: ViewTab, log_view_count: usize, display_number: Option<u64>) -> String {
    let name = dock_tab_name(tab);
    if matches!(tab, ViewTab::Log(_)) {
        let label = if log_view_count > 1 {
            format!("{name} {}", display_number.unwrap_or(1))
        } else {
            name.to_owned()
        };
        format!("{label}             ")
    } else if matches!(tab, ViewTab::Pinned | ViewTab::Templates) {
        format!("{name}     ")
    } else {
        name.to_owned()
    }
}

fn dock_tab_accessibility_label(
    tab: ViewTab,
    log_view_count: usize,
    display_number: Option<u64>,
) -> String {
    let title = dock_tab_title(tab, log_view_count, display_number)
        .trim()
        .to_owned();
    match tab {
        ViewTab::Log(_) if log_view_count == 1 => format!(
            "{title} tab. Add another Log View or open this view in a separate window. The final Log View cannot be closed."
        ),
        ViewTab::Log(_) => format!(
            "{title} tab. Add another Log View, open this view in a separate window, or close this Log View."
        ),
        ViewTab::Pinned | ViewTab::Templates => {
            format!("{title} tab. Open this view in a separate window.")
        }
        ViewTab::Timeline => title,
    }
}

fn dock_tab_action_rects(
    tab: ViewTab,
    tab_rect: egui::Rect,
    closeable: bool,
) -> Option<egui::Rect> {
    matches!(tab, ViewTab::Log(_) | ViewTab::Pinned | ViewTab::Templates).then(|| {
        let size = egui::vec2(22.0, tab_rect.height().min(22.0));
        let close_space = if closeable { size.x } else { 0.0 };
        let popout = egui::Rect::from_center_size(
            egui::pos2(
                tab_rect.right() - close_space - size.x * 0.65,
                tab_rect.center().y,
            ),
            size,
        );
        popout
    })
}

impl eframe::App for HaystackApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let is_main_viewport = ui.ctx().input(|i| i.viewport().parent.is_none());
        let viewport_id = ui.ctx().viewport_id();

        if is_main_viewport {
            self.update_main(ui);
        } else {
            self.update_detached(viewport_id, ui);
        }
    }
}

impl HaystackApp {
    fn apply_tracked_screenshot_view(&mut self, ctx: &egui::Context) {
        if std::env::var("HAYSTACK_SCREENSHOT_PATH").is_err() {
            return;
        }
        let applied_id = egui::Id::new("tracked_screenshot_view_applied");
        if ctx.data(|data| data.get_temp::<bool>(applied_id).unwrap_or(false)) {
            return;
        }
        let screenshot_view = std::env::var("HAYSTACK_SCREENSHOT_VIEW").ok();
        let desired = match screenshot_view.as_deref() {
            Some("pinned") => Some(ViewTab::Pinned),
            Some("templates") => Some(ViewTab::Templates),
            Some("log") => Some(ViewTab::Log(LogViewId::INITIAL)),
            _ => None,
        };
        let Some(tab) = self.active.and_then(|index| self.tabs.get_mut(index)) else {
            return;
        };
        if let Some(desired) = desired {
            if let Some(path) = tab.dock_state.find_tab(&desired) {
                let _ = tab.dock_state.set_active_tab(path);
            }
        }
        if screenshot_view.as_deref() == Some("multiple-logs") && tab.log_view_count() == 1 {
            let source = ViewTab::Log(tab.focused_log_view_id);
            if let Some(source_path) = tab.dock_state.find_tab(&source) {
                tab.dock_state
                    .set_focused_node_and_surface(source_path.node_path());
                let second = tab.add_log_view();
                tab.dock_state.push_to_focused_leaf(ViewTab::Log(second));
                if let Some(second_path) = tab.dock_state.find_tab(&ViewTab::Log(second)) {
                    let _ = tab.dock_state.set_active_tab(second_path);
                }
            }
        }
        tab.bottom_panel_open = true;
        ctx.data_mut(|data| data.insert_temp(applied_id, true));
    }

    fn update_main(&mut self, ui: &mut egui::Ui) {
        consume_tracked_screenshot(ui.ctx());
        if std::env::var("HAYSTACK_SCREENSHOT_PATH").is_ok() {
            ui.ctx()
                .all_styles_mut(|style| style.interaction.tooltip_delay = f32::INFINITY);
        }
        self.dark_mode = crate::ui::theme::apply_egui_theme(ui.ctx(), self.settings.theme_mode);

        let interactive_surface_open = overlay::interactive_surface_open(self, ui.ctx());
        overlay::mark_background_input(ui.ctx(), interactive_surface_open);

        self.poll_open_requests(ui.ctx());

        let dropped = ui.ctx().input(|i| i.raw.dropped_files.clone());
        for f in dropped {
            if let Some(path) = f.path {
                self.open_file(path);
            }
        }
        let hovering_files = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());

        let loaders_changed = self.poll_loaders();
        self.poll_reparse();
        self.apply_tracked_screenshot_view(ui.ctx());
        self.poll_mcp_dirty();
        self.poll_mcp_filters();
        self.poll_mcp_analyses();
        self.poll_tail_updates();
        self.poll_file_updates();
        // Push any GUI-originated doc mutations (trim/append) into MCP state.
        self.sync_mcp_active_doc();
        // Push any GUI-originated filter-set changes into MCP state.
        self.sync_mcp_filters();
        self.sync_mcp_analyses();
        let mut any_search = false;
        for tab in &mut self.tabs {
            if tab.poll_search() {
                any_search = true;
            }
            if tab.poll_visible_lines() {
                any_search = true;
            }
            if tab.poll_log_view_workers() {
                any_search = true;
            }
        }
        self.poll_mcp_search();
        self.drain_recent_query_updates();
        if should_request_repaint(
            any_search,
            loaders_changed,
            !self.loaders.is_empty(),
            self.mcp_enabled,
        ) {
            ui.ctx().request_repaint_after(Duration::from_millis(60));
        }

        // All application commands are consumed in one place so detached
        // views, the palette, and the cheat sheet cannot drift apart.
        if !interactive_surface_open {
            let text_edit_active = ui.ctx().memory(|memory| memory.focused().is_some());
            let commands = key_listener::consume(ui.ctx(), text_edit_active);
            for command in commands {
                self.dispatch_command(command);
            }
        }
        self.dismiss_app_overlay_on_escape(ui.ctx());

        if let Some(active_tab_idx) = self.active {
            if let Some(tab) = self.tabs.get_mut(active_tab_idx) {
                if let Some(view_to_detach) = tab.pending_detach.take() {
                    if view_to_detach == ViewTab::Timeline {
                        // Timeline lives in a fixed top panel (not the dock),
                        // so detaching just hides the panel and opens a window.
                        tab.timeline_detached = true;
                    } else {
                        tab.detach_dock_view(view_to_detach);
                    }
                }
                for closed_tab in std::mem::take(&mut tab.just_closed_viewports) {
                    tab.redock_view(closed_tab);
                }
            }
        }

        let mut close_request: Option<usize> = None;
        egui::Panel::top("top_panel").show(ui, |ui| {
            let compact = compact_top_bar(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().interact_size.y = icons::ACTION_HEIGHT;
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.add(icons::app_mark(ui.ctx(), 16.0, self.theme.text_muted));
                if !compact {
                    ui.label(RichText::new("Haystack").strong().size(16.0));
                }
                ui.separator();
                if icons::action_button(
                    ui,
                    Icon::OpenFile,
                    "Open file",
                    self.theme.text,
                    "Open a log or ZIP file (Cmd/Ctrl+O)",
                )
                .clicked()
                {
                    self.show_format_dropdown = false;
                    self.recent_show_dropdown = false;
                    self.show_filter_dropdown = false;
                    self.views_show_dropdown = false;
                    self.show_ai_assistant_popup = false;
                    self.open_file_dialog();
                }
                if compact {
                    let more = icons::action_button(
                        ui,
                        Icon::More,
                        "More",
                        self.theme.text,
                        "Recent files, saved filters, and commands",
                    );
                    if more.clicked() {
                        self.show_format_dropdown = false;
                        (
                            self.recent_show_dropdown,
                            self.show_filter_dropdown,
                            self.views_show_dropdown,
                        ) = dropdown_visibility(TopPanelDropdown::More, self.views_show_dropdown);
                        self.show_ai_assistant_popup = false;
                        self.show_settings_popup = false;
                    }
                    self.views_button_rect = Some(more.rect);
                } else {
                    let recent = icons::action_button(
                        ui,
                        Icon::History,
                        "Recent",
                        self.theme.text,
                        "Open a recently used file",
                    );
                    if recent.clicked() {
                        self.show_format_dropdown = false;
                        (
                            self.recent_show_dropdown,
                            self.show_filter_dropdown,
                            self.views_show_dropdown,
                        ) = dropdown_visibility(
                            TopPanelDropdown::Recent,
                            self.recent_show_dropdown,
                        );
                        self.show_ai_assistant_popup = false;
                        self.show_settings_popup = false;
                    }
                    self.recent_button_rect = Some(recent.rect);

                    let filters = icons::action_button(
                        ui,
                        Icon::Filter,
                        "Saved filters",
                        self.theme.text,
                        "Apply or save a filter set",
                    );
                    if filters.clicked() {
                        self.show_format_dropdown = false;
                        (
                            self.recent_show_dropdown,
                            self.show_filter_dropdown,
                            self.views_show_dropdown,
                        ) = dropdown_visibility(
                            TopPanelDropdown::Filters,
                            self.show_filter_dropdown,
                        );
                        self.show_ai_assistant_popup = false;
                        self.show_settings_popup = false;
                    }
                    self.filter_button_rect = Some(filters.rect);

                    if icons::action_button(
                        ui,
                        Icon::Commands,
                        "Commands",
                        self.theme.text,
                        "Open the command palette (Cmd/Ctrl+Shift+P)",
                    )
                    .clicked()
                    {
                        self.show_format_dropdown = false;
                        self.show_command_palette = true;
                        self.command_palette_query.clear();
                        self.recent_show_dropdown = false;
                        self.show_filter_dropdown = false;
                        self.views_show_dropdown = false;
                        self.show_ai_assistant_popup = false;
                        self.show_settings_popup = false;
                    }
                }

                // Keep the document controls visually separate from the file
                // actions. The status itself is rendered as a bounded chip
                // below so long format/file details cannot push controls out
                // of the top bar.
                ui.add_space(4.0);
                ui.separator();

                let format_tip = self
                    .active
                    .and_then(|index| self.tabs.get(index))
                    .map(|tab| {
                        tab.doc.record_profile().map_or_else(
                            || format!("Auto-detect · {}", tab.doc.format_name()),
                            |profile| profile.name.clone(),
                        )
                    })
                    .unwrap_or_else(|| "Choose or create a log format".into());
                let format_warning = self
                    .active
                    .and_then(|index| self.tabs.get(index))
                    .and_then(|tab| tab.doc.record_profile_match_rate())
                    .is_some_and(|(matched, total)| total > 0 && matched * 100 < total * 90);
                let format_label = if format_warning { "⚠ Format" } else { "Format" };
                let format_color = if format_warning { self.theme.warning } else { self.theme.text };
                let format_tip = if format_warning {
                    format!("{format_tip}\nWarning: the applied format matches fewer than 90% of log lines. Check and update Format.")
                } else { format_tip };
                let format =
                    icons::action_button(ui, Icon::Log, format_label, format_color, &format_tip);
                if format.clicked() {
                    self.show_format_dropdown = !self.show_format_dropdown;
                    self.recent_show_dropdown = false;
                    self.show_filter_dropdown = false;
                    self.views_show_dropdown = false;
                    self.show_ai_assistant_popup = false;
                    self.show_settings_popup = false;
                }
                self.format_button_rect = Some(format.rect);

                // Status and format details describe a document. Do not let
                // them survive into the empty workspace.
                let format_status = self.selected_log_format_status();
                let context = document_status_context(format_status.as_deref(), &self.status);
                let status_width = if compact { 160.0 } else { 300.0 };
                let status_response = egui::Frame::new()
                    .fill(self.theme.log_surface)
                    .stroke(Stroke::new(1.0, self.theme.border))
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::symmetric(6, 1))
                    .show(ui, |ui| {
                        ui.add_sized(
                            egui::vec2(status_width - 12.0, icons::ACTION_HEIGHT - 2.0),
                            egui::Label::new(
                                RichText::new(if context.is_empty() {
                                    "No file open"
                                } else {
                                    context.as_str()
                                })
                                .small()
                                .color(self.theme.text_muted),
                            )
                            .truncate(),
                        )
                    })
                    .response;
                if !context.is_empty() {
                    status_response.on_hover_text(&context);
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(4.0);
                    ui.separator();
                    let settings_resp = icons::action_button(
                        ui,
                        Icon::Settings,
                        if compact { "" } else { "Settings" },
                        self.theme.text_muted,
                        "Open settings",
                    );
                    if settings_resp.clicked() {
                        self.show_format_dropdown = false;
                        self.show_settings_popup = !self.show_settings_popup;
                        self.recent_show_dropdown = false;
                        self.show_filter_dropdown = false;
                        self.views_show_dropdown = false;
                        self.show_ai_assistant_popup = false;
                    }
                    self.settings_button_rect = Some(settings_resp.rect);

                    let ai_label = if compact {
                        ""
                    } else if self.mcp_enabled {
                        "AI ready"
                    } else {
                        "AI Assistant"
                    };
                    let ai = icons::action_button(
                        ui,
                        Icon::Mcp,
                        ai_label,
                        if self.mcp_enabled {
                            self.theme.accent
                        } else {
                            self.theme.text_muted
                        },
                        if self.mcp_enabled {
                            "AI Assistant connection is ready"
                        } else {
                            "Connect an AI coding assistant to the active log"
                        },
                    );
                    if ai.clicked() {
                        self.show_format_dropdown = false;
                        self.show_ai_assistant_popup = !self.show_ai_assistant_popup;
                        self.recent_show_dropdown = false;
                        self.show_filter_dropdown = false;
                        self.views_show_dropdown = false;
                        self.show_settings_popup = false;
                    }
                    self.ai_assistant_button_rect = Some(ai.rect);
                });
            });

            if !self.tabs.is_empty() || !self.loaders.is_empty() {
                ui.add_space(2.0);
                let mut tab_switch: Option<(Option<usize>, usize)> = None;
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for i in 0..self.tabs.len() {
                        let file_name = self.tabs[i].doc.file_name.clone();
                        let mcp_serving = self.tabs[i].mcp_serving;
                        let is_active = self.active == Some(i) && self.active_loader.is_none();
                        let tab_frame = egui::Frame::new()
                            .fill(if is_active {
                                self.theme.raised_surface
                            } else {
                                self.theme.log_surface
                            })
                            .stroke(Stroke::new(
                                1.0,
                                if is_active {
                                    self.theme.border
                                } else {
                                    self.theme.log_surface
                                },
                            ))
                            .corner_radius(4.0)
                            .inner_margin(egui::Margin::symmetric(5, 1))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 4.0;
                                    ui.add(icons::icon_image(
                                        ui.ctx(),
                                        Icon::Log,
                                        14.0,
                                        self.theme.text_muted,
                                    ));
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new(&file_name).color(self.theme.text),
                                            )
                                            .frame_when_inactive(false)
                                            .min_size(egui::Vec2::ZERO),
                                        )
                                        .on_hover_text(format!("Switch to {file_name}"))
                                        .clicked()
                                    {
                                        let old_active = self.active;
                                        info!("switched to tab {} ({})", i, file_name);
                                        self.active = Some(i);
                                        self.active_loader = None;
                                        if old_active != Some(i) {
                                            tab_switch = Some((old_active, i));
                                        }
                                    }
                                    if mcp_serving {
                                        ui.label(
                                            RichText::new("AI")
                                                .small()
                                                .strong()
                                                .color(self.theme.accent),
                                        )
                                        .on_hover_text(
                                            "This log is available to the connected AI assistant",
                                        );
                                    }
                                    if icons::icon_action_button(
                                        ui,
                                        Icon::Close,
                                        self.theme.text,
                                        format!("Close {file_name}"),
                                    )
                                        .clicked()
                                        {
                                            close_request = Some(i);
                                        }
                                    });
                            });
                        if is_active {
                            let rect = tab_frame.response.rect;
                            ui.painter().line_segment(
                                [rect.left_bottom(), rect.right_bottom()],
                                Stroke::new(1.5, self.theme.accent),
                            );
                        }
                    }
                    // Loading files appear as their own (new) log tab, appended
                    // after the fully-loaded tabs. They aren't real LogTabs yet,
                    // so they're rendered straight from the loader state.
                    for (li, loader) in self.loaders.iter().enumerate() {
                        let is_active = self.active_loader == Some(li);
                        egui::Frame::new()
                            .fill(if is_active {
                                self.theme.raised_surface
                            } else {
                                self.theme.log_surface
                            })
                            .stroke(Stroke::new(1.0, self.theme.border))
                            .corner_radius(4.0)
                            .inner_margin(egui::Margin::symmetric(5, 1))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new(&loader.name).color(self.theme.text),
                                            )
                                            .frame_when_inactive(false)
                                            .min_size(egui::Vec2::ZERO),
                                        )
                                        .on_hover_text(format!("Focus loading {}", loader.name))
                                        .clicked()
                                    {
                                        info!("focused loading tab {} ({})", li, loader.name);
                                        self.active_loader = Some(li);
                                        self.active = None;
                                    }
                                    if icons::icon_action_button(
                                        ui,
                                        Icon::Close,
                                        self.theme.text,
                                        format!("Cancel loading {}", loader.name),
                                    )
                                    .clicked()
                                    {
                                        loader
                                            .cancel
                                            .store(true, std::sync::atomic::Ordering::Relaxed);
                                    }
                                });
                            });
                    }
                });
                if let Some((old, new)) = tab_switch {
                    self.on_tab_switched(old, new);
                    self.save_workspace();
                }
            }
        });
        if let Some(i) = close_request {
            self.request_close_tab(i);
        }

        // ---- fixed-height timeline panel (always fully visible) ----
        // The timeline is rendered in a non-resizable top panel so the user
        // can never shrink it and hide filter lanes. Its height grows/shrinks
        // with the number of filters. Hidden when popped out.
        if let Some(idx) = self.active {
            let tab = &mut self.tabs[idx];
            if !tab.stale && !tab.timeline_detached && !tab.log_focus_mode {
                let height = timeline::panel_height(tab);
                egui::Panel::top("timeline_panel")
                    .exact_size(height)
                    .show(ui, |ui| {
                        timeline::show(ui, tab, &self.theme);
                    });
            }
        }

        egui::CentralPanel::default().show(ui, |ui| {
            // A focused loading file renders its progress as its own tab
            // (never in the current log tab's content area).
            if let Some(li) = self.active_loader {
                if let Some(loader) = self.loaders.get(li) {
                    ui.centered_and_justified(|ui| {
                        ui.vertical(|ui| {
                            ui.spinner();
                            ui.label(
                                RichText::new(format!("Opening {}", loader.name))
                                    .size(18.0)
                                    .strong(),
                            );
                            ui.add_space(8.0);
                            let pct = (loader.progress * 100.0) as u32;
                            ui.add(
                                egui::ProgressBar::new(loader.progress)
                                    .desired_width(360.0)
                                    .text(format!("{} — {pct}%", loader.stage.label())),
                            );
                            ui.add_space(8.0);
                            if icons::action_button(
                                ui,
                                Icon::Close,
                                "Cancel",
                                self.theme.text,
                                "Cancel opening this file",
                            )
                            .clicked()
                            {
                                loader
                                    .cancel
                                    .store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                        });
                    });
                    return;
                }
            }

            if self.loaders.is_empty() && self.tabs.is_empty() {
                // Keep the intro's complete drop target in one bounded region.
                // The width is comfortable on desktop and contracts with a
                // narrow window instead of relying on unrelated spacer rows.
                let content_width = ui.available_width().min(520.0);
                let shortcut = open_file_shortcut();
                let recent: Vec<PathBuf> = self
                    .settings
                    .recent_files()
                    .iter()
                    .take(5)
                    .cloned()
                    .collect();
                let open_error = self.open_error.clone();
                let mut open_recent = None;
                let mut retry_path = None;
                let intro = ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), ui.available_height()),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(content_width, ui.available_height()),
                            egui::Layout::centered_and_justified(egui::Direction::TopDown),
                            |ui| {
                                egui::Frame::new()
                                    .fill(if hovering_files {
                                        self.theme.accent.linear_multiply(0.10)
                                    } else {
                                        self.theme.surface
                                    })
                                    .corner_radius(10.0)
                                    .inner_margin(egui::Margin::symmetric(24, 22))
                                    .show(ui, |ui| {
                                        ui.set_width(content_width - 48.0);
                                        ui.vertical_centered(|ui| {
                                            ui.add(icons::app_mark(
                                                ui.ctx(),
                                                44.0,
                                                self.theme.text_muted,
                                            ));

                                            ui.add_space(18.0);
                                            ui.label(
                                                RichText::new(format!(
                                                    "Drop a log or .zip here, or use {shortcut}"
                                                ))
                                                .size(14.0)
                                                .color(self.theme.text_muted),
                                            );
                                            ui.add_space(14.0);
                                            if icons::action_button(
                                                ui,
                                                Icon::OpenFile,
                                                "Open file",
                                                self.theme.text,
                                                &format!(
                                                    "Choose a log, text, or ZIP file ({shortcut})"
                                                ),
                                            )
                                            .clicked()
                                            {
                                                self.open_file_dialog();
                                            }

                                            if let Some((_, error)) = &open_error {
                                                ui.add_space(10.0);
                                                ui.label(
                                                    RichText::new(format!(
                                                        "Could not open file: {error}"
                                                    ))
                                                    .small()
                                                    .color(self.theme.warning),
                                                );
                                                if icons::action_button(
                                                    ui,
                                                    Icon::Reset,
                                                    "Retry",
                                                    self.theme.warning,
                                                    "Try opening the file again",
                                                )
                                                .clicked()
                                                {
                                                    retry_path = open_error
                                                        .as_ref()
                                                        .map(|(path, _)| path.clone());
                                                }
                                            }

                                            if !recent.is_empty() {
                                                ui.add_space(18.0);
                                                ui.separator();
                                                ui.add_space(4.0);
                                                ui.label(
                                                    RichText::new("Recent files")
                                                        .small()
                                                        .strong()
                                                        .color(self.theme.text_muted),
                                                );
                                                for (recent_index, path) in
                                                    recent.iter().enumerate()
                                                {
                                                    let exists = path.exists();
                                                    let file_name = path
                                                        .file_name()
                                                        .map(|name| {
                                                            name.to_string_lossy().to_string()
                                                        })
                                                        .unwrap_or_else(|| {
                                                            path.display().to_string()
                                                        });
                                                    let parent = path
                                                        .parent()
                                                        .map(|parent| parent.display().to_string())
                                                        .unwrap_or_default();
                                                    let text_color = if exists {
                                                        self.theme.text
                                                    } else {
                                                        self.theme.text_muted
                                                    };
                                                    let row_rect = egui::Rect::from_min_size(
                                                        ui.available_rect_before_wrap().min,
                                                        egui::vec2(ui.available_width(), 38.0),
                                                    );
                                                    let row_hovered = ui.input(|input| {
                                                        input.pointer.hover_pos().is_some_and(
                                                            |pos| row_rect.contains(pos),
                                                        )
                                                    });
                                                    if row_hovered {
                                                        ui.painter().rect_filled(
                                                            row_rect,
                                                            egui::CornerRadius::same(4),
                                                            ui.visuals()
                                                                .widgets
                                                                .hovered
                                                                .weak_bg_fill,
                                                        );
                                                        ui.output_mut(|output| {
                                                            output.cursor_icon =
                                                                egui::CursorIcon::PointingHand;
                                                        });
                                                    }
                                                    ui.scope_builder(
                                                        egui::UiBuilder::new()
                                                            .max_rect(row_rect)
                                                            .layout(egui::Layout::left_to_right(
                                                                egui::Align::Center,
                                                            )),
                                                        |ui| {
                                                            ui.horizontal(|ui| {
                                                                ui.add(icons::icon_image(
                                                                    ui.ctx(),
                                                                    Icon::Log,
                                                                    14.0,
                                                                    text_color,
                                                                ));
                                                                ui.vertical(|ui| {
                                                                    ui.add(
                                                                        egui::Label::new(
                                                                        RichText::new(&file_name)
                                                                            .size(12.0)
                                                                            .color(text_color),
                                                                        )
                                                                        .selectable(false),
                                                                    );
                                                                    ui.add(
                                                                        egui::Label::new(
                                                                            RichText::new(if exists {
                                                                                parent.clone()
                                                                            } else {
                                                                                format!("Missing · {parent}")
                                                                            })
                                                                            .small()
                                                                            .color(self.theme.text_muted),
                                                                        )
                                                                        .selectable(false),
                                                                    );
                                                                });
                                                            });
                                                        },
                                                    );
                                                    ui.advance_cursor_after_rect(row_rect);
                                                    // Register the row after its visual children so
                                                    // this single hit target wins over the icon and
                                                    // text widgets beneath the pointer.
                                                    let response = ui.interact(
                                                        row_rect,
                                                        ui.id()
                                                            .with(("intro_recent", recent_index)),
                                                        egui::Sense::click(),
                                                    );
                                                    if exists && response.clicked() {
                                                        open_recent = Some(path.clone());
                                                    }
                                                    if !exists {
                                                        response.on_hover_text("File is missing");
                                                    }
                                                }
                                                ui.add_space(4.0);
                                                let recent_button = icons::action_button(
                                                    ui,
                                                    Icon::History,
                                                    "View all recent files",
                                                    self.theme.text,
                                                    "Open the full recent-file history",
                                                );
                                                if recent_button.clicked() {
                                                    self.recent_button_rect =
                                                        Some(recent_button.rect);
                                                    self.recent_show_dropdown = true;
                                                }
                                            }
                                        });
                                    });
                            },
                        );
                    },
                );
                if let Some(path) = open_recent {
                    self.open_file(path);
                } else if let Some(path) = retry_path {
                    self.open_file(path);
                }
                let _ = intro;
                return;
            }

            if let Some(idx) = self.active {
                let tab = &mut self.tabs[idx];

                // Filter removal confirmation popup.
                if let Some(ki) = tab.pending_filter_removal {
                    if self.settings.skip_filter_delete_confirm {
                        // The positive confirmation preference is stored as its
                        // existing inverse for sidecar compatibility.
                        tab.remove_filter_with_undo(ki);
                        tab.pending_filter_removal = None;
                    } else {
                        let filter_text = tab
                            .filters
                            .get(ki)
                            .map(|k| k.text.clone())
                            .unwrap_or_default();
                        let mut confirmed = false;
                        let mut cancelled = false;
                        overlay::modal(
                            ui.ctx(),
                            "remove_filter_modal",
                            "Remove filter",
                            egui::vec2(420.0, 220.0),
                            |ui| {
                                ui.label(
                                    RichText::new(format!("Remove filter '{}'?", filter_text))
                                        .size(14.0),
                                );
                                ui.add_space(10.0);

                                let mut confirm_future = !self.settings.skip_filter_delete_confirm;
                                if ui
                                    .checkbox(
                                        &mut confirm_future,
                                        "Confirm before future filter deletions",
                                    )
                                    .changed()
                                {
                                    self.settings.skip_filter_delete_confirm = !confirm_future;
                                    self.settings.save();
                                }

                                ui.add_space(10.0);
                                ui.separator();
                                ui.horizontal(|ui| {
                                    if icons::action_button(
                                        ui,
                                        Icon::Close,
                                        "Cancel",
                                        self.theme.text,
                                        "Keep this filter",
                                    )
                                    .clicked()
                                    {
                                        cancelled = true;
                                    }
                                    if icons::primary_action_button(
                                        ui,
                                        Icon::Remove,
                                        "Remove filter",
                                        self.theme.warning,
                                        "Remove this filter",
                                    )
                                    .clicked()
                                    {
                                        confirmed = true;
                                    }
                                });
                            },
                        );
                        if confirmed {
                            tab.remove_filter_with_undo(ki);
                            tab.pending_filter_removal = None;
                        } else if cancelled {
                            tab.pending_filter_removal = None;
                        }
                    }
                }

                // "Clear all filters" confirmation popup.
                if tab.pending_clear_filters {
                    let n = tab.filters.len();
                    if n == 0 {
                        tab.pending_clear_filters = false;
                    } else if self.settings.skip_filter_delete_confirm {
                        tab.clear_all_filters();
                        tab.pending_clear_filters = false;
                    } else {
                        let mut confirmed = false;
                        let mut cancelled = false;
                        overlay::modal(
                            ui.ctx(),
                            "clear_filters_modal",
                            "Clear filters",
                            egui::vec2(420.0, 220.0),
                            |ui| {
                                ui.label(
                                    RichText::new(format!("Remove all {n} filters?")).size(14.0),
                                );
                                ui.add_space(10.0);

                                let mut confirm_future = !self.settings.skip_filter_delete_confirm;
                                if ui
                                    .checkbox(
                                        &mut confirm_future,
                                        "Confirm before future filter deletions",
                                    )
                                    .changed()
                                {
                                    self.settings.skip_filter_delete_confirm = !confirm_future;
                                    self.settings.save();
                                }

                                ui.add_space(10.0);
                                ui.separator();
                                ui.horizontal(|ui| {
                                    if icons::action_button(
                                        ui,
                                        Icon::Close,
                                        "Cancel",
                                        self.theme.text,
                                        "Keep the current filters",
                                    )
                                    .clicked()
                                    {
                                        cancelled = true;
                                    }
                                    if icons::primary_action_button(
                                        ui,
                                        Icon::Remove,
                                        "Remove all",
                                        self.theme.warning,
                                        "Remove every filter",
                                    )
                                    .clicked()
                                    {
                                        confirmed = true;
                                    }
                                });
                            },
                        );
                        if confirmed {
                            tab.clear_all_filters();
                            tab.pending_clear_filters = false;
                        } else if cancelled {
                            tab.pending_clear_filters = false;
                        }
                    }
                }

                // Pin creation/editing modal (shared by log view & pin viewer).
                log_view::pin_modal_ui(ui, tab, &self.theme);
                log_view::full_line_inspector_ui(ui, tab, &self.theme);
                let inspector_mode_before = tab.embedded_inspector_mode;
                log_view::embedded_data_inspector_ui(ui, tab, &self.theme);
                if tab.embedded_inspector_mode != inspector_mode_before {
                    if let Some(mode) = tab.embedded_inspector_mode.persisted_name() {
                        self.settings.embedded_inspector_mode = mode.to_owned();
                        self.settings.save();
                    }
                }

                if tab.stale {
                    ui.centered_and_justified(|ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new("File changed on disk")
                                    .size(20.0)
                                    .color(self.theme.warning),
                            );
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new("Only file appending (tailing) is supported.")
                                    .size(14.0),
                            );
                            ui.label(
                                "In-place modifications or truncation requires a full reload.",
                            );
                            ui.add_space(12.0);
                            if icons::primary_action_button(
                                ui,
                                Icon::Reset,
                                "Close and reopen file",
                                self.theme.accent,
                                "Reload the changed file from disk",
                            )
                            .clicked()
                            {
                                // Close via close_request mechanism
                                self.request_close_tab(idx);
                            }
                        });
                    });
                    return;
                }

                if tab.log_focus_mode {
                    let focused_id = tab.focused_log_view_id;
                    if tab.is_view_detached(ViewTab::Log(focused_id)) {
                        tab.log_focus_mode = false;
                    } else {
                        ui.horizontal(|ui| {
                            if icons::action_button(
                                ui,
                                Icon::Collapse,
                                "Exit focus",
                                self.theme.text,
                                "Restore the timeline and dock layout (Escape)",
                            )
                            .clicked()
                            {
                                tab.log_focus_mode = false;
                            }
                            ui.label(
                                RichText::new("Focused Log View")
                                    .small()
                                    .color(self.theme.text_muted),
                            );
                        });
                        if tab.log_focus_mode {
                            let interacted = log_view_pointer_interaction(ui);
                            show_log_view(ui, tab, &self.theme, focused_id, interacted);
                            return;
                        }
                    }
                }

                let mut dock_state = std::mem::replace(&mut tab.dock_state, DockState::new(vec![]));
                let dock_path = tab.doc.path.clone();
                let main_workspace_rect = ui.available_rect_before_wrap();
                let has_main_logs = dock_state
                    .iter_all_tabs()
                    .any(|(_, view)| matches!(view, ViewTab::Log(_)));
                let has_main_utilities = dock_state.iter_all_tabs().any(|(_, view)| {
                    matches!(view, ViewTab::Pinned | ViewTab::Templates)
                });
                let empty_main_drop_rect = (!has_main_logs)
                .then(|| {
                    let rect = egui::Rect::from_min_max(
                        main_workspace_rect.min,
                        egui::pos2(
                            main_workspace_rect.max.x,
                            main_workspace_rect.min.y + main_workspace_rect.height() * 0.78,
                        ),
                    );
                    ui.allocate_rect(rect, egui::Sense::hover());
                    ui.painter().rect_stroke(
                        rect.shrink(6.0),
                        6.0,
                        Stroke::new(1.0, self.theme.border),
                        egui::StrokeKind::Inside,
                    );
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "Drag a Log View here to restore the top panel",
                        egui::FontId::proportional(14.0),
                        self.theme.text_muted,
                    );
                    rect
                });
                let mut drop_targets = Vec::new();
                let mut tab_viewer = TabViewer {
                    tab,
                    theme: &self.theme,
                    allow_popout: true,
                    return_to_main: None,
                    dock_container: DockContainer::Main,
                    drop_targets: &mut drop_targets,
                };
                let dock_style = crate::ui::theme::dock_style(self.dark_mode);
                DockArea::new(&mut dock_state)
                    .id(dock_area_id(dock_path.as_os_str()))
                    .style(dock_style)
                    // Only non-final Log Views are closeable. The dock's
                    // standalone add button creates Log Views in the clicked leaf.
                    .show_close_buttons(true)
                    .show_add_buttons(true)
                    .draggable_tabs(false)
                    // The main workspace has two fixed homes. Dragging is
                    // still supported for tab ordering and drop-on-tab, but
                    // the dock library must not offer arbitrary split sides.
                    .allowed_splits(egui_dock::AllowedSplits::None)
                    .show_leaf_close_all_buttons(false)
                    .show_inside(ui, &mut tab_viewer);
                drop(tab_viewer);
                if let Some(add_path) = tab.pending_add_log_view.take() {
                    if dock_state.leaf(add_path).is_ok() {
                        dock_state.set_focused_node_and_surface(add_path);
                        let new_id = tab.add_log_view();
                        let new_tab = ViewTab::Log(new_id);
                        dock_state.push_to_focused_leaf(new_tab);
                        if let Some(new_path) = dock_state.find_tab(&new_tab) {
                            let _ = dock_state.set_active_tab(new_path);
                        }
                    }
                }
                let main_drop_rect = empty_main_drop_rect.or_else(|| {
                    dock_state.iter_all_tabs().find_map(|(path, view)| {
                        matches!(view, ViewTab::Log(_))
                            .then(|| dock_state[path.node_path()].rect())
                            .flatten()
                    })
                });
                let utility_drop_rect = if !has_main_logs || !has_main_utilities {
                    Some(egui::Rect::from_min_max(
                        egui::pos2(
                            main_workspace_rect.min.x,
                            main_workspace_rect.min.y + main_workspace_rect.height() * 0.78,
                        ),
                        main_workspace_rect.max,
                    ))
                } else {
                    dock_state.iter_all_tabs().find_map(|(path, view)| {
                        matches!(view, ViewTab::Pinned | ViewTab::Templates)
                            .then(|| dock_state[path.node_path()].rect())
                            .flatten()
                    })
                };
                if let Some(main_drop_rect) = main_drop_rect {
                    drop_targets.push(DockTargetVisual {
                        target: DockDropTarget::MainLog,
                        hit_rect: main_drop_rect,
                        preview_rect: main_drop_rect.shrink(6.0),
                        label: "Dock in Log panel",
                    });
                }
                if let Some(utility_drop_rect) = utility_drop_rect {
                    drop_targets.push(DockTargetVisual {
                        target: DockDropTarget::MainUtility,
                        hit_rect: utility_drop_rect,
                        preview_rect: utility_drop_rect.shrink(6.0),
                        label: "Dock in utility panel",
                    });
                }
                tab.dock_state = dock_state;
                if !tab.main_dock_layout_is_legal() {
                    tab.normalize_main_dock_layout();
                }
                show_cross_window_drop_targets(
                    ui,
                    tab,
                    &self.theme,
                    DockContainer::Main,
                    &drop_targets,
                );
                // A Timeline pin click first lets Log consume its scroll
                // request, then switches this dock leaf to the matching card.
                tab.finish_pin_navigation();
            }
        });

        // ---- detached viewport windows (pop-out) ----
        if let Some(active_tab_idx) = self.active {
            let (path, detached_windows, file_name, timeline_detached) = {
                let tab = &self.tabs[active_tab_idx];
                (
                    tab.doc.path.clone(),
                    tab.detached_views.clone(),
                    tab.doc.file_name.clone(),
                    tab.timeline_detached,
                )
            };
            let mut detached: Vec<_> = detached_windows
                .into_iter()
                .map(|window| {
                    (
                        dock_window_viewport_id(path.as_os_str(), window),
                        DetachedViewport::Dock(window),
                    )
                })
                .collect();
            if timeline_detached {
                detached.push((
                    detached_viewport_id(path.as_os_str(), ViewTab::Timeline),
                    DetachedViewport::Timeline,
                ));
            }

            for (viewport_id, detached_viewport) in detached {
                // Re-resolve tab index by path to avoid stale indices after tab reorder/removal
                let resolved_idx = self
                    .tabs
                    .iter()
                    .position(|t| t.doc.path == path)
                    .unwrap_or(active_tab_idx);
                // A preceding detached viewport in this frame may have
                // received the last Log View from this source window. The
                // iteration snapshot still contains it, but recreating its
                // dock state here would resurrect an empty native window.
                if let DetachedViewport::Dock(window) = detached_viewport {
                    if !self.tabs[resolved_idx].detached_views.contains(&window) {
                        continue;
                    }
                }
                self.viewport_map
                    .insert(viewport_id, (resolved_idx, detached_viewport));

                let representative = match detached_viewport {
                    DetachedViewport::Timeline => ViewTab::Timeline,
                    DetachedViewport::Dock(window) => self.tabs[resolved_idx]
                        .detached_dock_states
                        .get(&window)
                        .and_then(|state| state.iter_all_tabs().next().map(|(_, tab)| *tab))
                        .unwrap_or(ViewTab::Pinned),
                };
                let title = detached_view_title(
                    &file_name,
                    representative,
                    self.tabs[resolved_idx].log_view_count(),
                    match representative {
                        ViewTab::Log(id) => self.tabs[resolved_idx]
                            .log_views
                            .get(&id)
                            .map(|view| view.display_number),
                        _ => None,
                    },
                );
                let initial_geometry = match detached_viewport {
                    DetachedViewport::Dock(window)
                        if self.tabs[resolved_idx]
                            .pending_detached_window_geometry
                            .remove(&window) =>
                    {
                        self.tabs[resolved_idx]
                            .detached_window_geometry
                            .get(&window)
                            .copied()
                    }
                    _ => None,
                };
                let mut viewport_builder = egui::ViewportBuilder::default().with_title(title);
                if let Some(geometry) = initial_geometry {
                    viewport_builder = viewport_builder.with_inner_size(geometry.inner_size);
                    if let Some(position) = geometry.position {
                        viewport_builder = viewport_builder.with_position(position);
                    }
                }

                ui.ctx()
                    .show_viewport_immediate(viewport_id, viewport_builder, |ctx, _| {
                        // Apply the same token-driven styling in native child windows.
                        let resolved_dark_mode =
                            crate::ui::theme::apply_egui_theme(ctx, self.settings.theme_mode);

                        // Re-resolve tab index by path to avoid stale indices
                        let resolved_idx = self
                            .tabs
                            .iter()
                            .position(|t| t.doc.path == path)
                            .unwrap_or(active_tab_idx);
                        self.viewport_map
                            .insert(viewport_id, (resolved_idx, detached_viewport));

                        let mut return_to_main = false;
                        let mut dock_transfer_closed_viewport = false;
                        if let Some(&(tab_idx, detached_inner)) =
                            self.viewport_map.get(&viewport_id)
                        {
                            if let Some(tab) = self.tabs.get_mut(tab_idx) {
                                if let DetachedViewport::Dock(window) = detached_inner {
                                    let (position, inner_size) = ctx.input(|input| {
                                        (
                                            input.viewport().outer_rect.map(|rect| rect.min),
                                            input.viewport().inner_rect.map(|rect| rect.size()),
                                        )
                                    });
                                    let geometry =
                                        tab.detached_window_geometry.entry(window).or_default();
                                    if position.is_some() {
                                        geometry.position = position;
                                    }
                                    if let Some(inner_size) = inner_size {
                                        geometry.inner_size = inner_size;
                                    }
                                }
                                let viewport_focused =
                                    ctx.input(|input| input.viewport().focused.unwrap_or(false));
                                egui::CentralPanel::default().show(
                                    ctx,
                                    |ui| match detached_inner {
                                        DetachedViewport::Timeline => {
                                            timeline::show(ui, tab, &self.theme)
                                        }
                                        DetachedViewport::Dock(window) => {
                                            let mut detached_state = tab
                                                .detached_dock_states
                                                .remove(&window)
                                                .unwrap_or_else(|| DockState::new(vec![]));
                                            let dock_style =
                                                crate::ui::theme::dock_style(resolved_dark_mode);
                                            let mut drop_targets = Vec::new();
                                            let mut viewer = TabViewer {
                                                tab,
                                                theme: &self.theme,
                                                allow_popout: false,
                                                return_to_main: Some(&mut return_to_main),
                                                dock_container: DockContainer::Detached(window),
                                                drop_targets: &mut drop_targets,
                                            };
                                            DockArea::new(&mut detached_state)
                                                .id(egui::Id::new((
                                                    "detached_dock_area",
                                                    viewport_id,
                                                )))
                                                .style(dock_style)
                                                .show_close_buttons(true)
                                                .show_add_buttons(true)
                                                .draggable_tabs(false)
                                                .show_leaf_close_all_buttons(false)
                                                .show_inside(ui, &mut viewer);
                                            drop(viewer);
                                            if let Some(add_path) = tab.pending_add_log_view.take()
                                            {
                                                if detached_state.leaf(add_path).is_ok() {
                                                    detached_state
                                                        .set_focused_node_and_surface(add_path);
                                                    let new_id = tab.add_log_view();
                                                    let new_tab = ViewTab::Log(new_id);
                                                    detached_state.push_to_focused_leaf(new_tab);
                                                    if let Some(new_path) =
                                                        detached_state.find_tab(&new_tab)
                                                    {
                                                        let _ =
                                                            detached_state.set_active_tab(new_path);
                                                    }
                                                }
                                            }
                                            append_detached_leaf_drop_targets(
                                                &mut drop_targets,
                                                &detached_state,
                                                window,
                                                tab.active_dock_drag.as_ref().map(|drag| drag.tab),
                                            );
                                            tab.detached_dock_states.insert(window, detached_state);
                                            let moved = show_cross_window_drop_targets(
                                                ui,
                                                tab,
                                                &self.theme,
                                                DockContainer::Detached(window),
                                                &drop_targets,
                                            );
                                            if moved && !tab.detached_views.contains(&window) {
                                                dock_transfer_closed_viewport = true;
                                            }
                                            if viewport_focused {
                                                if let Some(id) = tab
                                                    .detached_dock_states
                                                    .get(&window)
                                                    .and_then(|state| {
                                                        state.iter_all_tabs().find_map(
                                                            |(_, tab)| match tab {
                                                                ViewTab::Log(id) => Some(*id),
                                                                _ => None,
                                                            },
                                                        )
                                                    })
                                                {
                                                    tab.focus_log_view(id);
                                                }
                                            }
                                        }
                                    },
                                );
                            }
                        }

                        if dock_transfer_closed_viewport {
                            self.viewport_map.remove(&viewport_id);
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            return;
                        }

                        if return_to_main {
                            self.viewport_map.remove(&viewport_id);
                            if let Some(tab) = self.tabs.get_mut(resolved_idx) {
                                if let DetachedViewport::Dock(window) = detached_viewport {
                                    tab.just_closed_viewports.push(window);
                                }
                            }
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            return;
                        }

                        // Handle close: clean up state so the window isn't recreated
                        if ctx.input(|i| i.viewport().close_requested()) {
                            if let Some((tab_idx, detached_inner)) =
                                self.viewport_map.remove(&viewport_id)
                            {
                                if let Some(tab) = self.tabs.get_mut(tab_idx) {
                                    match detached_inner {
                                        DetachedViewport::Timeline => tab.timeline_detached = false,
                                        DetachedViewport::Dock(window) => {
                                            tab.just_closed_viewports.push(window)
                                        }
                                    }
                                }
                            }
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
            }
        }

        // A cross-window move can empty a *different* detached source window
        // (for example, dropping its final Log View into the main window).
        // It will not be rendered again, so explicitly close its native
        // viewport instead of leaving an empty OS window behind.
        let emptied_detached_viewports: Vec<_> = self
            .viewport_map
            .iter()
            .filter_map(|(viewport_id, (tab_idx, detached))| {
                let DetachedViewport::Dock(window) = detached else {
                    return None;
                };
                self.tabs
                    .get(*tab_idx)
                    .is_some_and(|tab| !tab.detached_views.contains(window))
                    .then_some(*viewport_id)
            })
            .collect();
        for viewport_id in emptied_detached_viewports {
            self.viewport_map.remove(&viewport_id);
            ui.ctx()
                .send_viewport_cmd_to(viewport_id, egui::ViewportCommand::Close);
        }

        if self.tabs.iter().any(|tab| tab.active_dock_drag.is_some()) {
            ui.ctx().request_repaint();
            for viewport_id in self.viewport_map.keys().copied() {
                ui.ctx().request_repaint_of(viewport_id);
            }
        }

        self.views_dropdown_ui(ui);
        format_menu::show(self, ui.ctx());

        if self.recent_show_dropdown {
            if let Some(button_rect) = self.recent_button_rect {
                let popup =
                    overlay::popover(ui.ctx(), "recent_popup", button_rect.left_bottom(), |ui| {
                        ui.set_min_width(260.0);
                        ui.set_max_width(360.0);
                        ui.label(RichText::new("Recent Files").strong().size(14.0));
                        ui.separator();

                        let recent: Vec<PathBuf> = self.settings.recent_files().to_vec();
                        if recent.is_empty() {
                            ui.label(
                                RichText::new(
                                    "No recent files yet.\nOpen a log file and it'll show up here.",
                                )
                                .small()
                                .color(self.theme.text_muted),
                            );
                        } else {
                            let mut remove_missing: Option<usize> = None;
                            let row_height = 32.0;
                            egui::ScrollArea::vertical()
                                .max_height(280.0)
                                .auto_shrink([false, false])
                                .show_rows(ui, row_height, recent.len(), |ui, range| {
                                    for i in range {
                                        let path = &recent[i];
                                        let exists = path.exists();
                                        let label = path
                                            .file_name()
                                            .map(|n| n.to_string_lossy().to_string())
                                            .unwrap_or_else(|| path.to_string_lossy().to_string());
                                        let color = if exists {
                                            self.theme.text
                                        } else {
                                            self.theme.text_muted
                                        };
                                        ui.horizontal(|ui| {
                                            ui.add(icons::icon_image(
                                                ui.ctx(),
                                                Icon::Log,
                                                14.0,
                                                color,
                                            ));
                                            let resp = ui.add(
                                                egui::Button::new(
                                                    RichText::new(&label).color(color).size(12.0),
                                                )
                                                .frame_when_inactive(false)
                                                .min_size(egui::Vec2::ZERO),
                                            );
                                            let path_hint = path.to_string_lossy();
                                            let resp = resp.on_hover_text(if exists {
                                                path_hint.to_string()
                                            } else {
                                                format!("Missing: {path_hint}")
                                            });
                                            if resp.clicked() && exists {
                                                self.open_file(path.clone());
                                                self.recent_show_dropdown = false;
                                            }
                                            if resp.hovered() && exists {
                                                ui.output_mut(|output| {
                                                    output.cursor_icon =
                                                        egui::CursorIcon::PointingHand
                                                });
                                            }
                                            if !exists
                                                && icons::icon_action_button(
                                                    ui,
                                                    Icon::Remove,
                                                    self.theme.text,
                                                    "Remove this missing file from Recent",
                                                )
                                                .clicked()
                                            {
                                                remove_missing = Some(i);
                                            }
                                        });
                                    }
                                });
                            if let Some(idx) = remove_missing {
                                self.settings.recent_files.remove(idx);
                                self.settings.save();
                            }
                        }
                    });
                if popup.should_close() {
                    self.recent_show_dropdown = false;
                }
            }
        }

        self.zip_import_ui(ui.ctx());

        if self.show_new_filter_popup {
            overlay::modal(
                ui.ctx(),
                "save_filter_modal",
                "Save filter set",
                egui::vec2(460.0, 180.0),
                |ui| {
                    ui.label("Give the active filters a name you can recognize later.");
                    ui.horizontal(|ui| {
                        ui.label("Name:");
                        let resp = ui.text_edit_singleline(&mut self.new_filter_name);
                        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            if !self.new_filter_name.is_empty() {
                                self.save_filter(&self.new_filter_name.clone());
                                self.show_new_filter_popup = false;
                                self.new_filter_name.clear();
                            }
                        }
                    });
                    ui.add_space(10.0);
                    ui.separator();
                    ui.horizontal(|ui| {
                        if icons::action_button(
                            ui,
                            Icon::Close,
                            "Cancel",
                            self.theme.text,
                            "Close without saving this filter set",
                        )
                        .clicked()
                        {
                            self.show_new_filter_popup = false;
                            self.new_filter_name.clear();
                        }
                        if icons::primary_action_button_enabled(
                            ui,
                            !self.new_filter_name.trim().is_empty(),
                            Icon::Save,
                            "Save",
                            self.theme.accent,
                            if self.new_filter_name.trim().is_empty() {
                                "Enter a name first"
                            } else {
                                "Save this filter set"
                            },
                        )
                        .clicked()
                        {
                            if !self.new_filter_name.is_empty() {
                                self.save_filter(&self.new_filter_name.clone());
                                self.show_new_filter_popup = false;
                                self.new_filter_name.clear();
                            }
                        }
                    });
                },
            );
        }

        if self.show_rename_filter_popup {
            overlay::modal(
                ui.ctx(),
                "rename_filter_modal",
                format!("Rename ‘{}’", self.rename_filter_target),
                egui::vec2(460.0, 180.0),
                |ui| {
                    ui.label("Choose a clear name for this saved filter set.");
                    ui.horizontal(|ui| {
                        ui.label("New name:");
                        let resp = ui.text_edit_singleline(&mut self.rename_filter_new_name);
                        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            if !self.rename_filter_new_name.is_empty() {
                                self.rename_filter(
                                    &self.rename_filter_target.clone(),
                                    &self.rename_filter_new_name.clone(),
                                );
                                self.show_rename_filter_popup = false;
                            }
                        }
                    });
                    ui.add_space(10.0);
                    ui.separator();
                    ui.horizontal(|ui| {
                        if icons::action_button(
                            ui,
                            Icon::Close,
                            "Cancel",
                            self.theme.text,
                            "Keep the current filter-set name",
                        )
                        .clicked()
                        {
                            self.show_rename_filter_popup = false;
                        }
                        if icons::primary_action_button_enabled(
                            ui,
                            !self.rename_filter_new_name.trim().is_empty(),
                            Icon::Edit,
                            "Rename",
                            self.theme.accent,
                            if self.rename_filter_new_name.trim().is_empty() {
                                "Enter a new name first"
                            } else {
                                "Rename this saved filter set"
                            },
                        )
                        .clicked()
                        {
                            if !self.rename_filter_new_name.is_empty() {
                                self.rename_filter(
                                    &self.rename_filter_target.clone(),
                                    &self.rename_filter_new_name.clone(),
                                );
                                self.show_rename_filter_popup = false;
                            }
                        }
                    });
                },
            );
        }

        if self.show_filter_dropdown {
            self.show_filters_dropdown(ui);
        }

        self.ai_assistant_popup_ui(ui);

        self.command_palette_ui(ui.ctx());
        self.goto_ui(ui.ctx());
        self.cheat_sheet_ui(ui.ctx());
        self.close_save_error_ui(ui.ctx());
        self.sidecar_recovery_ui(ui.ctx());

        settings::show_settings_popup(ui, self);
        settings::show_integrate_popup(ui, self);
        custom_date::show_custom_date_popup(self, ui.ctx());
        record_format::show_record_format_popup(self, ui.ctx());

        // MCP error popup — show when MCP server fails to start
        if self.mcp_error_popup.is_some() {
            let mut dismissed = false;
            let error_msg = self.mcp_error_popup.clone().unwrap_or_default();
            overlay::modal(
                ui.ctx(),
                "mcp_error_modal",
                "AI Assistant connection error",
                egui::vec2(560.0, 220.0),
                |ui| {
                    ui.label(RichText::new(&error_msg).size(14.0));
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new("Check that another Haystack connection is not already using the local port, then try again.")
                            .small()
                            .color(self.theme.text_muted),
                    );
                    ui.separator();
                    if icons::primary_action_button(
                        ui,
                        Icon::Close,
                        "Close",
                        self.theme.accent,
                        "Dismiss this error",
                    )
                    .clicked()
                    {
                        dismissed = true;
                    }
                },
            );
            if dismissed {
                self.mcp_error_popup = None;
            }
        }

        if let Some(idx) = self.pending_restore_tab {
            if idx >= self.tabs.len() {
                self.pending_restore_tab = None;
            } else {
                let mut restore = false;
                let mut keep_notes = false;
                overlay::modal(
                    ui.ctx(),
                    "restore_positions_modal",
                    "Log changed on disk",
                    egui::vec2(500.0, 200.0),
                    |ui| {
                        ui.label("This log changed since the investigation was saved.");
                        ui.label("Restore the previous line positions and pins?");
                        ui.add_space(10.0);
                        ui.separator();
                        ui.horizontal(|ui| {
                            if icons::action_button(
                                ui,
                                Icon::Close,
                                "Keep notes only",
                                self.theme.text,
                                "Keep notes but discard saved line positions",
                            )
                            .clicked()
                            {
                                keep_notes = true;
                            }
                            if icons::primary_action_button(
                                ui,
                                Icon::Reset,
                                "Restore positions",
                                self.theme.accent,
                                "Restore saved line positions and pins",
                            )
                            .clicked()
                            {
                                restore = true;
                            }
                        });
                    },
                );
                if restore || keep_notes {
                    if let Some(tab) = self.tabs.get_mut(idx) {
                        tab.confirm_sidecar_restore(restore);
                    }
                    self.pending_restore_tab = None;
                    let _ = self.save_tab_sidecar(idx);
                }
            }
        }

        // Toast notification (self-dismissing, ~5s). Tracked screenshots
        // deliberately show the steady-state UI rather than transient hints.
        if std::env::var_os("HAYSTACK_SCREENSHOT_PATH").is_some() {
            for tab in &mut self.tabs {
                tab.pending_toast = None;
            }
            self.toast_message = None;
            self.toast_at = None;
        } else {
            if let Some(idx) = self.active {
                if let Some(tab) = self.tabs.get_mut(idx) {
                    if let Some(message) = tab.pending_toast.take() {
                        self.show_toast(message);
                    }
                }
            }
            if let Some(at) = self.toast_at {
                if let Some(msg) = self.toast_message.clone() {
                    if at.elapsed() < Duration::from_secs(5) {
                        egui::Area::new(egui::Id::new("app_toast"))
                            .order(egui::Order::Tooltip)
                            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -24.0))
                            .show(ui.ctx(), |ui| {
                                egui::Frame::NONE
                                    .fill(self.theme.surface)
                                    .corner_radius(4.0)
                                    .inner_margin(egui::Margin::symmetric(12, 6))
                                    .show(ui, |ui| {
                                        ui.label(RichText::new(&msg).color(self.theme.text));
                                    });
                            });
                        ui.ctx().request_repaint();
                    } else {
                        self.toast_message = None;
                        self.toast_at = None;
                    }
                } else {
                    self.toast_at = None;
                }
            }
        }
        if std::env::var_os("HAYSTACK_SCREENSHOT_PATH").is_none() {
            self.poll_sidecar_saves();
        }
        request_tracked_screenshot(
            ui.ctx(),
            !self.tabs.is_empty()
                && self.loaders.is_empty()
                && self.tabs.iter().all(|tab| {
                    tab.search_rx.is_none()
                        && tab.visible_rx.is_none()
                        && tab.filter_scan_progress.is_none()
                }),
        );
    }

    fn update_detached(&mut self, viewport_id: egui::ViewportId, ui: &mut egui::Ui) {
        // Apply theme visuals
        self.dark_mode = crate::ui::theme::apply_egui_theme(ui.ctx(), self.settings.theme_mode);
        overlay::mark_background_input(ui.ctx(), overlay::interactive_surface_open(self, ui.ctx()));

        let mut return_to_main = false;
        let mut dock_transfer_closed_viewport = false;
        // Look up which tab + view this viewport belongs to
        if let Some(&(tab_idx, detached_viewport)) = self.viewport_map.get(&viewport_id) {
            if let Some(tab) = self.tabs.get_mut(tab_idx) {
                if let DetachedViewport::Dock(window) = detached_viewport {
                    let (position, inner_size) = ui.ctx().input(|input| {
                        (
                            input.viewport().outer_rect.map(|rect| rect.min),
                            input.viewport().inner_rect.map(|rect| rect.size()),
                        )
                    });
                    let geometry = tab.detached_window_geometry.entry(window).or_default();
                    if position.is_some() {
                        geometry.position = position;
                    }
                    if let Some(inner_size) = inner_size {
                        geometry.inner_size = inner_size;
                    }
                }
                if !return_to_main {
                    egui::CentralPanel::default().show(ui, |ui| match detached_viewport {
                        DetachedViewport::Timeline => timeline::show(ui, tab, &self.theme),
                        DetachedViewport::Dock(window) => {
                            let mut detached_state = tab
                                .detached_dock_states
                                .remove(&window)
                                .unwrap_or_else(|| DockState::new(vec![]));
                            let dock_style = crate::ui::theme::dock_style(self.dark_mode);
                            let mut drop_targets = Vec::new();
                            let mut viewer = TabViewer {
                                tab,
                                theme: &self.theme,
                                allow_popout: false,
                                return_to_main: Some(&mut return_to_main),
                                dock_container: DockContainer::Detached(window),
                                drop_targets: &mut drop_targets,
                            };
                            DockArea::new(&mut detached_state)
                                .id(egui::Id::new(("detached_dock_area", viewport_id)))
                                .style(dock_style)
                                .show_close_buttons(true)
                                .show_add_buttons(true)
                                .draggable_tabs(false)
                                .show_leaf_close_all_buttons(false)
                                .show_inside(ui, &mut viewer);
                            drop(viewer);
                            if let Some(add_path) = tab.pending_add_log_view.take() {
                                if detached_state.leaf(add_path).is_ok() {
                                    detached_state.set_focused_node_and_surface(add_path);
                                    let new_id = tab.add_log_view();
                                    let new_tab = ViewTab::Log(new_id);
                                    detached_state.push_to_focused_leaf(new_tab);
                                    if let Some(new_path) = detached_state.find_tab(&new_tab) {
                                        let _ = detached_state.set_active_tab(new_path);
                                    }
                                }
                            }
                            append_detached_leaf_drop_targets(
                                &mut drop_targets,
                                &detached_state,
                                window,
                                tab.active_dock_drag.as_ref().map(|drag| drag.tab),
                            );
                            tab.detached_dock_states.insert(window, detached_state);
                            let moved = show_cross_window_drop_targets(
                                ui,
                                tab,
                                &self.theme,
                                DockContainer::Detached(window),
                                &drop_targets,
                            );
                            if moved && !tab.detached_views.contains(&window) {
                                dock_transfer_closed_viewport = true;
                            }
                        }
                    });
                }
            }
        }

        if dock_transfer_closed_viewport {
            self.viewport_map.remove(&viewport_id);
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        if return_to_main {
            let tab_idx = self.viewport_map.get(&viewport_id).map(|(idx, _)| *idx);
            let detached = self.viewport_map.get(&viewport_id).map(|(_, view)| *view);
            self.viewport_map.remove(&viewport_id);
            if let (Some(tab_idx), Some(DetachedViewport::Dock(window))) = (tab_idx, detached) {
                if let Some(tab) = self.tabs.get_mut(tab_idx) {
                    tab.just_closed_viewports.push(window);
                }
            }
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        // Handle close: clean up state so the window isn't recreated
        if ui.ctx().input(|i| i.viewport().close_requested()) {
            if let Some((tab_idx, detached)) = self.viewport_map.remove(&viewport_id) {
                if let Some(tab) = self.tabs.get_mut(tab_idx) {
                    match detached {
                        DetachedViewport::Timeline => tab.timeline_detached = false,
                        DetachedViewport::Dock(window) => tab.just_closed_viewports.push(window),
                    }
                }
            }
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

fn saved_filter_row_id(filter_name: &str) -> egui::Id {
    egui::Id::new(("saved_filter_row", filter_name))
}

fn dock_area_id(path: &std::ffi::OsStr) -> egui::Id {
    egui::Id::new(("haystack_dock_area", path))
}

#[cfg(test)]
mod tests {
    use eframe::egui;
    use std::ffi::OsStr;

    use super::{
        compact_top_bar, detached_view_title, detached_viewport_id, dock_area_id,
        dock_tab_accessibility_label, dock_tab_action_rects, dock_tab_title,
        document_status_context, dropdown_visibility, saved_filter_row_id, should_request_repaint,
        LogViewId, TopPanelDropdown, ViewTab,
    };

    #[test]
    fn top_panel_dropdowns_are_mutually_exclusive_and_toggle() {
        assert_eq!(
            dropdown_visibility(TopPanelDropdown::Filters, true),
            (false, false, false)
        );
        assert_eq!(
            dropdown_visibility(TopPanelDropdown::Recent, false),
            (true, false, false)
        );
        assert_eq!(
            dropdown_visibility(TopPanelDropdown::More, false),
            (false, false, true)
        );
    }

    #[test]
    fn top_bar_compacts_only_below_the_supported_width() {
        assert!(compact_top_bar(960.0));
        assert!(!compact_top_bar(1100.0));
        assert!(!compact_top_bar(1440.0));
    }

    #[test]
    fn empty_workspace_hides_stale_document_status() {
        assert_eq!(
            document_status_context(None, "Loaded `old.log` — 42 lines"),
            ""
        );
        assert_eq!(
            document_status_context(Some("format: plain · date: none"), ""),
            "format: plain · date: none"
        );
    }

    #[test]
    fn completed_loader_requests_a_follow_up_frame() {
        assert!(should_request_repaint(false, true, false, false));
        assert!(!should_request_repaint(false, false, false, false));
    }

    #[test]
    fn saved_filter_row_ids_are_stable_and_distinct() {
        assert_eq!(
            saved_filter_row_id("Default-1"),
            saved_filter_row_id("Default-1")
        );
        assert_ne!(
            saved_filter_row_id("Default-1"),
            saved_filter_row_id("Default-2")
        );
    }

    #[test]
    fn dock_area_ids_are_stable_and_distinct_per_document() {
        assert_eq!(
            dock_area_id(OsStr::new("iOS-1K.log")),
            dock_area_id(OsStr::new("iOS-1K.log"))
        );
        assert_ne!(
            dock_area_id(OsStr::new("iOS-1K.log")),
            dock_area_id(OsStr::new("android.log"))
        );
    }

    #[test]
    fn detached_log_viewport_ids_are_stable_and_distinct_per_view() {
        let path = OsStr::new("iOS-1K.log");
        let first = ViewTab::Log(LogViewId::INITIAL);
        let second = ViewTab::Log(LogViewId(LogViewId::INITIAL.0 + 1));
        assert_eq!(
            detached_viewport_id(path, first),
            detached_viewport_id(path, first)
        );
        assert_ne!(
            detached_viewport_id(path, first),
            detached_viewport_id(path, second)
        );
    }

    #[test]
    fn detached_titles_include_filename_and_view_identity() {
        let first = ViewTab::Log(LogViewId::INITIAL);
        assert_eq!(
            detached_view_title("server.log", first, 1, Some(1)),
            "server.log · Log View"
        );
        assert_eq!(
            detached_view_title("server.log", first, 2, Some(7)),
            "server.log · Log View 7"
        );
        assert_eq!(
            detached_view_title("server.log", ViewTab::Timeline, 2, None),
            "server.log · Timeline"
        );
    }

    #[test]
    fn dock_tab_headers_reserve_only_detach_actions() {
        let tab_rect = egui::Rect::from_min_size(egui::pos2(20.0, 10.0), egui::vec2(100.0, 24.0));
        let log = ViewTab::Log(LogViewId::INITIAL);
        for tab in [log, ViewTab::Pinned, ViewTab::Templates] {
            assert!(dock_tab_title(tab, 1, Some(1)).ends_with("     "));
            let popout = dock_tab_action_rects(tab, tab_rect, false).unwrap();
            assert!(tab_rect.contains_rect(popout));
            assert!(popout.center().x > tab_rect.center().x);
        }
        assert_eq!(dock_tab_title(ViewTab::Timeline, 1, None), "Timeline");
        assert!(dock_tab_action_rects(ViewTab::Timeline, tab_rect, false).is_none());
    }

    #[test]
    fn multiple_log_tab_titles_are_numbered_and_actions_avoid_close_button() {
        let tab = ViewTab::Log(LogViewId::INITIAL);
        assert_eq!(dock_tab_title(tab, 1, Some(1)).trim(), "Log");
        assert_eq!(dock_tab_title(tab, 2, Some(7)).trim(), "Log 7");

        let tab_rect = egui::Rect::from_min_size(egui::pos2(20.0, 10.0), egui::vec2(140.0, 24.0));
        let popout = dock_tab_action_rects(tab, tab_rect, true).unwrap();
        assert!(popout.right() <= tab_rect.right() - 11.0);
    }

    #[test]
    fn log_tab_accessibility_copy_explains_actions_and_final_view_invariant() {
        let tab = ViewTab::Log(LogViewId::INITIAL);
        let only = dock_tab_accessibility_label(tab, 1, Some(1));
        assert!(only.starts_with("Log tab."));
        assert!(only.contains("Add another Log View"));
        assert!(only.contains("cannot be closed"));

        let multiple = dock_tab_accessibility_label(tab, 2, Some(7));
        assert!(multiple.starts_with("Log 7 tab."));
        assert!(multiple.contains("separate window"));
        assert!(multiple.contains("close this Log View"));
    }
}
