use eframe::egui;
use egui::{Color32, RichText};

use crate::ui::icons::{self, Icon};
use crate::ui::theme::Theme;

use crate::ui::app::model::{HaystackApp, SettingsSection};
use crate::ui::app::overlay;
use haystack::core::settings::{LogLineDisplayMode, ThemeMode};

fn section_header(ui: &mut egui::Ui, title: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(title).strong().size(13.0));
}

fn section_header_with_reset(
    ui: &mut egui::Ui,
    title: &str,
    reset_label: &str,
    reset_hint: &str,
) -> bool {
    ui.add_space(8.0);
    let mut reset = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).strong().size(13.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            reset = icons::action_button(
                ui,
                Icon::Reset,
                reset_label,
                ui.visuals().text_color(),
                reset_hint,
            )
            .clicked();
        });
    });
    reset
}

fn confirm_before_deleting(skip_confirmation: bool) -> bool {
    !skip_confirmation
}

impl SettingsSection {
    fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Parsing => "Log parsing",
            Self::Assistant => "AI Assistant",
            Self::Support => "Support",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Appearance => Icon::ThemeLight,
            Self::Parsing => Icon::Log,
            Self::Assistant => Icon::Mcp,
            Self::Support => Icon::Help,
        }
    }
}

fn settings_nav(ui: &mut egui::Ui, app: &mut HaystackApp) {
    ui.vertical(|ui| {
        ui.set_width(150.0);
        for section in [
            SettingsSection::Appearance,
            SettingsSection::Parsing,
            SettingsSection::Assistant,
            SettingsSection::Support,
        ] {
            let selected = app.settings_section == section;
            let response = settings_nav_item(ui, app, section, selected);
            if response.clicked() {
                app.settings_section = section;
            }
        }
    });
}

fn settings_nav_item(
    ui: &mut egui::Ui,
    app: &HaystackApp,
    section: SettingsSection,
    selected: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 34.0), egui::Sense::click());
    let fill = if selected {
        app.theme.selection_bg
    } else if response.hovered() {
        app.theme.hover
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect(
        rect,
        egui::CornerRadius::same(4),
        fill,
        egui::Stroke::NONE,
        egui::StrokeKind::Inside,
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(8.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.add(icons::icon_image(
        child.ctx(),
        section.icon(),
        14.0,
        if selected {
            app.theme.text
        } else {
            app.theme.text_muted
        },
    ));
    child.add_space(8.0);
    // The row owns the click target. Keep the child text non-selectable so it
    // cannot take over the pointer interaction from the row response.
    child.add(egui::Label::new(RichText::new(section.label()).color(app.theme.text)).selectable(false));
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, section.label()));
    response
}

