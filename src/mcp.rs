//! logotomy MCP server — embedded mode (no separate binary needed).
//!
//! Run either via the CLI (`logotomy --mcp`) or in-process from the GUI.
//! Provides an MCP (Model Context Protocol) server over stdio, allowing AI
//! assistants to load, search, and query log files.
//!
//! Protocol:
//!   - stdio: newline-delimited JSON-RPC 2.0 over stdin/stdout
//!   - GUI attachment: authenticated private loopback IPC, never exposed as an
//!     MCP transport or placed in agent configuration
//!
//! The server has two modes:
//!   - **Headless** (default): load_log, list_logs, close_log (with `log_id`)
//!     plus the shared analysis tools. Used via `logotomy --mcp`.
//!   - **GUI** (via `set_active_doc`): the shared analysis tools plus `trim`,
//!     operating on a single active document without `log_id` or
//!     load/list/close. Used when the MCP server is started from within the GUI.
//!
//! Tools (headless):
//!   load_log(path)                          → log_id + stats
//!   list_logs()                             → loaded documents
//!   close_log(log_id)                       → drop a document
//!   filters_get(log_id)                     → [{id, filter_text}]
//!   filters_add(log_id, filter_text)        → updated [{id, filter_text}]
//!   filters_remove(log_id, id)              → updated [{id, filter_text}]
//!   find_occurrences(log_id, keyword, max_results?, offset?, case_sensitive?,
//!                    after?, before?, with_filtered_log?)
//!                                         → paginated [line, epoch_ms|null]
//!                                            tuples + pagination metadata
//!
//! Tools (GUI — single active doc, no log_id):
//!   filters_get() / filters_add(filter_text) / filters_remove(id)
//!   find_occurrences(keyword, max_results?, offset?, case_sensitive?, after?,
//!                    before?, with_filtered_log?)
//!   trim(start?, end?)                      → focus the visible window
//!   get_analysis() / add_analysis({text, lines?}) → Pin-tab analysis cards
//!
//! Analysis tools (both modes; headless takes log_id, GUI does not; each takes
//! an optional trailing `with_filtered_log?`, default true = run on the
//! filtered log):
//!   summarize_log(start?, end?)             → one-call orientation: stats, top
//!                                             templates, error-ish templates,
//!                                             biggest time gaps, densest minute,
//!                                             plus byte-size budget estimates
//!   get_timeline_histogram(start?, end?, buckets?, keyword?|template_id?)
//!                                         → tiny [{x, count}] distribution
//!   get_template_anomalies(start?, end?, limit?)
//!                                         → rare / first-seen-late / bursty
//!   get_template(ids?)                      → {id: {pattern, count, example_line}}
//!   get_template_samples(template_id, n?, strategy?)
//!                                         → a few concrete lines per template
//!   log_sequence(start?, end?, max_entries?, collapse?)
//!                                         → dense [[epoch_ms|null, line, template_id]]
//!                                           with a `truncated` flag when capped
//!   raw_log(start, end, max_lines?)         → raw lines by line number or time
//!
//! Design: stateless + self-describing — analysis tools take optional line/time
//! ranges (`start`/`end`), responses default to counts + anchors (line no,
//! timestamp, template_id), and large arrays carry size/truncated fields so the
//! agent can budget without separate pre-call size tools.
//!
//! Filters (`with_filtered_log`, default true): every analysis tool takes an
//! optional trailing `with_filtered_log` flag. When true (the default) its
//! results are restricted to the **filtered log** — the union of lines matching
//! the current filter keyword set (see filters_get / filters_add /
//! filters_remove). The GUI's "Everything Else" lane is never part of MCP
//! arithmetic. When `with_filtered_log` is true but no filter keyword is set,
//! the tool short-circuits with a `{"comment": "no log", "reason": …}` reply
//! instead of scanning the file — pass `with_filtered_log=false` for full-file
//! traversal or add a filter first.

use std::collections::HashMap;

use std::io::{BufRead, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

use crate::core::document::LogDocument;
use crate::core::search;
use crate::core::time::{format_ms, parse_time_param};

mod bridge;
pub mod session;

pub use bridge::run_gui_bridge;
use bridge::GuiClient;

const MODERN_PROTOCOL_VERSION: &str = "2026-07-28";
const PROTOCOL_VERSIONS: &[&str] = &[
    MODERN_PROTOCOL_VERSION,
    "2025-11-25",
    "2025-06-18",
    "2025-03-26",
    "2024-11-05",
];
const MAX_LINE_TEXT: usize = 1000;
const HARD_MAX_LINES: usize = 2000;
/// Cap on the per-log filter keyword set (mirrors the GUI's MAX_FILTERS).
const MAX_FILTERS: usize = 20;

/// A user-visible analysis card in the GUI's Pin tab. `lines` uses zero-based
/// document indices internally; MCP request/response payloads use one-based
/// line numbers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinAnalysis {
    pub text: String,
    pub lines: Vec<usize>,
}

// ---------------------------------------------------------------------------
// File logger — implements the `log::Log` trait to write to logotomy.log
// ---------------------------------------------------------------------------

use log::{Level, Metadata, Record};

struct FileLogger {
    path: String,
}

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Debug
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let ms = now.as_millis();
        let total_secs = ms / 1000;
        let h = (total_secs / 3600) % 24;
        let m = (total_secs / 60) % 60;
        let s = total_secs % 60;
        let millis = ms % 1000;
        let line = format!(
            "[{:02}:{:02}:{:02}.{:03}] {} {}\n",
            h,
            m,
            s,
            millis,
            record.level(),
            record.args()
        );
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .write(true)
            .open(&self.path)
            .map(|mut f| {
                let _ = f.write_all(line.as_bytes());
            });
    }

    fn flush(&self) {}
}

/// Initialise logging: stderr + file output.
/// Returns the log file path.
fn log_init() -> String {
    let dir = crate::core::settings::Settings::log_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("logotomy.log");
    let path_str = path.to_string_lossy().to_string();

    // Write header (append-only; never truncate).
    let header = format!(
        "=== logotomy-mcp started (PID {}) ===\n",
        std::process::id()
    );
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .write(true)
        .open(&path)
        .map(|mut f| {
            let _ = f.write_all(header.as_bytes());
        });

    // Store the log path for later retrieval.
    let _ = LOG_PATH.set(path_str.clone());

    // Build a custom multi-target logger that writes to both stderr and the file.
    let file_logger = FileLogger {
        path: path_str.clone(),
    };
    let stderr_logger =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
            .target(env_logger::Target::Stderr)
            .format_timestamp_millis()
            .build();

    struct MultiLogger {
        file: FileLogger,
        stderr: env_logger::Logger,
    }

    impl log::Log for MultiLogger {
        fn enabled(&self, metadata: &log::Metadata) -> bool {
            self.stderr.enabled(metadata)
        }

        fn log(&self, record: &log::Record) {
            self.file.log(record);
            self.stderr.log(record);
        }

        fn flush(&self) {
            self.file.flush();
            self.stderr.flush();
        }
    }

    let multi = MultiLogger {
        file: file_logger,
        stderr: stderr_logger,
    };

    // Set the multi logger (ignore the error if already set e.g. in tests).
    let _ = log::set_boxed_logger(Box::new(multi))
        .map(|()| log::set_max_level(log::LevelFilter::Debug));

    path_str
}

/// Helper: capture panic info and log it.
fn set_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic".to_string()
        };
        let location = info.location().map(|l| l.to_string()).unwrap_or_default();
        log::error!("PANIC at {location}: {payload}");
        prev(info);
    }));
}

/// Return the log file path (empty if not initialised).
pub fn log_path() -> String {
    LOG_PATH.get().cloned().unwrap_or_default()
}

static LOG_PATH: std::sync::OnceLock<String> = std::sync::OnceLock::new();

// ---------------------------------------------------------------------------
// Server state
// ---------------------------------------------------------------------------

/// Shared state between the MCP server and optionally the GUI.
pub struct ServerState {
    pub logs: HashMap<String, Arc<LogDocument>>,
    pub next_id: u64,
    /// (log_id, keyword, case_sensitive) → sorted matching line indices.
    pub match_cache: HashMap<(String, String, bool), Arc<Vec<usize>>>,
    /// Single active document for GUI mode. When set, the server operates in
    /// simplified mode (no load_log/list_logs/close_log, no log_id parameter).
    pub active_doc: Option<Arc<LogDocument>>,
    /// Selects the GUI tool surface even while no active document is attached.
    /// This keeps GUI servers and the stdio bridge from exposing headless
    /// load/list/close tools during brief disconnected periods.
    gui_mode: bool,
    /// Set to true when `active_doc` is replaced (MCP→GUI direction). The GUI
    /// polls this flag to keep the UI in sync with MCP-originated changes.
    /// GUI-originated mutations (trim/append) flow the other way: the GUI
    /// pushes the mutated Arc back via `set_active_doc` (see
    /// `LogotomyApp::sync_mcp_active_doc`) and clears this flag itself.
    pub active_doc_dirty: Arc<AtomicBool>,
    /// Per-log keyword filter list; keyed by `log_id` in headless mode and by
    /// `"_active"` in GUI mode. Drives the `with_filtered_log` filtered view
    /// (union of each filter's matching lines). The GUI's "Everything Else"
    /// lane is never part of MCP arithmetic — filtered tools only ever see
    /// lines that match at least one real filter keyword.
    pub filters: HashMap<String, Vec<String>>,
    /// Set to true when the `_active` filter set changed (MCP→GUI direction).
    /// The GUI polls this flag to re-apply MCP-originated filter edits to the
    /// served tab. GUI-originated filter edits flow the other way (see
    /// `LogotomyApp::sync_mcp_filters`) and clear this flag themselves.
    pub filters_dirty: Arc<AtomicBool>,
    /// Analysis cards for the GUI's currently served Pin tab. This is GUI-only
    /// state: standalone logs deliberately do not retain user annotations.
    pub analyses: Vec<PinAnalysis>,
    /// Set when MCP changes the GUI analysis cards. The GUI mirrors the same
    /// bidirectional synchronization pattern used for filters.
    pub analyses_dirty: Arc<AtomicBool>,
}

impl Default for ServerState {
    fn default() -> Self {
        Self {
            logs: HashMap::new(),
            next_id: 0,
            match_cache: HashMap::new(),
            active_doc: None,
            gui_mode: false,
            active_doc_dirty: Arc::new(AtomicBool::new(false)),
            filters: HashMap::new(),
            filters_dirty: Arc::new(AtomicBool::new(false)),
            analyses: Vec::new(),
            analyses_dirty: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl ServerState {
    pub fn get_doc(&self, log_id: &str) -> Result<Arc<LogDocument>, String> {
        self.logs
            .get(log_id)
            .cloned()
            .ok_or_else(|| format!("unknown log_id '{log_id}' (see list_logs)"))
    }

    pub fn matches_for(
        &mut self,
        log_id: &str,
        keyword: &str,
        case_sensitive: bool,
    ) -> Result<Arc<Vec<usize>>, String> {
        let key = (log_id.to_string(), keyword.to_string(), case_sensitive);
        if let Some(m) = self.match_cache.get(&key) {
            return Ok(Arc::clone(m));
        }
        let doc = if log_id == "_active" {
            self.get_active_doc()?
        } else {
            self.get_doc(log_id)?
        };
        let matches = search::find_lines(
            doc.as_ref(),
            None,
            keyword,
            !case_sensitive,
            &AtomicBool::new(false),
        );
        let matches = Arc::new(matches);
        self.match_cache.insert(key, Arc::clone(&matches));
        Ok(matches)
    }

    /// Add a document to the state and return its log_id.
    pub fn add_doc(&mut self, doc: LogDocument) -> String {
        self.next_id += 1;
        let log_id = format!("log_{}", self.next_id);
        self.logs.insert(log_id.clone(), Arc::new(doc));
        log_id
    }

    /// Return a stats JSON object for a loaded document.
    pub fn doc_stats(&self, doc: &LogDocument) -> Value {
        let mut top: Vec<_> = doc.templates.iter().collect();
        top.sort_by_key(|t| std::cmp::Reverse(t.count));
        let top: Vec<Value> = top
            .iter()
            .take(5)
            .map(|t| json!({ "template_id": t.id, "count": t.count, "pattern": t.pattern }))
            .collect();
        json!({
            "file": doc.file_name,
            "path": doc.path.display().to_string(),
            "lines": doc.total_lines(),
            "size_bytes": doc.file_size,
            "time_range": doc.time_range.map(|(a, b)| json!({
                "start": format_ms(a), "end": format_ms(b),
                "start_epoch_ms": a, "end_epoch_ms": b,
            })),
            "template_count": doc.templates.len(),
            "top_templates": top,
        })
    }

    /// Set the active document for GUI mode. Switches the server to simplified
    /// tool set (no log_id, no load/list/close).
    pub fn set_active_doc(&mut self, doc: Arc<LogDocument>) {
        self.gui_mode = true;
        self.active_doc = Some(doc);
        self.match_cache.retain(|(id, _, _), _| id != "_active"); // drop stale matches
        self.active_doc_dirty.store(true, Ordering::Relaxed);
    }

    /// Clear the active document while retaining the GUI tool surface.
    pub fn clear_active_doc(&mut self) {
        self.active_doc = None;
        self.match_cache.retain(|(id, _, _), _| id != "_active");
        self.filters.remove("_active");
        self.analyses.clear();
        self.analyses_dirty.store(false, Ordering::Relaxed);
    }

    /// Get the active document, or an error if none is set.
    pub fn get_active_doc(&self) -> Result<Arc<LogDocument>, String> {
        self.active_doc
            .clone()
            .ok_or_else(|| "no active log — start MCP from the GUI with a log open".to_string())
    }

    /// Returns true if the server is in GUI mode (single active doc).
    pub fn is_gui_mode(&self) -> bool {
        self.gui_mode
    }

    /// Select the GUI tool surface before a document or proxy connection is
    /// available. Used by the stable stdio GUI bridge.
    pub fn enable_gui_mode(&mut self) {
        self.gui_mode = true;
    }

    // ------------------------------------------------------------ filters

    /// Current filter keyword list for a log (empty when none are set).
    pub fn get_filters(&self, log_id: &str) -> Vec<String> {
        self.filters.get(log_id).cloned().unwrap_or_default()
    }

    /// Replace the filter set wholesale (used by GUI-originated sync).
    /// Marks the MCP→GUI `filters_dirty` flag for the active document,
    /// mirroring `set_active_doc`.
    pub fn set_filters(&mut self, log_id: &str, filters: Vec<String>) {
        if filters.is_empty() {
            self.filters.remove(log_id);
        } else {
            self.filters.insert(log_id.to_string(), filters);
        }
        if log_id == "_active" {
            self.filters_dirty.store(true, Ordering::Relaxed);
        }
    }

    /// Add a keyword filter (trimmed, case-sensitive dedupe, `MAX_FILTERS`
    /// cap). Returns the full updated filter list.
    pub fn add_filter(&mut self, log_id: &str, text: &str) -> Result<Vec<String>, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("filter_text must not be empty".to_string());
        }
        let list = self.filters.entry(log_id.to_string()).or_default();
        if list.len() >= MAX_FILTERS {
            return Err(format!("filter cap reached ({MAX_FILTERS} filters)"));
        }
        if list.iter().any(|f| f == text) {
            return Err(format!("filter '{text}' already exists"));
        }
        list.push(text.to_string());
        if log_id == "_active" {
            self.filters_dirty.store(true, Ordering::Relaxed);
        }
        Ok(list.clone())
    }

    /// Remove a filter by its `id` (position, as returned by `filters_get`).
    /// Returns the full updated filter list.
    pub fn remove_filter(&mut self, log_id: &str, id: usize) -> Result<Vec<String>, String> {
        let list = self.filters.entry(log_id.to_string()).or_default();
        if id >= list.len() {
            return Err(format!("unknown filter id {id} (see filters_get)"));
        }
        list.remove(id);
        if log_id == "_active" {
            self.filters_dirty.store(true, Ordering::Relaxed);
        }
        Ok(list.clone())
    }

    // ----------------------------------------------------------- analyses

    /// Replace the analysis cards from the GUI's served Pin tab.
    pub fn set_analyses(&mut self, analyses: Vec<PinAnalysis>) {
        self.analyses = analyses;
        self.analyses_dirty.store(true, Ordering::Relaxed);
    }

    /// Add one analysis card originating from a GUI-attached MCP client.
    pub fn add_analysis(&mut self, analysis: PinAnalysis) {
        self.analyses.push(analysis);
        self.analyses_dirty.store(true, Ordering::Relaxed);
    }

    /// The filtered view for tools running with `with_filtered_log=true`:
    /// the sorted union of line indices matching every filter keyword.
    /// `Ok(None)` when no filters are set (the view is the whole log) — callers
    /// then apply no restriction. "Everything Else" lines (matching no filter)
    /// are never included.
    pub fn visible_lines_for(&mut self, log_id: &str) -> Result<Option<Arc<Vec<usize>>>, String> {
        let filters = self.get_filters(log_id);
        if filters.is_empty() {
            return Ok(None);
        }
        let mut all: Vec<usize> = Vec::new();
        for kw in &filters {
            let m = self.matches_for(log_id, kw, true)?;
            all.extend_from_slice(m.as_slice());
        }
        all.sort_unstable();
        all.dedup();
        Ok(Some(Arc::new(all)))
    }
}

/// Start the MCP server in stdio mode (blocking, runs forever).
pub fn run_stdio(state: Arc<Mutex<ServerState>>) {
    log_init();
    set_panic_hook();
    log::info!("logotomy-mcp stdio mode started");

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut gui_client: Option<GuiClient> = None;

    for line in stdin.lock().lines() {
        let start = std::time::Instant::now();
        let Ok(line) = line else {
            log::error!("stdio read error, exiting");
            break;
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(trimmed) else {
            log::warn!("stdio: failed to parse JSON-RPC");
            continue;
        };
        let Some(id) = msg.get("id").cloned() else {
            continue;
        };
        let method = msg
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);

        log::info!("stdio request: method={method} id={}", json_id(&id));

        let response = route_stdio_request(id, &method, &params, &msg, &state, &mut gui_client);
        let elapsed = start.elapsed();
        log::info!(
            "stdio response: method={method} elapsed={}ms",
            elapsed.as_millis()
        );
        let _ = writeln!(out, "{}", response);
        let _ = out.flush();
    }
}

fn route_stdio_request(
    id: Value,
    method: &str,
    params: &Value,
    message: &Value,
    state: &Arc<Mutex<ServerState>>,
    gui_client: &mut Option<GuiClient>,
) -> String {
    let tool_name = params.get("name").and_then(Value::as_str);
    if method == "tools/call" && tool_name == Some("attach_gui_session") {
        let args = params.get("arguments").cloned().unwrap_or(json!({}));
        return attach_stdio_session(id, &args, gui_client, GuiClient::attach);
    }
    if method == "tools/call" && tool_name == Some("detach_gui_session") {
        let was_attached = gui_client.take().is_some();
        return ok(
            id,
            tool_ok(json!({
                "mode": "standalone",
                "attached": false,
                "was_attached": was_attached,
                "instruction": "Standalone mode restored. Call load_log before analysis unless a previously loaded log_id is still available."
            })),
        );
    }

    let forward_to_gui = gui_client.is_some()
        && (method == "tools/call"
            || (method == "resources/read"
                && params.get("uri").and_then(Value::as_str) == Some("logotomy://session")));
    if forward_to_gui {
        let result = gui_client.as_ref().unwrap().request(message);
        return match result {
            Ok(Some(response)) => response,
            Ok(None) => String::new(),
            Err(_) => {
                *gui_client = None;
                if method == "tools/call" {
                    ok(
                        id,
                        tool_err_retryable("GUI session expired or became unavailable. Ask the user for a new session ID, then call attach_gui_session again."),
                    )
                } else {
                    err(
                        id,
                        -32001,
                        "GUI session expired or became unavailable; attach again with a new session ID",
                    )
                }
            }
        };
    }

    let mut guard = state.lock().unwrap();
    protocol_response(id, method, params, &mut guard)
}

fn attach_stdio_session<F>(
    id: Value,
    args: &Value,
    gui_client: &mut Option<GuiClient>,
    connect: F,
) -> String
where
    F: FnOnce(&str) -> Result<GuiClient, String>,
{
    let session_id = args.get("session_id").and_then(Value::as_str).unwrap_or("");
    match connect(session_id) {
        Ok(client) => {
            *gui_client = Some(client);
            ok(
                id,
                tool_ok(json!({
                    "mode": "gui_attached",
                    "temporary": true,
                    "attached": true,
                    "next_call": "session_info",
                    "instruction": "The selected GUI log is attached. Do not call load_log or pass log_id while attached."
                })),
            )
        }
        Err(_) => ok(
            id,
            tool_err("GUI session ID is invalid, expired, or unavailable"),
        ),
    }
}

/// Run the GUI-owned private IPC socket. This is not an MCP transport: agent
/// clients only connect to `logotomy --mcp` over stdio. Each short-lived local
/// connection carries an authenticated JSON envelope containing one JSON-RPC
/// request, which is dispatched against the GUI's live state.
pub fn run_gui_ipc(
    port: u16,
    state: Arc<Mutex<ServerState>>,
    shutdown: Arc<AtomicBool>,
    bound_tx: Option<std::sync::mpsc::Sender<Result<u16, String>>>,
    session_id: String,
) -> Result<(), String> {
    log_init();
    set_panic_hook();
    let address = format!("127.0.0.1:{port}");
    let listener = match TcpListener::bind(&address) {
        Ok(listener) => listener,
        Err(error) => {
            let message = format!("Failed to bind private GUI socket at {address}: {error}");
            if let Some(sender) = bound_tx {
                let _ = sender.send(Err(message.clone()));
            }
            return Err(message);
        }
    };
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("Failed to configure private GUI socket: {error}"))?;
    let actual_port = listener
        .local_addr()
        .map_err(|error| format!("Failed to inspect private GUI socket: {error}"))?
        .port();
    if let Some(sender) = bound_tx {
        let _ = sender.send(Ok(actual_port));
    }
    log::info!("private GUI IPC started");

