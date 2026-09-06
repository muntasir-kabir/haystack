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

Release installers register `.log`, `.txt`, `.out`, `.err`, `.csv`, `.json`,
`.xml`, and `.md` with logotomy. After installation,
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
│ LOGotomy | Open file | Recent | Saved filters | Commands │  primary actions
│             format/date status | AI Assistant | Settings │  context + utilities
│ [log app.log close] [log server.log close]               │  tabs (multi-file)
│ Filter ┌──────────────────────────┐ Add filter            │  filter strip
│        │ filter + Enter           │                       │
│                └──────────────────────┘                  │
│ ┌──────┬────────────────────────────┬────┐              │
│ │ eye EE │ ▁▂█▆▅▃ density (full-hgt) │    │              │  timeline
│ │ eye kw1│ ───────────── 1px line    │    │              │  (visibility +
│ │ eye kw2│ ── ▌ ███ ── ▌ occurrences│ del│              │   remove actions)
│ │      │ 10:00      Δ3.3s  10:01   │    │              │   (inter-tick dur)
│ │      │ [══════minimap══════]       │    │              │
│ └──────┴────────────────────────────┴────┘              │
│  1234 T 7  2026-07-19T10:00:01.123Z INFO hello           │
│  1235 T 9  2026-07-19T10:00:01.223Z ERROR boom  ←center  │  log view
│  ...                                  │ Templates        │  (virtualized)
│                                       │ ×9812 T3 GET…    │  (right panel)
│                              ┃ (scroll bar)              │
├──────────────────────────────────────────────────────────┤
│ Pinned · 3 findings        Copy | Save Markdown | Clear   │  bottom panel
│ L1235  GET /api/users 200 OK                  Edit Remove │  (collapsible,
│ L1235  This is the failing request — boom                │   pinned + notes)
└──────────────────────────────────────────────────────────┘
```

### Open a file
- **Drag & drop** any text file (`.log`, `.txt`, `.out`, `.csv`, …) anywhere into the window, or
- click **Open file**.
- **ZIP archives:** drop a `.zip` (or choose it with **Open File**) to extract it in the background into a folder beside the archive, named after the ZIP. Existing folders/files are preserved by choosing a numbered name such as `logs (2)`. The file picker then opens in that folder; select one or more logs to open as tabs. The extraction window shows progress and offers **Cancel**. Canceling extraction removes the incomplete folder; canceling the picker keeps the extracted files for later. Password-protected archives, unsupported compression, damaged ZIPs, and unsafe paths/symbolic links show an error instead of opening binary data as a log.
- A **progress bar** shows real indexing progress (two stages: *Indexing lines*, then *Mining templates & timestamps*). Use **Cancel** if you chose the wrong file.
- Each file opens in its own cohesive **tab** with a log icon and integrated close action. A spinner replaces the tab icon while the file is opening.
- Every open tab watches for appended data, including tabs that are not active. New lines are indexed and template-mined in the background, existing filters scan only the appended range, and the updated document appears atomically. The current view stays responsive while this happens; truncation or in-place edits still require reopening the file.

### Investigation state

Logotomy restores the open tab list and active tab between sessions. Each opened
file also gets a versioned sidecar next to it, named `.filename.log.logotomy`
for a file named `filename.log`. The sidecar stores filters and lane visibility,
search and selection, scroll position, timeline zoom, pins and notes, trim
range, the Templates panel, dock layout, and log font size. It is written
atomically and never changes the source log.

When the log's canonical path, size, or modification time no longer matches the
saved sidecar, notes are restored as text-only entries first. Logotomy asks
before applying old line positions and uses line hashes and timestamps to
best-effort remap anchors. Investigation state is saved automatically when it
changes, at least once per minute, when a tab closes, and when the application
exits.

### Custom date recognizers
logotomy auto-detects the timestamp family (shown as `format: … · date: …` in the top bar). If your log uses a date shape it doesn't recognize, add your own:

1. Open **Settings → Log parsing → Custom date recognizers**.
2. Give the format a **Name**, paste a **Regex** that captures the timestamp using named groups — required `year, month, day, hour, min, sec`, optional `ms` (milliseconds) and `ampm` (for 12-hour `AM`/`PM`). Example for `2026-08-14 4:08:23.668 PM`:
   ```text
   (?P<year>\d{4})-(?P<month>\d{2})-(?P<day>\d{2}) (?P<hour>\d{1,2}):(?P<min>\d{2}):(?P<sec>\d{2})\.(?P<ms>\d{3}) (?P<ampm>[AP]M)
   ```
3. Paste a **sample log line** under it. The window live-verifies the regex and prints the parsed components (`Year: … Month: … Date: … Hour: … Min: … Sec: … MILLI SECOND: …`).
4. Click **Add custom date format** (enabled once the regex matches). It's saved to `~/.logotomy/custom_date_format_list.json` and tried together with the built-in families on the **next file you open**.
5. To apply to the file that's already open, use **Re-scan active log** in the same window (per-tab filters/pins/scroll reset on re-scan).

### Settings and AI Assistant

- **Settings** groups controls into Appearance, Behavior, Log parsing, AI Assistant, and Support. Everyday choices appear first.
- **Advanced parsing** is collapsed by default. Template similarity, header sample lines, and Drain tree depth affect newly opened files; each control explains the tradeoff in plain language.
- **Confirm before deleting filters** is positively worded. Turning it off preserves the existing stored setting and skips future filter confirmations.
- The top-bar **AI Assistant** menu is the single place for live connection status, Start/Stop, copying temporary session instructions, and opening integration guidance. The Settings AI Assistant section exposes the same one-action-at-a-time state without a duplicate disabled Start button.
- Dropdowns and lightweight popovers close with **Esc** or an outside click. Editors and confirmations close with **Esc** and retain an explicit **Cancel** or **Close** action.

### The log view (center)
- Virtualized: a 5-million-line file scrolls as smoothly as a 50-line one.
- Gutter shows the **line number** and the line's **template ID** (`T12`).
- Click any line to select it (white selection bar on the timeline).
- Changing the selected line keeps the current viewport when the target is fully visible, scrolls only enough to reveal a nearby target (within five rows) with a two-row safety margin, and centers a distant target. If manual scrolling moves the selected line out of view, selection follows the nearest visible row.
- **Right-click** any line to pin it or copy the full row, its message without timestamp/header, or a numbered line. Drag-select rows to copy or export the range; use the Log toolbar to export visible/filtered rows or the current timeline range.
- The right edge has a **black scroll-position indicator bar** showing where you are in the file.
- Log lines render in the embedded **Space Mono** monospace font (SIL OFL 1.1 — see the README thanks section); the **A− / A+** toolbar buttons change the size from 8 to 24 px.
- Choose **Settings → Long log lines** to truncate, horizontally scroll, or wrap complete long records. **Truncate** remains the default: it safely shows a 2,000-byte Unicode-aware prefix and a `…` control that opens the complete-line inspector. **Horizontal scroll** keeps each full record on one row with a bottom scrollbar; **Wrap** shows the complete record across visual rows.
- Truncated rows show `… match` when a Find result is beyond the visible prefix. Open the full-line inspector to view and copy the complete source text.
- Structured values embedded anywhere in visible log records are detected in the background: JSON, single or grouped logfmt/key-value events, conservative colon fields, Swift/Foundation/Python/JVM debug values, XML and labeled OpenStep property lists, HTTP, protobuf text, stack traces, and JWT/Base64/hex/PEM. Double-, single-, and backtick-quoted values, URL/path values, and compact or multiline payloads are supported; parsing never runs in the paint path.
- A translucent format cue (**JSON**, **KV**, **HTTP**, **TRACE**, etc.) scales with the active log font and sits immediately above each payload's opening token without reserving layout space or shifting log text. Multiline payloads receive one cue at their opener, while every exact visible source fragment receives a subtle underline; gaps between grouped fields remain untouched.
- Rest the pointer over an underlined source fragment or its cue for about 0.8 seconds to open a semi-transparent source callout. Structured key/value entries appear one per line; the callout grows to fit modest payloads but caps its width/height and truncates oversized previews, while **Open inspector** provides the complete value. It points back to the source and offers **Copy source**; once visible it stays locked while you cross dense highlights on the way to the callout, then dismisses after leaving the source-to-callout region. Press **Esc** or click outside the callout to close it immediately. Clicking a cue still opens the inspector immediately.
- Explicit timestamps use a quieter blue underline and no gutter cue. Their delayed callout identifies the detected timestamp family and can copy either the exact source or its normalized UTC value. Forward-filled continuation lines are not marked because they contain no timestamp source.
- The Apple profile understands classic Foundation `{ key = value; }` / multiline array descriptions as well as Swift `Type(field: value)`, nested `Optional(...)`, enum associated values, labeled tuples, `[...]` collections, `<NSObject: address; …>` descriptions, and multiline `dump`/Mirror trees. Ordinary `(foo, bar)` prose and `[Subsystem:Category]` tags are deliberately left alone.
- XML plists and OpenStep collections explicitly labeled `plist`/`propertyList` open as normalized trees. A **BPLIST** cue identifies literal, hex, or Base64 `bplist00` data; its safe summary is immediate, while **Decode preview** only decodes the text transport and does not automatically interpret the binary object table.
- Structured values open in **Pretty** by default and offer Tree/Pretty/Raw; the last selected tab is remembered for future payloads and sessions. Stack traces start in Frames; encoded values start with a safe Summary and require **Decode preview**. The inspector sizes itself to the payload, can be resized from its edges, and scrolls when needed. Copy the exact raw payload, formatted value, or an individual tree value. Press **Esc**, click outside, or use the close icon to dismiss it.
- Filtered views still analyze the contiguous physical log record—hidden filter rows are never joined into artificial JSON. Timestamped multiline records retain a compact boundary index so jumping directly into the middle can recover the opener.
- Filters you add (below) are **highlighted inline**, in the filter's color.
- **Find group** (in the Log toolbar): the search icon is attached to the input. Choose **Text (Aa)** for exact-case text, **Text (Ab)** for ASCII case-insensitive text, **Regex** for a bounded Rust regular expression, or **Template ID** for `42`, `T42`, or `T{42}`. The selected matcher searches visible log lines using the same rules as Timeline filters; regexes and Template IDs are validated as you type and whenever the type changes. Press **Enter** to run a valid search. Amber highlights mark text/regex spans and whole Template ID rows. Use the close icon or **Esc** to cancel the active search; with multiple results, use the previous/next buttons (or Up/Down arrows) to step through matches.
- **Double-click any word** in a log line to highlight every occurrence of that word in cyan. The word is also pre-filled into the search box — press **Enter** to turn it into a full search. Single-click anywhere clears the keyword highlight.

### Keyboard navigation and Commands
- **⌘/Ctrl+O** opens a file; **⌘/Ctrl+W** closes the active tab. Before closing, Logotomy saves the investigation sidecar; if that cannot be saved, it offers retry, discard, or cancel rather than silently losing notes and filters.
- **Ctrl+Tab** and **Ctrl+Shift+Tab** cycle open log tabs. **⌘/Ctrl+F** focuses Find; **F3** / **Shift+F3** move to the next/previous search result.
- **⌘/Ctrl+L** or **⌘/Ctrl+G** opens Go to Line or Time. Enter a one-based line number, an RFC3339 timestamp, or `@` followed by epoch seconds/milliseconds; a timestamp jumps to the nearest log record.
- **Home/End** select the top/bottom currently rendered row; **⌘/Ctrl+Home/End** jump to the first/last line in the current filtered view.
- Select a timeline filter lane and press **Space** to toggle it. Select a pin card or filter lane and press **Delete** to remove it; **⌘/Ctrl+Z** restores the latest deletion in that tab. Filter deletion still honors the confirmation preference in Settings.
- Click **Commands** in the top bar or press **⌘/Ctrl+Shift+P** to search and run app commands. Below 1100px wide, **Recent files**, **Saved filters**, and **Commands** move into the labeled **More** menu while **Open file** remains visible. Press **?** for a keyboard and mouse gesture cheat sheet.

### Filter bookmarks
- The Timeline header begins with a recognizable **Filter** icon and input.
- Type a filter in the text field and press **Enter** (e.g. `ERROR`, `timeout`, `user_id=42`) or click **Add filter**.
- Up to **20 filters** are supported. The input is disabled once the cap is reached.
- Scanning happens in the background (spinner while chewing).
- Each filter remains editable after adding it. Use **Aa** for case-sensitive
  matching, **Exclude** to subtract its matches, and **.*** for a regular
  expression. Regexes are validated before scanning and use Rust's linear-time
  regex engine with compilation-memory limits.
- Choose **Match: any/all** to union or intersect active include filters.
  Exclude filters are always subtracted afterwards. Focus an empty filter
  field to choose a recent filter that is not already active; choosing one
  starts its scan immediately.
- Enable **Current timeline range** to scope a new scan to the current zoom;
  scans always respect an applied trim. Long scans show their line count and
  can be cancelled without blocking the log view.

### The timeline
- The timeline is a **fixed-height panel** at the top — it always shows the full histogram, all filter lanes, axis labels, and the zoom/minimap strip, and can never be shrunk to hide lanes. Its height grows/shrinks with the number of filters.
- Shows the whole file as a full-height density histogram, with one **colored lane per filter**. Each lane has a **straight 1px line** in the filter's color across the full lane width. A bucket containing one visible match shows a **2px × 7px marker at the occurrence's exact time/line position**. Multi-occurrence buckets are edge-to-edge rectangles spanning the bucket's full timeline width: small is **7px** high, medium **10px**, and dense **13px**. Adjacent non-empty buckets therefore read as a continuous density strip. Exact counts remain available on hover, and zooming re-resolves the buckets until distinguishable occurrences become exact markers.
- **Left column** shows filter names (up to 14 chars) with visible/hidden SVG controls; enabled lanes also use bold labels, so state is not communicated by color alone. Click to toggle. The first lane is "Everything Else" — it has the visibility control but **cannot be removed**.
- The otherwise-empty **Everything Else** lane also shows pinned evidence with pin markers at the first and last selected log rows (one marker for a single-row pin). Markers remain available without filters, use line positions for timestamp-less logs, and follow the active zoom window.
- Hover a pin marker to read its saved analysis and pinned line range. Click it to jump the Log view to the pin's first selected row, then select and reveal the corresponding card in the **Pinned** tab.
- **Hover a filter's name/eye** to see a tooltip with the **full filter text** and its **total match count**, e.g. `Some Filter (334 occurrences)`.
- Each filter lane has a recognizable remove control on the right of its label. **Settings → Behavior → Confirm before deleting filters** controls whether Logotomy asks first; it maps to the existing persisted preference for compatibility.
- Click a filter lane, exact occurrence marker, or density bucket to select the lane. The Timeline header shows previous/next navigation with Left/Right arrows whenever the current Log View line is an occurrence in that lane. The current occurrence is derived from the selected lane and current line; no separate occurrence-selection marker is stored.
- **Lower-left filter actions** (shown beneath the filter labels while filters exist) keep bulk visibility and destructive actions out of the compact Timeline header:
  - **Disable all / Enable all** toggles every filter lane at once; the **Everything Else** lane is never touched.
  - **Clear filters** removes every filter (behind a confirmation popup when confirmation is enabled).
- The visible gesture hint reads **“Scroll to zoom · Drag to pan · Shift-drag to select”**; zoom, pan, and reset controls keep detailed tooltips.
- **Zoom** — scroll anywhere over the timeline. Zoom is continuous, pointer-anchored, works on both trackpads and mouse wheels, and can reach millisecond/individual-line detail.
- **Pan** — drag left/right (without shift). The visible span is preserved and snaps at the file boundaries.
- **Brush select** — shift+drag to draw a rectangle; on release, zooms to that range.
- **Reset zoom** — double-click anywhere on the timeline, or click the ↺ button.
- **Minimap** — click anywhere on the minimap to jump to that position.
- **Hover** the timeline for re-resolved visible-column details (time/line position, line count, and per-filter counts). Density buckets also show their exact occurrence count and boundary lines.
- **Click** the timeline background → jumps to the nearest real log line; clicking a filter marker/bucket jumps to the nearest occurrence in that lane.
  A white marker shows your current position.
- **Axis labels** are smart: if all ticks share the same hour, only `MM:SS.ms` is shown.
- **Inter-tick duration labels** (e.g. `Δ 3.3s`, `Δ 1m 34s`) appear between each pair of tick labels.

### Bottom panel (pinned lines + analyses)
- **Right-click** any log line → context menu:
  - **📌 Pin** — opens a movable, resizable analysis bubble anchored to the line. Enter saves the optional analysis note; Escape or clicking elsewhere cancels.
- Drag across log rows to open an anchored **Pin / Copy / Copy + lines / Cancel** bubble. Choosing **Pin** switches it to the analysis editor; the selected rows remain highlighted until the bubble is saved or cancelled.
- The analysis editor uses `Add analysis/info or enter to save` as its placeholder and can save a pin without any analysis text.
- Expand/collapse the panel with the **▼/▶** header. When collapsed, shows counts.
- When empty, shows a brief hint to right-click a log line and pin it.
- Pinned lines show **line number + text snippet**; use the SVG **Edit** action to reopen the pin window and change its comment/lines, or **Remove** to unpin.
- Selecting a Timeline pin marker opens this card and scrolls it into view; the matching Log row is selected first, even if current filter visibility would otherwise hide it.
- GUI-attached AI agents can read existing entries with `get_analysis()` and add user-visible entries with `add_analysis({text, lines})`. An empty `lines` array creates a text-only analysis card at the top of this panel.
- **"Clear all"** empties everything and collapses the panel.

### Templates tab
- logotomy runs the **Drain template-mining algorithm** natively in Rust while loading.
- The dockable **Templates** tab starts beside **Pinned** and can be dragged or opened in a separate window like the other views. Each active dock leaf has exactly one external-window action at its right edge; closing the separate window restores the prior dock layout.
- Search template IDs or patterns; sort by count, first seen, last seen, or rarity.
- The Pinned tab is the initially visible lower dock tab for every newly opened log; Templates remains directly beside it.
- Select a template to move the Log view to its occurrence nearest the current viewport midpoint. Up/down changes the selected template row; left/right moves through its occurrences.
- The selected row shows an inline action strip to search its Template ID in the Log tab, add a Template ID filter, or copy its full pattern. Left/Right still moves through occurrences.
- Rare, late-arriving, and bursty templates are marked directly in the list.
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
and uses the returned `log_id`. To analyze the log open in the GUI, open the **AI Assistant** menu, start the connection, copy
the GUI session instruction, and paste it into the agent conversation. The agent calls
`attach_gui_session`, then `session_info`; while `mode` is `gui_attached`, it must not call
`load_log` or pass `log_id`. Stop the connection from the same menu when finished to invalidate the temporary session ID.

In GUI-attached mode, the GUI already provides the open log: `load_log`, `list_logs`, and
`close_log` are unavailable, and `log_id` is not needed. A useful approach is to understand the
user's question and Pin-tab findings with `get_analysis()`, explore log shape with
`summarize_log(with_filtered_log: false)` and targeted `find_occurrences`, then apply filters
with `filters_add` or
`trim` when they help test a hypothesis. Request bounded `raw_log` only when exact evidence is
needed, and use `add_analysis({text, lines})` to post useful evidence-backed root-cause findings.

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
| `find_occurrences` | Paginated `[line_number, epoch_ms\|null]` matches for `keyword`; supports `offset`, `max_results`, `after`/`before`, `with_filtered_log`, and ASCII `case_sensitive` mode |
| `get_analysis` / `add_analysis` | (GUI-attached) Read or add the current Pin-tab analysis cards; `add_analysis({text, lines:[]})` adds a text-only card at the top |
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
- Line indexes are stored as 32-bit values; a single source is limited to 4,294,967,295 lines.
- Embedded JSON analysis is debounced while scrolling, cancellable, and worker-only. GUI record scans are bounded to 8 MiB / 20,000 lines, 128 nesting levels, 20,000 normalized nodes, and 64 results per physical interval; values exceeding a bound are not presented as valid detections.
- Tested edge cases: CRLF endings, missing trailing newline, blank lines,
  multi-MB single lines, invalid UTF-8 (lossy display), files with no timestamps.

## 5. Keyboard & mouse cheat sheet

| Action | Effect |
|---|---|
| Drop file on window | Open in new tab |
| Drop `.zip` on window | Extract beside the ZIP, then choose logs from the extracted folder |
| Click tab | Switch file |
| Tab close button | Close file |
| Filter box + Enter or **Add filter** | Add a text/regex filter, or choose **Template ID** and enter `42`, `T42`, or `T{42}` |
| Click timeline | Jump to nearest filter match, white marker shows position |
| Scroll on timeline | Zoom in/out (continuous, trackpad + mouse) |
| Drag (no shift) on timeline | Pan left/right |
| Shift+drag on timeline | Brush-select range → zoom to selection |
| Double-click timeline | Reset zoom to full range |
| Click minimap | Pan view to that position |
| Click log line | Select line (white line on timeline) |
| Right-click log line | Open copy, pin, inspect, and trim actions |
| Click lane visibility icon | Toggle filter lane on/off (filters log view) |
| Hover filter name/eye | Tooltip with full filter text + match count |
| Click Remove on a filter lane | Remove that filter (with confirmation when enabled) |
| Timeline **Clear filters** | Remove every filter (with confirmation when enabled) |
| Timeline **Enable all / Disable all** | Toggle every filter lane at once |
| Pinned **Edit** button | Reopen the pin window to edit the pin |
| A− / A+ buttons | Decrease / increase log text size |
| Templates tab → right-click row | Navigate, copy, sample, or filter that template |
| Pinned header expand/collapse button | Expand / collapse pinned lines and analyses |
| External Window button | Open the active dock view in a separate window; close it to restore the dock layout |

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
