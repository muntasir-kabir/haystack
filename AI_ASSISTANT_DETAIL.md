# Haystack — High-performance log analyzer & visualizer

The main shell and Log View use a restrained file-tab indicator distinct from
dock tabs. Find and Add filter expose explicit case modes and Enter hints while
retaining the existing asynchronous scans; the compact viewer toolbar keeps
long-line and per-view text-size controls beside the viewer. Detached Log Views
put Return-to-main in their dock tab rather than a separate header, and source
annotations use persistent quiet cues with hover, inspector, and context-menu
access. The main workspace fixes Log Views in its upper dock panel and
Pinned/Templates in its lower panel, while detached Log View windows remain
free-form and can receive individual Log Views through drag-and-drop.
Focus Log View temporarily hides the main-window timeline and dock peers
without persisting or changing the underlying dock layout.

## What it is
Rust tool that chews through 50MB+ log files, auto-detects the log format + timestamps, mines Drain templates, and shows an interactive timeline + virtualized log view. Also exposes an MCP server for AI assistants.

## Single binary
`haystack` is a single binary with two primary entry modes:
- `haystack` — GUI mode (eframe/egui, macOS native)
- `haystack --mcp` — stdio MCP server mode, standalone or explicitly GUI-attached (cross-platform)

`haystack --mcp-gui` remains only as a deprecated compatibility alias for old configurations.

`haystack extract_template --file FILE --store STORE --output OUTPUT [--format FORMAT]`
is an independent JSON-only batch process. It reuses the backend record parser,
masker, and Drain miner without entering or altering the GUI loading path; a
versioned JSON store restores prior template IDs before it appends new ones.

The `gui` Cargo feature gates GUI dependencies. Release builds include full features (GUI + MCP) on all platforms.

GUI launches accept file paths as arguments (`haystack path/to/app.log`). The first GUI process owns a cross-platform loopback command endpoint and lock under `~/.haystack`; later launches forward their paths to the existing window, which focuses itself and opens each path as a tab. This covers Finder, Windows Explorer/file associations, and Linux desktop-file launches.

## Repository structure
```
src/core/         — Library crate (document, record profiles/templates, embedded_data, drain, format, time parsing/query, masking, search, timeline)
src/mcp.rs        — stdio MCP server library and private GUI IPC listener
src/mcp/          — secure GUI-session manifest and authenticated IPC client
src/ui/           — egui GUI modules. `app/model.rs` separates file-level `LogTab` investigation state from per-viewport `LogViewState`; `app/tab_model.rs` contains their operations; `app/view.rs` is the main shell, and `app/overlay.rs` owns the shared modal/popover input boundary used to block base-view raw input before foreground surfaces render. `app/filters_dropdown.rs` and `app/templates.rs` provide focused popup/browser UI. `log_view/view.rs` owns row interaction and inspectors, `log_view/annotation_popup.rs` owns delayed embedded/timestamp callouts and their hover lifecycle, `log_view/analysis_popup.rs` owns the movable pin/analysis bubbles for context-menu pins and drag-selected ranges, while `log_view/highlight.rs` owns shared row text layout and highlight precedence. Timeline, filters, pin viewer, settings, custom date, icons, and theme remain in their feature folders.
src/ui/fonts/     — embedded Inter Regular UI font plus Space Mono log font (SIL OFL 1.1)
src/main.rs       — CLI dispatcher: --mcp / --mcp-gui → MCP mode, else → GUI
examples/         — bench.rs (performance benchmark), gen_ios_logs.rs (Rust iOS test log generator), gen_record_logs.rs (configurable record/timeline workload generator), profile_pipeline.rs (opt-in production-loader stage timing)
features/         — Feature retrospectives
```

## Tech stack
Rust, eframe/egui (GUI), memmap2 (mmap I/O), memchr (SIMD line indexing), aho-corasick / regex (search), chrono (timestamps), crossbeam-channel (background workers), Drain algorithm (template mining), log + env_logger (logging).