    while !shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let state = Arc::clone(&state);
                let expected_id = session_id.clone();
                thread::spawn(move || handle_gui_ipc_client(stream, state, &expected_id));
            }
            Err(ref error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => log::error!("private GUI IPC connection failed: {error}"),
        }
    }
    log::info!("private GUI IPC stopped");
    Ok(())
}

fn handle_gui_ipc_client(
    mut stream: TcpStream,
    state: Arc<Mutex<ServerState>>,
    expected_session_id: &str,
) {
    // Accepted sockets inherit the listener's nonblocking mode on macOS.
    // Request framing relies on EOF, so make the per-client stream blocking
    // before reading it; otherwise an early EAGAIN is treated as a rejected
    // GUI request.
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(60)));
    let mut bytes = Vec::new();
    if Read::by_ref(&mut stream)
        .take(16 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return;
    }
    let Ok(envelope) = serde_json::from_slice::<Value>(&bytes) else {
        return;
    };
    let supplied_id = envelope
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    if !session::session_id_matches(expected_session_id, supplied_id) {
        return;
    }
    let Some(request) = envelope.get("request") else {
        return;
    };
    let Some(id) = request.get("id").cloned() else {
        return;
    };
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let params = request.get("params").cloned().unwrap_or(Value::Null);
    let response = protocol_response(id, method, &params, &mut state.lock().unwrap());
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// Short string representation of a JSON-RPC id value.
fn json_id(id: &Value) -> String {
    match id {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// JSON-RPC helpers
// ---------------------------------------------------------------------------

fn initialize_result(params: &Value, state: &ServerState) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = match requested {
        Some(v) if PROTOCOL_VERSIONS[1..].contains(&v) => v,
        _ => PROTOCOL_VERSIONS[1],
    };
    json!({
        "protocolVersion": version,
        "capabilities": server_capabilities(),
        "serverInfo": {
            "name": "logotomy-mcp",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": mode_instructions(state)
    })
}

fn modern_request_version(params: &Value) -> Option<&str> {
    params
        .get("_meta")
        .and_then(|meta| meta.get("io.modelcontextprotocol/protocolVersion"))
        .and_then(Value::as_str)
}

fn modern_request_error(id: Value, requested: Option<&str>, message: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32022,
            "message": message,
            "data": {
                "supported": PROTOCOL_VERSIONS,
                "requested": requested
            }
        }
    })
    .to_string()
}

fn server_capabilities() -> Value {
    json!({ "tools": {}, "resources": {} })
}

fn server_discover_result(state: &ServerState) -> Value {
    json!({
        "resultType": "complete",
        "supportedVersions": PROTOCOL_VERSIONS,
        "capabilities": server_capabilities(),
        "_meta": {
            "io.modelcontextprotocol/serverInfo": {
                "name": "logotomy-mcp",
                "version": env!("CARGO_PKG_VERSION")
            }
        },
        "instructions": mode_instructions(state),
        "ttlMs": 0,
        "cacheScope": "private"
    })
}

fn mode_instructions(_state: &ServerState) -> &'static str {
    "Call session_info first. Logotomy starts in standalone mode: call load_log with an absolute path and retain its log_id. To analyze the log selected in a running Logotomy GUI, call attach_gui_session with the temporary session ID copied from the GUI. In GUI-attached mode, the GUI already supplies the open log: load_log, list_logs, and close_log are unavailable, and log_id is not needed. Suggested approach: understand the user's problem and Pin-tab findings with get_analysis; explore the log shape with summarize_log (with_filtered_log=false for full-log orientation) and targeted find_occurrences; then use filters (filters_add) and trim when a hypothesis merits a narrower scope. Request bounded raw_log only for exact evidence, and add useful evidence-backed root-cause conclusions with add_analysis."
}

fn session_info_payload(state: &ServerState) -> Value {
    if state.is_gui_mode() {
        let active_log = state.active_doc.as_ref().map(|doc| {
            json!({
                "file": doc.file_name,
                "path": doc.path.display().to_string(),
                "lines": doc.total_lines()
            })
        });
        json!({
            "mode": "gui_attached",
            "gui_attached": true,
            "temporary": true,
            "connected": state.active_doc.is_some(),
            "active_log": active_log,
            "filter_count": state.get_filters("_active").len(),
            "active_log_can_change": true,
            "lifecycle": "Owned by the running Logotomy GUI; access ends when MCP is stopped or the GUI exits."
        })
    } else {
        json!({
            "mode": "standalone",
            "gui_attached": false,
            "temporary": false,
            "connected": true,
            "loaded_log_count": state.logs.len(),
            "active_log_can_change": false,
            "lifecycle": "Owned by this MCP stdio process. Call attach_gui_session with a temporary GUI session ID to switch targets."
        })
    }
}

fn resources_list() -> Value {
    json!({
        "resources": [
            {
                "uri": "logotomy://session",
                "name": "Logotomy MCP session",
                "description": "Current standalone or GUI-attached target, active log, filters, and temporary-session lifecycle.",
                "mimeType": "application/json"
            },
            {
                "uri": "logotomy://guide",
                "name": "Logotomy investigation guide",
                "description": "Compact, token-efficient workflow for investigating logs with Logotomy tools.",
                "mimeType": "text/markdown"
            }
        ]
    })
}

fn resource_read(params: &Value, state: &ServerState) -> Result<Value, String> {
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing or invalid resource uri".to_string())?;
    let (mime_type, text) = match uri {
        "logotomy://session" => (
            "application/json",
            serde_json::to_string_pretty(&session_info_payload(state))
                .map_err(|e| format!("failed to encode session resource: {e}"))?,
        ),
        "logotomy://guide" => (
            "text/markdown",
            "# Logotomy investigation guide\n\nCall `session_info` first. In `standalone` mode, call `load_log` and retain its `log_id`. In `gui_attached` mode, the GUI already provides the open log: `load_log`, `list_logs`, and `close_log` are unavailable, and `log_id` is not needed.\n\nSuggested approach, not a required checklist: understand the user's question and Pin-tab findings with `get_analysis`; explore the log shape with `summarize_log` (`with_filtered_log=false` for full-log orientation) and targeted `find_occurrences`; then use filters (`filters_add`), anomalies, histograms, templates, sequences, and `trim` as they help test a hypothesis and narrow the scope. Request bounded `raw_log` only for exact evidence, honor pagination and `truncated`, and add useful evidence-backed root-cause conclusions with `add_analysis`.\n".to_string(),
        ),
        _ => return Err(format!("unknown resource uri '{uri}'")),
    };
    Ok(json!({
        "contents": [{ "uri": uri, "mimeType": mime_type, "text": text }]
    }))
}

fn protocol_response(id: Value, method: &str, params: &Value, state: &mut ServerState) -> String {
    let modern_version = modern_request_version(params);
    if let Some(version) = modern_version {
        if version != MODERN_PROTOCOL_VERSION {
            return modern_request_error(id, Some(version), "Unsupported protocol version");
        }
        let meta = params.get("_meta").and_then(Value::as_object);
        if meta
            .and_then(|m| m.get("io.modelcontextprotocol/clientCapabilities"))
            .and_then(Value::as_object)
            .is_none()
        {
            return err(
                id,
                -32602,
                "modern MCP requests require _meta.io.modelcontextprotocol/clientCapabilities",
            );
        }
    }

    match method {
        "server/discover" if modern_version.is_some() => ok(id, server_discover_result(state)),
        "server/discover" => err(id, -32601, "method not found: server/discover"),
        "initialize" if modern_version.is_none() => ok(id, initialize_result(params, state)),
        "ping" => ok(id, json!({})),
        "tools/list" => ok(id, json!({ "tools": tools(state) })),
        "tools/call" => handle_tool_call(id, params, state),
        "resources/list" => ok(id, resources_list()),
        "resources/read" => match resource_read(params, state) {
            Ok(result) => ok(id, result),
            Err(message) => err(id, -32602, &message),
        },
        _ => err(id, -32601, &format!("method not found: {method}")),
    }
}

fn ok(id: Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn err(id: Value, code: i64, message: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
    .to_string()
}

fn tool_ok(payload: Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": payload.to_string() }],
        "structuredContent": payload
    })
}

fn tool_err(message: &str) -> Value {
    tool_error(message, false)
}

fn tool_err_retryable(message: &str) -> Value {
    tool_error(message, true)
}

fn tool_error(message: &str, retryable: bool) -> Value {
    let payload = json!({
        "error": "tool_error",
        "message": message,
        "retryable": retryable
    });
    json!({
        "content": [{ "type": "text", "text": payload.to_string() }],
        "structuredContent": payload,
        "isError": true
    })
}

fn handle_tool_call(id: Value, params: &Value, state: &mut ServerState) -> String {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    log::info!("tool call: {name}");
    let result = dispatch(&name, &args, state);
    let payload = match result {
        Ok(v) => tool_ok(v),
        Err(e) => {
            log::error!("tool error: {name}: {e}");
            tool_err(&e)
        }
    };
    ok(id, payload)
}

fn dispatch(name: &str, args: &Value, state: &mut ServerState) -> Result<Value, String> {
    if name == "session_info" {
        return Ok(session_info_payload(state));
    }
    if matches!(name, "attach_gui_session" | "detach_gui_session") {
        return Err(format!(
            "{name} is available only through the public `logotomy --mcp` stdio server"
        ));
    }
    if state.is_gui_mode() && args.get("log_id").is_some() {
        return Err(
            "log_id must be omitted while attached to a GUI session; the selected GUI log is already the target"
                .to_string(),
        );
    }
    if matches!(name, "get_analysis" | "add_analysis") {
        if !state.is_gui_mode() {
            return Err(format!(
                "{name} is available only while attached to a GUI session; call attach_gui_session first"
            ));
        }
        return match name {
            "get_analysis" => tool_get_analysis(state),
            "add_analysis" => tool_add_analysis(args, state),
            _ => unreachable!(),
        };
    }
    // Compression-first tools work identically in both modes; they resolve the
    // target document from the mode (log_id argument in headless mode, the
    // active document in GUI mode).
    match name {
        "summarize_log" => return tool_summarize(args, state),
        "get_timeline_histogram" => return tool_timeline_histogram(args, state),
        "get_template_anomalies" => return tool_template_anomalies(args, state),
        "get_template" => return tool_get_template(args, state),
        "get_template_samples" => return tool_template_samples(args, state),
        "log_sequence" => return tool_log_sequence(args, state),
        "raw_log" => return tool_raw_log(args, state),
        "filters_get" => return tool_filters_get(args, state),
        "filters_add" => return tool_filters_add(args, state),
        "filters_remove" => return tool_filters_remove(args, state),
        _ => {}
    }
    if state.is_gui_mode() {
        // GUI mode: simplified tools operating on the single active doc.
        match name {
            "find_occurrences" => tool_find_occurrences_simple(args, state),
            "trim" => tool_trim_gui(args, state),
            other => Err(format!("unknown tool '{other}' — in GUI mode, tools load_log, list_logs, and close_log are not available. Use the simplified tools which operate on the currently open log.")),
        }
    } else {
        // Headless mode: full tool set with log_id.
        match name {
            "load_log" => tool_load_log(args, state),
            "list_logs" => tool_list_logs(state),
            "close_log" => tool_close_log(args, state),
            "find_occurrences" => tool_find_occurrences(args, state),
            "trim" => Err(
                "trim is available only while attached to a GUI session; call attach_gui_session first"
                    .to_string(),
            ),
            other => Err(format!("unknown tool '{other}'")),
        }
    }
}

// ---------------------------------------------------------------- arguments

fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing or invalid argument '{key}' (string expected)"))
}

fn arg_log_id(args: &Value) -> Result<&str, String> {
    arg_str(args, "log_id")
}

fn arg_usize(args: &Value, key: &str, default: usize) -> Result<usize, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Number(n)) => n
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| format!("argument '{key}' must be a non-negative integer")),
        Some(_) => Err(format!("argument '{key}' must be a non-negative integer")),
    }
}

fn arg_bool(args: &Value, key: &str, default: bool) -> Result<bool, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(format!("argument '{key}' must be a boolean")),
    }
}

fn arg_time(args: &Value, key: &str) -> Result<Option<i64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_i64()
            .map(Some)
            .ok_or_else(|| format!("argument '{key}' is not a valid integer")),
        Some(Value::String(s)) => parse_time_param(s)
            .map(Some)
            .ok_or_else(|| format!("argument '{key}': cannot parse '{s}' as time")),
        _ => Err(format!(
            "argument '{key}' must be a time string or epoch millis"
        )),
    }
}

// ------------------------------------------------------------------- tools
// Headless tools (with log_id)
// -------------------------------------------------------------------

fn tool_load_log(args: &Value, state: &mut ServerState) -> Result<Value, String> {
    let path = arg_str(args, "path")?;
    log::info!("load_log: {path}");
    let doc = LogDocument::open(Path::new(path))?;
    let log_id = state.add_doc(doc);
    let stats = state.doc_stats(&*state.get_doc(&log_id)?);
    Ok(json!({
        "log_id": log_id,
        "stats": stats,
    }))
}

fn tool_list_logs(state: &mut ServerState) -> Result<Value, String> {
    let mut logs: Vec<(String, Value)> = state
        .logs
        .iter()
        .map(|(id, doc)| (id.clone(), state.doc_stats(doc)))
        .collect();
    logs.sort_by(|a, b| a.0.cmp(&b.0));
    let entries: Vec<Value> = logs
        .into_iter()
        .map(|(id, stats)| json!({ "log_id": id, "stats": stats }))
        .collect();
    Ok(json!({ "logs": entries }))
}

fn tool_close_log(args: &Value, state: &mut ServerState) -> Result<Value, String> {
    let log_id = arg_log_id(args)?;
    if state.logs.remove(log_id).is_none() {
        return Err(format!("unknown log_id '{log_id}'"));
    }
    state.match_cache.retain(|(id, _, _), _| id != log_id);
    state.filters.remove(log_id);
    Ok(json!({ "closed": log_id }))
}

// ------------------------------------------------------------------- tools
// Filter-state tools (both modes; headless takes log_id, GUI does not)
// -------------------------------------------------------------------

/// Resolve the target log for the filter tools: the active doc in GUI mode
/// (key `_active`) or the `log_id` argument in headless mode. Validates the
/// target exists so `filters_*` behaves like every other per-log tool.
fn resolve_filter_log(state: &ServerState, args: &Value) -> Result<String, String> {
    if state.is_gui_mode() {
        state.get_active_doc()?; // require a live active document
        Ok("_active".to_string())
    } else {
        let log_id = arg_log_id(args)?.to_string();
        state.get_doc(&log_id)?;
        Ok(log_id)
    }
}

/// Render the filter list as `[{id, filter_text}, …]` where `id` is the
/// filter's position in the list.
fn filters_payload(filters: &[String]) -> Vec<Value> {
    filters
        .iter()
        .enumerate()
        .map(|(id, text)| json!({ "id": id, "filter_text": text }))
        .collect()
}

