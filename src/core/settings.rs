//! Persistent settings for logotomy — stored as `~/.logotomy/settings.json`.
//!
//! Tracks recent files (last 20), dark/light mode preference, and provides the
//! log directory path for file-based logging.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::core::time::CustomDateFormat;

/// Maximum number of recent files to remember.
const MAX_RECENT: usize = 20;

/// Application settings persisted to disk.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Settings {
    /// Most-recently opened files, most recent first.
    #[serde(default)]
    pub recent_files: Vec<PathBuf>,
    /// Whether dark mode is enabled (default: true).
    #[serde(default = "default_dark")]
    pub dark_mode: bool,
    /// Default filter set to apply to new tabs.
    #[serde(default)]
    pub default_filter: Option<String>,
    /// Drain similarity threshold for template mining (0.3–0.9, default 0.5).
    #[serde(default = "default_sim_threshold")]
    pub sim_threshold: f64,
    /// Leading lines sampled to learn the common log header (default 200).
    #[serde(default = "default_header_sample_lines")]
    pub header_sample_lines: usize,
    /// Drain parse-tree depth (default 4). Depth N = token count + (N-2) routing tokens.
    #[serde(default = "default_drain_depth")]
    pub drain_depth: usize,
    /// When true, deleting a filter (trash icon / "Clear all filters") proceeds
    /// without a confirmation popup. Default: false → always confirm.
    #[serde(default)]
    pub skip_filter_delete_confirm: bool,
    /// Preferred embedded-data inspector tab (`tree`, `pretty`, or `raw`).
    /// Special detector-specific tabs are never persisted here.
    #[serde(default = "default_embedded_inspector_mode")]
    pub embedded_inspector_mode: String,
}

fn default_dark() -> bool {
    true
}
fn default_sim_threshold() -> f64 {
    0.5
}
fn default_header_sample_lines() -> usize {
    200
}
fn default_drain_depth() -> usize {
    4
}
fn default_embedded_inspector_mode() -> String {
    "pretty".to_owned()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            recent_files: Vec::new(),
            dark_mode: true,
            default_filter: None,
            sim_threshold: default_sim_threshold(),
            header_sample_lines: default_header_sample_lines(),
            drain_depth: default_drain_depth(),
            skip_filter_delete_confirm: false,
            embedded_inspector_mode: default_embedded_inspector_mode(),
        }
    }
}

impl Settings {
    /// Root data directory (`~/.logotomy`).
    pub fn home_dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".logotomy")
    }

    /// Path to the filters directory (`~/.logotomy/filters/`).
    pub fn filters_dir() -> PathBuf {
        Self::home_dir().join("filters")
    }

    /// Path to the settings JSON file.
    pub fn path() -> PathBuf {
        Self::home_dir().join("settings.json")
    }

    /// Path to the logs directory (`~/.logotomy/logs/`).
    pub fn log_dir() -> PathBuf {
        Self::home_dir().join("logs")
    }

    /// Path to the user-defined date-format list (`~/.logotomy/custom_date_format_list.json`).
    pub fn custom_date_formats_path() -> PathBuf {
        Self::home_dir().join("custom_date_format_list.json")
    }

    /// Load the user-defined custom date formats (empty list if missing/unparsable).
    pub fn load_custom_date_formats() -> Vec<CustomDateFormat> {
        let path = Self::custom_date_formats_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(e) => {
                    log::warn!(
                        "failed to parse custom date formats ({}), using none: {e}",
                        path.display()
                    );
                    Vec::new()
                }
            },
            Err(_) => Vec::new(),
        }
    }

    /// Persist the user-defined custom date formats to disk.
    pub fn save_custom_date_formats(formats: &[CustomDateFormat]) {
        let dir = Self::home_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            log::error!("failed to create config dir ({}): {e}", dir.display());
            return;
        }
        let path = Self::custom_date_formats_path();
        match serde_json::to_string_pretty(formats) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&path, &text) {
                    log::error!(
                        "failed to write custom date formats ({}): {e}",
                        path.display()
                    );
                }
            }
            Err(e) => log::error!("failed to serialize custom date formats: {e}"),
        }
    }

    /// Load settings from disk, or return defaults if the file doesn't exist
    /// or can't be parsed.
    pub fn load() -> Self {
        let path = Self::path();
        if !path.exists() {
            return Self::default();
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(s) => s,
                Err(e) => {
                    log::warn!(
                        "failed to parse settings file ({}), using defaults: {e}",
                        path.display()
                    );
                    Self::default()
                }
            },
            Err(e) => {
                log::warn!(
                    "failed to read settings file ({}), using defaults: {e}",
                    path.display()
                );
                Self::default()
            }
        }
    }

    /// Save settings to disk. Creates the `~/.logotomy/` directory if needed.
    pub fn save(&self) {
        let dir = Self::home_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            log::error!("failed to create settings dir ({}): {e}", dir.display());
            return;
        }
        let path = Self::path();
        match serde_json::to_string_pretty(self) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&path, &text) {
                    log::error!("failed to write settings file ({}): {e}", path.display());
                }
            }
            Err(e) => {
                log::error!("failed to serialize settings: {e}");
            }
        }
    }

    /// Add a file to the recent list (dedup, push front, cap at MAX_RECENT).
    pub fn add_recent_file(&mut self, path: PathBuf) {
        // Remove any existing entry for the same path.
        self.recent_files.retain(|p| p != &path);
        // Push to the front.
        self.recent_files.insert(0, path);
        // Cap at MAX_RECENT.
        self.recent_files.truncate(MAX_RECENT);
    }

    /// Return the list of recent files (ordered most-recent first).
    pub fn recent_files(&self) -> &[PathBuf] {
        &self.recent_files
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_filter_delete_confirm_defaults_to_false() {
        let s = Settings::default();
        assert!(
            !s.skip_filter_delete_confirm,
            "default must always ask for confirmation"
        );
    }

    #[test]
    fn skip_filter_delete_confirm_round_trips_through_serde() {
        // Missing key → default (false) so old settings files keep confirming.
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert!(!s.skip_filter_delete_confirm);

        // Explicit true round-trips.
        let s: Settings = serde_json::from_str(r#"{"skip_filter_delete_confirm":true}"#).unwrap();
        assert!(s.skip_filter_delete_confirm);
        let json = serde_json::to_string(&Settings::default()).unwrap();
        assert!(json.contains("\"skip_filter_delete_confirm\":false"));
    }

    #[test]
    fn embedded_inspector_mode_defaults_to_pretty_and_round_trips() {
        let settings: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.embedded_inspector_mode, "pretty");

        let settings: Settings =
            serde_json::from_str(r#"{"embedded_inspector_mode":"raw"}"#).unwrap();
        assert_eq!(settings.embedded_inspector_mode, "raw");
    }
}
