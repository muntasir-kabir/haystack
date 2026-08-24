use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aho_corasick::AhoCorasick;
use crossbeam_channel::Receiver;
use eframe::egui;
use egui::Color32;
use egui_dock::DockState;
use log::{error, info};

use crate::ui::{icons, theme::Theme};
use logotomy::core::document::{FileChange, LoadProgress, LoadStage, LogDocument, ParsingConfig};
use logotomy::core::embedded_data::{AnalysisLimits, Detection, EmbeddedDataEngine, SourceSpan};
use logotomy::core::saved_filter::SavedFilter;
use logotomy::core::search;
use logotomy::core::settings::Settings;
use logotomy::core::time::{CustomDateFormat, CustomTimeFormat};
use logotomy::core::timeline::{Timeline, DEFAULT_BUCKETS};

#[path = "tab_model.rs"]
mod tab_model;

/// Actions triggered from the right-click context menu on timeline or log view.
#[derive(Clone, Copy, Debug)]
pub enum TrimAction {
    TrimRight(usize),
    TrimLeft(usize),
}

/// A saved pin entry — a pinned range of log lines with optional user comment.
#[derive(Clone, Debug)]
pub struct PinEntry {
    pub start_line: usize,
    pub line_numbers: Vec<usize>,
    pub start_ts: i64,
    pub end_ts: i64,
    pub comment: String,
}

/// Maximum number of filters supported in the timeline (excluding
/// the "Everything Else" lane). The filter input is disabled at this cap.
pub const MAX_FILTERS: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ViewTab {
    Timeline,
    Log,
    Pinned,
}

pub struct Filter {
    pub text: String,
    pub color: Color32,
}

/// Complete output of the background filter worker. Building timeline density
/// and filter-point indexes walks the document, so it belongs beside the
/// already-background Aho-Corasick scan rather than on the next UI frame.
pub(crate) struct FilterScanResult {
    matches: Arc<Vec<Vec<u32>>>,
    timeline: Timeline,
}

/// Completed filtered-line index, built without holding up the UI after a
/// lane visibility change.
pub(crate) struct VisibleLinesResult {
    visible_lines: Option<Arc<Vec<usize>>>,
    preserve_anchor: Option<usize>,
}

struct TailUpdateResult {
    doc: Box<LogDocument>,
    old_line_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EmbeddedScanKey {
    epoch: u64,
    file_size: u64,
    intervals: Vec<(usize, usize)>,
    interest_lines: Vec<usize>,
}

struct EmbeddedScanResult {
    key: EmbeddedScanKey,
    detections: Arc<Vec<Detection>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EmbeddedInspectorMode {
    #[default]
    Pretty,
    Tree,
    Raw,
    /// Stack traces use their parsed frame hierarchy instead of pretending
    /// their primary representation is a generic value tree.
    Frames,
    /// Encoded values start with safe metadata; deeper decoding is explicit.
    Summary,
    Decoded,
}

impl EmbeddedInspectorMode {
    /// Convert the user-selectable inspector tabs to their settings value.
    /// Detector-specific modes are intentionally not persisted.
    pub(crate) fn persisted_name(self) -> Option<&'static str> {
        match self {
            Self::Tree => Some("tree"),
            Self::Pretty => Some("pretty"),
            Self::Raw => Some("raw"),
            Self::Frames | Self::Summary | Self::Decoded => None,
        }
    }

    pub(crate) fn from_persisted_name(name: &str) -> Self {
        match name {
            "tree" => Self::Tree,
            "raw" => Self::Raw,
            _ => Self::Pretty,
        }
    }
}

pub struct LogTab {
    pub doc: Arc<LogDocument>,
    pub filters: Vec<Filter>,
    /// Per-filter sorted matching line indices.
    pub matches: Arc<Vec<Vec<u32>>>,
    pub timeline: Timeline,
    /// Line shown in the bottom context panel.
    pub context_line: Option<usize>,
    /// One-shot scroll request for the central log view.
    pub pending_scroll: Option<usize>,
    pub show_templates: bool,
    /// Zoom window on the timeline: (start_x, end_x) in epoch ms or line index.
    /// None = auto (full range).
    pub timeline_zoom: Option<(i64, i64)>,
    /// The filter lane + real line index of the currently selected diamond, if any.
    pub selected_diamond: Option<(usize, usize)>,
    /// The filter lane currently selected for left/right occurrence navigation.
    pub selected_lane: Option<usize>,
    /// One-shot UI message queued by a tab view and drained by the app toast.
    pub pending_toast: Option<String>,
    pub filter_input: String,
    /// Automaton used for cheap per-line highlight of visible rows.
    pub highlighter: Option<Arc<AhoCorasick>>,
    /// In-flight background filter scan: result channel + cancel flag.
    pub search_rx: Option<(Receiver<FilterScanResult>, Arc<AtomicBool>)>,
    /// In-flight visible-lines rebuild caused by toggling timeline lanes.
    pub visible_rx: Option<(Receiver<VisibleLinesResult>, Arc<AtomicBool>)>,
    /// In-flight staged append. The worker owns a detached document copy, so
    /// tailing never mutates indexes while the UI is reading them.
    tail_rx: Option<Receiver<Result<TailUpdateResult, String>>>,
    /// Per-filter lane active toggle (true = show lane + include in filter).
    pub lane_active: Vec<bool>,
    /// Whether the "Everything Else" lane (lines matching no filter) is active.
    pub everything_else_active: bool,
    /// Filter index pending removal confirmation (None = no pending removal).
    pub pending_filter_removal: Option<usize>,
    /// Shows the "remove ALL filters?" confirmation popup (set by the timeline
    /// "Clear all filters" button, consumed by app/view.rs).
    pub pending_clear_filters: bool,
    /// Whether the timeline is popped out into its own window.
    /// When true, the fixed top panel is hidden and a detached viewport shows.
    pub timeline_detached: bool,
    /// Filtered visible line indices. None = all lines visible.
    pub visible_lines: Option<Arc<Vec<usize>>>,
    /// Font size for log view and context panel (points).
    pub log_font_size: f32,
    /// Saved pin entries.
    pub pins: Vec<PinEntry>,
    /// Whether the bottom panel is expanded.
    pub bottom_panel_open: bool,
    /// Text input buffer for the pin comment modal.
    pub pin_comment: String,
    /// Range being edited in the pin modal (if any).
    pub pin_modal: Option<(usize, usize)>,
    /// Index into `pins` of the pin currently being edited in the pin modal
    /// (None = the modal is creating a brand-new pin).
    pub pin_edit_index: Option<usize>,
    /// Multi-line drag selection state.
    pub selection_range: Option<(usize, usize)>,
    pub pending_selection: Option<(usize, usize)>,
    pub drag_selecting: bool,
    pub drag_start_line: Option<usize>,
    pub drag_current_line: Option<usize>,
    pub drag_start_pos: Option<egui::Pos2>,
    pub selection_popup_pos: Option<egui::Pos2>,
    pub selection_popup_opened_at: Option<std::time::Instant>,
    /// First and last *real* line index visible in the log viewport.
    pub viewport_range: Option<(usize, usize)>,
    /// Real line index to place at the top of the log viewport after a
    /// filter change (set by rebuild_visible_lines, consumed by log_view).
    pub preserve_anchor: Option<usize>,
    pub applied_filter: Option<String>,

    pub find_input: String,
    pub find_query: String,
    /// Whether Log View find matches must use the exact ASCII letter case.
    pub find_case_sensitive: bool,
    pub find_matches: Vec<usize>,
    pub find_pos: Option<usize>,
    pub find_automaton: Option<Arc<AhoCorasick>>,
    pub find_rx: Option<(Receiver<Vec<usize>>, Arc<AtomicBool>)>,
    pub keyword_highlight: Option<String>,
    pub keyword_automaton: Option<Arc<AhoCorasick>>,

    /// Structured payloads intersecting the most recently analyzed viewport.
    pub embedded_detections: Arc<Vec<Detection>>,
    /// In-flight viewport analysis + cancellation flag.
    embedded_rx: Option<(Receiver<EmbeddedScanResult>, Arc<AtomicBool>)>,
    embedded_scan_key: Option<EmbeddedScanKey>,
    embedded_pending_key: Option<EmbeddedScanKey>,
    embedded_pending_at: Option<Instant>,
    embedded_epoch: u64,
    /// Detection currently open in the persistent inspector.
    pub embedded_inspector: Option<Detection>,
    /// Screen-space anchor of the JSON cue that opened the inspector.
    pub embedded_inspector_anchor: Option<egui::Pos2>,
    pub embedded_inspector_mode: EmbeddedInspectorMode,