fn show_appearance_controls(ui: &mut egui::Ui, app: &mut HaystackApp) {
    section_header(ui, "Theme");
    let system_available = ui.ctx().system_theme().is_some();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for mode in ThemeMode::ALL {
            let enabled = mode != ThemeMode::System || system_available;
            let response = ui
                .add_enabled_ui(enabled, |ui| {
                    ui.selectable_label(app.settings.theme_mode == mode, mode.label())
                })
                .inner;
            if response.clicked() {
                app.set_theme_mode(mode, ui.ctx());
            }
            if mode == ThemeMode::System {
                response.on_disabled_hover_text("System theme tracking is unavailable");
            }
        }
    });
    ui.label(
        RichText::new(if system_available {
            "System follows the operating system theme and updates open views."
        } else {
            "System theme tracking is unavailable on this platform."
        })
        .small()
        .color(app.theme.text_muted),
    );

    section_header(ui, "Preview");
    appearance_preview(ui, app);

    let reset_reading = section_header_with_reset(
        ui,
        "Reading",
        "Reset reading",
        "Restore the default long-line presentation",
    );
    let mut mode = app.settings.log_line_display_mode;
    if reset_reading {
        mode = LogLineDisplayMode::default();
    }
    egui::Grid::new("reading_settings_grid")
        .num_columns(2)
        .spacing(egui::vec2(18.0, 8.0))
        .show(ui, |ui| {
            ui.label("Long log lines");
            egui::ComboBox::from_id_salt("log_line_display_mode")
                .selected_text(mode.label())
                .show_ui(ui, |ui| {
                    for candidate in LogLineDisplayMode::ALL {
                        ui.selectable_value(&mut mode, candidate, candidate.label())
                            .on_hover_text(candidate.description());
                    }
                });
            ui.end_row();
        });
    if mode != app.settings.log_line_display_mode {
        app.settings.log_line_display_mode = mode;
        for tab in &mut app.tabs {
            tab.log_line_display_mode = mode;
        }
        app.settings.save();
    }

    let reset_filters = section_header_with_reset(
        ui,
        "Filters",
        "Reset filters",
        "Clear the default saved filter and restore deletion confirmations",
    );
    let mut filters_changed = reset_filters;
    if reset_filters {
        app.settings.default_filter = None;
        app.settings.skip_filter_delete_confirm = false;
    }
    let mut confirm = confirm_before_deleting(app.settings.skip_filter_delete_confirm);
    if ui
        .checkbox(&mut confirm, "Confirm before deleting filters")
        .changed()
    {
        app.settings.skip_filter_delete_confirm = !confirm;
        filters_changed = true;
    }
    let filter_names = app.available_filters.clone();
    egui::Grid::new("filter_settings_grid")
        .num_columns(2)
        .spacing(egui::vec2(18.0, 8.0))
        .show(ui, |ui| {
            ui.label("Default saved filter");
            let selected = app
                .settings
                .default_filter
                .clone()
                .unwrap_or_else(|| "None".into());
            egui::ComboBox::from_id_salt("default_saved_filter")
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(app.settings.default_filter.is_none(), "None")
                        .clicked()
                    {
                        app.settings.default_filter = None;
                        filters_changed = true;
                    }
                    for name in &filter_names {
                        if ui
                            .selectable_label(
                                app.settings.default_filter.as_deref() == Some(name),
                                name,
                            )
                            .clicked()
                        {
                            app.settings.default_filter = Some(name.clone());
                            filters_changed = true;
                        }
                    }
                });
            ui.end_row();
        });
    if filters_changed {
        app.settings.save();
    }
}

fn appearance_preview(ui: &mut egui::Ui, app: &HaystackApp) {
    egui::Frame::group(ui.style())
        .fill(app.theme.log_surface)
        .stroke(egui::Stroke::new(1.0, app.theme.border))
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            let rows = [
                (
                    "09:41:07",
                    "INFO",
                    "worker started; searching logs",
                    app.theme.severity_info,
                ),
                (
                    "09:41:08",
                    "WARN",
                    "retrying request for /api/events",
                    app.theme.severity_warning,
                ),
                (
                    "09:41:09",
                    "ERROR",
                    "connection timeout after 30s",
                    app.theme.severity_error,
                ),
            ];
            for (index, (time, level, message, severity)) in rows.into_iter().enumerate() {
                let fill = if index == 1 {
                    app.theme.selection_bg
                } else {
                    Color32::TRANSPARENT
                };
                egui::Frame::NONE.fill(fill).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(time).monospace().color(app.theme.timestamp));
                        ui.label(RichText::new(level).monospace().color(severity).strong());
                        if let Some(start) = message.find("request") {
                            ui.label(&message[..start]);
                            ui.label(
                                RichText::new("request")
                                    .monospace()
                                    .background_color(app.theme.search_highlight_bg),
                            );
                            ui.label(&message[start + "request".len()..]);
                        } else {
                            ui.label(message);
                        }
                    });
                });
            }
        });
}

