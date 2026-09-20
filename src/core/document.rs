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
use std::ops::{Index, IndexMut};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crossbeam_channel::Sender;
use memchr::memchr_iter;
use memmap2::{Mmap, MmapOptions};

use crate::core::drain::{Drain, DrainUndo};
use crate::core::format::{
    classify_value_with_time_key, detect_time_key, learn_header_slots, normalize_json_value,
    FormatContext, FormatDetector, LogFormat,
};
use crate::core::masking::{LogMasker, MaskCache};
use crate::core::record::{
    discovery_line_indices, resolve_profile_time, CompiledProfile, DetectionConfidence,
    DetectionDiagnostics, HeaderTime, LineTimeState, RecordClassification, RecordProfile,
    RecordStateMachine, TemplateMatch, TimeProvenance, TimestampSelection, DISCOVERY_MAX_BYTES,
    DISCOVERY_MAX_LINE_BYTES,
};
use crate::core::time::{
    CustomTimeFormat, TimeDetector, TimeFormatKind, YearlessReference, TIME_FORMATS,
};

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

/// Optional, reproducible stage timings for the production loading pipeline.
///
/// Normal loads do not collect these values, avoiding per-line clock reads in
/// the application. Benchmark/profile examples opt in through
/// [`LogDocument::open_profiled`]. Durations are stage totals rather than a
/// claim that nested work can be added to obtain wall-clock time.
#[derive(Clone, Debug, Default)]
pub struct LoadProfile {
    pub bytes: u64,
    pub physical_lines: usize,
    pub explicit_timestamps: usize,
    pub index_build: Duration,
    pub profile_discovery: Duration,
    pub header_learning: Duration,
    pub line_decode: Duration,
    pub timestamp_extract: Duration,
    pub normalization_and_masking: Duration,
    pub drain_mining: Duration,
    pub analysis_wall: Duration,
    pub total_wall: Duration,
}

/// Exact source interval changed by one append transaction. `added_lines` may
/// be zero when bytes only extend the previously unterminated physical line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppendUpdate {
    pub first_changed_line: usize,
    pub added_lines: usize,
}

#[derive(Clone)]
struct TailTransaction {
    line: usize,
    drain_undo: DrainUndo,
    prior_time_range: Option<(i64, i64)>,
    prior_invalid_count: usize,
    prior_invalid_sample_len: usize,
    prior_overflow_len: usize,
    prior_max_line_width: usize,
}

/// Report progress at most every 4 MiB so the channel stays quiet.
const PROGRESS_STRIDE: u64 = 4 * 1024 * 1024;
const INDEX_CHUNK_LEN: usize = 65_536;

/// Append-friendly copy-on-write storage for per-line indexes. A staged live
/// append shares all completed chunks with the rendered document and copies
/// at most the final partial chunk when extending it.
#[derive(Clone, Debug)]
pub struct ChunkedIndex<T> {
    chunks: Arc<Vec<Arc<Vec<T>>>>,
    len: usize,
}

impl<T> Default for ChunkedIndex<T> {
    fn default() -> Self {
        Self {
            chunks: Arc::new(Vec::new()),
            len: 0,
        }
    }
}

impl<T> ChunkedIndex<T> {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        (index < self.len).then(|| &self[index])
    }

    pub fn last(&self) -> Option<&T> {
        self.len.checked_sub(1).map(|index| &self[index])
    }

    fn iter_range(&self, range: std::ops::Range<usize>) -> impl Iterator<Item = &T> {
        range.map(|index| &self[index])
    }

    #[cfg(test)]
    fn shares_chunk_with(&self, other: &Self, index: usize) -> bool {
        let chunk = index / INDEX_CHUNK_LEN;
        self.chunks
            .get(chunk)
            .zip(other.chunks.get(chunk))
            .is_some_and(|(left, right)| Arc::ptr_eq(left, right))
    }
}

impl<T: Clone> ChunkedIndex<T> {
    pub fn push(&mut self, value: T) {
        let chunks = Arc::make_mut(&mut self.chunks);
        if let Some(last) = chunks
            .last_mut()
            .filter(|chunk| chunk.len() < INDEX_CHUNK_LEN)
        {
            Arc::make_mut(last).push(value);
        } else {
            chunks.push(Arc::new(vec![value]));
        }
        self.len += 1;
    }

    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        let chunks = Arc::make_mut(&mut self.chunks);
        let value = Arc::make_mut(chunks.last_mut()?).pop();
        if chunks.last().is_some_and(|chunk| chunk.is_empty()) {
            chunks.pop();
        }
        self.len -= 1;
        value
    }

    pub fn resize(&mut self, new_len: usize, value: T) {
        if new_len < self.len {
            let chunks = Arc::make_mut(&mut self.chunks);
            let needed_chunks = new_len.div_ceil(INDEX_CHUNK_LEN);
            chunks.truncate(needed_chunks);
            let remainder = new_len % INDEX_CHUNK_LEN;
            if remainder > 0 {
                if let Some(last) = chunks.last_mut() {
                    Arc::make_mut(last).truncate(remainder);
                }
            }
            self.len = new_len;
        } else {
            while self.len < new_len {
                self.push(value.clone());
            }
        }
    }
}

impl<T> From<Vec<T>> for ChunkedIndex<T> {
    fn from(values: Vec<T>) -> Self {
        let len = values.len();
        let mut iter = values.into_iter();
        let mut chunks = Vec::with_capacity(len.div_ceil(INDEX_CHUNK_LEN));
        loop {
            let chunk: Vec<T> = iter.by_ref().take(INDEX_CHUNK_LEN).collect();
            if chunk.is_empty() {
                break;
            }
            chunks.push(Arc::new(chunk));
        }
        Self {
            chunks: Arc::new(chunks),
            len,
        }
    }
}

impl<T> Index<usize> for ChunkedIndex<T> {
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        assert!(index < self.len, "chunked index out of bounds");
        &self.chunks[index / INDEX_CHUNK_LEN][index % INDEX_CHUNK_LEN]
    }
}

impl<T: Clone> IndexMut<usize> for ChunkedIndex<T> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        assert!(index < self.len, "chunked index out of bounds");
        let chunks = Arc::make_mut(&mut self.chunks);
        &mut Arc::make_mut(&mut chunks[index / INDEX_CHUNK_LEN])[index % INDEX_CHUNK_LEN]
    }
}

// One packed, line-relative (start, length) pair per field per record. The
// uncommon >4 GiB span uses a side table; neither path retains field strings.
const MISSING_FIELD_SPAN: u64 = u64::MAX;
const OVERFLOW_FIELD_SPAN: u64 = u64::MAX - 1;

#[derive(Clone, Debug)]
struct OverflowFieldSpan {
    ordinal: u32,
    field: u8,
    start: u64,
    len: u64,
}

#[derive(Clone, Debug)]
struct FieldCaptureStore {
    columns: Vec<ChunkedIndex<u64>>,
    overflow: Vec<OverflowFieldSpan>,
}

impl FieldCaptureStore {
    fn new(field_count: usize) -> Self {
        Self {
            columns: (0..field_count).map(|_| ChunkedIndex::default()).collect(),
            overflow: Vec::new(),
        }
    }

    fn len(&self) -> usize {
        self.columns.first().map_or(0, ChunkedIndex::len)
    }

    fn push(&mut self, matched: &TemplateMatch) {
        let ordinal = self.len() as u32;
        for (field, column) in self.columns.iter_mut().enumerate() {
            let packed = matched
                .captures
                .span_at(field)
                .map_or(MISSING_FIELD_SPAN, |span| {
                    let len = span.len();
                    if span.start < u32::MAX as usize && len <= u32::MAX as usize {
                        ((span.start as u64) << 32) | len as u64
                    } else {
                        self.overflow.push(OverflowFieldSpan {
                            ordinal,
                            field: field as u8,
                            start: span.start as u64,
                            len: len as u64,
                        });
                        OVERFLOW_FIELD_SPAN
                    }
                });
            column.push(packed);
        }
    }

    fn pop(&mut self) {
        let Some(last) = self.len().checked_sub(1) else {
            return;
        };
        for column in &mut self.columns {
            column.pop();
        }
        while self
            .overflow
            .last()
            .is_some_and(|span| span.ordinal as usize == last)
        {
            self.overflow.pop();
        }
    }

    fn span(&self, ordinal: usize, field: usize) -> Option<std::ops::Range<u64>> {
        let packed = *self.columns.get(field)?.get(ordinal)?;
        match packed {
            MISSING_FIELD_SPAN => None,
            OVERFLOW_FIELD_SPAN => {
                let index = self
                    .overflow
                    .binary_search_by_key(&(ordinal as u32, field as u8), |span| {
                        (span.ordinal, span.field)
                    })
                    .ok()?;
                let span = &self.overflow[index];
                Some(span.start..span.start.checked_add(span.len)?)
            }
            _ => {
                let start = packed >> 32;
                Some(start..start + (packed & u32::MAX as u64))
            }
        }
    }
}

fn pack_timestamp_span(span: std::ops::Range<usize>) -> u32 {
    let len = span.end.saturating_sub(span.start);
    if len == 0 || span.start > u16::MAX as usize || len > u16::MAX as usize {
        return 0;
    }
    ((span.start as u32) << 16) | len as u32
}

fn unpack_timestamp_span(packed: u32) -> Option<std::ops::Range<usize>> {
    if packed == 0 {
        return None;
    }
    let start = (packed >> 16) as usize;
    let len = (packed & u16::MAX as u32) as usize;
    Some(start..start + len)
}

fn set_packed_time_state(words: &mut ChunkedIndex<u64>, line: usize, state: LineTimeState) {
    let word_index = line / 32;
    let shift = (line % 32) * 2;
    let mask = 0b11u64 << shift;
    words[word_index] = (words[word_index] & !mask) | ((state as u64) << shift);
}

fn packed_time_state(words: &ChunkedIndex<u64>, line: usize) -> LineTimeState {
    let shift = (line % 32) * 2;
    let bits = words
        .get(line / 32)
        .map_or(0, |word| ((word >> shift) & 0b11) as u8);
    LineTimeState::from_bits(bits)
}

