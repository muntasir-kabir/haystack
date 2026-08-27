# logotomy — High-performance log analyzer & visualizer

## What it is
Rust tool that chews through 50MB+ log files, auto-detects the log format + timestamps, mines Drain templates, and shows an interactive timeline + virtualized log view. Also exposes an MCP server for AI assistants.

## Single binary
`logotomy` is a single binary with two primary entry modes:
- `logotomy` — GUI mode (eframe/egui, macOS native)
- `logotomy --mcp` — stdio MCP server mode, standalone or explicitly GUI-attached (cross-platform)

`logotomy --mcp-gui` remains only as a deprecated compatibility alias for old configurations.

The `gui` Cargo feature gates GUI dependencies. Release builds include full features (GUI + MCP) on all platforms.

GUI launches accept file paths as arguments (`logotomy path/to/app.log`). The first GUI process owns a cross-platform loopback command endpoint and lock under `~/.logotomy`; later launches forward their paths to the existing window, which focuses itself and opens each path as a tab. This covers Finder, Windows Explorer/file associations, and Linux desktop-file launches.

## Repository structure
```
src/core/         — Library crate (document, embedded_data, drain, format, time, masking, search, timeline)
src/mcp.rs        — stdio MCP server library and private GUI IPC listener
src/mcp/          — secure GUI-session manifest and authenticated IPC client
src/ui/           — egui GUI modules. `app/model.rs` holds shared state definitions and app-level behavior; `app/tab_model.rs` contains `LogTab` operations; `app/view.rs` is the main shell, with `app/filters_dropdown.rs` and `app/templates.rs` for focused popup/browser UI. `log_view/view.rs` owns row interaction and inspectors, while `log_view/highlight.rs` owns shared row text layout and highlight precedence. Timeline, filters, pin viewer, settings, custom date, icons, and theme remain in their feature folders.
src/ui/fonts/     — embedded Space Mono monospace font (log text only, SIL OFL 1.1)
src/main.rs       — CLI dispatcher: --mcp / --mcp-gui → MCP mode, else → GUI
examples/         — bench.rs (performance benchmark), gen_ios_logs.rs (Rust iOS test log generator), profile_pipeline.rs (per-phase pipeline timing)
features/         — Feature retrospectives
```

## Tech stack
Rust, eframe/egui (GUI), memmap2 (mmap I/O), memchr (SIMD line indexing), aho-corasick / regex (search), chrono (timestamps), crossbeam-channel (background workers), Drain algorithm (template mining), log + env_logger (logging).

