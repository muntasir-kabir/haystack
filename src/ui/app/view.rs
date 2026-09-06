use std::path::PathBuf;
use std::time::Duration;

use eframe::egui;
use egui::{Color32, RichText};
use egui_dock::{DockArea, DockState};
use log::info;

use crate::ui::icons::{self, Icon};
use crate::ui::log_view;
use crate::ui::pin_viewer;
use crate::ui::template_view;
use crate::ui::theme::Theme;
use crate::ui::timeline;
use logotomy::core::time::parse_time_param;

use super::model::*;
use crate::ui::custom_date;
use crate::ui::settings;

#[path = "filters_dropdown.rs"]
mod filters_dropdown;
#[path = "key_listener.rs"]
mod key_listener;
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

fn popup_should_close(
    escape_pressed: bool,
    click_pos: Option<egui::Pos2>,
    button_rect: egui::Rect,
    popup_rect: egui::Rect,
) -> bool {
    escape_pressed
        || click_pos.is_some_and(|position| {
            !button_rect.contains(position) && !popup_rect.contains(position)
        })
}

/// Save a renderer-native viewport capture when the tracked-screenshot
/// environment hook is active. This avoids OS screen-recording permissions and
/// keeps README captures deterministic across local and CI workflows.
fn consume_tracked_screenshot(ctx: &egui::Context) {
    let Ok(target) = std::env::var("LOGOTOMY_SCREENSHOT_PATH") else {
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
    let Ok(target) = std::env::var("LOGOTOMY_SCREENSHOT_PATH") else {
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

impl Drop for LogotomyApp {
    fn drop(&mut self) {
        if self.mcp_enabled {
            self.stop_mcp();
        }
        // Tracked screenshot runs are renderer-only previews. Avoid changing
        // the user's workspace, sidecars, recents, or persisted settings.
        if std::env::var_os("LOGOTOMY_SCREENSHOT_PATH").is_some() {
            return;
        }
        self.flush_sidecars();
        self.save_workspace();
        self.settings.save();
        info!("settings saved on exit");
    }
}

impl LogotomyApp {
    fn dismiss_app_overlay_on_escape(&mut self, ctx: &egui::Context) {
        let tab_confirmation_open = self
            .active
            .and_then(|index| self.tabs.get(index))
            .is_some_and(|tab| tab.pending_filter_removal.is_some() || tab.pending_clear_filters);
        let has_overlay = self.show_integrate_popup
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
            || tab_confirmation_open;
        if !has_overlay
            || !ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            return;
        }

        if self.show_integrate_popup {
            self.show_integrate_popup = false;
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
        } else if let Some(tab) = self.active.and_then(|index| self.tabs.get_mut(index)) {
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
        let Some(line) = tab.timeline.nearest_line(&tab.doc, timestamp) else {
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
        let mut open = true;
        let mut execute = None;
        egui::Window::new("Commands")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 60.0])
            .show(ctx, |ui| {
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
            });
        self.show_command_palette = open;
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

        let area = egui::Area::new(egui::Id::new("views_popup"))
            .current_pos(button_rect.left_bottom())
            .order(egui::Order::Foreground)
            .fixed_pos(button_rect.left_bottom());
        let area_resp = area.show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
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
            });
        });

        let escape = ui.input(|input| input.key_pressed(egui::Key::Escape));
        let click = ui.input(|input| {
            input
                .pointer
                .any_click()
                .then(|| input.pointer.interact_pos())
                .flatten()
        });
        if popup_should_close(escape, click, button_rect, area_resp.response.rect) {
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

        let area = egui::Area::new(egui::Id::new("ai_assistant_popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(button_rect.right_bottom() - egui::vec2(330.0, 0.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(330.0);
                    ui.horizontal(|ui| {
                        ui.add(icons::icon_image(ui.ctx(), Icon::Mcp, 16.0, self.theme.text));
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
                        "Set up Logotomy in Codex, Claude, or Cline",
                    )
                    .clicked()
                    {
                        self.show_integrate_popup = true;
                        self.show_ai_assistant_popup = false;
                    }
                });
            });

        let escape = ui.input(|input| input.key_pressed(egui::Key::Escape));
        let click = ui.input(|input| {
            input
                .pointer
                .any_click()
                .then(|| input.pointer.interact_pos())
                .flatten()
        });
        if popup_should_close(escape, click, button_rect, area.response.rect) {
            self.show_ai_assistant_popup = false;
        }
    }

    fn goto_ui(&mut self, ctx: &egui::Context) {
        if !self.show_goto_popup {
            return;
        }
        let mut open = true;
        let mut submit = false;
        egui::Window::new("Go to line or time")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
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
            });
        self.show_goto_popup = self.show_goto_popup && open;
        if submit {
            self.submit_goto();
        }
    }

    fn cheat_sheet_ui(&mut self, ctx: &egui::Context) {
        if !self.show_cheat_sheet {
            return;
        }
        let mut open = true;
        egui::Window::new("Keyboard & Mouse Shortcuts")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(520.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
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
                                AppCommand::GoTo => format!("{} or {}", spec.shortcut.map(|s| ctx.format_shortcut(&s)).unwrap_or_default(), ALIASES.iter().find(|(command, _)| *command == AppCommand::GoTo).map(|(_, shortcut)| ctx.format_shortcut(shortcut)).unwrap_or_default()),
                                _ => spec.shortcut.map(|s| ctx.format_shortcut(&s)).unwrap_or_default(),
                            };
                            ui.label(RichText::new(label).monospace().small().color(self.theme.text_muted));
                        });
                    });
                }
                ui.separator();
                ui.label(RichText::new("Mouse").strong());
                for gesture in [
                    "Timeline: scroll to zoom, drag to pan, Shift-drag to brush, double-click to reset.",
                    "Timeline lanes: click a lane or occurrence to select it; click eye to toggle; click trash to remove.",
                    "Log: click a row to select; right-click to pin; drag across rows to select a range.",
                    "Pins: click a card title to select it; use Remove or Delete to remove it.",
                ] { ui.label(RichText::new(gesture).small().color(self.theme.text_muted)); }
            });
        self.show_cheat_sheet = open;
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
        egui::Window::new("Could not save investigation state")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(if waiting_for_restore {
                    "This tab is waiting for a restored-investigation decision."
                } else {
                    "Logotomy could not save this tab's filters, notes, and view state."
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
            });
        if retry {
            self.close_save_error = None;
            self.request_close_tab(idx);
        }
        if discard {
            self.close_save_error = None;
            self.close_tab(idx);
        }
    }
}

