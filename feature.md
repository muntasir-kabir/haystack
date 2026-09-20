# Haystack — Feature Inventory

Everything currently implemented, and what's deliberately not (yet).

## ✅ Ingestion & parsing core (`src/core/`)

| Feature | Detail |
|---|---|
| Any text file | `.log`, `.txt`, `.out`, `.csv`, … — extension-agnostic |
| Memory-mapped I/O | `memmap2`; 50MB+ files never fully materialized |
| SIMD line index | one `memchr` pass builds the line-offset table |
| Real progress reporting | 2 stages (Indexing / Analyzing), byte-accurate %, cancellable |
| Log format detection | Pluggable recognizer: JSON, CEF, RFC 5424, Apple Unified Logging (`log show`), logcat brief, iOS OSLog console — one file per format (`src/core/format/`), registry-extensible, `plain` fallback |
| Timestamp detection | Built-in families (ISO-8601, YYYY/MM/DD, BSD syslog, Apache CLF, epoch, logcat threadtime, glog, ISO-8601 12h AM/PM) + user-defined custom recognizers (regex with named groups, live-verified, saved to `~/.haystack/custom_date_format_list.json`); exact source spans retained compactly for annotation (`src/core/time/`, `src/ui/custom_date/`) |
| Timestamp auto-detection | Pluggable: ISO-8601 (`Z`, offsets, comma/dot millis, space/`T` separator), `YYYY/MM/DD`, syslog `Jan  5`, Apache `10/Oct/2024:13:55:36 -0700`, epoch s/ms, logcat threadtime, glog — one file per family (`src/core/time/`) |
| Record time provenance | Continuations inherit only their owning record's known time; missing/invalid headers reset it and untimed preambles stay unknown |
| Compact record boundaries | one bit per physical line marks actual record headers independent of timestamp validity, with rank-assisted lookup of large multiline records |
| Embedded data detection | Registry-based, viewport-scoped detectors for JSON, logfmt/key-value, colon fields, Swift/Foundation/Python/JVM debug values, XML and labeled OpenStep plists, binary plist magic, HTTP, protobuf text, stack traces, and encoded JWT/Base64/hex/PEM; exact source spans, cancellation, and safety limits |
| Native Drain template mining | fixed-depth parse tree, `<*>` wildcards, per-line template ID, occurrence counts, example line — zero Python |
| Robustness | CRLF, missing trailing newline, blank lines, invalid UTF-8 (lossy), multi-MB single lines, timeless files |
| Filter engine | Aho-Corasick, case-sensitive, all filters in a single pass, background thread + cancellation |
| Timeline bucketing | Source-line axis for every file; exact record-start overview and physical-line filter counts, screen-width viewport re-resolution, exact 2×7px singleton markers, and edge-to-edge full-bucket-width 7/10/13px collision tiers |
| Timeline lane toggles | Per-filter eye (visible/invisible) toggles + "Everything Else" lane; log view filters to active lanes only; trash icon per lane removes filter with confirmation |
| Axis labels | Physical source line numbers; event times do not control positions |
| Time-window analytics | count-in-window, first/last occurrence helpers |

## ✅ Desktop GUI (`src/ui/`, egui/eframe)

| Feature | Detail |
|---|---|
| Application shell | `Haystack` product mark; grouped Open file/Recent/Saved filters/Commands actions; format/date status; AI Assistant state menu; Settings; responsive More menu below 1100px |
| SVG action system | exhaustive embedded 24×24 `currentColor` outline catalog, aligned compact 22px controls, quiet icon+label and icon-only helpers, action-oriented tooltips, light/dark rendering tests |
| Drag & drop | drop any file anywhere; clear file-type overlay while hovering files; actionable empty/loading/error states |
| Open dialog | native picker via `rfd` |
| Progress bar | per-loading-file card with stage label + % + cancel |
| Multi-tab | cohesive log-icon tabs with integrated close action, loading spinner, and compact AI status badge; re-dropping an open file focuses its tab |
| Multiple Log Views | add stable numbered Log Views for one file; each owns selection, scroll, Find, inspectors, and transient row state while document/filter/timeline/pin/template state stays shared; views can dock or detach independently, and the final view cannot be closed |
| Virtualized log view | renders only visible rows; line # + template ID gutter; filter highlight; color marker per line |
| Filter occurrence context | Shift-click a log row to center it in a scrollable overlay with the nearest prior and next occurrence from every configured filter; retains log highlighting and embedded-data cues |
| Source annotations | quiet exact timestamp underlines with raw/normalized-UTC hover actions; background structured-data analysis with font-scaled zero-layout cues, exact underlines, delayed source actions, Tree/Pretty/Raw inspector, frame-oriented trace view, and explicit encoded preview |
| Log font | embedded Space Mono monospace (SIL OFL 1.1) — log text only, rest of UI stays on default fonts |
| Log font size controls | A− / A+ buttons in both log view and context panel (8–24px range) |
| Lane-filtered log view | toggling timeline checkboxes filters the log view to only active lanes |
| Filter bookmarks | add/remove chips, per-filter color, live match count, background rescan |
| Timeline view | adaptively resolved density histogram + per-filter colored lanes, exact singleton markers and continuous full-width collision buckets, exact hover counts, current occurrence derived from lane + Log View line |
| Timeline lane toggles | per-filter eye (visible/invisible) toggle + "Everything Else" lane; grays out disabled lanes; trash icon removes filter with confirmation |
| Timeline legend | 120px left column, 10.5pt monospace labels, 14-char truncation with full-name tooltip |
| Smart axis labels | shorthand: same hour → `MM:SS.ms`, same date → `HH:MM:SS.ms`, multi-day → full; duration label between ticks |
| Timeline navigation | click → nearest filter match (or approx. position) |
| Context panel | selected line ± 5 (radius adjustable 1–50), timestamp + template header, jump-to-full-view, clear |
| Template browser | right panel, mined patterns sorted by frequency, click → example line |
| Docked/separate views | the dock panel has a standalone add button for Log Views; every numbered Log View, Pinned, and Templates places an External Window action beside the dock-tab title; detached windows are dock panels that retain added tabs/layout, and closing a child window restores its dock location |
| Dark/light mode | semantic colour palette, theme control in Settings, all panels and SVGs themed with readable secondary text |
| UX details | title-height Timeline controls with bulk actions moved into the lower filter-label area, single-row Log/Pinned/Templates chrome, pointer cursors, clear empty states, sentence-case copy, Escape/outside-click dismissal, explicit dialog footers |
| Status bar format readout | shows the active log's detected format + date format (`format: json · date: field-based`) |