fn show_settings_detail(ui: &mut egui::Ui, app: &mut HaystackApp) {
    match app.settings_section {
        SettingsSection::Appearance => show_appearance_controls(ui, app),
        SettingsSection::Parsing => {
            section_header(ui, "Format templates");
            if icons::action_button(
                ui,
                Icon::Log,
                "Log format templates",
                app.theme.text,
                "Define record headers and preview multiline boundaries",
            )
            .clicked()
            {
                app.record_editor.open = true;
                app.show_settings_popup = false;
            }
            section_header(ui, "Date formats");
            if icons::action_button(
                ui,
                Icon::Date,
                "Custom date formats",
                app.theme.text,
                "Add or manage user-defined date and time recognizers",
            )
            .clicked()
            {
                app.show_custom_date_popup = true;
                app.show_settings_popup = false;
            }
            ui.add_space(8.0);
            section_header(ui, "Parsing defaults");
            ui.label(
                RichText::new("These settings apply to newly opened files.")
                    .small()
                    .color(app.theme.text_muted),
            );
            ui.horizontal(|ui| {
                ui.label("Template similarity");
                let mut value = app.settings.sim_threshold as f32;
                if ui
                    .add(
                        egui::DragValue::new(&mut value)
                            .speed(0.01)
                            .range(0.3..=0.9),
                    )
                    .changed()
                {
                    app.settings.sim_threshold = (value as f64 * 100.0).round() / 100.0;
                    app.settings.save();
                }
            });
            ui.horizontal(|ui| {
                ui.label("Header sample lines");
                let mut value = app.settings.header_sample_lines as u32;
                if ui
                    .add(egui::DragValue::new(&mut value).speed(1).range(0..=2000))
                    .changed()
                {
                    app.settings.header_sample_lines = value as usize;
                    app.settings.save();
                }
            });
            ui.horizontal(|ui| {
                ui.label("Template tree depth");
                let mut value = app.settings.drain_depth as u32;
                if ui
                    .add(egui::DragValue::new(&mut value).speed(1).range(3..=8))
                    .changed()
                {
                    app.settings.drain_depth = value as usize;
                    app.settings.save();
                }
            });
        }
        SettingsSection::Assistant => {
            section_header(ui, "Connection");
            ui.label(if app.mcp_enabled {
                "Ready for a private GUI connection"
            } else if app.tabs.is_empty() {
                "Open a log before starting the connection"
            } else {
                "Connection is stopped"
            });
            ui.horizontal_wrapped(|ui| {
                if app.mcp_enabled {
                    if icons::action_button(
                        ui,
                        Icon::Stop,
                        "Stop connection",
                        app.theme.text,
                        "Stop MCP and invalidate the temporary GUI session",
                    )
                    .clicked()
                    {
                        app.stop_mcp();
                        app.show_toast("AI Assistant connection stopped".into());
                    }
                    if let Some(instruction) = app.mcp_instruction() {
                        if icons::action_button(
                            ui,
                            Icon::Copy,
                            "Copy session",
                            app.theme.text,
                            "Copy temporary session instructions for your coding agent",
                        )
                        .clicked()
                        {
                            ui.ctx().copy_text(instruction);
                            app.show_toast("GUI session instructions copied to clipboard".into());
                        }
                    }
                } else if icons::action_button_enabled(
                    ui,
                    !app.tabs.is_empty(),
                    Icon::Start,
                    "Start connection",
                    app.theme.text,
                    "Start a private MCP connection for the active log",
                )
                .clicked()
                {
                    app.start_mcp();
                }
                if icons::action_button(
                    ui,
                    Icon::Integrate,
                    "Integration guide",
                    app.theme.text,
                    "Set up Haystack in Codex, Claude, or Cline",
                )
                .clicked()
                {
                    app.show_integrate_popup = true;
                    app.show_settings_popup = false;
                }
            });
        }
        SettingsSection::Support => {
            section_header(ui, "Resources");
            if icons::action_button(
                ui,
                Icon::Bug,
                "Report a bug",
                app.theme.text,
                "Open the Haystack issue tracker",
            )
            .clicked()
            {
                open_url("https://github.com/muntasir-kabir/haystack/issues");
                app.show_settings_popup = false;
            }
            if icons::action_button(
                ui,
                Icon::Info,
                "About",
                app.theme.text,
                "Open the Haystack project page",
            )
            .clicked()
            {
                open_url("https://github.com/muntasir-kabir/haystack");
                app.show_settings_popup = false;
            }
        }
    }
}

