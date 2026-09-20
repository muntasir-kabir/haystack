//! Haystack — high-performance log analysis & visualization.
//! Find the needle in giant log files without swallowing the whole haystack.
//!
//! Usage:
//!   haystack              — launch the GUI
//!   haystack --mcp        — run MCP server (stdio mode)
//!   haystack extract_template — mine templates into JSON files
//!   haystack --help       — show this help

#![cfg_attr(
    all(not(debug_assertions), feature = "gui"),
    windows_subsystem = "windows"
)]

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[cfg(feature = "gui")]
use std::io::Write;

const USAGE: &str = r#"haystack — high-performance log analyzer

USAGE:
  haystack              Launch the GUI
  haystack --mcp        Run MCP server (stdio mode)
  haystack --mcp-gui    Deprecated compatibility bridge
  haystack extract_template --file FILE --store STORE --output OUTPUT [--format FORMAT]
                       Mine templates in a standalone process (JSON files only)
  haystack --help       Show this help message
"#;

fn main() {
    let args: Vec<OsString> = std::env::args_os().collect();

    // Check for --help first
    if args.iter().any(|a| a == "--help" || a == "-h") {
        eprintln!("{USAGE}");
        return;
    }

    if args
        .get(1)
        .is_some_and(|argument| argument == "extract_template")
    {
        run_extract_template(&args[2..]);
        return;
    }

    // Check for --mcp flag
    let is_mcp = args.iter().any(|a| a == "--mcp");
    let is_mcp_gui = args.iter().any(|a| a == "--mcp-gui");

    if is_mcp_gui {
        eprintln!("warning: --mcp-gui is deprecated; configure --mcp and use attach_gui_session");
        haystack::mcp::run_gui_bridge();
    } else if is_mcp {
        if let Some(option) = unsupported_mcp_option(&args) {
            eprintln!(
                "error: {option} is no longer supported; Haystack MCP is exposed only through stdio (`haystack --mcp`)"
            );
            std::process::exit(2);
        }
        let state: Arc<Mutex<haystack::mcp::ServerState>> = Arc::default();
        haystack::mcp::run_stdio(state);
    } else {
        // GUI mode — non-option arguments are paths supplied by the shell,
        // Finder, Explorer, or a desktop-file file association.
        run_gui(gui_paths(&args));
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ExtractTemplateArgs {
    file: PathBuf,
    format: String,
    store: PathBuf,
    output: PathBuf,
}

fn run_extract_template(args: &[OsString]) {
    match parse_extract_template_args(args).and_then(|args| {
        haystack::extract_templates(&args.file, &args.format, &args.store, &args.output)
    }) {
        Ok(summary) => match serde_json::to_string_pretty(&summary) {
            Ok(json) => println!("{json}"),
            Err(error) => {
                emit_extract_error(format!("failed to encode result: {error}"));
                std::process::exit(1);
            }
        },
        Err(error) => {
            emit_extract_error(error);
            std::process::exit(2);
        }
    }
}

fn emit_extract_error(error: String) {
    eprintln!(
        "{}",
        serde_json::json!({
            "schema_version": 1,
            "error": error,
        })
    );
}

fn parse_extract_template_args(args: &[OsString]) -> Result<ExtractTemplateArgs, String> {
    let mut file = None;
    let mut format = "{time} {log}".to_string();
    let mut format_seen = false;
    let mut store = None;
    let mut output = None;
    let mut index = 0;

    while index < args.len() {
        let option = args[index].to_string_lossy();
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("{option} requires a value"))?;
        match option.as_ref() {
            "--file" => set_extract_path(&mut file, value, "--file")?,
            "--format" => {
                if format_seen {
                    return Err("--format may only be supplied once".to_string());
                }
                format_seen = true;
                format = value
                    .to_str()
                    .ok_or_else(|| "--format must be valid UTF-8".to_string())?
                    .to_string();
            }
            "--store" => set_extract_path(&mut store, value, "--store")?,
            "--output" => set_extract_path(&mut output, value, "--output")?,
            _ => return Err(format!("unknown extract_template option: {option}")),
        }
        index += 2;
    }

    Ok(ExtractTemplateArgs {
        file: file.ok_or_else(|| "--file is required".to_string())?,
        format,
        store: store.ok_or_else(|| "--store is required".to_string())?,
        output: output.ok_or_else(|| "--output is required".to_string())?,
    })
}

fn set_extract_path(
    slot: &mut Option<PathBuf>,
    value: &OsString,
    option: &str,
) -> Result<(), String> {
    if slot.replace(PathBuf::from(value)).is_some() {
        return Err(format!("{option} may only be supplied once"));
    }
    Ok(())
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
    // Set up file logging in ~/.haystack/logs/
    let log_dir = haystack::core::settings::Settings::log_dir();
    let _ = std::fs::create_dir_all(&log_dir);
    let log_path = log_dir.join("haystack.log");

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
        "haystack GUI starting — logs also written to {}",
        log_path.display()
    );

    let instance = match ui::instance::SingleInstance::acquire(paths) {
        Ok(ui::instance::Startup::Primary(instance)) => instance,
        Ok(ui::instance::Startup::Forwarded) => return,
        Err(error) => {
            log::error!("failed to start single-instance GUI: {error}");
            eprintln!("haystack: {error}");
            return;
        }
    };
    let initial_paths = instance.initial_paths().to_vec();
    let open_requests = instance.requests();

    // Decode the embedded app icon (haystack_256.png) into RGBA for the native
    // window icon. On Windows and Linux this sets the window/taskbar icon; on
    // macOS the Dock icon is governed by the .icns bundle instead.
    fn app_icon() -> std::sync::Arc<eframe::egui::IconData> {
        let bytes: &[u8] = include_bytes!("ui/icons/haystack_256.png");
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
            .with_title("Haystack — log analyzer")
            .with_icon(app_icon())
            .with_drag_and_drop(true),
        ..Default::default()
    };

    let _ = eframe::run_native(
        "haystack",
        options,
        Box::new(move |cc| {
            Ok(Box::new(ui::app::model::HaystackApp::new(
                cc,
                initial_paths,
                open_requests,
            )))
        }),
    );
}