## Core behaviors
- GUI file opening routes `.zip` paths to queued background extraction (`src/ui/app/archive.rs`, `zip_import.rs`), creating a unique sibling folder before opening a multi-file picker there. Extraction is cancellable, streams file data, rejects unsafe paths/symlinks, skips macOS metadata, and removes incomplete output on failure/cancellation. ZIP support is gated behind `gui`.
- Filter updates retain a selected visible real line; if hidden, select the nearest visible line before requesting any needed Log View scroll.

## Key features & related files
| Feature | Files |
|---|---|
| Memory-mapped loading + compact, 65,536-entry copy-on-write per-line index chunks (`offset`, forward-filled record timestamp, exact timestamp span, record-start bit, two-bit time provenance, record-rank directory, template ID) + SIMD indexing; every tab stages live-tail updates, reverses/replays an amended final partial line (including Drain mining), scans filters from the first changed line, and installs atomically; yearless time assumptions are pinned per document | `src/core/document.rs`, `src/core/record/`, `src/ui/app/model.rs`, `src/ui/app/tab_model.rs` |
| Viewport-scoped embedded-data detection (JSON, key/value, debug literals, HTTP/protobuf, stack traces, encoded values; exact spans, bounded/cancellable worker, timestamp-record anchoring) | `src/core/embedded_data/{detectors,parse,model,scan}.rs`, `src/ui/log_view/embedded/highlighters/`, `src/core/document.rs`, `src/ui/app/tab_model.rs` |
| Log format detection & normalization (JSON, CEF, RFC 5424, Apple ULS, logcat brief, OSLog console, plain) | `src/core/format/` |
| Versioned record-profile schema + bounded anchored placeholder/advanced-regex compiler, sequential multiline state machine, typed explicit/inherited/missing/invalid time provenance, and rank-assisted record ownership lookup integrated with document loading | `src/core/record/`, `src/core/document.rs`, `src/core/format/` |
| One self-contained template grammar: generic fields default to tokens and typed `{ignore}` declarations match without retained captures; bounded path/text matching | `src/core/record/`, `tests/record_inline_contract.rs` |
| Bounded/distributed record and timestamp detection with explicit-choice precedence, sparse-header evidence, ambiguity/confidence diagnostics, 14 built-in families (ISO-8601, YYYY/MM/DD, MM/DD/YYYY, MM-DD-YYYY, DD-MM-YYYY, DD.MM.YYYY, YYYY.MM.DD, BSD syslog, Apache CLF, RFC 2822, epoch, logcat threadtime, glog, ISO-8601 12h AM/PM), and user-defined custom recognizers | `src/core/record/detect.rs`, `src/core/time/`, `src/core/format/` |
| Drain template mining (native Rust) | `src/core/drain.rs` |
| Standalone persistent template extraction (versioned JSON store + per-line template-ID sequence) | `src/core/template_extract.rs`, `src/main.rs` |
| Pre-mining masking (IPs, UUIDs, paths, JSON, numbers → semantic placeholders; bounded 65,536-entry token cache) | `src/core/masking.rs` |
| Advanced multi-filter search (text, regex, and typed Drain Template ID modes; per-filter case mode, include/exclude, Any/All composition, bounded validation, zoom/trim scoping, cancellable progress, compact shared `u32` GUI match lanes; unchanged matcher lanes and document-wide timeline density survive filter-set edits) | `src/core/search.rs`, `src/ui/filters/`, `src/ui/app/tab_model.rs` |
| Single-pattern find box / keyword highlight (`find_lines`, `build_find_automaton`) | `src/core/search.rs` |
| Multiple keyed Log Views per file with independent selection/scroll/Find/embedded state, shared filters/timeline/document, focused-view command routing, docked or detached lifecycle, and all-view reconciliation across filter/trim/append/reparse/MCP changes | `src/ui/app/{model,tab_model,view}.rs`, `src/ui/log_view/`, `src/ui/timeline/` |
| Persistent settings and open workspace (JSON, `~/.haystack/settings.json`), including a responsive two-column Settings window, Light/Dark/System theme migration and OS tracking, grouped Reading/Timeline/Filters controls with previews/resets, Overview minimap visibility/colour/opacity, and separate zoom/pan control visibility | `src/core/settings.rs`, `src/ui/settings/`, `src/ui/timeline/` |
| Schema-4 per-file investigation sidecars (shared filters/notes plus keyed Log View searches, anchors, focus, dock/detached layout); pre-release schemas 1-3 are safely rejected/reset, not migrated | `src/core/sidecar/`, `src/ui/app/model.rs`, `src/ui/log_view/` |
| Custom date recognizers (regex + named groups, verified live, saved to ~/.haystack/custom_date_format_list.json) | `src/core/time/custom.rs`, `src/core/settings.rs`, `src/ui/custom_date/` |
| Log-format editor with compact Name/rules/Template/Sample/Preview groups, collapsed advanced diagnostics, syntax-highlighted code-editor completion, revision-safe worker-only preview compilation, readable field preview, versioned presets, embedded per-file profile snapshots, background atomic document replacement, and explicit headless profile selection/diagnostics | `src/core/record/`, `src/core/sidecar/`, `src/ui/record_format/`, `src/ui/app/model.rs`, `src/mcp.rs` |
| Top-bar Format menu (actual applied profile/syntax, saved selection, Auto-detect, cancellable file-specific background reparse with zero-header guard) | `src/ui/app/format_menu.rs`, `src/ui/app/model.rs` |
| Compact applied-format capture columns, record-owner lookup, source byte segments, partial-tail replay | `src/core/document.rs` |
| Typed Field-mode Search/Filter expressions over captured header fields (`{log}` stays in Text/Regex modes), exact decimal/time comparisons, bounded captured-value suggestions, query persistence and captured-span highlighting | `src/core/field_query.rs`, `src/core/search.rs`, `src/ui/field_query_ui.rs`, `src/ui/{filters,log_view,app}/` |
| Focused desktop shell (`Haystack` naming, grouped primary/context/utility actions, responsive More menu below 1100px, cohesive file/loading tabs, task-oriented empty/error states, AI Assistant state menu) | `src/ui/app/{model,view}.rs`, `src/ui/settings/` |
| Shared overlay input ownership (centered modal editors/inspectors, transparent popover shields, topmost Escape handling, and raw-input suppression across main/detached views) | `src/ui/app/overlay.rs`, `src/ui/app/view.rs`, `src/ui/{log_view,record_format,custom_date,settings}/` |
| Embedded SVG action system (24×24 `currentColor` outline assets, exhaustive catalog parsing/raster tests, shared compact 22px icon-only/icon+label helpers plus emphasized primary actions, no runtime icon dependency) | `src/ui/icons.rs`, `src/ui/icons/` |
| Three selectable Timeline domains + filter lanes: Line uses source-line positions and labels; Time retains source-line positions while labeling ticks and intervals with record timestamps; Real Time uses timestamp-sorted auxiliary indexes to proportionally expose inactive gaps from the oldest through latest event while all Log Views and occurrence navigation remain in physical source order. Overview density counts record starts, filter lanes count physical matches, clicks in Real Time gaps resolve to the nearest timestamped source line, the minimap follows the selected coordinates, and line/time zooms are retained independently during a session. | `src/core/timeline.rs`, `src/ui/timeline/`, `src/ui/app/{model,tab_model}.rs` |
| Multi-tab log view (truncate / horizontal-scroll / wrap long-line modes, full-line inspector + untruncated copy, hidden-prefix search indication, interval highlighting, Shift-click filter-occurrence context with independent embedded-data scanning, compact `u32` Find/visible indexes, selection-aware minimal scrolling with distant-target centering and scroll-driven reselection; sorted-lane merge and cancellable background rebuild after lane toggles) | `src/ui/log_view/`, `src/ui/app/model.rs`, `src/ui/app/tab_model.rs`, `src/core/settings.rs` |
| Embedded-data Log View helpers (per-format cue, dedicated one-KV-per-line bounded hover callout, content-sized/resizable inspector, remembered Pretty/Tree/Raw preference, Frames, and encoded Summary/Decode overlay) | `src/ui/log_view/embedded/`, `src/ui/log_view/{annotation_popup,view}.rs`, `src/ui/app/tab_model.rs`, `src/core/settings.rs` |
| Log View advanced find + keyword highlight (text case modes, regex, typed Template ID, debounced validation, cancellable shared matcher scan, span/row highlighting, and filter promotion) | `src/core/search.rs`, `src/ui/app/tab_model.rs`, `src/ui/log_view/{view,highlight}.rs` |
| Dockable Log/Pinned/Templates views (standalone dock add button, per-tab External Window actions, filename-aware detached titles, compact shared/per-view scope chrome, Return-to-main/remove actions, independent dock panels/layout, and dock-layout restoration) and cached Templates browsing/sort/search/navigation actions | `src/ui/template_view/`, `src/ui/app/`, `src/core/document.rs` |
| Central application command registry/listener (shortcuts, top-bar Commands palette, keyboard/mouse cheat sheet, go-to navigation, tab cycling, deletion undo) | `src/ui/app/key_listener.rs`, `src/ui/app/view.rs`, `src/ui/app/tab_model.rs` |
| Embedded Inter Regular UI font and Space Mono log font — baked into the binary (`include_bytes!`); Inter is installed as egui's default UI face while Space Mono is registered under a dedicated family for log text and pinned lines (SIL OFL 1.1, `OFL.txt` in each font directory) | `src/ui/fonts/`, `src/ui/log_view/`, `src/ui/pin_viewer/` |
| Filter bookmarks (grouped Filter/Add controls + chip row; positively worded confirmation preference mapped to the existing persisted inverse) | `src/ui/filters/`, `src/ui/settings/` |
| Pinned lines + analyses bottom panel (per-card Edit reopens the pin window; Timeline marker navigation scrolls the selected card into view) | `src/ui/pin_viewer/` |
| Scroll-position indicator bar + right-click copy/pin/trim menu, row/timeline export | `src/ui/log_view/` |
| Token-driven dark/light theme (shared canvas, reading/elevated surfaces, interaction states, severity/search/filter/embedded cues across egui, dock, and custom painters) | `src/ui/theme.rs`, `src/ui/app/` |
| MCP server (dual-era stdio JSON-RPC; standalone and GUI-attached modes; bounded keyword-match LRU and invalidation-driven filtered-union cache) | `src/mcp.rs` |
| Compression-first MCP tools (summarize_log with budget bytes, get_timeline_histogram, get_template_anomalies, get_template, get_template_samples, source-ordered log_sequence dense triples + collapse, raw_log with explicit line mapping; exact discontiguous time predicates) + filter tools (filters_get/filters_add/filters_remove), GUI Pin-tab analysis tools (get_analysis/add_analysis), and per-tool `with_filtered_log` (default true = run on the filtered log; Everything Else lane excluded; zero filters + true short-circuits with a `{"comment":"no log"}` hint) | `src/core/time_query.rs`, `src/mcp.rs`, `src/ui/app/`, `src/ui/pin_viewer/` |
| MCP GUI controls and Codex/Claude/Cline integration (one AI Assistant status menu with state-appropriate Start/Stop and copy action; one permanent `--mcp` config, random 12-character hexadecimal temporary session ID, authenticated private IPC, session-only attach instruction) | `src/mcp/`, `src/ui/app/`, `src/ui/settings/` |
| AI assistant integration popup | `src/ui/settings/`, `src/ui/app/` |