fn filters_out(log_id: &str, filters: &[String]) -> Value {
    let mut out = json!({
        "filters": filters_payload(filters),
        "filter_count": filters.len(),
    });
    if log_id != "_active" {
        out["log_id"] = json!(log_id);
    }
    out
}

/// List the current filter set for the target log.
fn tool_filters_get(args: &Value, state: &mut ServerState) -> Result<Value, String> {
    let log_id = resolve_filter_log(state, args)?;
    let filters = state.get_filters(&log_id);
    Ok(filters_out(&log_id, &filters))
}

/// Add a keyword filter; returns the full updated filter list.
fn tool_filters_add(args: &Value, state: &mut ServerState) -> Result<Value, String> {
    let log_id = resolve_filter_log(state, args)?;
    let text = arg_str(args, "filter_text")?;
    let filters = state.add_filter(&log_id, text)?;
    Ok(filters_out(&log_id, &filters))
}

/// Remove a filter by id; returns the full updated filter list.
fn tool_filters_remove(args: &Value, state: &mut ServerState) -> Result<Value, String> {
    let log_id = resolve_filter_log(state, args)?;
    let id = args
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| "missing or invalid argument 'id' (integer expected)".to_string())?
        as usize;
    let filters = state.remove_filter(&log_id, id)?;
    Ok(filters_out(&log_id, &filters))
}

// ------------------------------------------------------------------- tools
// GUI Pin-tab analysis tools
// -------------------------------------------------------------------