## Key features & related files
| Feature | Files |
|---|---|
| Memory-mapped loading + compact per-line index (`offset`, forward-filled timestamp, template ID) + SIMD indexing; staged background live-tail updates | `src/core/document.rs`, `src/ui/app/model.rs`, `src/ui/app/tab_model.rs` |
| Viewport-scoped embedded-data detection (JSON, key/value, debug literals, HTTP/protobuf, stack traces, encoded values; exact spans, bounded/cancellable worker, timestamp-record anchoring) | `src/core/embedded_data/{detectors,parse,model,scan}.rs`, `src/ui/log_view/embedded/highlighters/`, `src/core/document.rs`, `src/ui/app/tab_model.rs` |
| Log format detection & normalization (JSON, CEF, RFC 5424, Apple ULS, logcat brief, OSLog console, plain) | `src/core/format/` |
| Auto timestamp detection (14 built-in families: ISO-8601, YYYY/MM/DD, MM/DD/YYYY, MM-DD-YYYY, DD-MM-YYYY, DD.MM.YYYY, YYYY.MM.DD, BSD syslog, Apache CLF, RFC 2822, epoch, logcat threadtime, glog, ISO-8601 12h AM/PM, + user-defined custom) | `src/core/time/` |
| Drain template mining (native Rust) | `src/core/drain.rs` |
| Pre-mining masking (IPs, UUIDs, paths, JSON, numbers → semantic placeholders; bounded 65,536-entry token cache) | `src/core/masking.rs` |
| Aho-Corasick multi-filter search (including compact `u32` GUI match indexes) | `src/core/search.rs` |
| Single-pattern find box / keyword highlight (`find_lines`, `build_find_automaton`) | `src/core/search.rs` |
| Persistent settings (JSON, ~/.logotomy/settings.json) | `src/core/settings.rs` |
| Custom date recognizers (regex + named groups, verified live, saved to ~/.logotomy/custom_date_format_list.json) | `src/core/time/custom.rs`, `src/core/settings.rs`, `src/ui/custom_date/` |
| Timeline histogram + filter lanes (exact viewport re-resolution while zooming/panning, exact 2×7px singleton markers, edge-to-edge full-bucket-width 7/10/13px density rectangles, derived current-occurrence state, full-height histogram, 1px density lines, eye-toggle + trash per lane, "Everything Else" lane, smart axis labels with duration, minimap, fixed-height always-visible panel, hover details); normal chronological logs reuse the document timestamp index while out-of-order logs receive a sorted fallback, and filter-result timeline construction stays on the worker thread | `src/core/timeline.rs`, `src/ui/timeline/`, `src/ui/app/model.rs` |
| Virtualized multi-tab log view (UTF-8-safe long-line cap, interval-based highlighting, shared filtered index; cancellable background rebuild after lane toggles) | `src/ui/log_view/`, `src/ui/app/model.rs`, `src/ui/app/tab_model.rs` |
| Embedded-data Log View helpers (per-format cue, content-sized/resizable inspector, remembered Pretty/Tree/Raw preference, Frames, and encoded Summary/Decode overlay) | `src/ui/log_view/embedded/`, `src/ui/log_view/`, `src/ui/app/tab_model.rs`, `src/core/settings.rs` |
| Log view find state + keyword highlight (`find_input`, `find_query`, `find_matches`, `find_pos`, `find_rx`, `keyword_highlight`, `keyword_automaton`) | `src/ui/app/tab_model.rs`, `src/ui/log_view/highlight.rs` |
| Embedded Space Mono log font — baked into the binary (`include_bytes!`), registered under a dedicated egui family so only log text uses it (SIL OFL 1.1, `OFL.txt` in `src/ui/fonts/Space_Mono/`) | `src/ui/fonts/`, `src/ui/log_view/`, `src/ui/pin_viewer/` |
| Filter bookmarks (compact toolbar add + chip row; delete confirmations with optional "do not ask again") | `src/ui/filters/` |
| Pinned lines + analyses bottom panel (per-card Edit reopens the pin window) | `src/ui/pin_viewer/` |
| Scroll-position indicator bar + right-click context menu | `src/ui/log_view/` |
| Dark/light theme toggle | `src/ui/theme.rs`, `src/ui/app/` |
| MCP server (dual-era stdio JSON-RPC; standalone and GUI-attached modes) | `src/mcp.rs` |
| Compression-first MCP tools (summarize_log with budget bytes, get_timeline_histogram, get_template_anomalies, get_template, get_template_samples, log_sequence dense triples + collapse, raw_log line/time range; find_occurrences paginated `[line, epoch_ms|null]` tuples with case mode) + filter tools (filters_get/filters_add/filters_remove), GUI Pin-tab analysis tools (get_analysis/add_analysis), and per-tool `with_filtered_log` (default true = run on the filtered log; Everything Else lane excluded; zero filters + true short-circuits with a `{"comment":"no log"}` hint) | `src/mcp.rs`, `src/ui/app/`, `src/ui/pin_viewer/` |
| MCP GUI controls and Codex/Claude/Cline integration (one permanent `--mcp` config, random 12-character hexadecimal temporary session ID, authenticated private IPC, session-only attach instruction) | `src/mcp/`, `src/ui/app/`, `src/ui/settings/` |
| AI assistant integration popup | `src/ui/settings/`, `src/ui/app/` |