/// Show the settings window with navigation on the left and controls on the right.
pub fn show_settings_popup(ui: &mut egui::Ui, app: &mut HaystackApp) {
    if !app.show_settings_popup {
        return;
    }
    let response =
        overlay::modal_without_title(ui.ctx(), "settings_modal", egui::vec2(760.0, 560.0), |ui| {
            ui.horizontal(|ui| {
                ui.heading("Settings");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icons::action_button(
                        ui,
                        Icon::Close,
                        "Close",
                        app.theme.text,
                        "Close settings",
                    )
                    .clicked()
                    {
                        app.show_settings_popup = false;
                    }
                });
            });
            ui.add_space(4.0);
            let body_size = ui.available_size();
            ui.allocate_ui_with_layout(
                body_size,
                egui::Layout::left_to_right(egui::Align::Min),
                |ui| {
                    settings_nav(ui, app);
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .id_salt("settings_detail_scroll")
                        .auto_shrink([false, false])
                        .max_height(ui.available_height())
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.set_width(ui.available_width().max(280.0));
                                show_settings_detail(ui, app);
                            });
                        });
                },
            );
        });
    if response.backdrop_response.clicked() {
        app.show_settings_popup = false;
    }
}

/// Show the integrate guide modal window.
pub fn show_integrate_popup(ui: &mut egui::Ui, app: &mut HaystackApp) {
    if !app.show_integrate_popup {
        return;
    }
    overlay::modal(
        ui.ctx(),
        "integrate_modal",
        "Integrate with AI coding agents",
        egui::vec2(640.0, 560.0),
        |ui| {
            ui.label(
                RichText::new("Connect a local AI coding agent to Haystack through MCP.")
                    .small()
                    .color(app.theme.text_muted),
            );
            ui.label(RichText::new("Configure one stdio server once, then use it standalone or with a temporary GUI session.").small().color(app.theme.text_muted));
            ui.separator();

            let exe_path = std::env::current_exe()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| "/path/to/haystack".to_string());
            let server_json = mcp_server_json(&exe_path);
            let cline_server_json = mcp_cline_server_json(&exe_path);
            let codex_server_toml = mcp_codex_server_toml(&exe_path);
            let claude_code_command = mcp_claude_code_command(&exe_path);
            let setup_prompt = mcp_agent_setup_prompt(&exe_path);

            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.add_space(4.0);
                integration_section_header(ui, "Recommended setup");
                ui.label(RichText::new("Paste this into any local coding agent. It configures Haystack in that client's user/global MCP settings.").small().color(app.theme.text_muted));
                agent_section(ui, &app.theme, "Agent setup prompt",
                    "Approve the configuration change if asked, then reload the agent's MCP servers:",
                    "", &setup_prompt);

                ui.separator();
                integration_section_header(ui, "Manual setup");
                ui.label(RichText::new("Choose your client and copy its native user-level configuration.").small().color(app.theme.text_muted));
                ui.add_space(6.0);
                agent_section(ui, &app.theme, "Codex (CLI, desktop, VS Code)",
                    "Add to the user configuration so Haystack is available across projects:",
                    "~/.codex/config.toml", &codex_server_toml);
                agent_section(ui, &app.theme, "Claude Desktop",
                    "Open Settings → Developer → Edit Config and merge this entry:",
                    claude_desktop_config_path(), &server_json);
                agent_section(ui, &app.theme, "Claude Code (CLI and VS Code)",
                    "Run once with user scope so Haystack is available across projects:", "",
                    &claude_code_command);
                agent_section(ui, &app.theme, "Cline (CLI and VS Code)",
                    "Add to Cline's user MCP settings so Haystack is available across projects:",
                    "~/.cline/mcp.json", &cline_server_json);

                ui.separator();
                integration_section_header(ui, "Use Haystack");
                ui.label(RichText::new("Standalone").strong());
                ui.label(RichText::new("Ask the agent to call `load_log`, then use the returned `log_id`.").small().color(app.theme.text_muted));
                ui.add_space(4.0);
                ui.label(RichText::new("Interactive GUI session").strong());
                ui.label(RichText::new("Open and select a log, start MCP, then copy the GUI session instruction to the agent. It attaches to the selected tab without changing its MCP configuration.").small().color(app.theme.text_muted));
                ui.add_space(10.0);
                ui.separator();
                if icons::action_button(
                    ui,
                    Icon::Close,
                    "Close",
                    app.theme.text,
                    "Close the integration guide",
                )
                .clicked()
                {
                    app.show_integrate_popup = false;
                }
            });
        },
    );
}