## Tests
500+ tests across `src/core/` (including embedded-data and timestamp-source detection), `src/mcp.rs`, UI models/views (including SVG, responsive-shell, preference-mapping, and popup contracts), and integration fixture contracts. `tests/fixtures/record_parsing/oracle.json` is the human-authored correctness oracle for the record/profile rollout; Phase 0 validates the oracle itself without preserving known-broken parser output. Run with `cargo test`.

Tracked README screenshots can be regenerated without OS screen-recording permission by setting `HAYSTACK_SCREENSHOT_PATH`, `HAYSTACK_SCREENSHOT_THEME=light|dark`, and optionally `HAYSTACK_SCREENSHOT_VIEW=log|multiple-logs|pinned|templates` when launching the GUI. The `multiple-logs` fixture adds a second keyed view in the same Log leaf. The renderer waits for loading and background searches to settle, captures the viewport, and exits without changing the persisted theme.

## Benchmark
`cargo run --release --example bench -- [logfile] [filters...]` — with no args, generates a 64MB/787k-line synthetic log (load ~3.3s, 3-filter scan ~0.7s, timeline build ~13ms). Pass a path to bench a real file, e.g. `cargo run --release --example bench -- examples/iOS-100K.log ERROR user_id`. Output includes record markers and the logical payload of the compact per-line indexes (not RSS).