#[derive(Clone, Debug)]
struct OverflowTimestampSpan {
    line: u32,
    start: u64,
    len: u32,
}

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
    pub line_offsets: ChunkedIndex<u64>,
    /// Forward-filled timestamps: untimestamped lines (stack traces, etc.)
    /// inherit the previous line's time. -1 before the first known timestamp.
    /// Indexed by *original* (untrimmed) line index. Use `ts_at` for
    /// trim-relative access.
    ts_ff: ChunkedIndex<i64>,
    /// Compact bitset of physical lines that contained an explicit timestamp.
    /// It lets viewport analyzers recover the start/end of multiline records
    /// without retaining a second timestamp array (one bit per source line).
    record_starts: ChunkedIndex<u64>,
    /// Packed exact timestamp source spans, indexed by original line. The high
    /// 16 bits store the byte start and the low 16 bits store the byte length;
    /// zero means the line has no representable explicit timestamp. This keeps
    /// annotation lookup allocation-free without rerunning parsers while the
    /// UI paints.
    timestamp_spans: ChunkedIndex<u32>,
    /// Rare timestamp spans that cannot fit the packed 16-bit start/length.
    timestamp_span_overflow: Vec<OverflowTimestampSpan>,
    /// Two-bit known/missing/invalid/unknown time status per physical line.
    time_states: ChunkedIndex<u64>,
    /// Cumulative record starts before every 512-line block.
    record_rank: ChunkedIndex<u32>,
    /// Total invalid header timestamps and a bounded representative sample.
    pub invalid_time_count: usize,
    pub invalid_time_lines: Vec<u32>,
    /// Per-line Drain template cluster ID.
    /// Indexed by *original* (untrimmed) line index. Use `template_at` for
    /// trim-relative access.
    template_ids: ChunkedIndex<u32>,
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
    /// Explicit text profile, when one was selected for this document.
    record_profile: Option<Arc<CompiledProfile>>,
    /// Packed captures for explicit-profile record starts, in record order.
    field_captures: Option<FieldCaptureStore>,
    /// Reversible contribution of the final unterminated physical line.
    tail_transaction: Option<TailTransaction>,
    /// One top-level JSON event-time field, resolved once per document.
    json_time_key: Option<String>,
    /// Bounded evidence and confidence for the locked parser selection.
    pub detection: DetectionDiagnostics,
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

