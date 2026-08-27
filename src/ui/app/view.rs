use std::path::PathBuf;
use std::time::Duration;

use eframe::egui;
use egui::{Color32, RichText};
use egui_dock::{DockArea, DockState};
use log::info;

use crate::ui::icons::{self, Icon};
use crate::ui::log_view;
use crate::ui::pin_viewer;
use crate::ui::theme::Theme;
use crate::ui::timeline;

use super::model::*;
use crate::ui::custom_date;
use crate::ui::settings;

#[path = "filters_dropdown.rs"]
mod filters_dropdown;
#[path = "templates.rs"]
mod templates;
use templates::template_browser;

impl Drop for LogotomyApp {
    fn drop(&mut self) {
        if self.mcp_enabled {
            self.stop_mcp();
        }
        self.settings.save();
        info!("settings saved on exit");
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
        }
    }

    fn title(&mut self, tab: &mut Self::Tab) -> egui::WidgetText {
        match tab {
            ViewTab::Timeline => "Timeline".into(),
            // Reserve visible trailing space for the resize affordance drawn
            // at the right edge of these detachable tab titles. Non-breaking
            // spaces ensure egui includes the padding in the measured width.
            ViewTab::Log => {
                "Log\u{00a0}\u{00a0}\u{00a0}\u{00a0}\u{00a0}\u{00a0}\u{00a0}\u{00a0}".into()
            }
            ViewTab::Pinned => {
                "Pinned\u{00a0}\u{00a0}\u{00a0}\u{00a0}\u{00a0}\u{00a0}\u{00a0}\u{00a0}".into()
            }
        }
    }

    fn is_closeable(&self, tab: &Self::Tab) -> bool {
        // The Timeline view is mandatory and cannot be closed. The Log and
        // Pinned views report as closeable so egui_dock reserves the
        // close-button space (preventing the title from overlapping our
        // pop-out icon); the close is intercepted in `on_close` instead.
        !matches!(tab, ViewTab::Timeline)
    }

    fn on_close(&mut self, tab: &mut Self::Tab) -> egui_dock::widgets::tab_viewer::OnCloseResponse {
        // Log and Pinned can't actually be closed — the close button is
        // repurposed as a pop-out (detach) action.
        if matches!(tab, ViewTab::Log | ViewTab::Pinned) {
            self.tab.pending_detach = Some(*tab);
            egui_dock::widgets::tab_viewer::OnCloseResponse::Ignore
        } else {
            egui_dock::widgets::tab_viewer::OnCloseResponse::Close
        }
    }

    fn on_tab_button(&mut self, tab: &mut Self::Tab, response: &egui::Response) {
        // Replace the dock's default close button with a pop-out (window
        // resize) button on the right edge of each Log and Pinned tab title.
        // The native close button is disabled on the DockArea. Keep the
        // affordance at the title's trailing edge and handle its click here.
        if matches!(tab, ViewTab::Log | ViewTab::Pinned) {
            let ctx = response.ctx.clone();
            let btn_size = 14.0;
            let btn_center =
                egui::Pos2::new(response.rect.right() - 12.0, response.rect.center().y);
            let btn_rect = egui::Rect::from_center_size(btn_center, egui::Vec2::splat(btn_size));

            let hovered = ctx
                .pointer_hover_pos()
                .is_some_and(|p| btn_rect.contains(p));

            let color = if hovered {
                self.theme.text
            } else {
                self.theme.text_muted
            };
            let painter = ctx.layer_painter(response.layer_id);
            icons::paint_icon(&ctx, &painter, Icon::WindowResize, btn_center, 12.0, color);

            // Detect clicks on the pop-out button area using raw pointer
            // input. The dock's own close button (which we cover with this
            // icon) doesn't register as `response.clicked()`, so we hit-test
            // the pointer position directly.
            if ctx.input(|i| i.pointer.any_click()) {
                if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                    if btn_rect.contains(pos) {
                        self.tab.pending_detach = Some(*tab);
                    }
                }
            }

            if hovered {
                response.clone().on_hover_text("Pop out to a new window");
            }
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
        if !matches!(active_view, ViewTab::Log | ViewTab::Pinned) {
            continue;
        }

        let header_rect = egui::Rect::from_min_max(
            egui::pos2(leaf.rect.right() - 24.0, leaf.rect.top()),
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
            Icon::WindowResize,
            header_rect.center(),
            14.0,
            color,
        );
        let response = response.on_hover_text("Pop out to a new window");
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
    fn update_main(&mut self, ui: &mut egui::Ui) {
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
        if any_search || !self.loaders.is_empty() {
            ui.ctx().request_repaint_after(Duration::from_millis(60));
        }

        // Cmd/Ctrl+F focuses the log search box of the active tab (works from
        // any dock view; the flag is consumed once the Log view renders it).
        if let Some(idx) = self.active {
            let cmd_f = ui.input_mut(|i| {
                i.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::COMMAND,
                    egui::Key::F,
                ))
            });
            if cmd_f {
                if let Some(tab) = self.tabs.get_mut(idx) {
                    tab.trigger_search_focus();
                }
            }
        }

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
            ui.horizontal(|ui| {
                ui.add(icons::app_logo(ui.ctx(), 20.0));
                ui.label(RichText::new("LOGotomoy").strong().size(16.0));
                ui.separator();
                if ui.button("Open file").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_file() {
                        self.open_file(path);
                    }
                }
                let recent_resp = ui.button("Recent").on_hover_text("Open a recently used file");
                if recent_resp.clicked() { self.recent_show_dropdown = !self.recent_show_dropdown; }
                self.recent_button_rect = Some(recent_resp.rect);

                let filter_resp = ui.button("Saved filters").on_hover_text("Apply or save a text filter");
                if filter_resp.clicked() { self.show_filter_dropdown = !self.show_filter_dropdown; }
                self.filter_button_rect = Some(filter_resp.rect);

                if let Some(idx) = self.active {
                    let tab = &mut self.tabs[idx];
                    ui.toggle_value(&mut tab.show_templates, "Templates");
                }

                if let Some(info) = self.selected_log_format_status() {
                    ui.label(RichText::new(info).small().color(self.theme.text_muted));
                }
                ui.label(RichText::new(&self.status).small().color(self.theme.text_muted));
                // Push settings to far right
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let settings_resp = ui.button("Settings").on_hover_text("Show settings");
                    if settings_resp.clicked() {
                        self.show_settings_popup = !self.show_settings_popup;
                    }
                    self.settings_button_rect = Some(settings_resp.rect);

                    // MCP controls — tied to the active log file, so they only
                    // appear when a real log tab is focused.
                    if self.active.is_some() {
                        ui.separator();
                        if self.mcp_enabled {
                            if let Some(instruction) = self.mcp_instruction() {
                                if ui.button("Copy GUI session instruction")
                                    .on_hover_text("Copy the temporary session ID and safe instructions for your coding agent")
                                    .clicked()
                                {
                                    ui.ctx().copy_text(instruction);
                                    self.show_toast(
                                        "GUI session instruction copied to clipboard".to_string(),
                                    );
                                }
                            }
                            if ui.button("Stop MCP").on_hover_text("Stop MCP server").clicked() {
                                self.stop_mcp();
                                self.show_toast("MCP server stopped".to_string());
                            }
                        } else {
                            let start_resp = ui
                                .add_enabled(!self.tabs.is_empty(), egui::Button::new("Start MCP"));
                            if self.tabs.is_empty() {
                                start_resp.clone().on_hover_text("Open a log file first");
                            }
                            if !self.tabs.is_empty() && start_resp.clicked() {
                                self.start_mcp();
                                if self.mcp_enabled {
                                    self.show_toast("MCP started. Open Settings → Integrate with AI Assistant to connect.".to_string());
                                }
                            }
                        }
                    }
                });
            });

            if !self.tabs.is_empty() || !self.loaders.is_empty() {
                ui.add_space(2.0);
                let mut tab_switch: Option<(Option<usize>, usize)> = None;
                ui.horizontal_wrapped(|ui| {
                    for (i, tab) in self.tabs.iter().enumerate() {
                        let is_active = self.active == Some(i) && self.active_loader.is_none();
                        let tab_label = if tab.mcp_serving {
                            format!("{} (MCP)", tab.doc.file_name)
                        } else {
                            tab.doc.file_name.clone()
                        };
                        if ui.selectable_label(is_active, &tab_label).clicked() {
                            let old_active = self.active;
                            info!("switched to tab {} ({})", i, tab.doc.file_name);
                            self.active = Some(i);
                            self.active_loader = None;
                            if old_active != Some(i) {
                                tab_switch = Some((old_active, i));
                            }
                        }
                        if icons::image_button(ui, Icon::Close, egui::vec2(16.0, 16.0), self.theme.text)
                            .on_hover_text("Close tab")
                            .clicked()
                        {
                            close_request = Some(i);
                        }
                        ui.separator();
                    }
                    // Loading files appear as their own (new) log tab, appended
                    // after the fully-loaded tabs. They aren't real LogTabs yet,
                    // so they're rendered straight from the loader state.
                    for (li, loader) in self.loaders.iter().enumerate() {
                        let is_active = self.active_loader == Some(li);
                        let label = format!("{} ⏳", loader.name);
                        if ui.selectable_label(is_active, &label).clicked() {
                            info!("focused loading tab {} ({})", li, loader.name);
                            self.active_loader = Some(li);
                            self.active = None;
                        }
                        if icons::image_button(ui, Icon::Close, egui::vec2(16.0, 16.0), self.theme.text)
                            .on_hover_text("Cancel load")
                            .clicked()
                        {
                            loader.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                        ui.separator();
                    }
                });
                if let Some((old, new)) = tab_switch {
                    self.on_tab_switched(old, new);
                }
            }

        });
        if let Some(i) = close_request {
            self.close_tab(i);
        }

        let templates_open = self.active.map_or(false, |i| self.tabs[i].show_templates);
        if templates_open {
            egui::Panel::right("templates_panel")
                .default_size(320.0)
                .show(ui, |ui| {
                    if let Some(idx) = self.active {
                        // Show the tab (the title + subtitle) with a close button so
                        // the Templates panel can be dismissed without toggling the
                        // top-bar button a second time.
                        let templates_open = &mut self.tabs[idx].show_templates;
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Templates").strong().size(14.0));
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(
                                    "What this file is made of. Click one to see an example.",
                                )
                                .color(self.theme.text_muted),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if icons::image_button(
                                        ui,
                                        Icon::Close,
                                        egui::vec2(16.0, 16.0),
                                        self.theme.text,
                                    )
                                    .on_hover_text("Close Templates")
                                    .clicked()
                                    {
                                        *templates_open = false;
                                    }
                                },
                            );
                        });
                        ui.separator();
                        template_browser(ui, &mut self.tabs[idx]);
                    }
                });
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
                            ui.label(RichText::new(format!("Loading {}", loader.name)).size(18.0).strong());
                            ui.add_space(8.0);
                            let pct = (loader.progress * 100.0) as u32;
                            ui.add(egui::ProgressBar::new(loader.progress)
                                .desired_width(360.0)
                                .text(format!("{} — {pct}%", loader.stage.label())));
                            ui.add_space(8.0);
                            if ui.button("Cancel").clicked() {
                                loader.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                        });
                    });
                    return;
                }
            }

            if self.loaders.is_empty() && self.tabs.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("logotomy?\n\nNothing here yet.\nDrop a .log / .txt / any text file,\nor hit Open.").size(18.0).color(self.theme.placeholder));
                });
                return;
            }

            if let Some(idx) = self.active {
                let tab = &mut self.tabs[idx];

                // Filter removal confirmation popup.
                if let Some(ki) = tab.pending_filter_removal {
                    if self.settings.skip_filter_delete_confirm {
                        // User chose "Do not ask me again": delete immediately.
                        tab.remove_filter(ki);
                        tab.pending_filter_removal = None;
                    } else {
                        let filter_text = tab.filters.get(ki).map(|k| k.text.clone()).unwrap_or_default();
                        let mut confirmed = false;
                        let mut cancelled = false;
                        egui::Window::new("Remove Filter")
                            .collapsible(false)
                            .resizable(false)
                            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                            .show(ui.ctx(), |ui| {
                                ui.label(RichText::new(format!("Remove filter '{}'?", filter_text)).size(14.0));
                                ui.add_space(10.0);

                                let mut skip = self.settings.skip_filter_delete_confirm;
                                if ui.checkbox(&mut skip, "Do not ask me again").changed() {
                                    self.settings.skip_filter_delete_confirm = skip;
                                    self.settings.save();
                                }

                                ui.add_space(10.0);
                                ui.horizontal(|ui| {
                                    if ui.button("Remove").clicked() {
                                        confirmed = true;
                                    }
                                    if ui.button("Cancel").clicked() {
                                        cancelled = true;
                                    }
                                });
                            });
                        if confirmed {
                            tab.remove_filter(ki);
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
                        egui::Window::new("Clear All Filters")
                            .collapsible(false)
                            .resizable(false)
                            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                            .show(ui.ctx(), |ui| {
                                ui.label(RichText::new(format!("Remove all {n} filters?")).size(14.0));
                                ui.add_space(10.0);

                                let mut skip = self.settings.skip_filter_delete_confirm;
                                if ui.checkbox(&mut skip, "Do not ask me again").changed() {
                                    self.settings.skip_filter_delete_confirm = skip;
                                    self.settings.save();
                                }

                                ui.add_space(10.0);
                                ui.horizontal(|ui| {
                                    if ui.button("Remove all").clicked() {
                                        confirmed = true;
                                    }
                                    if ui.button("Cancel").clicked() {
                                        cancelled = true;
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
                            ui.label(RichText::new("File changed on disk").size(20.0).color(self.theme.warning));
                            ui.add_space(8.0);
                            ui.label(RichText::new("Only file appending (tailing) is supported.").size(14.0));
                            ui.label("In-place modifications or truncation requires a full reload.");
                            ui.add_space(12.0);
                            if ui.button("Close & Reopen file").clicked() {
                                // Close via close_request mechanism
                                self.close_tab(idx);
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
                draw_dock_resize_buttons(ui, &dock_state, tab_bar_height, tab_viewer.tab, &self.theme);
                tab.dock_state = dock_state;
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
                            let row_height = 20.0;
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
                                        let resp = ui.add(
                                            egui::Label::new(
                                                RichText::new(&label).color(color).size(12.0),
                                            )
                                            .sense(egui::Sense::click()),
                                        );
                                        if resp.clicked() && exists {
                                            self.open_file(path.clone());
                                            self.recent_show_dropdown = false;
                                        }
                                        if resp.hovered() {
                                            ui.output_mut(|o| {
                                                o.cursor_icon = egui::CursorIcon::PointingHand
                                            });
                                        }
                                        if !exists {
                                            if icons::image_button(
                                                ui,
                                                Icon::Close,
                                                egui::vec2(16.0, 16.0),
                                                self.theme.text,
                                            )
                                            .on_hover_text("Remove missing file")
                                            .clicked()
                                            {
                                                remove_missing = Some(i);
                                            }
                                        }
                                    }
                                });
                            if let Some(idx) = remove_missing {
                                self.settings.recent_files.remove(idx);
                                self.settings.save();
                            }
                        }
                    });
                });
                // Close on click outside
                if ui.input(|i| i.pointer.any_click()) {
                    if let Some(click_pos) = ui.input(|i| i.pointer.interact_pos()) {
                        let on_button = button_rect.contains(click_pos);
                        let on_popup = area_resp.response.rect.contains(click_pos);
                        if !on_button && !on_popup {
                            self.recent_show_dropdown = false;
                        }
                    }
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
                "Drop it. Let's see logotomy happened",
                egui::FontId::proportional(28.0),
                Color32::WHITE,
            );
            ui.ctx().request_repaint();
        }

        if self.show_new_filter_popup {
            let mut open = self.show_new_filter_popup;
            egui::Window::new("Save New Filter")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
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
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            if !self.new_filter_name.is_empty() {
                                self.save_filter(&self.new_filter_name.clone());
                                self.show_new_filter_popup = false;
                                self.new_filter_name.clear();
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_new_filter_popup = false;
                            self.new_filter_name.clear();
                        }
                    });
                });
            if !open {
                self.show_new_filter_popup = false;
            }
        }

        if self.show_rename_filter_popup {
            let mut open = self.show_rename_filter_popup;
            egui::Window::new(format!("Rename '{}'", self.rename_filter_target))
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
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
                    ui.horizontal(|ui| {
                        if ui.button("Rename").clicked() {
                            if !self.rename_filter_new_name.is_empty() {
                                self.rename_filter(
                                    &self.rename_filter_target.clone(),
                                    &self.rename_filter_new_name.clone(),
                                );
                                self.show_rename_filter_popup = false;
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_rename_filter_popup = false;
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

        settings::show_settings_popup(ui, self);
        settings::show_integrate_popup(ui, self);
        custom_date::show_custom_date_popup(self, ui.ctx());

        // MCP error popup — show when MCP server fails to start
        if self.mcp_error_popup.is_some() {
            let mut open = true;
            let mut dismissed = false;
            let error_msg = self.mcp_error_popup.clone().unwrap_or_default();
            egui::Window::new("MCP Server Error")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.label(RichText::new(&error_msg).size(14.0));
                    ui.add_space(10.0);
                    if ui.button("OK").clicked() {
                        dismissed = true;
                    }
                });
            if !open || dismissed {
                self.mcp_error_popup = None;
            }
        }

        // Toast notification (self-dismissing, ~5s)
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

    use super::{dock_area_id, saved_filter_row_id};

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