fn analyses_payload(analyses: &[PinAnalysis]) -> Value {
    let analyses: Vec<Value> = analyses
        .iter()
        .map(|analysis| {
            json!({
                "text": analysis.text,
                "lines": analysis.lines.iter().map(|line| line + 1).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({ "analyses": analyses })
}

/// Return the current GUI Pin-tab entries as agent-readable analyses.
fn tool_get_analysis(state: &ServerState) -> Result<Value, String> {
    state.get_active_doc()?; // Require an active tab, like every GUI tool.
    Ok(analyses_payload(&state.analyses))
}

/// Add an analysis card to the active GUI Pin tab. Public MCP lines are
/// one-based; zero is rejected so it cannot silently become the first line.
fn tool_add_analysis(args: &Value, state: &mut ServerState) -> Result<Value, String> {
    let doc = state.get_active_doc()?;
    let text = arg_str(args, "text")?.trim();
    if text.is_empty() {
        return Err("text must not be empty".to_string());
    }
    let values: &[Value] = match args.get("lines") {
        None | Some(Value::Null) => &[],
        Some(Value::Array(values)) => values.as_slice(),
        Some(_) => {
            return Err("argument 'lines' must be an array of 1-based line numbers".to_string())
        }
    };
    let mut lines = Vec::with_capacity(values.len());
    for value in values {
        let line = value.as_u64().ok_or_else(|| {
            "argument 'lines' must contain only positive integer line numbers".to_string()
        })?;
        if line == 0 || line as usize > doc.total_lines() {
            return Err(format!(
                "line {line} is outside the active log (valid range: 1..={})",
                doc.total_lines()
            ));
        }
        lines.push(line as usize - 1);
    }
    lines.sort_unstable();
    lines.dedup();
    state.add_analysis(PinAnalysis {
        text: text.to_string(),
        lines,
    });
    Ok(analyses_payload(&state.analyses))
}

fn tool_find_occurrences(args: &Value, state: &mut ServerState) -> Result<Value, String> {
    let log_id = arg_log_id(args)?.to_string();
    let doc = state.get_doc(&log_id)?;
    let visible = match filtered_view_or_no_log(state, &log_id, args) {
        Ok(v) => v,
        Err(payload) => return Ok(payload),
    };
    let keyword = arg_str(args, "keyword")?.to_string();
    let case_sensitive = arg_bool(args, "case_sensitive", true)?;
    let matches = state.matches_for(&log_id, &keyword, case_sensitive)?;
    find_occurrences_payload(
        doc.as_ref(),
        &matches,
        args,
        visible.as_deref().map(|v| v.as_slice()),
    )
}

// ------------------------------------------------------------------- tools
// GUI simplified tools (no log_id, operate on active_doc)
// -------------------------------------------------------------------

fn tool_find_occurrences_simple(args: &Value, state: &mut ServerState) -> Result<Value, String> {
    let doc = state.get_active_doc()?;
    let log_id = "_active".to_string();
    let visible = match filtered_view_or_no_log(state, &log_id, args) {
        Ok(v) => v,
        Err(payload) => return Ok(payload),
    };
    let keyword = arg_str(args, "keyword")?.to_string();
    let case_sensitive = arg_bool(args, "case_sensitive", true)?;
    let matches = state.matches_for(&log_id, &keyword, case_sensitive)?;
    find_occurrences_payload(
        doc.as_ref(),
        &matches,
        args,
        visible.as_deref().map(|v| v.as_slice()),
    )
}

fn line_payload(doc: &LogDocument, i: usize) -> Value {
    let line = doc.line(i);
    let text = if line.len() > MAX_LINE_TEXT {
        format!("{}…", line.get(..MAX_LINE_TEXT).unwrap_or(&line))
    } else {
        line.into_owned()
    };
    let t = doc.ts_at(i);
    json!({
        "line": i + 1,
        "time": if t >= 0 { Value::String(format_ms(t)) } else { Value::Null },
        "template_id": doc.template_at(i),
        "text": text,
    })
}

// ------------------------------------------------------------------- tools
// Compression-first analysis tools (shared by headless + GUI modes)
// -------------------------------------------------------------------

/// Minimum run length that gets collapsed in sequence/context views.
const MIN_COLLAPSE_RUN: usize = 3;
/// Max chars of sample text embedded in collapsed runs and template entries.
const SAMPLE_CHARS: usize = 200;
/// Line-index buckets used for burst detection in get_template_anomalies.
const ANOMALY_BUCKETS: usize = 50;
/// Hard cap on dense `log_sequence` entries returned in a single call.
const HARD_MAX_SEQUENCE: usize = 2000;
/// Rough per-entry byte estimate for a dense log_sequence triple.
const SEQ_BYTES_PER_ENTRY: usize = 40;

/// Resolve the target document for a compression-first tool: the active doc
/// in GUI mode, or the `log_id` argument in headless mode. Returns the cache
/// key used for match lookups ("_active" in GUI mode).
fn resolve_doc(state: &ServerState, args: &Value) -> Result<(String, Arc<LogDocument>), String> {
    if state.is_gui_mode() {
        Ok(("_active".to_string(), state.get_active_doc()?))
    } else {
        let log_id = arg_log_id(args)?.to_string();
        let doc = state.get_doc(&log_id)?;
        Ok((log_id, doc))
    }
}

/// Whether a tool should restrict its results to the filtered view of the
/// target log. Defaults to `true` when the argument is absent. The "Everything
/// Else" lane is never part of MCP arithmetic — the filtered view is the union
/// of the filter keywords' matches (see `ServerState::visible_lines_for`).
fn with_filtered_log(args: &Value) -> bool {
    args.get("with_filtered_log")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

/// The reply produced when a tool runs with `with_filtered_log=true` but the
/// log has no filter keywords set — there is no filtered log to work on. It is
/// returned as a successful tool result so the agent can act on the `reason`.
fn no_filtered_log_payload() -> Value {
    json!({
        "comment": "no log",
        "reason": "with_filtered_log=true and no filters are configured, so the filtered view is empty",
        "filter_count": 0,
        "suggested_next_calls": [
            { "tool": "summarize_log", "arguments": { "with_filtered_log": false }, "purpose": "analyze the full log" },
            { "tool": "filters_add", "arguments": { "filter_text": "ERROR" }, "purpose": "create a filtered investigation scope" }
        ]
    })
}

/// Resolve the filtered view for a tool using `with_filtered_log`. Short-
/// circuits: when the flag is true (the default) but no filter keyword is set,
/// returns `Err(payload)` so the tool bails with the "no log" message. When
/// the flag is false, returns `Ok(None)` (no restriction — the full log).
fn filtered_view_or_no_log(
    state: &mut ServerState,
    log_id: &str,
    args: &Value,
) -> Result<Option<Arc<Vec<usize>>>, Value> {
    if !with_filtered_log(args) {
        return Ok(None);
    }
    if state.get_filters(log_id).is_empty() {
        return Err(no_filtered_log_payload());
    }
    state.visible_lines_for(log_id).map_err(Value::String)
}

fn template_pattern(doc: &LogDocument, id: u32) -> String {
    doc.templates
        .iter()
        .find(|t| t.id == id)
        .map(|t| t.pattern.clone())
        .unwrap_or_else(|| "<unknown>".to_string())
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}…", s.get(..max).unwrap_or(s))
    } else {
        s.to_string()
    }
}

fn fmt_ts(t: i64) -> Value {
    if t >= 0 {
        Value::String(format_ms(t))
    } else {
        Value::Null
    }
}

/// A dense `log_sequence` entry: `[epoch_ms | null, line, template_id]`.
fn seq_entry(doc: &LogDocument, i: usize) -> Value {
    let t = doc.ts_at(i);
    json!([
        if t >= 0 { Value::from(t) } else { Value::Null },
        i + 1,
        doc.template_at(i),
    ])
}

/// A `log_sequence` collapsed-run entry (used when `collapse=true`).
fn collapsed_run_entry(doc: &LogDocument, r: &Run) -> Value {
    json!({
        "start_line": r.start + 1,
        "end_line": r.end + 1,
        "count": r.count,
        "template_id": r.tpl,
        "pattern": template_pattern(doc, r.tpl),
        "time_start": fmt_ts(doc.ts_at(r.start)),
        "time_end": fmt_ts(doc.ts_at(r.end)),
    })
}

/// Emit sequence items (+ covered line count) for a list of template runs,
/// collapsing runs of `MIN_COLLAPSE_RUN` or more and stopping at
/// `max_entries`.
fn seq_runs_to_items(doc: &LogDocument, runs: &[Run], max_entries: usize) -> (Vec<Value>, usize) {
    let mut items = Vec::new();
    let mut covered = 0usize;
    for r in runs {
        if items.len() >= max_entries {
            break;
        }
        let len = r.count;
        if len >= MIN_COLLAPSE_RUN {
            items.push(collapsed_run_entry(doc, r));
            covered += len;
        } else {
            for i in r.start..=r.end {
                if items.len() >= max_entries {
                    break;
                }
                items.push(seq_entry(doc, i));
                covered += 1;
            }
        }
    }
    (items, covered)
}

/// Resolve an optional `start`/`end` bound into a 0-based, trim-relative line
/// index. Integers are 1-based line numbers (clamped to the visible window);
/// strings are parsed as timestamps and binary-searched against the
/// forward-filled per-line timestamps. `is_end` selects the inclusive end
/// (last line with ts <= t) vs. the inclusive start (first line with ts >= t).
/// Returns `Ok(None)` when the argument is absent/null.
fn resolve_bound(
    v: Option<&Value>,
    doc: &LogDocument,
    is_end: bool,
) -> Result<Option<usize>, String> {
    let n = doc.total_lines();
    if n == 0 {
        return Ok(None);
    }
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(num)) => {
            let line = num
                .as_u64()
                .ok_or_else(|| "line bound must be a non-negative integer".to_string())?
                as usize;
            if line == 0 {
                return Err("line numbers are 1-based (0 is invalid)".to_string());
            }
            Ok(Some((line - 1).min(n - 1)))
        }
        Some(Value::String(s)) => {
            let t = parse_time_param(s).ok_or_else(|| format!("cannot parse '{s}' as a time"))?;
            // ts_at is forward-filled and monotonically non-decreasing, so a
            // binary search finds the boundary in O(log n).
            if is_end {
                // Last visible index whose ts <= t: first index with ts > t, minus 1.
                let (mut lo, mut hi) = (0usize, n);
                while lo < hi {
                    let mid = lo + (hi - lo) / 2;
                    if doc.ts_at(mid) <= t {
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                Ok(Some(lo.saturating_sub(1)))
            } else {
                // First visible index whose ts >= t.
                let (mut lo, mut hi) = (0usize, n);
                while lo < hi {
                    let mid = lo + (hi - lo) / 2;
                    if doc.ts_at(mid) < t {
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                Ok(Some(lo.min(n - 1)))
            }
        }
        Some(_) => Err("bound must be an integer (line) or string (time)".to_string()),
    }
}

/// Resolve an optional `start`/`end` pair into a `(lo, hi)` half-open visible
/// range, defaulting to the full visible window.
fn resolve_range(args: &Value, doc: &LogDocument) -> Result<(usize, usize), String> {
    let n = doc.total_lines();
    let lo = resolve_bound(args.get("start"), doc, false)?.unwrap_or(0);
    let hi = resolve_bound(args.get("end"), doc, true)?
        .map(|e| e + 1)
        .unwrap_or(n)
        .min(n);
    if hi <= lo {
        return Err("empty range (end is before start)".to_string());
    }
    Ok((lo, hi))
}

/// (min, max) forward-filled timestamp over the visible range `[lo, hi)`.
fn range_time(doc: &LogDocument, lo: usize, hi: usize) -> Option<(i64, i64)> {
    time_range_of_lines(doc, lo..hi)
}

/// (min, max) forward-filled timestamp over an arbitrary line-index iterator
/// (e.g. the filtered view).
fn time_range_of_lines(
    doc: &LogDocument,
    lines: impl Iterator<Item = usize>,
) -> Option<(i64, i64)> {
    let mut mn = i64::MAX;
    let mut mx = i64::MIN;
    for i in lines {
        let t = doc.ts_at(i);
        if t >= 0 {
            mn = mn.min(t);
            mx = mx.max(t);
        }
    }
    if mn <= mx {
        Some((mn, mx))
    } else {
        None
    }
}

/// Per-template occurrence counts within the visible range `[lo, hi)`.
fn template_counts_in_range(doc: &LogDocument, lo: usize, hi: usize) -> HashMap<u32, usize> {
    template_counts_of(doc, lo..hi)
}

/// Per-template occurrence counts over an arbitrary line-index iterator (used
/// for the filtered view).
fn template_counts_of(
    doc: &LogDocument,
    lines: impl Iterator<Item = usize>,
) -> HashMap<u32, usize> {
    let mut counts = HashMap::new();
    for i in lines {
        *counts.entry(doc.template_at(i)).or_insert(0) += 1;
    }
    counts
}

/// Build the `{ "<id>": { pattern, count, example_line } }` dictionary, either
/// for a specific set of `ids` or (when `None`) for every template.
fn template_dict(doc: &LogDocument, ids: Option<&[u32]>) -> Value {
    let mut map = serde_json::Map::new();
    for t in &doc.templates {
        if let Some(ids) = ids {
            if !ids.contains(&t.id) {
                continue;
            }
        }
        map.insert(
            t.id.to_string(),
            json!({
                "pattern": t.pattern,
                "count": t.count,
                "example_line": t.example_line.saturating_sub(doc.trim_start) + 1,
            }),
        );
    }
    Value::Object(map)
}

/// Like `template_dict`, but with externally-computed per-template counts
/// (e.g. restricted to the filtered view). Every template in the document is
/// still listed — count 0 when it has no visible lines.
fn template_dict_counts(
    doc: &LogDocument,
    ids: Option<&[u32]>,
    counts: &HashMap<u32, usize>,
) -> Value {
    let mut map = serde_json::Map::new();
    for t in &doc.templates {
        if let Some(ids) = ids {
            if !ids.contains(&t.id) {
                continue;
            }
        }
        map.insert(
            t.id.to_string(),
            json!({
                "pattern": t.pattern,
                "count": counts.get(&t.id).copied().unwrap_or(0),
                "example_line": t.example_line.saturating_sub(doc.trim_start) + 1,
            }),
        );
    }
    Value::Object(map)
}
/// For contiguous ranges `count == end - start + 1`; for filtered views (a
/// sorted subset of line indices) `count` is the number of lines in the run.
struct Run {
    start: usize,
    end: usize, // inclusive
    tpl: u32,
    count: usize,
}

/// Group the lines in `[lo, hi)` into template runs.
fn build_runs(doc: &LogDocument, lo: usize, hi: usize) -> Vec<Run> {
    let mut runs = Vec::new();
    let mut i = lo;
    while i < hi {
        let tpl = doc.template_at(i);
        let mut j = i + 1;
        while j < hi && doc.template_at(j) == tpl {
            j += 1;
        }
        runs.push(Run {
            start: i,
            end: j - 1,
            tpl,
            count: j - i,
        });
        i = j;
    }
    runs
}

/// Group any sorted line-index list (e.g. the filtered view) into template
/// runs. Used for `with_filtered_log=true` views where line indices are not
/// contiguous.
fn build_runs_of(doc: &LogDocument, lines: &[usize]) -> Vec<Run> {
    let mut runs = Vec::new();
    let mut w = 0;
    while w < lines.len() {
        let tpl = doc.template_at(lines[w]);
        let mut j = w + 1;
        while j < lines.len() && doc.template_at(lines[j]) == tpl {
            j += 1;
        }
        runs.push(Run {
            start: lines[w],
            end: lines[j - 1],
            tpl,
            count: j - w,
        });
        w = j;
    }
    runs
}

/// find_occurrences core: paginated `[line_number, epoch_ms|null]` tuples.
/// `visible` is the filtered view (from `ServerState::visible_lines_for`)
/// when `with_filtered_log=true` and filters are set; `None` means no
/// restriction.
fn find_occurrences_payload(
    doc: &LogDocument,
    matches: &[usize],
    args: &Value,
    visible: Option<&[usize]>,
) -> Result<Value, String> {
    let after = arg_time(args, "after")?;
    let before = arg_time(args, "before")?;
    let max_results = arg_usize(args, "max_results", 50)?.min(HARD_MAX_LINES);
    let offset = arg_usize(args, "offset", 0)?;
    if args.get("format").is_some() || args.get("context").is_some() {
        return Err(
            "find_occurrences now returns only [line_number, epoch_ms|null] tuples; remove 'format' and 'context'"
                .to_string(),
        );
    }
    // Optional time-window filter (after/before) on forward-filled timestamps,
    // intersected with the filtered view when one is active.
    let windowed: Vec<usize> = matches
        .iter()
        .copied()
        .filter(|&i| {
            let t = doc.ts_at(i);
            after.map_or(true, |a| t >= a)
                && before.map_or(true, |b| t <= b)
                && visible.map_or(true, |v| v.binary_search(&i).is_ok())
        })
        .collect();
    let page: Vec<usize> = windowed
        .iter()
        .skip(offset)
        .take(max_results)
        .copied()
        .collect();
    let occurrences: Vec<Value> = page
        .iter()
        .map(|&i| {
            let timestamp = doc.ts_at(i);
            json!([
                i + 1,
                if timestamp >= 0 {
                    Value::from(timestamp)
                } else {
                    Value::Null
                }
            ])
        })
        .collect();
    Ok(json!({
        "total_matches": windowed.len(),
        "offset": offset,
        "returned": occurrences.len(),
        "has_more": offset.saturating_add(occurrences.len()) < windowed.len(),
        "occurrences": occurrences,
    }))
}

/// summarize_log: one-call orientation — stats, top templates, error-ish
/// templates, biggest time gaps, densest minute.
fn tool_summarize(args: &Value, state: &mut ServerState) -> Result<Value, String> {
    let (log_id, doc) = resolve_doc(state, args)?;
    // Short-circuit: with_filtered_log=true (the default) and no filters set →
    // there is no filtered log to work on.
    let vis = match filtered_view_or_no_log(state, &log_id, args) {
        Ok(v) => v,
        Err(payload) => return Ok(payload),
    };
    if doc.total_lines() == 0 {
        return Err("log is empty".to_string());
    }
    let (lo, hi) = resolve_range(args, &doc)?;
    // When filters are set, every metric below is restricted to the union of
    // their matches; the "Everything Else" lane is never included.
    let (vis_a, vis_b) = match &vis {
        Some(v) => (
            v.partition_point(|&i| i < lo),
            v.partition_point(|&i| i < hi),
        ),
        None => (0, 0),
    };
    let m = match &vis {
        Some(_) => vis_b - vis_a,
        None => hi - lo,
    };

    // Per-template occurrence counts within the analysed (possibly filtered)
    // range.
    let counts = match &vis {
        Some(v) => template_counts_of(&doc, v[vis_a..vis_b].iter().copied()),
        None => template_counts_in_range(&doc, lo, hi),
    };

    // Ranked templates (id, pattern, example_line, range count), count > 0.
    let mut ranked: Vec<(u32, String, usize, usize)> = doc
        .templates
        .iter()
        .filter_map(|t| {
            let c = counts.get(&t.id).copied().unwrap_or(0);
            if c == 0 {
                return None;
            }
            Some((t.id, t.pattern.clone(), t.example_line, c))
        })
        .collect();
    ranked.sort_by_key(|e| std::cmp::Reverse(e.3));
    let top_templates: Vec<Value> = ranked
        .iter()
        .take(5)
        .map(|(id, pattern, example, c)| {
            json!({
                "template_id": id,
                "count": c,
                "pattern": pattern,
                "example_line": example.saturating_sub(doc.trim_start) + 1,
            })
        })
        .collect();

    // Error-ish templates within the range.
    const ERRORISH: &[&str] = &["error", "fatal", "exception", "panic"];
    let err_tpls: Vec<&(u32, String, usize, usize)> = ranked
        .iter()
        .filter(|(_, pattern, _, _)| {
            let p = pattern.to_lowercase();
            ERRORISH.iter().any(|k| p.contains(k))
        })
        .collect();
    let error_line_count: usize = err_tpls.iter().map(|(_, _, _, c)| *c).sum();
    let error_templates: Vec<Value> = err_tpls
        .iter()
        .take(20)
        .map(|(id, pattern, example, c)| {
            json!({
                "template_id": id,
                "count": c,
                "pattern": truncate(pattern, SAMPLE_CHARS),
                "example_line": example.saturating_sub(doc.trim_start) + 1,
            })
        })
        .collect();

    // Top-3 time gaps between consecutive (forward-filled) timestamps within
    // the analysed (possibly filtered) lines.
    let mut gaps: Vec<(i64, usize)> = Vec::new();
    match &vis {
        Some(v) => {
            for w in v[vis_a..vis_b].windows(2) {
                let (a, b) = (doc.ts_at(w[0]), doc.ts_at(w[1]));
                if a >= 0 && b > a {
                    gaps.push((b - a, w[1]));
                }
            }
        }
        None => {
            for i in (lo + 1)..hi {
                let (a, b) = (doc.ts_at(i - 1), doc.ts_at(i));
                if a >= 0 && b > a {
                    gaps.push((b - a, i));
                }
            }
        }
    }
    gaps.sort_by_key(|g| std::cmp::Reverse(g.0));
    let time_gaps: Vec<Value> = gaps
        .iter()
        .take(3)
        .map(|&(gap, i)| json!({ "line": i + 1, "time": fmt_ts(doc.ts_at(i)), "gap_ms": gap }))
        .collect();

    // Densest 60-second window within the analysed (possibly filtered) range.
    let densest = {
        let base = match &vis {
            Some(v) => time_range_of_lines(&doc, v[vis_a..vis_b].iter().copied()),
            None => range_time(&doc, lo, hi),
        };
        base.and_then(|(start, _)| {
            let mut buckets: HashMap<i64, usize> = HashMap::new();
            let mut tick = |i: usize| {
                let t = doc.ts_at(i);
                if t >= 0 {
                    *buckets.entry((t - start) / 60_000).or_default() += 1;
                }
            };
            match &vis {
                Some(v) => {
                    for &i in &v[vis_a..vis_b] {
                        tick(i);
                    }
                }
                None => {
                    for i in lo..hi {
                        tick(i);
                    }
                }
            }
            buckets.into_iter().max_by_key(|(_, c)| *c).map(|(b, c)| {
                json!({
                    "window_start": format_ms(start + b * 60_000),
                    "window_start_epoch_ms": start + b * 60_000,
                    "count": c,
                })
            })
        })
    };

    let mut out = state.doc_stats(&doc);
    out["lines"] = json!(m);
    out["time_range"] = match &vis {
        Some(v) => time_range_of_lines(&doc, v[vis_a..vis_b].iter().copied()),
        None => range_time(&doc, lo, hi),
    }
    .map(|(a, b)| {
        json!({
            "start": format_ms(a), "end": format_ms(b),
            "start_epoch_ms": a, "end_epoch_ms": b,
        })
    })
    .unwrap_or(Value::Null);
    // Keep template_count consistent with load_log/doc_stats: it describes the
    // indexed document. The number that actually contributed to this analysis
    // is reported separately, which avoids confusing 116-vs-115 results when
    // a range or filter excludes templates.
    out["template_count"] = json!(doc.templates.len());
    out["matched_template_count"] = json!(ranked.len());
    out["top_templates"] = json!(top_templates);
    out["window"] = json!({ "start_line": lo + 1, "end_line": hi });
    out["error_template_count"] = json!(err_tpls.len());
    out["error_line_count"] = json!(error_line_count);
    out["error_templates"] = json!(error_templates);
    out["largest_time_gaps"] = json!(time_gaps);
    out["densest_minute"] = densest.unwrap_or(Value::Null);
    // Budget estimates folded in from the former `log_size` tool.
    out["template_size_bytes"] = json!(serde_json::to_vec(&template_dict(&doc, None))
        .map(|v| v.len())
        .unwrap_or(0));
    out["sequence_estimate_bytes"] = json!(doc.total_lines().saturating_mul(SEQ_BYTES_PER_ENTRY));
    if log_id != "_active" {
        out["log_id"] = json!(log_id);
    }
    Ok(out)
}

/// get_template: resolve template ids to `{pattern, count, example_line}`.
/// Omit `ids` to get every template as a dictionary keyed by id.
fn tool_get_template(args: &Value, state: &mut ServerState) -> Result<Value, String> {
    let (log_id, doc) = resolve_doc(state, args)?;
    // Short-circuit: with_filtered_log=true (the default) and no filters set →
    // there is no filtered log to work on.
    let vis = match filtered_view_or_no_log(state, &log_id, args) {
        Ok(v) => v,
        Err(payload) => return Ok(payload),
    };
    let ids: Option<Vec<u32>> = match args.get("ids") {
        None | Some(Value::Null) => None,
        Some(Value::Array(arr)) => {
            let mut v = Vec::with_capacity(arr.len());
            for item in arr {
                match item {
                    Value::Number(n) => v.push(
                        n.as_u64()
                            .ok_or_else(|| "template id out of range".to_string())?
                            as u32,
                    ),
                    Value::String(s) => v.push(
                        s.parse::<u32>()
                            .map_err(|_| format!("invalid template id '{s}'"))?,
                    ),
                    _ => return Err("'ids' must be integers or numeric strings".to_string()),
                }
            }
            Some(v)
        }
        Some(_) => return Err("'ids' must be an array of template ids".to_string()),
    };
    let dict = match vis {
        Some(v) => {
            let counts = template_counts_of(&doc, v.iter().copied());
            template_dict_counts(&doc, ids.as_deref(), &counts)
        }
        None => template_dict(&doc, ids.as_deref()),
    };
    let mut out = json!({
        "template_count": ids.as_ref().map_or(doc.templates.len(), |v| v.len()),
        "templates": dict,
    });
    if log_id != "_active" {
        out["log_id"] = json!(log_id);
    }
    Ok(out)
}

/// log_sequence: dense `[[epoch_ms|null, line, template_id], …]` over a range
/// (line- or time-bounded). Optional `collapse` folds long same-template runs
/// into a single `{start_line, end_line, count, template_id, pattern}` entry.
/// Self-describes: `total_entries` vs `returned` plus a `truncated` flag tell
/// the caller to narrow `start`/`end` instead of a separate size call.
fn tool_log_sequence(args: &Value, state: &mut ServerState) -> Result<Value, String> {
    let (log_id, doc) = resolve_doc(state, args)?;
    // Short-circuit: with_filtered_log=true (the default) and no filters set →
    // there is no filtered log to work on.
    let vis = match filtered_view_or_no_log(state, &log_id, args) {
        Ok(v) => v,
        Err(payload) => return Ok(payload),
    };
    if doc.total_lines() == 0 {
        return Err("log is empty".to_string());
    }
    let (lo, hi) = resolve_range(args, &doc)?;
    let max_entries = arg_usize(args, "max_entries", HARD_MAX_SEQUENCE)?.min(HARD_MAX_SEQUENCE);
    let collapse = args
        .get("collapse")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    // Runs are built over the filtered lines only when a filtered view is set.
    let (items, covered, total_entries) = match &vis {
        Some(v) => {
            let a = v.partition_point(|&i| i < lo);
            let b = v.partition_point(|&i| i < hi);
            let slice = &v[a..b];
            let total = slice.len();
            let (items, covered) = if collapse {
                seq_runs_to_items(&doc, &build_runs_of(&doc, slice), max_entries)
            } else {
                let mut items = Vec::new();
                let mut covered = 0usize;
                for &i in slice {
                    if items.len() >= max_entries {
                        break;
                    }
                    items.push(seq_entry(&doc, i));
                    covered += 1;
                }
                (items, covered)
            };
            (items, covered, total)
        }
        None => {
            let total = hi - lo;
            let (items, covered) = if collapse {
                seq_runs_to_items(&doc, &build_runs(&doc, lo, hi), max_entries)
            } else {
                let mut items = Vec::new();
                let mut covered = 0usize;
                for i in lo..hi {
                    if items.len() >= max_entries {
                        break;
                    }
                    items.push(seq_entry(&doc, i));
                    covered += 1;
                }
                (items, covered)
            };
            (items, covered, total)
        }
    };
    let truncated = covered < total_entries;
    let size_bytes = serde_json::to_vec(&Value::Array(items.clone()))
        .map(|v| v.len())
        .unwrap_or(0);

    let mut out = json!({
        "start_line": lo + 1,
        "end_line": hi,
        "total_entries": total_entries,
        "returned": covered,
        "truncated": truncated,
        "size_bytes": size_bytes,
        "collapsed": collapse,
        "sequence": items,
    });
    if log_id != "_active" {
        out["log_id"] = json!(log_id);
    }
    Ok(out)
}

/// raw_log: raw log lines over a range bounded by line numbers or timestamps.
fn tool_raw_log(args: &Value, state: &mut ServerState) -> Result<Value, String> {
    let (log_id, doc) = resolve_doc(state, args)?;
    // Short-circuit: with_filtered_log=true (the default) and no filters set →
    // there is no filtered log to work on.
    let vis = match filtered_view_or_no_log(state, &log_id, args) {
        Ok(v) => v,
        Err(payload) => return Ok(payload),
    };
    if doc.total_lines() == 0 {
        return Err("log is empty".to_string());
    }
    let lo = resolve_bound(args.get("start"), &doc, false)?
        .ok_or_else(|| "argument 'start' is required".to_string())?;
    let end = resolve_bound(args.get("end"), &doc, true)?
        .ok_or_else(|| "argument 'end' is required".to_string())?;
    if end < lo {
        return Err("end is before start".to_string());
    }
    let hi = (end + 1).min(doc.total_lines());
    let max_lines = arg_usize(args, "max_lines", HARD_MAX_LINES)?.min(HARD_MAX_LINES);
    // `total` counts only the filtered lines in the window when a filtered
    // view is active.
    let (total, lines): (usize, Vec<Value>) = match &vis {
        Some(v) => {
            let a = v.partition_point(|&i| i < lo);
            let b = v.partition_point(|&i| i < hi);
            let slice = &v[a..b];
            let total = slice.len();
            let mut lines = Vec::new();
            for &i in slice {
                if lines.len() >= max_lines {
                    break;
                }
                lines.push(Value::String(truncate(&doc.line(i), MAX_LINE_TEXT)));
            }
            (total, lines)
        }
        None => {
            let total = hi - lo;
            let mut lines = Vec::new();
            for i in lo..hi {
                if lines.len() >= max_lines {
                    break;
                }
                lines.push(Value::String(truncate(&doc.line(i), MAX_LINE_TEXT)));
            }
            (total, lines)
        }
    };
    let truncated = lines.len() < total;
    let mut out = json!({
        "start_line": lo + 1,
        "end_line": hi,
        "total": total,
        "returned": lines.len(),
        "truncated": truncated,
        "lines": lines,
    });
    if log_id != "_active" {
        out["log_id"] = json!(log_id);
    }
    Ok(out)
}

/// trim (GUI mode only): focus the active document's visible window to a
/// line- or time-bounded range. Round-trips through `set_active_doc` so the
/// GUI view and the `_active` match cache stay in sync.
fn tool_trim_gui(args: &Value, state: &mut ServerState) -> Result<Value, String> {
    let doc = state.get_active_doc()?;
    let full = doc.total_lines_untrimmed();
    let start = resolve_bound(args.get("start"), &doc, false)?;
    let end = resolve_bound(args.get("end"), &doc, true)?;
    let (s_abs, e_abs) = match (start, end) {
        (None, None) => (0usize, full.saturating_sub(1)),
        (Some(s), Some(e)) => (doc.trim_start + s, doc.trim_start + e),
        (Some(s), None) => (doc.trim_start + s, full.saturating_sub(1)),
        (None, Some(e)) => (0usize, doc.trim_start + e),
    };
    if e_abs < s_abs {
        return Err("end is before start".to_string());
    }
    let mut new_doc = doc;
    Arc::make_mut(&mut new_doc).trim_range(s_abs, e_abs);
    state.set_active_doc(new_doc);
    Ok(json!({ "remaining_lines": state.get_active_doc()?.total_lines() }))
}

/// get_timeline_histogram: tiny [{x, count}] distribution for the whole log,
/// a keyword, or a template. Time domain when timestamps exist, else line
/// index. Lets the caller find the spike/window before fetching any lines.
fn tool_timeline_histogram(args: &Value, state: &mut ServerState) -> Result<Value, String> {
    let (log_id, doc) = resolve_doc(state, args)?;
    // Short-circuit: with_filtered_log=true (the default) and no filters set →
    // there is no filtered log to work on.
    let vis = match filtered_view_or_no_log(state, &log_id, args) {
        Ok(v) => v,
        Err(payload) => return Ok(payload),
    };
    let n = doc.total_lines();
    if n == 0 {
        return Err("log is empty".to_string());
    }
    let (lo, hi) = resolve_range(args, &doc)?;
    let nb = arg_usize(args, "buckets", 50)?.clamp(16, 1024);
    let keyword = args.get("keyword").and_then(Value::as_str);
    let template_id = args
        .get("template_id")
        .and_then(Value::as_u64)
        .map(|v| v as u32);
    if keyword.is_some() && template_id.is_some() {
        return Err("pass either 'keyword' or 'template_id', not both".to_string());
    }
    // The time-domain decision is then made from the filtered lines when a
    // filtered view is active.
    let tr = match &vis {
        Some(v) => {
            let a = v.partition_point(|&i| i < lo);
            let b = v.partition_point(|&i| i < hi);
            time_range_of_lines(&doc, v[a..b].iter().copied())
        }
        None => range_time(&doc, lo, hi),
    };
    let time_domain = matches!(tr, Some((a, b)) if b > a);
    let (ta, tb) = tr.unwrap_or((0, 0));
    let span = (tb - ta).max(1);
    let x_of = |i: usize| -> i64 {
        if time_domain {
            doc.ts_at(i)
        } else {
            i as i64
        }
    };
    let bucket_of = |v: i64| -> usize {
        if time_domain {
            ((v - ta).clamp(0, span) * (nb as i64 - 1) / span) as usize
        } else {
            ((v - lo as i64).clamp(0, (hi - lo) as i64 - 1) * (nb as i64 - 1)
                / ((hi - lo) as i64 - 1).max(1)) as usize
        }
    };

    let matches = match keyword {
        Some(kw) => Some(state.matches_for(&log_id, kw, true)?),
        None => None,
    };
    let mut counts = vec![0u64; nb];
    // `contains_visible` checks membership in the filtered view (already
    // range-checked by the caller's loop bounds).
    let filtered = vis.as_deref();
    match (&matches, template_id) {
        (Some(m), _) => {
            for &i in m.iter() {
                if i < lo || i >= hi {
                    continue;
                }
                if let Some(v) = filtered {
                    if v.binary_search(&i).is_err() {
                        continue;
                    }
                }
                let v = x_of(i);
                if v >= 0 {
                    counts[bucket_of(v)] += 1;
                }
            }
        }
        (None, Some(tid)) => {
            if let Some(v) = filtered {
                let a = v.partition_point(|&i| i < lo);
                let b = v.partition_point(|&i| i < hi);
                for &i in &v[a..b] {
                    if doc.template_at(i) == tid {
                        let x = x_of(i);
                        if x >= 0 {
                            counts[bucket_of(x)] += 1;
                        }
                    }
                }
            } else {
                for i in lo..hi {
                    if doc.template_at(i) == tid {
                        let x = x_of(i);
                        if x >= 0 {
                            counts[bucket_of(x)] += 1;
                        }
                    }
                }
            }
        }
        (None, None) => {
            if let Some(v) = filtered {
                let a = v.partition_point(|&i| i < lo);
                let b = v.partition_point(|&i| i < hi);
                for &i in &v[a..b] {
                    let x = x_of(i);
                    if x >= 0 {
                        counts[bucket_of(x)] += 1;
                    }
                }
            } else {
                for i in lo..hi {
                    let x = x_of(i);
                    if x >= 0 {
                        counts[bucket_of(x)] += 1;
                    }
                }
            }
        }
    }

    let xs: Vec<i64> = (0..nb)
        .map(|i| {
            if time_domain {
                ta + span * i as i64 / (nb as i64 - 1).max(1)
            } else {
                lo as i64 + ((hi - lo) as i64 - 1).max(1) * i as i64 / (nb as i64 - 1).max(1)
            }
        })
        .collect();
    let total: u64 = counts.iter().sum();
    let mut out = json!({
        "domain": if time_domain { "time" } else { "sequence" },
        "x_unit": if time_domain { "epoch_ms" } else { "line_index" },
        "buckets": nb,
        "x": xs,
        "counts": counts,
        "total": total,
    });
    if let Some(kw) = keyword {
        out["keyword"] = json!(kw);
    }
    if let Some(tid) = template_id {
        out["template_id"] = json!(tid);
    }
    if log_id != "_active" {
        out["log_id"] = json!(log_id);
    }
    Ok(out)
}

/// get_template_anomalies: rare, first-seen-late, and bursty templates —
/// the "what is unusual in this log" question.
fn tool_template_anomalies(args: &Value, state: &mut ServerState) -> Result<Value, String> {
    let (log_id, doc) = resolve_doc(state, args)?;
    // Short-circuit: with_filtered_log=true (the default) and no filters set →
    // there is no filtered log to work on.
    let vis = match filtered_view_or_no_log(state, &log_id, args) {
        Ok(v) => v,
        Err(payload) => return Ok(payload),
    };
    let n = doc.total_lines();
    if n == 0 {
        return Err("log is empty".to_string());
    }
    let (lo, hi) = resolve_range(args, &doc)?;
    let limit = arg_usize(args, "limit", 50)?;

    // The analysis window below spans only the filtered lines.
    let (vis_a, vis_b) = match &vis {
        Some(v) => (
            v.partition_point(|&i| i < lo),
            v.partition_point(|&i| i < hi),
        ),
        None => (0, 0),
    };
    let m = match &vis {
        Some(_) => vis_b - vis_a,
        None => hi - lo,
    };
    if m == 0 {
        return Ok(json!({
            "total_lines": 0,
            "template_count": 0,
            "anomaly_count": 0,
            "anomalies": [],
        }));
    }

    // Per-template occurrence counts within the analysed (possibly filtered)
    // range, used for rare/bursty thresholds instead of the full-window count.
    let counts = match &vis {
        Some(v) => template_counts_of(&doc, v[vis_a..vis_b].iter().copied()),
        None => template_counts_in_range(&doc, lo, hi),
    };

    // Single pass: first-seen line (absolute index) per template +
    // per-template line-index histograms (only for templates frequent enough
    // to burst). Buckets are physical positions within `[lo, hi)`.
    let mut first_seen: HashMap<u32, usize> = HashMap::new();
    let mut hists: HashMap<u32, Vec<u32>> = doc
        .templates
        .iter()
        .filter(|t| counts.get(&t.id).copied().unwrap_or(0) >= 10)
        .map(|t| (t.id, vec![0u32; ANOMALY_BUCKETS]))
        .collect();
    let range_len = (hi - lo).max(1);
    match &vis {
        Some(v) => {
            for &i in &v[vis_a..vis_b] {
                let tid = doc.template_at(i);
                first_seen.entry(tid).or_insert(i);
                if let Some(h) = hists.get_mut(&tid) {
                    let b = ((i - lo) * ANOMALY_BUCKETS / range_len).min(ANOMALY_BUCKETS - 1);
                    h[b] += 1;
                }
            }
        }
        None => {
            for i in lo..hi {
                let tid = doc.template_at(i);
                first_seen.entry(tid).or_insert(i);
                if let Some(h) = hists.get_mut(&tid) {
                    let b = ((i - lo) * ANOMALY_BUCKETS / range_len).min(ANOMALY_BUCKETS - 1);
                    h[b] += 1;
                }
            }
        }
    }

    let rare_max = (m / 1000).max(1); // ≤0.1% of lines (or exactly 1)
    let late_from = m * 9 / 10; // first seen in the last 10% of the range
    let mut anomalies: Vec<Value> = Vec::new();
    for t in &doc.templates {
        let count = counts.get(&t.id).copied().unwrap_or(0);
        if count == 0 {
            continue;
        }
        let mut reasons = Vec::new();
        if count <= rare_max {
            reasons.push("rare");
        }
        let first = first_seen.get(&t.id).copied().unwrap_or(0);
        if first >= late_from {
            reasons.push("first_seen_late");
        }
        if let Some(h) = hists.get(&t.id) {
            let peak = h.iter().copied().max().unwrap_or(0) as usize;
            if peak * 2 >= count {
                reasons.push("bursty"); // ≥50% of hits inside 1/50th of the range
            }
        }
        if reasons.is_empty() {
            continue;
        }
        anomalies.push(json!({
            "template_id": t.id,
            "pattern": truncate(&t.pattern, SAMPLE_CHARS),
            "count": count,
            "reasons": reasons,
            "first_line": first + 1,
            "first_time": fmt_ts(doc.ts_at(first)),
        }));
    }
    // Most interesting first: multi-reason, then rarest.
    anomalies.sort_by(|a, b| {
        let ra = a["reasons"].as_array().map_or(0, |r| r.len());
        let rb = b["reasons"].as_array().map_or(0, |r| r.len());
        rb.cmp(&ra)
            .then(a["count"].as_u64().cmp(&b["count"].as_u64()))
    });
    anomalies.truncate(limit);

    let mut out = json!({
        "total_lines": m,
        "template_count": counts.values().filter(|&&c| c > 0).count(),
        "anomaly_count": anomalies.len(),
        "anomalies": anomalies,
    });
    if log_id != "_active" {
        out["log_id"] = json!(log_id);
    }
    Ok(out)
}

/// get_template_samples: a few concrete lines for a template without dumping
/// all matches. Pairs with get_templates / get_template_anomalies.
fn tool_template_samples(args: &Value, state: &mut ServerState) -> Result<Value, String> {
    let (log_id, doc) = resolve_doc(state, args)?;
    // Short-circuit: with_filtered_log=true (the default) and no filters set →
    // there is no filtered log to work on.
    let vis = match filtered_view_or_no_log(state, &log_id, args) {
        Ok(v) => v,
        Err(payload) => return Ok(payload),
    };
    let tid = args
        .get("template_id")
        .and_then(Value::as_u64)
        .ok_or_else(|| "missing or invalid argument 'template_id' (integer expected)".to_string())?
        as u32;
    let n_samples = arg_usize(args, "n", 3)?.clamp(1, 10);
    let strategy = args
        .get("strategy")
        .and_then(Value::as_str)
        .unwrap_or("first_last_random");

    let idx: Vec<usize> = match vis {
        // Only sample lines that survive the current filters.
        Some(v) => v
            .iter()
            .copied()
            .filter(|&i| doc.template_at(i) == tid)
            .collect(),
        None => (0..doc.total_lines())
            .filter(|&i| doc.template_at(i) == tid)
            .collect(),
    };
    if idx.is_empty() {
        return Err(format!(
            "template_id {tid} has no matching lines in the current view (see get_template)"
        ));
    }
    let len = idx.len();
    let picks: Vec<usize> = if len <= n_samples {
        idx.clone()
    } else {
        match strategy {
            "first" => idx[..n_samples].to_vec(),
            "even" => {
                if n_samples == 1 {
                    vec![idx[0]]
                } else {
                    (0..n_samples)
                        .map(|k| idx[k * (len - 1) / (n_samples - 1)])
                        .collect()
                }
            }
            "first_last_random" => {
                if n_samples == 1 {
                    vec![idx[0]]
                } else {
                    // Deterministic pseudo-random picks (seeded by match count)
                    // so repeated calls return the same samples.
                    let mut picks: Vec<usize> = vec![idx[0], idx[len - 1]];
                    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15 ^ (len as u64);
                    let mut attempts = 0;
                    while picks.len() < n_samples && attempts < 100 {
                        seed = seed
                            .wrapping_mul(6364136223846793005)
                            .wrapping_add(1442695040888963407);
                        let p = idx[((seed >> 33) as usize) % len];
                        if !picks.contains(&p) {
                            picks.push(p);
                        }
                        attempts += 1;
                    }
                    // Fallback: fill any missing slots with even spacing.
                    let mut k = 0;
                    while picks.len() < n_samples {
                        let p = idx[k * (len - 1) / (n_samples - 1)];
                        if !picks.contains(&p) {
                            picks.push(p);
                        }
                        k += 1;
                    }
                    picks.sort_unstable();
                    picks
                }
            }
            other => {
                return Err(format!(
                    "unknown strategy '{other}' (expected first_last_random, first, or even)"
                ))
            }
        }
    };
    let samples: Vec<Value> = picks.iter().map(|&i| line_payload(&doc, i)).collect();
    let mut out = json!({
        "template_id": tid,
        "pattern": template_pattern(&doc, tid),
        "total_matches": len,
        "strategy": strategy,
        "samples": samples,
    });
    if log_id != "_active" {
        out["log_id"] = json!(log_id);
    }
    Ok(out)
}

// -------------------------------------------------------------- tool schema

fn tools(_state: &ServerState) -> Value {
    let mut tools = unified_tools();
    if let Some(list) = tools.as_array_mut() {
        list.insert(
            0,
            json!({
                "name": "session_info",
                "description": "Inspect the current target before analysis. Reports standalone mode and loaded-log count, or a temporary GUI attachment with its active log, filters, and lifecycle. Never returns the session ID.",
                "inputSchema": { "type": "object", "properties": {} }
            }),
        );
        for tool in list {
            let name = tool.get("name").and_then(Value::as_str).unwrap_or("");
            let read_only = !matches!(
                name,
                "load_log"
                    | "close_log"
                    | "filters_add"
                    | "filters_remove"
                    | "add_analysis"
                    | "trim"
                    | "attach_gui_session"
                    | "detach_gui_session"
            );
            let idempotent = matches!(
                name,
                "list_logs"
                    | "filters_get"
                    | "summarize_log"
                    | "get_timeline_histogram"
                    | "get_template"
                    | "get_template_samples"
                    | "log_sequence"
                    | "raw_log"
                    | "find_occurrences"
                    | "get_analysis"
                    | "session_info"
                    | "attach_gui_session"
                    | "detach_gui_session"
            );
            tool["annotations"] = json!({
                "readOnlyHint": read_only,
                "destructiveHint": matches!(name, "close_log" | "trim"),
                "idempotentHint": idempotent,
                "openWorldHint": false
            });
            tool["outputSchema"] = json!({
                "type": "object",
                "additionalProperties": true
            });
        }
    }
    tools
}

/// Stable catalog advertised by the public stdio server. MCP clients commonly
/// cache `tools/list`, so attaching to or detaching from a GUI must not change
/// tool names or schemas. Runtime mode checks remain authoritative.
fn unified_tools() -> Value {
    let mut tools = headless_tools();
    let list = tools.as_array_mut().unwrap();

    list.insert(
        0,
        json!({
            "name": "detach_gui_session",
            "description": "Detach this stdio MCP process from the temporary Logotomy GUI session and return to its preserved standalone state.",
            "inputSchema": { "type": "object", "properties": {} }
        }),
    );
    list.insert(
        0,
        json!({
            "name": "attach_gui_session",
            "description": "Attach this stdio MCP process to the log currently selected in the Logotomy GUI using the temporary 12-character hexadecimal session ID copied from the GUI. The ID is never persisted. While attached, omit log_id and do not call load_log.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {
                        "type": "string",
                        "description": "Temporary 12-character hexadecimal GUI session ID copied from Logotomy"
                    }
                },
                "required": ["session_id"]
            }
        }),
    );

    // Analysis and filter tools use log_id only in standalone mode. Keep the
    // property discoverable but make it conditionally required at runtime.
    for tool in list.iter_mut() {
        let name = tool.get("name").and_then(Value::as_str).unwrap_or("");
        if !matches!(name, "load_log" | "list_logs" | "close_log") {
            if let Some(required) = tool["inputSchema"]["required"].as_array_mut() {
                required.retain(|item| item.as_str() != Some("log_id"));
            }
            if tool["inputSchema"]["properties"]["log_id"].is_object() {
                tool["inputSchema"]["properties"]["log_id"]["description"] = json!(
                    "Required in standalone mode (ID returned by load_log); omit while attached to a GUI session"
                );
            }
        }
    }

    let trim = gui_tools()
        .as_array()
        .and_then(|items| items.iter().find(|tool| tool["name"] == "trim"))
        .cloned()
        .expect("GUI tool catalog must contain trim");
    list.push(trim);
    list.extend(gui_analysis_tools());
    tools
}

/// GUI-attached-only annotation tools. They are part of the public catalog so
/// clients that cache `tools/list` still discover them after attaching.
fn gui_analysis_tools() -> Vec<Value> {
    vec![
        json!({
            "name": "get_analysis",
            "description": "Read the user-visible analysis cards from the active GUI Pin tab. Available only while attached to a GUI session. Call this before beginning GUI log analysis so existing user notes are considered.",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "add_analysis",
            "description": "Add a user-visible analysis card to the active GUI Pin tab. `lines` is an optional list of 1-based line numbers; omit it or pass [] for a text-only card shown first in the Pin tab. Available only while attached to a GUI session.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "Analysis text shown to the user (required, non-empty)" },
                    "lines": { "type": "array", "items": { "type": "integer", "minimum": 1 }, "description": "Optional 1-based log line numbers; duplicates are normalized" }
                },
                "required": ["text"]
            }
        }),
    ]
}