## Test logs
`cargo run --release --example gen_ios_logs -- [SIZES...]` generates deterministic iOS-style app logs. The generator is pure Rust (seeded PCG64), so the same `--seed` produces byte-identical output on every platform — no Python, no cross-version drift. It produces realistic iOS-style logs with level distribution (5% ERROR, 1% FAULT, 10% WARNING, 14% NOTICE, 45% INFO, 25% DEBUG), ~200 token-parametrized message templates, bursty timestamps, multiple PIDs/threads, and multi-frame FAULT stack traces. Outputs `iOS-1K.log` (~1K lines), `iOS-10K.log` (~10K lines), `iOS-100K.log` (~100K lines), `iOS-1M.log` (~1M lines). Every size starts from the same seed, so smaller files are exact prefixes of larger ones. Pass sizes as args (`cargo run --release --example gen_ios_logs -- 100K 1M`), use `--all` for all four, or `--seed N` to override the RNG.

`cargo run --release --example profile_pipeline -- [logfile]` uses `LogDocument::open_profiled` to instrument the real production loading path. Normal application loads do not collect per-line timings. `cargo run --release --example gen_record_logs -- --help` generates fixed-seed workloads with configurable physical-line count, continuations, header layout, date family, backward-clock frequency, and payload size.

## Cross-platform builds
Releases are built automatically via the GitHub Actions workflow (`.github/workflows/release.yml`) when a `v*` tag is pushed. It builds full-featured binaries (GUI + MCP) natively on each platform's own runner (no cross-compilation), for:
- **Windows** x86_64 (`windows-latest`) → **NSIS installer** `haystack-<version>-setup.exe` (the runnable `.exe` is `haystack.exe`, icon + version info embedded at build time)
- **Linux** x86_64 (`ubuntu-latest`) → **`.deb` + `.AppImage`** installers
- **macOS** Apple Silicon (`macos-latest`) + Intel → **`.dmg`** installers (+ `.app` bundle)

