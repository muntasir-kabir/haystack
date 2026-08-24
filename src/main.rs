//! logotomy — high-performance log analysis & visualization.
//! 50MB log file? logotomy happened in there — let's find out.
//!
//! Usage:
//!   logotomy              — launch the GUI
//!   logotomy --mcp        — run MCP server (stdio mode)
//!   logotomy --help       — show this help

#![cfg_attr(
    all(not(debug_assertions), feature = "gui"),
    windows_subsystem = "windows"
)]

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[cfg(feature = "gui")]
use std::io::Write;

const USAGE: &str = r#"logotomy — high-performance log analyzer

USAGE:
  logotomy              Launch the GUI
  logotomy --mcp        Run MCP server (stdio mode)
  logotomy --mcp-gui    Deprecated compatibility bridge
  logotomy --help       Show this help message
"#;

fn main() {
    let args: Vec<OsString> = std::env::args_os().collect();

    // Check for --help first
    if args.iter().any(|a| a == "--help" || a == "-h") {
        eprintln!("{USAGE}");
        return;
    }

    // Check for --mcp flag
    let is_mcp = args.iter().any(|a| a == "--mcp");
    let is_mcp_gui = args.iter().any(|a| a == "--mcp-gui");

    if is_mcp_gui {
        eprintln!("warning: --mcp-gui is deprecated; configure --mcp and use attach_gui_session");
        logotomy::mcp::run_gui_bridge();
    } else if is_mcp {
        if let Some(option) = unsupported_mcp_option(&args) {
            eprintln!(
                "error: {option} is no longer supported; Logotomy MCP is exposed only through stdio (`logotomy --mcp`)"
            );
            std::process::exit(2);
        }
        let state: Arc<Mutex<logotomy::mcp::ServerState>> = Arc::default();
        logotomy::mcp::run_stdio(state);
    } else {
        // GUI mode — non-option arguments are paths supplied by the shell,
        // Finder, Explorer, or a desktop-file file association.
        run_gui(gui_paths(&args));
    }
}

fn unsupported_mcp_option(args: &[OsString]) -> Option<&str> {
    args.iter().find_map(|argument| {
        if argument == "--port" {
            Some("--port")
        } else if argument == "--status-file" {
            Some("--status-file")
        } else {
            None
        }
    })
}

fn gui_paths(args: &[OsString]) -> Vec<PathBuf> {
    let mut after_separator = false;
    args.iter()
        .skip(1)
        .filter_map(|arg| {
            if !after_separator && arg == "--" {
                after_separator = true;
                return None;
            }
            if !after_separator && arg.to_string_lossy().starts_with('-') {
                return None;
            }
            Some(PathBuf::from(arg))
        })
        .collect()
}