## Tests
357 tests across `src/core/` (including embedded-data detection), `src/mcp.rs`, UI models/views, and integration fixture contracts. Run with `cargo test`.

## Benchmark
`cargo run --release --example bench -- [logfile] [filters...]` — with no args, generates a 64MB/787k-line synthetic log (load ~3.3s, 3-filter scan ~0.7s, timeline build ~13ms). Pass a path to bench a real file, e.g. `cargo run --release --example bench -- examples/iOS-100K.log ERROR user_id`.

## Test logs
`cargo run --release --example gen_ios_logs -- [SIZES...]` generates deterministic iOS-style app logs. The generator is pure Rust (seeded PCG64), so the same `--seed` produces byte-identical output on every platform — no Python, no cross-version drift. It produces realistic iOS-style logs with level distribution (5% ERROR, 1% FAULT, 10% WARNING, 14% NOTICE, 45% INFO, 25% DEBUG), ~200 token-parametrized message templates, bursty timestamps, multiple PIDs/threads, and multi-frame FAULT stack traces. Outputs `iOS-1K.log` (~1K lines), `iOS-10K.log` (~10K lines), `iOS-100K.log` (~100K lines), `iOS-1M.log` (~1M lines). Every size starts from the same seed, so smaller files are exact prefixes of larger ones. Pass sizes as args (`cargo run --release --example gen_ios_logs -- 100K 1M`), use `--all` for all four, or `--seed N` to override the RNG. `cargo run --release --example profile_pipeline -- [logfile]` times each pipeline phase (slice → utf8 → ts extract → ts strip → mask → drain) on a file, or on a synthetic ~64MB log when no path is given.

## Cross-platform builds
Releases are built automatically via the GitHub Actions workflow (`.github/workflows/release.yml`) when a `v*` tag is pushed. It builds full-featured binaries (GUI + MCP) natively on each platform's own runner (no cross-compilation), for:
- **Windows** x86_64 (`windows-latest`) → **NSIS installer** `logotomy-<version>-setup.exe` (the runnable `.exe` is `logotomy.exe`, icon + version info embedded at build time)
- **Linux** x86_64 (`ubuntu-latest`) → **`.deb` + `.AppImage`** installers
- **macOS** Apple Silicon (`macos-latest`) + Intel → **`.dmg`** installers (+ `.app` bundle)

Installers replace the old raw `tar.gz`/`zip` archives. Packaging is done by [cargo-packager](https://github.com/crabnebula-dev/cargo-packager) using the `[package.metadata.packager]` table in `Cargo.toml`; the app icon (128×128, plus 256/512 and `logotomy.ico`) lives in `assets/icons/`. The installers associate `.log`, `.txt`, `.out`, `.err`, `.evt`, `.evtx`, `.sys`, `.csv`, `.json`, `.xml`, and `.md` with the GUI. Manual per-OS "build binary → make installer" steps and output locations: see `docs/release.md` (`scripts/package-release.sh` shells out to the same two commands).

Each release build also runs the full test suite (main app + `gen_ios_logs` example) and a benchmark against a generated `iOS-100K.log` before packaging. CI (`.github/workflows/rust.yml`) runs the same checks on `ubuntu-latest`/`macos-latest`/`windows-latest` for every push/PR to `main`.

### Windows file-lock semantics
Windows `LockFile` is mandatory (blocks all writers), unlike Unix advisory locks. `load_inner()` therefore skips `try_lock_shared()` on Windows — the mmap itself prevents truncation (`ERROR_USER_MAPPED_FILE`). Tests that need to shrink files are gated with `#[cfg(not(target_os = "windows"))]`; same-size content-change tests use in-place writes (`OpenOptions::new().write(true)`) instead of `std::fs::write` (which truncates).
