use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant, UNIX_EPOCH};

use aho_corasick::AhoCorasick;
use crossbeam_channel::Receiver;
use eframe::egui;
use egui::Color32;
use egui_dock::DockState;
use log::{error, info};
use serde::{Deserialize, Serialize};

use crate::ui::template_view::TemplateBrowserState;
use crate::ui::{icons, theme::Theme};
use logotomy::core::document::{FileChange, LoadProgress, LoadStage, LogDocument, ParsingConfig};
use logotomy::core::embedded_data::{AnalysisLimits, Detection, EmbeddedDataEngine, SourceSpan};
use logotomy::core::saved_filter::SavedFilter;
use logotomy::core::search;
use logotomy::core::search::FilterJoin;
use logotomy::core::settings::{LogLineDisplayMode, RecentFilter, Settings};
use logotomy::core::sidecar::{
    self, InvestigationState, LineAnchor, LineRange, LoadedState, MatchStatus, PinState,
    ScrollState, SearchState, SourceIdentity, TimelineZoomState,
};
use logotomy::core::time::{CustomDateFormat, CustomTimeFormat};
use logotomy::core::timeline::{Timeline, DEFAULT_BUCKETS};
use logotomy::mcp::PinAnalysis;

#[path = "tab_model.rs"]
mod tab_model;

/// Actions triggered from the right-click context menu on timeline or log view.
#[derive(Clone, Copy, Debug)]
pub enum TrimAction {
    TrimRight(usize),
    TrimLeft(usize),
}

/// A saved Pin-tab analysis entry. It may anchor a note to log lines, or be a
/// text-only user/agent analysis (`line_numbers` empty) shown first in the tab.
#[derive(Clone, Debug)]
pub struct PinEntry {
    pub start_line: usize,
    pub line_numbers: Vec<usize>,
    pub start_ts: i64,
    pub end_ts: i64,
    pub comment: String,
    pub unanchored: bool,
}

impl PinEntry {
    /// Build an anchored pin from its real selected rows. Pin endpoints always
    /// refer to the first and last selected rows, rather than the bounds of a
    /// drag range that may include hidden rows.
    pub fn anchored(
        doc: &LogDocument,
        mut line_numbers: Vec<usize>,
        comment: String,
    ) -> Option<Self> {
        line_numbers.retain(|&line| line < doc.total_lines());
        line_numbers.sort_unstable();
        line_numbers.dedup();
        let start_line = *line_numbers.first()?;
        let end_line = *line_numbers.last()?;
        Some(Self {
            start_line,
            start_ts: doc.ts_at_opt(start_line).unwrap_or(-1),
            end_ts: doc.ts_at_opt(end_line).unwrap_or(-1),
            line_numbers,
            comment,
            unanchored: false,
        })
    }

    /// Current first/last selected rows, excluding anchors outside the active
    /// trim window. These are the only rows a timeline marker can represent.
    pub fn visible_bounds(&self, total_lines: usize) -> Option<(usize, usize)> {
        if self.unanchored {
            return None;
        }
        let mut lines = self
            .line_numbers
            .iter()
            .copied()
            .filter(|&line| line < total_lines);
        let first = lines.next()?;
        let last = lines.last().unwrap_or(first);
        Some((first, last))
    }
}

/// The two transient surfaces shown while creating a line/range analysis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AnalysisPopupMode {
    Actions,
    Editor,
}

/// Where an analysis popup was opened from. Selection-originated popups own
/// the transient range highlight; context-menu popups only own their anchor.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AnalysisPopupState {
    pub(crate) range: (usize, usize),
    /// The current arrow target. Action bubbles use the release cursor while
    /// the editor uses the full selected-line bounds.
    pub(crate) anchor_rect: egui::Rect,
    /// The full source range geometry, retained when the action bubble
    /// switches into the editor.
    pub(crate) selected_rect: egui::Rect,
    pub(crate) mode: AnalysisPopupMode,
    pub(crate) from_selection: bool,
    pub(crate) id: u64,
    pub(crate) ignore_outside_click: bool,
}

fn pin_analyses(pins: &[PinEntry]) -> Vec<PinAnalysis> {
    pins.iter()
        .map(|pin| PinAnalysis {
            text: pin.comment.clone(),
            lines: pin.line_numbers.clone(),
        })
        .collect()
}

fn pins_from_analyses(doc: &LogDocument, analyses: Vec<PinAnalysis>) -> Vec<PinEntry> {
    analyses
        .into_iter()
        .map(|analysis| {
            PinEntry::anchored(doc, analysis.lines, analysis.text.clone()).unwrap_or(PinEntry {
                start_line: 0,
                line_numbers: Vec::new(),
                start_ts: -1,
                end_ts: -1,
                comment: analysis.text,
                unanchored: true,
            })
        })
        .collect()
}

/// Maximum number of filters supported in the timeline (excluding
/// the "Everything Else" lane). The filter input is disabled at this cap.
pub const MAX_FILTERS: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ViewTab {
    Timeline,
    Log,
    Pinned,
    Templates,
}

#[derive(Clone)]
pub struct Filter {
    pub text: String,
    pub color: Color32,
}

/// One in-memory, per-tab deletion which can be restored with Cmd/Ctrl+Z.
/// It is deliberately not persisted: reopening a file should not resurrect a
/// deletion the user already allowed the sidecar to save.
#[derive(Clone)]
pub enum UndoDelete {
    Filter {
        index: usize,
        filter: Filter,
        active: bool,
        case_sensitive: bool,
        exclude: bool,
        regex: bool,
        template_id: Option<u32>,
    },
    Pin {
        index: usize,
        pin: PinEntry,
    },
    Pins {
        pins: Vec<PinEntry>,
        bottom_panel_open: bool,
    },
    Trim {
        start: usize,
        end_exclusive: usize,
    },
}

