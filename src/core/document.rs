//! LogDocument: a memory-mapped, indexed, template-mined view of a log file.
//!
//! The file is never fully materialized as `String`s. We mmap it, build a
//! line-offset index in one `memchr` pass (GB/s territory), then run a single
//! analysis pass that extracts timestamps and mines Drain templates.
//! Progress is reported over a channel so the UI can paint a real progress
//! bar instead of a sad spinner.
//!
//! Supports trimming: `trim_left` / `trim_right` narrows the visible line
//! range without reloading the file. The mmap stays intact; only the per-line
//! arrays are truncated.

use std::borrow::Cow;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crossbeam_channel::Sender;
use memchr::memchr_iter;
use memmap2::Mmap;

use crate::core::drain::Drain;
use crate::core::format::{learn_header_slots, FormatContext, FormatDetector, LogFormat};
use crate::core::masking::{LogMasker, MaskCache};
use crate::core::time::{CustomTimeFormat, TimeFormatKind};

/// Tunables for the log-parsing pipeline (template mining + header learning).
#[derive(Clone, Copy, Debug)]
pub struct ParsingConfig {
    /// Drain similarity threshold for merging a line into a cluster.
    pub sim_threshold: f64,
    /// How many leading lines to sample when learning the common header shape.
    pub header_sample_lines: usize,
    /// Drain parse-tree depth (default 4). Depth N = token count + (N-2) routing tokens.
    pub drain_depth: usize,
}

impl Default for ParsingConfig {
    fn default() -> Self {
        Self {
            sim_threshold: 0.5,
            header_sample_lines: 200,
            drain_depth: 4,
        }
    }
}

/// Report progress at most every 4 MiB so the channel stays quiet.
const PROGRESS_STRIDE: u64 = 4 * 1024 * 1024;

/// Flattened, export-friendly view of one mined template.
#[derive(Clone, Debug)]
pub struct TemplateInfo {
    pub id: u32,
    pub pattern: String,
    pub count: usize,
    pub example_line: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadStage {
    Indexing,
    Analyzing,
}

impl LoadStage {
    pub fn label(&self) -> &'static str {
        match self {
            LoadStage::Indexing => "Indexing lines",
            LoadStage::Analyzing => "Mining templates & timestamps",
        }
    }
}

pub enum LoadProgress {
    Progress {
        stage: LoadStage,
        done: u64,
        total: u64,
    },
    Done(Box<LogDocument>),
    Error(String),
}

/// Result of checking whether the source file changed since this document was
/// loaded. This deliberately borrows the document immutably so callers that
/// share a `LogDocument` through an `Arc` can avoid cloning all per-line
/// indexes when the usual live-tail poll finds no new data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileChange {
    Unchanged,
    Appended,
    Shrunk,
    Modified,
}

pub struct LogDocument {
    pub path: PathBuf,
    pub file_name: String,
    data: Arc<Mmap>,
    /// Byte offset where each line starts; last element is the file size.
    #[doc(hidden)]
    file_handle: Arc<File>, // Keep the file handle to maintain the lock
    pub line_offsets: Vec<u64>,
    /// Forward-filled timestamps: untimestamped lines (stack traces, etc.)
    /// inherit the previous line's time. -1 before the first known timestamp.
    /// Indexed by *original* (untrimmed) line index. Use `ts_at` for
    /// trim-relative access.
    ts_ff: Vec<i64>,
    /// Compact bitset of physical lines that contained an explicit timestamp.
    /// It lets viewport analyzers recover the start/end of multiline records
    /// without retaining a second timestamp array (one bit per source line).
    record_starts: Vec<u64>,
    /// Per-line Drain template cluster ID.
    /// Indexed by *original* (untrimmed) line index. Use `template_at` for
    /// trim-relative access.
    template_ids: Vec<u32>,
    /// The Drain instance used for template mining.
    #[doc(hidden)]
    drain: Arc<Mutex<Drain>>,
    /// Pre-mining masker: replaces dynamic values (IPs, UUIDs, paths, etc.)
    /// with semantic placeholders before Drain clustering.
    masker: LogMasker,
    /// Learned per-file header slots: `Some(mask)` at position `i` means the
    /// i-th token is a consistently-dynamic header field (host, pid, thread)
    /// and gets replaced with that mask before Drain clustering.
    header_slots: Vec<Option<&'static str>>,
    /// Memoized per-token mask decisions, reused across all lines.
    mask_cache: MaskCache,
    /// Detected log format (drives per-line normalization).
    log_format: &'static dyn LogFormat,
    /// Detected timestamp family for this format (`None` when timeless).
    time_format: Option<TimeFormatKind>,
    /// Mined templates, ordered by cluster ID.
    pub templates: Vec<TemplateInfo>,
    /// (min, max) extracted timestamp in the file, if any.
    pub time_range: Option<(i64, i64)>,
    pub file_size: u64,
    pub file_mtime: SystemTime,
    /// Maximum byte length of any line (after stripping trailing newlines).
    /// Used to compute the horizontal scroll extent in the log view.
    pub max_line_width: usize,
    /// First line index of the visible range (inclusive). 0 = no trim.
    pub trim_start: usize,
    /// One-past-the-last line index of the visible range. Defaults to total_lines().
    pub trim_end: usize,
}

// A copy-on-write document must not share its mutable Drain state. Sharing it
// makes an appended background copy alter clustering behind the still-rendered
// document's back. The mmap/file handle remain cheap shared Arcs; per-line
// indexes and mutable mining state are independent.
impl Clone for LogDocument {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            file_name: self.file_name.clone(),
            data: Arc::clone(&self.data),
            file_handle: Arc::clone(&self.file_handle),
            line_offsets: self.line_offsets.clone(),
            ts_ff: self.ts_ff.clone(),
            record_starts: self.record_starts.clone(),
            template_ids: self.template_ids.clone(),
            drain: Arc::new(Mutex::new(self.drain.lock().unwrap().clone())),
            masker: self.masker.clone(),
            header_slots: self.header_slots.clone(),
            mask_cache: self.mask_cache.clone(),
            log_format: self.log_format,
            time_format: self.time_format.clone(),
            templates: self.templates.clone(),
            time_range: self.time_range,
            file_size: self.file_size,
            file_mtime: self.file_mtime,
            max_line_width: self.max_line_width,
            trim_start: self.trim_start,
            trim_end: self.trim_end,
        }
    }
}

impl LogDocument {
    /// Number of lines in the current (possibly trimmed) view.
    pub fn total_lines(&self) -> usize {
        self.trim_end - self.trim_start
    }

    /// Number of lines in the original, untrimmed file.
    pub fn total_lines_untrimmed(&self) -> usize {
        self.line_offsets.len().saturating_sub(1)
    }