/// Owns the mutable state for a single file tab and renders the UI for it.
struct TabViewer<'a> {
    tab: &'a mut LogTab,
    theme: &'a Theme,
}

impl<'a> egui_dock::TabViewer for TabViewer<'a> {
    type Tab = ViewTab;

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Self::Tab) {
        match tab {
            ViewTab::Timeline => timeline::show(ui, self.tab, self.theme),
            ViewTab::Log => log_view::show(ui, self.tab, self.theme),
            ViewTab::Pinned => pin_viewer::show(ui, self.tab, self.theme),
            ViewTab::Templates => template_view::show(ui, self.tab, self.theme),
        }
    }

    fn title(&mut self, tab: &mut Self::Tab) -> egui::WidgetText {
        match tab {
            ViewTab::Timeline => "Timeline".into(),
            ViewTab::Log => "Log".into(),
            ViewTab::Pinned => "Pinned".into(),
            ViewTab::Templates => "Templates".into(),
        }
    }

    fn is_closeable(&self, _tab: &Self::Tab) -> bool {
        false
    }

    fn on_tab_button(&mut self, _tab: &mut Self::Tab, response: &egui::Response) {
        if response.hovered() {
            response.clone().on_hover_text("Switch workspace view");
        }
    }
}

/// Draw the resize affordance that replaces egui-dock's leaf close-all button.
///
/// The native button is disabled on the `DockArea` because its action closes a
/// dock leaf. For Log and Pinned leaves the same location instead detaches the
/// active view into a viewport window.
fn draw_dock_resize_buttons(
    ui: &mut egui::Ui,
    dock_state: &DockState<ViewTab>,
    tab_bar_height: f32,
    log_tab: &mut LogTab,
    theme: &Theme,
) {
    for (path, leaf) in dock_state.iter_leaves() {
        let Some(active_view) = leaf.tabs.get(leaf.active.0) else {
            continue;
        };
        if !matches!(
            active_view,
            ViewTab::Log | ViewTab::Pinned | ViewTab::Templates
        ) {
            continue;
        }

        let header_rect = egui::Rect::from_min_max(
            egui::pos2(leaf.rect.right() - icons::ACTION_HEIGHT, leaf.rect.top()),
            egui::pos2(leaf.rect.right(), leaf.rect.top() + tab_bar_height),
        );
        let response = ui.interact(
            header_rect,
            egui::Id::new(("dock_leaf_resize", path)),
            egui::Sense::click(),
        );
        let color = if response.hovered() {
            theme.text
        } else {
            theme.text_muted
        };
        icons::paint_icon(
            ui.ctx(),
            ui.painter(),
            Icon::ExternalWindow,
            header_rect.center(),
            15.0,
            color,
        );
        let view_name = match active_view {
            ViewTab::Timeline => "Timeline",
            ViewTab::Log => "Log",
            ViewTab::Pinned => "Pinned",
            ViewTab::Templates => "Templates",
        };
        let response = response.on_hover_text(format!("Open {view_name} in a separate window"));
        if response.clicked() {
            log_tab.pending_detach = Some(*active_view);
        }
    }
}

