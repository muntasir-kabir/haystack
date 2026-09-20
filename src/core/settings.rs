//! Persistent settings for haystack — stored as `~/.haystack/settings.json`.
//!
//! Tracks recent files (last 20), theme preference, and provides the
//! log directory path for file-based logging.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::core::time::CustomDateFormat;

/// The persisted application theme preference.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    Light,
    Dark,
    System,
}

impl ThemeMode {
    pub const ALL: [Self; 3] = [Self::Light, Self::Dark, Self::System];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
            Self::System => "System",
        }
    }
}

impl Default for ThemeMode {
    fn default() -> Self {
        Self::Dark
    }
}

/// Maximum number of recent files to remember.
const MAX_RECENT: usize = 20;
/// Maximum number of recent search or filter inputs to remember.
pub const MAX_RECENT_QUERIES: usize = 20;

/// How Log View presents long lines by default.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LogLineDisplayMode {
    /// Paint a bounded, UTF-8-safe preview so every virtual row has one height.
    #[default]
    Truncate,
    /// Paint complete source text on a single row inside a horizontal scroll area.
    HorizontalScroll,
    /// Paint complete source text across multiple visual rows.
    Wrap,
}

impl LogLineDisplayMode {
    pub const ALL: [Self; 3] = [Self::Truncate, Self::HorizontalScroll, Self::Wrap];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Truncate => "Truncate long lines",
            Self::HorizontalScroll => "Horizontal scroll",
            Self::Wrap => "Wrap long lines",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::Truncate => "Fast 2,000-byte preview; open a line to inspect all text.",
            Self::HorizontalScroll => {
                "Show complete lines without wrapping; use the bottom scrollbar."
            }
            Self::Wrap => "Show complete lines wrapped to the Log View width.",
        }
    }
}

/// A reusable timeline-filter input, including the matching mode needed to
/// execute it directly from a recent-entry suggestion.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct RecentFilter {
    pub text: String,
    #[serde(default = "default_true")]
    pub case_sensitive: bool,
    #[serde(default)]
    pub regex: bool,
}

fn default_true() -> bool {
    true
}