/// Schemas for the compression-first tools. `log_id_prop` is `Some` in
/// headless mode (every tool takes a log_id) and `None` in GUI mode.
fn compression_tool_schemas(log_id_prop: Option<Value>) -> Vec<Value> {
    // Wrap a properties object + required list, injecting log_id in headless mode.
    let schema = |props: Value, mut required: Vec<&str>| -> Value {
        if let Some(id) = &log_id_prop {
            let mut p = props;
            p["log_id"] = id.clone();
            required.insert(0, "log_id");
            json!({ "type": "object", "properties": p, "required": required })
        } else {
            json!({ "type": "object", "properties": props, "required": required })
        }
    };
    let bound_prop = |desc: &str| {
        json!({
            "type": ["integer", "string"],
            "description": format!("{desc} — a 1-based line number (integer) or a time (string: RFC3339, 'YYYY-MM-DD HH:MM:SS', or epoch millis)")
        })
    };
    let filter_prop = json!({
        "type": "boolean",
        "description": "When true (default), operate on the filtered log — only lines matching the current filter set (the union of filters_get matches; the 'Everything Else' lane is never included). With zero filters the tool replies {comment: 'no log'} instead of scanning — set false for the full log or add a filter first (filters_add)."
    });
    vec![
        json!({
            "name": "summarize_log",
            "description": "One-call orientation (~200 tokens) over an optional line/time range: stats, time range, top-5 templates, error-ish templates (ERROR/FATAL/Exception/panic), the 3 largest time gaps, the densest minute, plus byte-size budget estimates (template_size_bytes, sequence_estimate_bytes). Respects the filtered log set (with_filtered_log=true, the default). Start here when investigating a log.",
            "inputSchema": schema(json!({
                "start": bound_prop("Range start"),
                "end": bound_prop("Range end"),
                "with_filtered_log": filter_prop.clone()
            }), vec![]),
        }),
        json!({
            "name": "get_timeline_histogram",
            "description": "Tiny distribution histogram ({x, counts}) over an optional line/time range, for the whole log, a keyword, or a template. Time domain (epoch ms) when the log has timestamps, else line-index domain. Use this to find the spike/window BEFORE fetching any lines.",
            "inputSchema": schema(json!({
                "start": bound_prop("Range start"),
                "end": bound_prop("Range end"),
                "buckets": { "type": "integer", "description": "Number of buckets (default 50, 16-1024)" },
                "keyword": { "type": "string", "description": "Restrict to lines containing this exact phrase (case-sensitive)" },
                "template_id": { "type": "integer", "description": "Restrict to lines of this template (see get_template)" },
                "with_filtered_log": filter_prop.clone()
            }), vec![]),
        }),
        json!({
            "name": "get_template_anomalies",
            "description": "What is unusual in this log (over an optional line/time range): rare templates (<=0.1% of lines), first-seen-late templates (new pattern appearing in the last 10% of the range), and bursty templates (most occurrences packed into a tiny window).",
            "inputSchema": schema(json!({
                "start": bound_prop("Range start"),
                "end": bound_prop("Range end"),
                "limit": { "type": "integer", "description": "Max anomalies to return (default 50)" },
                "with_filtered_log": filter_prop.clone()
            }), vec![]),
        }),
        json!({
            "name": "get_template",
            "description": "Resolve template ids to {pattern, count, example_line}. Omit 'ids' to get every template as a dictionary keyed by id. With with_filtered_log=true, 'count' reflects the filtered set.",
            "inputSchema": schema(json!({
                "ids": { "type": "array", "items": { "type": ["integer", "string"] }, "description": "Template ids to fetch (omit for all)" },
                "with_filtered_log": filter_prop.clone()
            }), vec![]),
        }),
        json!({
            "name": "get_template_samples",
            "description": "A few concrete example lines for a template without dumping all matches. Pairs with get_template / get_template_anomalies. With with_filtered_log=true, samples are drawn from the filtered set only.",
            "inputSchema": schema(json!({
                "template_id": { "type": "integer", "description": "Template ID from get_template (required)" },
                "n": { "type": "integer", "description": "Number of samples (default 3, max 10)" },
                "strategy": { "type": "string", "enum": ["first_last_random", "first", "even"], "description": "Sample picking strategy (default first_last_random, deterministic)" },
                "with_filtered_log": filter_prop.clone()
            }), vec!["template_id"]),
        }),
        json!({
            "name": "log_sequence",
            "description": "Dense [[epoch_ms|null, line, template_id], ...] over a line- or time-bounded range (the filtered set when with_filtered_log=true). Optional 'collapse' folds long same-template runs into one entry. Returns total_entries/returned/truncated so the caller can narrow start/end when truncated.",
            "inputSchema": schema(json!({
                "start": bound_prop("Range start"),
                "end": bound_prop("Range end"),
                "max_entries": { "type": "integer", "description": "Max entries to return (default 2000, hard cap 2000)" },
                "collapse": { "type": "boolean", "description": "Collapse same-template runs (default false)" },
                "with_filtered_log": filter_prop.clone()
            }), vec![]),
        }),
        json!({
            "name": "raw_log",
            "description": "Raw log lines over a range bounded by line numbers or timestamps (restricted to the filtered set when with_filtered_log=true). Truncated to max_lines with a 'truncated' flag.",
            "inputSchema": schema(json!({
                "start": bound_prop("Range start (required)"),
                "end": bound_prop("Range end (required)"),
                "max_lines": { "type": "integer", "description": "Max lines to return (default 2000, hard cap 2000)" },
                "with_filtered_log": filter_prop.clone()
            }), vec!["start", "end"]),
        }),
        json!({
            "name": "filters_get",
            "description": "List the log's current filter set (keyword terms) as [{id, filter_text}]. Other tools running with with_filtered_log=true (the default) restrict their results to the union of these filters' matches; the 'Everything Else' lane is never included.",
            "inputSchema": schema(json!({}), vec![]),
        }),
        json!({
            "name": "filters_add",
            "description": "Add a keyword filter to the log's filter set. Returns the full updated [{id, filter_text}] list. Enforces the 20-filter cap and case-sensitive dedupe.",
            "inputSchema": schema(json!({
                "filter_text": { "type": "string", "description": "Exact phrase to match (case-sensitive)" }
            }), vec!["filter_text"]),
        }),
        json!({
            "name": "filters_remove",
            "description": "Remove a filter by its id (position from filters_get). Returns the full updated [{id, filter_text}] list.",
            "inputSchema": schema(json!({
                "id": { "type": "integer", "description": "Filter id (position) to remove" }
            }), vec!["id"]),
        }),
    ]
}