    /// One-shot flag set by Cmd/Ctrl+F, consumed by `show_search_ui` to focus
    /// the log search box (and select existing text). Persists until the Log
    /// view renders so the shortcut works from any dock view.
    pub search_focus_requested: bool,
    /// When the search box last gained focus via Cmd/Ctrl+F, driving the short
    /// highlight "pulse" animation on the box border.
    pub search_focus_anim: Option<Instant>,
    /// Index of the most recently added filter + when it was added, driving the
    /// short "new filter" highlight animation on the timeline lane label.
    pub filter_highlight: Option<(usize, Instant)>,

    pub dock_state: DockState<ViewTab>,
    pub detached_views: HashSet<ViewTab>,
    pub detached_locations: HashMap<ViewTab, egui_dock::TabPath>,
    pub just_closed_viewports: Vec<ViewTab>,
    pub pending_detach: Option<ViewTab>,
    /// Full dock layout snapshot taken before the first pop-out; restored when
    /// the last detached view returns so the original split layout is preserved.
    pub saved_dock_state: Option<DockState<ViewTab>>,

    /// Whether this tab is currently being served by the MCP server.
    pub mcp_serving: bool,
    /// Whether the file on disk has changed in-place (not appended), making
    /// the document's indexes invalid until a full reload.
    pub stale: bool,
}

pub struct FileLoader {
    pub path: PathBuf,
    pub name: String,
    pub rx: Receiver<LoadProgress>,
    pub cancel: Arc<AtomicBool>,
    pub stage: LoadStage,
    pub progress: f32,
}

fn nearest_occurrence<I>(occurrences: I, line: usize) -> Option<(usize, usize)>
where
    I: Iterator<Item = usize>,
{
    occurrences
        .enumerate()
        .min_by_key(|&(_, occurrence)| (occurrence.abs_diff(line), occurrence))
}

pub struct LogotomyApp {
    pub tabs: Vec<LogTab>,
    pub active: Option<usize>,
    /// Index into `loaders` of the loading file currently shown (as its own tab).
    /// When `Some`, `active` is `None` (a loading tab has no doc yet).
    pub active_loader: Option<usize>,
    pub loaders: Vec<FileLoader>,
    pub status: String,

    pub theme: Theme,
    pub dark_mode: bool,

    // Persistent settings
    pub settings: Settings,

    // MCP server state
    pub mcp_enabled: bool,
    pub mcp_port: u16,
    pub mcp_state: Option<Arc<Mutex<logotomy::mcp::ServerState>>>,
    pub mcp_thread: Option<std::thread::JoinHandle<()>>,
    pub mcp_shutdown: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Random capability ID for attaching an stdio MCP process to this GUI.
    pub mcp_session_id: Option<String>,
    pub mcp_started_at: Option<Instant>,
    /// Error message to display in a popup when MCP server fails to start.
    pub mcp_error_popup: Option<String>,

    // Toast notification state
    pub toast_message: Option<String>,
    pub toast_at: Option<Instant>,

    // Integration guide popup (opened from settings)
    pub show_integrate_popup: bool,

    // Recent files popup
    pub recent_show_dropdown: bool,
    pub recent_button_rect: Option<egui::Rect>,

    // SavedFilter management
    pub available_filters: Vec<String>,
    pub show_filter_dropdown: bool,
    pub filter_button_rect: Option<egui::Rect>,
    pub show_new_filter_popup: bool,
    pub new_filter_name: String,
    pub show_rename_filter_popup: bool,
    pub rename_filter_target: String,
    pub rename_filter_new_name: String,

    // Window management
    pub viewport_map: HashMap<egui::ViewportId, (usize, ViewTab)>,

    pub show_settings_popup: bool,
    pub settings_button_rect: Option<egui::Rect>,

    // ---- Custom date recognizers ----
    /// User-defined custom date formats (persisted to
    /// `~/.logotomy/custom_date_format_list.json`). Tried alongside built-ins
    /// when a log file is opened.
    pub custom_date_formats: Vec<CustomDateFormat>,
    pub show_custom_date_popup: bool,
    /// "Add recognizer" form state.
    pub cd_name: String,
    pub cd_regex: String,
    pub cd_sample: String,

    // File update polling
    pub last_file_check: Option<Instant>,