fn mcp_agent_setup_prompt(exe_path: &str) -> String {
    format!(
        "Set up Haystack as a user/global MCP server for this client. Preserve existing MCP \
servers and use this client's native MCP configuration format.\n\n\
Transport: stdio\n\
command: \"{exe_path}\"\n\
args: [\"--mcp\"]\n\n\
Reload MCP servers and verify `session_info` is available. If you cannot update the user \
configuration, tell me the required file or UI step."
    )
}

fn integration_section_header(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).strong().size(14.0));
    ui.add_space(2.0);
}

/// Stable stdio configuration shared by standalone and GUI-attached use.
fn mcp_server_json(exe_path: &str) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": {
            "haystack": { "command": exe_path, "args": ["--mcp"] }
        }
    }))
    .unwrap_or_default()
}

fn mcp_cline_server_json(exe_path: &str) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": {
            "haystack": {
                "command": exe_path,
                "args": ["--mcp"],
                "disabled": false,
                "autoApprove": []
            }
        }
    }))
    .unwrap_or_default()
}

fn mcp_codex_server_toml(exe_path: &str) -> String {
    // JSON string escaping is compatible with TOML basic strings for paths and
    // gives us a safe quoted command on every supported platform.
    let command = serde_json::to_string(exe_path).unwrap_or_else(|_| "\"haystack\"".to_string());
    format!(
        "[mcp_servers.haystack]\ncommand = {command}\nargs = [\"--mcp\"]\nstartup_timeout_sec = 20"
    )
}

fn mcp_claude_code_command(exe_path: &str) -> String {
    format!(
        "claude mcp add --scope user haystack -- {} --mcp",
        shell_quote(exe_path)
    )
}

#[cfg(target_os = "macos")]
fn claude_desktop_config_path() -> &'static str {
    "~/Library/Application Support/Claude/claude_desktop_config.json"
}

#[cfg(target_os = "windows")]
fn claude_desktop_config_path() -> &'static str {
    "%APPDATA%\\Claude\\claude_desktop_config.json"
}

#[cfg(target_os = "linux")]
fn claude_desktop_config_path() -> &'static str {
    "~/.config/Claude/claude_desktop_config.json"
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn claude_desktop_config_path() -> &'static str {
    "Claude Desktop → Settings → Developer → Edit Config"
}

/// Quote a local executable for the user's shell when generating a Claude
/// Code command. The JSON-based integrations use serializers above instead.
#[cfg(not(target_os = "windows"))]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(target_os = "windows")]
fn shell_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\\\""))
}

