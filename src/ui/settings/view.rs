use std::time::Duration;

use eframe::egui;
use egui::{Color32, RichText};

use crate::ui::icons::{self, Icon};
use crate::ui::theme::Theme;

use crate::ui::app::model::LogotomyApp;

/// Show the settings popup (Area-based, anchored to the settings button).
pub fn show_settings_popup(ui: &mut egui::Ui, app: &mut LogotomyApp) {
    if !app.show_settings_popup {
        return;
    }
    let Some(button_rect) = app.settings_button_rect else {
        return;
    };

    let popup_id = egui::Id::new("settings_popup");
    let area = egui::Area::new(popup_id)
        .current_pos(button_rect.left_bottom())
        .order(egui::Order::Foreground)
        .fixed_pos(button_rect.left_bottom());
    let area_resp = area.show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_min_width(300.0);
            ui.set_max_width(400.0);
            ui.horizontal(|ui| {
                let ctx = ui.ctx().clone();
                ui.add(icons::icon_image(&ctx, Icon::Settings, 14.0, app.theme.text));
                ui.label(RichText::new("Settings").strong().size(14.0));
            });
            ui.separator();

            // Dark mode
            let mut dark_mode = app.dark_mode;
            if ui.checkbox(&mut dark_mode, "Dark mode").clicked() {
                app.toggle_theme();
            }

            // Filter deletion confirmations
            let mut skip_confirm = app.settings.skip_filter_delete_confirm;
            if ui
                .checkbox(&mut skip_confirm, "Do not ask before deleting a filter")
                .on_hover_text("Deletes the filter (trash icon / 'Clear all filters') immediately, without the confirmation popup.")
                .changed()
            {
                app.settings.skip_filter_delete_confirm = skip_confirm;
                app.settings.save();
            }

            // Default template
            ui.horizontal(|ui| {
                ui.label("Default filter:");
                let selected_template = app.settings.default_filter.clone().unwrap_or_else(|| "None".to_string());
                egui::ComboBox::from_label("")
                    .selected_text(selected_template)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(app.settings.default_filter.is_none(), "None").clicked() {
                            app.settings.default_filter = None;
                            app.settings.save();
                        }
                        for filter_name in &app.available_filters {
                            if ui.selectable_label(app.settings.default_filter.as_deref() == Some(filter_name), filter_name).clicked() {
                                app.settings.default_filter = Some(filter_name.clone());
                                app.settings.save();
                            }
                        }
                    });
            });

            ui.separator();
            ui.horizontal(|ui| {
                let ctx = ui.ctx().clone();
                ui.add(icons::icon_image(&ctx, Icon::Log, 14.0, app.theme.text));
                ui.label(RichText::new("Log Parsing").strong().size(14.0));
            });
            ui.add_space(2.0);

            // Custom date recognizers (opened from Settings; was in the top bar)
            ui.horizontal(|ui| {
                let ctx = ui.ctx().clone();
                ui.add(icons::icon_image(&ctx, Icon::Date, 14.0, app.theme.text));
                if ui.button("Custom date recognizers")
                    .on_hover_text("Add / manage user-defined date/time recognizers")
                    .clicked()
                {
                    app.show_custom_date_popup = !app.show_custom_date_popup;
                }
            });

            // Drain similarity threshold
            ui.horizontal(|ui| {
                ui.label("Similarity threshold:");
                let mut sim = app.settings.sim_threshold as f32;
                if ui.add(egui::DragValue::new(&mut sim).speed(0.01).range(0.3..=0.9)).changed() {
                    app.settings.sim_threshold = (sim as f64 * 100.0).round() / 100.0;
                    app.settings.save();
                }
            }).response.on_hover_text("Drain template merge threshold (0.3–0.9). Higher = stricter clustering, more templates. Applies to newly opened files.");

            // Header sample size
            ui.horizontal(|ui| {
                ui.label("Header sample lines:");
                let mut n = app.settings.header_sample_lines as u32;
                if ui.add(egui::DragValue::new(&mut n).speed(1).range(0..=2000)).changed() {
                    app.settings.header_sample_lines = n as usize;
                    app.settings.save();
                }
            }).response.on_hover_text("Leading lines sampled to learn the common log header (host/pid/thread slots). 0 disables header learning. Applies to newly opened files.");

            // Drain depth
            ui.horizontal(|ui| {
                ui.label("Drain depth:");
                let mut depth = app.settings.drain_depth as u32;
                if ui.add(egui::DragValue::new(&mut depth).speed(1).range(3..=8)).changed() {
                    app.settings.drain_depth = depth as usize;
                    app.settings.save();
                }
            }).response.on_hover_text("Drain parse-tree depth. Depth N = token count + (N-2) routing tokens. Higher = more precise routing but higher fragmentation risk. Default: 4. Applies to newly opened files.");

            ui.separator();
            ui.horizontal(|ui| {
                let ctx = ui.ctx().clone();
                ui.add(icons::icon_image(&ctx, Icon::Mcp, 14.0, app.theme.text));
                ui.label(RichText::new("MCP Server").strong().size(14.0));
            });
            ui.add_space(2.0);

            // Status indicator: green circle with pulse animation when running, gray when stopped
            let tooltip = if app.mcp_enabled {
                "Private GUI session ready for an attached agent".to_string()
            } else {
                "MCP not running".to_string()
            };
            ui.horizontal(|ui| {
                let color = if app.mcp_enabled {
                    let is_active = app.mcp_started_at.map(|t| t.elapsed() < Duration::from_secs(10)).unwrap_or(false);
                    let alpha = if is_active {
                        let t = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64();
                        let pulse = ((t * 2.0 * std::f64::consts::PI / 1.2).sin() * 0.3 + 0.7).clamp(0.4, 1.0);
                        (pulse * 255.0) as u8
                    } else { 255 };
                    let (gr, gg, gb) = if app.dark_mode { (0, 255, 0) } else { (0, 160, 0) };
                    Color32::from_rgba_unmultiplied(gr, gg, gb, alpha)
                } else {
                    app.theme.status_grey
                };
                ui.add(egui::Label::new(RichText::new("●").color(color).size(14.0)));
                ui.label(if app.mcp_enabled { "Running · private GUI session" } else { "Stopped" });
            }).response.on_hover_text(tooltip);

            // Start / Stop buttons
            if app.mcp_enabled {
                // Start is disabled while a server is already running.
                ui.horizontal(|ui| {
                    ui.add_enabled(false, egui::Button::new("Start MCP Server"));
                    ui.label(RichText::new("MCP already running").small().color(app.theme.text_muted));
                });
                if ui.button("Stop MCP Server").clicked() {
                    app.stop_mcp();
                    app.show_toast("MCP server stopped".to_string());
                    app.show_settings_popup = false;
                }
            } else {
                let is_disabled = app.tabs.is_empty();
                let start_resp = ui.add_enabled(!is_disabled, egui::Button::new("Start MCP Server"));
                if is_disabled {
                    start_resp.clone().on_hover_text("Open a log file first");
                }
                if start_resp.clicked() {
                    app.start_mcp();
                    if app.mcp_enabled {
                        app.show_toast(
                            "MCP started. Use Integrate with AI Assistant to connect.".to_string(),
                        );
                    }
                    app.show_settings_popup = false;
                }
            }

            // Prompt AI assistant section
            ui.add_space(4.0);
            if let Some(instruction) = app.mcp_instruction() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("GUI session instruction").strong().size(13.0));
                    if ui.button("Copy").on_hover_text("Copy temporary session ID and agent instructions").clicked() {
                        ui.ctx().copy_text(instruction);
                    }
                });
                ui.label(RichText::new("Configure `logotomy --mcp` once, then copy this temporary session instruction to the agent. The session ID is secret and must never be saved in MCP settings.").small().color(app.theme.text_muted));
            } else {
                ui.label(RichText::new("Start MCP to connect AI assistant").strong().size(13.0).color(app.theme.text_muted));
            }

            ui.separator();

            // Integrate with AI Assistant button
            if ui.button("Integrate with AI Assistant").clicked() {
                app.show_integrate_popup = true;
            }

            ui.separator();

            // Report Bug — opens the project's issue tracker in the browser.
            if ui.button("Report Bug")
                .on_hover_text("Open https://github.com/muntasir-kabir/logotomy/issues")
                .clicked()
            {
                open_url("https://github.com/muntasir-kabir/logotomy/issues");
            }

            // About — opens the project's GitHub page in the browser.
            if ui.button("About")
                .on_hover_text("Open https://github.com/muntasir-kabir/logotomy")
                .clicked()
            {
                open_url("https://github.com/muntasir-kabir/logotomy");
            }
        });
    });

    // Close on click outside
    if ui.input(|i| i.pointer.any_click()) {
        if let Some(click_pos) = ui.input(|i| i.pointer.interact_pos()) {
            let on_button = button_rect.contains(click_pos);
            let on_popup = area_resp.response.rect.contains(click_pos);
            if !on_button && !on_popup {
                app.show_settings_popup = false;
            }
        }
    }
}