#[cfg(feature = "gui")]
fn run_gui(paths: Vec<PathBuf>) {
    // Set up file logging in ~/.logotomy/logs/
    let log_dir = logotomy::core::settings::Settings::log_dir();
    let _ = std::fs::create_dir_all(&log_dir);
    let log_path = log_dir.join("logotomy.log");

    // Truncate if over 10MB
    if let Ok(metadata) = std::fs::metadata(&log_path) {
        if metadata.len() > 10 * 1024 * 1024 {
            let _ = std::fs::write(&log_path, "");
        }
    }

    // Open log file for appending
    let log_file = std::fs::File::options()
        .create(true)
        .append(true)
        .open(&log_path)
        .ok();
    let file_logger = log_file.map(|f| Arc::new(Mutex::new(f)));

    // Custom logger: writes to both stderr (via env_logger) and the log file
    struct DualLogger {
        file: Option<Arc<Mutex<std::fs::File>>>,
    }

    impl log::Log for DualLogger {
        fn enabled(&self, _metadata: &log::Metadata) -> bool {
            true
        }

        fn log(&self, record: &log::Record) {
            // Write to stderr
            eprintln!(
                "{} [{}] {} — {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                record.level(),
                record.target(),
                record.args()
            );

            // Write to file
            if let Some(ref file) = self.file {
                if let Ok(mut f) = file.lock() {
                    let _ = writeln!(
                        f,
                        "{} [{}] {}:{} — {}",
                        chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                        record.level(),
                        record.file().unwrap_or("<unknown>"),
                        record.line().unwrap_or(0),
                        record.args()
                    );
                }
            }
        }

        fn flush(&self) {
            if let Some(ref file) = self.file {
                if let Ok(mut f) = file.lock() {
                    let _ = f.flush();
                }
            }
        }
    }

    let level = log::LevelFilter::Info;
    log::set_boxed_logger(Box::new(DualLogger { file: file_logger }))
        .map(|()| log::set_max_level(level))
        .ok();

    log::info!(
        "logotomy GUI starting — logs also written to {}",
        log_path.display()
    );

    let instance = match ui::instance::SingleInstance::acquire(paths) {
        Ok(ui::instance::Startup::Primary(instance)) => instance,
        Ok(ui::instance::Startup::Forwarded) => return,
        Err(error) => {
            log::error!("failed to start single-instance GUI: {error}");
            eprintln!("logotomy: {error}");
            return;
        }
    };
    let initial_paths = instance.initial_paths().to_vec();
    let open_requests = instance.requests();

    // Decode the embedded app icon (logotomy_256.png) into RGBA for the native
    // window icon. On Windows and Linux this sets the window/taskbar icon; on
    // macOS the Dock icon is governed by the .icns bundle instead.
    fn app_icon() -> std::sync::Arc<eframe::egui::IconData> {
        let bytes: &[u8] = include_bytes!("ui/icons/logotomy_256.png");
        match image::load_from_memory_with_format(bytes, image::ImageFormat::Png) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                std::sync::Arc::new(eframe::egui::IconData {
                    rgba: rgba.into_raw(),
                    width: w,
                    height: h,
                })
            }
            Err(e) => {
                log::warn!("failed to decode app icon: {e}");
                std::sync::Arc::new(eframe::egui::IconData {
                    rgba: Vec::new(),
                    width: 0,
                    height: 0,
                })
            }
        }
    }

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 620.0])
            .with_title("logotomy — log analyzer")
            .with_icon(app_icon())
            .with_drag_and_drop(true),
        ..Default::default()
    };

    let _ = eframe::run_native(
        "logotomy",
        options,
        Box::new(move |cc| {
            Ok(Box::new(ui::app::model::LogotomyApp::new(
                cc,
                initial_paths,
                open_requests,
            )))
        }),
    );
}

#[cfg(not(feature = "gui"))]
fn run_gui(_paths: Vec<PathBuf>) {
    eprintln!("logotomy: GUI mode is not available in this build.");
    eprintln!(
        "Rebuild with the 'gui' feature enabled, or use 'logotomy --mcp' for the MCP server."
    );
    eprintln!("{USAGE}");
    std::process::exit(1);
}

/// The GUI module — only compiled when the "gui" feature is enabled.
/// Points to the existing `src/ui/` directory.
#[cfg(feature = "gui")]
mod ui;

#[cfg(test)]
mod tests {
    use super::{gui_paths, unsupported_mcp_option};
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn removed_mcp_transport_options_are_rejected() {
        let port = vec!["logotomy".into(), "--mcp".into(), "--port".into()];
        assert_eq!(unsupported_mcp_option(&port), Some("--port"));

        let status = vec!["logotomy".into(), "--mcp".into(), "--status-file".into()];
        assert_eq!(unsupported_mcp_option(&status), Some("--status-file"));
        assert_eq!(
            unsupported_mcp_option(&["logotomy".into(), "--mcp".into()]),
            None
        );
    }

    #[test]
    fn gui_arguments_keep_file_paths_and_support_dash_prefixed_paths_after_separator() {
        let args = vec![
            OsString::from("logotomy"),
            OsString::from("--"),
            OsString::from("-odd-name.log"),
            OsString::from("/tmp/example.log"),
        ];
        assert_eq!(
            gui_paths(&args),
            vec![
                PathBuf::from("-odd-name.log"),
                PathBuf::from("/tmp/example.log")
            ]
        );
    }

    #[test]
    fn gui_arguments_ignore_options_before_paths() {
        let args = vec![
            OsString::from("logotomy"),
            OsString::from("--unknown"),
            OsString::from("example.log"),
        ];
        assert_eq!(gui_paths(&args), vec![PathBuf::from("example.log")]);
    }
}