/// Application settings persisted to disk.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Settings {
    /// Most-recently opened files, most recent first.
    #[serde(default)]
    pub recent_files: Vec<PathBuf>,
    /// Most-recently executed Log View search terms, most recent first.
    #[serde(default)]
    pub recent_searches: Vec<String>,
    /// Typed searches are separate so replay never treats them as plain text.
    #[serde(default)]
    pub recent_field_searches: Vec<crate::core::field_query::FieldQuery>,
    /// Most-recently added Timeline filters, including their match mode.
    #[serde(default)]
    pub recent_filters: Vec<RecentFilter>,
    /// Default long-line presentation for newly opened logs.
    #[serde(default)]
    pub log_line_display_mode: LogLineDisplayMode,
    /// Files that were open in the previous GUI session, in tab order.
    #[serde(default)]
    pub open_files: Vec<PathBuf>,
    /// Canonical path of the active tab from the previous GUI session.
    #[serde(default)]
    pub active_file: Option<PathBuf>,
    /// Theme choice. Older settings files with `dark_mode` are migrated when loaded.
    #[serde(default)]
    pub theme_mode: ThemeMode,
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
            recent_searches: Vec::new(),
            recent_field_searches: Vec::new(),
            recent_filters: Vec::new(),
            log_line_display_mode: LogLineDisplayMode::default(),
            open_files: Vec::new(),
            active_file: None,
            theme_mode: ThemeMode::default(),
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
    /// Root data directory (`~/.haystack`).
    pub fn home_dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".haystack")
    }

    /// Path to the filters directory (`~/.haystack/filters/`).
    pub fn filters_dir() -> PathBuf {
        Self::home_dir().join("filters")
    }

    /// Path to the settings JSON file.
    pub fn path() -> PathBuf {
        Self::home_dir().join("settings.json")
    }

    /// Path to the logs directory (`~/.haystack/logs/`).
    pub fn log_dir() -> PathBuf {
        Self::home_dir().join("logs")
    }

    /// Path to the user-defined date-format list (`~/.haystack/custom_date_format_list.json`).
    pub fn custom_date_formats_path() -> PathBuf {
        Self::home_dir().join("custom_date_format_list.json")
    }

    /// Versioned reusable log-format templates. Selected definitions are also
    /// embedded in per-file sidecars so deleting a preset cannot alter a file.
    pub fn record_profiles_path() -> PathBuf {
        Self::home_dir().join("record_profiles.json")
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
            Ok(text) => match Self::from_persisted_json(&text) {
                Ok((s, migrated)) => {
                    if migrated {
                        s.save();
                    }
                    s
                }
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

    /// Parse settings while translating the pre-theme-mode boolean schema.
    /// The bool is deliberately not a field on `Settings`; after the first
    /// successful load, saving writes only the explicit theme choice.
    fn from_persisted_json(text: &str) -> Result<(Self, bool), serde_json::Error> {
        let value: serde_json::Value = serde_json::from_str(text)?;
        let legacy_dark_mode = value.get("dark_mode").and_then(serde_json::Value::as_bool);
        let has_theme_mode = value.get("theme_mode").is_some();
        let mut settings: Self = serde_json::from_value(value)?;
        let migrated = !has_theme_mode && legacy_dark_mode.is_some();
        if let Some(dark_mode) = legacy_dark_mode.filter(|_| !has_theme_mode) {
            settings.theme_mode = if dark_mode {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            };
        }
        Ok((settings, migrated))
    }

    /// Save settings to disk. Creates the `~/.haystack/` directory if needed.
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

    /// Remember an executed search, preserving a most-recent-first unique list.
    pub fn add_recent_search(&mut self, query: impl AsRef<str>) {
        let query = query.as_ref().trim();
        if query.is_empty() {
            return;
        }
        self.recent_searches.retain(|entry| entry != query);
        self.recent_searches.insert(0, query.to_owned());
        self.recent_searches.truncate(MAX_RECENT_QUERIES);
    }

    pub fn add_recent_field_search(&mut self, query: crate::core::field_query::FieldQuery) {
        self.recent_field_searches.retain(|entry| entry != &query);
        self.recent_field_searches.insert(0, query);
        self.recent_field_searches.truncate(MAX_RECENT_QUERIES);
    }

    /// Remember an added filter, including the mode necessary to replay it.
    pub fn add_recent_filter(&mut self, filter: RecentFilter) {
        let text = filter.text.trim();
        if text.is_empty() {
            return;
        }
        self.recent_filters.retain(|entry| entry.text != text);
        self.recent_filters.insert(
            0,
            RecentFilter {
                text: text.to_owned(),
                ..filter
            },
        );
        self.recent_filters.truncate(MAX_RECENT_QUERIES);
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

    #[test]
    fn workspace_fields_default_for_older_settings_and_round_trip() {
        let settings: Settings = serde_json::from_str("{}").unwrap();
        assert!(settings.open_files.is_empty());
        assert!(settings.active_file.is_none());

        let settings: Settings =
            serde_json::from_str(r#"{"open_files":["/tmp/one.log"],"active_file":"/tmp/one.log"}"#)
                .unwrap();
        assert_eq!(settings.open_files, vec![PathBuf::from("/tmp/one.log")]);
        assert_eq!(settings.active_file, Some(PathBuf::from("/tmp/one.log")));
    }

    #[test]
    fn long_line_preferences_and_recent_queries_round_trip() {
        let mut settings: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.log_line_display_mode, LogLineDisplayMode::Truncate);
        settings.log_line_display_mode = LogLineDisplayMode::Wrap;
        settings.add_recent_search(" error ");
        settings.add_recent_search("warning");
        settings.add_recent_search("error");
        let field = crate::core::field_query::FieldQuery::parse("b >= 9", true).unwrap();
        settings.add_recent_field_search(field.clone());
        settings.add_recent_field_search(field.clone());
        settings.add_recent_filter(RecentFilter {
            text: "panic".to_owned(),
            case_sensitive: false,
            regex: false,
        });
        assert_eq!(settings.recent_searches, ["error", "warning"]);
        assert_eq!(settings.recent_field_searches, [field]);
        assert_eq!(settings.recent_filters[0].text, "panic");

        let restored: Settings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(restored.log_line_display_mode, LogLineDisplayMode::Wrap);
        assert_eq!(restored.recent_searches, ["error", "warning"]);
        assert_eq!(
            restored.recent_field_searches,
            settings.recent_field_searches
        );
    }

    #[test]
    fn legacy_dark_mode_settings_migrate_to_explicit_theme_mode() {
        let (light, migrated) = Settings::from_persisted_json(r#"{"dark_mode":false}"#).unwrap();
        assert!(migrated);
        assert_eq!(light.theme_mode, ThemeMode::Light);

        let (dark, migrated) = Settings::from_persisted_json(r#"{"dark_mode":true}"#).unwrap();
        assert!(migrated);
        assert_eq!(dark.theme_mode, ThemeMode::Dark);

        let (explicit, migrated) =
            Settings::from_persisted_json(r#"{"dark_mode":false,"theme_mode":"system"}"#).unwrap();
        assert!(!migrated);
        assert_eq!(explicit.theme_mode, ThemeMode::System);
    }
}
