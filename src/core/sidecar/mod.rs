//! Per-file investigation sidecars.
//!
//! Sidecars are deliberately separate from application preferences.  They are
//! small, versioned JSON documents stored beside the log file and contain the
//! user's investigation state for that file.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

/// Current sidecar schema.  Readers must continue accepting older versions.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// The serialized state belonging to one source file.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct InvestigationState {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub source: SourceIdentity,
    #[serde(default)]
    pub filters: Vec<FilterState>,
    #[serde(default = "default_true")]
    pub everything_else_active: bool,
    #[serde(default)]
    pub selected_filter: Option<String>,
    #[serde(default)]
    pub applied_filter: Option<String>,
    #[serde(default)]
    pub search: SearchState,
    #[serde(default)]
    pub selected_line: Option<LineAnchor>,
    #[serde(default)]
    pub scroll: ScrollState,
    #[serde(default)]
    pub timeline_zoom: Option<TimelineZoomState>,
    #[serde(default)]
    pub pins: Vec<PinState>,
    #[serde(default)]
    pub trim: Option<LineRange>,
    #[serde(default)]
    pub show_templates: bool,
    #[serde(default = "default_templates_width")]
    pub templates_panel_width: f32,
    #[serde(default)]
    pub bottom_panel_open: bool,
    #[serde(default = "default_log_font_size")]
    pub log_font_size: f32,
    /// Serialized `egui_dock::DockState<ViewTab>`.  Keeping this as JSON in
    /// the core schema avoids coupling the core crate to GUI types.
    #[serde(default)]
    pub dock_layout: Option<serde_json::Value>,
    #[serde(default)]
    pub pre_detach_dock_layout: Option<serde_json::Value>,
    #[serde(default)]
    pub detached_views: Vec<String>,
    #[serde(default)]
    pub timeline_detached: bool,
}

fn default_schema_version() -> u32 {
    CURRENT_SCHEMA_VERSION
}
fn default_true() -> bool {
    true
}
fn default_templates_width() -> f32 {
    320.0
}
fn default_log_font_size() -> f32 {
    12.0
}

impl Default for InvestigationState {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            source: SourceIdentity::default(),
            filters: Vec::new(),
            everything_else_active: true,
            selected_filter: None,
            applied_filter: None,
            search: SearchState::default(),
            selected_line: None,
            scroll: ScrollState::default(),
            timeline_zoom: None,
            pins: Vec::new(),
            trim: None,
            show_templates: false,
            templates_panel_width: default_templates_width(),
            bottom_panel_open: false,
            log_font_size: default_log_font_size(),
            dock_layout: None,
            pre_detach_dock_layout: None,
            detached_views: Vec::new(),
            timeline_detached: false,
        }
    }
}