/// Complete output of the background filter worker. Building timeline density
/// and filter-point indexes walks the document, so it belongs beside the
/// already-background Aho-Corasick scan rather than on the next UI frame.
pub(crate) struct FilterScanResult {
    matches: Arc<Vec<Arc<Vec<u32>>>>,
    timeline: Timeline,
    specs: Arc<Vec<search::FilterSpec>>,
    scope: Option<(i64, i64)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FilterDocumentKey {
    path: PathBuf,
    file_size: u64,
    file_mtime: std::time::SystemTime,
    trim_start: usize,
    trim_end: usize,
    total_lines: usize,
}

impl FilterDocumentKey {
    fn of(doc: &LogDocument) -> Self {
        Self {
            path: doc.path.clone(),
            file_size: doc.file_size,
            file_mtime: doc.file_mtime,
            trim_start: doc.trim_start,
            trim_end: doc.trim_end,
            total_lines: doc.total_lines(),
        }
    }
}

/// Completed filtered-line index, built without holding up the UI after a
/// lane visibility change.
pub(crate) struct VisibleLinesResult {
    visible_lines: Option<Arc<Vec<u32>>>,
    preserve_anchor: Option<usize>,
}

/// Estimated cumulative visual heights for the Wrap Log View mode. The cache
/// is rebuilt only when its width, font, or visible-line identity changes,
/// allowing wrapped logs to remain virtualized while scrolling.
pub(crate) struct WrapLayout {
    pub content_width: f32,
    pub font_size: f32,
    pub visible_len: usize,
    pub visible_identity: usize,
    pub first_line: Option<usize>,
    pub last_line: Option<usize>,
    pub offsets: Arc<Vec<f32>>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum AnnotationHoverKey {
    Embedded {
        detector_id: &'static str,
        span: SourceSpan,
    },
    Timestamp {
        span: SourceSpan,
    },
}

impl AnnotationHoverKey {
    pub(crate) fn for_detection(detection: &Detection) -> Self {
        Self::Embedded {
            detector_id: detection.detector_id,
            span: detection.span,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AnnotationHoverState {
    pub key: AnnotationHoverKey,
    pub source_rect: egui::Rect,
    pub started_at: Instant,
    pub last_seen_at: Instant,
    pub bubble_rect: Option<egui::Rect>,
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
    /// Advanced options parallel to `filters`. Kept separate from the visual
    /// filter so old in-memory and MCP callers retain their simple text shape.
    pub filter_case_sensitive: Vec<bool>,
    pub filter_exclude: Vec<bool>,
    pub filter_regex: Vec<bool>,
    /// Optional Drain template ID matcher for each visual filter lane.
    /// `None` retains ordinary text/regex matching.
    pub filter_template_ids: Vec<Option<u32>>,
    pub filter_join: FilterJoin,
    /// Most-recent-first entries offered when the Add Filter field is focused
    /// and empty. Entries retain their matching mode so choosing one can run
    /// immediately.
    pub filter_history: Vec<RecentFilter>,
    /// Restrict new filter scans to the current timeline zoom (the document
    /// trim is always respected by `LogDocument`).
    pub filter_current_range: bool,
    /// Per-filter sorted matching line indices.
    pub matches: Arc<Vec<Arc<Vec<u32>>>>,
    /// Filter specifications and scope represented by `matches`.
    matched_filter_specs: Arc<Vec<search::FilterSpec>>,
    matched_filter_scope: Option<(i64, i64)>,
    matched_filter_doc: Weak<LogDocument>,
    matched_filter_doc_key: FilterDocumentKey,
    pub timeline: Timeline,
    /// Line shown in the bottom context panel.
    pub context_line: Option<usize>,
    /// One-shot scroll request for the central log view.
    pub pending_scroll: Option<usize>,
    /// Cached, per-tab state for the docked Templates browser.
    pub template_browser: TemplateBrowserState,
    /// Zoom window on the timeline: (start_x, end_x) in epoch ms or line index.
    /// None = auto (full range).
    pub timeline_zoom: Option<(i64, i64)>,
    /// Pointer x-coordinate where the active Shift-drag timeline brush began.
    /// Stored explicitly because egui's current interact position is not the
    /// drag origin once the pointer moves.
    pub timeline_brush_start: Option<f32>,
    /// The filter lane currently selected for left/right occurrence navigation.
    pub selected_lane: Option<usize>,
    /// Pin card selected for keyboard deletion. This is UI-only state.
    pub selected_pin: Option<usize>,
    /// A card that the Pinned view should reveal after a timeline marker click.
    pub pending_pin_scroll: Option<usize>,
    /// Defers activating Pinned until the Log View has consumed its scroll
    /// request. This preserves the marker-click navigation order in dock
    /// layouts where Log and Pinned share a leaf.
    pub pending_pin_activation: bool,
    /// Latest reversible deletion in this tab.
    pub undo_delete: Option<UndoDelete>,
    /// One-shot UI message queued by a tab view and drained by the app toast.
    pub pending_toast: Option<String>,
    pub filter_input: String,
    /// Options applied to the text currently in the Add filter field.
    pub filter_input_case_sensitive: bool,
    pub filter_input_regex: bool,
    /// Selects Drain template IDs rather than text. The input accepts `42`, `T42`, or `T{42}`.
    pub filter_input_template_id: bool,
    /// Debounced validation state for the Add Filter regex or Template ID input.
    pub filter_input_regex_validate_at: Option<Instant>,
    pub filter_input_regex_error: Option<String>,
    pub filter_input_regex_error_dismissed: bool,
    /// Automaton used for cheap per-line highlight of visible rows.
    pub highlighter: Option<Arc<search::FilterHighlighter>>,
    /// In-flight background filter scan: result channel + cancel flag.
    pub search_rx: Option<(Receiver<FilterScanResult>, Arc<AtomicBool>)>,
    /// Shared worker counter shown by the filter strip while scanning.
    pub filter_scan_progress: Option<Arc<logotomy::core::search::ScanProgress>>,
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
    pub visible_lines: Option<Arc<Vec<u32>>>,
    /// Font size for log view and context panel (points).
    pub log_font_size: f32,
    /// Runtime copy of the global long-line preference; keeping it on the tab
    /// lets detached Log View windows render without borrowing the app shell.
    pub log_line_display_mode: LogLineDisplayMode,
    pub(crate) wrap_layout: Option<WrapLayout>,
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
    /// Active movable/resizable analysis bubble for a context-menu pin or a
    /// drag-selected range.
    pub(crate) analysis_popup: Option<AnalysisPopupState>,
    pub(crate) next_analysis_popup_id: u64,
    /// First and last *real* line index fully visible in the log viewport.
    pub viewport_range: Option<(usize, usize)>,
    /// Actual Log View viewport height from the most recent layout pass.
    /// This differs from the available panel height when a horizontal
    /// scrollbar consumes part of the viewport.
    pub(crate) log_viewport_height: Option<f32>,
    /// Logical scroll position captured from the virtualized log view.
    pub scroll_top_line: Option<usize>,
    pub scroll_fraction: f32,
    /// One-shot logical scroll position restored from a sidecar.
    pub pending_scroll_restore: Option<(usize, f32)>,
    /// Real line index to place at the top of the log viewport after a
    /// filter change (set by rebuild_visible_lines, consumed by log_view).
    pub preserve_anchor: Option<usize>,
    pub applied_filter: Option<String>,

    pub find_input: String,
    pub find_query: String,
    /// Whether Log View text/regex matches must use the exact ASCII letter case.
    pub find_case_sensitive: bool,
    /// Search using the shared Rust-regex matcher instead of a literal phrase.
    pub find_regex: bool,
    /// Search mined Drain template IDs. The input accepts `42`, `T42`, or `T{42}`.
    pub find_template_id_mode: bool,
    /// Parsed ID of the active Template ID search, if any.
    pub find_template_id: Option<u32>,
    /// Matcher used by the completed/in-flight search. It is separate from
    /// the input controls so changing the dropdown cannot reinterpret results
    /// before the user runs the revised query.
    pub find_active_spec: Option<search::FilterSpec>,
    /// Debounced validation state for Regex and Template ID search input.
    pub find_validate_at: Option<Instant>,
    pub find_error: Option<String>,
    pub find_error_dismissed: bool,
    pub find_matches: Vec<u32>,
    pub find_pos: Option<usize>,
    /// One-matcher highlighter shared with filter text/regex highlighting.
    pub find_highlighter: Option<Arc<search::FilterHighlighter>>,
    pub find_rx: Option<(Receiver<Vec<u32>>, Arc<AtomicBool>)>,
    /// Most-recent-first executed find queries, seeded from Settings.
    pub search_history: Vec<String>,
    /// A search that the app should persist to Settings after this frame.
    pub pending_recent_search: Option<String>,
    /// A filter that the app should persist to Settings after this frame.
    pub pending_recent_filter: Option<RecentFilter>,
    /// Popup lifetimes are independent from text-input focus so a click on a
    /// suggestion is delivered before the popup disappears.
    pub search_suggestions_open: bool,
    pub filter_suggestions_open: bool,
    /// Raw full-line inspector opened from a Log View row.
    pub full_line_inspector: Option<usize>,
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
    /// Delayed source-hover interaction for the active timestamp or payload.
    pub(crate) annotation_hover: Option<AnnotationHoverState>,

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
    /// Last occurrence-navigation text and the time it changed, driving the
    /// short transition when moving between occurrences.
    pub occurrence_navigation_animation: Option<(String, Instant)>,

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
    /// Sidecar state whose line anchors are waiting for user confirmation
    /// because the source file changed since the last saved snapshot.
    pub pending_sidecar_restore: Option<InvestigationState>,
    /// Last successfully persisted snapshot, used for dirty detection.
    pub last_sidecar_snapshot: Option<InvestigationState>,
}

pub struct FileLoader {
    pub path: PathBuf,
    pub name: String,
    pub rx: Receiver<LoadProgress>,
    pub cancel: Arc<AtomicBool>,
    pub stage: LoadStage,
    pub progress: f32,
    pub sidecar: Option<LoadedState>,
}

fn nearest_occurrence<I>(occurrences: I, line: usize) -> Option<(usize, usize)>
where
    I: Iterator<Item = u32>,
{
    occurrences
        .map(|occurrence| occurrence as usize)
        .enumerate()
        .min_by_key(|&(_, occurrence)| (occurrence.abs_diff(line), occurrence))
}

fn stable_line_hash(bytes: &[u8]) -> u64 {
    // FNV-1a is small, deterministic, and sufficient as a candidate key. A
    // hash match is only used after the line bounds and timestamp are checked.
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn resolve_anchor(tab: &LogTab, anchor: &LineAnchor) -> Option<usize> {
    let candidate = anchor.line.checked_sub(tab.doc.trim_start)?;
    if anchor_matches(tab, candidate, anchor) {
        return Some(candidate);
    }

    // A changed log may have gained or lost lines before the old anchor. Keep
    // remapping bounded so restoring a large investigation never scans the
    // entire document on the UI thread.
    const SEARCH_RADIUS: usize = 4096;
    for distance in 1..=SEARCH_RADIUS {
        if let Some(before) = candidate.checked_sub(distance) {
            if anchor_matches(tab, before, anchor) {
                return Some(before);
            }
        }
        if let Some(after) = candidate.checked_add(distance) {
            if anchor_matches(tab, after, anchor) {
                return Some(after);
            }
        }
    }
    None
}

fn anchor_matches(tab: &LogTab, line: usize, anchor: &LineAnchor) -> bool {
    if line >= tab.doc.total_lines() {
        return false;
    }
    if let Some(hash) = anchor.line_hash {
        if stable_line_hash(tab.doc.line_bytes(line)) != hash {
            return false;
        }
    }
    anchor
        .timestamp
        .is_none_or(|timestamp| tab.doc.ts_at_opt(line) == Some(timestamp))
}

fn restore_pin(tab: &LogTab, pin: &PinState) -> PinEntry {
    let lines: Vec<usize> = pin
        .line_numbers
        .iter()
        .filter_map(|anchor| resolve_anchor(tab, anchor))
        .collect();
    let start_line = pin
        .start_line
        .as_ref()
        .and_then(|anchor| resolve_anchor(tab, anchor))
        .or_else(|| lines.first().copied())
        .unwrap_or(0);
    let anchored =
        !pin.unanchored && !pin.line_numbers.is_empty() && lines.len() == pin.line_numbers.len();
    if anchored {
        // Recompute timestamps from the resolved rows. A changed source can
        // remap line anchors, so persisted timestamps are only a fallback for
        // text-only analyses, never the timeline position of an anchored pin.
        PinEntry::anchored(&tab.doc, lines, pin.comment.clone()).unwrap_or(PinEntry {
            start_line,
            line_numbers: Vec::new(),
            start_ts: pin.start_timestamp.unwrap_or(-1),
            end_ts: pin.end_timestamp.unwrap_or(-1),
            comment: pin.comment.clone(),
            unanchored: true,
        })
    } else {
        PinEntry {
            start_line,
            line_numbers: Vec::new(),
            start_ts: pin.start_timestamp.unwrap_or(-1),
            end_ts: pin.end_timestamp.unwrap_or(-1),
            comment: pin.comment.clone(),
            unanchored: true,
        }
    }
}

pub struct LogotomyApp {
    pub tabs: Vec<LogTab>,
    pub active: Option<usize>,
    /// Index into `loaders` of the loading file currently shown (as its own tab).
    /// When `Some`, `active` is `None` (a loading tab has no doc yet).
    pub active_loader: Option<usize>,
    pub loaders: Vec<FileLoader>,
    pub status: String,
    pub(super) zip_imports: super::zip_import::ZipImports,

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

    // Compact AI Assistant controls anchored from the application toolbar.
    pub show_ai_assistant_popup: bool,
    pub ai_assistant_button_rect: Option<egui::Rect>,

    // Recent files popup
    pub recent_show_dropdown: bool,
    pub recent_button_rect: Option<egui::Rect>,

    // Views menu (commands and templates)
    pub views_show_dropdown: bool,
    pub views_button_rect: Option<egui::Rect>,

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

    // Discoverability and navigation overlays.
    pub show_command_palette: bool,
    pub command_palette_query: String,
    pub show_cheat_sheet: bool,
    pub show_goto_popup: bool,
    pub goto_input: String,
    pub goto_error: Option<String>,
    /// A close was requested but the investigation sidecar could not be saved.
    pub close_save_error: Option<usize>,

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

    /// Tab waiting for changed-file anchor confirmation.
    pub pending_restore_tab: Option<usize>,
    pub sidecar_save_deadline: Option<Instant>,
    pub last_sidecar_autosave: Instant,
    pub requested_active_file: Option<PathBuf>,
}

const SIDECAR_AUTOSAVE_INTERVAL: Duration = Duration::from_secs(60);

fn probe_all_file_updates(tabs: &mut [LogTab]) -> Vec<(usize, String, Result<FileChange, String>)> {
    tabs.iter_mut()
        .enumerate()
        .map(|(index, tab)| {
            let file_name = tab.doc.file_name.clone();
            let change = tab.doc.file_change();
            if matches!(change, Ok(FileChange::Appended)) {
                tab.start_tail_update();
            }
            (index, file_name, change)
        })
        .collect()
}

fn sidecar_autosave_due(last_save: Instant, now: Instant) -> bool {
    now.duration_since(last_save) >= SIDECAR_AUTOSAVE_INTERVAL
}

/// Build the real-line index selected by timeline lanes. `None` is the compact
/// all-lines representation. Cancellation is checked during every large walk.
fn build_visible_lines(
    n: usize,
    matches: &[Arc<Vec<u32>>],
    lane_active: &[bool],
    filter_exclude: &[bool],
    join: FilterJoin,
    everything_else_active: bool,
    cancel: Option<&AtomicBool>,
) -> Option<Arc<Vec<u32>>> {
    if n == 0 {
        return None;
    }
    debug_assert!(matches.len() <= 64, "lane mask is limited to 64 filters");
    let mut include_mask = 0u64;
    let mut exclude_mask = 0u64;
    for lane in 0..matches.len().min(64) {
        if !lane_active.get(lane).copied().unwrap_or(true) {
            continue;
        }
        let bit = 1u64 << lane;
        if filter_exclude.get(lane).copied().unwrap_or(false) {
            exclude_mask |= bit;
        } else {
            include_mask |= bit;
        }
    }

    // Match vectors are sorted. Merge them by line so lane changes need only
    // one cursor per filter and the compact output itself—never a byte (or a
    // byte per lane) for every line in the document.
    let mut cursors = vec![0usize; matches.len()];
    let mut visible = Vec::new();
    let mut next_gap_line = 0usize;
    let mut processed = 0usize;
    loop {
        if processed % 16_384 == 0 && cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return None;
        }
        let next = matches
            .iter()
            .zip(&cursors)
            .filter_map(|(lane, &cursor)| lane.get(cursor).copied())
            .min();
        let Some(line) = next else { break };
        let line_usize = line as usize;
        if everything_else_active && next_gap_line < line_usize.min(n) {
            for gap_line in next_gap_line..line_usize.min(n) {
                if gap_line % 16_384 == 0 && cancel.is_some_and(|flag| flag.load(Ordering::Relaxed))
                {
                    return None;
                }
                visible.push(gap_line as u32);
            }
        }

        let mut event_mask = 0u64;
        for (lane_index, (lane, cursor)) in matches.iter().zip(&mut cursors).enumerate() {
            while lane.get(*cursor).copied() == Some(line) {
                if lane_index < 64 {
                    event_mask |= 1u64 << lane_index;
                }
                *cursor += 1;
                processed += 1;
            }
        }
        if line_usize < n {
            let include_match = include_mask != 0
                && if join == FilterJoin::All {
                    event_mask & include_mask == include_mask
                } else {
                    event_mask & include_mask != 0
                };
            let excluded = event_mask & exclude_mask != 0;
            if include_match && !excluded {
                visible.push(line);
            }
            next_gap_line = line_usize.saturating_add(1);
        }
    }
    if everything_else_active && next_gap_line < n {
        for gap_line in next_gap_line..n {
            if gap_line % 16_384 == 0 && cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                return None;
            }
            visible.push(gap_line as u32);
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
    /// Persist UI-originated query history without making Log View depend on
    /// the application shell. Tabs queue entries as users execute them.
    pub(crate) fn drain_recent_query_updates(&mut self) {
        let mut changed = false;
        for tab in &mut self.tabs {
            if let Some(query) = tab.pending_recent_search.take() {
                self.settings.add_recent_search(query);
                changed = true;
            }
            if let Some(filter) = tab.pending_recent_filter.take() {
                self.settings.add_recent_filter(filter);
                changed = true;
            }
        }
        if changed {
            self.settings.save();
        }
    }

    fn remember_open_path(&mut self, path: &PathBuf) {
        if !self.settings.open_files.iter().any(|saved| saved == path) {
            self.settings.open_files.push(path.clone());
        }
    }

    fn forget_open_path(&mut self, path: &PathBuf) {
        self.settings.open_files.retain(|saved| saved != path);
        if self.settings.active_file.as_ref() == Some(path) {
            self.settings.active_file = None;
        }
    }

    pub(crate) fn save_workspace(&mut self) {
        self.settings.open_files = self
            .tabs
            .iter()
            .map(|tab| tab.doc.path.clone())
            .chain(self.loaders.iter().map(|loader| loader.path.clone()))
            .collect();
        self.settings.active_file = self
            .active
            .and_then(|idx| self.tabs.get(idx))
            .map(|tab| tab.doc.path.clone())
            .or_else(|| {
                self.active_loader
                    .and_then(|idx| self.loaders.get(idx))
                    .map(|loader| loader.path.clone())
            });
        self.settings.save();
    }
}

impl LogTab {
    fn document_identity(tab: &LogTab) -> SourceIdentity {
        SourceIdentity {
            canonical_path: tab.doc.path.clone(),
            file_size: tab.doc.file_size,
            mtime_unix_nanos: tab
                .doc
                .file_mtime
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        }
    }

    fn line_anchor(tab: &LogTab, line: usize) -> LineAnchor {
        let original = tab.doc.trim_start.saturating_add(line);
        LineAnchor {
            line: original,
            timestamp: tab.doc.ts_at_opt(line),
            line_hash: (original < tab.doc.total_lines_untrimmed())
                .then(|| stable_line_hash(tab.doc.line_bytes_untrimmed(original))),
        }
    }

    fn pin_state(tab: &LogTab, pin: &PinEntry) -> PinState {
        let anchor = |line| {
            let original = if line > usize::MAX / 2 {
                usize::MAX - line
            } else {
                tab.doc.trim_start.saturating_add(line)
            };
            LineAnchor {
                line: original,
                timestamp: (original < tab.doc.total_lines_untrimmed())
                    .then(|| {
                        tab.doc
                            .ts_at_opt(original.saturating_sub(tab.doc.trim_start))
                    })
                    .flatten()
                    .filter(|timestamp| *timestamp >= 0),
                line_hash: (original < tab.doc.total_lines_untrimmed())
                    .then(|| stable_line_hash(tab.doc.line_bytes_untrimmed(original))),
            }
        };
        PinState {
            start_line: Some(anchor(pin.start_line)),
            end_line: pin.line_numbers.last().copied().map(anchor),
            line_numbers: pin
                .line_numbers
                .iter()
                .copied()
                .map(|line| Self::line_anchor(tab, line))
                .collect(),
            comment: pin.comment.clone(),
            start_timestamp: (pin.start_ts >= 0).then_some(pin.start_ts),
            end_timestamp: (pin.end_ts >= 0).then_some(pin.end_ts),
            unanchored: pin.unanchored,
        }
    }

    pub(crate) fn investigation_state(&self) -> InvestigationState {
        let filters = self
            .filters
            .iter()
            .enumerate()
            .map(|(idx, filter)| logotomy::core::sidecar::FilterState {
                text: filter.text.clone(),
                active: self.lane_active.get(idx).copied().unwrap_or(true),
                case_sensitive: self.filter_case_sensitive.get(idx).copied().unwrap_or(true),
                exclude: self.filter_exclude.get(idx).copied().unwrap_or(false),
                regex: self.filter_regex.get(idx).copied().unwrap_or(false),
                template_id: self.filter_template_ids.get(idx).copied().flatten(),
            })
            .collect();
        let timeline_zoom = self.timeline_zoom.map(|(start, end)| {
            let sequence = matches!(
                self.timeline.domain,
                logotomy::core::timeline::TimelineDomain::Sequence
            );
            TimelineZoomState {
                domain: if sequence { "sequence" } else { "time" }.to_string(),
                start: if sequence {
                    start.saturating_add(self.doc.trim_start as i64)
                } else {
                    start
                },
                end: if sequence {
                    end.saturating_add(self.doc.trim_start as i64)
                } else {
                    end
                },
            }
        });
        let selected_line = self.context_line.map(|line| Self::line_anchor(self, line));
        let search_selected = self
            .find_pos
            .and_then(|pos| self.find_matches.get(pos).copied())
            .map(|line| line as usize)
            .map(|line| Self::line_anchor(self, line));
        let scroll_top_line = self
            .scroll_top_line
            .map(|line| Self::line_anchor(self, line));

        InvestigationState {
            schema_version: sidecar::CURRENT_SCHEMA_VERSION,
            source: Self::document_identity(self),
            filters,
            everything_else_active: self.everything_else_active,
            selected_filter: self
                .selected_lane
                .and_then(|idx| self.filters.get(idx))
                .map(|filter| filter.text.clone()),
            applied_filter: self.applied_filter.clone(),
            search: SearchState {
                input: self.find_input.clone(),
                query: self.find_query.clone(),
                case_sensitive: self.find_case_sensitive,
                regex: self.find_regex,
                template_id_mode: self.find_template_id_mode,
                selected_line: search_selected,
            },
            selected_line,
            scroll: ScrollState {
                top_line: scroll_top_line,
                fraction: self.scroll_fraction.clamp(0.0, 1.0),
            },
            timeline_zoom,
            pins: self
                .pins
                .iter()
                .map(|pin| Self::pin_state(self, pin))
                .collect(),
            trim: self.doc.is_trimmed().then_some(LineRange {
                start: self.doc.trim_start,
                end_exclusive: self.doc.trim_end,
            }),
            // Kept in the sidecar format so older releases can still read the
            // file. Templates is now represented by the dock layout instead.
            show_templates: true,
            templates_panel_width: 320.0,
            bottom_panel_open: self.bottom_panel_open,
            log_font_size: self.log_font_size,
            dock_layout: serde_json::to_value(&self.dock_state).ok(),
            pre_detach_dock_layout: self
                .saved_dock_state
                .as_ref()
                .and_then(|dock| serde_json::to_value(dock).ok()),
            detached_views: self
                .detached_views
                .iter()
                .map(|view| format!("{view:?}"))
                .collect(),
            timeline_detached: self.timeline_detached,
        }
    }

    pub(crate) fn restore_sidecar(&mut self, loaded: LoadedState, theme: &Theme) {
        let state = loaded.state;
        self.apply_sidecar_safe(&state, theme);
        match loaded.status {
            MatchStatus::Exact => self.apply_sidecar_anchors(&state),
            MatchStatus::Changed => {
                self.pins = state
                    .pins
                    .iter()
                    .map(|pin| PinEntry {
                        start_line: 0,
                        line_numbers: Vec::new(),
                        start_ts: pin.start_timestamp.unwrap_or(-1),
                        end_ts: pin.end_timestamp.unwrap_or(-1),
                        comment: pin.comment.clone(),
                        unanchored: true,
                    })
                    .collect();
                self.pending_sidecar_restore = Some(state);
            }
        }
    }

    fn apply_sidecar_safe(&mut self, state: &InvestigationState, theme: &Theme) {
        self.filters = state
            .filters
            .iter()
            .take(MAX_FILTERS)
            .enumerate()
            .map(|(idx, filter)| Filter {
                text: filter.text.clone(),
                color: theme.filter_colors[idx % theme.filter_colors.len()],
            })
            .collect();
        self.lane_active = state
            .filters
            .iter()
            .take(MAX_FILTERS)
            .map(|filter| filter.active)
            .collect();
        self.filter_case_sensitive = state
            .filters
            .iter()
            .take(MAX_FILTERS)
            .map(|filter| filter.case_sensitive)
            .collect();
        self.filter_exclude = state
            .filters
            .iter()
            .take(MAX_FILTERS)
            .map(|filter| filter.exclude)
            .collect();
        self.filter_regex = state
            .filters
            .iter()
            .take(MAX_FILTERS)
            .map(|filter| filter.regex)
            .collect();
        self.filter_template_ids = state
            .filters
            .iter()
            .take(MAX_FILTERS)
            .map(|filter| filter.template_id)
            .collect();
        self.everything_else_active = state.everything_else_active;
        self.selected_lane = state.selected_filter.as_ref().and_then(|selected| {
            self.filters
                .iter()
                .position(|filter| &filter.text == selected)
        });
        self.applied_filter = state.applied_filter.clone();
        self.find_case_sensitive = state.search.case_sensitive;
        self.find_regex = state.search.regex;
        self.find_template_id_mode = state.search.template_id_mode;
        self.find_template_id = None;
        self.find_active_spec = None;
        self.find_input = state.search.input.clone();
        self.find_query = state.search.query.clone();
        self.find_matches.clear();
        self.find_pos = None;
        self.bottom_panel_open = state.bottom_panel_open;
        self.log_font_size = state.log_font_size.clamp(8.0, 24.0);

        if let Some(layout) = &state.dock_layout {
            if let Ok(dock) = serde_json::from_value(layout.clone()) {
                self.dock_state = dock;
            }
        }
        // Old sidecars predate the Templates dock tab. Keep their saved layout
        // but add the new always-available view instead of silently hiding it.
        if self.dock_state.find_tab(&ViewTab::Templates).is_none() {
            self.dock_state
                .main_surface_mut()
                .push_to_focused_leaf(ViewTab::Templates);
        }
        self.saved_dock_state = state
            .pre_detach_dock_layout
            .as_ref()
            .and_then(|layout| serde_json::from_value(layout.clone()).ok());
        self.detached_views = state
            .detached_views
            .iter()
            .filter_map(|view| match view.as_str() {
                "Log" => Some(ViewTab::Log),
                "Pinned" => Some(ViewTab::Pinned),
                "Templates" => Some(ViewTab::Templates),
                "Timeline" => Some(ViewTab::Timeline),
                _ => None,
            })
            .collect();
        self.timeline_detached = state.timeline_detached;
        if !self.find_query.is_empty() {
            self.start_find(self.find_query.clone());
        }
        self.rescan_filters();
    }

    pub(crate) fn confirm_sidecar_restore(&mut self, apply_anchors: bool) {
        let Some(state) = self.pending_sidecar_restore.take() else {
            return;
        };
        if apply_anchors {
            self.apply_sidecar_anchors(&state);
        }
        self.last_sidecar_snapshot = None;
    }

    fn apply_sidecar_anchors(&mut self, state: &InvestigationState) {
        if let Some(trim) = &state.trim {
            let total = self.doc.total_lines_untrimmed();
            if trim.start < trim.end_exclusive && trim.end_exclusive <= total {
                Arc::make_mut(&mut self.doc)
                    .trim_range(trim.start, trim.end_exclusive.saturating_sub(1));
            }
        }
        self.timeline_zoom = state.timeline_zoom.as_ref().and_then(|zoom| {
            let (mut start, mut end) = (zoom.start, zoom.end);
            if zoom.domain == "sequence" {
                start = start.saturating_sub(self.doc.trim_start as i64);
                end = end.saturating_sub(self.doc.trim_start as i64);
            }
            (start < end).then_some((start, end))
        });
        self.context_line = state
            .selected_line
            .as_ref()
            .and_then(|anchor| resolve_anchor(self, anchor));
        self.pending_scroll_restore = state.scroll.top_line.as_ref().and_then(|anchor| {
            resolve_anchor(self, anchor).map(|line| (line, state.scroll.fraction.clamp(0.0, 1.0)))
        });
        self.pins = state
            .pins
            .iter()
            .map(|pin| restore_pin(self, pin))
            .collect();
        if let Some(selected_line) = state.search.selected_line.as_ref() {
            if let Some(line) = resolve_anchor(self, selected_line) {
                self.context_line = Some(line);
            }
        }
        self.find_input = state.search.input.clone();
        self.find_query.clear();
        if !state.search.query.is_empty() {
            self.start_find(state.search.query.clone());
        }
        self.rescan_filters();
    }
}

impl LogotomyApp {
    pub(crate) fn save_tab_sidecar(&mut self, idx: usize) -> bool {
        let Some(tab) = self.tabs.get(idx) else {
            return true;
        };
        if tab.pending_sidecar_restore.is_some() || tab.stale {
            return true;
        }
        let path = tab.doc.path.clone();
        let state = tab.investigation_state();
        match sidecar::save_for(&path, &state) {
            Ok(()) => {
                if let Some(tab) = self.tabs.get_mut(idx) {
                    tab.last_sidecar_snapshot = Some(state);
                }
                true
            }
            Err(error) => {
                error!(
                    "failed to save investigation sidecar for {}: {error}",
                    path.display()
                );
                false
            }
        }
    }

    pub(crate) fn poll_sidecar_saves(&mut self) {
        let dirty = self.tabs.iter().any(|tab| {
            tab.pending_sidecar_restore.is_none()
                && tab
                    .last_sidecar_snapshot
                    .as_ref()
                    .is_none_or(|saved| saved != &tab.investigation_state())
        });
        if dirty {
            self.sidecar_save_deadline
                .get_or_insert_with(|| Instant::now() + Duration::from_millis(400));
        }
        let periodic_save_due = sidecar_autosave_due(self.last_sidecar_autosave, Instant::now());
        if periodic_save_due
            || self
                .sidecar_save_deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
        {
            for idx in 0..self.tabs.len() {
                let _ = self.save_tab_sidecar(idx);
            }
            self.sidecar_save_deadline = None;
            if periodic_save_due {
                self.last_sidecar_autosave = Instant::now();
            }
        }
    }

    pub(crate) fn flush_sidecars(&mut self) {
        for idx in 0..self.tabs.len() {
            let _ = self.save_tab_sidecar(idx);
        }
        self.sidecar_save_deadline = None;
        self.last_sidecar_autosave = Instant::now();
    }

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
        let restore_workspace = initial_paths.is_empty();
        let requested_active_file = restore_workspace
            .then(|| settings.active_file.clone())
            .flatten();
        // The optional override is used by the tracked-screenshot workflow;
        // it never mutates the user's persisted theme preference.
        let dark_mode = match std::env::var("LOGOTOMY_SCREENSHOT_THEME").as_deref() {
            Ok("dark") => true,
            Ok("light") => false,
            _ => settings.dark_mode,
        };

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
            status: "Drop a .log, .txt, or .zip file anywhere.".to_string(),
            zip_imports: Default::default(),
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
            show_ai_assistant_popup: false,
            ai_assistant_button_rect: None,
            recent_show_dropdown: false,
            recent_button_rect: None,
            views_show_dropdown: false,
            views_button_rect: None,
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
            show_command_palette: false,
            command_palette_query: String::new(),
            show_cheat_sheet: false,
            show_goto_popup: false,
            goto_input: String::new(),
            goto_error: None,
            close_save_error: None,
            custom_date_formats,
            show_custom_date_popup: false,
            cd_name: String::new(),
            cd_regex: String::new(),
            cd_sample: String::new(),
            last_file_check: Some(Instant::now()),
            open_requests,
            pending_restore_tab: None,
            sidecar_save_deadline: None,
            last_sidecar_autosave: Instant::now(),
            requested_active_file,
        };
        let paths = if initial_paths.is_empty() {
            app.settings.open_files.clone()
        } else {
            initial_paths
        };
        for path in paths {
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
        if super::archive::is_zip(&path) {
            self.zip_imports.enqueue(path);
            return;
        }
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
        crate::ui::worker_pool::spawn(move || {
            LogDocument::load_with_custom(
                &path_for_load,
                parsing,
                &custom_for_load,
                tx,
                cancel_worker,
            )
        });
        let loaded_sidecar = match sidecar::load_for(&path) {
            Ok(state) => state,
            Err(error) => {
                log::warn!(
                    "ignoring investigation sidecar for {}: {error}",
                    path.display()
                );
                None
            }
        };
        self.loaders.push(FileLoader {
            path: path.clone(),
            name,
            rx,
            cancel,
            stage: LoadStage::Indexing,
            progress: 0.0,
            sidecar: loaded_sidecar,
        });
        // The loading file is shown in its own (new) log tab, so focus it.
        self.active_loader = Some(self.loaders.len() - 1);
        self.active = None;
        // Track in recent files
        self.settings.add_recent_file(path);
        let open_path = self.loaders.last().unwrap().path.clone();
        self.remember_open_path(&open_path);
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
        // Probe first while documents are still shared. Every open tab tails,
        // not just the selected one; each tab already coalesces concurrent
        // appends through its single in-flight staging receiver.
        for (index, file_name, change) in probe_all_file_updates(&mut self.tabs) {
            match change {
                Ok(FileChange::Unchanged | FileChange::Appended) => {}
                Ok(FileChange::Shrunk | FileChange::Modified) => {
                    log::warn!("File {file_name} changed on disk and requires a full reload");
                    self.status = format!("'{file_name}' changed on disk — only tailing is supported. Close and reopen the file.");
                    self.tabs[index].stale = true;
                }
                Err(error) => {
                    log::warn!("failed to check {file_name} for updates: {error}");
                    self.status = format!("failed to check '{file_name}' for updates: {error}");
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
                    let loaded_sidecar = self.loaders[i].sidecar.take();
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
                    new_tab.log_line_display_mode = self.settings.log_line_display_mode;
                    new_tab.search_history = self.settings.recent_searches.clone();
                    new_tab.filter_history = self.settings.recent_filters.clone();
                    if let Some(loaded) = loaded_sidecar {
                        new_tab.restore_sidecar(loaded, &self.theme);
                    } else if let Some(filter_name) = self.settings.default_filter.clone() {
                        apply_filter_to_tab(&mut new_tab, &filter_name, &self.theme);
                    }
                    self.tabs.push(new_tab);
                    let new_idx = self.tabs.len() - 1;
                    if self.tabs[new_idx].pending_sidecar_restore.is_none() {
                        let _ = self.save_tab_sidecar(new_idx);
                    } else {
                        self.pending_restore_tab = Some(new_idx);
                    }

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
                    let should_activate = self
                        .requested_active_file
                        .as_ref()
                        .map_or(true, |path| path == &self.tabs[new_idx].doc.path);
                    if should_activate {
                        self.active = Some(new_idx);
                        self.active_loader = None;
                        self.settings.active_file = Some(self.tabs[new_idx].doc.path.clone());
                    }
                    self.save_workspace();
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
    /// Full matcher settings and composition are synced. Lane visibility is
    /// GUI-only and never changes MCP arithmetic.
    pub fn sync_mcp_filters(&mut self) {
        let Some(ref mcp_state) = self.mcp_state else {
            return;
        };
        for tab in &self.tabs {
            if !tab.mcp_serving {
                continue;
            }
            let specs = tab.filter_specs_snapshot();
            let mut guard = mcp_state.lock().unwrap();
            if guard.get_filter_specs("_active") != specs
                || guard.filter_join("_active") != tab.filter_join
            {
                guard.set_filter_specs("_active", specs, tab.filter_join);
                // GUI-originated change — the GUI already shows the newest
                // filter set, so don't trigger the MCP→GUI re-apply.
                guard.filters_dirty.store(false, Ordering::Relaxed);
                info!("MCP: synced GUI filters into server state");
            }
        }
    }

    /// Push GUI Pin-tab analysis changes (remove, clear, or user-edited pins)
    /// into the GUI MCP state. PinEntry remains the single source model.
    pub fn sync_mcp_analyses(&mut self) {
        let Some(ref mcp_state) = self.mcp_state else {
            return;
        };
        for tab in &self.tabs {
            if !tab.mcp_serving {
                continue;
            }
            let analyses = pin_analyses(&tab.pins);
            let mut guard = mcp_state.lock().unwrap();
            if guard.analyses != analyses {
                guard.set_analyses(analyses);
                // The GUI already owns the new value, so it must not pull the
                // exact same state back in on the next frame.
                guard.analyses_dirty.store(false, Ordering::Relaxed);
                info!("MCP: synced GUI Pin-tab analyses into server state");
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
        let (filters, join) = {
            let guard = mcp_state.lock().unwrap();
            guard.filters_dirty.store(false, Ordering::Relaxed);
            (
                guard.get_filter_specs("_active"),
                guard.filter_join("_active"),
            )
        };
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.mcp_serving) {
            tab.apply_mcp_filters(filters, join, &self.theme.filter_colors);
            info!("MCP: re-applied filters from server state");
        }
    }

    /// Consume the latest MCP search only on the tab whose document it searched.
    pub fn poll_mcp_search(&mut self) {
        if self
            .tabs
            .iter()
            .any(|tab| tab.mcp_serving && (tab.search_rx.is_some() || tab.visible_rx.is_some()))
        {
            return;
        }
        let request = self
            .mcp_state
            .as_ref()
            .and_then(|state| state.lock().unwrap().pending_search.take());
        if let Some(request) = request {
            if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.mcp_serving) {
                tab.apply_mcp_search(request);
            }
        }
    }

    /// Apply analysis cards added by an MCP client to the served Pin tab.
    pub fn poll_mcp_analyses(&mut self) {
        let Some(ref mcp_state) = self.mcp_state else {
            return;
        };
        let dirty = mcp_state
            .lock()
            .unwrap()
            .analyses_dirty
            .load(Ordering::Relaxed);
        if !dirty {
            return;
        }
        let analyses = {
            let guard = mcp_state.lock().unwrap();
            guard.analyses_dirty.store(false, Ordering::Relaxed);
            guard.analyses.clone()
        };
        if let Some(tab) = self.active.and_then(|i| self.tabs.get_mut(i)) {
            tab.pins = pins_from_analyses(&tab.doc, analyses);
            if !tab.pins.is_empty() {
                tab.bottom_panel_open = true;
            }
            info!("MCP: applied analyses to GUI Pin tab");
        }
    }

    /// Persist before closing without interrupting the user's workflow.
    pub fn request_close_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        if self.tabs[idx].pending_sidecar_restore.is_some() {
            // A changed source file leaves anchor restoration unresolved. On
            // close, keep the safe notes-only state and persist it silently.
            if let Some(tab) = self.tabs.get_mut(idx) {
                tab.confirm_sidecar_restore(false);
            }
        }
        self.close_after_save(idx);
    }

    fn close_after_save(&mut self, idx: usize) {
        if self.save_tab_sidecar(idx) {
            self.close_tab(idx);
        } else {
            self.close_save_error = Some(idx);
        }
    }

    /// Cycle the ready document tabs. Loading tabs intentionally retain their
    /// own focus behavior; Ctrl+Tab never interrupts a background load.
    pub fn cycle_tabs(&mut self, backwards: bool) {
        if self.tabs.len() < 2 {
            return;
        }
        let old = self.active;
        let current = self.active.unwrap_or(0);
        let len = self.tabs.len();
        let next = if backwards {
            (current + len - 1) % len
        } else {
            (current + 1) % len
        };
        self.active = Some(next);
        self.active_loader = None;
        self.on_tab_switched(old, next);
        self.save_workspace();
    }

    pub fn close_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        let closed_path = self.tabs[idx].doc.path.clone();
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
                        guard.set_filter_specs(
                            "_active",
                            self.tabs[new_idx].filter_specs_snapshot(),
                            self.tabs[new_idx].filter_join,
                        );
                        guard.filters_dirty.store(false, Ordering::Relaxed);
                        guard.set_analyses(pin_analyses(&self.tabs[new_idx].pins));
                        guard.analyses_dirty.store(false, Ordering::Relaxed);
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
        self.forget_open_path(&closed_path);
        self.save_workspace();
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
            "Logotomy GUI session\n\n\
Log: {log_path}\n\
Session ID: {session_id}\n\n\
Keep the session ID secret; do not save or repeat it.\n\n\
Call `attach_gui_session`, then confirm `session_info` reports `gui_attached`. The GUI supplies \
the log: do not call `load_log` or use `log_id`.\n\n\
Review `get_analysis` and `filters_get`, investigate the user's question, and add evidence-backed \
findings with `add_analysis`. Call `detach_gui_session` when done.\n\n\
If MCP is unavailable, ask the user to configure it in Logotomy → Settings → Integrate with AI Assistant."
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
        // Dynamic port (OS-assigned) + a fresh short random credential per session.
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
                guard.set_filter_specs(
                    "_active",
                    self.tabs[active_idx].filter_specs_snapshot(),
                    self.tabs[active_idx].filter_join,
                );
                guard.filters_dirty.store(false, Ordering::Relaxed);
                guard.set_analyses(pin_analyses(&self.tabs[active_idx].pins));
                guard.analyses_dirty.store(false, Ordering::Relaxed);
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
                guard.set_filter_specs(
                    "_active",
                    self.tabs[new_idx].filter_specs_snapshot(),
                    self.tabs[new_idx].filter_join,
                );
                guard.filters_dirty.store(false, Ordering::Relaxed);
                guard.set_analyses(pin_analyses(&self.tabs[new_idx].pins));
                guard.analyses_dirty.store(false, Ordering::Relaxed);
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
        let matches = vec![Arc::new(vec![1, 3, 6]), Arc::new(vec![3, 4])];

        let only_everything_else = build_visible_lines(
            8,
            &matches,
            &[false, false],
            &[false, false],
            FilterJoin::Any,
            true,
            None,
        )
        .unwrap();
        assert_eq!(&*only_everything_else, &[0, 2, 5, 7]);

        let first_lane_only = build_visible_lines(
            8,
            &matches,
            &[true, false],
            &[false, false],
            FilterJoin::Any,
            false,
            None,
        )
        .unwrap();
        assert_eq!(&*first_lane_only, &[1, 3, 6]);

        assert!(build_visible_lines(
            8,
            &matches,
            &[true, true],
            &[false, false],
            FilterJoin::Any,
            true,
            None
        )
        .is_none());
    }

    #[test]
    fn visible_line_builder_intersects_includes_and_subtracts_excludes() {
        let matches = vec![
            Arc::new(vec![0, 1]),
            Arc::new(vec![1, 2]),
            Arc::new(vec![2]),
        ];
        let all = build_visible_lines(
            4,
            &matches,
            &[true, true, false],
            &[false, false, true],
            FilterJoin::All,
            false,
            None,
        )
        .unwrap();
        assert_eq!(&*all, &[1]);
        let any = build_visible_lines(
            4,
            &matches,
            &[true, true, true],
            &[false, false, true],
            FilterJoin::Any,
            false,
            None,
        )
        .unwrap();
        assert_eq!(&*any, &[0, 1]);
    }

    #[test]
    fn sorted_lane_merge_matches_reference_for_all_toggle_combinations() {
        let matches = vec![
            Arc::new(vec![0, 2, 5, 9]),
            Arc::new(vec![1, 2, 7]),
            Arc::new(vec![2, 4, 8]),
        ];
        let excludes = [false, false, true];
        for active_bits in 0u8..8 {
            let active = [
                active_bits & 1 != 0,
                active_bits & 2 != 0,
                active_bits & 4 != 0,
            ];
            for join in [FilterJoin::Any, FilterJoin::All] {
                for everything_else in [false, true] {
                    let expected: Vec<u32> = (0u32..10)
                        .filter(|line| {
                            let lane_hits = matches
                                .iter()
                                .map(|lane| lane.binary_search(line).is_ok())
                                .collect::<Vec<_>>();
                            let includes = (0..3)
                                .filter(|&lane| active[lane] && !excludes[lane])
                                .collect::<Vec<_>>();
                            let include_match = !includes.is_empty()
                                && if join == FilterJoin::All {
                                    includes.iter().all(|&lane| lane_hits[lane])
                                } else {
                                    includes.iter().any(|&lane| lane_hits[lane])
                                };
                            let excluded = (0..3)
                                .any(|lane| active[lane] && excludes[lane] && lane_hits[lane]);
                            (include_match
                                || (everything_else && !lane_hits.iter().any(|&hit| hit)))
                                && !excluded
                        })
                        .collect();
                    let actual = build_visible_lines(
                        10,
                        &matches,
                        &active,
                        &excludes,
                        join,
                        everything_else,
                        None,
                    );
                    if expected.len() == 10 {
                        assert!(actual.is_none());
                    } else {
                        assert_eq!(actual.unwrap().as_slice(), expected.as_slice());
                    }
                }
            }
        }
    }

    #[test]
    fn periodic_sidecar_autosave_is_due_after_one_minute() {
        let last_save = Instant::now();
        assert!(!sidecar_autosave_due(
            last_save,
            last_save + Duration::from_secs(59)
        ));
        assert!(sidecar_autosave_due(
            last_save,
            last_save + Duration::from_secs(60)
        ));
    }

    #[test]
    fn gui_mcp_instruction_is_actionable_and_safe() {
        let prompt = LogotomyApp::build_mcp_instruction("123456", "/tmp/example.log");
        assert!(prompt.contains("attach_gui_session"));
        assert!(prompt.contains("123456"));
        assert!(prompt.contains("Log: /tmp/example.log"));
        assert!(prompt.contains("Session ID: 123456"));
        assert!(prompt.contains("Logotomy GUI session"));
        assert!(prompt.contains("gui_attached"));
        assert!(prompt.contains("do not call `load_log` or use `log_id`"));
        assert!(prompt.contains("get_analysis"));
        assert!(prompt.contains("filters_get"));
        assert!(prompt.contains("add_analysis"));
        assert!(prompt.contains("do not save or repeat it"));
        assert!(prompt.contains("session_info"));
        assert!(prompt.contains("Logotomy → Settings → Integrate with AI Assistant"));
        assert!(prompt.contains("detach_gui_session"));
        assert!(!prompt.contains("http://"));
        assert!(!prompt.contains("Authorization"));
        assert!(!prompt.contains("--mcp-gui"));
    }

    #[test]
    fn pin_analysis_conversion_preserves_empty_line_cards() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z INFO first\n2026-07-19T10:00:01.000Z ERROR second\n",
        );
        let doc = LogDocument::open(&path).unwrap();
        std::fs::remove_file(path).ok();
        let pins = pins_from_analyses(
            &doc,
            vec![
                PinAnalysis {
                    text: "Unanchored user context".into(),
                    lines: vec![],
                },
                PinAnalysis {
                    text: "Error evidence".into(),
                    lines: vec![1],
                },
            ],
        );
        assert!(pins[0].unanchored);
        assert_eq!(pins[0].start_ts, -1);
        assert!(!pins[1].unanchored);
        assert_eq!(pin_analyses(&pins)[1].lines, vec![1]);
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
    fn anchored_pin_uses_sorted_selected_rows_for_endpoints() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z zero\n\
             2026-07-19T10:00:01.000Z one\n\
             2026-07-19T10:00:02.000Z two\n\
             2026-07-19T10:00:03.000Z three\n",
        );
        let doc = LogDocument::open(&path).unwrap();
        let pin = PinEntry::anchored(&doc, vec![3, 1, 3], "note".into()).unwrap();

        assert_eq!(pin.line_numbers, vec![1, 3]);
        assert_eq!(pin.start_line, 1);
        assert_eq!(pin.start_ts, doc.ts_at(1));
        assert_eq!(pin.end_ts, doc.ts_at(3));
        assert_eq!(pin.visible_bounds(doc.total_lines()), Some((1, 3)));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn pin_navigation_reveals_hidden_start_and_queues_card_focus() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z zero\n\
             2026-07-19T10:00:01.000Z one\n\
             2026-07-19T10:00:02.000Z two\n",
        );
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.pins
            .push(PinEntry::anchored(&tab.doc, vec![1, 2], "evidence".into()).unwrap());
        tab.visible_lines = Some(Arc::new(vec![0, 2]));

        assert!(tab.navigate_to_pin(0));
        assert_eq!(tab.context_line, Some(1));
        assert_eq!(tab.pending_scroll, Some(1));
        assert_eq!(tab.selected_pin, Some(0));
        assert_eq!(tab.pending_pin_scroll, Some(0));
        assert!(tab.pending_pin_activation);
        assert_eq!(tab.visible_lines.as_deref().unwrap().as_slice(), &[0, 1, 2]);
        std::fs::remove_file(path).ok();
    }

    fn finish_filter_work(tab: &mut LogTab) {
        for _ in 0..200 {
            let scanning = tab.poll_search();
            let rebuilding = tab.poll_visible_lines();
            if !scanning && !rebuilding {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting for filter work");
    }

    #[test]
    fn filter_deletion_undo_restores_position_and_lane_state() {
        let path = write_temp("alpha\nbeta\n");
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.filters = vec![
            Filter {
                text: "alpha".into(),
                color: Color32::RED,
            },
            Filter {
                text: "beta".into(),
                color: Color32::BLUE,
            },
        ];
        tab.lane_active = vec![true, false];
        tab.selected_lane = Some(1);

        tab.remove_filter_with_undo(1);
        assert_eq!(tab.filters.len(), 1);
        assert_eq!(tab.selected_lane, None);
        assert!(tab.undo_delete());
        assert_eq!(
            tab.filters
                .iter()
                .map(|filter| filter.text.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "beta"]
        );
        assert_eq!(tab.lane_active, vec![true, false]);
        assert_eq!(tab.selected_lane, Some(1));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn template_filter_input_creates_a_typed_timeline_lane() {
        let path = write_temp("INFO connected user=1\nINFO connected user=2\nERROR disk full\n");
        let doc = LogDocument::open(&path).unwrap();
        let template_id = doc.template_at(0);
        let mut tab = LogTab::new(doc);

        assert!(tab
            .push_template_filter_input(&format!("T{{{template_id}}}"), Color32::RED)
            .is_some());
        assert_eq!(tab.filters[0].text, format!("T{{{template_id}}}"));
        assert_eq!(tab.filter_template_ids, vec![Some(template_id)]);

        for _ in 0..100 {
            if !tab.poll_search() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(&*tab.matches[0], &[0, 1]);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn pin_deletion_undo_restores_selected_card() {
        let path = write_temp("line\n");
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.pins.push(PinEntry {
            start_line: 0,
            line_numbers: vec![0],
            start_ts: -1,
            end_ts: -1,
            comment: "keep me".into(),
            unanchored: false,
        });
        tab.selected_pin = Some(0);

        tab.remove_selected_pin_with_undo();
        assert!(tab.pins.is_empty());
        assert!(tab.undo_delete());
        assert_eq!(tab.pins[0].comment, "keep me");
        assert_eq!(tab.selected_pin, Some(0));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn visible_boundary_obeys_the_filtered_view() {
        let path = write_temp("zero\none\ntwo\nthree\n");
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.visible_lines = Some(Arc::new(vec![1, 3]));
        tab.viewport_range = Some((1, 1));
        assert_eq!(tab.visible_boundary(false), Some(1));
        assert_eq!(tab.visible_boundary(true), Some(3));
        assert_eq!(tab.viewport_boundary(false), Some(1));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn investigation_state_uses_original_lines_and_restores_trimmed_view() {
        let path = write_temp(
            "2026-08-23T10:00:00Z zero\n2026-08-23T10:00:01Z one\n2026-08-23T10:00:02Z two\n",
        );
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        Arc::make_mut(&mut tab.doc).trim_range(1, 2);
        tab.context_line = Some(1);
        tab.scroll_top_line = Some(0);
        tab.scroll_fraction = 0.25;
        tab.pins.push(PinEntry {
            start_line: 1,
            line_numbers: vec![1],
            start_ts: tab.doc.ts_at(1),
            end_ts: tab.doc.ts_at(1),
            comment: "important".into(),
            unanchored: false,
        });
        let state = tab.investigation_state();
        assert_eq!(state.trim.as_ref().unwrap().start, 1);
        assert_eq!(state.selected_line.as_ref().unwrap().line, 2);
        assert_eq!(state.pins[0].line_numbers[0].line, 2);

        let doc = LogDocument::open(&path).unwrap();
        let mut restored = LogTab::new(doc);
        restored.restore_sidecar(
            LoadedState {
                state,
                status: MatchStatus::Exact,
            },
            &Theme::dark(),
        );
        assert_eq!(restored.doc.trim_start, 1);
        assert_eq!(restored.context_line, Some(1));
        assert_eq!(restored.pins.len(), 1);
        assert_eq!(restored.pins[0].line_numbers, vec![1]);
        assert!(!restored.pins[0].unanchored);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn changed_sidecar_restores_notes_before_anchors() {
        let path = write_temp("2026-08-23T10:00:00Z zero\n");
        let doc = LogDocument::open(&path).unwrap();
        let mut source_tab = LogTab::new(doc);
        source_tab.pins.push(PinEntry {
            start_line: 0,
            line_numbers: vec![0],
            start_ts: source_tab.doc.ts_at(0),
            end_ts: source_tab.doc.ts_at(0),
            comment: "keep this hypothesis".into(),
            unanchored: false,
        });
        let mut state = source_tab.investigation_state();
        state.source.file_size += 1;

        let doc = LogDocument::open(&path).unwrap();
        let mut restored = LogTab::new(doc);
        restored.restore_sidecar(
            LoadedState {
                state,
                status: MatchStatus::Changed,
            },
            &Theme::dark(),
        );
        assert_eq!(restored.pins[0].comment, "keep this hypothesis");
        assert!(restored.pins[0].unanchored);
        assert!(restored.pending_sidecar_restore.is_some());

        restored.confirm_sidecar_restore(false);
        assert!(restored.pending_sidecar_restore.is_none());
        assert!(restored.pins[0].unanchored);
        std::fs::remove_file(path).ok();
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
    fn filter_edits_retain_unchanged_match_lane_allocations() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z INFO alpha\n\
             2026-07-19T10:00:01.000Z WARN beta\n\
             2026-07-19T10:00:02.000Z INFO alpha gamma\n",
        );
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        assert_eq!(
            tab.push_filter("alpha", Theme::light().filter_colors[0]),
            Some(0)
        );
        for _ in 0..100 {
            if !tab.poll_search() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let alpha_matches = Arc::clone(&tab.matches[0]);

        assert_eq!(
            tab.push_filter("gamma", Theme::light().filter_colors[1]),
            Some(1)
        );
        for _ in 0..100 {
            if !tab.poll_search() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(Arc::ptr_eq(&alpha_matches, &tab.matches[0]));
        assert_eq!(tab.matches[1].as_slice(), &[2]);

        tab.filter_exclude[0] = true;
        tab.rescan_filters();
        for _ in 0..100 {
            if !tab.poll_search() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(Arc::ptr_eq(&alpha_matches, &tab.matches[0]));

        tab.remove_filter(1);
        for _ in 0..100 {
            if !tab.poll_search() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(Arc::ptr_eq(&alpha_matches, &tab.matches[0]));

        tab.handle_trim(TrimAction::TrimLeft(1));
        for _ in 0..100 {
            if !tab.poll_search() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!Arc::ptr_eq(&alpha_matches, &tab.matches[0]));
        assert_eq!(tab.matches[0].as_slice(), &[1]);
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
        tab.matches = Arc::new(vec![Arc::new(vec![0, 2]), Arc::new(vec![1, 2])]);
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
        tab.matches = Arc::new(vec![Arc::new(vec![1, 3, 8])]);
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
    fn background_visible_rebuild_keeps_selected_line_over_viewport_anchor() {
        let path = write_temp("l0\nl1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\n");
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.filters = vec![Filter {
            text: "match".into(),
            color: Theme::light().filter_colors[0],
        }];
        tab.matches = Arc::new(vec![Arc::new(vec![1, 3, 8])]);
        tab.lane_active = vec![true];
        tab.everything_else_active = false;
        tab.context_line = Some(8);
        tab.viewport_range = Some((2, 4));
        tab.rebuild_visible_lines_background();
        for _ in 0..100 {
            if !tab.poll_visible_lines() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(tab.context_line, Some(8));
        assert_eq!(tab.pending_scroll, Some(8));
        assert_eq!(tab.preserve_anchor, None);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn background_visible_rebuild_selects_nearest_line_when_selection_removed() {
        let path = write_temp("l0\nl1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\n");
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.filters = vec![Filter {
            text: "match".into(),
            color: Theme::light().filter_colors[0],
        }];
        tab.matches = Arc::new(vec![Arc::new(vec![1, 3, 8])]);
        tab.lane_active = vec![true];
        tab.everything_else_active = false;
        tab.context_line = Some(5);
        tab.viewport_range = Some((1, 3));
        tab.rebuild_visible_lines_background();
        for _ in 0..100 {
            if !tab.poll_visible_lines() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(tab.context_line, Some(3));
        assert_eq!(tab.pending_scroll, Some(3));
        assert_eq!(tab.preserve_anchor, None);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn filter_visibility_reconciles_selection_before_requesting_scroll() {
        let path = write_temp("other\nalpha\nselected\nbeta\nother\n");
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.push_filter("alpha", Color32::RED);
        tab.push_filter("beta", Color32::BLUE);
        finish_filter_work(&mut tab);

        // Adding filters does not move a selection that is still rendered by
        // Everything Else, and an already-visible selection must not scroll.
        tab.context_line = Some(2);
        tab.viewport_range = Some((1, 3));
        tab.pending_scroll = None;
        tab.set_everything_else_active(true);
        assert_eq!(tab.context_line, Some(2));
        assert_eq!(tab.pending_scroll, None);

        // Hiding Everything Else removes line 2. The closest visible lines
        // are 1 and 3, so the deterministic tie-break selects the earlier
        // line and only then asks the Log View to reveal that final line.
        tab.set_everything_else_active(false);
        finish_filter_work(&mut tab);
        assert_eq!(tab.context_line, Some(1));
        assert_eq!(tab.pending_scroll, Some(1));

        // Re-enabling a lane must not replace the current selection merely
        // because more rows become visible; nor should it scroll when that
        // selected line is already inside the viewport.
        tab.viewport_range = Some((0, 2));
        tab.pending_scroll = None;
        tab.set_everything_else_active(true);
        finish_filter_work(&mut tab);
        assert_eq!(tab.context_line, Some(1));
        assert_eq!(tab.pending_scroll, None);

        // The individual filter eye control uses the same reconciliation
        // path: with Everything Else hidden, disabling alpha filters out the
        // selected alpha row and selects beta before requesting a reveal.
        tab.set_everything_else_active(false);
        finish_filter_work(&mut tab);
        tab.viewport_range = Some((0, 2));
        tab.pending_scroll = None;
        assert!(tab.set_lane_active(0, false));
        finish_filter_work(&mut tab);
        assert_eq!(tab.context_line, Some(3));
        assert_eq!(tab.pending_scroll, Some(3));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn filter_add_remove_undo_and_clear_preserve_visible_selection() {
        let path = write_temp("other\nalpha\nother\nbeta\nother\nalpha\n");
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);

        tab.context_line = Some(1);
        tab.viewport_range = Some((0, 2));
        assert_eq!(tab.push_filter("alpha", Color32::RED), Some(0));
        finish_filter_work(&mut tab);
        assert_eq!(
            tab.context_line,
            Some(1),
            "adding must retain a visible line"
        );
        assert_eq!(tab.pending_scroll, None);

        // With Everything Else hidden, removing beta leaves alpha selected;
        // undo and clearing filters likewise expand visibility without moving
        // an already-visible real log line.
        assert_eq!(tab.push_filter("beta", Color32::BLUE), Some(1));
        finish_filter_work(&mut tab);
        tab.set_everything_else_active(false);
        finish_filter_work(&mut tab);
        tab.viewport_range = Some((0, 2));
        tab.pending_scroll = None;

        tab.remove_filter_with_undo(1);
        finish_filter_work(&mut tab);
        assert_eq!(tab.context_line, Some(1));
        assert_eq!(tab.pending_scroll, None);

        assert!(tab.undo_delete());
        finish_filter_work(&mut tab);
        assert_eq!(tab.context_line, Some(1));
        assert_eq!(tab.pending_scroll, None);

        tab.clear_all_filters();
        finish_filter_work(&mut tab);
        assert_eq!(tab.context_line, Some(1));
        assert_eq!(tab.pending_scroll, None);
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
    fn background_tail_update_extends_completed_filter_lanes() {
        use std::io::Write;

        let path = write_temp("alpha old\nbeta\n");
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.push_filter("alpha", Theme::light().filter_colors[0]);
        for _ in 0..100 {
            if !tab.poll_search() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(tab.matches[0].as_slice(), &[0]);

        std::fs::File::options()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"alpha new\ngamma\n")
            .unwrap();
        tab.start_tail_update();
        for _ in 0..100 {
            if tab.poll_tail_update().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        for _ in 0..100 {
            if !tab.poll_search() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(tab.doc.total_lines(), 4);
        assert_eq!(tab.matches[0].as_slice(), &[0, 2]);
        assert_eq!(tab.timeline.density.iter().sum::<u32>(), 4);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn file_update_probe_schedules_every_open_tab() {
        use std::io::Write;

        let first_path = write_temp("first\n");
        let second_path = write_temp("second\n");
        let mut tabs = vec![
            LogTab::new(LogDocument::open(&first_path).unwrap()),
            LogTab::new(LogDocument::open(&second_path).unwrap()),
        ];
        for path in [&first_path, &second_path] {
            std::fs::File::options()
                .append(true)
                .open(path)
                .unwrap()
                .write_all(b"appended\n")
                .unwrap();
        }

        let changes = probe_all_file_updates(&mut tabs);
        assert_eq!(changes.len(), 2);
        assert!(changes
            .iter()
            .all(|(_, _, change)| matches!(change, Ok(FileChange::Appended))));
        assert!(tabs.iter().all(|tab| tab.tail_rx.is_some()));

        for tab in &mut tabs {
            for _ in 0..100 {
                if tab.poll_tail_update().is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(tab.doc.total_lines(), 2);
        }
        std::fs::remove_file(first_path).ok();
        std::fs::remove_file(second_path).ok();
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
        tab.matches = Arc::new(vec![Arc::new(vec![1, 3])]);
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
        assert_eq!(tab.selected_occurrence(), Some((0, 1)));
        assert_eq!(tab.find_pos, Some(0));
        tab.select_lane_next();
        assert_eq!(tab.context_line, Some(3));
        assert_eq!(tab.find_pos, Some(1));
        tab.select_lane_next();
        assert_eq!(tab.context_line, Some(1));
        tab.select_lane_previous();
        assert_eq!(tab.context_line, Some(3));
        assert_eq!(tab.pending_scroll, Some(3));

        // Search navigation can land on a line outside the selected lane; the
        // derived occurrence then becomes empty without clearing the lane.
        tab.find_pos = Some(0);
        tab.find_next();
        assert_eq!(tab.context_line, Some(2));
        assert_eq!(tab.selected_occurrence(), None);

        // Occurrence state derives from the selected lane and current log
        // line; another lane matching the line never changes the selection.
        tab.matches = Arc::new(vec![Arc::new(vec![1, 3]), Arc::new(vec![1, 4])]);
        tab.lane_active = vec![true, true];
        tab.selected_lane = Some(1);
        tab.context_line = Some(1);
        assert_eq!(tab.selected_occurrence(), Some((1, 1)));
        tab.context_line = Some(3);
        assert_eq!(tab.selected_lane, Some(1));
        assert_eq!(tab.selected_occurrence(), None);
        tab.context_line = Some(2);
        assert_eq!(tab.selected_lane, Some(1));
        assert_eq!(tab.selected_occurrence(), None);

        // An arbitrary timeline click keeps the clicked lane, even when the
        // line matches a different lane.
        tab.select_timeline_line(3, Some(1));
        assert_eq!(tab.context_line, Some(3));
        assert_eq!(tab.selected_lane, Some(1));
        assert_eq!(tab.selected_occurrence(), None);
        tab.select_timeline_line(4, Some(1));
        assert_eq!(tab.selected_occurrence(), Some((1, 4)));

        // A disabled lane remains navigable, but clicking it must not select
        // the disabled filter for occurrence navigation.
        tab.lane_active[1] = false;
        tab.select_timeline_line(2, Some(1));
        assert_eq!(tab.context_line, Some(2));
        assert_eq!(tab.selected_lane, None);
        assert_eq!(tab.selected_occurrence(), None);

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
        let highlighter = tab.find_highlighter.as_ref().unwrap();
        assert!(!highlighter.spans("error").is_empty());
        assert!(highlighter.spans("ERROR").is_empty());

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn template_action_starts_a_typed_log_search() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z INFO connected user=1\n\
             2026-07-19T10:00:01.000Z INFO connected user=2\n\
             2026-07-19T10:00:02.000Z WARN disconnected\n",
        );
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        let template_id = tab.doc.template_at(0);

        tab.start_template_id_search(template_id);
        for _ in 0..200 {
            if !tab.poll_find() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        assert!(tab.find_template_id_mode);
        assert_eq!(tab.find_input, format!("T{{{template_id}}}"));
        assert_eq!(tab.find_template_id, Some(template_id));
        assert_eq!(tab.find_matches, vec![0, 1]);
        assert!(tab.search_focus_requested);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn mcp_filter_sync_preserves_matcher_settings_and_lane_identity() {
        let path = write_temp("ERROR api timeout\nerror api ok\nINFO api timeout\n");
        let mut tab = LogTab::new(LogDocument::open(&path).unwrap());
        let theme = Theme::dark();
        let mut regex = search::FilterSpec::phrase("time.*");
        regex.regex = true;
        regex.case_sensitive = false;
        let mut exclude = search::FilterSpec::phrase("INFO");
        exclude.polarity = search::FilterPolarity::Exclude;
        tab.apply_mcp_filters(
            vec![regex.clone(), exclude.clone()],
            FilterJoin::All,
            &theme.filter_colors,
        );
        tab.lane_active = vec![false, true];
        tab.apply_mcp_filters(vec![exclude.clone()], FilterJoin::Any, &theme.filter_colors);
        assert_eq!(tab.filter_specs_snapshot(), vec![exclude]);
        assert_eq!(tab.filter_join, FilterJoin::Any);
        assert_eq!(tab.lane_active, vec![true]);
        assert_eq!(tab.filter_regex, vec![false]);
        assert_eq!(tab.filter_exclude, vec![true]);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn mcp_search_installs_exact_scope_reveals_hits_and_rejects_stale_documents() {
        let path = write_temp("ERROR one\nerror two\nother three\n");
        let mut tab = LogTab::new(LogDocument::open(&path).unwrap());
        tab.visible_lines = Some(Arc::new(vec![2]));
        tab.start_find("other".into());
        let cancel = tab.find_rx.as_ref().unwrap().1.clone();
        let mut spec = search::FilterSpec::phrase("err.*");
        spec.regex = true;
        spec.case_sensitive = false;
        let old_doc = Arc::clone(&tab.doc);
        tab.apply_mcp_search(logotomy::mcp::GuiSearch {
            doc: Arc::clone(&tab.doc),
            spec: spec.clone(),
            matches: vec![1],
            first_page_line: Some(1),
        });
        assert!(cancel.load(Ordering::Relaxed));
        assert!(tab.find_rx.is_none());
        assert_eq!(tab.find_input, "err.*");
        assert!(tab.find_regex);
        assert!(!tab.find_case_sensitive);
        assert_eq!(tab.find_matches, vec![1]);
        assert_eq!(tab.visible_lines.as_ref().unwrap().as_slice(), &[1, 2]);
        assert_eq!(tab.context_line, Some(1));
        assert!(tab.search_focus_requested);
        Arc::make_mut(&mut tab.doc).trim_left(1);
        tab.apply_mcp_search(logotomy::mcp::GuiSearch {
            doc: old_doc,
            spec,
            matches: vec![0],
            first_page_line: Some(0),
        });
        assert_eq!(tab.find_matches, vec![1]);
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

    #[test]
    fn trim_preserves_pins_and_undo_restores_the_window() {
        let path = write_temp("one\ntwo\nthree\nfour\n");
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.pins.push(PinEntry {
            start_line: 0,
            line_numbers: vec![0, 3],
            start_ts: -1,
            end_ts: -1,
            comment: "keep this".into(),
            unanchored: false,
        });
        tab.find_input = "three".into();
        tab.find_query = "three".into();

        tab.handle_trim(TrimAction::TrimLeft(2));
        assert_eq!(tab.doc.trim_start, 2);
        assert_eq!(tab.pins.len(), 1);
        assert_eq!(tab.pins[0].line_numbers, vec![usize::MAX, 1]);
        assert_eq!(tab.find_query, "three");

        assert!(tab.undo_delete());
        assert_eq!(tab.doc.trim_start, 0);
        assert_eq!(tab.doc.trim_end, 4);
        assert_eq!(tab.pins[0].line_numbers, vec![0, 3]);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn clear_pins_is_undoable() {
        let path = write_temp("one\n");
        let doc = LogDocument::open(&path).unwrap();
        let mut tab = LogTab::new(doc);
        tab.pins.push(PinEntry {
            start_line: 0,
            line_numbers: vec![0],
            start_ts: -1,
            end_ts: -1,
            comment: String::new(),
            unanchored: false,
        });
        tab.clear_pins_with_undo();
        assert!(tab.pins.is_empty());
        assert!(tab.undo_delete());
        assert_eq!(tab.pins.len(), 1);
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
