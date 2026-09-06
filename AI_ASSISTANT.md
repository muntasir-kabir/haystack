# AI_ASSISTANT.md — logotomy

High-performance Rust log analyzer & visualizer (GUI + MCP server for AI assistants).

## Strict rules
1. **Big changes** — read `UserGuide.md` + `AI_ASSISTANT_DETAIL.md` first. Update them if structure/features/behavior change. Keep `AI_ASSISTANT_DETAIL.md` concise.
2. **Fixes** — add a 1-2 sentence entry at the top of `changes.md` describing what changed and why.
3. **Tests** — run `cargo test` after code changes; all must pass. Always add test for new feature or bug changes
4. **No Python** — template mining is native Rust (Drain). Never shell out.
5. **Releases** — before creating or pushing a `vX.Y.Z` tag, set both `package.version` and `[package.metadata.packager].version` in `Cargo.toml` to the same `X.Y.Z`, let Cargo refresh the matching `Cargo.lock` package version, and run the exact **Validate release version** step from `.github/workflows/release.yml` locally. It checks the Cargo metadata package version and the packager-table version independently against the tag before tagging.
6. **Popup dismissal** — transient popups must close immediately on Escape or an outside click. Apply this consistently to new popup surfaces; modal editors may keep explicit Cancel actions but should still honor Escape where practical.

## Try to follow
- Focus on code quality & maintainence 
- Managable file size: make separate UI/functional files for logically different parts
- Write unit/UI test whenever possible.
- Comment TODO: IMPROVE: notes when its helpful.

## Vital facts
- Multi platform support **MUST**: Windows, MAC (Intel+Apple Silicon), Linux (x86_64), Ubuntu
- **Windows file-lock semantics**: `LockFile` is mandatory (blocks writers), so `load_inner()` skips `try_lock_shared()` on Windows; the mmap itself prevents truncation (`ERROR_USER_MAPPED_FILE`). Tests that shrink files are `#[cfg(not(target_os = "windows"))]`; same-size content changes use in-place writes instead of `std::fs::write`.
- Single binary: `logotomy` (GUI) or `logotomy --mcp` (stdio MCP server for standalone and explicitly GUI-attached work). `--mcp-gui` is a deprecated compatibility alias only.
- `gui` Cargo feature gates GUI deps; Linux/Windows build MCP-only with `--no-default-features`.
- Stack: Rust, eframe/egui, memmap2, memchr, aho-corasick, chrono, crossbeam-channel.
- MCP tools: session_info, attach_gui_session / detach_gui_session, load_log, list_logs, close_log, filters_get / filters_add / filters_remove (filter keyword sets), find_occurrences (paginated `[line, epoch_ms|null]` tuples, case mode, after/before window), plus the canonical stateless set: summarize_log (range + budget bytes), get_timeline_histogram (range), get_template_anomalies (range), get_template, get_template_samples, log_sequence (dense triples + collapse), raw_log (line/time range), GUI-attached get_analysis / add_analysis (the active Pin-tab entries), and GUI-attached trim. All analysis tools take `with_filtered_log` (default true) to run on the union of the filter set (Everything Else lane excluded); when true with zero filters they reply `{"comment":"no log",…}` until a filter is added or `with_filtered_log:false` is passed. GUI agents must immediately call `get_analysis`, treat those entries as the user's findings/hypotheses, use `filters_add` and `trim` to focus the investigation, then call `add_analysis` with the evidenced root-cause finding before completing. `logotomy://session` and `logotomy://guide` resources describe mode/lifecycle/workflow; results include structured content plus text fallback.

## Structure (short)
- `src/core/` — library: document (mmap + SIMD index), embedded_data (viewport-scoped structured payload detection), drain (template mining), masking (pre-mining dynamic value masking), format (log-format detect/normalize), time (timestamp detect), search (Aho-Corasick), timeline, saved_filter, settings
- `src/ui/` — egui GUI modules organized by feature:
  - `app/` — application state and main UI loop: `model.rs` contains shared state and app-level behavior, `tab_model.rs` contains `LogTab` operations, `view.rs` is the main shell, and the focused `filters_dropdown.rs` / `templates.rs` views handle saved filters and template browsing
  - `log_view/` — virtualized log rows, selection, embedded JSON cues/inspector, pin modal, and shared row highlighting/layout in `highlight.rs`
  - `timeline/` — density histogram, filter lanes, zoom/pan
  - `pin_viewer/` — pinned lines panel
  - `filters/` — filter input strip
  - `settings/` — settings and integration popups
  - `custom_date/` — custom timestamp-format popup
  - `icons.rs` + `icons/` — SVG icon system · `theme.rs` — dark/light palette
  - `fonts/` — embedded Space Mono monospace font for log text (SIL OFL 1.1, baked into binary)
- `src/mcp.rs` + `src/mcp/` — stdio MCP server, secure GUI-session manifest, and authenticated private IPC routing · `src/main.rs` — CLI dispatcher (`--mcp`, deprecated `--mcp-gui`, else GUI)

## Commands
- `cargo test` — 500+ tests
- `cargo run --release` — GUI
- `cargo run --release --example bench -- [logfile] [filters...]` — benchmark (no args → 64MB/787k-line synthetic log; pass a path to bench a real file, e.g. an iOS log)
- `cargo run --release --example gen_ios_logs -- [SIZES...] [--all] [--seed N]` — generate deterministic iOS test logs (seeded PCG64; iOS-1K/10K/100K/1M; `--all` for all, `--seed N` to override the fixed RNG)
- `cargo run --release --example profile_pipeline -- [logfile]` — per-phase pipeline timing (no args → synthetic ~64MB log; pass a path to profile a real file)

## Docs
- `AI_ASSISTANT_DETAIL.md` — detailed project reference (features, tests, builds)
- `UserGuide.md` — full user guide · `feature.md` — feature inventory · `changes.md` — changelog
