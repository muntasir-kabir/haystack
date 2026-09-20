# Haystack — User Guide

*A high-performance log analyzer for when you open a giant log file and need to find the needle.*

Haystack chews through 50MB+ text/log files without breaking a sweat, mines the
structure out of them automatically, and gives you an interactive timeline to
see exactly when things went sideways.

---

## 1. Installation & Running

### macOS

```bash
# Download the latest release .dmg (Apple Silicon: _aarch64, Intel: _x86_64),
# open it and drag haystack.app into Applications.
open /Applications/haystack.app     # Launch the GUI
```

### Windows

```bash
# Download haystack-<version>-setup.exe and run the NSIS installer.
# It installs haystack.exe (with app icon); launch from the Start Menu.
```

### Ubuntu / Linux

```bash
# .deb:  sudo apt install ./haystack_<version>_amd64.deb
# or .AppImage:  chmod +x haystack-<version>-x86_64.AppImage && ./haystack-<version>-x86_64.AppImage
haystack    # Launch the GUI
```

### From source (any platform)

Prereqs: a working [Rust toolchain](https://rustup.rs) (1.85+).

```bash
# Build and run the GUI (recommended for big files)
cargo run --release

# Just build the binary
cargo build --release
# → target/release/haystack        (the GUI)
```

### Open a file from the shell or file manager

Pass one or more paths to the GUI:

```bash
haystack app.log another.log
```

The first GUI process is the standalone instance. If it is already running,
opening a file from Finder, Windows Explorer, a Linux desktop-file association,
or another shell command reuses that window, focuses it, and opens the file in
a new tab instead of starting another GUI process.

Release installers register `.log`, `.txt`, `.out`, `.err`, `.csv`, `.json`,
`.xml`, and `.md` with haystack. After installation,
use **Open with** or **Set as default** in the operating system’s file manager.
The Linux `.deb`/AppImage association depends on the desktop environment’s MIME
database; the Windows and macOS installers register the extensions directly.

### MCP server mode (for AI assistants)

The same binary also runs the MCP server:

```bash
./haystack --mcp                          # stdio mode (standalone or GUI-attached)
```

Build MCP-only (Linux/Windows cross-compile):

```bash
cargo build --release --no-default-features
```

### Extract templates from the shell

`extract_template` is a standalone process for batch pipelines. It uses the
same backend record parser, masking, and native Drain miner as the application;
it neither opens nor changes the GUI.

```bash
haystack extract_template \
  --file /var/log/app.log \
  --format '{time} {log}' \
  --store /var/lib/haystack/app-templates.json \
  --output /tmp/app-template-sequence.json
```

`--file`, `--store`, and `--output` are required. `--format` is optional and
defaults to `{time} {log}`; it uses the same log-format-template grammar as the
Format editor. Keep one store for logs with the same normalized message layout.
The store is a versioned JSON object containing template IDs, patterns, and
cumulative counts. It is read before mining, so matching templates keep their
IDs across separate command invocations, while new shapes are appended. The
output is also JSON and contains one `template_ids` entry per physical source
line, in source order. The command's stdout is a JSON summary; failures are
JSON objects on stderr.

---

## 2. The GUI at a glance

```
┌──────────────────────────────────────────────────────────┐
│ Haystack | Open file | Recent | Saved filters | Commands │  primary actions
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
- Every open tab watches for appended data, including tabs that are not active. New lines and an amended final partial line are reanalyzed in the background; filters rescan from the first changed line so an old hit is replaced rather than duplicated. The updated document appears atomically and the current view stays responsive. Truncation or in-place edits still require reopening the file.

### Investigation state

Haystack restores the open tab list and active tab between sessions. Each opened
file also gets a versioned sidecar next to it, named `.filename.log.haystack`
for a file named `filename.log`. The sidecar stores filters and lane visibility,
every Log View's search, selection, and scroll position, the focused Log View,
timeline zoom, pins and notes, trim range, the Templates panel, dock/detached
layout, and log font size. It is written atomically and never changes the
source log.

When the log's canonical path, size, or modification time no longer matches the
saved sidecar, notes are restored as text-only entries first. Haystack asks
before applying old line positions and uses line hashes and timestamps to
best-effort remap anchors. Investigation state is saved automatically when it
changes, at least once per minute, when a tab closes, and when the application
exits.

If saved investigation settings cannot be used, Haystack explains the problem
before opening the file. Choose **Continue & repair** to use automatic format
detection and remove only an invalid saved log format, while retaining the
rest of the investigation. A malformed sidecar can be reset after a successful
open; newer-version or unreadable sidecars are never overwritten automatically.

### Custom date recognizers
haystack auto-detects the timestamp family (shown as `format: … · date: …` in the top bar). Detection uses bounded leading and distributed samples, so two consistent record headers can identify a sparse multiline file without requiring timestamps on a percentage of physical lines. Ambiguous layouts stay unresolved instead of being chosen by registration order. If your log uses a date shape it doesn't recognize, add your own:

1. Open **Settings → Log parsing → Custom date recognizers**.
2. Give the format a **Name**, paste a **Regex** that captures the timestamp using named groups — required `year, month, day, hour, min, sec`, optional `ms` (milliseconds) and `ampm` (for 12-hour `AM`/`PM`). Example for `2026-08-14 4:08:23.668 PM`:
   ```text
   (?P<year>\d{4})-(?P<month>\d{2})-(?P<day>\d{2}) (?P<hour>\d{1,2}):(?P<min>\d{2}):(?P<sec>\d{2})\.(?P<ms>\d{3}) (?P<ampm>[AP]M)
   ```
3. Paste a **sample log line** under it. The window live-verifies the regex and prints the parsed components (`Year: … Month: … Date: … Hour: … Min: … Sec: … MILLI SECOND: …`).
4. Click **Add custom date format** (enabled once the regex matches). It's saved to `~/.haystack/custom_date_format_list.json` and tried together with the built-in families on the **next file you open**.

### Log format templates

The top-bar **Format** button shows the selected file's applied format and
syntax. Choose a saved format to reparse that file in the background, or choose
**Auto-detect** to return to automatic recognition. The old view stays available
until parsing succeeds; a format that matches no record headers is not installed.

Open **Settings → Log parsing → Log format templates** to define which physical
lines begin records. The editor groups Name, collapsed **Template rules & syntax**,
Template and its validation together, then Sample log and Parsing preview. Advanced
options and Diagnostics are collapsed below the preview. New formats start with
`{time} {log}`; paste a sample and write a self-contained template such as
`{time} MyApp[{a}:{b:number}] <{loglevel}> {log}`. Any `{name}` is a token by
default; add `:number`, `:path`, or `:text` only when needed. You never need to
define a custom header field before using it. The editor offers field
and type completion, keeps normal caret navigation while suggestions are visible,
and highlights literals, names, and types while you type. Use Ctrl/Cmd+Space to
reopen completion. `{ignore}` matches a header value without retaining it as a
captured field; use `{ignore:number}` or `{ignore:path}` when its type matters.
Ignored values cannot be searched or filtered in Field mode, while raw text
search still sees the original log source. Preview work is revision-safe: a prior result remains visible
as updating while the latest template is checked, and a rename does not reparse the
sample. `{time}` and
`{log}` are required for new formats, and `{log}` must be last. The **Examples**
menu offers Basic, Named fields, Ignore header values, and Quoted path starters;
it changes only the template and sample, so the name you entered stays intact.
Diagnostics report header/continuation results and how many ignored values matched.
Advanced options contain timestamp-related settings only; header fields are
declared directly in the template.

A format name is required before it can be saved; the editor marks the missing
name and explains the disabled Save button. Template suggestions never take
over Left/Right navigation or Backspace: those keys continue editing normally.
Invalid saved formats from an earlier development build are removed
automatically; Haystack keeps valid saved formats and records the repair in its
application log.

When an applied format recognizes fewer than 90% of source log lines, the top
bar **Format** control turns amber with a warning sign. Open it to see the
matched-line percentage and use **Edit** to check and update the template.
For yearless BSD syslog, logcat threadtime, and glog dates, the assumed year is
shown in preview and locked when the file opens. You can set an explicit year
in the editor; appending data will not silently change that assumption.
The multiline sample preview shows extracted fields, exact source spans, record
starts, and explicit/inherited/missing time. Preview work is capped at 200 lines
or 256 KiB and runs after a short pause in editing.

**Save preset** stores a reusable, versioned format in
`~/.haystack/record_profiles.json`. **Use for this file / Apply to open file**
reparses the file in the background; the current view stays usable until the
new parse succeeds. The chosen profile is embedded in the file's investigation
sidecar, so removing a preset does not silently change that file's parsing.
Pins, line-based zoom/trim/scroll, and compatible text/regex/Field filters are
restored. Incompatible Field and Drain Template-ID filters are disabled for
review because fields may change type and re-mining may change IDs.
Legacy custom date recognizers appear as timestamp-first starter layouts;
prefix-dependent ones need an explicit header prefix and a checked preview.
5. To apply to the file that's already open, use **Re-scan active log** in the same window (per-tab filters/pins/scroll reset on re-scan).

### Settings and AI Assistant

- **Settings** groups controls into Appearance, Log parsing, AI Assistant, and Support. Appearance contains View and Filters; Timeline overview presentation is fixed and is not configurable. The full-file minimap remains visible with the default theme-aware density-bar colour and opacity. Filter bulk actions remain available whenever filters exist.
- The Settings dropdown stays compact rather than expanding to the bottom of a large window; use its vertical scrollbar for the remaining groups.
- **Advanced parsing** is collapsed by default. Template similarity, header sample lines, and Drain tree depth affect newly opened files; each control explains the tradeoff in plain language.
- **Confirm before deleting filters** is positively worded. Turning it off preserves the existing stored setting and skips future filter confirmations.
- The top-bar **AI Assistant** menu is the single place for live connection status, Start/Stop, copying temporary session instructions, and opening integration guidance. The Settings AI Assistant section exposes the same one-action-at-a-time state without a duplicate disabled Start button.
- Dropdowns and lightweight popovers close with **Esc** or an outside click. The dismissal click is consumed, so it cannot select a log row or activate a control underneath. Editors, inspectors, progress windows, and confirmations are centered modals: the rest of the application, including detached views and global shortcuts, remains inactive until the modal closes. Editors retain an explicit **Cancel** or **Close** action and honor **Esc**.

### The log view (center)
- Use the standalone **+** button at the right side of a dock tab bar to open another Log View in that dock leaf. Views of the same file share the document, filters, timeline, pins, templates, trim, parser, and live-tail updates, while each keeps its own selected line, scroll position, row selection, Find query/results, keyword highlight, and inspectors.
- With more than one view, tabs keep stable **Log 1**, **Log 2**, … labels and show a close action. Closing a Log View removes only that view; the final Log View cannot be closed. This is separate from the top file tab, and **⌘/Ctrl+W** still closes the whole file investigation.
- The main workspace has fixed dock homes: **Log Views** occupy the upper panel, while **Pinned** and **Templates** share the lower panel. Drag-and-drop remains available for the allowed panel, and incompatible split-side markers are not offered.
- Use the External Window action beside any Log tab to detach it. The separate native window opens as a dock panel with that Log View filling the initial leaf; use its dock add button and drag tabs to arrange additional Log Views in any direction. Drag a Log tab into another native Log View window to dock it there; dragging one over the main window shows a top-panel drop marker, including when every Log View is detached. Its Log tab contains a left-arrow action to return the whole window layout to the main window, while its normal close action permanently removes the view when another remains. An emptied detached window closes after its final Log View is transferred; closing a native child window also re-docks that view.
- Open **More → Focus Log View** or use the command palette to temporarily give the focused, docked Log View the workspace. It hides the Timeline and other dock panes without changing their layout; use **Exit focus** or **Esc** to restore them.
- Timeline markers, viewport shadow, Find shortcuts, Go to, template/pin navigation, and other location-based commands follow the currently or most recently focused Log View. Rendering or focusing Pinned/Templates does not move another Log View. Filter, trim, reparse, append, and MCP document changes remain shared and reconcile every view independently.
- Virtualized: a 5-million-line file scrolls as smoothly as a 50-line one.
- Gutter shows the **line number** and the line's **template ID** (`T12`).
- Click any line to select it (white selection bar on the timeline).
- **Shift-click** a line to open a scrollable occurrence context. It centers that line and shows the nearest previous and next occurrence of every configured filter, while retaining normal log highlighting and embedded-data cues. Press **Esc**, click outside, or use the close icon to return to the Log View.
- Changing the selected line keeps the current viewport when the target is fully visible, scrolls only enough to reveal a nearby target (within five rows) with a two-row safety margin, and centers a distant target. If manual scrolling moves the selected line out of view, selection follows the nearest visible row.
- **Right-click** any line to pin it or copy the full row, its message without timestamp/header, or a numbered line. Drag-select rows to copy or export the range; use the Log toolbar to export visible/filtered rows or the current timeline range.
- The right edge has a **black scroll-position indicator bar** showing where you are in the file.
- Application controls and UI text render in the embedded **Inter Regular** font. Log lines render in the embedded **Space Mono** monospace font (both are SIL OFL 1.1 — see the README font credits). Use the Log View toolbar's **Text** menu for **A− / A+**, the current font size, and **Reset text size**; the size remains independent for each view.
- Find is labeled **Find in this view** and filtering is labeled **Add filter**. Text modes state whether they are case-sensitive, while Regex, Template ID, and Field remain explicit. Enter is shown as a small key hint; the status beside the navigation arrows distinguishes validation, background searching, no matches, and completed counts.
- The Log View toolbar exposes **Long lines** beside the viewer. **Truncate** remains the default: it safely shows a 2,000-byte Unicode-aware prefix, marks the row as a preview, and provides a `…` control plus the context menu for the complete-line inspector/copy actions. **Horizontal scroll** keeps each full record on one row with a bottom scrollbar; **Wrap** shows the complete record across visual rows.
- File tabs use restrained surfaces and an active underline, separate from docked Log/Timeline/Pinned/Templates tabs. Export stays secondary and its menu names the supported scope: selected rows, the current view (filtered when applicable), or the current timeline range.
- Truncated rows show `… match` when a Find result is beyond the visible prefix. Open the full-line inspector to view and copy the complete source text.
- Structured values embedded anywhere in visible log records are detected in the background: JSON, single or grouped logfmt/key-value events, conservative colon fields, Swift/Foundation/Python/JVM debug values, XML and labeled OpenStep property lists, HTTP, protobuf text, stack traces, and JWT/Base64/hex/PEM. Double-, single-, and backtick-quoted values, URL/path values, and compact or multiline payloads are supported; parsing never runs in the paint path.
- A quiet dotted source cue marks inspectable payloads (**JSON**, **KV**, **HTTP**, **TRACE**, etc.) without adding persistent green underlines or shifting log text. Multiline payloads receive one cue at their opener, while exact visible source fragments remain available to the inspector.
- Rest the pointer over a dotted source cue for about 0.8 seconds to open a semi-transparent source callout. Structured key/value entries appear one per line; the callout grows to fit modest payloads but caps its width/height and truncates oversized previews, while **Open inspector** provides the complete value. It points back to the source and offers **Copy source**; the row context menu also exposes **Inspect structured data**, so access is not hover-only. Press **Esc** or click outside the callout to close it immediately.
- Explicit timestamps use the same quiet local cue. Their delayed callout identifies the detected timestamp family and can copy either the exact source or its normalized UTC value. Forward-filled continuation lines are not marked because they contain no timestamp source.
- The Apple profile understands classic Foundation `{ key = value; }` / multiline array descriptions as well as Swift `Type(field: value)`, nested `Optional(...)`, enum associated values, labeled tuples, `[...]` collections, `<NSObject: address; …>` descriptions, and multiline `dump`/Mirror trees. Ordinary `(foo, bar)` prose and `[Subsystem:Category]` tags are deliberately left alone.
- XML plists and OpenStep collections explicitly labeled `plist`/`propertyList` open as normalized trees. A **BPLIST** cue identifies literal, hex, or Base64 `bplist00` data; its safe summary is immediate, while **Decode preview** only decodes the text transport and does not automatically interpret the binary object table.
- Structured values open in **Pretty** by default and offer Tree/Pretty/Raw; the last selected tab is remembered for future payloads and sessions. Stack traces start in Frames; encoded values start with a safe Summary and require **Decode preview**. The inspector sizes itself to the payload, can be resized from its edges, and scrolls when needed. Copy the exact raw payload, formatted value, or an individual tree value. Press **Esc**, click outside, or use the close icon to dismiss it.
- Filtered views still analyze the contiguous physical log record—hidden filter rows are never joined into artificial JSON. Timestamped multiline records retain a compact boundary index so jumping directly into the middle can recover the opener.
- Filters you add (below) are **highlighted inline**, in the filter's color.
- **Find in this view** (in the Log toolbar): the search icon is attached to the input. Choose **Text (case-sensitive)**, **Text (ignore case)**, **Regex** for a bounded Rust regular expression, or **Template ID** for `42`, `T42`, or `T{42}`. The selected matcher searches visible log lines using the same rules as Timeline filters; regexes and Template IDs are validated as you type and whenever the type changes. Press **Enter** to run a valid search. Amber highlights mark text/regex spans and whole Template ID rows. Use the close icon or **Esc** to cancel the active search; with multiple results, use the previous/next buttons (or Up/Down arrows) to step through matches.
- Choose **Field** in Find or Timeline Filter to query a captured header field: `process = "12345"`, `b >= 9`, `loglevel = "FAULT"`, `source contains "CameraService"`, or `source matches "Camera.*"`. `{log}` is intentionally not offered as Field criteria; use Text or Regex mode for message-body searches. Type a field name to see available fields; after an operator, suggestions come from values actually captured in this file and current trim, before active filters are composed. Suggestions are bounded and say when coverage is limited; exact searches are not capped by the suggestion limit. `exists` and `missing` take no value. Text/regex modes search physical lines, while Field mode selects every physical row of a matching record, including continuations. Find shows both matching record and row counts; filter lanes show physical-row counts. Case mode applies to text/path values, and numeric comparisons preserve large integer precision. Right-click a row → **Captured fields** to copy any captured value or start a Field search/filter for header fields. Recent Field searches replay in Field mode, separate from plain-text history.
- **Double-click any word** in a log line to highlight every occurrence of that word in cyan. The word is also pre-filled into the search box — press **Enter** to turn it into a full search. Single-click anywhere clears the keyword highlight.

### Keyboard navigation and Commands
- **⌘/Ctrl+O** opens a file; **⌘/Ctrl+W** closes the active tab. Before closing, Haystack saves the investigation sidecar; if that cannot be saved, it offers retry, discard, or cancel rather than silently losing notes and filters.
- **Ctrl+Tab** and **Ctrl+Shift+Tab** cycle open file tabs. **⌘/Ctrl+F** focuses Find in the focused Log View; **F3** / **Shift+F3** move through that view's search results.
- **⌘/Ctrl+L** or **⌘/Ctrl+G** opens Go to Line or Time. Enter a one-based line number, an RFC3339 timestamp, or `@` followed by epoch seconds/milliseconds; a timestamp jumps to the closest valid record header, with ties going to the earliest source line. This lookup is independent of timeline coordinates and remains correct when clocks repeat or move backward.
- **Home/End** select the top/bottom currently rendered row; **⌘/Ctrl+Home/End** jump to the first/last line in the current filtered view.
- Select a timeline filter lane and press **Space** to toggle it. Select a pin card or filter lane and press **Delete** to remove it; **⌘/Ctrl+Z** restores the latest deletion in that tab. Filter deletion still honors the confirmation preference in Settings.
- Click **Commands** in the top bar or press **⌘/Ctrl+Shift+P** to search and run app commands. Below 1100px wide, **Recent files**, **Saved filters**, and **Commands** move into the labeled **More** menu while **Open file** remains visible. Press **?** for a keyboard and mouse gesture cheat sheet.

### Filter bookmarks
- The Timeline header begins with a recognizable **Filter** icon and input.
- Type a filter in the text field and press **Enter** (e.g. `ERROR`, `timeout`, `user_id=42`) or click **Add filter**.
- Up to **20 filters** are supported. The input is disabled once the cap is reached.
- Scanning happens in the background (spinner while chewing).
- Each filter remains editable after adding it. Choose **Text (case-sensitive)**
  or **Text (ignore case)**, use **Exclude** to subtract matches, and **.*** for a regular
  expression. Regexes are validated before scanning and use Rust's linear-time
  regex engine with compilation-memory limits.
- Field filters and the active Field search are saved in the investigation sidecar. Saved filter sets retain Field expressions; when a new log format removes a field or changes its type, the affected field lane is disabled for review rather than treated as ordinary text. Applying an old saved set to an incompatible file likewise keeps the expression visible but inactive.
- Choose **Match: any/all** to union or intersect active include filters.
  Exclude filters are always subtracted afterwards. Focus an empty filter
  field to choose a recent filter that is not already active; choosing one
  starts its scan immediately.
- Enable **Current timeline range** to scope a new scan to the current zoom;
  scans always respect an applied trim. Long scans show their line count and
  can be cancelled without blocking the log view.

### The timeline
- The timeline is a **fixed-height panel** at the top — it always shows the full histogram, all filter lanes and axis labels, plus the zoom/minimap strip when enabled, and can never be shrunk to hide lanes. Its height grows/shrinks with the number of filters and reclaims the minimap space when it is hidden.
- Shows record-start counts across the file's physical-line axis, with one **colored lane per filter** counting matching physical lines. Each lane has a quiet orientation baseline; full-strength lane colour is reserved for occurrence markers and density buckets. A bucket containing one visible match shows a **2px × 7px marker at that source-line position**. Multi-occurrence buckets are edge-to-edge rectangles spanning the bucket's full timeline width: small is **7px** high, medium **10px**, and dense **13px**. Adjacent non-empty buckets therefore read as a continuous density strip. Exact counts remain available on hover, and zooming re-resolves the buckets until distinguishable occurrences become exact markers.
- **Left column** shows left-aligned filter names with monochrome visible/hidden SVG controls and small identity swatches; long values remain available in the hover tooltip. Click to toggle. The first lane is "Everything Else" — it has a neutral swatch and visibility control but **cannot be removed**.
- The otherwise-empty **Everything Else** lane also shows pinned evidence with pin markers at the first and last selected log rows (one marker for a single-row pin). Markers remain available without filters, use line positions for timestamp-less logs, and follow the active zoom window.
- Hover a pin marker to read its saved analysis and pinned line range. Click it to jump the Log view to the pin's first selected row, then select and reveal the corresponding card in the **Pinned** tab.
- **Hover a filter's name/eye** to see a tooltip with the **full filter text** and its **total match count**, e.g. `Some Filter (334 occurrences)`.
- Each filter lane has a recognizable remove control on the right of its label. **Settings → Behavior → Confirm before deleting filters** controls whether Haystack asks first; it maps to the existing persisted preference for compatibility.
- Click a filter lane, exact occurrence marker, or density bucket to select the lane. The Timeline header shows previous/next navigation with Left/Right arrows whenever the current Log View line is an occurrence in that lane. The current occurrence is derived from the selected lane and current line; no separate occurrence-selection marker is stored.
- **Lower-left filter actions** (shown beneath the filter labels while filters exist) keep bulk visibility and destructive actions out of the compact Timeline header:
  - **Disable all / Enable all** toggles every filter lane at once; the **Everything Else** lane is never touched.
  - **Clear filters** removes every filter (behind a confirmation popup when confirmation is enabled).
- **Zoom** — scroll anywhere over the timeline. Zoom is continuous, pointer-anchored, works on both trackpads and mouse wheels, and can reach individual-line detail.
- **Pan** — drag left/right (without shift). The visible span is preserved and snaps at the file boundaries.
- **Brush select** — shift+drag to draw a rectangle; on release, zooms to that range.
- **Reset zoom** — double-click anywhere on the timeline, or click the labeled **Reset zoom** action.
- `L` means physical source line, and positions remain in file order rather than elapsed-time spacing.
- **Minimap** — click anywhere on the minimap to jump to that position.
- **Hover** the timeline for re-resolved visible-column details (time/line position, line count, and per-filter counts). Density buckets also show their exact occurrence count and boundary lines.
- **Click** the timeline background → jumps to the nearest real log line; clicking a filter marker/bucket jumps to the nearest occurrence in that lane.
  A white marker shows your current position.
- **Axis labels** show source line numbers. Event times are available in log rows and Go to Time, but never change timeline positions.

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
- Haystack runs the **Drain template-mining algorithm** natively in Rust while loading.
- The dockable **Templates** tab starts beside **Pinned** in the main window's fixed lower panel and can be opened in a separate window. Each active dock leaf has exactly one external-window action at its right edge; closing the separate window restores the prior dock layout.
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
Haystack first recognizes the **log format** from a sample, then applies a
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

The primary timeline always uses **physical source lines**, with one-based labels that
match the Log View. Its overview counts record starts per line interval, while filter
lanes count matching physical lines. Timestamps are annotations and query metadata:
equal, missing, or backward-moving times never reorder or stretch the source axis.
For multiline text, only an anchored, recognized header begins a record. Stack traces,
payload JSON, and other continuation lines inherit that record's timestamp even when
their payload contains a date. A recognized header with a missing or malformed time
still begins a record and resets inheritance, so it never borrows the prior event's time.
JSON Lines uses one selected top-level event-time key for the whole document rather
than switching keys or searching nested values.

---

## 3. The MCP server (AI-assistant bridge)

`haystack --mcp` speaks MCP over **stdio** (newline-delimited JSON-RPC 2.0), so an
AI assistant can load logs independently, query them with filters, and pull exact windows.

Configure `haystack --mcp` once. By default the agent works independently: it calls `load_log`
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

Haystack exposes MCP only through stdio. Private authenticated local IPC routes attached calls
to the GUI, so no URL, port, or temporary MCP server belongs in the agent configuration.

### Wire it into an MCP client

```json
{
  "mcpServers": {
    "haystack": {
      "command": "/absolute/path/to/haystack",
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
| `load_log` | `path` plus optional inline `record_profile` or saved `profile_id` → indexes the file, returns `log_id` and stats including selected profile/revision and detection diagnostics |
| `list_logs` | All loaded documents with stats |
| `close_log` | Unload a `log_id` and free memory |
| `find_occurrences` | Paginated `[line_number, epoch_ms\|null]` matches for `keyword`; supports `offset`, `max_results`, `after`/`before`, `with_filtered_log`, and ASCII `case_sensitive` mode |
| `get_analysis` / `add_analysis` | (GUI-attached) Read or add the current Pin-tab analysis cards; `add_analysis({text, lines:[]})` adds a text-only card at the top |
| `summarize_log` | One-call orientation (optional `start`/`end` range): stats, top/error-ish templates, biggest time gaps, densest minute, plus byte-size budget estimates |
| `get_timeline_histogram` | Tiny `{x, counts}` distribution for whole log / keyword / template, optional range. Set `domain:"line", count_unit:"records"` for the GUI-equivalent overview; omitted/`auto` keeps the legacy time axis when timestamps exist and counts physical lines by default. An explicit `domain:"time"` is clock analysis, not source navigation. |
| `get_template_anomalies` | Rare, first-seen-late, and bursty templates ("what's unusual here"), optional range |
| `get_template` | Resolve template ids to `{pattern, count, example_line}`; omit `ids` for all |
| `get_template_samples` | A few concrete example lines for a template |
| `log_sequence` | Dense `[[epoch_ms\|null, line, template_id], …]` over a line/time range; optional `collapse`, returns `truncated` when capped |
| `raw_log` | Raw lines over a line/time range (`start`, `end`), `max_lines` + `truncated` flag; `line_numbers` maps discontiguous time selections back to physical source lines |
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
 | ./haystack --mcp
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
- Use `cargo run --release --example profile_pipeline -- [logfile]` for opt-in
  timings from the actual production loader. It reports index construction,
  format/date discovery, header learning, line decoding, timestamp extraction,
  normalization/masking, Drain mining, and logical retained index bytes.
- Generate reproducible multiline, custom-layout, sparse-header, payload-date,
  and backward-clock workloads with `cargo run --release --example
  gen_record_logs -- --help`. Generation is outside the profiler's timed load.
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
| Shift-click log line | Open surrounding filter occurrences |
| Right-click log line | Open copy, pin, inspect, and trim actions |
| Click lane visibility icon | Toggle filter lane on/off (filters log view) |
| Hover filter name/eye | Tooltip with full filter text + match count |
| Click Remove on a filter lane | Remove that filter (with confirmation when enabled) |
| Timeline **Clear filters** | Remove every filter (with confirmation when enabled) |
| Timeline **Enable all / Disable all** | Toggle every filter lane at once |
| Pinned **Edit** button | Reopen the pin window to edit the pin |
| Log View **Text** menu | Decrease, increase, or reset this view's log text size |
| Templates tab → right-click row | Navigate, copy, sample, or filter that template |
| Pinned header expand/collapse button | Expand / collapse pinned lines and analyses |
| Tab-header External Window button | Open that Log, Pinned, or Templates view in a separate window; close it to restore the dock layout. Detached windows do not show another pop-out button. |

---

## 6. Distribution

Each release ships **native installers** (see `docs/release.md` for how they're
built with cargo-packager):

| Platform | Download | Contents |
|---|---|---|
| macOS (Intel + Apple Silicon) | `haystack-<tag>-<target>.dmg` | Full GUI + MCP (`haystack.app`) |
| Ubuntu / Linux x86_64 | `haystack-<tag>-<target>.deb` or `.AppImage` | Full GUI + MCP |
| Windows x86_64 | `haystack-<tag>-<target>.exe` (NSIS) | Full GUI + MCP (`haystack.exe`) |

Build from source for any platform: `cargo build --release`.