fn headless_tools() -> Value {
    let time_prop = |desc: &str| {
        json!({
            "type": "string",
            "description": format!("{desc} (RFC3339, 'YYYY-MM-DD HH:MM:SS', 'YYYY-MM-DD', or epoch millis)")
        })
    };
    let log_id_prop = json!({ "type": "string", "description": "ID returned by load_log" });
    let keyword_prop = json!({ "type": "string", "description": "Exact phrase to search for" });
    let filter_prop = json!({
        "type": "boolean",
        "description": "When true (default), operate on the filtered log — only lines matching the current filter set (the union of filters_get matches; the 'Everything Else' lane is never included). With zero filters the tool replies {comment: 'no log'} instead of scanning — set false for the full log or add a filter first (filters_add)."
    });

    let base = json!([
        {
            "name": "load_log",
            "description": "Load and index a log file (any text file). Returns a log_id used by all other tools, plus stats (lines, time range, top templates).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute path to the log file" }
                },
                "required": ["path"]
            }
        },
        {
            "name": "list_logs",
            "description": "List all currently loaded log files with their stats.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "close_log",
            "description": "Unload a log file and free its memory.",
            "inputSchema": {
                "type": "object",
                "properties": { "log_id": log_id_prop },
                "required": ["log_id"]
            }
        },
        {
            "name": "find_occurrences",
            "description": "Find log lines containing this exact phrase. Returns paginated [one_based_line_number, epoch_ms|null] tuples only. Set case_sensitive=false for ASCII case-insensitive matching; with_filtered_log restricts results to the active filter union.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "log_id": log_id_prop,
                    "keyword": keyword_prop,
                    "after": time_prop("Only include occurrences at or after this time"),
                    "before": time_prop("Only include occurrences at or before this time"),
                    "max_results": { "type": "integer", "description": "Max results to return (default 50, max 2000)" },
                    "offset": { "type": "integer", "description": "Skip this many matches (default 0)" },
                    "case_sensitive": { "type": "boolean", "description": "Match ASCII letter case exactly (default true); false folds ASCII case" },
                    "with_filtered_log": filter_prop.clone()
                },
                "required": ["log_id", "keyword"]
            }
        }
    ]);
    let mut list = base;
    list.as_array_mut()
        .unwrap()
        .extend(compression_tool_schemas(Some(log_id_prop)));
    list
}