fn agent_section(
    ui: &mut egui::Ui,
    theme: &Theme,
    agent: &str,
    instruction: &str,
    file_name: &str,
    code: &str,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(agent).strong().size(13.0));
        if icons::action_button(
            ui,
            Icon::Copy,
            "Copy",
            theme.text,
            format!("Copy configuration for {agent}"),
        )
        .clicked()
        {
            ui.ctx().copy_text(code.to_string());
        }
    });
    ui.label(RichText::new(instruction).small().color(theme.text_muted));
    if !file_name.is_empty() {
        ui.label(
            RichText::new(file_name)
                .monospace()
                .size(10.0)
                .color(theme.url_text),
        );
    }
    let code_lines: Vec<&str> = code.lines().collect();
    if code_lines.len() > 2 {
        let max_display_lines = 10usize.min(code_lines.len());
        let display_text: String = code_lines[..max_display_lines].join("\n");
        // Theme-aware code block background
        let code_bg = if theme.bg.r() > 128 {
            Color32::from_rgb(230, 230, 225) // light mode
        } else {
            Color32::from_rgb(35, 35, 45) // dark mode
        };
        let code_text_color = if theme.bg.r() > 128 {
            Color32::from_rgb(30, 30, 30)
        } else {
            Color32::from_rgb(200, 200, 200)
        };
        egui::Frame::default()
            .fill(code_bg)
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(&display_text)
                            .monospace()
                            .size(10.0)
                            .color(code_text_color),
                    )
                    .sense(egui::Sense::click()),
                );
            });
    } else {
        ui.add(
            egui::Label::new(
                RichText::new(code)
                    .monospace()
                    .size(10.0)
                    .color(theme.url_text),
            )
            .sense(egui::Sense::click()),
        );
    }
    ui.add_space(8.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_label_maps_to_existing_skip_setting() {
        assert!(confirm_before_deleting(false));
        assert!(!confirm_before_deleting(true));
        for skip in [false, true] {
            assert_eq!(!confirm_before_deleting(skip), skip);
        }
    }

    #[test]
    fn generated_json_configs_escape_executable_paths() {
        let path = if cfg!(windows) {
            r#"C:\Program Files\Haystack\haystack.exe"#
        } else {
            "/Applications/Haystack\" nightly/haystack"
        };
        for config in [mcp_server_json(path), mcp_cline_server_json(path)] {
            let parsed: serde_json::Value = serde_json::from_str(&config).unwrap();
            let command = parsed
                .pointer("/mcpServers/haystack/command")
                .and_then(serde_json::Value::as_str);
            assert_eq!(command, Some(path));
        }
    }

    #[test]
    fn generated_agent_configs_use_expected_transport_shapes() {
        let path = "/tmp/haystack";
        let cline: serde_json::Value = serde_json::from_str(&mcp_cline_server_json(path)).unwrap();
        assert_eq!(cline["mcpServers"]["haystack"]["args"][0], "--mcp");

        let codex = mcp_codex_server_toml(path);
        assert!(codex.contains("[mcp_servers.haystack]"));
        assert!(codex.contains("command = \"/tmp/haystack\""));
        assert!(codex.contains("args = [\"--mcp\"]"));
        assert!(!codex.contains("http"));
        assert!(!codex.contains("Authorization"));
        assert!(!codex.contains("--mcp-gui"));

        let claude = mcp_claude_code_command(path);
        assert!(claude.contains("--scope user"));
        assert!(claude.ends_with("--mcp"));
    }

    #[test]
    fn agent_setup_prompt_uses_dynamic_path_and_safe_global_stdio_config() {
        let prompt = mcp_agent_setup_prompt("/Applications/Haystack.app/Contents/MacOS/haystack");
        assert!(prompt.contains("user/global MCP server"));
        assert!(prompt.contains("/Applications/Haystack.app/Contents/MacOS/haystack"));
        assert!(prompt.contains("args: [\"--mcp\"]"));
        assert!(prompt.contains("Preserve existing MCP servers"));
        assert!(prompt.contains("Transport: stdio"));
        assert!(prompt.contains("session_info"));
        assert!(prompt.contains("required file or UI step"));
        assert!(!prompt.contains("http://"));
    }
}

/// Open a URL in the system default browser (cross-platform, no extra deps).
fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}