impl eframe::App for LogotomyApp {
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

impl LogotomyApp {
    fn apply_tracked_screenshot_view(&mut self, ctx: &egui::Context) {
        if std::env::var("LOGOTOMY_SCREENSHOT_PATH").is_err() {
            return;
        }
        let applied_id = egui::Id::new("tracked_screenshot_view_applied");
        if ctx.data(|data| data.get_temp::<bool>(applied_id).unwrap_or(false)) {
            return;
        }
        let desired = match std::env::var("LOGOTOMY_SCREENSHOT_VIEW").as_deref() {
            Ok("pinned") => Some(ViewTab::Pinned),
            Ok("templates") => Some(ViewTab::Templates),
            Ok("log") => Some(ViewTab::Log),
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
        tab.bottom_panel_open = true;
        ctx.data_mut(|data| data.insert_temp(applied_id, true));
    }

    fn update_main(&mut self, ui: &mut egui::Ui) {
        consume_tracked_screenshot(ui.ctx());
        if std::env::var("LOGOTOMY_SCREENSHOT_PATH").is_ok() {
            ui.ctx()
                .all_styles_mut(|style| style.interaction.tooltip_delay = f32::INFINITY);
        }
        ui.ctx().set_visuals(if self.dark_mode {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });

        self.poll_open_requests(ui.ctx());

        let dropped = ui.ctx().input(|i| i.raw.dropped_files.clone());
        for f in dropped {
            if let Some(path) = f.path {
                self.open_file(path);
            }
        }
        let hovering_files = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());

        self.poll_loaders();
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
            if tab.poll_find() {
                any_search = true;
            }
            if tab.poll_embedded_data() {
                any_search = true;
            }
        }
        self.poll_mcp_search();
        self.drain_recent_query_updates();
        if any_search || !self.loaders.is_empty() || self.mcp_enabled {
            ui.ctx().request_repaint_after(Duration::from_millis(60));
        }