## ✅ MCP server (`src/mcp.rs`, `haystack --mcp`)

Embedded in the same binary and exposed to agents only through `haystack --mcp` stdio.
The same stable tool catalog supports standalone file loading and explicit attachment to a live
GUI with `attach_gui_session`. Modern `server/discover` and initialization-era clients are
supported across `2024-11-05` through `2026-07-28`. GUI sessions use a random 12-character
hexadecimal temporary ID,
authenticated private loopback IPC, a private atomic manifest, and automatic invalidation.
The server exposes `haystack://session` and
`haystack://guide`, a `session_info` tool, structured result content plus text fallback, output
schemas, and behavioral annotations.

| Tool | Purpose |
|---|---|
| `session_info` | mode, temporary lifecycle, active GUI log/filter state, or headless load count |
| `attach_gui_session` / `detach_gui_session` | attach to a temporary GUI session or restore standalone routing |
| `load_log` | index a file → `log_id` + stats |
| `list_logs` | loaded documents + stats |
| `close_log` | drop a document + invalidate its caches |
| `find_occurrences` | paginated `[one_based_line_number, epoch_ms\|null]` keyword-hit tuples; supports `offset`, `max_results`, optional `after`/`before` window, `with_filtered_log`, and ASCII `case_sensitive` mode |
| `get_analysis` / `add_analysis` (GUI mode) | read the user's Pin-tab findings/hypotheses or add an evidenced root-cause analysis card; empty `lines` creates a text-only top card |
| `summarize_log` | one-call orientation over an optional line/time range: stats, error-ish templates, time gaps, densest minute, plus byte-size budget estimates (`template_size_bytes`, `sequence_estimate_bytes`) |
| `get_timeline_histogram` | histogram over an optional range; explicit line/time domain and record/physical-line units, with omitted/auto retaining legacy clock behavior |
| `get_template_anomalies` | rare / first-seen-late / bursty templates, optional range |
| `get_template` | resolve template ids to `{pattern, count, example_line}`; omit `ids` for all |
| `get_template_samples` | a few concrete lines per template |
| `log_sequence` | dense `[[epoch_ms\|null, line, template_id], …]` over a line/time range; optional `collapse`, `truncated` flag |
| `raw_log` | raw lines over a line/time range, `max_lines` + `truncated` flag |
| `trim` (GUI mode) | focus the active document's visible window to a line/time range |

Match results are cached per (log, keyword, case mode); time params accept RFC3339 /
`YYYY-MM-DD[ HH:MM[:SS]]` / epoch s/ms. `start`/`end` range bounds accept a
1-based line number (integer) or a time (string).

### GUI MCP Server controls
- A single **AI Assistant** top-bar menu contains connection status, Start/Stop, session-instruction copy, and integration guidance
- A compact tab badge identifies the GUI document currently available to the assistant
- One permanent `--mcp` setup for Codex, Claude, and Cline
- Explicit copy of a temporary GUI session instruction; no URL or temporary MCP configuration
- Exactly one appropriate Start or Stop action is shown for the current state
- Runs **in-process** on a background thread — shares loaded documents with the GUI
- Status bar updates on start/stop/ready/error

## ✅ Logging & observability

| Feature | Detail |
|---|---|
| Application-wide logging | `log` + `env_logger` crate: GUI logs to stderr, MCP server logs to both stderr and `haystack.log` |
| Configurable via `RUST_LOG` | standard env-var filtering: `RUST_LOG=debug`, `RUST_LOG=warn`, etc. |
| Panic hook | MCP server captures panics with location info to `haystack.log` |
| Key events logged | file open/close, tab switch, MCP start/stop, filter scan, load errors |

## ✅ Quality & verification

- 500+ unit and integration tests (core, MCP, GUI, SVG, responsive-shell, popup, and embedded-data contracts) — `cargo test`
- Release benchmark harness — `cargo run --release --example bench -- <file> [kws]`
  (generates a 64MB synthetic log when run without args)

## 🔮 Deliberate next steps (not implemented)

- Advanced filter expressions (nested boolean groups and reusable named expressions; lane visibility already filters the log view)
- Multi-file merged timeline
- MCP prompts (resources are implemented)
- Embedded data detectors for YAML, TOML, generic XML, and delimited records
- Structured-data field/path search, schema grouping, comparison, similarity search, and MCP extraction tools
- Keyboard selection and richer result highlighting in Commands
- Color-vision-safe filter palettes and optional non-color lane markers
- Automated screenshot regression coverage for major UI states
- Searchable saved-filter manager for large collections
- Configurable density, customizable toolbar actions, first-run guidance, native desktop menus, localization-ready strings, and richer accessibility semantics

## Removed

- `src/python/parser_ml_microservice.py` — the old half-baked Python sidecar.
  Template mining is native Rust now; nothing is shelled out, ever.
