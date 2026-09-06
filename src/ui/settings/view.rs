use eframe::egui;
use egui::{Color32, RichText};

use crate::ui::icons::{self, Icon};
use crate::ui::theme::Theme;

use crate::ui::app::model::LogotomyApp;
use logotomy::core::settings::LogLineDisplayMode;

fn section_header(ui: &mut egui::Ui, icon: Icon, title: &str, theme: &Theme) {
    ui.horizontal(|ui| {
        ui.add(icons::icon_image(ui.ctx(), icon, 15.0, theme.text));
        ui.label(RichText::new(title).strong().size(14.0));
    });
}

fn confirm_before_deleting(skip_confirmation: bool) -> bool {
    !skip_confirmation
}

/// Show the settings popup, grouped by everyday task rather than implementation detail.
pub fn show_settings_popup(ui: &mut egui::Ui, app: &mut LogotomyApp) {
    if !app.show_settings_popup {
        return;
    }
    let Some(button_rect) = app.settings_button_rect else {
        return;
    };

    let popup_width = 390.0;
    let area_resp = egui::Area::new(egui::Id::new("settings_popup"))
        .order(egui::Order::Foreground)
        .fixed_pos(button_rect.right_bottom() - egui::vec2(popup_width, 0.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(popup_width);
                egui::ScrollArea::vertical()
                    .max_height((ui.ctx().content_rect().height() - 80.0).max(260.0))
                    .show(ui, |ui| {
                        section_header(ui, Icon::Settings, "Settings", &app.theme);
                        ui.separator();

                        section_header(ui, Icon::ThemeLight, "Appearance", &app.theme);
                        let mut dark_mode = app.dark_mode;
                        if ui.checkbox(&mut dark_mode, "Use dark mode").changed() {
                            app.toggle_theme();
                        }

                        ui.add_space(8.0);
                        section_header(ui, Icon::Check, "Behavior", &app.theme);
                        let mut confirm = confirm_before_deleting(
                            app.settings.skip_filter_delete_confirm,
                        );
                        if ui
                            .checkbox(&mut confirm, "Confirm before deleting filters")
                            .on_hover_text("Ask before removing one filter or clearing all filters")
                            .changed()
                        {
                            app.settings.skip_filter_delete_confirm = !confirm;
                            app.settings.save();
                        }
                        ui.horizontal(|ui| {
                            ui.label("Long log lines");
                            let mut mode = app.settings.log_line_display_mode;
                            egui::ComboBox::from_id_salt("log_line_display_mode")
                                .selected_text(mode.label())
                                .show_ui(ui, |ui| {
                                    for candidate in LogLineDisplayMode::ALL {
                                        ui.selectable_value(&mut mode, candidate, candidate.label())
                                            .on_hover_text(candidate.description());
                                    }
                                });
                            if mode != app.settings.log_line_display_mode {
                                app.settings.log_line_display_mode = mode;
                                for tab in &mut app.tabs {
                                    tab.log_line_display_mode = mode;
                                }
                                app.settings.save();
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label("Default saved filter");
                            let selected = app
                                .settings
                                .default_filter
                                .clone()
                                .unwrap_or_else(|| "None".to_string());
                            egui::ComboBox::from_id_salt("default_saved_filter")
                                .selected_text(selected)
                                .show_ui(ui, |ui| {
                                    if ui
                                        .selectable_label(app.settings.default_filter.is_none(), "None")
                                        .clicked()
                                    {
                                        app.settings.default_filter = None;
                                        app.settings.save();
                                    }
                                    for filter_name in &app.available_filters {
                                        if ui
                                            .selectable_label(
                                                app.settings.default_filter.as_deref()
                                                    == Some(filter_name),
                                                filter_name,
                                            )
                                            .clicked()
                                        {
                                            app.settings.default_filter = Some(filter_name.clone());
                                            app.settings.save();
                                        }
                                    }
                                });
                        });

                        ui.separator();
                        section_header(ui, Icon::Log, "Log parsing", &app.theme);
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

                        egui::CollapsingHeader::new("Advanced parsing")
                            .id_salt("advanced_parsing_settings")
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new("These settings apply to newly opened files.")
                                        .small()
                                        .color(app.theme.text_muted),
                                );
                                ui.horizontal(|ui| {
                                    ui.label("Template similarity");
                                    let mut similarity = app.settings.sim_threshold as f32;
                                    if ui
                                        .add(
                                            egui::DragValue::new(&mut similarity)
                                                .speed(0.01)
                                                .range(0.3..=0.9),
                                        )
                                        .on_hover_text(
                                            "Higher values create more precise, more numerous templates",
                                        )
                                        .changed()
                                    {
                                        app.settings.sim_threshold =
                                            (similarity as f64 * 100.0).round() / 100.0;
                                        app.settings.save();
                                    }
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Header sample lines");
                                    let mut lines = app.settings.header_sample_lines as u32;
                                    if ui
                                        .add(
                                            egui::DragValue::new(&mut lines)
                                                .speed(1)
                                                .range(0..=2000),
                                        )
                                        .on_hover_text(
                                            "Leading lines used to learn recurring header fields; 0 disables it",
                                        )
                                        .changed()
                                    {
                                        app.settings.header_sample_lines = lines as usize;
                                        app.settings.save();
                                    }
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Template tree depth");
                                    let mut depth = app.settings.drain_depth as u32;
                                    if ui
                                        .add(
                                            egui::DragValue::new(&mut depth)
                                                .speed(1)
                                                .range(3..=8),
                                        )
                                        .on_hover_text(
                                            "Higher values route templates more precisely but can fragment groups",
                                        )
                                        .changed()
                                    {
                                        app.settings.drain_depth = depth as usize;
                                        app.settings.save();
                                    }
                                });
                            });

                        ui.separator();
                        section_header(ui, Icon::Mcp, "AI Assistant", &app.theme);
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
                                    app.show_toast("AI Assistant connection stopped".to_string());
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
                                        app.show_toast(
                                            "GUI session instructions copied to clipboard".to_string(),
                                        );
                                    }
                                }
                            } else if icons::action_button_enabled(
                                ui,
                                !app.tabs.is_empty(),
                                Icon::Start,
                                "Start connection",
                                app.theme.text,
                                if app.tabs.is_empty() {
                                    "Open a log file first"
                                } else {
                                    "Start a private MCP connection for the active log"
                                },
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
                                "Set up Logotomy in Codex, Claude, or Cline",
                            )
                            .clicked()
                            {
                                app.show_integrate_popup = true;
                                app.show_settings_popup = false;
                            }
                        });

                        ui.separator();
                        section_header(ui, Icon::Help, "Support", &app.theme);
                        ui.horizontal_wrapped(|ui| {
                            if icons::action_button(
                                ui,
                                Icon::Bug,
                                "Report a bug",
                                app.theme.text,
                                "Open the Logotomy issue tracker",
                            )
                            .clicked()
                            {
                                open_url("https://github.com/muntasir-kabir/logotomy/issues");
                                app.show_settings_popup = false;
                            }
                            if icons::action_button(
                                ui,
                                Icon::Info,
                                "About",
                                app.theme.text,
                                "Open the Logotomy project page",
                            )
                            .clicked()
                            {
                                open_url("https://github.com/muntasir-kabir/logotomy");
                                app.show_settings_popup = false;
                            }
                        });
                    });
            });
        });

    let escape = ui.input(|input| input.key_pressed(egui::Key::Escape));
    let outside_click = ui.input(|input| {
        input
            .pointer
            .any_click()
            .then(|| input.pointer.interact_pos())
            .flatten()
            .is_some_and(|position| {
                !button_rect.contains(position) && !area_resp.response.rect.contains(position)
            })
    });
    if escape || outside_click {
        app.show_settings_popup = false;
    }
}