// A staged document shares immutable mmap data and completed index chunks, but
// must not share its mutable Drain state. Appending copy-on-writes only the
// final partial index chunks while mining remains isolated from the renderer.
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
            timestamp_spans: self.timestamp_spans.clone(),
            timestamp_span_overflow: self.timestamp_span_overflow.clone(),
            time_states: self.time_states.clone(),
            record_rank: self.record_rank.clone(),
            invalid_time_count: self.invalid_time_count,
            invalid_time_lines: self.invalid_time_lines.clone(),
            template_ids: self.template_ids.clone(),
            drain: Arc::new(Mutex::new(self.drain.lock().unwrap().clone())),
            masker: self.masker.clone(),
            header_slots: self.header_slots.clone(),
            mask_cache: self.mask_cache.clone(),
            log_format: self.log_format,
            time_format: self.time_format.clone(),
            record_profile: self.record_profile.clone(),
            field_captures: self.field_captures.clone(),
            tail_transaction: self.tail_transaction.clone(),
            json_time_key: self.json_time_key.clone(),
            detection: self.detection.clone(),
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

    /// Count record-start markers in the current trim window.
    pub fn record_count(&self) -> usize {
        self.count_records(self.trim_start, self.trim_end)
    }

    /// Fraction of physical log lines recognized as headers by an explicitly
    /// applied record profile. This deliberately uses the whole source, not
    /// the current trim, so the Format warning cannot disappear while browsing.
    pub fn record_profile_match_rate(&self) -> Option<(usize, usize)> {
        self.record_profile.as_ref().map(|_| {
            (
                self.count_records(0, self.total_lines_untrimmed()),
                self.total_lines_untrimmed(),
            )
        })
    }

    /// Count record starts in an original-source half-open line interval.
    pub fn count_records(&self, start: usize, end: usize) -> usize {
        let end = end.min(self.total_lines_untrimmed());
        let start = start.min(end);
        self.record_rank_at(end)
            .saturating_sub(self.record_rank_at(start))
    }

    /// Whether a trim-relative physical line begins a classified record.
    pub fn is_record_start_at(&self, rel: usize) -> bool {
        let Some(real) = self.trim_start.checked_add(rel) else {
            return false;
        };
        real < self.trim_end
            && self
                .record_starts
                .get(real / 64)
                .is_some_and(|word| word & (1u64 << (real % 64)) != 0)
    }

    fn record_rank_at(&self, line: usize) -> usize {
        let line = line.min(self.total_lines_untrimmed());
        let block = line / 512;
        let mut count = self.record_rank.get(block).copied().unwrap_or(0) as usize;
        let block_start = block * 512;
        for current in block_start..line {
            if self
                .record_starts
                .get(current / 64)
                .is_some_and(|word| word & (1u64 << (current % 64)) != 0)
            {
                count += 1;
            }
        }
        count
    }

    /// Logical retained bytes in the compact per-line indexes.
    /// Excludes vector capacity, chunk metadata, mmap residency, Drain/cache,
    /// filters, and GUI allocations; it is deliberately not an RSS estimate.
    pub fn index_payload_bytes(&self) -> usize {
        self.line_offsets.len() * std::mem::size_of::<u64>()
            + self.ts_ff.len() * std::mem::size_of::<i64>()
            + self.record_starts.len() * std::mem::size_of::<u64>()
            + self.timestamp_spans.len() * std::mem::size_of::<u32>()
            + self.template_ids.len() * std::mem::size_of::<u32>()
            + self.time_states.len() * std::mem::size_of::<u64>()
            + self.record_rank.len() * std::mem::size_of::<u32>()
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

    /// Explicit timestamp value and exact byte span for a trim-relative line.
    /// Continuation lines intentionally return `None` even though `ts_at`
    /// exposes their forward-filled record time.
    pub fn explicit_timestamp_at(&self, rel: usize) -> Option<(i64, std::ops::Range<usize>)> {
        let real = self.trim_start.checked_add(rel)?;
        if real >= self.trim_end {
            return None;
        }
        if self.time_provenance_untrimmed(real) != TimeProvenance::Explicit {
            return None;
        }
        let span = self.timestamp_span_untrimmed(real)?;
        Some((*self.ts_ff.get(real)?, span))
    }

    pub fn time_provenance_at(&self, rel: usize) -> Option<TimeProvenance> {
        let real = self.trim_start.checked_add(rel)?;
        (real < self.trim_end).then(|| self.time_provenance_untrimmed(real))
    }

    fn time_provenance_untrimmed(&self, real: usize) -> TimeProvenance {
        let record_start = self
            .record_starts
            .get(real / 64)
            .is_some_and(|word| word & (1u64 << (real % 64)) != 0);
        match (packed_time_state(&self.time_states, real), record_start) {
            (LineTimeState::Known, true) => TimeProvenance::Explicit,
            (LineTimeState::Known, false) => TimeProvenance::Inherited,
            (LineTimeState::MissingHeader, true) => TimeProvenance::Missing,
            (LineTimeState::InvalidHeader, true) => TimeProvenance::Invalid,
            _ => TimeProvenance::Unknown,
        }
    }

    fn timestamp_span_untrimmed(&self, real: usize) -> Option<std::ops::Range<usize>> {
        if let Some(span) = self
            .timestamp_spans
            .get(real)
            .and_then(|packed| unpack_timestamp_span(*packed))
        {
            return Some(span);
        }
        let line = u32::try_from(real).ok()?;
        let overflow = self
            .timestamp_span_overflow
            .binary_search_by_key(&line, |span| span.line)
            .ok()
            .and_then(|index| self.timestamp_span_overflow.get(index))?;
        let start = usize::try_from(overflow.start).ok()?;
        Some(start..start.saturating_add(overflow.len as usize))
    }

    pub fn record_profile(&self) -> Option<&RecordProfile> {
        self.record_profile
            .as_ref()
            .map(|compiled| compiled.profile.as_ref())
    }

    /// Ordered names and types of fields retained from the applied profile.
    pub fn record_field_schema(&self) -> Vec<(&str, &str)> {
        self.record_profile
            .as_ref()
            .map_or_else(Vec::new, |profile| {
                (0..profile.field_count())
                    .filter_map(|index| {
                        Some((profile.field_name(index)?, profile.field_type_label(index)?))
                    })
                    .collect()
            })
    }

    /// Type of a field, accepting the inline schema's documented aliases.
    pub fn record_field_type(&self, name: &str) -> Option<&'static str> {
        let profile = self.record_profile.as_ref()?;
        profile.field_type_label(profile.field_index(name)?)
    }

    /// Exact mapped-file byte span for a field on the containing record.
    /// Preamble has no owner; an empty `{log}` has an empty (not missing) span.
    pub fn record_field_span(
        &self,
        original_line: usize,
        name: &str,
    ) -> Option<std::ops::Range<u64>> {
        let profile = self.record_profile.as_ref()?;
        let field = profile.field_index(name)?;
        let start_line = self.record_start_at_or_before(original_line)?;
        if original_line >= self.total_lines_untrimmed() {
            return None;
        }
        let ordinal = self.record_rank_at(start_line);
        let relative = self.field_captures.as_ref()?.span(ordinal, field)?;
        let base = self.line_offsets[start_line];
        Some(base.checked_add(relative.start)?..base.checked_add(relative.end)?)
    }

    /// Valid explicit event time of the containing record, independent of trim.
    pub fn explicit_time_untrimmed(&self, original_line: usize) -> Option<i64> {
        let start = self.record_start_at_or_before(original_line)?;
        if original_line >= self.total_lines_untrimmed()
            || self.time_provenance_untrimmed(start) != TimeProvenance::Explicit
        {
            return None;
        }
        self.ts_ff.get(start).copied()
    }

    /// Source segments for a field. `{log}` includes subsequent continuation
    /// lines; other fields have one segment. Returned ranges borrow no text.
    pub fn record_field_segments(
        &self,
        original_line: usize,
        name: &str,
    ) -> Option<Vec<std::ops::Range<u64>>> {
        let profile = self.record_profile.as_ref()?;
        let field = profile.field_index(name)?;
        let mut segments = vec![self.record_field_span(original_line, name)?];
        if profile.field_type_label(field) == Some("message") {
            let record = self.record_range_containing(original_line)?;
            for line in record.start + 1..record.end {
                let start = self.line_offsets[line];
                let end = start + self.line_bytes_untrimmed(line).len() as u64;
                segments.push(start..end);
            }
        }
        Some(segments)
    }

    /// The part of a captured field physically present on one source line.
    /// Non-message fields live only on their header; `{log}` also spans each
    /// continuation line without building the complete segment list.
    pub fn record_field_span_on_line(
        &self,
        original_line: usize,
        name: &str,
    ) -> Option<std::ops::Range<u64>> {
        let record = self.record_range_containing(original_line)?;
        if original_line == record.start {
            return self.record_field_span(original_line, name);
        }
        if self.record_field_type(name) != Some("message") {
            return None;
        }
        let start = self.line_offsets[original_line];
        Some(start..start + self.line_bytes_untrimmed(original_line).len() as u64)
    }

    /// Read a retained single segment without copying; invalid UTF-8 remains
    /// available through the returned bytes.
    pub fn source_bytes(&self, span: std::ops::Range<u64>) -> Option<&[u8]> {
        self.data
            .get(usize::try_from(span.start).ok()?..usize::try_from(span.end).ok()?)
    }

    /// Original-file bounds of the header-delimited record containing
    /// `original_line`. The end is exclusive. Returns `None` before the first
    /// recognized record header.
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

    pub fn record_start_line(&self, ordinal: usize) -> Option<usize> {
        self.select_record_start(ordinal)
    }

    fn record_start_at_or_before(&self, line: usize) -> Option<usize> {
        let rank = self.record_rank_at(line.saturating_add(1));
        rank.checked_sub(1)
            .and_then(|ordinal| self.select_record_start(ordinal))
    }

    fn record_start_after(&self, line: usize) -> Option<usize> {
        let next = line.checked_add(1)?;
        let ordinal = self.record_rank_at(next);
        self.select_record_start(ordinal)
    }

    /// Return the zero-based `ordinal` record start. The rank directory finds
    /// its 512-line block logarithmically, then at most eight bitset words are
    /// inspected. This keeps boundary lookup bounded even for giant records.
    fn select_record_start(&self, ordinal: usize) -> Option<usize> {
        let blocks = self.record_rank.len().checked_sub(1)?;
        if ordinal >= self.record_rank.get(blocks).copied()? as usize {
            return None;
        }
        let mut low = 0usize;
        let mut high = blocks;
        while low < high {
            let middle = low + (high - low) / 2;
            if self.record_rank[middle + 1] as usize <= ordinal {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let block = low;
        let mut remaining = ordinal - self.record_rank[block] as usize;
        let first_word = block * 8;
        for word_index in first_word..(first_word + 8).min(self.record_starts.len()) {
            let mut word = self.record_starts[word_index];
            let starts = word.count_ones() as usize;
            if remaining >= starts {
                remaining -= starts;
                continue;
            }
            while remaining > 0 {
                word &= word - 1;
                remaining -= 1;
            }
            let line = word_index * 64 + word.trailing_zeros() as usize;
            return (line < self.total_lines_untrimmed()).then_some(line);
        }
        None
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
        for &t in self.ts_ff.iter_range(self.trim_start..self.trim_end) {
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

    fn rebuild_record_rank_from(&mut self, first_changed_line: usize) {
        let total = self.total_lines_untrimmed();
        let blocks = total.div_ceil(512);
        let start_block = (first_changed_line / 512).min(blocks);
        let mut count = self.record_rank.get(start_block).copied().unwrap_or(0);
        self.record_rank.resize(start_block + 1, 0);
        for block in start_block..blocks {
            let start_word = block * 8;
            for word in start_word..(start_word + 8).min(self.record_starts.len()) {
                count = count.saturating_add(self.record_starts[word].count_ones());
            }
            self.record_rank.push(count);
        }
    }

    fn undo_provisional_tail(&mut self, old_last_line: usize) -> Result<(), String> {
        let transaction = self
            .tail_transaction
            .take()
            .ok_or("missing provisional-tail transaction")?;
        if transaction.line != old_last_line {
            return Err("provisional-tail transaction is out of date".into());
        }
        self.drain.lock().unwrap().undo_line(transaction.drain_undo);
        self.ts_ff.pop();
        self.timestamp_spans.pop();
        self.template_ids.pop();
        set_packed_time_state(&mut self.time_states, old_last_line, LineTimeState::Unknown);
        self.time_states.resize(old_last_line.div_ceil(32), 0);
        let tail_mask = 1u64 << (old_last_line % 64);
        let was_header = self.record_starts[old_last_line / 64] & tail_mask != 0;
        self.record_starts[old_last_line / 64] &= !tail_mask;
        if was_header {
            if let Some(store) = self.field_captures.as_mut() {
                store.pop();
            }
        }
        self.record_starts.resize(old_last_line.div_ceil(64), 0);
        self.timestamp_span_overflow
            .truncate(transaction.prior_overflow_len);
        self.invalid_time_count = transaction.prior_invalid_count;
        self.invalid_time_lines
            .truncate(transaction.prior_invalid_sample_len);
        self.max_line_width = transaction.prior_max_line_width;
        self.time_range = transaction.prior_time_range;
        Ok(())
    }

    /// Checks for appended data and incrementally loads it.
    /// Returns `Ok(true)` if new data was loaded, `Ok(false)` if no change.
    pub fn append_new_data(&mut self) -> Result<bool, String> {
        Ok(self.append_new_data_detailed()?.is_some())
    }

    /// Append with an exact changed-row boundary for incremental consumers.
    /// A partial-tail extension reports its old row even if no line was added.
    pub fn append_new_data_detailed(&mut self) -> Result<Option<AppendUpdate>, String> {
        let change = self.file_change()?;
        match change {
            FileChange::Unchanged => return Ok(None),
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
        let was_trimmed = self.trim_start > 0 || self.trim_end < old_line_count;
        let old_tail_partial =
            old_file_size == 0 || (old_file_size > 0 && self.data[old_file_size - 1] != b'\n');
        let first_changed_line = if old_tail_partial {
            old_line_count.saturating_sub(1)
        } else {
            old_line_count
        };

        // Map the observed size before mutating any old indexes. A failed map
        // leaves the current document usable; bytes appended concurrently
        // after this snapshot belong to the next append transaction.
        let new_mmap = unsafe {
            MmapOptions::new()
                .len(new_size as usize)
                .map(&*self.file_handle)
        }
        .map_err(|e| format!("mmap failed on append: {e}"))?;

        // If the old file did NOT end with a newline, the last "line" is partial.
        // We must pop the old file size marker from the offsets list so the new
        // scan can correctly extend this partial line.
        if old_tail_partial {
            self.undo_provisional_tail(first_changed_line)?;
            self.line_offsets.pop();
        }

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
        if new_line_count > u32::MAX as usize {
            return Err("log has more than the supported 4,294,967,295 lines".to_string());
        }

        // Create dummy progress reporters since this is not a background load with UI.
        let (tx, _) = crossbeam_channel::unbounded();
        let cancel = AtomicBool::new(false);
        self.analyze_chunk(first_changed_line, new_line_count, &tx, &cancel, None)?;

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
        drop(drain);

        // Update document state
        self.file_size = new_size;
        self.file_mtime = new_mtime;
        if self.trim_end == old_line_count {
            self.trim_end = new_line_count;
        }
        if was_trimmed {
            // Trim is a visible source interval; appended records outside a
            // right trim must not leak into its time bounds or template counts.
            self.rebuild_trimmed_arrays();
        }

        Ok(Some(AppendUpdate {
            first_changed_line,
            added_lines: new_line_count.saturating_sub(old_line_count),
        }))
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
        Self::load_inner(
            path,
            &tx,
            &AtomicBool::new(false),
            config,
            custom,
            None,
            None,
            None,
            None,
        )
    }

    /// Load with an explicitly compiled text record profile and resolved date
    /// parser. Explicit selection bypasses Auto popularity thresholds.
    pub fn open_with_record_profile(
        path: &Path,
        config: ParsingConfig,
        custom: &[CustomTimeFormat],
        record_profile: Arc<CompiledProfile>,
        time_format: Option<TimeFormatKind>,
    ) -> Result<Self, String> {
        let (tx, _rx) = crossbeam_channel::unbounded();
        Self::load_inner(
            path,
            &tx,
            &AtomicBool::new(false),
            config,
            custom,
            Some(record_profile),
            time_format,
            None,
            None,
        )
    }

    /// Load with an explicitly selected record profile and a previously
    /// persisted Drain state.  This is used by the standalone template
    /// extractor so that template IDs remain stable between processes.
    pub fn open_with_record_profile_and_drain(
        path: &Path,
        config: ParsingConfig,
        custom: &[CustomTimeFormat],
        record_profile: Arc<CompiledProfile>,
        drain: Drain,
    ) -> Result<Self, String> {
        let (tx, _rx) = crossbeam_channel::unbounded();
        Self::load_inner(
            path,
            &tx,
            &AtomicBool::new(false),
            config,
            custom,
            Some(record_profile),
            None,
            None,
            Some(drain),
        )
    }

    /// Blocking load with opt-in timings from the actual production pipeline.
    /// This intentionally performs per-line clock reads and is for benchmarks,
    /// diagnostics, and regression comparisons rather than interactive loads.
    pub fn open_profiled(
        path: &Path,
        config: ParsingConfig,
        custom: &[CustomTimeFormat],
    ) -> Result<(Self, LoadProfile), String> {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut profile = LoadProfile::default();
        let document = Self::load_inner(
            path,
            &tx,
            &AtomicBool::new(false),
            config,
            custom,
            None,
            None,
            Some(&mut profile),
            None,
        )?;
        Ok((document, profile))
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
        match Self::load_inner(path, &tx, &cancel, config, custom, None, None, None, None) {
            Ok(doc) => {
                let _ = tx.send(LoadProgress::Done(Box::new(doc)));
            }
            Err(e) => {
                let _ = tx.send(LoadProgress::Error(e));
            }
        }
    }

    /// Background load with a selected record profile. The compiled choice is
    /// locked for this document and never replaced opportunistically on tail.
    pub fn load_with_record_profile(
        path: &Path,
        config: ParsingConfig,
        custom: &[CustomTimeFormat],
        record_profile: Arc<CompiledProfile>,
        tx: Sender<LoadProgress>,
        cancel: Arc<AtomicBool>,
    ) {
        match Self::load_inner(
            path,
            &tx,
            &cancel,
            config,
            custom,
            Some(record_profile),
            None,
            None,
            None,
        ) {
            Ok(doc) => {
                let _ = tx.send(LoadProgress::Done(Box::new(doc)));
            }
            Err(error) => {
                let _ = tx.send(LoadProgress::Error(error));
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
        mut profile: Option<&mut LoadProfile>,
    ) -> Result<(), String> {
        let analysis_started = Instant::now();
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
        let record_profile = self.record_profile.clone();
        let mut record_state =
            if start_line > 0 && self.record_start_at_or_before(start_line - 1).is_some() {
                let active_time = (packed_time_state(&self.time_states, start_line - 1)
                    == LineTimeState::Known)
                    .then_some(self.ts_ff[start_line - 1])
                    .filter(|time| *time >= 0);
                RecordStateMachine::with_prior_record(active_time)
            } else {
                RecordStateMachine::default()
            };
        let needed_words = end_line.div_ceil(64);
        self.record_starts.resize(needed_words, 0);
        self.time_states.resize(end_line.div_ceil(32), 0);

        for i in start_line..end_line {
            let is_provisional_tail = i + 1 == end_line
                && end_line == self.total_lines_untrimmed()
                && self.data.last().is_none_or(|byte| *byte != b'\n');
            let tail_before = is_provisional_tail.then(|| {
                (
                    (min_ts <= max_ts).then_some((min_ts, max_ts)),
                    self.invalid_time_count,
                    self.invalid_time_lines.len(),
                    self.timestamp_span_overflow.len(),
                    self.max_line_width,
                )
            });
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

            let (line_len, classified, template_id, drain_undo, template_match) = {
                let decode_started = Instant::now();
                let line = self.line_untrimmed(i);
                let line_len = line.len();
                if let Some(profile) = profile.as_deref_mut() {
                    profile.line_decode += decode_started.elapsed();
                }

                let timestamp_started = Instant::now();
                let template_match = record_profile.as_ref().and_then(|compiled| {
                    compiled.match_line(self.line_bytes_untrimmed(i), i == 0, time_format.as_ref())
                });
                // JSON classification and normalization share this decode. A
                // structured line is never parsed twice in the production
                // pass merely to establish its boundary and Drain content.
                let json_value = if record_profile.is_none() && log_format.name() == "json" {
                    serde_json::from_str::<serde_json::Value>(&line).ok()
                } else {
                    None
                };
                let classification = if let Some(matched) = template_match.as_ref() {
                    RecordClassification::from_template(
                        matched.clone(),
                        record_profile
                            .as_ref()
                            .is_some_and(|compiled| compiled.field_index("time").is_some()),
                    )
                } else if record_profile.is_some() {
                    RecordClassification::Continuation
                } else if log_format.name() == "plain" {
                    classify_plain_header(&line, time_format.as_ref())
                        .unwrap_or(RecordClassification::Continuation)
                } else if log_format.name() == "json" {
                    json_value
                        .as_ref()
                        .and_then(|value| {
                            classify_value_with_time_key(
                                value,
                                &line,
                                self.json_time_key.as_deref(),
                            )
                        })
                        .unwrap_or(RecordClassification::Continuation)
                } else {
                    log_format
                        .classify_header(&line, time_format.as_ref())
                        .unwrap_or(RecordClassification::Continuation)
                };
                let ts_hint = match &classification {
                    RecordClassification::Header {
                        time: HeaderTime::Known { value, span },
                        ..
                    } => Some((*value, span.clone())),
                    _ => None,
                };
                if let Some(profile) = profile.as_deref_mut() {
                    profile.timestamp_extract += timestamp_started.elapsed();
                }
                // Delegate to the detected format: it strips the timestamp,
                // normalizes the structure, and masks dynamic values before
                // Drain clustering.
                let normalize_started = Instant::now();
                let mut ctx = FormatContext {
                    masker: &masker,
                    mask_cache: &mut mask_cache,
                    header_slots: &header_slots,
                };
                let normalized_content = if let (Some(compiled), Some(matched)) =
                    (record_profile.as_ref(), template_match.as_ref())
                {
                    normalize_profile_header(
                        compiled,
                        matched,
                        self.line_bytes_untrimmed(i),
                        &mut ctx,
                    )
                } else if record_profile.is_some() {
                    ctx.masker.mask_with_header(&line, &[], ctx.mask_cache)
                } else if let Some(value) = json_value.as_ref() {
                    normalize_json_value(value, &line, &mut ctx).content
                } else {
                    log_format.normalize(&line, ts_hint, &mut ctx).content
                };
                if let Some(profile) = profile.as_deref_mut() {
                    profile.normalization_and_masking += normalize_started.elapsed();
                }

                let drain_started = Instant::now();
                let (template_id, drain_undo) = if is_provisional_tail {
                    let (id, undo) = drain.add_line_with_undo(&normalized_content, i);
                    (id, Some(undo))
                } else {
                    (drain.add_line(&normalized_content, i), None)
                };
                if let Some(profile) = profile.as_deref_mut() {
                    profile.drain_mining += drain_started.elapsed();
                }
                let classified = record_state.classify(&classification);
                (
                    line_len,
                    classified,
                    template_id,
                    drain_undo,
                    template_match,
                )
            };

            if let (Some(drain_undo), Some(before)) = (drain_undo, tail_before) {
                self.tail_transaction = Some(TailTransaction {
                    line: i,
                    drain_undo,
                    prior_time_range: before.0,
                    prior_invalid_count: before.1,
                    prior_invalid_sample_len: before.2,
                    prior_overflow_len: before.3,
                    prior_max_line_width: before.4,
                });
            }

            self.max_line_width = self.max_line_width.max(line_len);
            if classified.provenance == TimeProvenance::Explicit {
                if let Some(profile) = profile.as_deref_mut() {
                    profile.explicit_timestamps += 1;
                }
                let t = classified.timestamp.expect("explicit time has a value");
                min_ts = min_ts.min(t);
                max_ts = max_ts.max(t);
            }
            let record_word = i / 64;
            let record_mask = 1u64 << (i % 64);
            if classified.record_start {
                self.record_starts[record_word] |= record_mask;
                if let (Some(store), Some(matched)) =
                    (self.field_captures.as_mut(), template_match.as_ref())
                {
                    store.push(matched);
                }
            } else {
                self.record_starts[record_word] &= !record_mask;
            }
            let packed_span = classified
                .timestamp_span
                .clone()
                .map_or(0, |span| pack_timestamp_span(span.clone()));
            if packed_span == 0 {
                if let Some(span) = classified.timestamp_span.as_ref() {
                    self.timestamp_span_overflow.push(OverflowTimestampSpan {
                        line: i as u32,
                        start: span.start as u64,
                        len: span.len().min(u32::MAX as usize) as u32,
                    });
                }
            }
            self.timestamp_spans.push(packed_span);
            set_packed_time_state(&mut self.time_states, i, classified.time_state);
            if classified.time_state == LineTimeState::InvalidHeader {
                self.invalid_time_count += 1;
                if self.invalid_time_lines.len() < 64 {
                    self.invalid_time_lines.push(i as u32);
                }
            }
            self.ts_ff.push(classified.timestamp.unwrap_or(-1));
            self.template_ids.push(template_id);
        }

        drop(drain);
        self.mask_cache = mask_cache;
        self.rebuild_record_rank_from(start_line);
        if let Some(store) = &self.field_captures {
            debug_assert_eq!(store.len(), self.record_rank_at(end_line));
        }
        self.time_range = if min_ts <= max_ts {
            Some((min_ts, max_ts))
        } else {
            self.time_range
        };
        if let Some(profile) = profile.as_deref_mut() {
            profile.analysis_wall += analysis_started.elapsed();
        }
        Ok(())
    }

    fn load_inner(
        path: &Path,
        tx: &Sender<LoadProgress>,
        cancel: &AtomicBool,
        config: ParsingConfig,
        custom: &[CustomTimeFormat],
        record_profile: Option<Arc<CompiledProfile>>,
        forced_time: Option<TimeFormatKind>,
        mut profile: Option<&mut LoadProfile>,
        initial_drain: Option<Drain>,
    ) -> Result<Self, String> {
        let total_started = Instant::now();
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
        let index_started = Instant::now();
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
        if n_lines > u32::MAX as usize {
            return Err("log has more than the supported 4,294,967,295 lines".to_string());
        }
        if let Some(profile) = profile.as_deref_mut() {
            profile.bytes = total;
            profile.physical_lines = n_lines;
            profile.index_build = index_started.elapsed();
        }

        // ---- Format + timestamp detection on a sample ----
        let discovery_started = Instant::now();
        let line_at = |i: usize| -> Cow<'_, str> {
            let start = offsets[i] as usize;
            let mut end = offsets[i + 1] as usize;
            while end > start && (mmap[end - 1] == b'\n' || mmap[end - 1] == b'\r') {
                end -= 1;
            }
            String::from_utf8_lossy(&mmap[start..end])
        };
        let discovery_indexes = discovery_line_indices(n_lines);
        let mut sample: Vec<Cow<'_, str>> = Vec::with_capacity(discovery_indexes.len());
        let mut sampled_bytes = 0usize;
        let mut truncated_lines = 0usize;
        let mut byte_limit_reached = false;
        for index in discovery_indexes.iter().copied() {
            if sample.len() % 256 == 0 && cancel.load(Ordering::Relaxed) {
                return Err("load cancelled".to_string());
            }
            let start = offsets[index] as usize;
            let mut end = offsets[index + 1] as usize;
            while end > start && (mmap[end - 1] == b'\n' || mmap[end - 1] == b'\r') {
                end -= 1;
            }
            if end == start {
                continue;
            }
            let available = DISCOVERY_MAX_BYTES.saturating_sub(sampled_bytes);
            if available == 0 {
                byte_limit_reached = true;
                break;
            }
            let inspected = (end - start).min(DISCOVERY_MAX_LINE_BYTES).min(available);
            truncated_lines += usize::from(inspected < end - start);
            sample.push(String::from_utf8_lossy(&mmap[start..start + inspected]));
            sampled_bytes += inspected;
            if sampled_bytes == DISCOVERY_MAX_BYTES {
                byte_limit_reached = true;
                break;
            }
        }
        let log_format: &'static dyn LogFormat = if record_profile.is_some() {
            &crate::core::format::Plain
        } else {
            FormatDetector::detect_sparse(sample.iter().map(|l| l.as_ref()))
        };
        let (time_format, mut detection) = if let Some(forced) = forced_time {
            let mut diagnostics = DetectionDiagnostics {
                profile_id: record_profile
                    .as_ref()
                    .map(|profile| profile.profile.id.clone()),
                time_format: Some(forced.name()),
                confidence: DetectionConfidence::Explicit,
                ..DetectionDiagnostics::default()
            };
            let supporting_headers = record_profile.as_ref().map_or(0, |profile| {
                sample
                    .iter()
                    .filter(|line| profile.match_text(line, false, Some(&forced)).is_some())
                    .count()
            });
            diagnostics.supporting_headers = supporting_headers;
            (Some(forced), diagnostics)
        } else if let Some(profile) = record_profile.as_ref() {
            resolve_profile_time(profile, sample.iter().map(|line| line.as_ref()), custom)
        } else if log_format.name() == "plain" {
            detect_plain_time(sample.iter().map(|line| line.as_ref()), custom)
        } else {
            let header_lines = sample
                .iter()
                .map(|line| line.as_ref())
                .filter(|line| log_format.matches(line));
            match TimeDetector::detect_any_from_headers(
                header_lines,
                log_format.time_formats(),
                custom,
            ) {
                Some((format, hits, conflicts)) => (
                    Some(format.clone()),
                    DetectionDiagnostics {
                        profile_id: Some(format!("builtin:{}", log_format.name())),
                        time_format: Some(format.name()),
                        confidence: if hits >= 2 {
                            DetectionConfidence::High
                        } else {
                            DetectionConfidence::Low
                        },
                        supporting_headers: hits,
                        conflicting_candidates: conflicts,
                        ..DetectionDiagnostics::default()
                    },
                ),
                None => (
                    None,
                    DetectionDiagnostics {
                        profile_id: Some(format!("builtin:{}", log_format.name())),
                        confidence: if log_format.time_formats().is_empty() {
                            DetectionConfidence::High
                        } else {
                            DetectionConfidence::Unresolved
                        },
                        ..DetectionDiagnostics::default()
                    },
                ),
            }
        };
        if let Some(profile) = record_profile.as_ref() {
            if matches!(
                profile.profile.timestamp,
                TimestampSelection::BuiltIn(_) | TimestampSelection::Custom(_)
            ) && time_format.is_none()
            {
                return Err(format!(
                    "selected timestamp format is unavailable for profile {:?}",
                    profile.profile.name
                ));
            }
        }
        let reference = record_profile
            .as_ref()
            .and_then(|profile| profile.profile.yearless_year)
            .map_or_else(YearlessReference::now, |year| {
                YearlessReference::now().with_explicit_year(year)
            });
        let time_format = time_format.map(|format| format.pin_yearless(reference));
        detection.yearless_reference_year = time_format
            .as_ref()
            .and_then(TimeFormatKind::yearless_reference)
            .map(|reference| reference.year);
        detection.sampled_lines = sample.len();
        detection.sampled_bytes = sampled_bytes;
        detection.line_limit_reached = discovery_indexes.len() < n_lines;
        detection.byte_limit_reached = byte_limit_reached;
        detection.truncated_lines = truncated_lines;
        let json_time_key = (log_format.name() == "json")
            .then(|| detect_time_key(sample.iter().map(|line| line.as_ref())))
            .flatten();
        if log_format.name() == "json" {
            detection.time_format = json_time_key.as_ref().map(|key| format!("json:{key}"));
        }
        if let Some(profile) = profile.as_deref_mut() {
            profile.profile_discovery = discovery_started.elapsed();
        }

        // ---- Learn the common header shape from a sample of leading lines ----
        // (plain format only) — for each leading token position, if most
        // sampled lines carry the same *dynamic* value class there (e.g. a
        // host, pid, or thread id), that position becomes a forced mask slot.
        let header_started = Instant::now();
        let header_slots = if record_profile.is_none() && log_format.uses_learned_header() {
            let sample_n = config.header_sample_lines.min(n_lines);
            let mut stripped: Vec<String> = Vec::with_capacity(sample_n);
            for i in 0..sample_n {
                let line = line_at(i);
                if line.trim().is_empty() {
                    continue;
                }
                if let Some(RecordClassification::Header { message_span, .. }) =
                    classify_plain_header(&line, time_format.as_ref())
                {
                    stripped.push(line[message_span].to_string());
                }
            }
            learn_header_slots(&stripped)
        } else {
            Vec::new()
        };
        if let Some(profile) = profile.as_deref_mut() {
            profile.header_learning = header_started.elapsed();
        }

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
            line_offsets: offsets.into(),
            ts_ff: ChunkedIndex::default(),
            record_starts: ChunkedIndex::default(),
            timestamp_spans: ChunkedIndex::default(),
            timestamp_span_overflow: Vec::new(),
            time_states: ChunkedIndex::default(),
            record_rank: ChunkedIndex::default(),
            invalid_time_count: 0,
            invalid_time_lines: Vec::new(),
            template_ids: ChunkedIndex::default(),
            drain: Arc::new(Mutex::new(initial_drain.unwrap_or_else(|| {
                Drain::new(config.drain_depth, config.sim_threshold, 100, 20_000)
            }))),
            masker: LogMasker::default(),
            header_slots,
            mask_cache: MaskCache::default(),
            log_format,
            time_format,
            field_captures: record_profile
                .as_ref()
                .map(|profile| FieldCaptureStore::new(profile.field_count())),
            record_profile,
            tail_transaction: None,
            json_time_key,
            detection,
            templates: Vec::new(),
            time_range: None,
            file_size: total,
            file_mtime: mtime,
            max_line_width: 0,
            trim_start: 0,
            trim_end: n_lines,
        };

        // --- Pass 2: analysis (timestamps + Drain templates) ----
        doc.analyze_chunk(0, n_lines, tx, cancel, profile.as_deref_mut())?;

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

        if let Some(profile) = profile.as_deref_mut() {
            profile.total_wall = total_started.elapsed();
        }
        Ok(doc)
    }
}

fn classify_plain_header(
    line: &str,
    time_format: Option<&TimeFormatKind>,
) -> Option<RecordClassification> {
    let parser = time_format?;
    let span = parser.recognize(line)?;
    let bytes = line.as_bytes();
    let family = parser.name();
    let (header_start, timestamp_span, mut message_start) = if span.start == 0 {
        (0, span.clone(), span.end)
    } else if family == "ISO-8601 12h AM/PM"
        && span.start == '\u{a0}'.len_utf8()
        && line.starts_with('\u{a0}')
    {
        // Some Apple console exports prefix every row with a non-breaking
        // space. Treat that exact decoration like a file-format prefix; do
        // not generalize this to ordinary indentation, which is continuation
        // evidence for multiline text.
        (0, span.clone(), span.end)
    } else if span.start == 1 && bytes.first() == Some(&b'[') && bytes.get(span.end) == Some(&b']')
    {
        (0, span.clone(), span.end + 1)
    } else if family == "glog"
        && span.start == 1
        && bytes
            .first()
            .is_some_and(|byte| matches!(byte, b'I' | b'W' | b'E' | b'F' | b'D'))
    {
        (0, span.clone(), span.end)
    } else if family == "Apache CLF"
        && span.start > 0
        && bytes.get(span.start - 1) == Some(&b'[')
        && bytes.get(span.end) == Some(&b']')
        && line[..span.start - 1].split_whitespace().count() >= 2
    {
        (0, span.clone(), span.end + 1)
    } else {
        return None;
    };
    if message_start < bytes.len() && !matches!(bytes[message_start], b' ' | b'\t') {
        return None;
    }
    while bytes
        .get(message_start)
        .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
    {
        message_start += 1;
    }
    let time = match parser.extract(line) {
        Some((value, extracted)) if extracted == span => HeaderTime::Known {
            value,
            span: timestamp_span,
        },
        _ => HeaderTime::Invalid {
            span: Some(timestamp_span),
        },
    };
    Some(RecordClassification::Header {
        time,
        header_span: header_start..message_start,
        message_span: message_start..line.len(),
    })
}

fn detect_plain_time<'a>(
    sample: impl IntoIterator<Item = &'a str>,
    custom: &[CustomTimeFormat],
) -> (Option<TimeFormatKind>, DetectionDiagnostics) {
    let lines = sample.into_iter().collect::<Vec<_>>();
    let mut scores = TIME_FORMATS
        .iter()
        .map(|format| TimeFormatKind::BuiltIn(*format))
        .chain(
            custom
                .iter()
                .cloned()
                .map(|format| TimeFormatKind::Custom(Arc::new(format))),
        )
        .map(|candidate| {
            let mut hits = 0usize;
            let mut covered_bytes = 0usize;
            for line in &lines {
                if let Some(RecordClassification::Header { time, .. }) =
                    classify_plain_header(line, Some(&candidate))
                {
                    hits += 1;
                    covered_bytes += match time {
                        HeaderTime::Known { span, .. } => span.len(),
                        HeaderTime::Invalid { span } => span.map_or(0, |span| span.len()),
                        HeaderTime::Missing => 0,
                    };
                }
            }
            (candidate, hits, covered_bytes)
        })
        .filter(|(_, hits, _)| *hits > 0)
        .collect::<Vec<_>>();
    scores.sort_by(|left, right| (right.1, right.2).cmp(&(left.1, left.2)));
    let mut diagnostics = DetectionDiagnostics {
        profile_id: Some("builtin:plain-leading-time".to_string()),
        ..DetectionDiagnostics::default()
    };
    let Some((winner, hits, covered_bytes)) = scores.first().cloned() else {
        return (None, diagnostics);
    };
    diagnostics.supporting_headers = hits;
    diagnostics.conflicting_candidates = scores
        .iter()
        .skip(1)
        .filter(|(_, other_hits, other_bytes)| *other_hits == hits && *other_bytes == covered_bytes)
        .map(|(candidate, _, _)| candidate.name())
        .collect();
    if !diagnostics.conflicting_candidates.is_empty() {
        diagnostics.confidence = DetectionConfidence::Ambiguous;
        return (None, diagnostics);
    }
    diagnostics.time_format = Some(winner.name());
    diagnostics.confidence = if hits >= 2 {
        DetectionConfidence::High
    } else if lines.len() <= 4 {
        DetectionConfidence::Low
    } else {
        DetectionConfidence::Unresolved
    };
    if diagnostics.confidence == DetectionConfidence::Unresolved {
        (None, diagnostics)
    } else {
        (Some(winner), diagnostics)
    }
}

fn normalize_profile_header<'a>(
    profile: &CompiledProfile,
    matched: &crate::core::record::TemplateMatch,
    line: &'a [u8],
    context: &mut FormatContext<'_>,
) -> Cow<'a, str> {
    let message = String::from_utf8_lossy(&line[matched.message_span.clone()]);
    let masked = context
        .masker
        .mask_with_header(message.as_ref(), &[], context.mask_cache);
    let level = profile
        .captured(matched, "log_level")
        .and_then(|span| std::str::from_utf8(&line[span]).ok());
    Cow::Owned(match level {
        Some(level) if !masked.is_empty() => format!("{level} {masked}"),
        Some(level) => level.to_string(),
        None => masked.into_owned(),
    })
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
            "haystack_test_{}_{}_{}.log",
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
    fn profiled_load_uses_the_same_production_result() {
        let path = write_temp(
            "2026-09-11T10:00:00.000Z INFO first\n\
             continuation\n\
             2026-09-11T10:00:01.000Z ERROR second\n",
        );
        let regular = LogDocument::open(&path).unwrap();
        let (profiled, profile) =
            LogDocument::open_profiled(&path, ParsingConfig::default(), &[]).unwrap();

        assert_eq!(profile.bytes, regular.file_size);
        assert_eq!(profile.physical_lines, regular.total_lines_untrimmed());
        assert_eq!(profile.explicit_timestamps, regular.record_count());
        assert_eq!(
            profiled.total_lines_untrimmed(),
            regular.total_lines_untrimmed()
        );
        assert_eq!(profiled.time_range, regular.time_range);
        assert_eq!(profiled.record_count(), regular.record_count());
        assert_eq!(
            profiled.index_payload_bytes(),
            regular.index_payload_bytes()
        );
        assert!(profile.total_wall >= profile.analysis_wall);
        assert_eq!(
            profiled
                .templates
                .iter()
                .map(|template| (&template.pattern, template.count))
                .collect::<Vec<_>>(),
            regular
                .templates
                .iter()
                .map(|template| (&template.pattern, template.count))
                .collect::<Vec<_>>()
        );
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
        let (_, first_span) = doc.explicit_timestamp_at(0).unwrap();
        assert_eq!(&doc.line(0)[first_span], "2026-07-19T10:00:00.000Z");
        assert!(doc.explicit_timestamp_at(1).is_none());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn retains_exact_json_timestamp_field_span() {
        let path =
            write_temp("{\"msg\":\"2026-08-15T19:40:01Z\",\"time\":\"2026-08-15T19:40:01Z\"}\n");
        let doc = LogDocument::open(&path).unwrap();
        let (_, span) = doc.explicit_timestamp_at(0).unwrap();
        assert_eq!(&doc.line(0)[span.clone()], "2026-08-15T19:40:01Z");
        assert_eq!(
            span.start,
            doc.line(0).rfind("2026-08-15T19:40:01Z").unwrap()
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn payload_dates_do_not_split_multiline_records_or_change_time_bounds() {
        let path = write_temp(
            "2026-09-11 10:00:00.100 ERROR request failed\n\
                 payload={\"retry_at\":\"2035-01-01T00:00:00Z\"}\n\
                 at Handler.run(Handler.java:42)\n\
             2026-09-11 10:00:00.098 INFO recovery started\n\
                 upstream last_seen=2020-01-01T00:00:00Z\n\
             2026-09-11 10:00:01.000 INFO complete\n",
        );
        let doc = LogDocument::open(&path).unwrap();

        assert_eq!(doc.record_count(), 3);
        assert_eq!(doc.record_range_containing(0), Some(0..3));
        assert_eq!(doc.record_range_containing(2), Some(0..3));
        assert_eq!(doc.record_range_containing(3), Some(3..5));
        assert_eq!(doc.record_range_containing(5), Some(5..6));
        assert_eq!(doc.time_provenance_at(0), Some(TimeProvenance::Explicit));
        assert_eq!(doc.time_provenance_at(1), Some(TimeProvenance::Inherited));
        assert_eq!(doc.time_provenance_at(4), Some(TimeProvenance::Inherited));
        assert!(
            doc.ts_at(3) < doc.ts_at(0),
            "clock reversal must be preserved"
        );
        assert_eq!(doc.ts_at(1), doc.ts_at(0));
        assert_eq!(doc.ts_at(4), doc.ts_at(3));
        assert_eq!(doc.time_range, Some((doc.ts_at(3), doc.ts_at(5))));
        assert!(doc.explicit_timestamp_at(1).is_none());
        assert!(doc.explicit_timestamp_at(4).is_none());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn explicit_detailed_profile_controls_boundaries_and_spans() {
        let path = write_temp(
            "2026-09-11 10:00:00.100 - [ERROR] - worker-7 Handler.rs:42 request failed\n\
                 payload.retry_at=2035-01-01T00:00:00Z\n\
                 at Handler.run()\n\
             2026-09-11 10:00:00.098 - [INFO] - worker-12 Handler.rs:57 recovering\n",
        );
        let compiled = CompiledProfile::compile(RecordProfile::text(
            "test:detailed",
            "Detailed",
            "{time} - [{log_level}] - {thread_id} {file}:{line} {log}",
        ))
        .unwrap();
        let doc = LogDocument::open_with_record_profile(
            &path,
            ParsingConfig::default(),
            &[],
            compiled,
            Some(TimeFormatKind::BuiltIn(&crate::core::time::Iso)),
        )
        .unwrap();

        assert_eq!(doc.record_count(), 2);
        assert_eq!(doc.record_range_containing(2), Some(0..3));
        assert_eq!(doc.record_range_containing(3), Some(3..4));
        let (_, span) = doc.explicit_timestamp_at(0).unwrap();
        assert_eq!(&doc.line(0)[span], "2026-09-11 10:00:00.100");
        assert_eq!(doc.time_provenance_at(2), Some(TimeProvenance::Inherited));
        assert!(doc.ts_at(3) < doc.ts_at(0));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn malformed_profile_time_starts_unknown_record_and_resets_inheritance() {
        let path = write_temp(
            "2026-09-11 10:00:00.100 first\n\
             continuation one\n\
             2026-02-30 10:00:01.000 impossible\n\
             continuation two\n\
             2026-09-11 10:00:02.000 final\n",
        );
        let compiled = CompiledProfile::compile(RecordProfile::default_text()).unwrap();
        let doc = LogDocument::open_with_record_profile(
            &path,
            ParsingConfig::default(),
            &[],
            compiled,
            Some(TimeFormatKind::BuiltIn(&crate::core::time::Iso)),
        )
        .unwrap();

        assert_eq!(doc.record_count(), 3);
        assert_eq!(doc.time_provenance_at(2), Some(TimeProvenance::Invalid));
        assert_eq!(doc.time_provenance_at(3), Some(TimeProvenance::Unknown));
        assert_eq!(doc.ts_at(2), -1);
        assert_eq!(doc.ts_at(3), -1);
        assert_eq!(doc.invalid_time_count, 1);
        assert_eq!(doc.invalid_time_lines, vec![2]);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn rfc5424_missing_time_is_a_boundary_and_resets_inheritance() {
        let path = write_temp(
            "<34>1 2026-09-11T10:00:00Z host app 1 one - first\n\
             continuation\n\
             <34>1 - host app 2 two - no clock\n\
             continuation after missing\n\
             <34>1 2026-09-11T10:00:01Z host app 3 three - final\n",
        );
        let doc = LogDocument::open(&path).unwrap();

        assert_eq!(doc.format_name(), "rfc5424");
        assert_eq!(doc.record_count(), 3);
        assert_eq!(doc.time_provenance_at(2), Some(TimeProvenance::Missing));
        assert_eq!(doc.time_provenance_at(3), Some(TimeProvenance::Unknown));
        assert_eq!(doc.record_range_containing(3), Some(2..4));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn json_locks_one_top_level_time_key_for_the_document() {
        let path = write_temp(
            "{\"time\":\"2026-09-11T10:00:00Z\",\"msg\":\"one\"}\n\
             {\"timestamp\":\"2026-09-11T10:00:01Z\",\"msg\":\"two\"}\n\
             {\"time\":\"2026-09-11T10:00:02Z\",\"msg\":\"three\"}\n",
        );
        let doc = LogDocument::open(&path).unwrap();

        assert_eq!(doc.format_name(), "json");
        assert_eq!(doc.record_count(), 3);
        assert_eq!(doc.time_provenance_at(0), Some(TimeProvenance::Explicit));
        assert_eq!(doc.time_provenance_at(1), Some(TimeProvenance::Missing));
        assert_eq!(doc.time_provenance_at(2), Some(TimeProvenance::Explicit));
        assert_eq!(doc.ts_at(1), -1);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn timestamp_span_overflow_preserves_explicit_provenance() {
        let padding = "a".repeat(70_000);
        let content = format!(
            "{{\"padding\":\"{padding}\",\"time\":\"2026-09-11T10:00:00Z\",\"msg\":\"one\"}}\n\
             {{\"padding\":\"{padding}\",\"time\":\"2026-09-11T10:00:01Z\",\"msg\":\"two\"}}\n"
        );
        let path = write_temp(&content);
        let doc = LogDocument::open(&path).unwrap();

        let (_, span) = doc.explicit_timestamp_at(0).unwrap();
        assert!(span.start > u16::MAX as usize);
        assert_eq!(&doc.line(0)[span], "2026-09-11T10:00:00Z");
        assert_eq!(doc.time_provenance_at(0), Some(TimeProvenance::Explicit));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn rank_queries_work_across_more_than_one_directory_block() {
        let mut content = String::new();
        for line in 0..1_200 {
            if line % 400 == 0 {
                content.push_str(&format!(
                    "2026-09-11 10:00:{:02}.000 header {line}\n",
                    line / 400
                ));
            } else {
                content.push_str("continuation\n");
            }
        }
        let path = write_temp(&content);
        let compiled = CompiledProfile::compile(RecordProfile::default_text()).unwrap();
        let doc = LogDocument::open_with_record_profile(
            &path,
            ParsingConfig::default(),
            &[],
            compiled,
            Some(TimeFormatKind::BuiltIn(&crate::core::time::Iso)),
        )
        .unwrap();

        assert_eq!(doc.record_count(), 3);
        assert_eq!(doc.count_records(1, 800), 1);
        assert_eq!(doc.count_records(399, 801), 2);
        assert_eq!(doc.record_range_containing(799), Some(400..800));
        assert_eq!(doc.record_range_containing(1_199), Some(800..1_200));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn auto_detection_finds_sparse_headers_in_distributed_windows() {
        let mut content = String::from("2026-09-11 10:00:00.000 first\n");
        for line in 1..1_000 {
            content.push_str(&format!(
                "    continuation {line} retry_at=2035-01-01T00:00:00Z\n"
            ));
        }
        content.push_str("2026-09-11 10:00:01.000 second\n");
        let path = write_temp(&content);
        let doc = LogDocument::open(&path).unwrap();

        assert_eq!(doc.time_format_name(), Some("ISO-8601".to_string()));
        assert_eq!(doc.record_count(), 2);
        assert_eq!(doc.record_range_containing(999), Some(0..1_000));
        assert_eq!(doc.detection.confidence, DetectionConfidence::High);
        assert_eq!(doc.detection.supporting_headers, 2);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn explicit_profile_auto_resolves_only_its_timestamp_slot() {
        let path = write_temp(
            "[worker-7] 2026-09-11 10:00:00.000 first\n\
             payload date 2035-01-01T00:00:00Z\n\
             [worker-8] 2026-09-11 10:00:01.000 second\n",
        );
        let profile = CompiledProfile::compile(RecordProfile::text(
            "test:prefixed",
            "Prefixed",
            "[{thread_id}] {time} {log}",
        ))
        .unwrap();
        let doc = LogDocument::open_with_record_profile(
            &path,
            ParsingConfig::default(),
            &[],
            profile,
            None,
        )
        .unwrap();

        assert_eq!(doc.time_format_name(), Some("ISO-8601".to_string()));
        assert_eq!(doc.record_count(), 2);
        assert_eq!(doc.detection.profile_id.as_deref(), Some("test:prefixed"));
        assert_eq!(doc.detection.supporting_headers, 2);
        assert_eq!(doc.time_provenance_at(1), Some(TimeProvenance::Inherited));
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

    fn assert_append_matches_fresh(appended: &LogDocument, fresh: &LogDocument) {
        assert_eq!(
            appended.total_lines_untrimmed(),
            fresh.total_lines_untrimmed()
        );
        assert_eq!(appended.record_count(), fresh.record_count());
        assert_eq!(appended.time_range, fresh.time_range);
        assert_eq!(appended.invalid_time_count, fresh.invalid_time_count);
        assert_eq!(appended.invalid_time_lines, fresh.invalid_time_lines);
        assert_eq!(appended.max_line_width, fresh.max_line_width);
        for line in 0..fresh.total_lines() {
            assert_eq!(
                appended.line_bytes(line),
                fresh.line_bytes(line),
                "line {line}"
            );
            assert_eq!(appended.ts_at(line), fresh.ts_at(line), "time at {line}");
            assert_eq!(
                appended.time_provenance_at(line),
                fresh.time_provenance_at(line),
                "provenance at {line}"
            );
            assert_eq!(
                appended.explicit_timestamp_at(line),
                fresh.explicit_timestamp_at(line),
                "timestamp span at {line}"
            );
            assert_eq!(
                appended.record_range_containing(line),
                fresh.record_range_containing(line),
                "record ownership at {line}"
            );
            for (name, _) in fresh.record_field_schema() {
                assert_eq!(
                    appended.record_field_span(line, name),
                    fresh.record_field_span(line, name),
                    "capture {name} at {line}"
                );
                assert_eq!(
                    appended.record_field_segments(line, name),
                    fresh.record_field_segments(line, name),
                    "segments {name} at {line}"
                );
            }
            assert_eq!(
                appended.template_at(line),
                fresh.template_at(line),
                "mining at {line}"
            );
        }
        let templates = |doc: &LogDocument| {
            doc.templates
                .iter()
                .map(|template| {
                    (
                        template.id,
                        template.pattern.clone(),
                        template.count,
                        template.example_line,
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(templates(appended), templates(fresh), "Drain clusters");
    }

    #[test]
    fn inline_fields_retain_exact_spans_and_multiline_record_ownership() {
        let path = write_temp("preamble\n2026-07-15 22:26:39.907481+0300 MyApp[12345:9] <FAULT> CameraService.swift:295 crash\n  detail one\n2026-07-15 22:26:40.907481+0300 MyApp[2:10] <INFO> recovered\n");
        let profile = CompiledProfile::compile(RecordProfile::inline(
            "test:inline",
            "Inline",
            "{time} MyApp[{a}:{b:number}] <{loglevel}> {log}",
        ))
        .unwrap();
        let doc = LogDocument::open_with_record_profile(
            &path,
            ParsingConfig::default(),
            &[],
            profile,
            None,
        )
        .unwrap();
        let value = |line, field| {
            doc.record_field_span(line, field)
                .and_then(|span| doc.source_bytes(span))
                .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        };
        assert_eq!(doc.record_field_schema()[2], ("b", "number"));
        assert_eq!(value(0, "a"), None);
        assert_eq!(value(1, "a"), Some("12345".into()));
        assert_eq!(value(2, "a"), Some("12345".into()));
        assert_eq!(value(2, "loglevel"), Some("FAULT".into()));
        assert_eq!(value(3, "b"), Some("10".into()));
        let segments = doc.record_field_segments(2, "log").unwrap();
        assert_eq!(segments.len(), 2);
        assert_eq!(
            doc.source_bytes(segments[0].clone()),
            Some(&b"CameraService.swift:295 crash"[..])
        );
        assert_eq!(
            doc.source_bytes(segments[1].clone()),
            Some(&b"  detail one"[..])
        );
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn refined_ignore_fields_match_headers_without_creating_capture_columns() {
        let path = write_temp(
            "2026-07-15 22:26:39.907481+0300 MyApp[12345:9] first\n\
             2026-07-15 22:26:40.907481+0300 MyApp[2:10] second\n",
        );
        let profile = CompiledProfile::compile(RecordProfile::refined_inline(
            "test:refined-ignore",
            "Refined ignore",
            "{time} MyApp[{thread:number}:{ignore:number}] {log}",
        ))
        .unwrap();
        let doc = LogDocument::open_with_record_profile(
            &path,
            ParsingConfig::default(),
            &[],
            profile,
            None,
        )
        .unwrap();
        let value = |line, field| {
            doc.record_field_span(line, field)
                .and_then(|span| doc.source_bytes(span))
                .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        };

        assert_eq!(
            doc.record_field_schema(),
            &[
                ("time".into(), "timestamp".into()),
                ("thread".into(), "number".into()),
                ("log".into(), "message".into())
            ]
        );
        assert_eq!(value(0, "thread"), Some("12345".into()));
        assert_eq!(value(0, "ignore"), None);
        assert_eq!(value(1, "thread"), Some("2".into()));
        assert_eq!(value(1, "log"), Some("second".into()));
        assert_eq!(doc.record_profile_match_rate(), Some((2, 2)));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn inline_capture_columns_replay_partial_append_like_fresh_load() {
        let final_content = "2026-07-15 22:26:39.907481+0300 MyApp[12345:9] <FAULT> first\n detail\n2026-07-15 22:26:40.907481+0300 MyApp[2:10] <INFO> recovered";
        let profile = CompiledProfile::compile(RecordProfile::inline(
            "test:inline",
            "Inline",
            "{time} MyApp[{a}:{b:number}] <{loglevel}> {log}",
        ))
        .unwrap();
        for cut in 1..final_content.len() {
            if !final_content.is_char_boundary(cut) {
                continue;
            }
            let path = write_temp(&final_content[..cut]);
            let mut appended = LogDocument::open_with_record_profile(
                &path,
                ParsingConfig::default(),
                &[],
                Arc::clone(&profile),
                Some(TimeFormatKind::BuiltIn(&crate::core::time::Iso)),
            )
            .unwrap();
            File::options()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(final_content[cut..].as_bytes())
                .unwrap();
            appended.append_new_data_detailed().unwrap();
            let fresh = LogDocument::open_with_record_profile(
                &path,
                ParsingConfig::default(),
                &[],
                Arc::clone(&profile),
                Some(TimeFormatKind::BuiltIn(&crate::core::time::Iso)),
            )
            .unwrap();
            assert_append_matches_fresh(&appended, &fresh);
            std::fs::remove_file(path).ok();
        }
    }

    #[test]
    fn append_at_every_byte_boundary_matches_fresh_profiled_load() {
        let final_content = "2026-09-11T10:00:00Z ERROR first\n    payload retry_at=2035-01-01T00:00:00Z\n2026-02-30T10:00:01Z WARN impossible\n continuation\n2026-09-11T09:59:59Z INFO recovered";
        let compiled = CompiledProfile::compile(RecordProfile::default_text()).unwrap();
        for cut in 1..final_content.len() {
            let path = write_temp(&final_content[..cut]);
            let mut appended = LogDocument::open_with_record_profile(
                &path,
                ParsingConfig::default(),
                &[],
                Arc::clone(&compiled),
                Some(TimeFormatKind::BuiltIn(&crate::core::time::Iso)),
            )
            .unwrap();
            let old_count = appended.total_lines_untrimmed();
            let old_partial = final_content.as_bytes()[cut - 1] != b'\n';
            let mut file = File::options().append(true).open(&path).unwrap();
            file.write_all(final_content[cut..].as_bytes()).unwrap();
            let update = appended.append_new_data_detailed().unwrap().unwrap();
            assert_eq!(
                update.first_changed_line,
                old_count - usize::from(old_partial),
                "cut={cut}"
            );
            let fresh = LogDocument::open_with_record_profile(
                &path,
                ParsingConfig::default(),
                &[],
                Arc::clone(&compiled),
                Some(TimeFormatKind::BuiltIn(&crate::core::time::Iso)),
            )
            .unwrap();
            assert_append_matches_fresh(&appended, &fresh);
            std::fs::remove_file(path).ok();
        }
    }

    #[test]
    fn yearless_profile_keeps_explicit_year_across_append() {
        let mut profile = RecordProfile::text("test:syslog", "Syslog", "{time} {log}");
        profile.timestamp = TimestampSelection::BuiltIn("BSD syslog".into());
        profile.yearless_year = Some(2024);
        let compiled = CompiledProfile::compile(profile).unwrap();
        let path = write_temp("Jan  5 03:22:11 first\nJan  5 03:22:12 second\n");
        let mut appended = LogDocument::open_with_record_profile(
            &path,
            ParsingConfig::default(),
            &[],
            Arc::clone(&compiled),
            None,
        )
        .unwrap();
        assert_eq!(appended.detection.yearless_reference_year, Some(2024));
        assert!(crate::core::time::format_ms(appended.ts_at(0)).starts_with("2024"));

        File::options()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"Jan  5 03:22:13 third\n")
            .unwrap();
        appended.append_new_data_detailed().unwrap();
        let fresh = LogDocument::open_with_record_profile(
            &path,
            ParsingConfig::default(),
            &[],
            compiled,
            None,
        )
        .unwrap();
        assert_append_matches_fresh(&appended, &fresh);
        assert_eq!(appended.detection.yearless_reference_year, Some(2024));
        assert!(crate::core::time::format_ms(appended.ts_at(2)).starts_with("2024"));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn repeated_partial_appends_do_not_drift() {
        let final_content =
            "2026-09-11T10:00:00Z ERROR first\n continuation\n2026-09-11T10:00:01Z INFO done";
        let path = write_temp(&final_content[..1]);
        let compiled = CompiledProfile::compile(RecordProfile::default_text()).unwrap();
        let mut appended = LogDocument::open_with_record_profile(
            &path,
            ParsingConfig::default(),
            &[],
            Arc::clone(&compiled),
            Some(TimeFormatKind::BuiltIn(&crate::core::time::Iso)),
        )
        .unwrap();
        for end in 2..=final_content.len() {
            let mut file = File::options().append(true).open(&path).unwrap();
            file.write_all(&final_content.as_bytes()[end - 1..end])
                .unwrap();
            appended.append_new_data_detailed().unwrap();
            let fresh = LogDocument::open_with_record_profile(
                &path,
                ParsingConfig::default(),
                &[],
                Arc::clone(&compiled),
                Some(TimeFormatKind::BuiltIn(&crate::core::time::Iso)),
            )
            .unwrap();
            assert_append_matches_fresh(&appended, &fresh);
        }
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn empty_file_can_gain_its_first_record_through_append() {
        let path = write_temp("");
        let compiled = CompiledProfile::compile(RecordProfile::default_text()).unwrap();
        let mut appended = LogDocument::open_with_record_profile(
            &path,
            ParsingConfig::default(),
            &[],
            Arc::clone(&compiled),
            Some(TimeFormatKind::BuiltIn(&crate::core::time::Iso)),
        )
        .unwrap();
        assert_eq!(appended.total_lines(), 1);
        assert_eq!(appended.line(0).as_ref(), "");
        assert_eq!(appended.record_count(), 0);

        File::options()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"2026-09-11T10:00:00Z INFO first\n")
            .unwrap();
        let update = appended.append_new_data_detailed().unwrap().unwrap();
        assert_eq!(update.first_changed_line, 0);
        assert_eq!(update.added_lines, 0);
        let fresh = LogDocument::open_with_record_profile(
            &path,
            ParsingConfig::default(),
            &[],
            compiled,
            Some(TimeFormatKind::BuiltIn(&crate::core::time::Iso)),
        )
        .unwrap();
        assert_append_matches_fresh(&appended, &fresh);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn append_updates_record_rank_across_directory_boundary() {
        let mut content = String::new();
        for index in 0..512 {
            content.push_str(&format!("2026-09-11T10:00:00Z INFO record {index}\n"));
        }
        content.push_str("2026-09-11T10:00:01Z WARN parti");
        let path = write_temp(&content);
        let compiled = CompiledProfile::compile(RecordProfile::default_text()).unwrap();
        let mut appended = LogDocument::open_with_record_profile(
            &path,
            ParsingConfig::default(),
            &[],
            Arc::clone(&compiled),
            Some(TimeFormatKind::BuiltIn(&crate::core::time::Iso)),
        )
        .unwrap();
        let mut file = File::options().append(true).open(&path).unwrap();
        file.write_all(b"al\n2026-09-11T10:00:02Z INFO after\n")
            .unwrap();
        let update = appended.append_new_data_detailed().unwrap().unwrap();
        assert_eq!(update.first_changed_line, 512);
        assert_eq!(appended.record_count(), 514);
        assert_eq!(appended.count_records(511, 514), 3);
        let fresh = LogDocument::open_with_record_profile(
            &path,
            ParsingConfig::default(),
            &[],
            compiled,
            Some(TimeFormatKind::BuiltIn(&crate::core::time::Iso)),
        )
        .unwrap();
        assert_append_matches_fresh(&appended, &fresh);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn append_outside_right_trim_does_not_change_visible_time_bounds() {
        let path =
            write_temp("2026-09-11T10:00:00Z INFO first\n2026-09-11T10:00:01Z INFO second\n");
        let mut doc = LogDocument::open(&path).unwrap();
        doc.trim_range(0, 0);
        let original_range = doc.time_range;
        let mut file = File::options().append(true).open(&path).unwrap();
        file.write_all(b"2026-09-11T10:00:30Z INFO hidden\n")
            .unwrap();
        doc.append_new_data_detailed().unwrap();
        assert_eq!(doc.total_lines(), 1);
        assert_eq!(doc.time_range, original_range);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn partial_json_object_and_rfc_header_replay_match_fresh_loads() {
        for (initial, suffix) in [
            (
                "{\"time\":\"2026-09-11T10:00:00Z\",\"msg\":\"first\"}\n{\"time\":\"2026-09-11T10:00:01Z\",\"msg\":\"second\"}\n{\"time\":\"2026-09-11T10:00:02Z\",\"msg\":\"thi",
                "rd\"}\n",
            ),
            (
                "<134>1 2026-09-11T10:00:00Z host app 1 msg - first\n<134>1 - host app 1 msg - second\n<134>1 2026-09-11T10:00:02Z host app 1 msg - thi",
                "rd\n",
            ),
        ] {
            let path = write_temp(initial);
            let mut appended = LogDocument::open(&path).unwrap();
            let mut file = File::options().append(true).open(&path).unwrap();
            file.write_all(suffix.as_bytes()).unwrap();
            appended.append_new_data_detailed().unwrap();
            let fresh = LogDocument::open(&path).unwrap();
            assert_append_matches_fresh(&appended, &fresh);
            std::fs::remove_file(path).ok();
        }
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

    #[test]
    fn chunked_indexes_share_completed_chunks_and_copy_only_the_tail() {
        let values: Vec<u32> = (0..INDEX_CHUNK_LEN as u32 + 2).collect();
        let original: ChunkedIndex<u32> = values.into();
        let mut staged = original.clone();
        assert!(original.shares_chunk_with(&staged, 0));
        assert!(original.shares_chunk_with(&staged, INDEX_CHUNK_LEN));

        staged.push(u32::MAX);
        assert!(original.shares_chunk_with(&staged, 0));
        assert!(!original.shares_chunk_with(&staged, INDEX_CHUNK_LEN));
        assert_eq!(original.len(), INDEX_CHUNK_LEN + 2);
        assert_eq!(staged.len(), INDEX_CHUNK_LEN + 3);
        assert_eq!(staged[INDEX_CHUNK_LEN + 2], u32::MAX);
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
