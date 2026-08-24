# logotomy — User Guide

*A high-performance log analyzer for when you open a giant log file and go "…logotomy happened here?"*

logotomy chews through 50MB+ text/log files without breaking a sweat, mines the
structure out of them automatically, and gives you an interactive timeline to
see exactly when things went sideways.

---

## 1. Installation & Running

### macOS

```bash
# Download the latest release .dmg (Apple Silicon: _aarch64, Intel: _x86_64),
# open it and drag logotomy.app into Applications.
open /Applications/logotomy.app     # Launch the GUI
```

### Windows

```bash
# Download logotomy-<version>-setup.exe and run the NSIS installer.
# It installs logotomy.exe (with app icon); launch from the Start Menu.
```

### Ubuntu / Linux

```bash
# .deb:  sudo apt install ./logotomy_<version>_amd64.deb
# or .AppImage:  chmod +x logotomy-<version>-x86_64.AppImage && ./logotomy-<version>-x86_64.AppImage
logotomy    # Launch the GUI
```

### From source (any platform)

Prereqs: a working [Rust toolchain](https://rustup.rs) (1.85+).

```bash
# Build and run the GUI (recommended for big files)
cargo run --release

# Just build the binary
cargo build --release
# → target/release/logotomy        (the GUI)
```

### Open a file from the shell or file manager

Pass one or more paths to the GUI:

```bash
logotomy app.log another.log
```

The first GUI process is the standalone instance. If it is already running,
opening a file from Finder, Windows Explorer, a Linux desktop-file association,
or another shell command reuses that window, focuses it, and opens the file in
a new tab instead of starting another GUI process.

Release installers register `.log`, `.txt`, `.out`, `.err`, `.evt`, `.evtx`,
`.sys`, `.csv`, `.json`, `.xml`, and `.md` with logotomy. After installation,
use **Open with** or **Set as default** in the operating system’s file manager.
The Linux `.deb`/AppImage association depends on the desktop environment’s MIME
database; the Windows and macOS installers register the extensions directly.

### MCP server mode (for AI assistants)

The same binary also runs the MCP server:

```bash
./logotomy --mcp                          # stdio mode (standalone or GUI-attached)
```

Build MCP-only (Linux/Windows cross-compile):

```bash
cargo build --release --no-default-features
```

---

## 2. The GUI at a glance

```
┌──────────────────────────────────────────────────────────┐
│ 🔥 logotomy | 📂 Open file | 🧩 Saved filters |    status   │  toolbar
│ [app.log ×] [server.log ×]                              │  tabs (multi-file)
│ 🔑 Add Filter ┌──────────────────────┐                  │  filter strip
│                │ filter + Enter      │ ➕ Add           │  (prominent frame)
│                └──────────────────────┘                  │
│ ┌──────┬────────────────────────────┬────┐              │
│ │ 👁 EE │ ▁▂█▆▅▃ density (full-hgt) │ ↕  │              │  timeline
│ │ 👁 kw1│ ───────────── 1px line    │ ↔  │              │  (eye markers,
│ │ 👁 kw2│ ── ◆ ◆ ── ◆ filter match│ 🗑 │              │   trash = remove)
│ │      │ 10:00      Δ3.3s  10:01   │    │              │   (inter-tick dur)
│ │      │ [══════minimap══════]       │    │              │
│ └──────┴────────────────────────────┴────┘              │
│  1234 T 7  2026-07-19T10:00:01.123Z INFO hello           │
│  1235 T 9  2026-07-19T10:00:01.223Z ERROR boom  ←center  │  log view
│  ...                                  │ 🧩 Templates     │  (virtualized)
│                                       │ ×9812 T3 GET…    │  (right panel)
│                              ┃ (scroll bar)              │
├──────────────────────────────────────────────────────────┤
│ ▼ 📌 1 pinned  |  📝 2 analyses                          │  bottom panel
│ 📌 L1235  GET /api/users 200 OK                 ×        │  (collapsible,
│ 📝 L1235  This is the failing request — boom      ×        │   pinned + notes)
└──────────────────────────────────────────────────────────┘
```

### Open a file
- **Drag & drop** any text file (`.log`, `.txt`, `.out`, `.csv`, …) anywhere into the window, or
- click **📂 Open file**.
- A **progress bar** shows real indexing progress (two stages: *Indexing lines*, then *Mining templates & timestamps*). Cancel with × if you dropped the wrong file.
- Each file opens in its own **tab** — open as many as you like, close them with the × next to the tab name.
- When a file is being appended (live tailing), new lines are indexed and template-mined in the background before appearing atomically. The current view stays responsive while this happens; truncation or in-place edits still require reopening the file.

### Custom date recognizers
logotomy auto-detects the timestamp family (shown as `format: … · date: …` in the top bar). If your log uses a date shape it doesn't recognize, add your own:

1. Click **Custom date** (top bar, left of Settings).
2. Give the format a **Name**, paste a **Regex** that captures the timestamp using named groups — required `year, month, day, hour, min, sec`, optional `ms` (milliseconds) and `ampm` (for 12-hour `AM`/`PM`). Example for `2026-08-14 4:08:23.668 PM`:
   ```text
   (?P<year>\d{4})-(?P<month>\d{2})-(?P<day>\d{2}) (?P<hour>\d{1,2}):(?P<min>\d{2}):(?P<sec>\d{2})\.(?P<ms>\d{3}) (?P<ampm>[AP]M)
   ```
3. Paste a **sample log line** under it. The window live-verifies the regex and prints the parsed components (`Year: … Month: … Date: … Hour: … Min: … Sec: … MILLI SECOND: …`).
4. Click **Add custom date format** (enabled once the regex matches). It's saved to `~/.logotomy/custom_date_format_list.json` and tried together with the built-in families on the **next file you open**.
5. To apply to the file that's already open, use **Re-scan active log** in the same window (per-tab filters/pins/scroll reset on re-scan).

### The log view (center)
- Virtualized: a 5-million-line file scrolls as smoothly as a 50-line one.
- Gutter shows the **line number** and the line's **template ID** (`T12`).
- Click any line to select it (white selection bar on the timeline).
- **Right-click** any line for a context menu with **📌 Pin log** and **📝 Add analysis**.
- The right edge has a **black scroll-position indicator bar** showing where you are in the file.
- Log lines render in the embedded **Space Mono** monospace font (SIL OFL 1.1 — see the README thanks section); the **A− / A+** toolbar buttons change the size from 8 to 24 px.
- Very long log rows are kept to one visual row and safely display a 2,000-byte Unicode-aware prefix, so an unusually large message cannot interrupt scrolling.
- Structured values embedded anywhere in visible log records are detected in the background: JSON, logfmt/key-value, colon fields, Foundation/Python/JVM debug values, HTTP, protobuf text, stack traces, and JWT/Base64/hex/PEM. Compact and multiline payloads are supported; parsing never runs in the paint path.
- A translucent format cue (**JSON**, **KV**, **HTTP**, **TRACE**, etc.) scales with the active log font and sits immediately above the payload's opening token without reserving layout space or shifting log text. The exact visible source receives a subtle underline. Hover the cue for its type, shape, line count, and byte size.
- Click a cue to open the matching in-view inspector beside the gutter. Structured values open in **Pretty** by default and offer Tree/Pretty/Raw; the last selected tab is remembered for future payloads and sessions. Stack traces start in Frames; encoded values start with a safe Summary and require **Decode preview**. The inspector sizes itself to the payload, can be resized from its edges, and scrolls when needed. Copy the exact raw payload, formatted value, or an individual tree value. Press **Esc**, click outside, or use the close icon to dismiss it.
- Filtered views still analyze the contiguous physical log record—hidden filter rows are never joined into artificial JSON. Timestamped multiline records retain a compact boundary index so jumping directly into the middle can recover the opener.
- Filters you add (below) are **highlighted inline**, in the filter's color.
- **Search box** (right side of toolbar): type a string and press **Enter** to find all occurrences in the visible log lines. Amber highlights mark every match. With multiple results, the Log View header shows Up/Down navigation and a shortcut hint toast; use **▲/▼** (or Up/Down arrows) to step through matches. Press **Esc** to clear the search, or **Esc** again to clear a keyword highlight.
- **Double-click any word** in a log line to highlight every occurrence of that word in cyan. The word is also pre-filled into the search box — press **Enter** to turn it into a full search. Single-click anywhere clears the keyword highlight.

### Filter bookmarks
- The add section is prominently framed with a heading "🔑 Add Filter".
- Type a filter in the text field and press **Enter** (e.g. `ERROR`, `timeout`, `user_id=42`) or click the **➕ Add** button.
- Up to **20 filters** are supported. The input is disabled once the cap is reached.
- Scanning happens in the background (spinner while chewing).

### The timeline
- The timeline is a **fixed-height panel** at the top — it always shows the full histogram, all filter lanes, axis labels, and the zoom/minimap strip, and can never be shrunk to hide lanes. Its height grows/shrinks with the number of filters.
- Shows the whole file as a full-height density histogram, with one **colored lane per filter**. Each lane has a **straight 1px line** in the filter's color across the full lane width, plus clickable ◆ diamonds for individual matches.
- **Left column** shows filter names (up to 14 chars) with **👁 eye markers** (👁 = lane enabled, bold label; 🚫 = disabled, normal weight label). Click to toggle. The first lane is "Everything Else" — it has the eye toggle but **cannot be removed**.
- **Hover a filter's name/eye** to see a tooltip with the **full filter text** and its **total match count**, e.g. `Some Filter (334 occurrences)`.
- Each filter lane has a **🗑 trash icon** on the right of its label. Clicking it always asks for confirmation before removing the filter — unless you tick **"Do not ask me again"** in the popup (or enable *Settings → Do not ask before deleting a filter*).
- Click a filter lane or one of its ◆ diamonds to select the lane. The Timeline header then shows previous/next navigation using Left/Right arrows; those keys select earlier/later occurrences and jump the Log View to that line.
- **Bottom toolbar** (shown while filters exist, left-aligned):
  - **🚫 Disable All / 👁 Enable All** — toggles every filter lane at once; the **Everything Else** lane is never touched.
  - **🗑 Delete All** — removes every filter (behind a confirmation popup, following the same "do not ask again" preference).
- **Right column** has ↕ (zoom) and ↔ (pan) hint icons with hover tooltips.
- **Zoom** — scroll anywhere over the timeline. Zoom is continuous and works on both trackpads and mouse wheels.
- **Pan** — drag left/right (without shift). Snaps at the file boundaries.
- **Brush select** — shift+drag to draw a rectangle; on release, zooms to that range.
- **Reset zoom** — double-click anywhere on the timeline, or click the ↺ button.
- **Minimap** — click anywhere on the minimap to jump to that position.
- **Hover** the timeline for per-bucket details (time, line count, per-filter counts).
- **Click** the timeline → jumps to the nearest filter match (or approximate position).
  A white marker shows your current position.
- **Axis labels** are smart: if all ticks share the same hour, only `MM:SS.ms` is shown.
- **Inter-tick duration labels** (e.g. `Δ 3.3s`, `Δ 1m 34s`) appear between each pair of tick labels.

### Bottom panel (pinned lines + analyses)
- **Right-click** any log line → context menu:
  - **📌 Pin log** — saves the line to the bottom panel for quick revisiting.
  - **📝 Add analysis** — opens a text input to write a free-text note about that line.
- Expand/collapse the panel with the **▼/▶** header. When collapsed, shows counts.
- When empty, shows a brief hint: "Right-click a log line to pin it or add an analysis."
- Pinned lines show **line number + text snippet**; click the **✏️ edit** button to reopen the pin window and edit its comment/lines, or **×** to unpin.
- Analyses show **line number, snippet, and your note**; click **×** to delete.
- **"Clear all"** empties everything and collapses the panel.

### Templates panel (right, 🧩)
- logotomy runs the **Drain template-mining algorithm** natively in Rust while loading.
- The panel lists mined patterns sorted by frequency, e.g. `×12043 T3 GET /api/<*> status=<*>`.
- **Click a template** to jump to an example occurrence.
- Great first stop when you don't even know what's *in* a file.

### Timestamps
Auto-detected per file from a sample of the first lines. Supported families:

| Family | Example |
|---|---|
| ISO-8601 | `2026-07-19T10:15:30.123Z`, `2026-07-19 10:15:30,456`, `…+06:00` |
| Slashed | `2026/07/19 10:15:30` |
| US numeric | `08/20/2026 10:15:30.125`, `08-20-2026 10:15:30` |
| Day-first numeric | `20-08-2026 10:15:30`, `20.08.2026 10:15:30` |
| Year-first dotted | `2026.08.20 10:15:30` |
| Syslog | `Jan  5 03:22:11` (assumes current year) |
| Apache | `10/Oct/2024:13:55:36 +0000` |
| RFC 2822 | `Thu, 20 Aug 2026 10:15:30 +0000` |
| Epoch | `1784158530123` or `1784158530` |
| Logcat threadtime | `07-15 22:00:01.123` (yearless) |
| glog | `I0715 22:00:01.123456` (yearless) |

### Log format detection
logotomy first recognizes the **log format** from a sample, then applies a
format-aware normalization before template mining — structured formats get
field-aware templates instead of raw token fragmentation:

| Format | Recognized shape | Timestamp |
|---|---|---|
| JSON | `{"time": …, "lvl": …, "msg": …}` | from `time`/`timestamp` field |
| CEF | `CEF:0\|Vendor\|Product\|…` | none (timeless) |
| RFC 5424 | `<134>1 2026-…Z host app proc msgid - msg` | ISO |
| Apple ULS | `2026-…+0300 0x… Default 0x… 12345 …` (`log show`) | ISO |
| Logcat brief | `D/Tag: message` | none |
| iOS OSLog console | `[Subsystem:Category] LEVEL: message` | none |
| Plain (fallback) | anything else | auto-detected |

If no timestamps are found, the timeline falls back to **line numbers** and says so.
Untimestamped lines (stack traces etc.) inherit the previous line's timestamp.

---

## 3. The MCP server (AI-assistant bridge)

`logotomy --mcp` speaks MCP over **stdio** (newline-delimited JSON-RPC 2.0), so an
AI assistant can load logs independently, query them with filters, and pull exact windows.

Configure `logotomy --mcp` once. By default the agent works independently: it calls `load_log`
and uses the returned `log_id`. To analyze the log open in the GUI, click **Start MCP**, copy
the GUI session instruction, and paste it into the agent conversation. The agent calls
`attach_gui_session`, then `session_info`; while `mode` is `gui_attached`, it must not call
`load_log` or pass `log_id`. Stop MCP when finished to invalidate the temporary session ID.

Logotomy exposes MCP only through stdio. Private authenticated local IPC routes attached calls
to the GUI, so no URL, port, or temporary MCP server belongs in the agent configuration.

### Wire it into an MCP client

```json
{
  "mcpServers": {
    "logotomy": {
      "command": "/absolute/path/to/logotomy",
      "args": ["--mcp"]
    }
  }
}
```

### Tools

| Tool | What it does |
|---|---|
| `session_info` | Mode, temporary lifecycle, active GUI log/filter state, or headless load count |
| `attach_gui_session` / `detach_gui_session` | Attach the stdio server to a temporary GUI session, or restore standalone routing |
| `load_log` | `path` → indexes the file, returns `log_id` + stats (lines, time range, top templates) |
| `list_logs` | All loaded documents with stats |
| `close_log` | Unload a `log_id` and free memory |
| `find_occurrences` | Lines containing `keyword`, paginated (`offset`, `max_results`); returns total count + first/last-seen, optional `after`/`before` window; `format="refs"` for anchors-only, `context=N` for collapsed surroundings |
| `summarize_log` | One-call orientation (optional `start`/`end` range): stats, top/error-ish templates, biggest time gaps, densest minute, plus byte-size budget estimates |
| `get_timeline_histogram` | Tiny `{x, counts}` distribution for whole log / keyword / template, optional range — find the spike first |
| `get_template_anomalies` | Rare, first-seen-late, and bursty templates ("what's unusual here"), optional range |
| `get_template` | Resolve template ids to `{pattern, count, example_line}`; omit `ids` for all |
| `get_template_samples` | A few concrete example lines for a template |
| `log_sequence` | Dense `[[epoch_ms\|null, line, template_id], …]` over a line/time range; optional `collapse`, returns `truncated` when capped |
| `raw_log` | Raw lines over a line/time range (`start`, `end`), `max_lines` + `truncated` flag |
| `trim` | (GUI mode) focus the open log's visible window to a line/time range |

Time arguments accept `RFC3339` (`2026-07-19T10:15:30Z`), `YYYY-MM-DD HH:MM:SS`,
`YYYY-MM-DD`, or epoch millis/seconds. `start`/`end` range bounds accept either a
1-based line number (integer) or a time (string).

### Example conversation flow

```
load_log(path="/var/log/app.log")                → log_1 (2.1M lines, 10:00→12:00)
summarize_log(log_1)                            → {stats, top/error templates, gaps, template_size_bytes, …}
get_timeline_histogram(log_1)                   → spike around 10:31
log_sequence(log_1, start="10:31:00", end="10:31:30")  → dense [time, line, template_id] triples
get_template(log_1, ids=[7, 12])                 → what those template ids mean
raw_log(log_1, start="10:31:05", end="10:31:08") → the exact lines
```

### Quick manual smoke test

```bash
printf '%s\n' \
 '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"cli","version":"1"}}}' \
 '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"load_log","arguments":{"path":"/tmp/app.log"}}}' \
 | ./logotomy --mcp
```

---

## 4. Performance & limits

- **Memory-mapped I/O** — the file is never fully materialized; the OS pages it lazily.
- **SIMD line indexing** (`memchr`) — GB/s territory for the offset index.
- **Single analysis pass** — format detection + timestamps + Drain templates in one sweep.
- **Aho-Corasick** — all filters matched in one scan pass; 12 filters cost ≈ 1.
- Measured on an M-series MacBook with a 64MB / 787k-line synthetic log:
  full load (index + timestamps + Drain mining) **≈ 3.3s** with a live progress
  bar, 3-filter whole-file scan **≈ 0.7s**, timeline build **≈ 13ms**.
  Run `cargo run --release --example bench` to check your machine.
- Per-line render length is capped at 2000 chars in the GUI (data stays intact in the mmap).
- Embedded JSON analysis is debounced while scrolling, cancellable, and worker-only. GUI record scans are bounded to 8 MiB / 20,000 lines, 128 nesting levels, 20,000 normalized nodes, and 64 results per physical interval; values exceeding a bound are not presented as valid detections.
- Tested edge cases: CRLF endings, missing trailing newline, blank lines,
  multi-MB single lines, invalid UTF-8 (lossy display), files with no timestamps.

## 5. Keyboard & mouse cheat sheet

| Action | Effect |
|---|---|
| Drop file on window | Open in new tab |
| Click tab | Switch file |
| Tab × | Close file |
| Filter box + Enter or ➕ Add | Add filter bookmark |
| Click timeline | Jump to nearest filter match, white marker shows position |
| Scroll on timeline | Zoom in/out (continuous, trackpad + mouse) |
| Drag (no shift) on timeline | Pan left/right |
| Shift+drag on timeline | Brush-select range → zoom to selection |
| Double-click timeline | Reset zoom to full range |
| Click minimap | Pan view to that position |
| Click log line | Select line (white line on timeline) |
| Right-click log line | Context menu: 📌 Pin log / 📝 Add analysis |
| Click 👁/🚫 lane marker | Toggle filter lane on/off (filters log view) |
| Hover filter name/eye | Tooltip with full filter text + match count |
| Click 🗑 on a filter lane | Remove that filter (with confirmation) |
| Timeline 🗑 Delete All | Remove every filter (with confirmation) |
| Timeline 👁️ Show/Hide all filters | Toggle every filter lane at once |
| Pinned ✏️ edit button | Reopen the pin window to edit the pin |
| A− / A+ buttons | Decrease / increase log text size |
| 🧩 Templates → click row | Jump to example occurrence |
| Bottom panel ▼/▶ | Expand / collapse pinned lines and analyses |

---

## 6. Distribution

Each release ships **native installers** (see `docs/release.md` for how they're
built with cargo-packager):

| Platform | Download | Contents |
|---|---|---|
| macOS (Intel + Apple Silicon) | `logotomy-<tag>-<target>.dmg` | Full GUI + MCP (`logotomy.app`) |
| Ubuntu / Linux x86_64 | `logotomy-<tag>-<target>.deb` or `.AppImage` | Full GUI + MCP |
| Windows x86_64 | `logotomy-<tag>-<target>.exe` (NSIS) | Full GUI + MCP (`logotomy.exe`) |

Build from source for any platform: `cargo build --release`.