    /// Paths sent by a later `logotomy <file>` launch while this instance is
    /// already running.
    pub open_requests: Receiver<Vec<PathBuf>>,
}

/// Build the real-line index selected by timeline lanes. `None` is the compact
/// all-lines representation. Cancellation is checked during every large walk.
fn build_visible_lines(
    n: usize,
    matches: &[Vec<u32>],
    lane_active: &[bool],
    everything_else_active: bool,
    cancel: Option<&AtomicBool>,
) -> Option<Arc<Vec<usize>>> {
    if n == 0 || (everything_else_active && lane_active.iter().all(|&a| a)) {
        return None;
    }
    let cancelled =
        |i: usize| i % 16_384 == 0 && cancel.is_some_and(|flag| flag.load(Ordering::Relaxed));
    let mut included = vec![everything_else_active; n];
    if everything_else_active {
        for filter_matches in matches {
            for (i, &line) in filter_matches.iter().enumerate() {
                if cancelled(i) {
                    return None;
                }
                let line = line as usize;
                if line < n {
                    included[line] = false;
                }
            }
        }
    }
    for (filter_idx, &active) in lane_active.iter().enumerate() {
        if !active {
            continue;
        }
        if let Some(filter_matches) = matches.get(filter_idx) {
            for (i, &line) in filter_matches.iter().enumerate() {
                if cancelled(i) {
                    return None;
                }
                let line = line as usize;
                if line < n {
                    included[line] = true;
                }
            }
        }
    }
    let mut visible = Vec::with_capacity(n);
    for (line, &is_included) in included.iter().enumerate() {
        if cancelled(line) {
            return None;
        }
        if is_included {
            visible.push(line);
        }
    }
    (visible.len() != n).then(|| Arc::new(visible))
}

/// Render the detected format + date format summary for a document.
fn doc_format_summary(doc: &LogDocument) -> String {
    let date = doc
        .time_format_name()
        .map(|s| s.to_string())
        .or_else(|| doc.time_range.is_some().then(|| "field-based".to_string()))
        .unwrap_or_else(|| "none".to_string());
    format!("format: {} · date: {}", doc.format_name(), date)
}

impl LogotomyApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        initial_paths: Vec<PathBuf>,
        open_requests: Receiver<Vec<PathBuf>>,
    ) -> Self {
        info!("logotomy GUI starting");
        // Install the embedded Space Mono font for log text immediately, so
        // every viewport renders log lines with it from the first frame.
        crate::ui::fonts::install(&cc.egui_ctx);
        let settings = Settings::load();
        let dark_mode = settings.dark_mode;

        // Ensure filter directory exists
        let filters_dir = Settings::filters_dir();
        if let Err(e) = std::fs::create_dir_all(&filters_dir) {
            error!("failed to create filters directory: {e}");
        }

        let available_filters = Self::load_available_filters();
        let custom_date_formats = Settings::load_custom_date_formats();

        info!("settings loaded: dark_mode={dark_mode}, recent_files={}, filters={}, custom_date_formats={}",
            settings.recent_files.len(), available_filters.len(), custom_date_formats.len());

        let mut app = Self {
            tabs: Vec::new(),
            active: None,
            active_loader: None,
            loaders: Vec::new(),
            status: "Drop a log file anywhere. Go on.".to_string(),
            theme: if dark_mode {
                Theme::dark()
            } else {
                Theme::light()
            },
            dark_mode,
            settings,
            mcp_enabled: false,
            mcp_port: 0,
            mcp_state: None,
            mcp_thread: None,
            mcp_shutdown: None,
            mcp_session_id: None,
            mcp_started_at: None,
            mcp_error_popup: None,
            toast_message: None,
            toast_at: None,
            show_integrate_popup: false,
            recent_show_dropdown: false,
            recent_button_rect: None,
            available_filters,
            show_filter_dropdown: false,
            filter_button_rect: None,
            show_new_filter_popup: false,
            new_filter_name: String::new(),
            show_rename_filter_popup: false,
            rename_filter_target: String::new(),
            rename_filter_new_name: String::new(),
            viewport_map: HashMap::new(),

            show_settings_popup: false,
            settings_button_rect: None,
            custom_date_formats,
            show_custom_date_popup: false,
            cd_name: String::new(),
            cd_regex: String::new(),
            cd_sample: String::new(),
            last_file_check: Some(Instant::now()),
            open_requests,
        };
        for path in initial_paths {
            app.open_file(path);
        }
        app
    }

    fn load_available_filters() -> Vec<String> {
        let filters_dir = Settings::filters_dir();
        let mut filters = Vec::new();
        if let Ok(entries) = std::fs::read_dir(filters_dir) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if name.ends_with(".json") {
                        filters.push(name.trim_end_matches(".json").to_string());
                    }
                }
            }
        }
        filters.sort();
        filters
    }

    pub fn toggle_theme(&mut self) {
        self.dark_mode = !self.dark_mode;
        self.theme = if self.dark_mode {
            Theme::dark()
        } else {
            Theme::light()
        };
        self.settings.dark_mode = self.dark_mode;
        self.settings.save();
        // Re-color filter lanes for the new theme so already-open filters
        // immediately pick up the mode-appropriate palette.
        let colors = &self.theme.filter_colors;
        for tab in &mut self.tabs {
            for (i, f) in tab.filters.iter_mut().enumerate() {
                f.color = colors[i % colors.len()];
            }
        }
        // Re-render icons with the new theme color.
        icons::clear_cache();
    }

    /// Detected log format + date format for the selected log (None when no log open).
    pub fn selected_log_format_status(&self) -> Option<String> {
        let idx = self.active?;
        Some(doc_format_summary(&self.tabs[idx].doc))
    }

    pub fn open_file(&mut self, path: PathBuf) {
        let path = crate::ui::instance::normalize_path_for_app(path);
        if let Some(i) = self.tabs.iter().position(|t| t.doc.path == path) {
            self.active = Some(i);
            self.status = "That file is already open. Nice try though.".to_string();
            info!("file already open: {}", path.display());
            return;
        }
        if self.loaders.iter().any(|loader| loader.path == path) {
            self.status = "That file is already opening.".to_string();
            info!("file is already loading: {}", path.display());
            return;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        info!("opening file: {} ({})", path.display(), name);
        let (tx, rx) = crossbeam_channel::unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_worker = Arc::clone(&cancel);
        let path_for_load = path.clone();
        let parsing = ParsingConfig {
            sim_threshold: self.settings.sim_threshold,
            header_sample_lines: self.settings.header_sample_lines,
            drain_depth: self.settings.drain_depth,
        };
        // Compile the user's custom date recognizers once and hand them to the
        // loader so time detection considers them alongside the built-ins.
        let custom_for_load: Vec<CustomTimeFormat> = self
            .custom_date_formats
            .iter()
            .filter_map(|d| d.compile().ok())
            .collect();
        std::thread::spawn(move || {
            LogDocument::load_with_custom(
                &path_for_load,
                parsing,
                &custom_for_load,
                tx,
                cancel_worker,
            )
        });
        self.loaders.push(FileLoader {
            path: path.clone(),
            name,
            rx,
            cancel,
            stage: LoadStage::Indexing,
            progress: 0.0,
        });
        // The loading file is shown in its own (new) log tab, so focus it.
        self.active_loader = Some(self.loaders.len() - 1);
        self.active = None;
        // Track in recent files
        self.settings.add_recent_file(path);
        self.settings.save();
    }

    /// Handle paths sent by a later shell/file-association launch and bring
    /// this native window to the foreground.
    pub fn poll_open_requests(&mut self, ctx: &egui::Context) {
        let mut received = false;
        while let Ok(paths) = self.open_requests.try_recv() {
            received = true;
            for path in paths {
                self.open_file(path);
            }
        }
        if received {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }

    /// Re-open the active log with the current custom date formats, so
    /// recognizers added *after* the file was opened take effect without a
    /// manual close/reopen. Any per-tab state (filters, pins, scroll) is reset.
    pub fn reopen_active_with_custom(&mut self) {
        let Some(idx) = self.active else { return };
        let path = self.tabs[idx].doc.path.clone();
        self.close_tab(idx);
        self.active = None;
        self.open_file(path);
    }

    pub fn poll_file_updates(&mut self) {
        if self
            .last_file_check
            .map_or(true, |t| t.elapsed() > Duration::from_secs(2))
        {
            self.check_for_file_updates();
            self.last_file_check = Some(Instant::now());
        }
    }

    pub fn check_for_file_updates(&mut self) {
        if let Some(active_idx) = self.active {
            if let Some(tab) = self.tabs.get_mut(active_idx) {
                // Probe first while the document is still shared. In GUI+MCP
                // mode the server intentionally holds another Arc, so calling
                // Arc::make_mut before this check cloned every per-line index
                // every two seconds even when the file was unchanged.
                match tab.doc.file_change() {
                    Ok(FileChange::Unchanged) => {}
                    Ok(FileChange::Appended) => {
                        tab.start_tail_update();
                    }
                    Ok(FileChange::Shrunk | FileChange::Modified) => {
                        let file_name = tab.doc.file_name.clone();
                        log::warn!(
                            "File {} changed on disk and requires a full reload",
                            file_name
                        );
                        self.status = format!("'{file_name}' changed on disk — only tailing is supported. Close and reopen the file.");
                        tab.stale = true;
                    }
                    Err(e) => {
                        log::warn!("failed to check {} for updates: {e}", tab.doc.file_name);
                        self.status =
                            format!("failed to check '{}' for updates: {e}", tab.doc.file_name);
                    }
                }
            }
        }
    }

    /// Install completed background tail updates before checking for further
    /// changes, so a steady writer coalesces into the next staged append.
    pub fn poll_tail_updates(&mut self) {
        for tab in &mut self.tabs {
            let Some(result) = tab.poll_tail_update() else {
                continue;
            };
            match result {
                Ok((new_lines, file_name)) => {
                    self.status = format!("Loaded {new_lines} new lines from '{file_name}'.");
                    log::info!("File {file_name} was appended. Loaded {new_lines} new lines.");
                }
                Err(error) => {
                    let file_name = tab.doc.file_name.clone();
                    log::warn!("File {file_name} append failed: {error}");
                    self.status = format!("'{file_name}' changed on disk — only tailing is supported. Close and reopen the file.");
                    tab.stale = true;
                }
            }
        }
    }

    pub fn poll_loaders(&mut self) {
        let mut i = 0;
        while i < self.loaders.len() {
            let mut remove = false;
            match self.loaders[i].rx.try_recv() {
                Ok(LoadProgress::Progress { stage, done, total }) => {
                    self.loaders[i].stage = stage;
                    self.loaders[i].progress = if total > 0 {
                        (done as f32 / total as f32).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                }
                Ok(LoadProgress::Done(doc)) => {
                    let n = doc.total_lines();
                    let mb = doc.file_size as f64 / 1e6;
                    info!(
                        "loaded {} — {} lines, {:.1} MB, {} templates",
                        self.loaders[i].name,
                        n,
                        mb,
                        doc.templates.len()
                    );
                    self.status = format!(
                        "Loaded `{}` — `{}` lines, {:.1} MB, {} templates.",
                        self.loaders[i].name,
                        n,
                        mb,
                        doc.templates.len()
                    );
                    let mut new_tab = LogTab::new_with_inspector_mode(
                        *doc,
                        EmbeddedInspectorMode::from_persisted_name(
                            &self.settings.embedded_inspector_mode,
                        ),
                    );
                    if let Some(filter_name) = self.settings.default_filter.clone() {
                        apply_filter_to_tab(&mut new_tab, &filter_name, &self.theme);
                    }
                    self.tabs.push(new_tab);

                    // If MCP is running, share the new file with the MCP server
                    if let Some(ref mcp_state) = self.mcp_state {
                        if let Ok(mut guard) = mcp_state.lock() {
                            if let Some(idx) = self.tabs.last() {
                                // If no active doc is set yet, set this one
                                if guard.active_doc.is_none() {
                                    guard.set_active_doc(Arc::clone(&idx.doc));
                                    if let Some(active_idx) = self.tabs.len().checked_sub(1) {
                                        self.tabs[active_idx].mcp_serving = true;
                                    }
                                    info!("MCP: set active doc to newly loaded file");
                                }
                            }
                        }
                    }
                    self.active = Some(self.tabs.len() - 1);
                    self.active_loader = None;
                    remove = true;
                }
                Ok(LoadProgress::Error(e)) => {
                    error!("failed to load {}: {e}", self.loaders[i].name);
                    self.status = format!("{}: {e}", self.loaders[i].name);
                    self.active_loader = None;
                    remove = true;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {}
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.active_loader = None;
                    remove = true;
                }
            }
            if remove {
                self.adjust_active_loader_on_remove(i);
                self.loaders.remove(i);
            } else {
                i += 1;
            }
        }
    }

    /// Keep `active_loader` pointing at the correct loader as earlier ones are
    /// removed from `self.loaders` (removal shifts later indices down).
    fn adjust_active_loader_on_remove(&mut self, removed_idx: usize) {
        if let Some(al) = self.active_loader {
            if al == removed_idx {
                self.active_loader = None;
            } else if al > removed_idx {
                self.active_loader = Some(al - 1);
            }
        }
    }

    /// Poll the MCP server's dirty flag. If the active doc was modified by an
    /// MCP tool call, re-read it and refresh the UI.
    pub fn poll_mcp_dirty(&mut self) {
        let Some(ref mcp_state) = self.mcp_state else {
            return;
        };
        let Some(active_idx) = self.active else {
            return;
        };

        let dirty = {
            let guard = mcp_state.lock().unwrap();
            guard.active_doc_dirty.load(Ordering::Relaxed)
        };
        if !dirty {
            return;
        }

        // Reset the dirty flag and refresh the active tab
        {
            let guard = mcp_state.lock().unwrap();
            guard.active_doc_dirty.store(false, Ordering::Relaxed);
        }

        // Re-read the active doc from MCP state and update the tab
        if let Some(tab) = self.tabs.get_mut(active_idx) {
            let guard = mcp_state.lock().unwrap();
            if let Some(ref mcp_doc) = guard.active_doc {
                // Only refresh if the doc pointer changed (was mutated)
                if !Arc::ptr_eq(&tab.doc, mcp_doc) {
                    tab.doc = Arc::clone(mcp_doc);
                    tab.invalidate_embedded_data();
                    tab.rescan_filters();
                    // The swapped-in doc may be trimmed/smaller than the previous
                    // one; clamp stale view indices so the next frame's ts_at()
                    // lookups can't run out of bounds (see clamp_view_state).
                    tab.clamp_view_state();
                    info!("MCP: refreshed UI from dirty doc");
                }
            }
        }
    }

    /// Push any GUI-originated document mutations (trim / trim-reset /
    /// file-append) back into the MCP server state. GUI mutations use
    /// `Arc::make_mut`, which deep-copies the document whenever the MCP
    /// server holds another reference — leaving the server with a stale
    /// pre-mutation Arc. This swap keeps `ServerState::active_doc` pointing
    /// at the same Arc the GUI is displaying, and `set_active_doc` drops the
    /// stale `_active` match-cache entries as a side effect.
    pub fn sync_mcp_active_doc(&mut self) {
        let Some(ref mcp_state) = self.mcp_state else {
            return;
        };
        for tab in &self.tabs {
            if !tab.mcp_serving {
                continue;
            }
            let mut guard = mcp_state.lock().unwrap();
            let stale = guard
                .active_doc
                .as_ref()
                .map_or(true, |d| !Arc::ptr_eq(d, &tab.doc));
            if stale {
                guard.set_active_doc(Arc::clone(&tab.doc));
                // GUI-originated change — the GUI already holds the newest
                // doc, so don't trigger the MCP→GUI refresh cycle.
                guard.active_doc_dirty.store(false, Ordering::Relaxed);
                info!("MCP: synced GUI-mutated doc into server state");
            }
        }
    }

    /// Push any GUI-originated filter-set changes (toolbar add/remove, saved
    /// filter apply, lane edits) into the MCP server's `_active` filter list.
    /// Only the filter texts are synced — the "Everything Else" lane and lane
    /// toggles are GUI-only and never flow into MCP arithmetic.
    pub fn sync_mcp_filters(&mut self) {
        let Some(ref mcp_state) = self.mcp_state else {
            return;
        };
        for tab in &self.tabs {
            if !tab.mcp_serving {
                continue;
            }
            let texts: Vec<String> = tab.filters.iter().map(|f| f.text.clone()).collect();
            let mut guard = mcp_state.lock().unwrap();
            if guard.get_filters("_active") != texts {
                guard.set_filters("_active", texts);
                // GUI-originated change — the GUI already shows the newest
                // filter set, so don't trigger the MCP→GUI re-apply.
                guard.filters_dirty.store(false, Ordering::Relaxed);
                info!("MCP: synced GUI filters into server state");
            }
        }
    }

    /// Poll the MCP server's `filters_dirty` flag. When the filter set was
    /// modified by an MCP tool call (filters_add/filters_remove), re-apply it
    /// to the served tab so the GUI lanes match what the agent set.
    pub fn poll_mcp_filters(&mut self) {
        let Some(ref mcp_state) = self.mcp_state else {
            return;
        };
        let dirty = mcp_state
            .lock()
            .unwrap()
            .filters_dirty
            .load(Ordering::Relaxed);
        if !dirty {
            return;
        }
        let filters: Vec<String> = {
            let guard = mcp_state.lock().unwrap();
            guard.filters_dirty.store(false, Ordering::Relaxed);
            guard.get_filters("_active")
        };
        if let Some(tab) = self.active.and_then(|i| self.tabs.get_mut(i)) {
            tab.filters.clear();
            let colors = &self.theme.filter_colors;
            for (i, text) in filters.into_iter().take(MAX_FILTERS).enumerate() {
                tab.filters.push(Filter {
                    text,
                    color: colors[i % colors.len()],
                });
            }
            tab.rescan_filters();
            info!("MCP: re-applied filters from server state");
        }
    }

    pub fn close_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        info!("closing tab {} ({})", idx, self.tabs[idx].doc.file_name);
        if let Some((_, cancel)) = &self.tabs[idx].search_rx {
            cancel.store(true, Ordering::Relaxed);
        }
        if let Some((_, cancel)) = &self.tabs[idx].visible_rx {
            cancel.store(true, Ordering::Relaxed);
        }
        if let Some((_, cancel)) = &self.tabs[idx].find_rx {
            cancel.store(true, Ordering::Relaxed);
        }

        // If the closed tab was being served by MCP, update MCP state
        let was_serving = self.tabs[idx].mcp_serving;
        self.tabs.remove(idx);

        if was_serving {
            if let Some(ref mcp_state) = self.mcp_state {
                let mut guard = mcp_state.lock().unwrap();
                // Try to serve another tab
                let new_active = if self.tabs.is_empty() {
                    None
                } else {
                    let new_idx = idx.min(self.tabs.len() - 1);
                    self.tabs[new_idx].mcp_serving = true;
                    Some(Arc::clone(&self.tabs[new_idx].doc))
                };
                match new_active {
                    Some(doc) => {
                        guard.set_active_doc(doc);
                        // Seed `_active` filters from the newly served tab
                        // (GUI-originated → clear the MCP→GUI dirty flag).
                        let new_idx = idx.min(self.tabs.len() - 1);
                        let texts: Vec<String> = self.tabs[new_idx]
                            .filters
                            .iter()
                            .map(|f| f.text.clone())
                            .collect();
                        guard.set_filters("_active", texts);
                        guard.filters_dirty.store(false, Ordering::Relaxed);
                        info!("MCP: switched active doc to another tab");
                    }
                    None => {
                        guard.clear_active_doc();
                        info!("MCP: no more tabs, cleared active doc");
                    }
                }
            }
        }

        self.active = if self.tabs.is_empty() {
            None
        } else {
            Some(idx.min(self.tabs.len() - 1))
        };
    }

    /// Temporary session ID used by an already-configured `logotomy --mcp`
    /// server to attach to this GUI instance. The private IPC socket remains
    /// an implementation detail and is never shown to the user.
    pub(crate) fn mcp_session_id(&self) -> Option<&str> {
        self.mcp_enabled
            .then_some(self.mcp_session_id.as_deref())
            .flatten()
            .filter(|session_id| !session_id.is_empty())
    }

    /// A ready-to-paste instruction for a coding agent, pointing it at the
    /// running MCP server and the active log file.
    pub fn mcp_instruction(&self) -> Option<String> {
        let session_id = self.mcp_session_id()?;
        let log_path = self
            .active
            .and_then(|i| self.tabs.get(i))
            .map(|tab| tab.doc.path.display().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        Some(Self::build_mcp_instruction(session_id, &log_path))
    }

    /// Build a prompt that attaches the normal stdio MCP server to the active
    /// GUI session. GUI mode already serves the active document, so the agent
    /// must not call `load_log` or pass `log_id`.
    fn build_mcp_instruction(session_id: &str, log_path: &str) -> String {
        format!(
            "Use the already-configured Logotomy MCP server (`logotomy --mcp`) for this task.\n\n\
If Logotomy MCP is not configured or its tools are unavailable, do not attempt a different \
transport. Ask the user to configure it first using: Logotomy → Settings → Integrate with AI \
Assistant → follow the guide. Continue only after the configured server exposes \
`attach_gui_session`.\n\n\
Call `attach_gui_session` once with this temporary GUI session ID:\n{session_id}\n\n\
Then call `session_info` and confirm that `mode` is `gui_attached`. The currently selected \
log is already available from the GUI:\n{log_path}\n\n\
In `gui_attached` mode, do not call `load_log` and do not pass `log_id`. Start with \
`summarize_log` using `with_filtered_log=false` unless the existing GUI filters are \
intentionally relevant. Read `logotomy://guide` if MCP resources are supported.\n\n\
The session ID expires when MCP is stopped or Logotomy exits. Treat it as a local secret: \
do not save it in MCP configuration, write it to files, print it, or expose it in your response. \
If attachment fails, ask the user to start MCP in Logotomy and provide a new session ID."
        )
    }

    /// Show a self-dismissing toast notification for a few seconds.
    pub fn show_toast(&mut self, message: String) {
        self.toast_message = Some(message);
        self.toast_at = Some(Instant::now());
    }

    pub fn start_mcp(&mut self) {
        if self.mcp_enabled {
            return;
        }
        if self.tabs.is_empty() {
            self.status = "Open a log file first before starting MCP server.".to_string();
            info!("MCP: cannot start — no tabs open");
            return;
        }
        // Remove a manifest left by an unclean previous process before
        // publishing this GUI-owned session.
        logotomy::mcp::session::clear();
        self.status = "Starting MCP server…".to_string();
        // Dynamic port (OS-assigned) + a fresh 256-bit credential per session.
        let session_id = logotomy::mcp::session::generate_session_id();
        info!("starting authenticated private MCP GUI socket");

        let state = Arc::new(Mutex::new(logotomy::mcp::ServerState::default()));
        {
            let mut guard = state.lock().unwrap();
            // Use the active tab's document directly (no disk reload)
            if let Some(active_idx) = self.active {
                let doc = Arc::clone(&self.tabs[active_idx].doc);
                guard.set_active_doc(doc);
                // Seed the server's filter set from the served tab's live
                // filters so `with_filtered_log=true` (the default) starts out
                // matching what the user is viewing. GUI-originated, so clear
                // the MCP→GUI dirty flag.
                let texts: Vec<String> = self.tabs[active_idx]
                    .filters
                    .iter()
                    .map(|f| f.text.clone())
                    .collect();
                guard.set_filters("_active", texts);
                guard.filters_dirty.store(false, Ordering::Relaxed);
                self.tabs[active_idx].mcp_serving = true;
                info!("MCP: set active doc from tab {}", active_idx);
            }
        }

        let port = 0u16; // 0 = OS-assigned dynamic port
        let state_clone = Arc::clone(&state);
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);

        // Channel to receive the bind result from the server thread
        let (bind_tx, bind_rx) = std::sync::mpsc::channel();

        let session_id_for_thread = session_id.clone();
        let thread = std::thread::Builder::new()
            .name("mcp-server".to_string())
            .spawn(move || {
                let _ = logotomy::mcp::run_gui_ipc(
                    port,
                    state_clone,
                    shutdown_clone,
                    Some(bind_tx),
                    session_id_for_thread,
                );
            });

        match thread {
            Ok(handle) => {
                // Wait briefly for the bind result. A little headroom avoids
                // reporting a false failure when the GUI is under load.
                // run_gui_ipc reports the bind result immediately after binding,
                // so a timeout means the server thread failed to start at all.
                let bind_result = bind_rx.recv_timeout(Duration::from_millis(500));
                match bind_result {
                    Ok(Ok(actual_port)) => {
                        if let Err(error) =
                            logotomy::mcp::session::publish(actual_port, session_id.clone())
                        {
                            shutdown.store(true, Ordering::Relaxed);
                            let _ = handle.join();
                            self.status = format!("MCP: {error}");
                            self.mcp_error_popup = Some(format!(
                                "MCP server failed to publish its GUI bridge session:\n\n{error}"
                            ));
                            if let Some(active_idx) = self.active {
                                self.tabs[active_idx].mcp_serving = false;
                            }
                            return;
                        }
                        self.mcp_thread = Some(handle);
                        self.mcp_state = Some(state);
                        self.mcp_shutdown = Some(shutdown);
                        self.mcp_port = actual_port;
                        self.mcp_session_id = Some(session_id);
                        self.mcp_enabled = true;
                        self.mcp_started_at = Some(Instant::now());
                        self.status = format!(
                            "Private MCP session ready — serving '{}'",
                            self.tabs[self.active.unwrap()].doc.file_name
                        );
                        info!("private MCP GUI session started");
                    }
                    Ok(Err(e)) => {
                        // Bind failed — clean up and show popup
                        error!("MCP: failed to bind: {e}");
                        self.status = format!("MCP: {e}");
                        self.mcp_error_popup = Some(format!("MCP server failed to start:\n\n{e}"));
                        // Clear serving flag
                        if let Some(active_idx) = self.active {
                            if active_idx < self.tabs.len() {
                                self.tabs[active_idx].mcp_serving = false;
                            }
                        }
                    }
                    Err(_) => {
                        // Timeout — the server thread never reported a bind result.
                        // Report an error instead of assuming success.
                        error!("MCP: timed out waiting for server thread to bind");
                        self.status = "MCP: timed out waiting for server to bind".to_string();
                        self.mcp_error_popup = Some("MCP server failed to start:\n\nTimed out waiting for the server thread to bind.".to_string());
                        // Clear serving flag
                        if let Some(active_idx) = self.active {
                            if active_idx < self.tabs.len() {
                                self.tabs[active_idx].mcp_serving = false;
                            }
                        }
                    }
                }
            }
            Err(e) => {
                error!("MCP: failed to start thread: {e}");
                self.status = format!("MCP: failed to start: {e}");
            }
        }
    }

    pub fn stop_mcp(&mut self) {
        info!("stopping MCP server");
        // Signal the server thread to shut down
        if let Some(shutdown) = self.mcp_shutdown.take() {
            shutdown.store(true, Ordering::Relaxed);
        }
        // Wait for the server thread to finish (with a timeout)
        if let Some(handle) = self.mcp_thread.take() {
            if handle.thread().id() != std::thread::current().id() {
                let _ = handle.join();
            }
        }
        // Clear mcp_serving flags
        for tab in &mut self.tabs {
            tab.mcp_serving = false;
        }
        self.mcp_enabled = false;
        self.mcp_port = 0;
        self.mcp_session_id = None;
        self.mcp_state = None;
        self.mcp_shutdown = None;
        self.mcp_started_at = None;
        logotomy::mcp::session::clear();
        self.status = "MCP server stopped".to_string();
        info!("MCP server shut down");
    }

    /// Called when the active tab changes. If MCP is running, update the
    /// active doc in the server state.
    pub fn on_tab_switched(&mut self, old_idx: Option<usize>, new_idx: usize) {
        if !self.mcp_enabled {
            return;
        }
        if let Some(ref mcp_state) = self.mcp_state {
            let mut guard = mcp_state.lock().unwrap();
            // Clear old serving flag
            if let Some(old) = old_idx {
                if old < self.tabs.len() {
                    self.tabs[old].mcp_serving = false;
                }
            }
            // Set new serving flag
            if new_idx < self.tabs.len() {
                self.tabs[new_idx].mcp_serving = true;
                guard.set_active_doc(Arc::clone(&self.tabs[new_idx].doc));
                // Seed `_active` filters from the newly served tab
                // (GUI-originated → clear the MCP→GUI dirty flag).
                let texts: Vec<String> = self.tabs[new_idx]
                    .filters
                    .iter()
                    .map(|f| f.text.clone())
                    .collect();
                guard.set_filters("_active", texts);
                guard.filters_dirty.store(false, Ordering::Relaxed);
                info!(
                    "MCP: switched active doc to tab {} ({})",
                    new_idx, self.tabs[new_idx].doc.file_name
                );
                let served_name = self.tabs[new_idx].doc.file_name.clone();
                drop(guard);
                self.show_toast(format!("MCP now serving '{served_name}'"));
            }
        }
    }

    pub fn apply_filter(&mut self, filter_name: &str) {
        if let Some(active_tab) = self.active.and_then(|i| self.tabs.get_mut(i)) {
            apply_filter_to_tab(active_tab, filter_name, &self.theme);
        }
    }

    pub fn save_filter(&mut self, filter_name: &str) {
        if filter_name.is_empty() {
            return;
        }
        let Some(active_tab) = self.active.and_then(|i| self.tabs.get(i)) else {
            return;
        };
        let filters: Vec<String> = active_tab.filters.iter().map(|k| k.text.clone()).collect();
        let filter = SavedFilter { filters };
        let filter_path = Settings::filters_dir().join(format!("{filter_name}.json"));

        match serde_json::to_string_pretty(&filter) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&filter_path, &text) {
                    error!("failed to write filter '{}': {e}", filter_path.display());
                } else {
                    info!("saved filter '{}'", filter_path.display());
                    self.available_filters = Self::load_available_filters();
                }
            }
            Err(e) => error!("failed to serialize filter '{filter_name}': {e}"),
        }
    }

    pub fn rename_filter(&mut self, old_name: &str, new_name: &str) {
        if old_name == new_name || new_name.is_empty() {
            return;
        }
        let old_path = Settings::filters_dir().join(format!("{old_name}.json"));
        let new_path = Settings::filters_dir().join(format!("{new_name}.json"));
        if new_path.exists() {
            error!("filter already exists: {}", new_path.display());
            return;
        }
        if let Err(e) = std::fs::rename(&old_path, &new_path) {
            error!("failed to rename filter '{old_name}' to '{new_name}': {e}");
        } else {
            info!("renamed filter '{old_name}' to '{new_name}'");
            self.available_filters = Self::load_available_filters();
            for tab in &mut self.tabs {
                if tab.applied_filter.as_deref() == Some(old_name) {
                    tab.applied_filter = Some(new_name.to_string());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_line_builder_applies_everything_else_and_lane_toggles() {
        let matches = vec![vec![1, 3, 6], vec![3, 4]];

        let only_everything_else =
            build_visible_lines(8, &matches, &[false, false], true, None).unwrap();
        assert_eq!(&*only_everything_else, &[0, 2, 5, 7]);

        let first_lane_only =
            build_visible_lines(8, &matches, &[true, false], false, None).unwrap();
        assert_eq!(&*first_lane_only, &[1, 3, 6]);

        assert!(build_visible_lines(8, &matches, &[true, true], true, None).is_none());
    }

    #[test]
    fn gui_mcp_instruction_is_actionable_and_safe() {
        let prompt = LogotomyApp::build_mcp_instruction("123456", "/tmp/example.log");
        assert!(prompt.contains("attach_gui_session"));
        assert!(prompt.contains("123456"));
        assert!(prompt.contains("/tmp/example.log"));
        assert!(prompt.contains("gui_attached"));
        assert!(prompt.contains("do not call `load_log`"));
        assert!(prompt.contains("do not pass `log_id`"));
        assert!(prompt.contains("with_filtered_log=false"));
        assert!(prompt.contains("do not save it in MCP configuration"));
        assert!(prompt.contains("logotomy --mcp"));
        assert!(prompt.contains("session_info"));
        assert!(prompt.contains("If Logotomy MCP is not configured"));
        assert!(prompt.contains("Logotomy → Settings → Integrate with AI Assistant"));
        assert!(prompt.contains("Continue only after"));
        assert!(!prompt.contains("http://"));
        assert!(!prompt.contains("Authorization"));
        assert!(!prompt.contains("--mcp-gui"));
    }

    fn write_temp(content: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "logotomy_app_model_test_{}_{}.log",
            std::process::id(),
            n
        ));
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn doc_format_summary_reports_format_and_date() {
        // JSON: field-based timestamp (no positional date format).
        let json_path = write_temp(
            "{\"time\": \"2026-08-15T19:40:01Z\", \"lvl\": 30, \"msg\": \"Page load\"}\n",
        );
        let json_doc = LogDocument::open(&json_path).unwrap();
        assert_eq!(
            doc_format_summary(&json_doc),
            "format: json · date: field-based"
        );

        // Plain ISO text.
        let plain_path = write_temp("2026-08-15 19:40:30.123456+0300 INFO hello\n");
        let plain_doc = LogDocument::open(&plain_path).unwrap();
        assert_eq!(
            doc_format_summary(&plain_doc),
            "format: plain · date: ISO-8601"
        );

        // CEF: timeless.
        let cef_path = write_temp("CEF:0|Vendor|Product|1.0|100|Name|3|spt=443\n");
        let cef_doc = LogDocument::open(&cef_path).unwrap();
        assert_eq!(doc_format_summary(&cef_doc), "format: cef · date: none");

        std::fs::remove_file(json_path).ok();
        std::fs::remove_file(plain_path).ok();
        std::fs::remove_file(cef_path).ok();
    }

    #[test]
    fn remove_filter_removes_and_rescans() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z INFO alpha\n\
             2026-07-19T10:00:01.000Z WARN beta\n\
             2026-07-19T10:00:02.000Z INFO alpha again\n",
        );
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);

        // Add two filters.
        tab.filters.push(Filter {
            text: "alpha".into(),
            color: Theme::light().filter_colors[0],
        });
        tab.filters.push(Filter {
            text: "beta".into(),
            color: Theme::light().filter_colors[1],
        });
        tab.rescan_filters();
        // Poll until the background scan lands.
        for _ in 0..100 {
            if !tab.poll_search() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(tab.filters.len(), 2);
        assert_eq!(tab.matches.len(), 2);
        assert_eq!(tab.matches[0].len(), 2); // alpha matches 2 lines
        assert_eq!(tab.matches[1].len(), 1); // beta matches 1 line

        // Remove the first filter.
        tab.remove_filter(0);
        for _ in 0..100 {
            if !tab.poll_search() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(tab.filters.len(), 1);
        assert_eq!(tab.filters[0].text, "beta");
        assert_eq!(tab.matches.len(), 1);
        assert_eq!(tab.matches[0].len(), 1);

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn remove_last_filter_reenables_everything_else() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z INFO alpha\n\
             2026-07-19T10:00:01.000Z WARN beta\n",
        );
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);

        tab.filters.push(Filter {
            text: "alpha".into(),
            color: Theme::light().filter_colors[0],
        });
        tab.rescan_filters();
        for _ in 0..100 {
            if !tab.poll_search() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // Turn off everything else so the view would be blank after removal.
        tab.everything_else_active = false;
        tab.rebuild_visible_lines();

        tab.remove_filter(0);
        for _ in 0..100 {
            if !tab.poll_search() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(tab.filters.is_empty());
        // Everything Else must be re-enabled so the view isn't blank.
        assert!(tab.everything_else_active);

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn toggle_all_lanes_toggles_every_lane_but_keeps_everything_else() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z INFO alpha\n\
             2026-07-19T10:00:01.000Z WARN beta\n",
        );
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);

        tab.filters.push(Filter {
            text: "alpha".into(),
            color: Theme::light().filter_colors[0],
        });
        tab.filters.push(Filter {
            text: "beta".into(),
            color: Theme::light().filter_colors[1],
        });
        tab.rescan_filters();
        for _ in 0..100 {
            if !tab.poll_search() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            tab.lane_active.iter().all(|&a| a),
            "new filters start visible"
        );
        let ee_before = tab.everything_else_active;

        tab.toggle_all_lanes();
        assert!(
            tab.lane_active.iter().all(|&a| !a),
            "toggle-all hides every lane"
        );
        assert_eq!(
            tab.everything_else_active, ee_before,
            "Everything Else is never toggled"
        );

        tab.toggle_all_lanes();
        assert!(
            tab.lane_active.iter().all(|&a| a),
            "toggle-all restores every lane"
        );

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn toggle_all_lanes_reenables_everything_else_when_view_would_blank() {
        let path = write_temp("2026-07-19T10:00:00.000Z INFO alpha\n");
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);

        tab.filters.push(Filter {
            text: "alpha".into(),
            color: Theme::light().filter_colors[0],
        });
        tab.rescan_filters();
        for _ in 0..100 {
            if !tab.poll_search() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        tab.everything_else_active = false;
        tab.rebuild_visible_lines();

        // Hiding the only filter lane would blank the view, so Everything Else
        // must be re-enabled.
        tab.toggle_all_lanes();
        assert!(tab.lane_active.iter().all(|&a| !a));
        assert!(tab.everything_else_active);

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn background_visible_rebuild_installs_lane_selection() {
        let path = write_temp("alpha\nbeta\nalpha beta\ngamma\n");
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.filters = vec![
            Filter {
                text: "alpha".into(),
                color: Theme::light().filter_colors[0],
            },
            Filter {
                text: "beta".into(),
                color: Theme::light().filter_colors[1],
            },
        ];
        tab.matches = Arc::new(vec![vec![0, 2], vec![1, 2]]);
        tab.lane_active = vec![true, false];
        tab.everything_else_active = false;
        tab.rebuild_visible_lines_background();
        for _ in 0..100 {
            if !tab.poll_visible_lines() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(tab.visible_lines.as_deref(), Some(&vec![0, 2]));
        assert!(tab.visible_rx.is_none());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn background_visible_rebuild_scrolls_to_nearest_remaining_viewport_line() {
        let path = write_temp("l0\nl1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\n");
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.filters = vec![Filter {
            text: "match".into(),
            color: Theme::light().filter_colors[0],
        }];
        // The old viewport begins at line 5. The new filter leaves 3 and 8
        // around it; 3 is the nearest remaining real line.
        tab.matches = Arc::new(vec![vec![1, 3, 8]]);
        tab.lane_active = vec![true];
        tab.everything_else_active = false;
        tab.viewport_range = Some((5, 7));
        tab.rebuild_visible_lines_background();
        for _ in 0..100 {
            if !tab.poll_visible_lines() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(tab.visible_lines.as_deref(), Some(&vec![1, 3, 8]));
        assert_eq!(tab.pending_scroll, Some(3));
        assert_eq!(tab.preserve_anchor, None);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn background_tail_update_swaps_in_appended_document() {
        use std::io::Write;

        let path = write_temp("2026-07-19T10:00:00.000Z INFO first\n");
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        std::fs::File::options()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"2026-07-19T10:00:01.000Z WARN second\n")
            .unwrap();
        tab.start_tail_update();
        let mut result = None;
        for _ in 0..100 {
            if let Some(update) = tab.poll_tail_update() {
                result = Some(update.unwrap());
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            result,
            Some((1, path.file_name().unwrap().to_string_lossy().into()))
        );
        assert_eq!(tab.doc.total_lines(), 2);
        assert!(tab.tail_rx.is_none());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn clear_all_filters_removes_everything_and_reenables_everything_else() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z INFO alpha\n\
             2026-07-19T10:00:01.000Z WARN beta\n",
        );
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);

        tab.filters.push(Filter {
            text: "alpha".into(),
            color: Theme::light().filter_colors[0],
        });
        tab.filters.push(Filter {
            text: "beta".into(),
            color: Theme::light().filter_colors[1],
        });
        tab.rescan_filters();
        for _ in 0..100 {
            if !tab.poll_search() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        tab.everything_else_active = false;
        tab.rebuild_visible_lines();

        tab.clear_all_filters();
        for _ in 0..100 {
            if !tab.poll_search() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(tab.filters.is_empty());
        assert!(tab.matches.is_empty());
        assert!(
            tab.everything_else_active,
            "Everything Else must be re-enabled after clearing all filters"
        );

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn ensure_viewport_visible_recenters_when_shadow_fully_out_of_view() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z A\n\
             2026-07-19T10:00:10.000Z B\n\
             2026-07-19T10:00:20.000Z C\n\
             2026-07-19T10:00:30.000Z D\n\
             2026-07-19T10:00:40.000Z E\n",
        );
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);

        // Confirm we're on a time domain and grab its absolute bounds.
        let (full_start, _full_end) = match tab.timeline.domain {
            logotomy::core::timeline::TimelineDomain::Time { start_ms, end_ms } => {
                (start_ms, end_ms)
            }
            _ => panic!("expected time domain"),
        };

        // Narrowly zoomed to the very start; viewport scrolled to the last two lines,
        // which are entirely outside the zoom window.
        tab.timeline_zoom = Some((full_start, full_start + 10_000));
        tab.viewport_range = Some((3, 4)); // ts(3)=+30000ms, ts(4)=+40000ms

        tab.ensure_viewport_visible();

        let (s, e) = tab.timeline_zoom.expect("zoom should have been recentered");
        // Span is preserved.
        assert_eq!(e - s, 10_000);
        // The shadow midpoint (+35000ms) is centered in the new window.
        assert_eq!(s, full_start + 30_000);
        assert_eq!(e, full_start + 40_000);

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn ensure_viewport_visible_keeps_zoom_when_shadow_inside_or_partial() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z A\n\
             2026-07-19T10:00:10.000Z B\n\
             2026-07-19T10:00:20.000Z C\n\
             2026-07-19T10:00:30.000Z D\n\
             2026-07-19T10:00:40.000Z E\n",
        );
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);

        let (full_start, _full_end) = match tab.timeline.domain {
            logotomy::core::timeline::TimelineDomain::Time { start_ms, end_ms } => {
                (start_ms, end_ms)
            }
            _ => panic!("expected time domain"),
        };

        // Case 1: shadow fully inside the zoom window -> unchanged.
        tab.timeline_zoom = Some((full_start, full_start + 40_000));
        tab.viewport_range = Some((1, 2)); // +10000..+20000ms
        tab.ensure_viewport_visible();
        assert_eq!(tab.timeline_zoom, Some((full_start, full_start + 40_000)));

        // Case 2: shadow partially overlaps the zoom window (not fully out) -> unchanged.
        tab.timeline_zoom = Some((full_start + 5_000, full_start + 25_000));
        tab.viewport_range = Some((2, 3)); // +20000..+30000ms, pokes past +25000
        tab.ensure_viewport_visible();
        assert_eq!(
            tab.timeline_zoom,
            Some((full_start + 5_000, full_start + 25_000))
        );

        // Case 3: no viewport range -> unchanged (early return).
        tab.viewport_range = None;
        tab.ensure_viewport_visible();
        assert_eq!(
            tab.timeline_zoom,
            Some((full_start + 5_000, full_start + 25_000))
        );

        std::fs::remove_file(path).ok();
    }

    /// Regression: after an MCP dirty-doc swap, `viewport_range` can still hold
    /// line indices that exceed the (smaller) current window. `ensure_viewport_visible`
    /// must not let the unchecked `ts_at()` index out of bounds — it skips instead.
    #[test]
    fn ensure_viewport_visible_ignores_stale_out_of_range_viewport() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z A\n\
             2026-07-19T10:00:10.000Z B\n\
             2026-07-19T10:00:20.000Z C\n\
             2026-07-19T10:00:30.000Z D\n\
             2026-07-19T10:00:40.000Z E\n",
        );
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        let (full_start, _full_end) = match tab.timeline.domain {
            logotomy::core::timeline::TimelineDomain::Time { start_ms, end_ms } => {
                (start_ms, end_ms)
            }
            _ => panic!("expected time domain"),
        };
        tab.timeline_zoom = Some((full_start, full_start + 40_000));
        // Stale viewport far beyond the 5-line doc (like after a doc swap).
        tab.viewport_range = Some((100, 100));
        tab.ensure_viewport_visible();
        // No panic; zoom left untouched because the shadow can't be mapped.
        assert_eq!(tab.timeline_zoom, Some((full_start, full_start + 40_000)));
        std::fs::remove_file(path).ok();
    }

    /// Regression: a stale `context_line` beyond the current window must make
    /// `ensure_visible` bail out instead of panicking in `ts_at`.
    #[test]
    fn ensure_visible_ignores_stale_out_of_range_context_line() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z A\n\
             2026-07-19T10:00:10.000Z B\n",
        );
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.context_line = Some(500);
        tab.ensure_visible();
        std::fs::remove_file(path).ok();
    }

    /// `clamp_view_state` (run on MCP doc swap) brings every doc-positioned view
    /// field back into the current window, so the next frame can't OOB index.
    #[test]
    fn clamp_view_state_brings_stale_indices_back_in_range() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z A\n\
             2026-07-19T10:00:10.000Z B\n\
             2026-07-19T10:00:20.000Z C\n\
             2026-07-19T10:00:30.000Z D\n\
             2026-07-19T10:00:40.000Z E\n\
             x\n",
        );
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        let before = tab.doc.total_lines();

        tab.context_line = Some(1060);
        tab.pending_scroll = Some(2000);
        tab.preserve_anchor = Some(3000);
        tab.drag_start_line = Some(4000);
        tab.drag_current_line = Some(5000);
        tab.viewport_range = Some((999, 1999));
        tab.pin_modal = Some((5000, 6000));
        tab.selection_range = Some((100, 200));
        tab.pending_selection = Some((300, 400));

        tab.clamp_view_state();

        for v in [
            tab.context_line,
            tab.pending_scroll,
            tab.preserve_anchor,
            tab.drag_start_line,
            tab.drag_current_line,
        ]
        .iter()
        .flatten()
        {
            assert!(*v < before, "position index {v} not clamped to < {before}");
        }
        for (a, b) in [
            tab.viewport_range,
            tab.pin_modal,
            tab.selection_range,
            tab.pending_selection,
        ]
        .iter()
        .flatten()
        {
            assert!(
                *a < before && *b < before,
                "range ({a},{b}) not clamped to < {before}"
            );
        }
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn find_next_and_prev_wrap_around_and_noop_on_empty() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z err a\n\
             2026-07-19T10:00:01.000Z err b\n\
             2026-07-19T10:00:02.000Z err c\n",
        );
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);

        // No matches: no-op.
        tab.find_matches = vec![];
        tab.find_pos = None;
        tab.find_next();
        tab.find_prev();
        assert!(tab.find_pos.is_none());

        // With no search results, Up/Down moves the selected log line and
        // clamps at the document boundaries.
        tab.context_line = Some(1);
        tab.select_adjacent_line(false);
        assert_eq!(tab.context_line, Some(0));
        tab.select_adjacent_line(true);
        assert_eq!(tab.context_line, Some(1));
        tab.select_adjacent_line(true);
        assert_eq!(tab.context_line, Some(2));
        tab.select_adjacent_line(true);
        assert_eq!(tab.context_line, Some(2));

        // Filtered Log View navigation skips hidden lines.
        tab.visible_lines = Some(Arc::new(vec![0, 2]));
        tab.context_line = Some(0);
        tab.select_adjacent_line(true);
        assert_eq!(tab.context_line, Some(2));
        tab.select_adjacent_line(false);
        assert_eq!(tab.context_line, Some(0));

        // Two matches: wrap around.
        tab.find_matches = vec![0, 2];
        tab.find_pos = Some(0);
        tab.find_next();
        assert_eq!(tab.find_pos, Some(1));
        tab.find_next();
        assert_eq!(tab.find_pos, Some(0));
        tab.find_prev();
        assert_eq!(tab.find_pos, Some(1));
        tab.find_prev();
        assert_eq!(tab.find_pos, Some(0));

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn selected_lane_navigation_wraps_and_updates_log_selection() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z first\n\
             2026-07-19T10:00:01.000Z second\n\
             2026-07-19T10:00:02.000Z third\n\
             2026-07-19T10:00:03.000Z fourth\n",
        );
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.filters.push(Filter {
            text: "error".to_string(),
            color: Color32::RED,
        });
        tab.matches = Arc::new(vec![vec![1, 3]]);
        tab.find_matches = vec![0, 2];
        tab.find_pos = Some(0);

        tab.select_lane(0);
        assert_eq!(tab.selected_lane, Some(0));
        assert!(tab
            .pending_toast
            .as_deref()
            .is_some_and(|message| message.contains("Left Arrow/Right Arrow")));

        tab.select_lane_next();
        assert_eq!(tab.context_line, Some(1));
        assert_eq!(tab.selected_diamond, Some((0, 1)));
        assert_eq!(tab.find_pos, Some(0));
        tab.select_lane_next();
        assert_eq!(tab.context_line, Some(3));
        assert_eq!(tab.find_pos, Some(1));
        tab.select_lane_next();
        assert_eq!(tab.context_line, Some(1));
        tab.select_lane_previous();
        assert_eq!(tab.context_line, Some(3));
        assert_eq!(tab.pending_scroll, Some(3));

        // Search navigation moves the selected lane cursor to its nearest
        // filter occurrence as well.
        tab.find_pos = Some(0);
        tab.find_next();
        assert_eq!(tab.context_line, Some(2));
        assert_eq!(tab.selected_diamond, None);

        // A selected log line updates only the already selected lane; it never
        // changes the lane just because another filter matches the line.
        tab.matches = Arc::new(vec![vec![1, 3], vec![1, 4]]);
        tab.selected_lane = Some(1);
        tab.sync_timeline_selection_to_line(1);
        assert_eq!(tab.selected_diamond, Some((1, 1)));
        tab.sync_timeline_selection_to_line(3);
        assert_eq!(tab.selected_lane, Some(1));
        assert_eq!(tab.selected_diamond, None);
        tab.sync_timeline_selection_to_line(2);
        assert_eq!(tab.selected_lane, Some(1));
        assert_eq!(tab.selected_diamond, None);

        // An arbitrary timeline click keeps the clicked lane, even when the
        // line matches a different lane.
        tab.select_timeline_line(3, Some(1));
        assert_eq!(tab.context_line, Some(3));
        assert_eq!(tab.selected_lane, Some(1));
        assert_eq!(tab.selected_diamond, None);
        tab.select_timeline_line(4, Some(1));
        assert_eq!(tab.selected_diamond, Some((1, 4)));

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn set_keyword_highlight_builds_and_clears_automaton() {
        let path = write_temp("2026-07-19T10:00:00.000Z INFO hello\n");
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);

        tab.set_keyword_highlight(Some("hello".into()));
        assert_eq!(tab.keyword_highlight, Some("hello".into()));
        assert!(tab.keyword_automaton.is_some());

        tab.set_keyword_highlight(None);
        assert!(tab.keyword_highlight.is_none());
        assert!(tab.keyword_automaton.is_none());

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn start_find_polls_to_completion_and_respects_subset() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z err a\n\
             2026-07-19T10:00:01.000Z err b\n\
             2026-07-19T10:00:02.000Z err c\n\
             2026-07-19T10:00:03.000Z ok  d\n",
        );
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.visible_lines = Some(Arc::new(vec![0, 2, 3]));
        tab.viewport_range = Some((2, 3));

        tab.start_find("err".to_string());
        for _ in 0..200 {
            if !tab.poll_find() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(tab.find_matches, vec![0, 2]);
        assert_eq!(tab.find_pos, Some(1));
        assert_eq!(tab.context_line, Some(2));
        assert_eq!(
            tab.pending_toast.as_deref(),
            Some("Up/Down Arrow to show previous/Next search occurrence")
        );

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn start_find_respects_case_sensitive_setting() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z ERROR first\n\
             2026-07-19T10:00:01.000Z error second\n",
        );
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);

        tab.find_case_sensitive = true;
        tab.start_find("error".to_string());
        for _ in 0..200 {
            if !tab.poll_find() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(tab.find_matches, vec![1]);
        assert!(tab.find_automaton.as_ref().unwrap().is_match("error"));
        assert!(!tab.find_automaton.as_ref().unwrap().is_match("ERROR"));

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn keyword_highlight_is_case_insensitive() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z INFO Error starting\\n\
             2026-07-19T10:00:01.000Z WARN error retry\\n\
             2026-07-19T10:00:02.000Z INFO ERROR final\\n",
        );
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);

        // Double-clicking a word must paint every case variant of it across the
        // view, matching the case-insensitive find box (regression: keyword
        // automaton used to be case-sensitive, highlighting only the exact-case
        // occurrence).
        tab.set_keyword_highlight(Some("Error".to_string()));
        let ac = tab
            .keyword_automaton
            .expect("keyword automaton must be built");
        assert!(ac.find_iter("Error starting").next().is_some());
        assert!(ac.find_iter("error retry").next().is_some());
        assert!(ac.find_iter("ERROR final").next().is_some());
        assert_eq!(ac.find_iter("nothing here").next(), None);

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn embedded_scan_uses_hidden_physical_continuation_lines() {
        let path = write_temp("INFO payload={\"value\":\n1}\nINFO unrelated\n");
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.visible_lines = Some(Arc::new(vec![0, 2]));
        tab.viewport_range = Some((0, 2));

        for _ in 0..200 {
            let _ = tab.schedule_embedded_scan();
            if !tab.poll_embedded_data() && !tab.embedded_detections.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(tab.embedded_detections.len(), 1);
        assert_eq!(tab.embedded_detections[0].span.start.line, 0);
        assert_eq!(tab.embedded_detections[0].span.end.line, 1);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn embedded_scan_recovers_record_start_far_above_viewport() {
        let mut content = String::new();
        for second in 0..30 {
            content.push_str(&format!(
                "2026-08-23T10:00:{second:02}Z INFO warmup record\n"
            ));
        }
        content.push_str("2026-08-23T10:01:00Z INFO payload={\n");
        for index in 0..80 {
            content.push_str(&format!("\"key_{index}\": {index},\n"));
        }
        content.push_str("\"complete\": true\n}\n");
        content.push_str("2026-08-23T10:01:01Z INFO next record\n");
        let path = write_temp(&content);
        let doc = LogDocument::open(&path).unwrap();
        let middle = 90;
        assert_eq!(doc.record_range_containing(middle), Some(30..113));

        let mut tab = LogTab::new(doc);
        tab.viewport_range = Some((middle, middle));
        for _ in 0..200 {
            let _ = tab.schedule_embedded_scan();
            if !tab.poll_embedded_data() && !tab.embedded_detections.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(tab.embedded_detections.len(), 1);
        assert_eq!(tab.embedded_detections[0].span.start.line, 30);
        assert_eq!(tab.embedded_detections[0].span.end.line, 112);
        std::fs::remove_file(path).ok();
    }
}

/// Apply a saved filter to a given tab.
pub fn apply_filter_to_tab(tab: &mut LogTab, filter_name: &str, theme: &Theme) {
    let filter_path = Settings::filters_dir().join(format!("{filter_name}.json"));
    if !filter_path.exists() {
        error!("filter not found: {}", filter_path.display());
        return;
    }
    match std::fs::read_to_string(&filter_path) {
        Ok(text) => match serde_json::from_str::<SavedFilter>(&text) {
            Ok(filter) => {
                tab.filters.clear();
                for (i, filter_text) in filter.filters.iter().take(MAX_FILTERS).enumerate() {
                    tab.filters.push(Filter {
                        text: filter_text.clone(),
                        color: theme.filter_colors[i % theme.filter_colors.len()],
                    });
                }
                tab.applied_filter = Some(filter_name.to_string());
                tab.rescan_filters();
                info!(
                    "applied filter '{filter_name}' to tab '{}'",
                    tab.doc.file_name
                );
            }
            Err(e) => error!("failed to parse filter '{filter_name}': {e}"),
        },
        Err(e) => error!("failed to read filter '{filter_name}': {e}"),
    }
}