    /// Zero-copy line access straight from the mmap (lossy if invalid UTF-8).
    /// `idx` is relative to the current trim window (0 = first visible line).
    pub fn line(&self, idx: usize) -> Cow<'_, str> {
        String::from_utf8_lossy(self.line_bytes(idx))
    }

    /// Exact bytes for a trim-relative line, excluding CR/LF terminators.
    pub fn line_bytes(&self, idx: usize) -> &[u8] {
        self.line_bytes_untrimmed(self.trim_start + idx)
    }

    /// Forward-filled timestamp for a trim-relative line index.
    /// `rel` is relative to the current trim window (0 = first visible line).
    /// Returns -1 before the first known timestamp.
    pub fn ts_at(&self, rel: usize) -> i64 {
        self.ts_ff[self.trim_start + rel]
    }

    /// Bounds-checked forward-filled timestamp for a trim-relative line index.
    /// Returns `None` when `rel` is outside the current trim window.
    pub fn ts_at_opt(&self, rel: usize) -> Option<i64> {
        let real = self.trim_start.checked_add(rel)?;
        self.ts_ff.get(real).copied()
    }

    /// Original-file bounds of the timestamp-delimited record containing
    /// `original_line`. The end is exclusive. Returns `None` before the first
    /// explicit timestamp or for timeless files.
    pub fn record_range_containing(&self, original_line: usize) -> Option<std::ops::Range<usize>> {
        if original_line >= self.total_lines_untrimmed() {
            return None;
        }
        let start = self.record_start_at_or_before(original_line)?;
        let end = self
            .record_start_after(original_line)
            .unwrap_or(self.total_lines_untrimmed());
        Some(start..end)
    }

    fn record_start_at_or_before(&self, line: usize) -> Option<usize> {
        let mut word_idx = line / 64;
        let bit = line % 64;
        let mut word = *self.record_starts.get(word_idx)?;
        word &= if bit == 63 {
            u64::MAX
        } else {
            (1u64 << (bit + 1)) - 1
        };
        loop {
            if word != 0 {
                return Some(word_idx * 64 + (63 - word.leading_zeros() as usize));
            }
            word_idx = word_idx.checked_sub(1)?;
            word = self.record_starts[word_idx];
        }
    }

    fn record_start_after(&self, line: usize) -> Option<usize> {
        let next = line.checked_add(1)?;
        let mut word_idx = next / 64;
        let bit = next % 64;
        let mut word = *self.record_starts.get(word_idx)? & (u64::MAX << bit);
        loop {
            if word != 0 {
                let found = word_idx * 64 + word.trailing_zeros() as usize;
                return (found < self.total_lines_untrimmed()).then_some(found);
            }
            word_idx += 1;
            word = *self.record_starts.get(word_idx)?;
        }
    }

    /// Drain template cluster ID for a trim-relative line index.
    /// `rel` is relative to the current trim window (0 = first visible line).
    pub fn template_at(&self, rel: usize) -> u32 {
        self.template_ids[self.trim_start + rel]
    }

    /// Name of the detected log format (e.g. "json", "cef", "rfc5424", "plain").
    pub fn format_name(&self) -> &'static str {
        self.log_format.name()
    }

    /// Name of the detected timestamp family, if any ("none" when timeless).
    pub fn time_format_name(&self) -> Option<String> {
        self.time_format.as_ref().map(|f| f.name())
    }

    /// Access a line by its original (untrimmed) index.
    pub fn line_untrimmed(&self, real_idx: usize) -> Cow<'_, str> {
        String::from_utf8_lossy(self.line_bytes_untrimmed(real_idx))
    }

    /// Exact bytes for an original-file line, excluding CR/LF terminators.
    ///
    /// Structured-data scanners use this instead of the lossy text accessor so
    /// their byte coordinates always map back to the mmap.
    pub fn line_bytes_untrimmed(&self, real_idx: usize) -> &[u8] {
        let start = self.line_offsets[real_idx] as usize;
        let mut end = self.line_offsets[real_idx + 1] as usize;
        while end > start && (self.data[end - 1] == b'\n' || self.data[end - 1] == b'\r') {
            end -= 1;
        }
        &self.data[start..end]
    }

    /// Whether the document has been trimmed.
    pub fn is_trimmed(&self) -> bool {
        self.trim_start > 0 || self.trim_end < self.total_lines_untrimmed()
    }

    /// Trim away all lines *before* `line` (keeping `line` and everything after).
    /// `line` is an untrimmed (original) line index.
    pub fn trim_left(&mut self, line: usize) {
        let total = self.total_lines_untrimmed();
        if line >= total.saturating_sub(1) {
            return; // keep at least one line
        }
        // Keep the existing right bound; only move the left bound forward.
        self.trim_start = line;
        if self.trim_end <= self.trim_start {
            self.trim_end = total;
        }
        self.rebuild_trimmed_arrays();
    }

    /// Trim away all lines *after* `line` (keeping everything up to and including `line`).
    /// `line` is an untrimmed (original) line index.
    pub fn trim_right(&mut self, line: usize) {
        let total = self.total_lines_untrimmed();
        let new_end = (line + 1).min(total);
        // No-op when the range would invert; never clobber trim_start.
        if new_end <= self.trim_start {
            return;
        }
        self.trim_end = new_end;
        self.rebuild_trimmed_arrays();
    }

    /// Set the visible window to the inclusive untrimmed range
    /// `[start_untrimmed, end_untrimmed_inclusive]` in one step (used by the MCP
    /// `trim` tool). Both bounds are original (untrimmed) line indices. Clamps to
    /// valid bounds and is a no-op when the range would invert or empty.
    pub fn trim_range(&mut self, start_untrimmed: usize, end_untrimmed_inclusive: usize) {
        let total = self.total_lines_untrimmed();
        let s = start_untrimmed.min(total.saturating_sub(1));
        let e = (end_untrimmed_inclusive + 1).min(total);
        if e <= s {
            return;
        }
        self.trim_start = s;
        self.trim_end = e;
        self.rebuild_trimmed_arrays();
    }

    /// Reset trim to show the full file.
    pub fn reset_trim(&mut self) {
        self.trim_start = 0;
        self.trim_end = self.total_lines_untrimmed();
        self.rebuild_trimmed_arrays();
    }

    /// Rebuild time_range and templates to match the current trim window.
    /// Per-line arrays (timestamps, ts_ff, template_ids) are kept at full length
    /// since `line()` and other accessors use `trim_start` to offset into them.
    fn rebuild_trimmed_arrays(&mut self) {
        // Recalculate time_range from trimmed timestamps.
        let mut min_ts = i64::MAX;
        let mut max_ts = i64::MIN;
        for &t in &self.ts_ff[self.trim_start..self.trim_end] {
            if t >= 0 {
                min_ts = min_ts.min(t);
                max_ts = max_ts.max(t);
            }
        }
        self.time_range = if min_ts <= max_ts {
            Some((min_ts, max_ts))
        } else {
            None
        };

        // Recalculate template counts for the trimmed view.
        self.recalculate_template_counts();
    }

    /// Update template counts based on the current trim window without re-mining.
    fn recalculate_template_counts(&mut self) {
        let mut counts = std::collections::HashMap::new();
        for i in self.trim_start..self.trim_end {
            let template_id = self.template_ids[i];
            *counts.entry(template_id).or_insert(0) += 1;
        }

        for template in &mut self.templates {
            template.count = counts.get(&template.id).copied().unwrap_or(0);
        }
    }

    /// Checks for appended data and incrementally loads it.
    /// Returns `Ok(true)` if new data was loaded, `Ok(false)` if no change.
    pub fn append_new_data(&mut self) -> Result<bool, String> {
        let change = self.file_change()?;
        match change {
            FileChange::Unchanged => return Ok(false),
            FileChange::Shrunk => {
                return Err("file has shrunk on disk; a full reload is required".to_string());
            }
            FileChange::Modified => {
                return Err("file has changed on disk; a full reload is required".to_string());
            }
            FileChange::Appended => {}
        }

        let new_meta = self
            .file_handle
            .metadata()
            .map_err(|e| format!("failed to get file metadata: {e}"))?;
        let new_size = new_meta.len();
        let new_mtime = new_meta
            .modified()
            .map_err(|e| format!("failed to get file modification time: {e}"))?;

        // --- File has grown, load new data ---
        log::info!(
            "File '{}' has grown from {} to {} bytes. Appending new data.",
            self.file_name,
            self.file_size,
            new_size
        );

        let old_file_size = self.file_size as usize;
        let old_line_count = self.total_lines_untrimmed();

        // If the old file did NOT end with a newline, the last "line" is partial.
        // We must pop the old file size marker from the offsets list so the new
        // scan can correctly extend this partial line.
        if old_file_size > 0 && self.data[old_file_size - 1] != b'\n' {
            self.line_offsets.pop();
        }

        // Re-map the file to the new size.
        let new_mmap = unsafe { Mmap::map(&*self.file_handle) }
            .map_err(|e| format!("mmap failed on append: {e}"))?;
        self.data = Arc::new(new_mmap);

        // --- Pass 1: Index new lines ---
        for pos in memchr_iter(b'\n', &self.data[old_file_size..]) {
            let next = (old_file_size + pos + 1) as u64;
            self.line_offsets.push(next);
        }
        if self.line_offsets.last() != Some(&new_size) {
            self.line_offsets.push(new_size);
        }
        let new_line_count = self.total_lines_untrimmed();

        // Create dummy progress reporters since this is not a background load with UI.
        let (tx, _) = crossbeam_channel::unbounded();
        let cancel = AtomicBool::new(false);
        self.analyze_chunk(old_line_count, new_line_count, &tx, &cancel)?;

        // Update templates from the modified Drain instance
        let drain = self.drain.lock().unwrap();
        self.templates = drain
            .clusters
            .iter()
            .map(|c| TemplateInfo {
                id: c.id,
                pattern: c.pattern(),
                count: c.size,
                example_line: c.example_line,
            })
            .collect();

        // Update document state
        self.file_size = new_size;
        self.file_mtime = new_mtime;
        if self.trim_end == old_line_count {
            self.trim_end = new_line_count;
        }

        Ok(true)
    }

    /// Check the backing file without changing the mmap or any indexes.
    ///
    /// A caller should use this before `Arc::make_mut`: an unchanged poll is
    /// the common live-tail case and must not copy a shared document merely to
    /// discover that there is nothing to do.
    pub fn file_change(&self) -> Result<FileChange, String> {
        let meta = self
            .file_handle
            .metadata()
            .map_err(|e| format!("failed to get file metadata: {e}"))?;
        let size = meta.len();
        let mtime = meta
            .modified()
            .map_err(|e| format!("failed to get file modification time: {e}"))?;

        Ok(if size < self.file_size {
            FileChange::Shrunk
        } else if size > self.file_size {
            FileChange::Appended
        } else if mtime != self.file_mtime {
            FileChange::Modified
        } else {
            FileChange::Unchanged
        })
    }

    /// Blocking load (MCP server, tests). No progress reporting.
    pub fn open(path: &Path) -> Result<Self, String> {
        Self::open_with_config(path, ParsingConfig::default())
    }

    /// Blocking load with explicit parsing tunables.
    pub fn open_with_config(path: &Path, config: ParsingConfig) -> Result<Self, String> {
        Self::open_with_custom(path, config, &[])
    }

    /// Blocking load with explicit parsing tunables plus user-defined custom
    /// date recognizers to consider alongside the built-in families.
    pub fn open_with_custom(
        path: &Path,
        config: ParsingConfig,
        custom: &[CustomTimeFormat],
    ) -> Result<Self, String> {
        let (tx, _rx) = crossbeam_channel::unbounded();
        Self::load_inner(path, &tx, &AtomicBool::new(false), config, custom)
    }

    /// Load on a background thread with progress reporting and cancellation.
    pub fn load(path: &Path, tx: Sender<LoadProgress>, cancel: Arc<AtomicBool>) {
        Self::load_with_config(path, ParsingConfig::default(), tx, cancel)
    }

    /// Background load with explicit parsing tunables.
    pub fn load_with_config(
        path: &Path,
        config: ParsingConfig,
        tx: Sender<LoadProgress>,
        cancel: Arc<AtomicBool>,
    ) {
        Self::load_with_custom(path, config, &[], tx, cancel)
    }

    /// Background load with explicit tunables plus custom date formats.
    pub fn load_with_custom(
        path: &Path,
        config: ParsingConfig,
        custom: &[CustomTimeFormat],
        tx: Sender<LoadProgress>,
        cancel: Arc<AtomicBool>,
    ) {
        match Self::load_inner(path, &tx, &cancel, config, custom) {
            Ok(doc) => {
                let _ = tx.send(LoadProgress::Done(Box::new(doc)));
            }
            Err(e) => {
                let _ = tx.send(LoadProgress::Error(e));
            }
        }
    }

    /// The core analysis pipeline for a chunk of lines.
    fn analyze_chunk(
        &mut self,
        start_line: usize,
        end_line: usize,
        tx: &Sender<LoadProgress>,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        let mut last_ts = if start_line > 0 {
            self.ts_ff[start_line - 1]
        } else {
            -1
        };
        let mut min_ts = self.time_range.map_or(i64::MAX, |(min, _)| min);
        let mut max_ts = self.time_range.map_or(i64::MIN, |(_, max)| max);
        let mut last_report = if start_line > 0 {
            self.line_offsets[start_line]
        } else {
            0
        };

        let mut drain = self.drain.lock().unwrap();
        // Take the cache out of self so the per-line borrows don't conflict;
        // it's put back when the chunk finishes (early returns re-store via
        // the guard pattern below).
        let mut mask_cache = std::mem::take(&mut self.mask_cache);
        let masker = self.masker.clone();
        let header_slots = self.header_slots.clone();
        let log_format = self.log_format;
        let time_format = self.time_format.clone();
        let needed_words = end_line.div_ceil(64);
        self.record_starts.resize(needed_words, 0);

        for i in start_line..end_line {
            if i % 65_536 == 0 {
                if cancel.load(Ordering::Relaxed) {
                    return Err("load cancelled".to_string());
                }
                if self.line_offsets[i] - last_report >= PROGRESS_STRIDE {
                    last_report = self.line_offsets[i];
                    report(
                        tx,
                        LoadStage::Analyzing,
                        self.line_offsets[i],
                        self.file_size,
                    );
                }
            }

            let (line_len, ts, template_id) = {
                let line = self.line_untrimmed(i);
                let line_len = line.len();
                let ts_hint = time_format.as_ref().and_then(|e| e.extract(&line));
                // Delegate to the detected format: it strips the timestamp,
                // normalizes the structure, and masks dynamic values before
                // Drain clustering.
                let mut ctx = FormatContext {
                    masker: &masker,
                    mask_cache: &mut mask_cache,
                    header_slots: &header_slots,
                };
                let normalized = log_format.normalize(&line, ts_hint, &mut ctx);
                let template_id = drain.add_line(&normalized.content, i);
                let ts = normalized.ts.map(|(timestamp, _)| timestamp);
                (line_len, ts, template_id)
            };

            self.max_line_width = self.max_line_width.max(line_len);
            if let Some(t) = ts {
                last_ts = t;
                min_ts = min_ts.min(t);
                max_ts = max_ts.max(t);
            }
            let record_word = i / 64;
            let record_mask = 1u64 << (i % 64);
            if ts.is_some() {
                self.record_starts[record_word] |= record_mask;
            } else {
                self.record_starts[record_word] &= !record_mask;
            }
            self.ts_ff.push(last_ts);
            self.template_ids.push(template_id);
        }

        self.mask_cache = mask_cache;
        self.time_range = if min_ts <= max_ts {
            Some((min_ts, max_ts))
        } else {
            self.time_range
        };
        Ok(())
    }

    fn load_inner(
        path: &Path,
        tx: &Sender<LoadProgress>,
        cancel: &AtomicBool,
        config: ParsingConfig,
        custom: &[CustomTimeFormat],
    ) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;

        // On Windows: LockFile is mandatory, so a shared lock would block log writers
        // from appending to the file, breaking live tailing entirely.  The mmap itself
        // already prevents truncation (ERROR_USER_MAPPED_FILE), so the lock is redundant
        // there.  On Unix the lock is advisory — it prevents accidental truncation by
        // cooperating processes while still allowing appends — so we keep it.
        #[cfg(not(windows))]
        file.try_lock_shared()
            .map_err(|e| format!("failed to acquire shared lock on {}: {e}. Is another process holding an exclusive lock?", path.display()))?;

        let metadata = file
            .metadata()
            .map_err(|e| format!("cannot get file metadata: {e}"))?;
        let mtime = metadata
            .modified()
            .map_err(|e| format!("cannot get file modification time: {e}"))?;

        let file_arc = Arc::new(file);
        let mmap = unsafe { Mmap::map(&*file_arc) }.map_err(|e| format!("mmap failed: {e}"))?;
        let total = mmap.len() as u64;

        // ---- Pass 1: line-offset index via SIMD memchr ----
        let mut offsets: Vec<u64> = Vec::with_capacity((total / 48) as usize + 2);
        offsets.push(0);
        let mut last_report = 0u64;
        for pos in memchr_iter(b'\n', &mmap) {
            let next = pos as u64 + 1;
            if next < total {
                offsets.push(next);
            }
            if pos as u64 - last_report >= PROGRESS_STRIDE {
                last_report = pos as u64;
                report(tx, LoadStage::Indexing, pos as u64, total);
                if cancel.load(Ordering::Relaxed) {
                    return Err("load cancelled".to_string());
                }
            }
        }
        offsets.push(total);
        let n_lines = offsets.len() - 1;

        // ---- Format + timestamp detection on a sample ----
        let line_at = |i: usize| -> Cow<'_, str> {
            let start = offsets[i] as usize;
            let mut end = offsets[i + 1] as usize;
            while end > start && (mmap[end - 1] == b'\n' || mmap[end - 1] == b'\r') {
                end -= 1;
            }
            String::from_utf8_lossy(&mmap[start..end])
        };
        let sample: Vec<Cow<'_, str>> = (0..n_lines)
            .map(|i| line_at(i))
            .filter(|l| !l.trim().is_empty())
            .take(512)
            .collect();
        let log_format = FormatDetector::detect(sample.iter().map(|l| l.as_ref()));
        let time_format = FormatDetector::detect_time_custom(
            sample.iter().map(|l| l.as_ref()),
            log_format,
            custom,
        );

        // ---- Learn the common header shape from a sample of leading lines ----
        // (plain format only) — for each leading token position, if most
        // sampled lines carry the same *dynamic* value class there (e.g. a
        // host, pid, or thread id), that position becomes a forced mask slot.
        let header_slots = if log_format.uses_learned_header() {
            let sample_n = config.header_sample_lines.min(n_lines);
            let mut stripped: Vec<String> = Vec::with_capacity(sample_n);
            for i in 0..sample_n {
                let line = line_at(i);
                if line.trim().is_empty() {
                    continue;
                }
                let s = match time_format.as_ref().and_then(|e| e.extract(&line)) {
                    Some((_, span)) => {
                        let mut owned = line.into_owned();
                        owned.replace_range(span, "");
                        owned
                    }
                    None => line.into_owned(),
                };
                stripped.push(s);
            }
            learn_header_slots(&stripped)
        } else {
            Vec::new()
        };

        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        log::info!(
            "{}: format={} time_format={} header_slots={:?} (sampled {} lines)",
            file_name,
            log_format.name(),
            time_format
                .as_ref()
                .map_or("none".to_string(), |f| f.name()),
            header_slots,
            config.header_sample_lines
        );

        let mut doc = LogDocument {
            path: path.to_path_buf(),
            file_name,
            data: Arc::new(mmap),
            file_handle: file_arc,
            line_offsets: offsets,
            ts_ff: Vec::with_capacity(n_lines),
            record_starts: Vec::with_capacity(n_lines.div_ceil(64)),
            template_ids: Vec::with_capacity(n_lines),
            drain: Arc::new(Mutex::new(Drain::new(
                config.drain_depth,
                config.sim_threshold,
                100,
                20_000,
            ))),
            masker: LogMasker::default(),
            header_slots,
            mask_cache: MaskCache::default(),
            log_format,
            time_format,
            templates: Vec::new(),
            time_range: None,
            file_size: total,
            file_mtime: mtime,
            max_line_width: 0,
            trim_start: 0,
            trim_end: n_lines,
        };

        // --- Pass 2: analysis (timestamps + Drain templates) ----
        doc.analyze_chunk(0, n_lines, tx, cancel)?;

        {
            let drain = doc.drain.lock().unwrap();
            doc.templates = drain
                .clusters
                .iter()
                .map(|c| TemplateInfo {
                    id: c.id,
                    pattern: c.pattern(),
                    count: c.size,
                    example_line: c.example_line,
                })
                .collect();
        }

        Ok(doc)
    }
}