/// File identity used to decide whether line anchors can be applied safely.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct SourceIdentity {
    pub canonical_path: PathBuf,
    pub file_size: u64,
    pub mtime_unix_nanos: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FilterState {
    pub text: String,
    #[serde(default = "default_true")]
    pub active: bool,
    #[serde(default = "default_true")]
    pub case_sensitive: bool,
    #[serde(default)]
    pub exclude: bool,
    #[serde(default)]
    pub regex: bool,
    /// A typed Drain-template filter. Older sidecars omit this and remain text filters.
    #[serde(default)]
    pub template_id: Option<u32>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct SearchState {
    #[serde(default)]
    pub input: String,
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub template_id_mode: bool,
    #[serde(default)]
    pub selected_line: Option<LineAnchor>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LineAnchor {
    pub line: usize,
    #[serde(default)]
    pub timestamp: Option<i64>,
    #[serde(default)]
    pub line_hash: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ScrollState {
    #[serde(default)]
    pub top_line: Option<LineAnchor>,
    #[serde(default)]
    pub fraction: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LineRange {
    pub start: usize,
    /// Exclusive end, matching `LogDocument::trim_end`.
    pub end_exclusive: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TimelineZoomState {
    /// Either `time` or `sequence`.
    pub domain: String,
    pub start: i64,
    pub end: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct PinState {
    #[serde(default)]
    pub start_line: Option<LineAnchor>,
    #[serde(default)]
    pub end_line: Option<LineAnchor>,
    #[serde(default)]
    pub line_numbers: Vec<LineAnchor>,
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub start_timestamp: Option<i64>,
    #[serde(default)]
    pub end_timestamp: Option<i64>,
    #[serde(default)]
    pub unanchored: bool,
}

/// Whether persisted line positions may be applied automatically.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchStatus {
    Exact,
    Changed,
}

#[derive(Clone, Debug)]
pub struct LoadedState {
    pub state: InvestigationState,
    pub status: MatchStatus,
}

/// Return the adjacent sidecar path for a source file.
pub fn path_for(source: &Path) -> PathBuf {
    let parent = source.parent().unwrap_or_else(|| Path::new("."));
    let name = source
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| std::borrow::Cow::Borrowed("unknown"));
    parent.join(format!(".{name}.logotomy"))
}

/// Read the sidecar and classify it against the current source metadata.
pub fn load_for(source: &Path) -> Result<Option<LoadedState>, String> {
    let path = path_for(source);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("failed to read {}: {error}", path.display())),
    };
    let mut state: InvestigationState = serde_json::from_str(&text)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
    migrate(&mut state)?;

    let current = source_identity(source)?;
    let status = if state.source == current {
        MatchStatus::Exact
    } else {
        MatchStatus::Changed
    };
    Ok(Some(LoadedState { state, status }))
}

/// Save a sidecar using a temporary sibling followed by rename.
pub fn save_for(source: &Path, state: &InvestigationState) -> Result<(), String> {
    let path = path_for(source);
    let parent = path
        .parent()
        .ok_or_else(|| format!("invalid sidecar path {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    let text = serde_json::to_vec_pretty(state)
        .map_err(|error| format!("failed to encode {}: {error}", path.display()))?;
    let temporary = path.with_extension(format!("logotomy.tmp-{}", std::process::id()));
    fs::write(&temporary, text)
        .map_err(|error| format!("failed to write {}: {error}", temporary.display()))?;
    if let Err(error) = fs::rename(&temporary, &path) {
        #[cfg(windows)]
        let replaced = path.exists()
            && fs::remove_file(&path)
                .and_then(|_| fs::rename(&temporary, &path))
                .is_ok();
        #[cfg(not(windows))]
        let replaced = false;
        if !replaced {
            let _ = fs::remove_file(&temporary);
            return Err(format!("failed to replace {}: {error}", path.display()));
        }
    }
    #[cfg(windows)]
    mark_hidden(&path)?;
    Ok(())
}

#[cfg(windows)]
fn mark_hidden(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileAttributesW, SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN, INVALID_FILE_ATTRIBUTES,
    };

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: `wide` is a valid, NUL-terminated UTF-16 path for the duration
    // of both Windows API calls.
    let attributes = unsafe { GetFileAttributesW(wide.as_ptr()) };
    if attributes == INVALID_FILE_ATTRIBUTES {
        return Err(format!("failed to inspect {}", path.display()));
    }
    if attributes & FILE_ATTRIBUTE_HIDDEN == 0 {
        let updated = attributes | FILE_ATTRIBUTE_HIDDEN;
        if unsafe { SetFileAttributesW(wide.as_ptr(), updated) } == 0 {
            return Err(format!("failed to hide {}", path.display()));
        }
    }
    Ok(())
}

/// Build the identity used when writing a new snapshot.
pub fn source_identity(source: &Path) -> Result<SourceIdentity, String> {
    let canonical_path = fs::canonicalize(source)
        .map_err(|error| format!("failed to canonicalize {}: {error}", source.display()))?;
    let metadata = fs::metadata(&canonical_path)
        .map_err(|error| format!("failed to stat {}: {error}", canonical_path.display()))?;
    let mtime_unix_nanos = metadata
        .modified()
        .ok()
        .and_then(|mtime| mtime.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    Ok(SourceIdentity {
        canonical_path,
        file_size: metadata.len(),
        mtime_unix_nanos,
    })
}

fn migrate(state: &mut InvestigationState) -> Result<(), String> {
    if state.schema_version > CURRENT_SCHEMA_VERSION {
        return Err(format!(
            "sidecar schema {} is newer than supported schema {}",
            state.schema_version, CURRENT_SCHEMA_VERSION
        ));
    }
    // Version 1 is the initial schema. Future migrations should be explicit
    // and finish by setting `schema_version` to CURRENT_SCHEMA_VERSION.
    state.schema_version = CURRENT_SCHEMA_VERSION;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn temp_path() -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "logotomy_sidecar_test_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn sidecar_path_uses_the_full_source_filename() {
        assert_eq!(
            path_for(Path::new("/tmp/app.log")),
            PathBuf::from("/tmp/.app.log.logotomy")
        );
    }

    #[test]
    fn schema_round_trips_and_missing_fields_default() {
        let mut state = InvestigationState::default();
        state.source.canonical_path = PathBuf::from("/tmp/app.log");
        state.filters.push(FilterState {
            text: "T{42}".into(),
            active: false,
            case_sensitive: true,
            exclude: false,
            regex: false,
            template_id: Some(42),
        });
        let text = serde_json::to_string(&state).unwrap();
        let decoded: InvestigationState = serde_json::from_str(&text).unwrap();
        assert_eq!(decoded, state);

        let old: InvestigationState = serde_json::from_str(
            r#"{"source":{"canonical_path":"/tmp/app.log","file_size":1,"mtime_unix_nanos":2}}"#,
        )
        .unwrap();
        assert_eq!(old.schema_version, CURRENT_SCHEMA_VERSION);
        assert!(old.everything_else_active);
        assert_eq!(old.log_font_size, 12.0);
    }

    #[test]
    fn newer_schema_is_rejected() {
        let mut state: InvestigationState = serde_json::from_str(
            r#"{"schema_version":99,"source":{"canonical_path":"/tmp/app.log","file_size":1,"mtime_unix_nanos":2}}"#,
        )
        .unwrap();
        assert!(migrate(&mut state).is_err());
    }

    #[test]
    fn load_classifies_exact_and_changed_sources() {
        let source = temp_path();
        fs::write(&source, "one\ntwo\n").unwrap();
        let identity = source_identity(&source).unwrap();
        let mut state = InvestigationState::default();
        state.source = identity.clone();
        save_for(&source, &state).unwrap();
        assert_eq!(
            load_for(&source).unwrap().unwrap().status,
            MatchStatus::Exact
        );

        state.source.file_size += 1;
        save_for(&source, &state).unwrap();
        assert_eq!(
            load_for(&source).unwrap().unwrap().status,
            MatchStatus::Changed
        );

        fs::remove_file(path_for(&source)).ok();
        fs::remove_file(source).ok();
    }
}