        // All application commands are consumed in one place so detached
        // views, the palette, and the cheat sheet cannot drift apart.
        let text_edit_active = ui.ctx().memory(|memory| memory.focused().is_some());
        let commands = key_listener::consume(ui.ctx(), text_edit_active);
        for command in commands {
            self.dispatch_command(command);
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
                        // If the close button removed the tab before we could
                        // detach it, re-add it to the dock first.
                        if tab.dock_state.find_tab(&view_to_detach).is_none() {
                            tab.dock_state
                                .main_surface_mut()
                                .push_to_focused_leaf(view_to_detach);
                        }
                        if let Some(tab_location) = tab.dock_state.find_tab(&view_to_detach) {
                            // Save a full snapshot of the dock layout before mutating it,
                            // so we can restore the original split arrangement when the
                            // view returns.
                            if tab.saved_dock_state.is_none() {
                                tab.saved_dock_state = Some(tab.dock_state.clone());
                            }
                            tab.detached_locations.insert(view_to_detach, tab_location);
                            tab.detached_views.insert(view_to_detach);
                            tab.dock_state.remove_tab(tab_location);
                        }
                    }
                }
                for closed_tab in tab.just_closed_viewports.drain(..) {
                    if tab.detached_views.is_empty() {
                        // All views are back — restore the original layout.
                        if let Some(saved) = tab.saved_dock_state.take() {
                            tab.dock_state = saved;
                        } else {
                            tab.dock_state
                                .main_surface_mut()
                                .push_to_focused_leaf(closed_tab);
                        }
                    } else {
                        // Other views still detached; push to focused leaf for now.
                        tab.dock_state
                            .main_surface_mut()
                            .push_to_focused_leaf(closed_tab);
                    }
                }
            }
        }

        let mut close_request: Option<usize> = None;
        egui::Panel::top("top_panel").show(ui, |ui| {
            let compact = compact_top_bar(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().interact_size.y = icons::ACTION_HEIGHT;
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.add(icons::app_logo(ui.ctx(), 20.0));
                if !compact {
                    ui.label(RichText::new("LOGotomy").strong().size(16.0));
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
                        self.show_command_palette = true;
                        self.command_palette_query.clear();
                        self.recent_show_dropdown = false;
                        self.show_filter_dropdown = false;
                        self.views_show_dropdown = false;
                        self.show_ai_assistant_popup = false;
                        self.show_settings_popup = false;
                    }
                }

                let context = match self.selected_log_format_status() {
                    Some(info) if !self.status.is_empty() => format!("{info} · {}", self.status),
                    Some(info) => info,
                    None => self.status.clone(),
                };
                let status_width = if compact { 160.0 } else { 300.0 };
                ui.add_sized(
                    egui::vec2(status_width, icons::ACTION_HEIGHT),
                    egui::Label::new(RichText::new(&context).small().color(self.theme.text_muted))
                        .truncate(),
                )
                .on_hover_text(&context);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let settings_resp = icons::action_button(
                        ui,
                        Icon::Settings,
                        if compact { "" } else { "Settings" },
                        self.theme.text,
                        "Open settings",
                    );
                    if settings_resp.clicked() {
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
                            self.theme.text
                        },
                        if self.mcp_enabled {
                            "AI Assistant connection is ready"
                        } else {
                            "Connect an AI coding assistant to the active log"
                        },
                    );
                    if ai.clicked() {
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
                        egui::Frame::new()
                            .fill(if is_active {
                                ui.visuals().selection.bg_fill
                            } else {
                                self.theme.surface
                            })
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
                                        .selectable_label(is_active, &file_name)
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
                    }
                    // Loading files appear as their own (new) log tab, appended
                    // after the fully-loaded tabs. They aren't real LogTabs yet,
                    // so they're rendered straight from the loader state.
                    for (li, loader) in self.loaders.iter().enumerate() {
                        let is_active = self.active_loader == Some(li);
                        egui::Frame::new()
                            .fill(if is_active {
                                ui.visuals().selection.bg_fill
                            } else {
                                self.theme.surface
                            })
                            .corner_radius(4.0)
                            .inner_margin(egui::Margin::symmetric(5, 1))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    if ui.selectable_label(is_active, &loader.name).clicked() {
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
            if !tab.stale && !tab.timeline_detached {
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
                ui.centered_and_justified(|ui| {
                    ui.vertical_centered(|ui| {
                        ui.add(icons::app_logo(ui.ctx(), 52.0));
                        ui.add_space(12.0);
                        ui.label(
                            RichText::new("Open a log to start investigating")
                                .size(22.0)
                                .strong()
                                .color(self.theme.text),
                        );
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("Drop any log or .zip to start")
                                .size(14.0)
                                .color(self.theme.text_muted),
                        );
                        ui.add_space(14.0);
                        // `available_width` shrinks as the centered column's
                        // minimum rect grows, so use the full scope width for
                        // the button row and center its main axis explicitly.
                        let buttons_width = ui.max_rect().width();
                        ui.allocate_ui_with_layout(
                            egui::vec2(buttons_width, icons::ACTION_HEIGHT),
                            egui::Layout::left_to_right(egui::Align::Center)
                                .with_main_align(egui::Align::Center)
                                .with_cross_align(egui::Align::Center),
                            |ui| {
                                if icons::primary_action_button(
                                    ui,
                                    Icon::OpenFile,
                                    "Open file",
                                    self.theme.accent,
                                    "Choose a log, text, or ZIP file (Cmd/Ctrl+O)",
                                )
                                .clicked()
                                {
                                    self.open_file_dialog();
                                }
                                if !self.settings.recent_files().is_empty() {
                                    let recent = icons::action_button(
                                        ui,
                                        Icon::History,
                                        "Recent files",
                                        self.theme.text,
                                        "Open a recently used file",
                                    );
                                    if recent.clicked() {
                                        self.recent_button_rect = Some(recent.rect);
                                        self.recent_show_dropdown = true;
                                    }
                                }
                            },
                        );
                    });
                });
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
                        egui::Window::new("Remove filter")
                            .collapsible(false)
                            .resizable(false)
                            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                            .show(ui.ctx(), |ui| {
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
                            });
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
                        egui::Window::new("Clear filters")
                            .collapsible(false)
                            .resizable(false)
                            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                            .show(ui.ctx(), |ui| {
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
                            });
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

                let mut dock_state = std::mem::replace(&mut tab.dock_state, DockState::new(vec![]));
                let dock_path = tab.doc.path.clone();
                let mut tab_viewer = TabViewer {
                    tab,
                    theme: &self.theme,
                };
                let dock_style = egui_dock::Style::from_egui(
                    ui.ctx()
                        .style_of(egui::Theme::from_dark_mode(self.dark_mode))
                        .as_ref(),
                );
                let tab_bar_height = dock_style.tab_bar.height;
                DockArea::new(&mut dock_state)
                    .id(dock_area_id(dock_path.as_os_str()))
                    .style(dock_style)
                    // Log and Pinned are detachable, not closable. Their
                    // resize affordances are rendered by TabViewer and below.
                    .show_close_buttons(false)
                    .show_leaf_close_all_buttons(false)
                    .show_inside(ui, &mut tab_viewer);
                draw_dock_resize_buttons(
                    ui,
                    &dock_state,
                    tab_bar_height,
                    tab_viewer.tab,
                    &self.theme,
                );
                tab.dock_state = dock_state;
                // A Timeline pin click first lets Log consume its scroll
                // request, then switches this dock leaf to the matching card.
                tab.finish_pin_navigation();
            }
        });

        // ---- detached viewport windows (pop-out) ----
        if let Some(active_tab_idx) = self.active {
            let (path, detached_views, file_name, timeline_detached) = {
                let tab = &self.tabs[active_tab_idx];
                (
                    tab.doc.path.clone(),
                    tab.detached_views.clone(),
                    tab.doc.file_name.clone(),
                    tab.timeline_detached,
                )
            };
            let mut detached_views = detached_views;
            if timeline_detached {
                detached_views.insert(ViewTab::Timeline);
            }

            for view_tab in detached_views {
                let viewport_id = egui::ViewportId::from_hash_of((path.as_os_str(), view_tab));

                // Re-resolve tab index by path to avoid stale indices after tab reorder/removal
                let resolved_idx = self
                    .tabs
                    .iter()
                    .position(|t| t.doc.path == path)
                    .unwrap_or(active_tab_idx);
                self.viewport_map
                    .entry(viewport_id)
                    .or_insert((resolved_idx, view_tab));

                let title = match view_tab {
                    ViewTab::Timeline => "Timeline",
                    ViewTab::Log => "Log",
                    ViewTab::Pinned => "Pinned",
                    ViewTab::Templates => "Templates",
                };

                let dark_mode = self.dark_mode;

                ui.ctx().show_viewport_immediate(
                    viewport_id,
                    egui::ViewportBuilder::default()
                        .with_title(format!("{} - {}", title, file_name))
                        .with_inner_size([600.0, 400.0]),
                    |ctx, _| {
                        // Apply theme visuals
                        ctx.set_visuals(if dark_mode {
                            egui::Visuals::dark()
                        } else {
                            egui::Visuals::light()
                        });

                        // Re-resolve tab index by path to avoid stale indices
                        let resolved_idx = self
                            .tabs
                            .iter()
                            .position(|t| t.doc.path == path)
                            .unwrap_or(active_tab_idx);
                        self.viewport_map
                            .entry(viewport_id)
                            .or_insert((resolved_idx, view_tab));

                        if let Some(&(tab_idx, view_tab_inner)) =
                            self.viewport_map.get(&viewport_id)
                        {
                            if let Some(tab) = self.tabs.get_mut(tab_idx) {
                                egui::CentralPanel::default().show(
                                    ctx,
                                    |ui| match view_tab_inner {
                                        ViewTab::Timeline => timeline::show(ui, tab, &self.theme),
                                        ViewTab::Log => log_view::show(ui, tab, &self.theme),
                                        ViewTab::Pinned => pin_viewer::show(ui, tab, &self.theme),
                                        ViewTab::Templates => {
                                            template_view::show(ui, tab, &self.theme)
                                        }
                                    },
                                );
                            }
                        }

                        // Handle close: clean up state so the window isn't recreated
                        if ctx.input(|i| i.viewport().close_requested()) {
                            if let Some((tab_idx, view_tab_inner)) =
                                self.viewport_map.remove(&viewport_id)
                            {
                                if let Some(tab) = self.tabs.get_mut(tab_idx) {
                                    if view_tab_inner == ViewTab::Timeline {
                                        tab.timeline_detached = false;
                                    } else {
                                        tab.detached_views.remove(&view_tab_inner);
                                        tab.just_closed_viewports.push(view_tab_inner);
                                    }
                                }
                            }
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    },
                );
            }
        }

        self.views_dropdown_ui(ui);

        if self.recent_show_dropdown {
            if let Some(button_rect) = self.recent_button_rect {
                let popup_id = egui::Id::new("recent_popup");
                let area = egui::Area::new(popup_id)
                    .current_pos(button_rect.left_bottom())
                    .order(egui::Order::Foreground)
                    .fixed_pos(button_rect.left_bottom());
                let area_resp = area.show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
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
                                                egui::Label::new(
                                                    RichText::new(&label).color(color).size(12.0),
                                                )
                                                .sense(egui::Sense::click()),
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
                });
                let escape = ui.input(|input| input.key_pressed(egui::Key::Escape));
                let click = ui.input(|input| {
                    input
                        .pointer
                        .any_click()
                        .then(|| input.pointer.interact_pos())
                        .flatten()
                });
                if popup_should_close(escape, click, button_rect, area_resp.response.rect) {
                    self.recent_show_dropdown = false;
                }
            }
        }

        if hovering_files {
            let screen = ui.ctx().globally_used_rect();
            let painter = ui.ctx().layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("drop_overlay"),
            ));
            painter.rect_filled(screen, egui::CornerRadius::same(0), self.theme.overlay_bg);
            painter.text(
                screen.center(),
                egui::Align2::CENTER_CENTER,
                "Drop to open\n.log, .txt, .zip, or any text file",
                egui::FontId::proportional(28.0),
                Color32::WHITE,
            );
            ui.ctx().request_repaint();
        }

        self.zip_import_ui(ui.ctx());

        if self.show_new_filter_popup {
            let mut open = self.show_new_filter_popup;
            egui::Window::new("Save filter set")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
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
                });
            if !open {
                self.show_new_filter_popup = false;
            }
        }

        if self.show_rename_filter_popup {
            let mut open = self.show_rename_filter_popup;
            egui::Window::new(format!("Rename ‘{}’", self.rename_filter_target))
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
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
                });
            if !open {
                self.show_rename_filter_popup = false;
            }
        }

        if self.show_filter_dropdown {
            self.show_filters_dropdown(ui);
        }

        self.ai_assistant_popup_ui(ui);

        self.command_palette_ui(ui.ctx());
        self.goto_ui(ui.ctx());
        self.cheat_sheet_ui(ui.ctx());
        self.close_save_error_ui(ui.ctx());

        settings::show_settings_popup(ui, self);
        settings::show_integrate_popup(ui, self);
        custom_date::show_custom_date_popup(self, ui.ctx());

        // MCP error popup — show when MCP server fails to start
        if self.mcp_error_popup.is_some() {
            let mut open = true;
            let mut dismissed = false;
            let error_msg = self.mcp_error_popup.clone().unwrap_or_default();
            egui::Window::new("AI Assistant connection error")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.label(RichText::new(&error_msg).size(14.0));
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new("Check that another Logotomy connection is not already using the local port, then try again.")
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
                });
            if !open || dismissed {
                self.mcp_error_popup = None;
            }
        }

        if let Some(idx) = self.pending_restore_tab {
            if idx >= self.tabs.len() {
                self.pending_restore_tab = None;
            } else {
                let mut restore = false;
                let mut keep_notes = false;
                egui::Window::new("Log changed on disk")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .show(ui.ctx(), |ui| {
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
                    });
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
        if std::env::var_os("LOGOTOMY_SCREENSHOT_PATH").is_some() {
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
        if std::env::var_os("LOGOTOMY_SCREENSHOT_PATH").is_none() {
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
        let visuals = if self.dark_mode {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        ui.ctx().set_visuals(visuals);

        // Look up which tab + view this viewport belongs to
        if let Some(&(tab_idx, view_tab)) = self.viewport_map.get(&viewport_id) {
            if let Some(tab) = self.tabs.get_mut(tab_idx) {
                egui::CentralPanel::default().show(ui, |ui| match view_tab {
                    ViewTab::Timeline => timeline::show(ui, tab, &self.theme),
                    ViewTab::Log => log_view::show(ui, tab, &self.theme),
                    ViewTab::Pinned => pin_viewer::show(ui, tab, &self.theme),
                    ViewTab::Templates => template_view::show(ui, tab, &self.theme),
                });
            }
        }

        // Handle close: clean up state so the window isn't recreated
        if ui.ctx().input(|i| i.viewport().close_requested()) {
            if let Some((tab_idx, view_tab)) = self.viewport_map.remove(&viewport_id) {
                if let Some(tab) = self.tabs.get_mut(tab_idx) {
                    if view_tab == ViewTab::Timeline {
                        tab.timeline_detached = false;
                    } else {
                        tab.detached_views.remove(&view_tab);
                        tab.just_closed_viewports.push(view_tab);
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
    egui::Id::new(("logotomy_dock_area", path))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use eframe::egui;

    use super::{
        compact_top_bar, dock_area_id, dropdown_visibility, popup_should_close,
        saved_filter_row_id, TopPanelDropdown,
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
    fn popup_dismissal_handles_escape_and_outside_clicks() {
        let button = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(30.0, 28.0));
        let popup = egui::Rect::from_min_size(egui::pos2(10.0, 42.0), egui::vec2(200.0, 160.0));

        assert!(popup_should_close(true, None, button, popup));
        assert!(!popup_should_close(
            false,
            Some(egui::pos2(20.0, 20.0)),
            button,
            popup
        ));
        assert!(!popup_should_close(
            false,
            Some(egui::pos2(30.0, 60.0)),
            button,
            popup
        ));
        assert!(popup_should_close(
            false,
            Some(egui::pos2(400.0, 400.0)),
            button,
            popup
        ));
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
}