#[cfg(not(feature = "gui"))]
fn run_gui(_paths: Vec<PathBuf>) {
    eprintln!("haystack: GUI mode is not available in this build.");
    eprintln!(
        "Rebuild with the 'gui' feature enabled, or use 'haystack --mcp' for the MCP server."
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
    use super::{gui_paths, parse_extract_template_args, unsupported_mcp_option};
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn removed_mcp_transport_options_are_rejected() {
        let port = vec!["haystack".into(), "--mcp".into(), "--port".into()];
        assert_eq!(unsupported_mcp_option(&port), Some("--port"));

        let status = vec!["haystack".into(), "--mcp".into(), "--status-file".into()];
        assert_eq!(unsupported_mcp_option(&status), Some("--status-file"));
        assert_eq!(
            unsupported_mcp_option(&["haystack".into(), "--mcp".into()]),
            None
        );
    }

    #[test]
    fn gui_arguments_keep_file_paths_and_support_dash_prefixed_paths_after_separator() {
        let args = vec![
            OsString::from("haystack"),
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
            OsString::from("haystack"),
            OsString::from("--unknown"),
            OsString::from("example.log"),
        ];
        assert_eq!(gui_paths(&args), vec![PathBuf::from("example.log")]);
    }

    #[test]
    fn extract_template_arguments_use_the_default_format() {
        let args = vec![
            "--file".into(),
            "source.log".into(),
            "--store".into(),
            "templates.json".into(),
            "--output".into(),
            "sequence.json".into(),
        ];
        let parsed = parse_extract_template_args(&args).unwrap();
        assert_eq!(parsed.file, PathBuf::from("source.log"));
        assert_eq!(parsed.format, "{time} {log}");
        assert_eq!(parsed.store, PathBuf::from("templates.json"));
        assert_eq!(parsed.output, PathBuf::from("sequence.json"));
    }

    #[test]
    fn extract_template_arguments_require_all_file_paths() {
        let error =
            parse_extract_template_args(&["--file".into(), "source.log".into()]).unwrap_err();
        assert_eq!(error, "--store is required");
    }
}