/// Show the integrate guide modal window.
pub fn show_integrate_popup(ui: &mut egui::Ui, app: &mut LogotomyApp) {
    if !app.show_integrate_popup {
        return;
    }
    let mut open = app.show_integrate_popup;
    egui::Window::new("Integrate with AI Coding Agents")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size([560.0, 480.0])
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ui.ctx(), |ui| {
            ui.label(RichText::new("Connect Logotomy to Codex, Claude, or Cline via MCP.").small().color(app.theme.text_muted));
            ui.label(RichText::new("Configure one stdio server once. It works standalone and can attach to a temporary GUI session when you provide a session ID.").small().color(app.theme.text_muted));
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
                ui.label(RichText::new("First, try the simple agent-assisted setup").strong());
                ui.label(RichText::new("Copy this prompt into your coding agent. It asks the agent to add Logotomy to its global/user MCP configuration using the correct format for that client.").small().color(app.theme.text_muted));
                agent_section(ui, &app.theme, "Agent setup prompt",
                    "Paste into the agent, approve the configuration change if asked, then reload the agent's MCP servers:",
                    "", &setup_prompt);

                ui.separator();
                ui.label(RichText::new("If agent setup fails, add it manually").strong());
                ui.label(RichText::new("Choose your client below and copy its native configuration.").small().color(app.theme.text_muted));
                ui.add_space(6.0);
                ui.label(RichText::new("One MCP server (configure once)").strong());
                ui.label(RichText::new("This permanent configuration contains no session credential. Without attachment it opens logs independently; after `attach_gui_session` it operates on the log selected in the GUI.").small().color(app.theme.text_muted));
                ui.add_space(6.0);
                agent_section(ui, &app.theme, "Codex CLI, ChatGPT app, VS Code",
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
                ui.label(RichText::new("Using the same server").strong());
                ui.label(RichText::new("Standalone: ask the agent to call `load_log`, then use the returned `log_id`. Live GUI: start MCP here, copy the GUI session instruction, and give it to the agent. The agent attaches without changing its MCP configuration.").small().color(app.theme.text_muted));
            });
        });
    if !open {
        app.show_integrate_popup = false;
    }
}