/// Show the integrate guide modal window.
pub fn show_integrate_popup(ui: &mut egui::Ui, app: &mut LogotomyApp) {
    if !app.show_integrate_popup {
        return;
    }
    let mut open = app.show_integrate_popup;
    egui::Window::new("Integrate with AI coding agents")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size([640.0, 560.0])
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ui.ctx(), |ui| {
            ui.label(RichText::new("Connect a local AI coding agent to Logotomy through MCP.").small().color(app.theme.text_muted));
            ui.label(RichText::new("Configure one stdio server once, then use it standalone or with a temporary GUI session.").small().color(app.theme.text_muted));
            ui.separator();

            let exe_path = std::env::current_exe()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| "/path/to/logotomy".to_string());
            let server_json = mcp_server_json(&exe_path);
            let cline_server_json = mcp_cline_server_json(&exe_path);
            let codex_server_toml = mcp_codex_server_toml(&exe_path);
            let claude_code_command = mcp_claude_code_command(&exe_path);
            let setup_prompt = mcp_agent_setup_prompt(&exe_path);

            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.add_space(4.0);
                integration_section_header(ui, "Recommended setup");
                ui.label(RichText::new("Paste this into any local coding agent. It configures Logotomy in that client's user/global MCP settings.").small().color(app.theme.text_muted));
                agent_section(ui, &app.theme, "Agent setup prompt",
                    "Approve the configuration change if asked, then reload the agent's MCP servers:",
                    "", &setup_prompt);

                ui.separator();
                integration_section_header(ui, "Manual setup");
                ui.label(RichText::new("Choose your client and copy its native user-level configuration.").small().color(app.theme.text_muted));
                ui.add_space(6.0);
                agent_section(ui, &app.theme, "Codex (CLI, desktop, VS Code)",
                    "Add to the user configuration so Logotomy is available across projects:",
                    "~/.codex/config.toml", &codex_server_toml);
                agent_section(ui, &app.theme, "Claude Desktop",
                    "Open Settings → Developer → Edit Config and merge this entry:",
                    claude_desktop_config_path(), &server_json);
                agent_section(ui, &app.theme, "Claude Code (CLI and VS Code)",
                    "Run once with user scope so Logotomy is available across projects:", "",
                    &claude_code_command);
                agent_section(ui, &app.theme, "Cline (CLI and VS Code)",
                    "Add to Cline's user MCP settings so Logotomy is available across projects:",
                    "~/.cline/mcp.json", &cline_server_json);

                ui.separator();
                integration_section_header(ui, "Use Logotomy");
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
        });
    if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
        open = false;
    }
    if !open {
        app.show_integrate_popup = false;
    }
}