Installers replace the old raw `tar.gz`/`zip` archives. Packaging is done by [cargo-packager](https://github.com/crabnebula-dev/cargo-packager) using the `[package.metadata.packager]` table in `Cargo.toml`; the app icon (128×128, plus 256/512 and `haystack.ico`) lives in `assets/icons/`. The installers associate `.log`, `.txt`, `.out`, `.err`, `.csv`, `.json`, `.xml`, and `.md` with the GUI. Manual per-OS "build binary → make installer" steps and output locations: see `docs/release.md` (`scripts/package-release.sh` shells out to the same two commands).

Each release build also runs the full test suite (main app + `gen_ios_logs` example) and a benchmark against a generated `iOS-100K.log` before packaging. CI (`.github/workflows/rust.yml`) runs the same checks on `ubuntu-latest`/`macos-latest`/`windows-latest` for every push/PR to `main`.

### Windows file-lock semantics
Windows `LockFile` is mandatory (blocks all writers), unlike Unix advisory locks. `load_inner()` therefore skips `try_lock_shared()` on Windows — the mmap itself prevents truncation (`ERROR_USER_MAPPED_FILE`). Tests that need to shrink files are gated with `#[cfg(not(target_os = "windows"))]`; same-size content-change tests use in-place writes (`OpenOptions::new().write(true)`) instead of `std::fs::write` (which truncates).