fn mcp_agent_setup_prompt(exe_path: &str) -> String {
    let config = serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": {
            "logotomy": {
                "args": ["--mcp"],
                "command": exe_path,
                "disabled": false
            }
        }
    }))
    .unwrap_or_default();
    format!(
        "Logotomy is a local log analyzer. Add its MCP server globally (user scope) for this \
agent client. Preserve existing MCP servers and translate the following configuration to this \
client's native config format if necessary:\n\n{config}\n\nAfter updating the configuration, \
tell me whether the client must reload or restart. Verify that the Logotomy server exposes \
`session_info` and `attach_gui_session`. Use exactly the command and arguments above; do not \
substitute another transport or a legacy flag. If you cannot modify global MCP settings from \
this session, say so and tell me which settings file or UI to update."
    )
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
        if ui
            .button("Copy")
            .on_hover_text(format!("Copy config for {}", agent))
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
        assert!(prompt.contains("globally (user scope)"));
        assert!(prompt.contains("/Applications/Logotomy.app/Contents/MacOS/logotomy"));
        assert!(prompt.contains("\"args\": [\n        \"--mcp\""));
        assert!(prompt.contains("\"disabled\": false"));
        assert!(prompt.contains("Preserve existing MCP servers"));
        assert!(prompt.contains("session_info"));
        assert!(prompt.contains("attach_gui_session"));
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