fn mcp_agent_setup_prompt(exe_path: &str) -> String {
    format!(
        "Set up Logotomy as a user/global MCP server for this client. Preserve existing MCP \
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
            "logotomy": { "command": exe_path, "args": ["--mcp"] }
        }
    }))
    .unwrap_or_default()
}

fn mcp_cline_server_json(exe_path: &str) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": {
            "logotomy": {
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
    let command = serde_json::to_string(exe_path).unwrap_or_else(|_| "\"logotomy\"".to_string());
    format!(
        "[mcp_servers.logotomy]\ncommand = {command}\nargs = [\"--mcp\"]\nstartup_timeout_sec = 20"
    )
}

fn mcp_claude_code_command(exe_path: &str) -> String {
    format!(
        "claude mcp add --scope user logotomy -- {} --mcp",
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
            r#"C:\Program Files\Logotomy\logotomy.exe"#
        } else {
            "/Applications/Logotomy\" nightly/logotomy"
        };
        for config in [mcp_server_json(path), mcp_cline_server_json(path)] {
            let parsed: serde_json::Value = serde_json::from_str(&config).unwrap();
            let command = parsed
                .pointer("/mcpServers/logotomy/command")
                .and_then(serde_json::Value::as_str);
            assert_eq!(command, Some(path));
        }
    }

    #[test]
    fn generated_agent_configs_use_expected_transport_shapes() {
        let path = "/tmp/logotomy";
        let cline: serde_json::Value = serde_json::from_str(&mcp_cline_server_json(path)).unwrap();
        assert_eq!(cline["mcpServers"]["logotomy"]["args"][0], "--mcp");

        let codex = mcp_codex_server_toml(path);
        assert!(codex.contains("[mcp_servers.logotomy]"));
        assert!(codex.contains("command = \"/tmp/logotomy\""));
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
        let prompt = mcp_agent_setup_prompt("/Applications/Logotomy.app/Contents/MacOS/logotomy");
        assert!(prompt.contains("user/global MCP server"));
        assert!(prompt.contains("/Applications/Logotomy.app/Contents/MacOS/logotomy"));
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