fn report(tx: &Sender<LoadProgress>, stage: LoadStage, done: u64, total: u64) {
    let _ = tx.send(LoadProgress::Progress { stage, done, total });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::thread;

    fn write_temp(content: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "logotomy_test_{}_{}_{}.log",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
            content.len()
        ));
        let mut f = File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn indexes_and_mines() {
        let mut content = String::new();
        for i in 0..10_000 {
            content.push_str(&format!(
                "2026-07-19T10:{:02}:{:02}.{:03}Z INFO worker-{} request id={} status=200\n",
                (i / 60) % 60,
                i % 60,
                i % 1000,
                i % 8,
                i
            ));
        }
        let path = write_temp(&content);
        let doc = LogDocument::open(&path).unwrap();
        assert_eq!(doc.total_lines(), 10_000);
        assert!(doc.time_range.is_some());
        assert!(doc.templates.len() >= 2); // at least the mined one + <EMPTY>
                                           // The template pattern should preserve the structural tokens
                                           // (INFO, request, status) while masking dynamic values.
                                           // The `worker-0` prefix is masked as `<NUM>` via header slot detection.
        assert!(
            doc.templates.iter().any(|t| t.pattern.contains("request")),
            "template should contain 'request', got patterns: {:?}",
            doc.templates.iter().map(|t| &t.pattern).collect::<Vec<_>>()
        );
        assert!(
            doc.templates.iter().any(|t| t.pattern.contains("status")),
            "template should contain 'status'"
        );
        let line = doc.line(42);
        assert!(line.contains("id=42"));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn handles_no_trailing_newline() {
        let path = write_temp("alpha\nbeta\ngamma");
        let doc = LogDocument::open(&path).unwrap();
        assert_eq!(doc.total_lines(), 3);
        assert_eq!(doc.line(2).as_ref(), "gamma");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn handles_crlf() {
        let path = write_temp("alpha\r\nbeta\r\n");
        let doc = LogDocument::open(&path).unwrap();
        assert_eq!(doc.total_lines(), 2);
        assert_eq!(doc.line(0).as_ref(), "alpha");
        assert_eq!(doc.line(1).as_ref(), "beta");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn forward_fills_timestamps() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z INFO boom\n    at stack.frame(Foo.rs:1)\n2026-07-19T10:00:01.000Z INFO next\n",
        );
        let doc = LogDocument::open(&path).unwrap();
        assert_eq!(doc.ts_at(1), doc.ts_at(0));
        assert_eq!(doc.ts_ff[1], doc.ts_ff[0]); // inherits previous
        assert!(doc.ts_ff[2] > doc.ts_ff[1]);
        std::fs::remove_file(path).ok();
    }

    // ---- trim tests ----

    #[test]
    fn trim_left_removes_lines_before() {
        let path = write_temp("line0\nline1\nline2\nline3\nline4\n");
        let mut doc = LogDocument::open(&path).unwrap();
        assert_eq!(doc.total_lines(), 5);

        doc.trim_left(2); // keep lines 2,3,4
        assert_eq!(doc.total_lines(), 3);
        assert_eq!(doc.line(0).as_ref(), "line2");
        assert_eq!(doc.line(1).as_ref(), "line3");
        assert_eq!(doc.line(2).as_ref(), "line4");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn trim_right_removes_lines_after() {
        let path = write_temp("line0\nline1\nline2\nline3\nline4\n");
        let mut doc = LogDocument::open(&path).unwrap();

        doc.trim_right(2); // keep lines 0,1,2
        assert_eq!(doc.total_lines(), 3);
        assert_eq!(doc.line(0).as_ref(), "line0");
        assert_eq!(doc.line(1).as_ref(), "line1");
        assert_eq!(doc.line(2).as_ref(), "line2");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn trim_left_and_right_compose() {
        let path = write_temp("line0\nline1\nline2\nline3\nline4\n");
        let mut doc = LogDocument::open(&path).unwrap();

        doc.trim_left(1); // keep lines 1,2,3,4
        assert_eq!(doc.total_lines(), 4);
        doc.trim_right(2); // keep lines 1,2
        assert_eq!(doc.total_lines(), 2);
        assert_eq!(doc.line(0).as_ref(), "line1");
        assert_eq!(doc.line(1).as_ref(), "line2");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn reset_trim_restores_all_lines() {
        let path = write_temp("line0\nline1\nline2\nline3\nline4\n");
        let mut doc = LogDocument::open(&path).unwrap();

        doc.trim_left(2);
        assert_eq!(doc.total_lines(), 3);
        doc.reset_trim();
        assert_eq!(doc.total_lines(), 5);
        assert_eq!(doc.line(0).as_ref(), "line0");
        assert_eq!(doc.line(4).as_ref(), "line4");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn trim_keeps_at_least_one_line() {
        let path = write_temp("only_line\n");
        let mut doc = LogDocument::open(&path).unwrap();

        doc.trim_left(0); // should be a no-op (only line)
        assert_eq!(doc.total_lines(), 1);
        doc.trim_right(0); // should be a no-op
        assert_eq!(doc.total_lines(), 1);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn trim_preserves_timestamps() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z first\n\
             2026-07-19T10:01:00.000Z second\n\
             2026-07-19T10:02:00.000Z third\n",
        );
        let mut doc = LogDocument::open(&path).unwrap();
        let original_ts = doc.ts_ff.clone();

        doc.trim_left(1); // keep lines 1,2 (original indices 1 and 2)
        assert_eq!(doc.total_lines(), 2);
        // ts_ff is indexed by original line index, so ts_ff[1] is the first visible line
        assert_eq!(doc.ts_ff[1], original_ts[1]);
        assert_eq!(doc.ts_ff[2], original_ts[2]);
        assert!(doc.time_range.is_some());
        assert_eq!(doc.time_range.unwrap().0, original_ts[1]);
        assert_eq!(doc.time_range.unwrap().1, original_ts[2]);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn trim_recalculates_template_counts() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z ERROR disk full\n\
2026-07-19T10:01:00.000Z INFO all good\n\
2026-07-19T10:00:00.000Z ERROR disk full\n",
        );
        let mut doc = LogDocument::open(&path).unwrap();
        let error_template_id = doc.template_ids[0];
        let info_template_id = doc.template_ids[1];
        assert_eq!(
            doc.templates
                .iter()
                .find(|t| t.id == error_template_id)
                .unwrap()
                .count,
            2
        );
        assert_eq!(
            doc.templates
                .iter()
                .find(|t| t.id == info_template_id)
                .unwrap()
                .count,
            1
        );

        doc.trim_right(0); // keep only line 0
        assert_eq!(doc.total_lines(), 1);

        // Template counts should be recalculated for the trimmed view.
        assert_eq!(
            doc.templates
                .iter()
                .find(|t| t.id == error_template_id)
                .unwrap()
                .count,
            1
        );
        assert_eq!(
            doc.templates
                .iter()
                .find(|t| t.id == info_template_id)
                .unwrap()
                .count,
            0
        );
        // The example_line should still reference the original line index.
        assert_eq!(
            doc.templates
                .iter()
                .find(|t| t.id == error_template_id)
                .unwrap()
                .example_line,
            0
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn trim_range_sets_visible_window() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z line0\n\
             2026-07-19T10:01:00.000Z line1\n\
             2026-07-19T10:02:00.000Z line2\n\
             2026-07-19T10:03:00.000Z line3\n\
             2026-07-19T10:04:00.000Z line4\n",
        );
        let mut doc = LogDocument::open(&path).unwrap();
        let ts2 = doc.ts_ff[2];
        let ts3 = doc.ts_ff[3];
        doc.trim_range(2, 3); // keep original lines 2,3
        assert_eq!(doc.total_lines(), 2);
        assert_eq!(doc.trim_start, 2);
        assert_eq!(doc.trim_end, 4);
        assert!(doc.line(0).contains("line2"));
        assert!(doc.line(1).contains("line3"));
        // time_range recalculated to the visible window.
        assert_eq!(doc.time_range, Some((ts2, ts3)));
        // ts_at / template_at are trim-relative.
        assert_eq!(doc.ts_at(0), ts2);
        assert_eq!(doc.ts_at(1), ts3);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn trim_range_clamps_and_noops_on_invert() {
        let path = write_temp("line0\nline1\nline2\nline3\nline4\n");
        let mut doc = LogDocument::open(&path).unwrap();

        // Start clamps to the last valid line; end clamps to the total.
        doc.trim_range(999, 999);
        assert_eq!(doc.total_lines(), 1);
        assert_eq!(doc.line(0).as_ref(), "line4");

        // Inverted range is a no-op (window preserved).
        doc.trim_range(3, 1);
        assert_eq!(doc.total_lines(), 1);
        assert_eq!(doc.line(0).as_ref(), "line4");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn trim_range_recalculates_template_counts() {
        let path = write_temp(
            "2026-07-19T10:00:00.000Z ERROR disk full\n\
             2026-07-19T10:01:00.000Z INFO all good\n\
             2026-07-19T10:02:00.000Z ERROR disk full\n",
        );
        let mut doc = LogDocument::open(&path).unwrap();
        let error_template_id = doc.template_ids[0];
        let info_template_id = doc.template_ids[1];

        doc.trim_range(1, 1); // keep only the INFO line
        assert_eq!(doc.total_lines(), 1);
        assert_eq!(
            doc.templates
                .iter()
                .find(|t| t.id == error_template_id)
                .unwrap()
                .count,
            0
        );
        assert_eq!(
            doc.templates
                .iter()
                .find(|t| t.id == info_template_id)
                .unwrap()
                .count,
            1
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn ios_style_logs_learn_header_slots() {
        // iOS Console format: `timestamp MyApp[pid:tid] <LEVEL> File.swift:N message`
        // After timestamp-stripping, slot 0 (`MyApp[pid:tid]`) should be
        // detected as a consistently-dynamic header position.
        let mut content = String::new();
        let pids = [12345i64, 91234, 45678, 78901];
        for i in 0..10_000u64 {
            let pid = pids[(i % pids.len() as u64) as usize];
            let tid = i % 16 + 1;
            let level = if i % 4 == 0 { "ERROR" } else { "INFO" };
            content.push_str(&format!(
                "2026-07-15 {:02}:{:02}:{:02}.{:06}+0300 MyApp[{}:{}] <{}> AppDelegate.swift:{} User login user_id={}\n",
                (i / 3600) % 24,
                (i / 60) % 60,
                i % 60,
                i % 1_000_000,
                pid,
                tid,
                level,
                i % 300 + 1,
                i % 99_999 + 1
            ));
        }
        let path = write_temp(&content);
        let doc = LogDocument::open(&path).unwrap();
        // Slot 0 (the `MyApp[pid:tid]` token) must be learned as a dynamic
        // mask slot — this is the core regression this test guards.
        assert!(
            doc.header_slots.first().is_some_and(|s| s.is_some()),
            "expected slot 0 to be a forced mask, got header_slots: {:?}",
            doc.header_slots
        );
        assert_eq!(doc.header_slots[0], Some(crate::core::masking::MASK_NUM));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn ios_varying_levels_reach_source_file_slot() {
        // Mirrors iOS-10K.log: the <LEVEL> position varies across 6 values,
        // which previously truncated the header scan at slot 1 — the
        // File.swift:N slot was never learned and Drain collapsed it to <*>.
        let levels = ["INFO", "DEBUG", "NOTICE", "WARNING", "ERROR", "FAULT"];
        let files = [
            "AppDelegate.swift",
            "NetworkManager.swift",
            "LocationManager.swift",
        ];
        let pids = [12345i64, 91234, 45678];
        let (doc, path) = doc_from_lines(
            |i| {
                format!(
                    "2026-07-15 22:{:02}:{:02}.{:06}+0300 MyApp[{}:{}] <{}> {}:{} Deep link handled id={}",
                    (i / 60) % 60,
                    i % 60,
                    i % 1_000_000,
                    pids[(i % pids.len() as u64) as usize],
                    i % 16 + 1,
                    levels[(i % levels.len() as u64) as usize],
                    files[(i % files.len() as u64) as usize],
                    i % 300 + 1,
                    i
                )
            },
            2_000,
        );
        // Slot 0: process token (dynamic). Slot 1: level (closed set → None).
        // Slot 2: File.swift:N (dynamic).
        assert!(
            doc.header_slots.len() >= 3,
            "expected >=3 header slots, got {:?}",
            doc.header_slots
        );
        assert_eq!(doc.header_slots[0], Some(crate::core::masking::MASK_NUM));
        assert_eq!(
            doc.header_slots[1], None,
            "level is a closed set, not a mask slot"
        );
        assert_eq!(
            doc.header_slots[2],
            Some(crate::core::masking::MASK_NUM),
            "File.swift:N slot must be learned, got {:?}",
            doc.header_slots
        );
        // Shape-aware masking: templates keep the app name and file names.
        assert!(
            doc.templates
                .iter()
                .any(|t| t.pattern.contains("MyApp[<NUM>:<NUM>]")),
            "patterns: {:?}",
            doc.templates.iter().map(|t| &t.pattern).collect::<Vec<_>>()
        );
        assert!(
            doc.templates
                .iter()
                .any(|t| t.pattern.contains("AppDelegate.swift:<NUM>")),
            "patterns: {:?}",
            doc.templates.iter().map(|t| &t.pattern).collect::<Vec<_>>()
        );
        std::fs::remove_file(path).ok();
    }

    /// Build N log lines from a template closure and open them as a document.
    fn doc_from_lines(make_line: impl Fn(u64) -> String, n: u64) -> (LogDocument, PathBuf) {
        let mut content = String::new();
        for i in 0..n {
            content.push_str(&make_line(i));
            content.push('\n');
        }
        let path = write_temp(&content);
        let doc = LogDocument::open(&path).unwrap();
        (doc, path)
    }

    /// Build a document from a fixed string (one call, no closure).
    fn doc_from_str(content: &str) -> (LogDocument, PathBuf) {
        let path = write_temp(content);
        let doc = LogDocument::open(&path).unwrap();
        (doc, path)
    }

    #[test]
    fn detects_json_format_and_extracts_field_time() {
        let (doc, path) = doc_from_str(
            "{\"time\": \"2026-08-15T19:40:01Z\", \"lvl\": 30, \"msg\": \"Page load: /v1/user\", \"env\": \"prod\"}\n\
             {\"time\": \"2026-08-15T19:40:05Z\", \"lvl\": 20, \"msg\": \"API Latency\", \"endpoint\": \"/v1/user\", \"duration\": 45}\n",
        );
        assert_eq!(doc.format_name(), "json");
        assert_eq!(
            doc.time_format_name(),
            None,
            "JSON time is field-based, not positional"
        );
        assert!(
            doc.time_range.is_some(),
            "JSON time field should populate the timeline"
        );
        assert!(
            doc.templates
                .iter()
                .any(|t| t.pattern.contains("lvl=<NUM>")),
            "patterns: {:?}",
            doc.templates.iter().map(|t| &t.pattern).collect::<Vec<_>>()
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn detects_cef_format_timeless() {
        let (doc, path) = doc_from_str(
            "CEF:0|VendorX|AppY|1.0|100|Login Success|3|suser=mkabir spt=443\n\
             CEF:0|VendorX|AppY|1.0|100|Login Success|3|suser=other spt=8443\n",
        );
        assert_eq!(doc.format_name(), "cef");
        assert_eq!(doc.time_format_name(), None, "CEF is timeless");
        assert!(doc.time_range.is_none(), "CEF has no timestamps");
        // Same CEF signature → same template; spt values masked to <NUM>.
        assert!(
            doc.templates
                .iter()
                .any(|t| t.pattern.contains("spt=<NUM>")),
            "patterns: {:?}",
            doc.templates.iter().map(|t| &t.pattern).collect::<Vec<_>>()
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn detects_rfc5424_format() {
        let (doc, path) = doc_from_str(
            "<134>1 2026-08-15T19:40:20.123Z srv-alpha auth-api 1201 tx_882 - Login successful\n\
             <134>1 2026-08-15T19:40:21.000Z srv-alpha auth-api 1201 tx_882 - Login failed for user bob\n",
        );
        assert_eq!(doc.format_name(), "rfc5424");
        assert_eq!(doc.time_format_name(), Some("ISO-8601".to_string()));
        assert!(doc.time_range.is_some());
        assert!(
            doc.templates.iter().any(|t| t.pattern.contains("RFC5424")),
            "patterns: {:?}",
            doc.templates.iter().map(|t| &t.pattern).collect::<Vec<_>>()
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn detects_os_log_unified_format() {
        let (doc, path) = doc_from_str(
            "2026-08-15 19:40:30.123456+0300 0x1a2b3c Default 0x0 12345 2 com.app: Transitioning to SettingsView for user_id=42\n\
             2026-08-15 19:40:31.123456+0300 0x1a2b3c Error 0x0 12345 2 com.app: Transitioning failed for user_id=43\n",
        );
        assert_eq!(doc.format_name(), "os_log");
        assert_eq!(doc.time_format_name(), Some("ISO-8601".to_string()));
        assert!(
            doc.time_range.is_some(),
            "ULS timestamps should populate the timeline"
        );
        assert!(
            doc.templates
                .iter()
                .any(|t| t.pattern.contains("OSLOG Default")),
            "patterns: {:?}",
            doc.templates.iter().map(|t| &t.pattern).collect::<Vec<_>>()
        );
        assert!(
            doc.templates
                .iter()
                .any(|t| t.pattern.contains("OSLOG Error")),
            "patterns: {:?}",
            doc.templates.iter().map(|t| &t.pattern).collect::<Vec<_>>()
        );
        // Thread/activity/PID columns should not leak into the template.
        assert!(
            doc.templates
                .iter()
                .all(|t| !t.pattern.contains("0x1a2b3c")),
            "patterns: {:?}",
            doc.templates.iter().map(|t| &t.pattern).collect::<Vec<_>>()
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn android_logcat_threadtime_learns_header_slots() {
        // `07-15 22:00:01.123  1234  5678 I Tag: message`
        let (doc, path) = doc_from_lines(
            |i| {
                format!(
                    "07-15 22:{:02}:{:02}.{:03}  {}  {} {} MyTag: user action id={}",
                    (i / 60) % 60,
                    i % 60,
                    i % 1000,
                    1000 + (i % 4),
                    2000 + (i % 8),
                    if i % 3 == 0 { "I" } else { "D" },
                    i
                )
            },
            2_000,
        );
        // Timestamps detected and stripped.
        assert!(
            doc.time_range.is_some(),
            "logcat timestamps should be detected"
        );
        // pid & tid slots learned as dynamic.
        assert!(
            doc.header_slots.len() >= 2,
            "expected >=2 header slots, got {:?}",
            doc.header_slots
        );
        assert_eq!(doc.header_slots[0], Some(crate::core::masking::MASK_NUM));
        assert_eq!(doc.header_slots[1], Some(crate::core::masking::MASK_NUM));
        // Template keeps the structural tag.
        assert!(
            doc.templates.iter().any(|t| t.pattern.contains("MyTag")),
            "patterns: {:?}",
            doc.templates.iter().map(|t| &t.pattern).collect::<Vec<_>>()
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn glog_logs_learn_header_slots() {
        // `I0715 22:00:01.123456 12345 server.cc:42] message`
        let (doc, path) = doc_from_lines(
            |i| {
                format!(
                    "I0715 22:{:02}:{:02}.{:06} {} server.cc:{}] request handled id={}",
                    (i / 60) % 60,
                    i % 60,
                    i % 1_000_000,
                    12000 + (i % 16),
                    i % 500 + 1,
                    i
                )
            },
            2_000,
        );
        assert!(
            doc.time_range.is_some(),
            "glog timestamps should be detected"
        );
        assert!(
            doc.header_slots.len() >= 2,
            "expected >=2 header slots, got {:?}",
            doc.header_slots
        );
        assert_eq!(doc.header_slots[0], Some(crate::core::masking::MASK_NUM));
        assert!(
            doc.templates.iter().any(|t| t.pattern.contains("request")),
            "patterns: {:?}",
            doc.templates.iter().map(|t| &t.pattern).collect::<Vec<_>>()
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn nginx_error_logs_learn_header_slots() {
        // `2024/10/10 13:55:36 [error] 1234#5678: *1 message`
        let (doc, path) = doc_from_lines(
            |i| {
                format!(
                    "2024/10/10 13:{:02}:{:02} [error] {}#{}: *{} upstream timed out",
                    (i / 60) % 60,
                    i % 60,
                    1234 + (i % 4),
                    5000 + (i % 64),
                    i
                )
            },
            2_000,
        );
        assert!(
            doc.time_range.is_some(),
            "nginx slash timestamps should be detected"
        );
        // `[error]` is a constant slot; `pid#tid:` must be dynamic.
        assert!(
            doc.header_slots.len() >= 2,
            "expected >=2 header slots, got {:?}",
            doc.header_slots
        );
        assert_eq!(doc.header_slots[0], None, "[error] should stay constant");
        assert_eq!(doc.header_slots[1], Some(crate::core::masking::MASK_NUM));
        assert!(
            doc.templates.iter().any(|t| t.pattern.contains("upstream")),
            "patterns: {:?}",
            doc.templates.iter().map(|t| &t.pattern).collect::<Vec<_>>()
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn serilog_bracketed_level_logs_learn_header_slots() {
        // Serilog-ish: `2026-07-19 10:15:30.123 +06:00 [INF] [Thread-7] message`
        // The `[Thread-7]` token must normalize to a dynamic NUM slot despite
        // brackets, and `[INF]` must stay constant even though normalized.
        let (doc, path) = doc_from_lines(
            |i| {
                format!(
                    "2026-07-19 10:{:02}:{:02}.{:03} +06:00 [INF] [worker-{}] order processed id={}",
                    (i / 60) % 60,
                    i % 60,
                    i % 1000,
                    i % 8,
                    i
                )
            },
            2_000,
        );
        assert!(
            doc.time_range.is_some(),
            "ISO timestamps should be detected"
        );
        assert!(
            doc.header_slots.len() >= 2,
            "expected >=2 header slots, got {:?}",
            doc.header_slots
        );
        assert_eq!(doc.header_slots[0], None, "[INF] should stay constant");
        assert_eq!(doc.header_slots[1], Some(crate::core::masking::MASK_NUM));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn log4j_logs_learn_header_slots() {
        // log4j/log4net: `2026-07-19 10:15:30,123 INFO [thread-3] com.app.Main - message`
        let (doc, path) = doc_from_lines(
            |i| {
                format!(
                    "2026-07-19 10:{:02}:{:02},{:03} INFO [pool-{}-thread-{}] com.app.Main - event id={}",
                    (i / 60) % 60,
                    i % 60,
                    i % 1000,
                    i % 3,
                    i % 16,
                    i
                )
            },
            2_000,
        );
        assert!(
            doc.time_range.is_some(),
            "ISO comma-millis timestamps should be detected"
        );
        assert!(
            doc.header_slots.len() >= 2,
            "expected >=2 header slots, got {:?}",
            doc.header_slots
        );
        assert_eq!(doc.header_slots[0], None, "INFO should stay constant");
        assert_eq!(doc.header_slots[1], Some(crate::core::masking::MASK_NUM));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn syslog_logs_learn_header_slots() {
        // `Jan  5 03:22:11 myhost sshd[1234]: message`
        let (doc, path) = doc_from_lines(
            |i| {
                format!(
                    "Jan  5 03:{:02}:{:02} myhost sshd[{}]: Accepted password for user{}",
                    (i / 60) % 60,
                    i % 60,
                    1000 + (i % 64),
                    i % 100
                )
            },
            2_000,
        );
        assert!(
            doc.time_range.is_some(),
            "syslog timestamps should be detected"
        );
        assert!(
            doc.header_slots.len() >= 2,
            "expected >=2 header slots, got {:?}",
            doc.header_slots
        );
        assert_eq!(doc.header_slots[0], None, "hostname should stay constant");
        assert_eq!(doc.header_slots[1], Some(crate::core::masking::MASK_NUM));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn wrapped_lines_dont_break_header_learning() {
        // 5% of lines are continuation lines (stack traces) with no header —
        // learning must still find the slots.
        let mut content = String::new();
        for i in 0..2_000u64 {
            if i % 20 == 19 {
                content.push_str("\tat com.app.Foo.bar(Foo.java:42)\n");
            }
            content.push_str(&format!(
                "2026-07-19T10:{:02}:{:02}.{:03}Z INFO worker-{} request id={}\n",
                (i / 60) % 60,
                i % 60,
                i % 1000,
                i % 8,
                i
            ));
        }
        let path = write_temp(&content);
        let doc = LogDocument::open(&path).unwrap();
        assert!(
            doc.header_slots.len() >= 2,
            "expected >=2 header slots, got {:?}",
            doc.header_slots
        );
        assert_eq!(doc.header_slots[1], Some(crate::core::masking::MASK_NUM));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn masking_improves_clustering_with_dynamic_values() {
        // Lines with different IPs, hex IDs, and numbers should cluster
        // into a single template thanks to pre-mining masking.
        let path = write_temp(
            "2026-07-19T10:00:00.000Z User user_981a2f3b logged in from 192.168.1.50\n\
             2026-07-19T10:01:00.000Z User user_ab45ef12 logged in from 10.0.0.15\n\
             2026-07-19T10:02:00.000Z User user_cd78ab90 logged in from 172.16.0.1\n\
             2026-07-19T10:03:00.000Z ERROR disk full\n",
        );
        let doc = LogDocument::open(&path).unwrap();
        assert_eq!(doc.total_lines(), 4);

        // All three "User logged in" lines should share one template
        assert_eq!(
            doc.template_ids[0], doc.template_ids[1],
            "different IPs/hex IDs should still share a template via masking"
        );
        assert_eq!(
            doc.template_ids[0], doc.template_ids[2],
            "different IPs/hex IDs should still share a template via masking"
        );

        // The ERROR line should be different
        assert_ne!(doc.template_ids[0], doc.template_ids[3]);

        // The template pattern should contain semantic masks
        let user_template = &doc
            .templates
            .iter()
            .find(|t| t.pattern.contains("User"))
            .expect("should have a User template")
            .pattern;
        assert!(user_template.contains("User"), "template: {user_template}");
        assert!(
            user_template.contains("<HEX>") || user_template.contains("<*>"),
            "template should mask hex IDs: {user_template}"
        );
        assert!(
            user_template.contains("<IP>") || user_template.contains("<*>"),
            "template should mask IPs: {user_template}"
        );

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn different_timestamps_same_content_share_template() {
        // Lines with different timestamps but same message content
        // should share a template ID because timestamps are stripped before Drain.
        let path = write_temp(
            "2026-07-19T10:00:00.000Z INFO request id=42 status=200\n\
             2026-07-19T10:01:00.000Z INFO request id=42 status=200\n\
             2026-07-20T12:00:00.000Z INFO request id=42 status=200\n\
             2026-07-19T10:02:00.000Z ERROR disk full\n\
             2026-07-19T10:03:00.000Z ERROR disk full\n",
        );
        let doc = LogDocument::open(&path).unwrap();
        assert_eq!(doc.total_lines(), 5);

        // Lines 0, 1, 2 all have the same content after timestamp-stripping
        // → they should share a template ID
        assert_eq!(
            doc.template_ids[0], doc.template_ids[1],
            "same log message with different timestamps should share template"
        );
        assert_eq!(
            doc.template_ids[0], doc.template_ids[2],
            "same log message with different timestamps should share template"
        );

        // Lines 3 and 4 have different content → different template
        assert_ne!(
            doc.template_ids[0], doc.template_ids[3],
            "different log messages should have different templates"
        );

        // Lines 3 and 4 have the same content → share template
        assert_eq!(
            doc.template_ids[3], doc.template_ids[4],
            "same log message should share template even with different timestamps"
        );

        // All lines retain their extracted timestamp through the compact
        // forward-filled timestamp index.
        for i in 0..5 {
            assert!(doc.ts_at(i) >= 0, "line {i} should have a timestamp");
        }

        // Verify the pattern is clean (no timestamp tokens in the template)
        let info_template = &doc
            .templates
            .iter()
            .find(|t| t.pattern.contains("INFO"))
            .expect("should have an INFO template")
            .pattern;
        assert!(
            !info_template.contains("2026"),
            "template '{info_template}' should not contain timestamp literals"
        );
        assert!(
            info_template.contains("INFO request"),
            "template should preserve log message: {info_template}"
        );

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn clustering_quality_no_degraded_templates() {
        // Realistic mixed workload: known event shapes with dynamic values.
        // After mining, NO template should be mostly wildcards in its head,
        // and the event shapes should land in a small number of clusters.
        let events = [
            "request completed path=/api/users status=200",
            "db query took 45ms sql=SELECT * FROM sessions",
            "cache miss for key user:1234",
            "retry attempt 3 for job sync-photos",
            "connection timeout to backend-7:8443 after 3000ms",
            "payment authorized order_id=ORD-9812 amount=99.99",
            "user login user_id=42 session=sess-8811",
        ];
        let mut content = String::new();
        for i in 0..5_000u64 {
            let event = events[(i % events.len() as u64) as usize];
            content.push_str(&format!(
                "2026-07-19T10:{:02}:{:02}.{:03}Z INFO worker-{} {}\n",
                (i / 60) % 60,
                i % 60,
                i % 1000,
                i % 8,
                event
            ));
        }
        let path = write_temp(&content);
        let doc = LogDocument::open(&path).unwrap();

        // Should collapse into roughly one template per event shape.
        assert!(
            doc.templates.len() <= events.len() + 4,
            "too many templates ({}), clustering is fragmenting: {:?}",
            doc.templates.len(),
            doc.templates.iter().map(|t| &t.pattern).collect::<Vec<_>>()
        );

        // No frequent template may be mostly wildcards.
        for t in &doc.templates {
            if t.count < 100 {
                continue;
            }
            let toks: Vec<&str> = t.pattern.split_whitespace().collect();
            let wild = toks.iter().filter(|x| **x == "<*>").count();
            assert!(
                wild * 10 <= toks.len() * 5,
                "frequent template is >50% wildcards: {t:?}"
            );
            // The first 4 tokens must not ALL be wildcards.
            let head_wild = toks.iter().take(4).filter(|x| **x == "<*>").count();
            assert!(head_wild < 4, "template head collapsed to wildcards: {t:?}");
        }
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn live_tailing_append_new_data_loads_appended_content() {
        use std::time::Duration;

        let initial_content =
            "2026-07-19T10:00:00.000Z INFO line 1\n2026-07-19T10:00:01.000Z INFO line 2\n";
        let appended_content = "2026-07-19T10:00:02.000Z WARN line 3\n";

        let path = write_temp(initial_content);

        // Main thread opens the document, acquiring a shared lock.
        let mut doc = LogDocument::open(&path).unwrap();
        assert_eq!(doc.total_lines(), 2);
        assert_eq!(doc.line(1).as_ref(), "2026-07-19T10:00:01.000Z INFO line 2");
        assert_eq!(doc.file_change().unwrap(), FileChange::Unchanged);

        let path_clone = path.clone();
        let append_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            let mut file = std::fs::File::options()
                .append(true)
                .open(&path_clone)
                .unwrap();
            file.write_all(appended_content.as_bytes()).unwrap();
        });

        append_thread.join().unwrap();

        assert_eq!(doc.file_change().unwrap(), FileChange::Appended);
        let appended = doc.append_new_data().unwrap();
        assert!(appended);

        assert_eq!(doc.total_lines(), 3);
        assert_eq!(doc.line(2).as_ref(), "2026-07-19T10:00:02.000Z WARN line 3");

        let has_warn_template = doc.templates.iter().any(|t| t.pattern.contains("WARN"));
        assert!(
            has_warn_template,
            "Templates should be re-mined to include 'WARN'"
        );

        // Check that calling it again with no new data is a no-op.
        let appended_again = doc.append_new_data().unwrap();
        assert!(
            !appended_again,
            "append_new_data should return false when no new data is available"
        );
        assert_eq!(
            doc.total_lines(),
            3,
            "line count should be unchanged after no-op append"
        );
        assert_eq!(doc.file_change().unwrap(), FileChange::Unchanged);

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn cloned_document_detaches_mining_state_for_background_append() {
        let path = write_temp("2026-07-19T10:00:00.000Z INFO first\n");
        let doc = LogDocument::open(&path).unwrap();
        let mut staged = doc.clone();
        assert!(
            !Arc::ptr_eq(&doc.drain, &staged.drain),
            "a staged append must not mutate the displayed document's Drain state"
        );
        std::fs::File::options()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"2026-07-19T10:00:01.000Z WARN second\n")
            .unwrap();
        assert!(staged.append_new_data().unwrap());
        assert_eq!(doc.total_lines(), 1);
        assert_eq!(staged.total_lines(), 2);
        std::fs::remove_file(path).ok();
    }

    // On Windows, shrinking a file requires SetEndOfFile, which the OS refuses
    // on a file with an active memory-mapped section (ERROR_USER_MAPPED_FILE).
    // The mmap itself physically prevents anyone from shrinking the file under
    // us, so this shrink-detection path is inherently untestable on Windows.
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn live_tailing_append_new_data_errors_if_file_shrinks() {
        let initial_content = "line 1\nline 2\nline 3\n";
        let path = write_temp(initial_content);
        let mut doc = LogDocument::open(&path).unwrap();
        let original_size = doc.file_size;
        let original_line_count = doc.total_lines();

        // --- Scenario: Delete last line partially ---
        let shrunk_content_1 = "line 1\nline 2\nli";
        std::fs::write(&path, shrunk_content_1).unwrap();

        let result1 = doc.append_new_data();
        assert!(result1.is_err(), "should err on partial shrink");
        assert_eq!(
            result1.unwrap_err(),
            "file has shrunk on disk; a full reload is required"
        );
        // Document state should be unchanged
        assert_eq!(doc.total_lines(), original_line_count);
        assert_eq!(doc.file_size, original_size);

        // --- Scenario: Delete one full line ---
        let shrunk_content_2 = "line 1\nline 2\n";
        std::fs::write(&path, shrunk_content_2).unwrap();

        let result2 = doc.append_new_data();
        assert!(result2.is_err(), "should err on full line shrink");

        // --- Scenario: Change a middle line by removing characters ---
        let shrunk_content_3 = "line 1\nline2\nline 3\n"; // "line 2" -> "line2"
        std::fs::write(&path, shrunk_content_3).unwrap();

        let result3 = doc.append_new_data();
        assert!(result3.is_err(), "should err on middle-of-file shrink");

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn live_tailing_append_new_data_errors_if_content_changes_but_size_is_same() {
        let initial_content = "line A\nline B\nline C\n";
        let path = write_temp(initial_content);
        let mut doc = LogDocument::open(&path).unwrap();
        assert_eq!(doc.line(1).as_ref(), "line B");

        // Sleep to ensure the modification time will be different.
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Modify a line in the middle, keeping the size the same.
        // Use an in-place write (no truncation) so this works on Windows too:
        // Windows forbids truncating a file with an active mmap, but plain
        // writes to a mapped file are allowed.
        let modified_content = "line A\nline X\nline C\n";
        assert_eq!(initial_content.len(), modified_content.len());
        let mut f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        f.write_all(modified_content.as_bytes()).unwrap();

        // append_new_data should detect an in-place modification via mtime and return an error.
        let result = doc.append_new_data();
        assert!(
            result.is_err(),
            "should return an error for in-place modification"
        );
        assert_eq!(
            result.unwrap_err(),
            "file has changed on disk; a full reload is required"
        );

        // The mmap provides a live view, but our indexes are stale.
        // The application should not use the document in this state.
        assert_eq!(
            doc.line(1).as_ref(),
            "line X",
            "mmap should reflect the live file content"
        );

        std::fs::remove_file(path).ok();
    }

    #[test]
    fn detects_12_hour_sample_log() {
        // Mirror the real sample.log bytes: leading non-breaking space (U+00A0)
        // and a narrow no-break space (U+202F) before "PM".
        let content = "\u{a0}2026-08-14 4:08:23.668\u{202f}PM [com.apple.main-thread:18836] D hi\n\
                       \u{a0}2026-08-14 4:08:24.000\u{202f}PM [com.apple.main-thread:18836] D bye\n";
        let path = write_temp(content);
        let doc = LogDocument::open(&path).unwrap();
        assert_eq!(
            doc.time_format_name(),
            Some("ISO-8601 12h AM/PM".to_string())
        );
        assert!(
            doc.time_range.is_some(),
            "12h AM/PM timestamps should populate the timeline"
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn uses_custom_date_format_when_supplied() {
        let content = "2026_08_15 10:08:00 alpha\n2026_08_15 10:08:01 beta\nno time\n";
        let path = write_temp(content);
        let def = crate::core::time::CustomDateFormat {
            name: "underscore".into(),
            regex: r"(?P<year>\d{4})_(?P<month>\d{2})_(?P<day>\d{2}) (?P<hour>\d{2}):(?P<min>\d{2}):(?P<sec>\d{2})".into(),
        };
        let custom = vec![def.compile().unwrap()];
        let doc = LogDocument::open_with_custom(&path, ParsingConfig::default(), &custom).unwrap();
        assert_eq!(doc.time_format_name(), Some("underscore".to_string()));
        assert!(doc.time_range.is_some());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn without_custom_formats_no_false_custom_detection() {
        // Same shape as the custom test, but opened without custom recognizers:
        // built-ins don't match it, so there is no timestamp.
        let content = "2026_08_15 10:08:00 alpha\n2026_08_15 10:08:01 beta\n";
        let path = write_temp(content);
        let doc = LogDocument::open(&path).unwrap();
        assert_eq!(doc.time_format_name(), None);
        assert!(doc.time_range.is_none());
        std::fs::remove_file(path).ok();
    }
}