fn gui_tools() -> Value {
    let time_prop = |desc: &str| {
        json!({
            "type": "string",
            "description": format!("{desc} (RFC3339, 'YYYY-MM-DD HH:MM:SS', 'YYYY-MM-DD', or epoch millis)")
        })
    };
    let keyword_prop = json!({ "type": "string", "description": "Exact phrase to search for" });
    let filter_prop = json!({
        "type": "boolean",
        "description": "When true (default), operate on the filtered log — only lines matching the current filter set (the union of filters_get matches; the 'Everything Else' lane is never included). With zero filters the tool replies {comment: 'no log'} instead of scanning — set false for the full log or add a filter first (filters_add)."
    });

    let base = json!([
        {
            "name": "find_occurrences",
            "description": "Find log lines containing this exact phrase in the currently open log. Returns paginated [one_based_line_number, epoch_ms|null] tuples only. Set case_sensitive=false for ASCII case-insensitive matching; with_filtered_log restricts results to the active filter union.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "keyword": keyword_prop,
                    "after": time_prop("Only include occurrences at or after this time"),
                    "before": time_prop("Only include occurrences at or before this time"),
                    "max_results": { "type": "integer", "description": "Max results to return (default 50, max 2000)" },
                    "offset": { "type": "integer", "description": "Skip this many matches (default 0)" },
                    "case_sensitive": { "type": "boolean", "description": "Match ASCII letter case exactly (default true); false folds ASCII case" },
                    "with_filtered_log": filter_prop.clone()
                },
                "required": ["keyword"]
            }
        },
        {
            "name": "trim",
            "description": "Focus the currently open log's visible window to a line- or time-bounded range (both bounds optional; omit both to reset to the full file). Returns the remaining visible line count.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "start": { "type": ["integer", "string"], "description": "Range start — a 1-based line number (integer) or a time (string: RFC3339, 'YYYY-MM-DD HH:MM:SS', or epoch millis)" },
                    "end": { "type": ["integer", "string"], "description": "Range end — a 1-based line number (integer) or a time (string: RFC3339, 'YYYY-MM-DD HH:MM:SS', or epoch millis)" }
                }
            }
        }
    ]);
    let mut list = base;
    list.as_array_mut()
        .unwrap()
        .extend(compression_tool_schemas(None));
    list
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::timeline::Timeline;
    use std::io::Write;

    #[test]
    fn initialize_instructions_are_stable_across_runtime_modes() {
        let headless = ServerState::default();
        let result = initialize_result(&json!({"protocolVersion": "2025-06-18"}), &headless);
        let headless_instructions = result["instructions"].as_str().unwrap();
        assert!(headless_instructions.contains("starts in standalone mode"));
        assert!(headless_instructions.contains("call load_log"));
        assert!(headless_instructions.contains("attach_gui_session"));

        let mut gui = ServerState::default();
        gui.enable_gui_mode();
        let result = initialize_result(&json!({"protocolVersion": "2025-06-18"}), &gui);
        let gui_instructions = result["instructions"].as_str().unwrap();
        assert_eq!(headless_instructions, gui_instructions);
        assert!(gui_instructions.contains("list_logs, and close_log are unavailable"));
        assert!(gui_instructions.contains("Suggested approach"));
        assert!(gui_instructions.contains("with_filtered_log=false"));
        assert!(gui_instructions.contains("targeted find_occurrences"));
        assert!(gui_instructions.contains("filters (filters_add) and trim"));
        assert!(gui_instructions.contains("root-cause"));
    }

    #[test]
    fn modern_discovery_advertises_dual_era_versions_and_tools() {
        let mut state = ServerState::default();
        let params = json!({
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": MODERN_PROTOCOL_VERSION,
                "io.modelcontextprotocol/clientInfo": {"name": "test", "version": "1"},
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        });
        let response = protocol_response(json!(1), "server/discover", &params, &mut state);
        let response: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["result"]["resultType"], "complete");
        assert_eq!(
            response["result"]["supportedVersions"][0],
            MODERN_PROTOCOL_VERSION
        );
        assert!(response["result"]["capabilities"]["tools"].is_object());
        assert!(response["result"]["capabilities"]["resources"].is_object());
        assert_eq!(response["result"]["cacheScope"], "private");
    }

    #[test]
    fn session_info_and_resources_describe_runtime_mode() {
        let mut headless = ServerState::default();
        let info = dispatch("session_info", &json!({}), &mut headless).unwrap();
        assert_eq!(info["mode"], "standalone");
        assert_eq!(info["gui_attached"], false);
        assert_eq!(info["temporary"], false);

        let mut gui = ServerState::default();
        gui.enable_gui_mode();
        let info = dispatch("session_info", &json!({}), &mut gui).unwrap();
        assert_eq!(info["mode"], "gui_attached");
        assert_eq!(info["gui_attached"], true);
        assert_eq!(info["temporary"], true);
        assert_eq!(info["active_log_can_change"], true);

        let listed = resources_list();
        assert_eq!(listed["resources"].as_array().unwrap().len(), 2);
        let session = resource_read(&json!({"uri": "logotomy://session"}), &gui).unwrap();
        let text = session["contents"][0]["text"].as_str().unwrap();
        assert!(text.contains("\"temporary\": true"));
        let guide = resource_read(&json!({"uri": "logotomy://guide"}), &gui).unwrap();
        let guide = guide["contents"][0]["text"].as_str().unwrap();
        assert!(guide.contains("get_analysis"));
        assert!(guide.contains("not a required checklist"));
        assert!(guide.contains("find_occurrences"));
        assert!(guide.contains("trim"));
        assert!(guide.contains("root-cause"));
    }

    #[test]
    fn unified_stdio_attaches_routes_detaches_and_preserves_standalone_state() {
        use std::sync::mpsc;

        let gui_state = Arc::new(Mutex::new(gui_state(&repetitive_log())));
        let shutdown = Arc::new(AtomicBool::new(false));
        let (bound_tx, bound_rx) = mpsc::channel::<Result<u16, String>>();
        let session_id = session::generate_session_id();
        let server_session_id = session_id.clone();
        let stop = Arc::clone(&shutdown);
        let server = std::thread::spawn(move || {
            let _ = run_gui_ipc(0, gui_state, stop, Some(bound_tx), server_session_id);
        });
        let port = bound_rx.recv().unwrap().unwrap();
        let client = GuiClient::from_session(session::GuiSession {
            pid: std::process::id(),
            port,
            session_id: session_id.clone(),
            started_at_epoch_ms: 0,
        });
        let expired_client = client.clone();

        let standalone = Arc::new(Mutex::new(ServerState::default()));
        let path = write_temp("2026-07-19T10:00:00Z standalone\n");
        standalone
            .lock()
            .unwrap()
            .add_doc(LogDocument::open(&path).unwrap());
        std::fs::remove_file(path).ok();

        let mut attachment = None;
        let attached = attach_stdio_session(
            json!(1),
            &json!({"session_id": session_id}),
            &mut attachment,
            |_| Ok(client),
        );
        let attached: Value = serde_json::from_str(&attached).unwrap();
        assert_eq!(
            attached["result"]["structuredContent"]["mode"],
            "gui_attached"
        );
        assert!(attachment.is_some());

        let info_message = json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "session_info", "arguments": {}}
        });
        let info = route_stdio_request(
            json!(2),
            "tools/call",
            &info_message["params"],
            &info_message,
            &standalone,
            &mut attachment,
        );
        let info: Value = serde_json::from_str(&info).unwrap();
        assert_eq!(info["result"]["structuredContent"]["mode"], "gui_attached");

        let detach_message = json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": "detach_gui_session", "arguments": {}}
        });
        let detached = route_stdio_request(
            json!(3),
            "tools/call",
            &detach_message["params"],
            &detach_message,
            &standalone,
            &mut attachment,
        );
        let detached: Value = serde_json::from_str(&detached).unwrap();
        assert_eq!(
            detached["result"]["structuredContent"]["mode"],
            "standalone"
        );
        assert!(attachment.is_none());

        let local_info = route_stdio_request(
            json!(4),
            "tools/call",
            &info_message["params"],
            &info_message,
            &standalone,
            &mut attachment,
        );
        let local_info: Value = serde_json::from_str(&local_info).unwrap();
        assert_eq!(
            local_info["result"]["structuredContent"]["loaded_log_count"],
            1
        );

        shutdown.store(true, Ordering::Relaxed);
        server.join().unwrap();

        attachment = Some(expired_client);
        let expired = route_stdio_request(
            json!(5),
            "tools/call",
            &info_message["params"],
            &info_message,
            &standalone,
            &mut attachment,
        );
        let expired: Value = serde_json::from_str(&expired).unwrap();
        assert_eq!(expired["result"]["isError"], true);
        assert!(expired["result"]["structuredContent"]["message"]
            .as_str()
            .unwrap()
            .contains("expired"));
        assert!(attachment.is_none());
    }

    #[test]
    fn attachment_failures_are_generic_and_never_echo_the_session_id() {
        let supplied = session::generate_session_id();
        let mut attachment = None;
        let response = attach_stdio_session(
            json!(1),
            &json!({"session_id": supplied}),
            &mut attachment,
            |_| Err("internal endpoint and secret details".to_string()),
        );
        assert!(!response.contains(&supplied));
        assert!(!response.contains("internal endpoint"));
        let response: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["result"]["isError"], true);
        assert!(attachment.is_none());
    }

    #[test]
    fn gui_mode_rejects_log_id_instead_of_silently_ignoring_it() {
        let mut state = gui_state(&repetitive_log());
        let error = dispatch(
            "summarize_log",
            &json!({"log_id": "log_1", "with_filtered_log": false}),
            &mut state,
        )
        .unwrap_err();
        assert!(error.contains("log_id must be omitted"));
    }

    #[test]
    fn successful_tool_results_include_structured_and_text_content() {
        let payload = json!({"answer": 42});
        let result = tool_ok(payload.clone());
        assert_eq!(result["structuredContent"], payload);
        let text: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text, result["structuredContent"]);
    }

    #[test]
    fn modern_requests_reject_unsupported_versions_and_missing_capabilities() {
        let mut state = ServerState::default();
        let unsupported = json!({
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2099-01-01",
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        });
        let response = protocol_response(json!(2), "tools/list", &unsupported, &mut state);
        let response: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["error"]["code"], -32022);
        assert!(response["error"]["data"]["supported"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == MODERN_PROTOCOL_VERSION));

        let missing_capabilities = json!({
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": MODERN_PROTOCOL_VERSION
            }
        });
        let response = protocol_response(json!(3), "tools/list", &missing_capabilities, &mut state);
        let response: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["error"]["code"], -32602);
    }

    fn write_temp(content: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "logotomy_mcp_test_{}_{}.log",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    /// Regression: after `trim_left`, the timeline density and MCP `line_payload`
    /// must reflect the *visible* lines, not the original absolute indices.
    #[test]
    fn trim_left_keeps_timeline_and_mcp_payload_in_sync() {
        let content = "2026-07-19T10:00:00.000Z line0\n\
                       2026-07-19T10:01:00.000Z line1\n\
                       2026-07-19T10:02:00.000Z line2\n\
                       2026-07-19T10:03:00.000Z line3\n\
                       2026-07-19T10:04:00.000Z line4\n";
        let path = write_temp(content);
        let mut doc = LogDocument::open(&path).unwrap();
        doc.trim_left(2); // visible lines are original 2,3,4 → 10:02, 10:03, 10:04
        assert_eq!(doc.total_lines(), 3);

        // --- MCP line_payload must report the visible line's timestamp ---
        // Visible lines are original 2,3,4 → 10:02, 10:03, 10:04.
        let expected_first = chrono::DateTime::parse_from_rfc3339("2026-07-19T10:02:00.000Z")
            .unwrap()
            .timestamp_millis();
        let expected_last = chrono::DateTime::parse_from_rfc3339("2026-07-19T10:04:00.000Z")
            .unwrap()
            .timestamp_millis();
        assert_eq!(doc.ts_at(0), expected_first);
        assert_eq!(doc.ts_at(2), expected_last);

        let p0 = line_payload(&doc, 0);
        assert_eq!(p0["time"], Value::String(format_ms(expected_first)));
        let p2 = line_payload(&doc, 2);
        assert_eq!(p2["time"], Value::String(format_ms(expected_last)));
        // The payload text must also be the visible line, not the trimmed-away one.
        assert!(p0["text"].as_str().unwrap().contains("line2"));
        assert!(p2["text"].as_str().unwrap().contains("line4"));

        // --- Timeline density must be centered on the visible lines ---
        let tl = Timeline::build(&doc, &[], 256);
        let sum: u32 = tl.density.iter().sum();
        assert_eq!(sum as usize, doc.total_lines());

        // Density-weighted mean of bucket centers ≈ mean of visible timestamps.
        let mut weighted_sum: i64 = 0;
        let mut total: u32 = 0;
        for (i, &c) in tl.density.iter().enumerate() {
            if c > 0 {
                weighted_sum += tl.bucket_center(i) * c as i64;
                total += c;
            }
        }
        let mean_ts = (doc.ts_at(0) + doc.ts_at(1) + doc.ts_at(2)) / 3;
        let weighted_mean = weighted_sum / total as i64;
        assert!(
            (weighted_mean - mean_ts).abs() < 2000,
            "timeline density peak drifted: weighted_mean={weighted_mean} expected≈{mean_ts}"
        );

        std::fs::remove_file(path).ok();
    }

    /// Regression: GUI mutations use `Arc::make_mut`, which deep-copies the
    /// document while the server holds a reference — leaving `active_doc`
    /// stale unless the GUI pushes the new Arc back via `set_active_doc`.
    /// This test simulates that swap pattern and asserts the server converges
    /// and the `_active` match cache is invalidated.
    #[test]
    fn gui_mutation_swap_updates_active_doc_and_invalidates_cache() {
        let content = "2026-07-19T10:00:00.000Z ERROR line0\n\
                       2026-07-19T10:01:00.000Z ok line1\n\
                       2026-07-19T10:02:00.000Z ERROR line2\n\
                       2026-07-19T10:03:00.000Z ok line3\n";
        let path = write_temp(content);
        let doc = Arc::new(LogDocument::open(&path).unwrap());

        let mut state = ServerState::default();
        state.set_active_doc(Arc::clone(&doc));

        // Populate the `_active` match cache as an MCP tool call would.
        let matches = state.matches_for("_active", "ERROR", true).unwrap();
        assert_eq!(matches.len(), 2);
        assert!(!state.match_cache.is_empty());

        // --- GUI mutation path: make_mut deep-copies (state holds a ref) ---
        let mut gui_doc = Arc::clone(&doc);
        Arc::make_mut(&mut gui_doc).trim_left(1); // drops "ERROR line0"
        assert!(!Arc::ptr_eq(&gui_doc, &doc), "make_mut should have copied");

        // Without the swap, the server would keep serving the stale doc.
        assert!(Arc::ptr_eq(state.active_doc.as_ref().unwrap(), &doc));

        // The fix: GUI pushes the mutated Arc back (sync_mcp_active_doc).
        state.set_active_doc(Arc::clone(&gui_doc));
        state.active_doc_dirty.store(false, Ordering::Relaxed); // GUI-originated

        // Server now serves the trimmed document…
        let served = state.get_active_doc().unwrap();
        assert!(Arc::ptr_eq(&served, &gui_doc));
        assert_eq!(served.total_lines(), 3);

        // …the stale `_active` cache entries were dropped…
        assert!(state.match_cache.is_empty());

        // …and a fresh scan reflects the trimmed content (1 ERROR, not 2).
        let matches = state.matches_for("_active", "ERROR", true).unwrap();
        assert_eq!(matches.len(), 1);

        std::fs::remove_file(path).ok();
    }

    // ---- compression-first tool tests ----

    /// Build a GUI-mode ServerState with `content` as the active document.
    fn gui_state(content: &str) -> ServerState {
        let path = write_temp(content);
        let doc = LogDocument::open(&path).unwrap();
        std::fs::remove_file(path).ok();
        let mut state = ServerState::default();
        state.set_active_doc(Arc::new(doc));
        state
    }

    /// 21 repetitive INFO lines around a single ERROR.
    fn repetitive_log() -> String {
        let mut s = String::new();
        for i in 0..10 {
            s.push_str(&format!(
                "2026-07-19T10:00:{i:02}.000Z INFO request completed in {i}ms\n"
            ));
        }
        s.push_str("2026-07-19T10:00:10.000Z ERROR disk /dev/sda full\n");
        for i in 11..21 {
            s.push_str(&format!(
                "2026-07-19T10:00:{i:02}.000Z INFO request completed in {i}ms\n"
            ));
        }
        s
    }

    #[test]
    fn log_sequence_dense_triples() {
        let mut state = gui_state(&repetitive_log());
        let out = tool_log_sequence(
            &json!({ "start": 1, "end": 21, "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["total_entries"], 21);
        assert_eq!(out["returned"], 21);
        assert_eq!(out["truncated"], false);
        assert_eq!(out["collapsed"], false);
        let seq = out["sequence"].as_array().unwrap();
        assert_eq!(seq.len(), 21);
        // Each entry is [epoch_ms|null, line, template_id].
        let first = seq[0].as_array().unwrap();
        assert_eq!(first[1], 1);
        assert!(first[2].as_u64().is_some());
        let eleventh = seq[10].as_array().unwrap();
        assert_eq!(eleventh[1], 11); // the ERROR line
                                     // GUI mode must not leak bookkeeping fields.
        assert!(out.get("log_id").is_none());
    }

    #[test]
    fn log_sequence_collapses_long_runs() {
        let mut state = gui_state(&repetitive_log());
        let out = tool_log_sequence(
            &json!({ "collapse": true, "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        let seq = out["sequence"].as_array().unwrap();
        // 10 INFO (collapsed) + 1 ERROR (dense triple) + 10 INFO (collapsed) = 3.
        assert_eq!(seq.len(), 3, "seq: {seq:?}");
        assert_eq!(seq[0]["count"], 10);
        assert_eq!(seq[0]["start_line"], 1);
        assert_eq!(seq[0]["end_line"], 10);
        assert!(seq[0]["pattern"].as_str().unwrap().contains("INFO request"));
        // Short middle run stays a dense triple [ts, line, template_id].
        assert_eq!(seq[1].as_array().unwrap()[1], 11);
        assert_eq!(seq[2]["count"], 10);
        assert_eq!(seq[2]["start_line"], 12);
    }

    #[test]
    fn log_sequence_truncates_and_flags() {
        let mut state = gui_state(&repetitive_log());
        let out = tool_log_sequence(
            &json!({ "max_entries": 5, "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["total_entries"], 21);
        assert_eq!(out["returned"], 5);
        assert_eq!(out["truncated"], true);
        assert_eq!(out["sequence"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn raw_log_by_line_and_time() {
        let mut state = gui_state(&repetitive_log());
        // By line.
        let out = tool_raw_log(
            &json!({ "start": 10, "end": 12, "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        let lines = out["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 3);
        assert!(lines[1].as_str().unwrap().contains("ERROR"));
        // By time (the ERROR is at 10:00:10).
        let out = tool_raw_log(
            &json!({ "start": "2026-07-19T10:00:10.000Z", "end": "2026-07-19T10:00:10.000Z", "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        let lines = out["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].as_str().unwrap().contains("ERROR"));
        // Inverted range errors.
        assert!(tool_raw_log(
            &json!({ "start": 5, "end": 3, "with_filtered_log": false }),
            &mut state
        )
        .is_err());
    }

    #[test]
    fn summarize_includes_budget_fields() {
        let mut state = gui_state(&repetitive_log());
        let out = tool_summarize(&json!({ "with_filtered_log": false }), &mut state).unwrap();
        assert_eq!(out["lines"], 21);
        assert!(out["template_count"].as_u64().unwrap() >= 2);
        assert!(out["template_size_bytes"].as_u64().unwrap() > 0);
        assert!(out["sequence_estimate_bytes"].as_u64().unwrap() > 0);
    }

    #[test]
    fn get_template_dict_shapes() {
        let mut state = gui_state(&repetitive_log());
        let all = tool_get_template(&json!({ "with_filtered_log": false }), &mut state).unwrap();
        let dict = all["templates"].as_object().unwrap();
        assert!(dict.len() >= 2);
        for v in dict.values() {
            assert!(v["pattern"].as_str().is_some());
            assert!(v["count"].as_u64().is_some());
            assert!(v["example_line"].as_u64().is_some());
        }
        // Fetch a specific id.
        let tid = state.get_active_doc().unwrap().template_at(0);
        let one = tool_get_template(
            &json!({ "ids": [tid], "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        let dict = one["templates"].as_object().unwrap();
        assert_eq!(dict.len(), 1);
        assert!(dict.contains_key(&tid.to_string()));
        // Unknown id is simply absent, not an error.
        let none = tool_get_template(
            &json!({ "ids": [999999], "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(none["templates"].as_object().unwrap().len(), 0);
        // Non-integer ids error.
        assert!(tool_get_template(
            &json!({ "ids": [1.5], "with_filtered_log": false }),
            &mut state
        )
        .is_err());
    }

    #[test]
    fn trim_focuses_window_and_invalidates_cache() {
        let mut state = gui_state(&repetitive_log());
        // Prime the `_active` match cache as an MCP tool call would.
        let m = state.matches_for("_active", "ERROR", true).unwrap();
        assert_eq!(m.len(), 1);
        assert!(!state.match_cache.is_empty());
        // Trim to lines 1..=5.
        let out = tool_trim_gui(&json!({ "start": 1, "end": 5 }), &mut state).unwrap();
        assert_eq!(out["remaining_lines"], 5);
        // The stale `_active` cache was invalidated.
        assert!(state.match_cache.is_empty());
        assert_eq!(state.get_active_doc().unwrap().total_lines(), 5);
    }

    #[test]
    fn summarize_range_restricts_results() {
        let mut state = gui_state(&repetitive_log());
        // Lines 1..=10 are all INFO; the ERROR is at line 11.
        let out = tool_summarize(
            &json!({ "start": 1, "end": 10, "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["lines"], 10);
        assert_eq!(out["window"]["start_line"], 1);
        assert_eq!(out["window"]["end_line"], 10);
        assert_eq!(out["error_template_count"], 0);
        // Restrict to just the ERROR line.
        let out = tool_summarize(
            &json!({ "start": 11, "end": 11, "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["lines"], 1);
        assert_eq!(out["error_template_count"], 1);
        assert_eq!(out["error_line_count"], 1);
    }

    #[test]
    fn histogram_range_restricts_buckets() {
        let mut state = gui_state(&repetitive_log());
        let out = tool_timeline_histogram(
            &json!({ "start": 1, "end": 10, "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["total"], 10);
        // Keyword within a range.
        let out = tool_timeline_histogram(
            &json!({ "start": 11, "end": 11, "keyword": "ERROR", "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["total"], 1);
    }

    #[test]
    fn anomalies_range_restricts_results() {
        let mut state = gui_state(&repetitive_log());
        let out = tool_template_anomalies(
            &json!({ "start": 1, "end": 5, "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["total_lines"], 5);
    }

    #[test]
    fn log_sequence_by_time() {
        let mut state = gui_state(&repetitive_log());
        let out = tool_log_sequence(
            &json!({ "start": "2026-07-19T10:00:10.000Z", "end": "2026-07-19T10:00:10.000Z", "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["total_entries"], 1);
        assert_eq!(out["start_line"], 11);
        assert_eq!(out["end_line"], 11);
    }

    #[test]
    fn resolve_bound_rejects_zero_and_clamps() {
        let mut state = gui_state(&repetitive_log());
        // 0 is not a valid 1-based line number.
        assert!(tool_raw_log(
            &json!({ "start": 0, "end": 5, "with_filtered_log": false }),
            &mut state
        )
        .is_err());
        // A line beyond the end clamps instead of erroring.
        let out = tool_raw_log(
            &json!({ "start": 1, "end": 999, "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["total"], 21);
    }

    #[test]
    fn raw_log_missing_bounds_errors() {
        let mut state = gui_state(&repetitive_log());
        assert!(tool_raw_log(&json!({ "with_filtered_log": false }), &mut state).is_err());
        assert!(tool_raw_log(
            &json!({ "start": 1, "with_filtered_log": false }),
            &mut state
        )
        .is_err());
    }

    #[test]
    fn get_template_string_ids() {
        let mut state = gui_state(&repetitive_log());
        let tid = state.get_active_doc().unwrap().template_at(0);
        let out = tool_get_template(
            &json!({ "ids": [tid.to_string()], "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["templates"].as_object().unwrap().len(), 1);
    }

    #[test]
    fn trim_by_time_and_reset() {
        let mut state = gui_state(&repetitive_log());
        // Trim to just the ERROR line by time.
        let out = tool_trim_gui(
            &json!({ "start": "2026-07-19T10:00:10.000Z", "end": "2026-07-19T10:00:10.000Z" }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["remaining_lines"], 1);
        // Omit both bounds to reset to the full file.
        let out = tool_trim_gui(&json!({}), &mut state).unwrap();
        assert_eq!(out["remaining_lines"], 21);
    }

    #[test]
    fn trim_inverted_range_errors() {
        let mut state = gui_state(&repetitive_log());
        assert!(tool_trim_gui(&json!({ "start": 5, "end": 2 }), &mut state).is_err());
    }

    #[test]
    fn histogram_sums_to_line_count_and_filters() {
        let mut state = gui_state(&repetitive_log());
        let out = tool_timeline_histogram(
            &json!({ "buckets": 20, "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["domain"], "time");
        assert_eq!(out["total"], 21);
        assert_eq!(out["counts"].as_array().unwrap().len(), 20);

        // Keyword filter: only the ERROR line lands in a bucket.
        let out = tool_timeline_histogram(
            &json!({ "buckets": 20, "keyword": "ERROR", "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["total"], 1);

        // Both filters is an error.
        assert!(tool_timeline_histogram(
            &json!({ "keyword": "x", "template_id": 1, "with_filtered_log": false }),
            &mut state
        )
        .is_err());
    }

    #[test]
    fn anomalies_finds_rare_late_and_bursty() {
        let mut content = String::new();
        let ts = |i: usize| format!("2026-07-19T10:{:02}:{:02}.000Z", (i / 60) % 60, i % 60);
        // Lines 0..469: alternate two steady templates (938 lines spread evenly).
        for i in 0..470 {
            content.push_str(&format!("{} INFO alpha tick\n", ts(i)));
            content.push_str(&format!("{} INFO beta tock\n", ts(i)));
        }
        // Line 940: one unique snowflake (rare).
        content.push_str(&format!("{} TRACE unique snowflake event\n", ts(940)));
        // Lines 941..960: 20 identical WARN lines packed together (bursty).
        for i in 941..961 {
            content.push_str(&format!("{} WARN disk almost full\n", ts(i)));
        }
        // Lines 961..999: steady templates again.
        for i in 961..1000 {
            content.push_str(&format!("{} INFO alpha tick\n", ts(i)));
        }
        // Last lines: brand-new template appears for the first time (late).
        content.push_str(&format!("{} FATAL shutdown initiated\n", ts(999)));

        let mut state = gui_state(&content);
        let out =
            tool_template_anomalies(&json!({ "with_filtered_log": false }), &mut state).unwrap();
        let anomalies = out["anomalies"].as_array().unwrap();
        let by_pattern = |needle: &str| {
            anomalies
                .iter()
                .find(|a| a["pattern"].as_str().unwrap().contains(needle))
                .unwrap_or_else(|| panic!("no anomaly for '{needle}': {anomalies:?}"))
        };
        let reasons = |a: &Value| {
            a["reasons"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r.as_str().unwrap().to_string())
                .collect::<Vec<String>>()
        };
        assert!(reasons(by_pattern("TRACE unique"))
            .iter()
            .any(|r| r == "rare"));
        assert!(reasons(by_pattern("FATAL shutdown"))
            .iter()
            .any(|r| r == "first_seen_late"));
        assert!(reasons(by_pattern("WARN disk"))
            .iter()
            .any(|r| r == "bursty"));
        // Steady templates must not be flagged.
        assert!(anomalies
            .iter()
            .all(|a| !a["pattern"].as_str().unwrap().contains("alpha tick")));
    }

    #[test]
    fn samples_strategies_pick_expected_lines() {
        let mut content = String::new();
        for i in 0..10 {
            content.push_str(&format!(
                "2026-07-19T10:00:{i:02}.000Z INFO request completed in {i}ms\n"
            ));
        }
        let mut state = gui_state(&content);
        let tid = state.get_active_doc().unwrap().template_at(0);

        let out = tool_template_samples(
            &json!({ "template_id": tid, "n": 3, "strategy": "first", "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        let lines: Vec<u64> = out["samples"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["line"].as_u64().unwrap())
            .collect();
        assert_eq!(lines, vec![1, 2, 3]);

        // Default strategy: first + last always included, deterministic.
        let out = tool_template_samples(
            &json!({ "template_id": tid, "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        let lines: Vec<u64> = out["samples"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["line"].as_u64().unwrap())
            .collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines.first(), Some(&1));
        assert_eq!(lines.last(), Some(&10));
        let out2 = tool_template_samples(
            &json!({ "template_id": tid, "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(
            out["samples"], out2["samples"],
            "samples must be deterministic"
        );

        // Unknown template id errors.
        assert!(tool_template_samples(
            &json!({ "template_id": 9999, "with_filtered_log": false }),
            &mut state
        )
        .is_err());
    }

    #[test]
    fn find_occurrences_returns_paginated_tuples_and_rejects_legacy_output_modes() {
        let mut state = gui_state(
            "2026-07-19T10:00:00.000Z ERROR one\n\
             2026-07-19T10:00:01.000Z error two\n\
             2026-07-19T10:00:02.000Z ERROR three\n",
        );
        let out = dispatch(
            "find_occurrences",
            &json!({ "keyword": "error", "case_sensitive": false, "offset": 1, "max_results": 1, "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["total_matches"], 3);
        assert_eq!(out["offset"], 1);
        assert_eq!(out["returned"], 1);
        assert_eq!(out["has_more"], true);
        assert_eq!(
            out["occurrences"],
            json!([[2, state.get_active_doc().unwrap().ts_at(1)]])
        );
        assert!(out.get("lines").is_none());
        assert!(out.get("first_seen").is_none());

        // Case-sensitive matching and cache entries stay independent.
        let out = dispatch(
            "find_occurrences",
            &json!({ "keyword": "ERROR", "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["occurrences"].as_array().unwrap().len(), 2);

        // The old rich-output switches cannot silently change the contract.
        assert!(dispatch(
            "find_occurrences",
            &json!({ "keyword": "ERROR", "format": "refs", "with_filtered_log": false }),
            &mut state,
        )
        .is_err());
        assert!(dispatch(
            "find_occurrences",
            &json!({ "keyword": "ERROR", "context": 1, "with_filtered_log": false }),
            &mut state,
        )
        .is_err());
    }

    #[test]
    fn find_occurrences_applies_time_bounds_and_uses_null_for_missing_timestamps() {
        let mut state = gui_state(&repetitive_log());
        // Full: exactly one ERROR at line 11 (10:00:10).
        let out = dispatch(
            "find_occurrences",
            &json!({ "keyword": "ERROR", "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["total_matches"], 1);
        assert_eq!(out["occurrences"][0][0], 11);
        assert!(out["occurrences"][0][1].as_i64().is_some());
        // A window that excludes the ERROR yields zero matches.
        let out = dispatch(
            "find_occurrences",
            &json!({ "keyword": "ERROR", "before": "2026-07-19T10:00:09.000Z", "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["total_matches"], 0);
        // A window that includes it yields one match.
        let out = dispatch(
            "find_occurrences",
            &json!({ "keyword": "ERROR", "after": "2026-07-19T10:00:10.000Z", "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["total_matches"], 1);

        let mut timeless = gui_state("INFO one\nERROR two\n");
        let out = dispatch(
            "find_occurrences",
            &json!({ "keyword": "ERROR", "with_filtered_log": false }),
            &mut timeless,
        )
        .unwrap();
        assert_eq!(out["occurrences"], json!([[2, null]]));
    }

    #[test]
    fn find_occurrences_uses_the_same_case_and_tuple_contract_in_standalone_mode() {
        let path = write_temp("INFO Error first\nINFO error second\n");
        let mut state = ServerState::default();
        let log_id = state.add_doc(LogDocument::open(&path).unwrap());
        std::fs::remove_file(path).ok();

        let out = dispatch(
            "find_occurrences",
            &json!({ "log_id": log_id, "keyword": "error", "case_sensitive": false, "max_results": 1, "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["total_matches"], 2);
        assert_eq!(out["occurrences"], json!([[1, null]]));
        assert_eq!(out["has_more"], true);
        assert!(out.get("log_id").is_none());
    }

    #[test]
    fn summarize_surfaces_errors_gaps_and_densest_minute() {
        let mut content = String::new();
        content.push_str("2026-07-19T10:00:00.000Z INFO boot ok\n");
        content.push_str("2026-07-19T10:00:01.000Z ERROR disk full\n");
        // 300-second gap (process hung?) then more lines.
        content.push_str("2026-07-19T10:05:01.000Z INFO recovered\n");
        content.push_str("2026-07-19T10:05:02.000Z INFO recovered again\n");
        let mut state = gui_state(&content);
        let out = tool_summarize(&json!({ "with_filtered_log": false }), &mut state).unwrap();
        assert_eq!(out["lines"], 4);
        assert_eq!(out["error_template_count"], 1);
        assert_eq!(out["error_line_count"], 1);
        assert!(out["error_templates"][0]["pattern"]
            .as_str()
            .unwrap()
            .contains("ERROR"));
        let gaps = out["largest_time_gaps"].as_array().unwrap();
        assert_eq!(gaps[0]["gap_ms"], 300_000);
        assert_eq!(gaps[0]["line"], 3);
        assert!(out["densest_minute"]["count"].as_u64().unwrap() >= 2);
    }

    #[test]
    fn headless_compression_tools_require_log_id() {
        // Headless mode (no active doc): compression tools need log_id.
        let mut state = ServerState::default();
        assert!(tool_summarize(&json!({}), &mut state).is_err());
        // With a loaded log they work and include log_id.
        let path = write_temp("2026-07-19T10:00:00.000Z INFO hi\n");
        let log_id = state.add_doc(LogDocument::open(&path).unwrap());
        std::fs::remove_file(path).ok();
        let out = tool_summarize(
            &json!({ "log_id": log_id, "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["log_id"], json!(log_id));
        // Schemas advertise the new tools in both modes.
        assert!(headless_tools()
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "log_sequence"));
        assert!(gui_tools()
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "summarize_log"));
        // GUI-mode schemas must not require log_id.
        let gui = gui_tools();
        let seq = gui
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "log_sequence")
            .unwrap();
        assert!(seq["inputSchema"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r.as_str() != Some("log_id")));
    }

    #[test]
    fn schema_advertises_new_tools() {
        let headless = headless_tools();
        let headless_names: Vec<&str> = headless
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        for n in ["get_template", "log_sequence", "raw_log"] {
            assert!(headless_names.contains(&n), "headless missing {n}");
        }
        // trim is GUI-only.
        assert!(!headless_names.contains(&"trim"));

        let gui = gui_tools();
        let gui_names: Vec<&str> = gui
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(gui_names.contains(&"trim"));
        assert!(gui_names.contains(&"raw_log"));

        // raw_log requires start + end.
        let raw = headless
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "raw_log")
            .unwrap();
        let required: Vec<&str> = raw["inputSchema"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r.as_str())
            .collect();
        assert!(required.contains(&"start") && required.contains(&"end"));
    }

    #[test]
    fn tool_errors_are_structured_and_tools_have_agent_hints() {
        let error = tool_err("bad argument");
        assert_eq!(error["isError"], true);
        let text = error["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).unwrap();
        assert_eq!(payload["error"], "tool_error");
        assert_eq!(payload["message"], "bad argument");
        assert_eq!(payload["retryable"], false);
        let retryable = tool_err_retryable("session expired");
        assert_eq!(retryable["structuredContent"]["retryable"], true);

        let state = ServerState::default();
        let list = tools(&state).as_array().unwrap().to_vec();
        let load = list.iter().find(|t| t["name"] == "load_log").unwrap();
        assert_eq!(load["annotations"]["readOnlyHint"], false);
        assert_eq!(load["annotations"]["destructiveHint"], false);
        let summary = list.iter().find(|t| t["name"] == "summarize_log").unwrap();
        assert_eq!(summary["annotations"]["readOnlyHint"], true);
        assert_eq!(summary["annotations"]["idempotentHint"], true);
        assert!(summary["outputSchema"].is_object());
        let session = list.iter().find(|t| t["name"] == "session_info").unwrap();
        assert_eq!(session["annotations"]["readOnlyHint"], true);
        let get_analysis = list.iter().find(|t| t["name"] == "get_analysis").unwrap();
        assert_eq!(get_analysis["annotations"]["readOnlyHint"], true);
        let add_analysis = list.iter().find(|t| t["name"] == "add_analysis").unwrap();
        assert_eq!(add_analysis["annotations"]["readOnlyHint"], false);
    }

    #[test]
    fn public_tool_catalog_is_stable_and_mode_conditional() {
        let headless = ServerState::default();
        let mut gui = ServerState::default();
        gui.enable_gui_mode();
        assert_eq!(tools(&headless), tools(&gui));

        let catalog = tools(&headless);
        let list = catalog.as_array().unwrap();
        let attach = list
            .iter()
            .find(|tool| tool["name"] == "attach_gui_session")
            .unwrap();
        assert!(attach["description"]
            .as_str()
            .unwrap()
            .contains("12-character hexadecimal"));
        assert!(
            attach["inputSchema"]["properties"]["session_id"]["description"]
                .as_str()
                .unwrap()
                .contains("12-character hexadecimal")
        );
        assert!(list.iter().any(|tool| tool["name"] == "detach_gui_session"));
        assert!(list.iter().any(|tool| tool["name"] == "load_log"));
        assert!(list.iter().any(|tool| tool["name"] == "trim"));
        assert!(list.iter().any(|tool| tool["name"] == "get_analysis"));
        assert!(list.iter().any(|tool| tool["name"] == "add_analysis"));
        for name in ["find_occurrences", "summarize_log", "filters_get"] {
            let tool = list.iter().find(|tool| tool["name"] == name).unwrap();
            let required = tool["inputSchema"]["required"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(!required.iter().any(|item| item == "log_id"));
            assert!(tool["inputSchema"]["properties"]["log_id"]["description"]
                .as_str()
                .unwrap()
                .contains("standalone mode"));
        }
    }

    #[test]
    fn numeric_arguments_reject_invalid_values() {
        assert!(arg_usize(&json!({ "n": -1 }), "n", 3).is_err());
        assert!(arg_usize(&json!({ "n": "3" }), "n", 3).is_err());
        assert_eq!(arg_usize(&json!({}), "n", 3).unwrap(), 3);
    }

    // ---- filter tools + with_filtered_log ----

    #[test]
    fn filters_get_add_remove_flow() {
        let mut state = gui_state(&repetitive_log());
        let empty = tool_filters_get(&json!({}), &mut state).unwrap();
        assert_eq!(empty["filters"].as_array().unwrap().len(), 0);
        assert_eq!(empty["filter_count"], 0);
        assert!(
            empty.get("log_id").is_none(),
            "GUI mode must not leak log_id"
        );

        let add1 = tool_filters_add(&json!({ "filter_text": "ERROR" }), &mut state).unwrap();
        let fl = add1["filters"].as_array().unwrap();
        assert_eq!(fl.len(), 1);
        assert_eq!(fl[0]["id"], 0);
        assert_eq!(fl[0]["filter_text"], "ERROR");
        // Case-sensitive dedupe: differing case is a distinct filter; empty rejected.
        let dup = tool_filters_add(&json!({ "filter_text": "error" }), &mut state).unwrap();
        assert_eq!(dup["filters"].as_array().unwrap().len(), 2);
        assert_eq!(dup["filters"][1]["filter_text"], "error");
        assert!(tool_filters_add(&json!({ "filter_text": "   " }), &mut state).is_err());

        let add2 = tool_filters_add(&json!({ "filter_text": "INFO" }), &mut state).unwrap();
        assert_eq!(add2["filters"].as_array().unwrap().len(), 3);

        // Remove by id; ids renumber after removal.
        let rm = tool_filters_remove(&json!({ "id": 0 }), &mut state).unwrap();
        let fl = rm["filters"].as_array().unwrap();
        assert_eq!(fl.len(), 2);
        assert_eq!(fl[0]["id"], 0);
        assert_eq!(fl[0]["filter_text"], "error");
        assert_eq!(fl[1]["id"], 1);
        assert_eq!(fl[1]["filter_text"], "INFO");
        assert!(tool_filters_remove(&json!({ "id": 5 }), &mut state).is_err());
    }

    #[test]
    fn filters_cap_and_missing_args_error() {
        let mut state = gui_state(&repetitive_log());
        for i in 0..MAX_FILTERS {
            state.add_filter("_active", &format!("kw{i:02}")).unwrap();
        }
        assert!(state.add_filter("_active", "overflow").is_err());
        assert!(tool_filters_add(&json!({}), &mut state).is_err());
        assert!(tool_filters_remove(&json!({ "id": "x" }), &mut state).is_err());
    }

    #[test]
    fn gui_pin_tab_analysis_add_get_and_validate_lines() {
        let mut state = gui_state(
            "2026-07-19T10:00:00.000Z INFO first\n\
             2026-07-19T10:00:01.000Z ERROR second\n",
        );
        assert_eq!(tool_get_analysis(&state).unwrap()["analyses"], json!([]));

        let out = dispatch(
            "add_analysis",
            &json!({ "text": "  Check the retry path.  ", "lines": [2, 1, 2] }),
            &mut state,
        )
        .unwrap();
        assert_eq!(
            out["analyses"][0],
            json!({ "text": "Check the retry path.", "lines": [1, 2] })
        );
        assert!(state.analyses_dirty.load(Ordering::Relaxed));

        let out = dispatch(
            "add_analysis",
            &json!({ "text": "User context without a line" }),
            &mut state,
        )
        .unwrap();
        assert_eq!(
            out["analyses"][1],
            json!({ "text": "User context without a line", "lines": [] })
        );
        assert_eq!(tool_get_analysis(&state).unwrap(), out);

        for args in [
            json!({ "text": "" }),
            json!({ "text": "bad", "lines": 2 }),
            json!({ "text": "bad", "lines": [0] }),
            json!({ "text": "bad", "lines": [3] }),
            json!({ "text": "bad", "lines": [1.5] }),
        ] {
            assert!(
                dispatch("add_analysis", &args, &mut state).is_err(),
                "{args}"
            );
        }

        let mut headless = ServerState::default();
        assert!(dispatch("get_analysis", &json!({}), &mut headless).is_err());
        assert!(dispatch(
            "add_analysis",
            &json!({ "text": "not attached" }),
            &mut headless,
        )
        .is_err());
    }

    #[test]
    fn filters_work_headless_with_log_id() {
        let mut state = ServerState::default();
        let path =
            write_temp("2026-07-19T10:00:00.000Z ERROR x\n2026-07-19T10:00:01.000Z INFO y\n");
        let log_id = state.add_doc(LogDocument::open(&path).unwrap());
        std::fs::remove_file(path).ok();

        let out = tool_filters_add(
            &json!({ "log_id": log_id, "filter_text": "ERROR" }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["log_id"], json!(log_id));
        assert_eq!(out["filter_count"], 1);
        let out = tool_filters_get(&json!({ "log_id": log_id }), &mut state).unwrap();
        assert_eq!(out["filters"].as_array().unwrap().len(), 1);
        // Headless mode requires log_id.
        assert!(tool_filters_get(&json!({}), &mut state).is_err());
        // Closing the log drops its filters.
        tool_close_log(&json!({ "log_id": log_id }), &mut state).unwrap();
        assert!(tool_filters_get(&json!({ "log_id": log_id }), &mut state).is_err());
    }

    #[test]
    fn with_filtered_log_defaults_true_and_excludes_everything_else() {
        let content = "2026-07-19T10:00:00.000Z ERROR disk full\n\
                       2026-07-19T10:00:01.000Z INFO boot ok\n\
                       2026-07-19T10:00:02.000Z ERROR retry n\n\
                       2026-07-19T10:00:03.000Z DEBUG noise\n";
        let mut state = gui_state(content);
        // No filters set: with_filtered_log=true (the default) short-circuits
        // with the "no log" hint instead of scanning the whole file.
        let out = dispatch(
            "find_occurrences",
            &json!({ "keyword": "ERROR" }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["comment"], "no log");
        assert!(out["reason"]
            .as_str()
            .unwrap()
            .contains("no filters are configured"));
        assert_eq!(out["filter_count"], 0);
        assert_eq!(out["suggested_next_calls"][0]["tool"], "summarize_log");

        state.add_filter("_active", "ERROR").unwrap();
        // INFO/DEBUG lines exist but are excluded from the filtered view —
        // the "Everything Else" lane is never part of MCP arithmetic.
        let out = dispatch(
            "find_occurrences",
            &json!({ "keyword": "INFO" }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["total_matches"], 0);
        let out = dispatch(
            "find_occurrences",
            &json!({ "keyword": "DEBUG" }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["total_matches"], 0);
        // Explicitly unfiltered restores the full log.
        let out = dispatch(
            "find_occurrences",
            &json!({ "keyword": "INFO", "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["total_matches"], 1);
    }

    #[test]
    fn log_sequence_and_raw_log_respect_filtered_view() {
        let mut state = gui_state(&repetitive_log()); // 10 INFO / 1 ERROR / 10 INFO
        state.add_filter("_active", "ERROR").unwrap();

        let seq = tool_log_sequence(&json!({}), &mut state).unwrap();
        assert_eq!(seq["total_entries"], 1);
        assert_eq!(seq["sequence"].as_array().unwrap().len(), 1);
        assert_eq!(seq["sequence"][0].as_array().unwrap()[1], 11);
        let seq = tool_log_sequence(&json!({ "with_filtered_log": false }), &mut state).unwrap();
        assert_eq!(seq["total_entries"], 21);

        let raw = tool_raw_log(&json!({ "start": 1, "end": 21 }), &mut state).unwrap();
        assert_eq!(raw["total"], 1);
        assert!(raw["lines"][0].as_str().unwrap().contains("ERROR"));
        let raw = tool_raw_log(
            &json!({ "start": 1, "end": 21, "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(raw["total"], 21);
    }

    #[test]
    fn summarize_histogram_and_anomalies_respect_filtered_view() {
        let mut state = gui_state(&repetitive_log());
        state.add_filter("_active", "ERROR").unwrap();

        let s = tool_summarize(&json!({}), &mut state).unwrap();
        assert_eq!(s["lines"], 1);
        assert_eq!(s["error_line_count"], 1);
        let s = tool_summarize(&json!({ "with_filtered_log": false }), &mut state).unwrap();
        assert_eq!(s["lines"], 21);

        let h = tool_timeline_histogram(&json!({ "buckets": 20 }), &mut state).unwrap();
        assert_eq!(h["total"], 1);
        let h = tool_timeline_histogram(
            &json!({ "buckets": 20, "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(h["total"], 21);

        let a = tool_template_anomalies(&json!({}), &mut state).unwrap();
        assert_eq!(a["total_lines"], 1);
    }

    #[test]
    fn get_template_and_samples_respect_filtered_view() {
        let mut state = gui_state(&repetitive_log());
        let info_tid = state
            .get_active_doc()
            .unwrap()
            .templates
            .iter()
            .find(|t| t.pattern.contains("INFO request"))
            .unwrap()
            .id;
        state.add_filter("_active", "ERROR").unwrap();

        // get_template: INFO count reflects the filtered set (0 = filtered away).
        let out = tool_get_template(&json!({ "ids": [info_tid] }), &mut state).unwrap();
        assert_eq!(out["templates"][info_tid.to_string()]["count"], 0);
        let out = tool_get_template(
            &json!({ "ids": [info_tid], "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["templates"][info_tid.to_string()]["count"], 20);

        // get_template_samples: an INFO-only template has no visible lines.
        assert!(tool_template_samples(&json!({ "template_id": info_tid }), &mut state).is_err());
        let out = tool_template_samples(
            &json!({ "template_id": info_tid, "with_filtered_log": false, "n": 1, "strategy": "first" }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["total_matches"], 20);
    }

    #[test]
    fn schema_advertises_filter_tools_and_with_filtered_log() {
        for (tools, require_log_id) in [(headless_tools(), true), (gui_tools(), false)] {
            let arr = tools.as_array().unwrap();
            for n in ["filters_get", "filters_add", "filters_remove"] {
                assert!(arr.iter().any(|t| t["name"] == n), "missing {n}");
            }
            for n in [
                "find_occurrences",
                "raw_log",
                "log_sequence",
                "summarize_log",
                "get_timeline_histogram",
                "get_template_anomalies",
                "get_template",
                "get_template_samples",
            ] {
                let t = arr.iter().find(|t| t["name"] == n).unwrap();
                assert!(
                    t["inputSchema"]["properties"]["with_filtered_log"].is_object(),
                    "{n} missing with_filtered_log"
                );
            }
            // Headless filter tools require log_id; GUI ones must not.
            let fget = arr.iter().find(|t| t["name"] == "filters_get").unwrap();
            let req: Vec<&str> = fget["inputSchema"]["required"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            assert_eq!(req.contains(&"log_id"), require_log_id);
        }
    }

    #[test]
    fn with_filtered_log_no_filters_short_circuits_every_tool() {
        let mut state = gui_state(&repetitive_log());
        // All 8 tools under with_filtered_log=true (the default) with zero
        // filters must reply with the "no log" hint — not silently scan.
        let cases: &[(&str, Value)] = &[
            ("find_occurrences", json!({ "keyword": "ERROR" })),
            ("raw_log", json!({ "start": 1, "end": 21 })),
            ("log_sequence", json!({})),
            ("summarize_log", json!({})),
            ("get_timeline_histogram", json!({})),
            ("get_template_anomalies", json!({})),
            ("get_template", json!({})),
            ("get_template_samples", json!({ "template_id": 1 })),
        ];
        for (name, args) in cases {
            let out = dispatch(name, args, &mut state).unwrap();
            assert_eq!(
                out["comment"], "no log",
                "{name} should short-circuit: {out}"
            );
            assert!(
                out["reason"]
                    .as_str()
                    .unwrap()
                    .contains("no filters are configured"),
                "{name}: reason says: {out}"
            );
            assert_eq!(
                out["suggested_next_calls"][1]["tool"], "filters_add",
                "{name}: missing filters_add suggestion: {out}"
            );
        }
        // with_filtered_log=false + no filters still traverses the full file.
        let out = dispatch(
            "summarize_log",
            &json!({ "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["lines"], 21);
        let out = dispatch(
            "log_sequence",
            &json!({ "with_filtered_log": false }),
            &mut state,
        )
        .unwrap();
        assert_eq!(out["total_entries"], 21);
    }
}
