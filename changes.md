# 2026-09-22
- Fixed spacing mismatch in timeline panel_height calculation: reduced allocated gap after histogram from 10.0px to 4.0px to match actual rendering. This eliminates the 6.0px of unaccounted whitespace that was creating a visible gap between filter lanes and axis labels.
- Eliminated vertical spacing between timeline header and filter lanes by setting item_spacing.y = 0.0 immediately after the header closes, before any subsequent layout operations. This removes implicit egui spacing that was visibly separating the filter input from the lane controls below.
- Reclaimed vertical space across all major views by reducing header-to-content gaps from 8.0px to 0.0px: Pin Viewer, Template View, and App Menu now match Timeline and Log View. This minimal 0px spacing maximizes viewport real estate while keeping UI structure clear through visual separation (frames, separators).
- Improved Timeline axis labels with better text contrast (switched to primary text color), increased font sizes by 1px for better readability, and removed the quarter-label skip logic to show duration labels for all adjacent tick pairs. Fixed header alignment by reducing button padding to 6.0×3.0 (from global 8.0×5.0) to match input field and label heights, and reduced HEADER_HEIGHT to 22.0.
- Persisted Timeline display mode (Line/Time/Real Time) per-file in sidecars and as a global last-used default in settings, so reopening a file restores its selected domain and new files default to the most-recently-used mode. Elevated the three mode-selection buttons with a raised frame to visually separate them from other timeline controls.
- Persisted stable detached-window IDs, free-form dock trees, and native position/size in schema-4 investigation sidecars, with backward-compatible restoration of older schema-4 detached-tab data.
- Made release outside every legal dock target create a native detached window at the pointer, with an inexpensive window-outline/title-strip preview during the drag and backend-default placement where screen coordinates are unavailable.
- Replaced delayed dock-library tab dragging with a 4-point app-level gesture, a persistent grabbing cursor and tab ghost, exact tab insertion markers, and live center/edge previews for free-form detached joins and splits.
- Gave detached docks stable window identities and unified Log, Pinned, and Templates rendering there, so utility tabs can join free-form detached layouts and a window stays open when its original tab moves away.
- Made Log-tab drag-and-drop track the captured pointer in monitor coordinates, so legal drop zones respond across main and detached native windows and mouse release no longer has to be delivered to the destination window.

# 2026-09-21
- Updated the record-log generator with deterministic 10-minute-to-7-day timestamp gaps, capped at 20 gaps per 1,000 records by default, and fixed date formatting across multi-day gaps for Real Time timeline testing.
- Added selectable Line, Time, and Real Time Timeline domains. Time keeps source-line spacing with log-time labels and human-readable timestamp deltas; Real Time proportionally spans the oldest-to-latest log timestamps, exposes inactive gaps in the histogram/minimap, and maps gap clicks to the nearest timestamped source line without changing Log View order.

# 2026-09-20
- Increased the application version to 0.1.5.
- Restricted the main dock to upper Log Views and a shared lower Pinned/Templates panel, while keeping detached Log View windows free-form. Individual Log Views now drag between the main panel and detached Log View windows, with an empty source window closing automatically.
- Removed the Appearance → Timeline settings and persisted minimap customization; the Timeline overview minimap is always visible with its default theme-aware colors.
- Removed the Timeline zoom/pan hint controls and their Appearance setting so the histogram and lanes use the full available width.
- Corrected Timeline lane controls: Everything Else now has a neutral swatch, eye/delete icons use theme foreground colours, zoom/pan visibility no longer hides filter bulk actions, and Overview Minimap settings now clearly apply to the full-file density minimap.
- Removed the redundant Timeline icon and title from the timeline controls row.
- Restored the Log View text-size label, A−/A+ controls, pixel-size readout, and reset action inline in the viewer header instead of hiding them under the Text popup.
- Added Focus Log View, a reversible reading mode from More or the command palette that gives the focused docked Log View the workspace and restores the timeline/dock layout with Exit focus or Escape.
- Removed detached Log View's duplicate header. Return-to-main now lives in the Log tab beside the existing view controls, while the normal tab-close action remains the permanent-remove path.
- Compacted dock and Log View chrome to reclaim vertical reading space. The Log View now puts per-view text-size controls in a single Text menu with aligned toolbar metrics.
- Fixed icon-and-label controls so text does not steal row clicks or show a selectable-text cursor, including Intro recent files, Settings navigation, tabs, and the Recent popup; the Intro Open file action now uses the standard app theme.
- Updated the in-app SVG brand mark to match the refreshed full-colour application icon, including distinct line colours and the two corner markers.
- Refreshed the canonical application icon with distinct blue, coral, lavender, and mint log lines, removed the vertical yellow line, and added small yellow and red corner markers; regenerated all runtime and packaged icon assets.
- Fixed files opened from Recent or the Log View after startup: the one-shot startup active-file preference is now cleared after restoration, allowing newly loaded files to become active instead of leaving an empty tab area.
- Refined Settings into responsive grouped sections with full-width navigation, immediate per-section resets, readable values, and a compact highlighted log preview; migrated the legacy dark-mode boolean to Light/Dark/System OS-tracked themes. Detached Log View windows now use filename-aware titles and compact chrome for focused-pane scope, shared document/filter state, Return to main window, and explicit permanent removal while preserving native-close and final-view behavior.
- Simplified the Timeline presentation without changing source-order coordinates or occurrence navigation: lane labels are left-aligned with quiet swatches, baselines and viewport overlays are subdued, zoom retains a labeled Reset zoom action, and occurrence navigation is named explicitly. Renamed the minimap preference to Overview minimap, exposed zoom/pan visibility separately, and made untouched minimap defaults theme-aware while preserving saved overrides.
- Refined the main shell and Log View hierarchy: file tabs now use a restrained active indicator, Find/Filter controls expose explicit modes and Enter hints, long-line/font/export controls sit beside the viewer, and selected rows/inspectable data use quieter persistent cues. Existing async search, inspectors, filters, pin actions, and per-view state are unchanged.
- Fixed the empty workspace intro so its logo, guidance, Open file action, drop target, and five recent-file entries share a responsive centered block; cleared stale document status when no file remains open.
- Kept filter-match text readable by separating categorical marker colours from text colour, preserved selected-row fills beneath overlapping highlights, and changed timeline lane labels to neutral text with explicit colour swatches.
- Unified the light and dark visual foundations around semantic canvas, surface, text, interaction, severity, search, filter, and embedded-data tokens. Standard egui controls, dock tabs, and custom log/timeline surfaces now share the same palette, including distinct hover, focus, selected, and inactive-window states.
- Switched the embedded UI face from Inter ExtraLight to Inter Regular, fixed the first frame after a file load so the new log dock renders immediately, and centered the empty-state action buttons.

# 2026-09-19
- Added Shift-click filter-occurrence context in Log View, centering the chosen row between each filter's nearest surrounding occurrences with retained highlights and embedded-data cues.
- Replaced the Haystack app icon with the approved transparent midnight-dark magnifying glass, pale subtle lens, and prominent orange lowercase `h` across runtime and packaged icon assets.
- Refined the Haystack app icon into a minimal magnifying-glass mark by removing the haystack, enlarging the lens, and using a large warm-gold H on the navy background.
- Rebranded the application as Haystack across the crate, binary, GUI/MCP identifiers, installer metadata, documentation, and assets; replaced the old medical-procedure messaging with needle-in-a-haystack positioning and a new haystack magnifying-glass app icon.
- Switched the embedded non-log UI face from Inter Thin to Inter ExtraLight (weight 200); Space Mono remains unchanged for Log View and pin content.
- Set primary non-log UI text to pure white in dark mode and pure black in light mode, including egui-inherited labels and controls; muted, accent, warning, and log colours remain distinct.
- Embedded Inter Thin for all non-log UI labels, controls, and text while preserving Space Mono for Log View and pin content. Added Inter's original SIL OFL 1.1 license and README attribution.
- Improved Log View search navigation and Settings clarity: the clear-search action now appears only when search content exists, Enter advances active results in the selected Log View, and Appearance uses larger icon-free group headings with the minimap renamed to Zoomed panel.
- Fixed the Settings body sizing so the right-side control panel fills the popup and scrolls through all grouped controls instead of being clipped to the navigation height.
- Flattened Settings navigation to top-level Appearance, Log parsing, AI Assistant, and Support; Appearance now shows View, Timeline, Zoom Panel, Minimap, and Filters as grouped controls in the right panel, with the Close action in the modal title row.
- Fixed the Settings window hierarchy and layout: removed the duplicate header, stacked all detail controls vertically, grouped View/Timeline/Zoom Panel/Minimap/Filters under Appearance, and added an explicit Close button.
- Fixed Settings navigation inheriting the modal's horizontal layout, which caused section labels and controls to overlap.
- Replaced the anchored Settings dropdown with a centered two-column settings window, separating Timeline, Zoom Panel, and Minimap controls. Added a persisted Zoom Panel visibility preference while retaining minimap show/hide, colour, and opacity controls.
- Moved the Log View add action to the dock panel's standalone add button and recentered the shared timeline when switching to a view whose selected line is outside the current timeline scope.
- Detached Log View windows now render their own dock panel, starting with one full-panel view and retaining added tabs and layout changes until re-docking.

# Investigate one file from multiple Log Views

Each open file can now have several independent Log Views in dock tabs or native windows. Use the plus action beside a Log tab to branch from its current location; selections, scroll positions, Find sessions, row ranges, inspectors, and embedded-data work stay independent while filters, timeline, pins, templates, trim, parsing, live tailing, and MCP document changes remain shared. Timeline and navigation actions follow the focused Log View. Views keep stable numbered titles, can be re-docked or permanently closed, and the final Log View is protected. Schema-4 sidecars restore the complete keyed layout; earlier unreleased development schemas are rejected safely without migration.

# Keep the Settings dropdown compact and scrollable

Opening application Settings now caps the dropdown at a compact height instead of expanding down the full window. Shorter viewports reduce the cap further, and overflowing settings remain accessible through the vertical scrollbar.

# Put pop-out controls beside workspace tab titles

Log, Pinned, and Templates now each show an External Window control immediately after the tab title instead of at the far edge of the dock leaf. The control exists only in the main dock tab header, so detached windows do not offer a redundant second pop-out action.

# Keep `{log}` out of Field criteria

Field-mode Search and Timeline Filter suggestions now offer only captured header fields. The `{log}` message capture is also excluded from row-context Field actions; users search message bodies with the existing Text or Regex modes instead.

# Make the Timeline minimap configurable

Settings now groups view and filter controls by feature and adds Timeline minimap visibility, colour, and opacity preferences under View → Timeline → Minimap. Existing installations keep the minimap shown with its current opaque bar colour, while hiding it reclaims the unused Timeline height.

# Align record-format tests with the current schema-3 grammar

Record matcher, preview, document-capture, and GUI reparse tests now use required `{time}` templates and exact capture names while preserving their original typing, bounding, ownership, and anchor-restoration coverage.

# Apply the overlay boundary to analysis and suggestion surfaces

Movable analysis bubbles and interactive annotation callouts now receive a transparent modal shield, while recent-search, recent-filter, and focused Field suggestions participate in base-input suppression. Documentation now defines the common modal and popover interaction contract.

# Stop dropdown dismissal clicks from reaching the workspace

Top-bar Format, Recent, Saved Filters, More, Settings, and AI Assistant popovers now sit over a transparent click-and-drag shield. Outside clicks still dismiss immediately, but can no longer select log rows or trigger controls behind the popover.

# Prevent inspectors and pin editors from leaking input

Pin editing, full-line viewing, and structured-data inspection now use the shared modal host, keeping text selection, copying, keyboard navigation, and backdrop clicks isolated from the underlying log.

# Center application dialogs behind one input-blocking modal host

Custom-date and integration editors, command/navigation sheets, filter confirmations, recovery/error dialogs, and ZIP progress now share the centered modal backdrop, so application controls cannot react through them.

# Isolate modal editor input from the log workspace

The log-format editor is now a centered modal, while a shared overlay input owner prevents app shortcuts, Log View selection, and template navigation from consuming events intended for interactive foreground surfaces. Template Left/Right movement is native again instead of relying on manual caret mutation.

# Support ignored log-header fields

New log formats support repeatable typed `{ignore}` declarations such as `{ignore:number}` and `{ignore:path}`. They participate in header matching but are excluded from stored record captures, Field search/filter suggestions, and captured-field inspection; raw text search still sees the original source.
Template field names now default to token matching, so custom-field setup is no longer required before using a name. The unreleased log-format editor uses one grammar rather than profile-version compatibility behavior.

# Guide log-format customization with examples and diagnostics

The format editor now includes Basic, Named fields, Ignore header values, and Quoted path examples. Preview summaries distinguish records and continuations, while Diagnostics reports successful ignored-header matches without exposing their values as fields.

# Keep template preview work out of the editor frame

Log-format preview compilation and custom date recognizer preparation now run in the background with revision-tagged results. Renaming a format no longer reparses its sample, and the editor keeps the last preview visible as `Updating…` until the latest draft result is ready.

# Make template editing behave like a code editor

Template completion now replaces the complete field or type around the caret, keeps Left/Right navigation native, and uses an overlay so suggestions no longer push the editor layout. Up/Down/Enter/Tab control visible choices, Ctrl/Cmd+Space reopens them, and template literals, names, types, and invalid declarations receive readable syntax coloring.

# Compact log-format authoring groups

The Log format editor now keeps Name, collapsed rules, Template and its validation together, then Sample log and Parsing preview, with Advanced options and Diagnostics collapsed below. The Format dropdown uses the same compact text scale and keeps Edit/New beside the active format, so the first customization flow no longer hides its name or preview behind a large rules block.

# Search and filter captured log-format fields

Search and Timeline Filter now have an explicit Field mode for expressions such as `b >= 9` and `loglevel = "FAULT"`. Matching uses retained record captures, includes continuation rows, offers bounded suggestions from real values, highlights captured spans, and persists typed queries in sidecars, saved filters, and separate recent-search history. Incompatible queries are disabled for review after a format change instead of becoming text queries.

# Retain typed format captures by record

Explicit log formats now keep compact field spans tied to record headers. Captures resolve from continuation rows, and log fields expose the first-line remainder plus continuation segments. A partial live-tail line replays its captures on append instead of leaving stale values.

# Switch log formats from the top bar

A visible Format menu now shows the selected file's applied name and syntax, saved formats, and Auto-detect. Selecting a format reparses the file in the background, and a profile matching zero headers leaves the current view intact.

# Make the log-format editor readable and inline-first

New formats start with self-contained syntax and a sample; the editor now shows full-size rules, syntax help, live field/type/value preview, and cursor-aware placeholder completion. Saving and applying have separate labels, while legacy controls remain available under Advanced options.

# Add schema-2 inline log format parsing

New record profiles can declare arbitrary fields inline with token, number, path, or text types; common aliases resolve to the same semantic fields. Matching is bounded, human-authored iOS/path/URL fixtures are covered, and schema-1 saved formats retain their existing behavior.

# Simplify the log format editor

The Log format window now leads with a real sample line and its template, while regex, timestamp, prefix, and custom-field controls are tucked into Advanced options. The starter iOS layout supports the compact `{a}` and `{b}` token placeholders used in the sample.

# Verify record parsing and document the remaining release gates

Added an explicit-layout production benchmark and recorded deterministic fixture fingerprints, five-run release measurements, full/debug/MCP-only tests, and Linux/Windows cross-target compile checks. Added a regression for appending a first record to an empty file and updated the guide's line-axis and MCP histogram descriptions. The report calls out noisy load measurements and unverified UI/performance gates; no push, tag, or release was made.

# Replay amended live-tail lines exactly

Live tailing now reverses and reanalyzes a final unterminated physical line when more bytes arrive, including its record boundary, time/provenance, diagnostics, and Drain template changes. Incremental filter scans replace hits on that changed row. Yearless BSD syslog, logcat, and glog dates use one visible, per-document reference year, with an explicit profile override that remains stable across appends.

# Apply custom record profiles without losing the investigation

Added a log-format template editor with typed fields, timestamp choice, a bounded background multiline preview, reusable versioned presets, and explicit per-file apply. The selected profile is embedded in the investigation sidecar and available to headless `load_log`; reparsing keeps the old view until success, restores line-based state, and disables stale Template-ID filters for review.

# Keep the primary timeline in source order

The GUI timeline now always uses physical source-line positions, so equal, missing, reversed, and widely separated timestamps cannot reorder or stretch the file. Overview buckets count record starts through the document rank index, filter lanes retain physical-line counts, and legacy saved time zooms migrate through exact time selection to a source-line envelope. MCP histograms add explicit line/time domains and record/physical-line units while retaining their omitted-argument compatibility behavior.

# Make time queries independent of timeline order

Added a provenance-aware time-query service for exact inclusive selections, closest valid-header navigation, and signed clock deltas. GUI Go to Time no longer depends on timeline coordinates, and MCP range tools now evaluate event-time predicates by source scan, preserve discontiguous matches in physical order, exclude unknown-time records, return empty out-of-range selections, and map raw rows back through explicit line numbers.

# Resolve sparse record headers with bounded evidence

Automatic parser discovery now samples distributed contiguous regions under independent 8,192-line and 2 MiB budgets, selects structured formats from anchored header evidence, and resolves timestamp families within the chosen header shape. Two consistent headers can identify long multiline records, explicit profile/date choices bypass popularity thresholds, ambiguous ties remain unresolved, and each document exposes selection confidence, support counts, conflicts, and truncation limits.

# Parse multiline records from anchored headers

Record starts now come from validated, format-aware headers instead of any date found in a physical line. Continuations inherit only their owning record's time; missing or malformed header times reset inheritance and retain typed provenance. Document indexes now include compact time state, rank-assisted record lookup, exact overflow spans, and one locked top-level JSON time field, while RFC 5424 missing timestamps remain real boundaries.

# Compile versioned log-format profiles safely

Added the core schema and bounded anchored compiler for placeholder-based record profiles, including exact byte spans, typed/custom fields, configurable levels, literal-brace escaping, fixed indentation, terminal prefixes, Windows paths, and advanced anchored regex rules. The compiler is available for preview and later parsing phases but is not yet wired into document loading.

# Establish record-parsing correctness and performance baselines

Added a human-authored multiline-record oracle, a deterministic configurable record-log generator, and opt-in stage timings from the production loader. Benchmarks now report record markers and logical retained index bytes so later parser/timeline phases can compare equivalent workloads without treating current false-positive behavior as the oracle.

# Fix Windows analysis-popup tests

Analysis-popup test fixtures now use an atomic numeric suffix rather than Rust test thread names, whose `::` separators are invalid in Windows filenames. A regression check keeps those fixture paths Windows-safe.

# Prepare v0.1.4 release

Set the application and native installer metadata to `0.1.4` so the release workflow can validate and package the consolidated feature set under the `v0.1.4` tag.

# Simplify closing and the empty-state entry point

Copy actions now dismiss the multi-line selection bubble, tab closes save investigation sidecars without a confirmation prompt, and the default opening page uses centered Open/Recent actions with shorter drag-and-drop copy and no ZIP explanation line.

# Fine-tune Timeline and Log View interactions

Removed the Timeline gesture hint to reclaim vertical space and moved the minimap up with it, removed embedded-data badge overlays while retaining source underlines and annotations, and made Enter advance an already-started Log View search even while the search field remains focused.

# Refresh the complete desktop UX

Standardized the app on recognizable theme-colored SVG actions, rebuilt the responsive `Haystack` shell and AI Assistant controls, clarified tabs/empty states, compacted and aligned workspace controls around the data, and reduced every dock leaf to one External Window action. Settings, dialogs, hints, deletion wording, transient-surface dismissal, and both themes now follow consistent grouping, copy, contrast, and interaction rules without changing analysis or persisted schemas.

# Keep selection visible during analysis entry

Drag-selection action bubbles now open near the release cursor, then move the analysis editor beside the selected lines while preserving the selected highlight for the editor lifetime.

# Fix analysis bubble positional awareness

Drag-selected analysis bubbles now open at the release endpoint, and their arrows point to that visible endpoint instead of a hidden position inside the bubble. The selected range remains active until the user takes an action or cancels.

# Replace Log View pin surfaces with anchored analysis bubbles

Right-click Pin and drag-selected row actions now use movable, resizable bubbles anchored to the source text. The selection remains visible until save/cancel, and the editor accepts optional analysis with `Add analysis/info or enter to save` guidance.

# Tune annotation popup transparency and dismissal

Made the annotation callout transparency adjustable through one top-level constant and added immediate Escape/outside-click dismissal, establishing the same interaction rule for future transient popups.

# Improve embedded annotation hover popups

Moved annotation callouts into a focused module with one key/value per line, viewport-aware dynamic sizing, bounded/truncated previews, and a sticky handoff lifecycle that remains usable across dense highlights.

# Fix Rust formatting

Applied the repository’s current rustfmt output so Cargo formatting checks pass consistently.

# Improve MCP integration prompts and guide readability

Shortened the agent-setup and GUI-session prompts while preserving the required stdio transport, attachment, security, and investigation guidance. The integration window now groups recommended setup, manual client configuration, and usage into consistent, easier-to-scan sections.

# Add ZIP drag-and-drop support

Drop or open a `.zip` to extract it in the background into a uniquely named folder beside the archive, then choose logs from that folder in the file picker. Updated drop hints, progress/cancel controls, and extraction error handling make archives discoverable and preserve existing files.

# Fix annotation hover callout handoff and field layout

Structured annotation callouts now stay open along the direct path from the highlighted source to the popup, including near viewport edges, and constrain their size to the screen. Structured values render their detected fields on separate lines with scrolling for large payloads.

# Fix: preserve selected lines through every filter update

Filter additions, removals, lane visibility changes, undo, and clear operations now retain a still-visible selection, otherwise choose the nearest visible line before revealing it. Centralized filter-lane mutations and regression coverage protect this selection-and-viewport ordering.

# Fix: keep selection stable across filter lane changes

Filter lane changes now keep the selected log line and move the viewport only when needed; if that line is filtered out, the nearest remaining line is selected before the viewport is adjusted.

# Fix: preserve disabled timeline lane navigation

Disabled filter lanes now retain a muted baseline line without occurrence markers or buckets, and clicking one navigates to the nearest log line without selecting the disabled filter.

# Fix: preserve Log View position while changing selection

Log View navigation now leaves the viewport unchanged when the selected line is already fully visible, minimally reveals nearby targets within five rows with a two-row safety margin, and centers distant targets. Manual scrolling also moves an off-screen selection to the nearest visible row across Truncate, Horizontal Scroll, and Wrap modes.

# Performance: make live tailing incremental across tabs

Every open tab now checks for appended data. Staged documents share completed 65,536-entry per-line index chunks, existing filters scan only appended lines, and chronological/sequence timelines extend from their prior density summary; results still install atomically while the visible document remains responsive.

# Performance: scan only changed filter lanes

Timeline filter edits now retain shared match vectors for unchanged matchers and scan only newly added or materially edited lanes. Include/exclude and removal changes reuse existing hits plus the document-wide timeline density, while document trims, replacements, and range-scope changes invalidate reuse safely.

# Performance: compact timeline match indexes

Timeline filter lanes and the out-of-order timestamp fallback now retain only x-sorted 32-bit line IDs. Rendering and navigation resolve timestamps through the document index, eliminating the padded `(u32, i64)` copy previously held for every timeline hit.

# Add log sharing and export actions

# Make long lines and recent queries first-class

Log View can now truncate, wrap, or horizontally scroll complete long lines; truncated rows flag hidden search hits and open a full-line inspector with an untruncated copy action. Wrap mode uses cached variable-height virtualization so scrolling, selection, Timeline viewport tracking, and keyboard navigation remain responsive. Export scopes are consolidated in a right-aligned menu, while empty focused Find and Timeline filter fields offer immediately executable recent entries (excluding filters already added).

Log rows and selected ranges can now be copied in the common formats, while visible rows and timeline ranges export to text. Template counts export as CSV/JSON and the Pin panel saves pins plus analysis notes as Markdown for incident sharing.

# Preserve investigation work while narrowing logs

Trimming now retains pins and active investigation state, with out-of-range pin anchors hidden until they return to view. Trim and pin clearing support one-level undo, and closing a tab with investigation work now asks for confirmation after saving its sidecar.

# Refine top-panel dropdown navigation

Reordered the top-panel actions to Open File, Recent, Saved Filter, and Views; grouped Commands and Templates under the Views dropdown and made settings actions dismiss the settings menu after use.

# Remove unsupported binary file associations

Release installers no longer associate `.evt`, `.evtx`, or `.sys` files because haystack accepts text input rather than Windows Event Log binaries. Updated release validation and packaging documentation to match.

# Persist per-file investigation state

Persist per-file investigation state in versioned adjacent sidecars, including filters, anchors, notes, layout, and scroll position; restore changed-file notes safely, autosaving silently on changes, at least once per minute, on tab close, and on application exit.

# Fix intermittent GUI MCP bridge requests

Explicitly restore blocking mode on accepted GUI IPC sockets before reading request envelopes. On macOS, inherited nonblocking mode could return `EAGAIN` before the request arrived, causing the bridge test and real GUI requests to be rejected intermittently.

# Detect XML, OpenStep, and binary property lists

Log View now parses XML plists and explicitly labeled OpenStep/ASCII property lists into bounded trees while preserving exact multiline source spans. Ordinary Foundation descriptions keep their Apple classification. Binary plist magic is recognized in literal, hex, and Base64 transports as a separate safe summary; its transport is decoded only after the explicit inspector action, without automatic binary object-table interpretation.

# Expand Swift and Foundation value detection

The Apple embedded-data profile now parses synthesized Swift structs/classes, nested `Optional(...)` values, enum associated values, labeled tuples, Swift dictionaries/arrays, multiline `dump`/Mirror output, classic Foundation collections, and `<NSObject: address; property = value>` descriptions. Stricter confidence rejects ordinary parenthetical prose and bracketed log tags, node limits remain enforced, and overlap arbitration preserves Swift containers with JSON-compatible children without letting loose field or protobuf-like wrappers hide strict nested values.

# Annotate exact log timestamps

Explicit source timestamps now receive a quiet format-specific underline and the same delayed source callout behavior without noisy per-line gutter badges. The load pass retains compact exact timestamp spans for allocation-free rendering; hover can copy the raw source or normalized UTC value, continuation lines remain unmarked, and JSON field spans resolve the actual timestamp key even when another field repeats the same value.

# Add source-aware structured-data interactions

Structured detections now retain exact source fragments, underline only real fields, and render every opening cue on a row. A stable 800 ms hover over either underlined source or its cue opens a semi-transparent callout with Copy source and Open inspector actions; movement, scrolling, selection, and a short source-to-callout grace period keep it unobtrusive.

# Detect single key/value log events

Log View embedded-data analysis now recognizes single `key=value` and conservative `key: value` events, spaces around separators, double/single/backtick quoted text, multiline values, and intact URL/path values. Shared bounded parsing adds the requested coverage without turning timestamps, URL schemes, source locations, or ordinary colon prose into fields.

# Fix v0.1.1 packager metadata

Aligned the cargo-packager installer version with the `0.1.1` crate and release tag so the GitHub release workflow validates successfully.

# Make GUI-agent investigation guidance flexible

Reworked the GUI MCP instruction from a rigid sequence into a concise suggested approach: use the GUI-bound document, understand the user's findings, explore log shape, narrow hypotheses only when useful, and publish evidence-backed conclusions.

# Focus GUI-agent analysis workflow and shorten session IDs

GUI-attached agents now immediately read the user's Pin-tab findings, focus investigation with filters and document trimming, and publish an evidenced root-cause analysis back to the Pin tab. Temporary GUI session IDs are now random 12-character hexadecimal values, with matching validation, MCP schemas, documentation, and tests.

# Resolve timeline buckets at every zoom level

Timeline zoom and pan now re-resolve the visible histogram and filter lanes from exact timestamp/line indexes instead of magnifying fixed whole-file buckets. Singleton occurrences render at their exact time/line as 2×7px markers. Multi-occurrence buckets join edge-to-edge across their full timeline width at 7px (small), 10px (medium), or 13px (dense) height. The current occurrence is derived from the selected lane and Log View line instead of stored as duplicate diamond state; corrected pan, brush, minimap, and log-viewport synchronization keep navigation stable.

# Add case-sensitive Log View search

Added an `Aa` toggle to Log View search for exact-case matching, while preserving case-insensitive search as the default. The search worker and highlight automaton now use the selected mode consistently.

# Refresh filter controls and lane layout

Updated the timeline bulk visibility controls to use clearer Disable/Enable labels with visibility icons, replaced Add/ Add Filter keyboard glyphs with a reusable enter SVG icon, and widened the timeline filter label/action column by 20% for better spacing.

# Refine timeline filter alignment

Removed the gap between the timeline header and filter lanes, aligned the Everything Else label with its visibility control, changed Clear All Filters to Delete All with a delete icon, and widened the filter label/action column by another 20%.

# Retry Windows GUI instance lock contention

Treat Windows lock violation errors from `LockFileEx` as retryable contention so a second GUI launch forwards its paths instead of failing immediately.

# Remove unused GUI constructor warning

Limited the default `LogTab::new` convenience constructor to test builds, where it is used extensively. Production GUI builds use the settings-aware constructor and now compile without the dead-code warning.

# Agent-assisted MCP setup and safer GUI handoff

Added a copyable setup prompt at the top of the AI Integration guide so Codex, Claude, or Cline can translate Haystack's dynamic executable path into its native global/user MCP configuration, while preserving existing servers. Manual client-specific setup remains directly below as a fallback. The live GUI instruction now stops when Haystack MCP is unavailable and directs the user to **Settings → Integrate with AI Assistant** before sharing the temporary session ID.

# Unified MCP verification

Verified the stdio-only MCP architecture with the 349-test full suite, MCP-focused GUI attachment tests, full-feature Clippy, GUI and MCP-only builds, live initialize/tool discovery/session-info requests, generic invalid-session handling, and explicit rejection of removed HTTP CLI flags.

# Unified MCP integration documentation

Rewrote the MCP guide and aligned the in-app guide, README, user guide, feature inventory, and contributor references around one permanent `haystack --mcp` stdio configuration for Codex, Claude, and Cline. Documented standalone versus `gui_attached` behavior, session-ID handling, resources, compatibility, security, and troubleshooting without any HTTP setup path.

Removed HTTP-era `--port` and `--status-file` behavior now fails explicitly instead of being silently ignored.

# Private IPC replaces MCP HTTP

Removed the HTTP MCP transport, CLI port mode, bearer headers, and GUI HTTP endpoint. Agents now connect only through `haystack --mcp` stdio; a temporary 256-bit GUI session ID authenticates requests forwarded over a private loopback JSON IPC socket.

# Attachment-aware MCP resources and guidance

Updated `session_info`, `haystack://session`, `haystack://guide`, initialization guidance, and lifecycle errors for the unified `standalone` / `gui_attached` model. Expired GUI attachments now clear automatically and return a retryable structured error without exposing the session ID.

# Explicit GUI attachment through standard MCP stdio

Added `attach_gui_session` and `detach_gui_session` to the normal `haystack --mcp` server. A user-supplied 256-bit temporary session ID is validated against the private GUI manifest and a live authenticated ping before calls are routed to the selected GUI log; detach and expiry restore the same preserved standalone state without leaking credentials.

# Reusable private GUI MCP client

Extracted the authenticated loopback forwarding logic into a reusable internal GUI client. The legacy `--mcp-gui` bridge keeps its behavior while the normal stdio server can reuse the same validated request path for explicit temporary GUI attachments.

# Unified MCP session contract

Made the public MCP tool catalog and initialization guidance stable across standalone and GUI-attached operation. Analysis schemas now describe `log_id` as mode-conditional and advertise the complete lifecycle/analysis surface so clients can safely cache discovery results before a temporary GUI attachment.

# MCP integration verification

Verified the completed MCP integration with formatting, full-feature Clippy, the 351-test repository suite, an MCP-only build, legacy and modern stdio protocol smoke tests, resource discovery, and the unavailable-GUI bridge path. Updated the documented test inventory and removed a new Clippy warning from the session credential accessor.

# MCP integration documentation

Rewrote the MCP guide and aligned the README, user guide, feature inventory, and contributor references around the live-GUI bridge, temporary bearer-authenticated HTTP, standalone headless mode, session resources, compatibility contract, client-specific setup, security boundaries, and troubleshooting.

# MCP client-specific integration setup

Redesigned the GUI integration guide around a stable `--mcp-gui` bridge for Codex, Claude, and Cline across their CLI, desktop/app, and VS Code surfaces. Temporary direct-HTTP snippets now use the canonical `/mcp` endpoint with a separate bearer header, explicitly forbid global persistence, and are no longer copied automatically when the server starts.

# MCP self-describing sessions and structured results

Added a `session_info` tool and MCP resources that describe the current GUI/headless mode, lifecycle, active log, filters, and recommended investigation workflow. Tool responses now include both text fallback content and `structuredContent`, and tool declarations expose output schemas and behavior annotations for more reliable agent planning.

# Stable MCP bridge for live GUI sessions

Added `haystack --mcp-gui`, a stable stdio bridge that agents can configure once while Haystack keeps its random-port GUI HTTP session temporary. GUI sessions now publish an atomic user-private manifest with a 256-bit credential, remove it on stop/exit, avoid logging secrets, and keep the GUI tool surface stable even while no document is attached.

# MCP dual-era transport foundation

Added modern MCP `2026-07-28` discovery and request validation while preserving initialization-based clients through `2025-11-25` and the existing older revisions. Streamable HTTP now has a canonical `/mcp` endpoint, validates loopback browser origins and modern protocol headers, separates `/health`, supports bearer authentication, and keeps the token-prefixed routes temporarily for backward compatibility.

# UI: make the data inspector remember the user's view and resize naturally

The embedded-data inspector now opens in Pretty by default, remembers Tree/Pretty/Raw choices across payloads and new sessions, and sizes itself to the content while remaining user-resizable.

# Native installer file associations

Registered `.log`, `.txt`, `.out`, `.err`, `.evt`, `.evtx`, `.sys`, `.csv`, `.json`, `.xml`, and `.md` with the GUI through cargo-packager metadata. Windows NSIS and macOS bundles register the extensions directly; Linux desktop entries advertise their corresponding MIME types. Added a release-workflow validation step to keep the association list complete.

# Feature: open files from the shell with one reusable GUI instance

GUI launches now accept file paths from command-line, Finder, Explorer, and Linux desktop-file associations. The first GUI process owns a cross-platform local endpoint; later launches forward their paths to it, focus the existing window, and open each file as a tab.

Refactored the UI into smaller navigation-focused modules without changing behavior: LogTab state operations, log highlighting, saved-filter popup rendering, and template browsing now live beside their parent views.

Tightened the JSON inspector’s horizontal offset toward the line-number gutter while keeping its vertical placement unchanged, and lowered gutter text by one pixel for better alignment with the source rows.

Updated the Log View gutter with a wider translucent light/dark theme-aware treatment, and replaced the embedded JSON context window with a foreground overlay anchored beside the line-number gutter. The overlay keeps Tree/Pretty/Raw views, copy actions, and Escape/outside-click dismissal while choosing a visible side of the selected row.

# UI: scale JSON cues and dismiss the inspector naturally

The floating JSON cue now measures and scales with the active log font and uses a more translucent treatment between rows. The inspector is fixed-size and non-movable, and closes on Escape or any click outside it, including clicks on other controls.

# Fix: keep embedded JSON cues out of the log layout

Replaced the fixed-width JSON gutter and continuation rail with a tiny floating cue anchored above the exact opening delimiter. Detected data remains underlined and fully interactive without shifting every log line to the right.

# Feature: detect and inspect embedded JSON in visible logs

The virtualized Log View now detects bounded single-line and multiline JSON on cancellable background workers, marks exact source spans with JSON cues, and opens Tree/Pretty/Raw inspection with exact copy actions. A compact timestamp-record boundary index lets direct viewport jumps recover payloads that begin far above the visible lines while filtered views continue to analyze only contiguous physical source.

# Fix: stabilize saved-filter popup widget IDs

Saved-filter rows and per-document dock areas now use stable IDs, preventing egui from warning that popup or dock widgets changed identity between layout passes during filter application and tab changes.

# UI: synchronize timeline and Log View selection navigation

Timeline lane clicks now move directly to arbitrary lines while preserving the clicked lane, Log View Up/Down navigates search occurrences or visible lines as appropriate, and selected lanes remain stable when the log selection changes. Timeline occurrence status is emphasized and shown only for matching selected lines. Detachable Log and Pinned views use overlapping-window resize controls.

# UI: add spacing before detachable tab resize icons

Added wider title padding to the Log and Pinned dock tabs so their labels no longer sit beneath the resize icon at compact tab widths.

# Fix: replace all dock close controls for detachable views

Disabled the native egui-dock close buttons for Log and Pinned views and added real resize-icon hit targets in each tab title and dock header, so no misleading close glyph remains.

# UI: identify detachable Log and Pinned views with a resize icon

The Log and Pinned dock-tab title controls now show a valid overlapping-window resize icon instead of a confusing close glyph; detached viewport close behavior is unchanged.

# Fix: preserve the log position after timeline lane toggles

After a timeline visibility change completes, the log view now explicitly scrolls to the nearest remaining line from the prior viewport. This prevents egui's retained scroll state from resetting the view to the start of the log.

# Performance: stage live-tail updates off the UI thread

Appended log data is now indexed and template-mined on a background worker, then atomically swapped into the view. The staged document has independent mutable mining state, preventing live tailing from blocking scrolling or mutating the document currently read by the UI/MCP server.

# Performance: halve GUI filter-match index memory

The GUI now stores filter match line IDs as compact 32-bit values rather than platform-sized integers. This halves the retained match-index memory on 64-bit systems for high-cardinality filters.

# Performance: rebuild filtered log indexes in the background

Changing timeline lane visibility now builds the filtered real-line index on a cancellable worker while the current log view remains responsive. Filter matches are shared between the scanner and this worker instead of being copied for each toggle.

# Fix: place the log scrollbar at the container edge

The log view no longer limits the scroll area to the longest line's width. It now uses the full remaining container width, keeping the vertical scrollbar at the far right without leaving empty space beside it.

# Fix: remove the custom log-view scrollbar

Removed the custom scroll-position indicator from the log view. The view now uses the standard scroll area without an additional scrollbar or reserved indicator space.

# Fix: keep selected timeline diamonds stable while zooming

Timeline diamond selection now stores the matching real line number instead of a zoom-slice-local point offset, so the selected marker remains attached to the same occurrence as the visible range changes.

# Performance: bound high-cardinality masking cache memory

The token masking cache now retains at most 65,536 unique entries. Additional unique tokens are still masked correctly for their current line but no longer grow document memory indefinitely.

# Performance: aggregate timeline painting to screen pixels

The timeline histogram, dense filter lanes, and minimap now aggregate stored buckets into screen columns before painting. This preserves hover/navigation data while reducing per-frame egui paint commands on large logs.

# Performance: keep filter completion off the UI thread

The filter worker now builds the corresponding timeline before returning its result, so installing completed filters no longer performs a full document walk on the next UI frame.

# Performance: reduce large-log UI stalls and document memory

Live-tail polling now checks file metadata before copy-on-write, filtered find scans share their visible-line index, and selected filtered ranges use binary search. Log rows avoid per-byte highlighting allocations, safely cap Unicode long lines, and the document no longer retains unused original-timestamp or timestamp-span arrays.

# Maintenance: restore Rust formatting consistency

Ran the repository-wide Rust formatter to remove formatting drift and keep source files consistent with `cargo fmt --check`.

# Fix: keep timeline lanes visible with top-aligned filter controls

Constrained the timeline header separator and matched the fixed header height to the default controls so the header no longer consumes the entire panel and hides the timeline lanes.

# Fix: restore default theme and alignment for filter controls

Removed the special filter-control colors from the timeline and log-view Add Filter controls, and top-aligned the timeline header filter input and button with the other header controls.

# Fix: keep cached cargo-packager binaries architecture-specific

The release workflow now includes `runner.arch` in the `cargo-packager` cache key. This prevents the Apple Silicon packager executable from being restored on the Intel macOS runner, where it fails with `Bad CPU type in executable`.

# Change: MCP initialization instructions distinguish GUI and headless workflows

Fixed release benchmark aggregation so the generated `benchmark-results.txt` is excluded from its own input glob, preventing the release job from failing when `cat` reads and writes the same file.

Bumped the application and installer metadata to version 0.1.1 for the release tag.

Made release packaging safer and more reproducible by using the native Intel macOS runner, validating release tags against Cargo and packager versions, pinning cargo-packager, and publishing platform-specific first-run instructions.

Captured the benchmark output from every release target and publish it as one combined `benchmark-results.txt` file alongside the installers and checksums.

Added common regional and RFC-2822 log timestamp extraction families, fractional seconds for year-first slash dates, and regression coverage so these logs receive a timeline automatically.

Clarified MCP initialization instructions so agents distinguish headless `load_log` setup from GUI sessions with an already attached log, and follow the recommended full-log analysis workflow.

# Fix: restore text selection in the central log view

Improved MCP agent interoperability with initialization workflow guidance, tool safety/idempotency annotations, structured tool errors, and strict numeric argument validation; corrected filter-case documentation to match the case-sensitive implementation.

Clarified empty-filter responses with actionable next-call suggestions and separated total indexed templates from templates matched by a summary range/filter.

Updated the AI-agent integration guide with current Claude Code, VS Code/Copilot, Cursor, Cline, and Codex formats; safely escapes executable paths and explains standalone stdio versus GUI HTTP connections.

Improved the GUI Start MCP experience with an actionable GUI-mode prompt, clearer security/status messaging, disabled startup without an open log, more tolerant startup timing, and tab-switch notifications.

Re-enabled native egui text selection for log content so users can select and copy text with the mouse. Whole-line drag selection and its selected-line count remain available for pinning.

# Fix: timeline filter controls are grouped beside the Timeline header

Moved the Add Filter section out of the app toolbar and into the Timeline header, beside the Show/Hide and Clear all filter controls. The controls are now left-aligned after the “Timeline” label, with a separator before Add Filter.

# Change: timeline filter controls moved to the header; "Custom date" moved into Settings → Log Parsing

Two UI relocations. (1) `src/ui/timeline/view.rs`: the "Show/Hide all filters" and "Clear all filters" buttons moved from the bottom filter toolbar into the **top "Timeline" header row** (right-aligned, shown only while filters exist, plain native egui buttons). The now-unused bottom bar, its `BOTTOM_BAR_HEIGHT` constant, and that height contribution in `panel_height()` were removed; the unused `icon_text_button_at` helper was dropped from `src/ui/icons.rs`. (2) `src/ui/app/view.rs` + `src/ui/settings/view.rs`: the "Custom date" button was removed from the app top bar and re-added under **Settings → Log Parsing** (with a Date icon), right before the "Similarity threshold" group; it toggles the same `show_custom_date_popup` modal as before.

Replaced hand-drawn `paint_icon` + `ui.interact` buttons with **default egui `Button`s** (icon or icon+text) wherever they fit, so hover feedback and animation come from egui itself and stay consistent across the app. New reusable helpers in `src/ui/icons.rs`: `icon_button_at` (icon-only button placed at an exact rect) and `icon_text_button_at` (icon+text button), both using `ui.put` + zero/minimal `button_padding` for precise placement on the painter-layout timeline. Converted: timeline filter eye **visible/invisible** toggle and **trash delete** per lane (and the Everything Else eye), the **reset-zoom** button, and the bottom-bar **"Show/Hide all filters"** + **"Clear all filters"**; log-view search **▲/▼/✕** now use `Button::new(icon_image(...))` (nav stays disabled/muted when no matches). Also added a **whole-lane hover highlight** in `src/ui/timeline/view.rs`: hovering any part of a filter lane (or Everything Else) paints a translucent lane-colored fill+border background spanning the label column through the lane content, so the visible/invisible marker, filter text, delete button and lane read as one controllable row; the native buttons still render their own hover on top. (`src/ui/timeline/view.rs`, `src/ui/log_view/view.rs`, `src/ui/icons.rs`.)

`sample.log`-style lines use a **non-zero-padded hour with a 12-hour `AM/PM` marker** (often preceded by `U+202F`), which the ISO-8601 regex (zero-padded 24h hour, no AM/PM) rejected, so no timestamp family was detected and the file had no timeline/date. Added a separate `src/core/time/iso12.rs` family (single/double-digit hour, `AM`/`PM`, space/narrow-no-break-space tolerant, 12h→24h conversion) registered in `TIME_FORMATS`, and a full **custom date-recognizer** system: users define a regex with named groups (`year month day hour min sec` + optional `ms`, `ampm`), verify it live in a new **"Custom date"** popup (top bar) which prints `Year: … Month: … Date: … Hour: … Min: … Sec: … MILLI SECOND: …`, and persist to `~/.haystack/custom_date_format_list.json`. Custom recognizers are compiled and tried alongside the built-ins whenever a file is opened (`LogDocument::open_with_custom` / `load_with_custom`), with a "Re-scan active log" button to re-run on the current file. (`src/core/time/iso12.rs`, `src/core/time/custom.rs`, `src/core/time/mod.rs`, `src/core/document.rs`, `src/core/format/mod.rs`, `src/core/settings.rs`, `src/ui/custom_date/`, `src/ui/app/model.rs`, `src/ui/app/view.rs`.)



In `render_row` (`src/ui/log_view/view.rs`), every `LayoutJob` section's background was being overwritten with the row-selection colour (`galley.format.background = bg`), which erased the per-span search and keyword highlight backgrounds that `line_job`/`append_highlighted` paint behind matched text. As a result neither search-box matches nor double-clicked keywords showed any highlight. Removed that overwrite so the themed `search_highlight_bg` / `keyword_highlight_bg` (the "text highlight colour") render on every visible matching span, on every row, and re-apply as you scroll (selection tint still applies to non-highlighted spans). Regression test: `line_job_preserves_search_and_keyword_highlight_backgrounds`.

# Fix: log-view header ▲/▼/✕ buttons were inert; double-click no longer populates the search box

Two log-view navigation fixes (`src/ui/log_view/view.rs`):
- The **▲ / ▼ / ✕ buttons** in the search box did nothing when clicked. Root cause: `icons::icon_image` returns an `egui::Image`, which only senses **hover** by default, so `Response::clicked()` was always false. They now get `.sense(egui::Sense::click())`, making Previous/Next match and Clear search work (arrows stay disabled when there are no matches; hover tooltips still show).
- **Double-click** on a log line now only paints the keyword highlight and no longer populates the search box (`find_input`) nor triggers egui's native word text-selection. The content label is now `.selectable(false)` (mirroring the line numbers). Highlighting stays case-insensitive and persists across scrolling until cleared by Esc or a single click — matching the originally intended behavior.

# Fix: log-view search controls alignment, Enter-to-search, and case-insensitive double-click highlight

Three log-view navigation fixes (`src/ui/log_view/view.rs`, `src/ui/app/model.rs`):
- Search controls are now **left-aligned** after the status ("N lines") text + separator instead of being right-aligned by a `right_to_left` layout in the toolbar.
- **Enter** in the search box now actually runs the search. Previously the handler checked `response.has_focus()`, but egui surrenders focus (and the event) on Enter in a singleline `TextEdit`, so the branch never fired; it now uses the documented `response.lost_focus() && key_pressed(Enter)` idiom and re-requests focus.
- **Double-click keyword highlight** now matches **case-insensitively** (`build_find_automaton(…, true)`), so every case variant of a word highlights across the log view, consistent with the case-insensitive find box. Regression test: `keyword_highlight_is_case_insensitive`.

# Log view: header search box + double-click keyword highlight

The log view now has two lightweight, ephemeral navigation aids that sit inside the log view and never touch the filter set:

1. **Header search** — type + Enter runs a background scan, shows `n / total`, and `▲`/`▼` step through matches. Left/Up and Right/Down step matches from the keyboard.
2. **Double-click keyword highlight** — double-clicking a word in a log line paints every occurrence of that word in the visible rows. Esc, or a single click on a row, clears it. The keyword is also pre-filled into the search box so Enter promotes it to a full search.

Search is case-insensitive and scans only currently-visible lines when lane filters are active.

# Fix: CI release smoke test now builds the binary first

The Release workflow (`.github/workflows/release.yml`) failed in the "Smoke-test built binary" step on Windows/macOS because `cargo-packager` does not build the binary itself (and `cargo test` only leaves test-harness artifacts), so `target/<triple>/release/haystack` never existed. Added a `cargo build --release --target ${{ matrix.target }}` step before the smoke test (which also guarantees the binary exists for the packager step).

# Native OS installers for release (cargo-packager)

Releases now publish **native installers** instead of raw binary tarballs/zips. `.github/workflows/release.yml` builds `haystack-<version>-setup.exe` (NSIS) on Windows, `.deb` + `.AppImage` on Ubuntu, and `.dmg` (Apple Silicon + Intel) on macOS, uploading them plus `checksums.sha256` to the GitHub Release. The packaging config lives in `Cargo.toml` under `[package.metadata.packager]` (identifier, icons, NSIS/macOS/Linux options). New committed icon assets in `assets/icons/` (128×128 app icon as requested, plus 256/512 PNGs and `haystack.ico`); on Windows the `.exe` itself embeds the icon + version info via `build.rs`/`winres`, and the NSIS installer uses the same 128-px icon. Manual "binary → installer" steps are documented in `docs/release.md`; `scripts/package-release.sh` wraps build+package for one command.

# Fix cross-platform keyboard shortcut startup deadlock

Moved the platform check outside egui's input lock so the Backspace/Delete shortcut alias cannot re-enter the context lock and deadlock the GUI on Windows or macOS. Added regression tests covering the public consumer and macOS shortcut path.

# Embed Space Mono font for log text

Log text (the central log view, the pin preview modal, and the pinned-lines panel) is now rendered with the embedded **Space Mono** monospace font instead of egui's default mono. The four Space Mono faces (Regular/Bold/Italic/BoldItalic, ~410 KB) are baked into the binary via `include_bytes!` and registered under a dedicated `space_mono` egui font family in `src/ui/fonts/` — so **only log text** uses Space Mono, while the rest of the UI (timeline axis labels, settings, template panel) keeps its default fonts. Space Mono is SIL OFL 1.1 licensed (`OFL.txt` ships next to the TTFs; credited in the README). The A−/A+ font size controls still drive the log text size. Tests: font embedding/registration unit tests.

# Timeline filter controls, delete confirmations, pin editing, new pop-out icon

GUI timeline/filter/pin UX batch:
- **Filter tooltip with match count** — hovering a timeline filter label/eye now shows the full filter text plus its total match count (e.g. `Some Filter (334 occurrences)`) instead of just the truncated name (`src/ui/timeline/view.rs`).
- **Delete-filter confirmation is always-on by default** — new persistent setting `skip_filter_delete_confirm` (`~/.haystack/settings.json`, default `false` = always ask). The "Remove Filter" popup gained a **"Do not ask me again"** checkbox that flips and saves the setting; Settings popup gained the matching *Do not ask before deleting a filter* checkbox (`src/core/settings.rs`, `src/ui/app/view.rs`, `src/ui/settings/view.rs`).
- **Timeline bottom toolbar** — when filters exist, two left-aligned buttons under the minimap: **Hide/Show all filters** (toggles every filter lane at once, never touches "Everything Else", re-enables it if the view would go blank) and **Clear all filters** (confirmation popup → removes them all). New `LogTab::toggle_all_lanes` / `LogTab::clear_all_filters` + `pending_clear_filters` state (`src/ui/timeline/view.rs`, `src/ui/app/model.rs`).
- **Pin editing** — each pinned card gained an ✏️ **Edit** button that reopens the same pin creation window pre-filled (`LogTab::pin_edit_index`); `save_pin` now updates the entry in place instead of appending. The pin modal moved to a shared `log_view::pin_modal_ui` drawn at the app level so it works from any dock tab / detached viewport (`src/ui/app/model.rs`, `src/ui/pin_viewer/view.rs`, `src/ui/log_view/view.rs`, `src/ui/app/view.rs`).
- **`window_resize.svg` redesigned** — replaced the single-rectangle-with-corner-arrows with an asymmetric double-rectangle (overlapping windows) icon so the pop-out affordance reads clearly.
- Tests: settings serde default + round-trip for the new flag, `toggle_all_lanes` (toggles all lanes, keeps Everything Else, blanks-safe), `clear_all_filters`, and `save_pin` edit-in-place vs new-pin behavior.

# Rename app to haystack + app icon + settings links

Renamed the project from `waddaheck` to `haystack` everywhere: Cargo package/lib/bin name, the single binary + MCP CLI (`haystack` / `haystack --mcp`), the data dir `~/.waddaheck` → `~/.haystack`, the log filename, the MCP server name + config strings in the integrate guide, examples, release workflow, and docs. The app icon is now the bundled `src/ui/icons/haystack_256.png` — decoded at startup via the `image` crate and set as the native window icon, and rendered in the top-left corner of the toolbar next to the "LOGotomoy" app name. The settings popup gained **Report Bug** (opens the GitHub issues page) and **About** (opens the GitHub repo) buttons, both opening the default browser via a new cross-platform `open_url` helper in `src/ui/settings/view.rs`.
# MCP: `with_filtered_log=true` with zero filters short-circuits with a "no log" hint

All 8 MCP analysis tools (`find_occurrences`, `raw_log`, `log_sequence`, `summarize_log`, `get_timeline_histogram`, `get_template_anomalies`, `get_template`, `get_template_samples`) now **short-circuit** when invoked with `with_filtered_log=true` (the default) while `filter_count == 0`: instead of silently scanning the whole file they return a successful `{"comment":"no log","reason":"with_filtered_log=true and no filter count = 0, so no log. Try with with_filtered_log=false for full log file traversal or add filter (tool: filters_add)"}`. The check runs immediately after document resolution (before other arg validation) and applies uniformly in headless + GUI modes. `with_filtered_log=false` restores full-file traversal with zero filters. Tests: `with_filtered_log_no_filters_short_circuits_every_tool` (all 8 tools) plus full-log-path assertions; existing full-log tool tests now pass `with_filtered_log:false`.

# MCP: filter tools (filters_get/filters_add/filters_remove) + filtered-log default for all analysis tools

`src/mcp.rs` now keeps a per-log filter keyword set (`ServerState::filters`) managed by three new tools — `filters_get`, `filters_add` (case-insensitive dedupe, 20-cap), `filters_remove` (by position) — sharing `[{id, filter_text}]` shapes across headless (`log_id`) and GUI (`_active`) modes. Every analysis tool (`find_occurrences`, `raw_log`, `log_sequence`, `summarize_log`, `get_timeline_histogram`, `get_template_anomalies`, `get_template`, `get_template_samples`) gained an optional trailing `with_filtered_log` flag that defaults to **true**: results are then restricted to the union of the filter set's matches (the GUI "Everything Else" lane is never included), and `false` restores the full log. With no filters set the filtered view is the whole log, so existing calls are unchanged. In GUI mode the filter set round-trips with the served tab's live lanes via new `poll_mcp_filters`/`sync_mcp_filters` (mirroring the active-doc dirty-flag pattern). Tests: `filters_get_add_remove_flow`, `filters_cap_and_missing_args_error`, `filters_work_headless_with_log_id`, `with_filtered_log_defaults_true_and_excludes_everything_else`, per-tool filtered-view tests, `schema_advertises_filter_tools_and_with_filtered_log`.

# Fix: MCP dirty-doc swap no longer panics on stale viewport indices

When an MCP tool call mutated the served document, the GUI's `poll_mcp_dirty` swapped in the new (often trimmed/smaller) `LogDocument`, but tab view state (`context_line`, `viewport_range`, scroll/selection anchors) still held line indices from the previous larger window. On the next frame `ensure_visible`/`ensure_viewport_visible` fed those stale indices into the unchecked `ts_at()` (`src/core/document.rs:167`), panicking on an out-of-bounds access (`index 1060, len 1000`) and aborting the app. Fix (`src/ui/app/model.rs`): a new `LogTab::clamp_view_state()` clamps all doc-positioned view state to the current window after the dirty-doc swap, and `ensure_visible`/`ensure_viewport_visible` now use bounds-checked `ts_at_opt` and bail out gracefully instead of letting `ts_at` panic. Regression tests: `ensure_viewport_visible_ignores_stale_out_of_range_viewport`, `ensure_visible_ignores_stale_out_of_range_context_line`, `clamp_view_state_brings_stale_indices_back_in_range`.

# Fix: MCP HTTP server force-`Connection: close` on error responses

Error (4xx/5xx) HTTP responses from the MCP server (`src/mcp.rs`) now include a `Connection: close` header. Clients hitting a rejected path — a request with a missing or wrong secret token, a bad JSON body, or an unknown route — previously had to wait on the keep-alive socket (in practice timing out with 0 bytes, e.g. `GET /health` without the token). Now the server tells the client to disconnect so failed requests terminate immediately. Added unit + end-to-end regression tests (`http_response_error_statuses_send_connection_close`, `http_rejects_missing_secret_with_404_and_closes`).

# MCP: dynamic port + secret-token URL, top-bar start/stop + copy-instruction

The GUI-started MCP server no longer uses a port from settings — it binds an OS-assigned dynamic port and appends a fresh random 6-digit secret token to its URL (`http://127.0.0.1:PORT/SECRET`), which the server enforces (wrong/missing token → 404). "Start MCP"/"Stop MCP" and "Copy MCP instruction" now live in the top toolbar next to Settings (shown when a log tab is active), starting MCP auto-copies the connection instruction and shows a 5s toast, and the settings port input was removed (Start is disabled with "MCP already running" while a server is up).

# Show detected log format + date format in the status bar

The top toolbar now shows the active log's detected format and date format (e.g. `format: json · date: field-based`, `format: plain · date: ISO-8601`, `format: cef · date: none`) via a new `HaystackApp::selected_log_format_status()` helper in `src/ui/app/model.rs`, rendered as a muted label in `src/ui/app/view.rs`. JSON reports `field-based` since its timestamp comes from a field rather than a positional date format.

# Add Apple Unified Logging System (ULS) text format

Added `src/core/format/os_log.rs` to recognize the columnar `log show` text exports (default and `--style compact`), mapping the `0x…` thread/activity columns, PID/TTL, and mixed-case level (`Default`/`Info`/`Debug`/`Error`/`Fault`) into a clean `OSLOG <level> <process> <message>` Drain header while the ISO `+HHMM` timestamp feeds the timeline. Renamed the previous `oslog.rs` (the simplified `[Subsystem:Category] LEVEL:` console shape) to `oslog_console.rs` so the two Apple formats are unambiguous; the binary `tracev3` store remains out of scope.

# Add pluggable log-format detection & normalization (format → time → Drain)

Reworked the parsing pipeline so the log *format* is detected first (JSON, CEF, RFC 5424, logcat brief, iOS OSLog, or `plain` fallback), then the timestamp family, then each line is normalized per-format before Drain mining. Replaced `src/core/timestamp.rs`'s closed `Kind` enum with one-file-per-type module trees: `src/core/time/` (iso, slash, syslog, apache, epoch, logcat_threadtime, glog) and `src/core/format/` (json, cef, rfc5424, logcat_brief, oslog, plain). Each recognizer/extractor has its own unit tests plus a common validator test asserting exactly one intended format is picked; structured formats now mine field-aware templates (JSON schema + `msg`, CEF pipe headers, RFC 5424 structured headers, logcat tags, OSLog subsystem/category). `LogDocument` gained `format_name()`/`time_format_name()`; existing plain/iOS/bench logs still detect as `plain` + ISO.

# Merge redundant MCP keyword tools and fold log_size into summarize_log

Reduced the MCP tool surface for tighter agent tool-selection: removed `get_occurrence_count` and `get_occurrence_time_range` (their count + `first_seen`/`last_seen` are now returned by `find_occurrences`, which also gained optional `after`/`before` time-window filtering), and merged `log_size` into `summarize_log` (now returns `template_size_bytes` and `sequence_estimate_bytes`). Headless mode drops from 14 to 11 tools, GUI mode from 12 to 9. Updated `src/mcp.rs`, its tests, and the tool tables in `docs/mcp.md`, `AI_ASSISTANT.md`, `UserGuide.md`, and `feature.md`.

# Redesign MCP tool API to a stateless, self-describing canonical set

Reworked the MCP tool surface around how AI agents actually investigate logs. Retired three overlapping tools (`get_logs_within_time` → `raw_log`, `get_templates` → `get_template`, `get_log_sequence` → `log_sequence`) and added a `log_size` budget tool plus GUI-only `trim`. `summarize_log`, `get_timeline_histogram`, and `get_template_anomalies` now accept optional `start`/`end` ranges (line number or time), and `log_sequence`/`raw_log`/`get_template` take line- or time-bounded ranges too. Responses self-describe size (`log_size` returns `template_size_bytes`/`sequence_estimate_bytes`; `log_sequence`/`raw_log` return `total`/`returned`/`truncated`) instead of requiring separate `*_size()` pre-calls. Added `LogDocument::trim_range`, a shared `resolve_bound`/`resolve_range` helper, and 20 new unit tests (trim_range, dense/collapse/truncated log_sequence, raw_log line+time, log_size, get_template, trim cache invalidation, and range-restriction on summarize/histogram/anomalies). `src/mcp.rs`, `src/core/document.rs`.

# Rename app "keywords" to "text filters" and the saved keyword-set "Templates" dropdown to "Saved filters"

The GUI's text-filtering feature is renamed from "keywords" to "filters"/"text filter" across code identifiers, comments, UI labels, and docs (e.g. `tab.keywords`→`tab.filters`, `Keyword`→`Filter`, `MAX_KEYWORDS`→`MAX_FILTERS`, timeline `keyword_buckets`→`filter_buckets`, module `src/ui/keywords/`→`src/ui/filters/`). The toolbar dropdown that saved/loaded keyword sets was previously labeled "Templates", colliding with Drain template mining; it is now "Saved filters" (`core/template.rs`→`core/saved_filter.rs`, `Settings::default_template`→`default_filter`, `~/.haystack/templates/`→`filters/`). Drain log-structure mining and the MCP tool API (which still uses the `keyword` parameter) are intentionally untouched.

# File-open progress shown in its own new log tab; release matrix targets only supported OS/archs

Opening a file (Recent, Open dialog, or drag-drop) while at least one log tab is open now creates and auto-focuses a dedicated new log tab that shows the loading progress — it no longer appears "inside" the current log tab's content area. Loading tabs appear in the top tab bar as `<name> ⏳` with an inline cancel, and become the normal log tab once loading finishes (`active_loader` state in `HaystackApp`). Also updated `.github/workflows/release.yml` to keep only latest runners (ubuntu-latest, macos-latest, windows-latest) and to publish binaries solely for windows x86_64, ubuntu/linux x86_64, and mac arm (Apple Silicon) + x86_64 — dropping the `ubuntu-24.04-arm` (aarch64 linux) build.

# Recenter timeline zoom when log-view shadow scrolls out of view

When scrolling the log view scrolls the visible-range shadow (window shadow) completely outside the current timeline zoom window, the timeline view is now recentered on the shadow's midpoint (preserving the zoom span). Previously only the shadow band moved while the timeline zoom window stayed frozen, so the user could lose their position. Added `LogTab::ensure_viewport_visible()`, called from `update_viewport_range`.

# CI + release workflows: native runners, no cross, iOS-log tests & benchmarks

Updated `.github/workflows/rust.yml` (CI) to run the `gen_ios_logs` example tests explicitly (`cargo test --release --example gen_ios_logs`) and to benchmark against a generated `iOS-100K.log` (`gen_ios_logs -- 100K` then `bench -- iOS-100K.log ERROR user_id`) instead of the no-arg synthetic log. Rewrote `.github/workflows/release.yml` to build each target natively on its own runner — `ubuntu-latest` (x86_64 linux), `ubuntu-24.04-arm` (aarch64 linux), `macos-14` (Apple Silicon), `macos-13` (Intel), `windows-latest` (x86_64) — removing the `use_cross` flag, `taiki-e/setup-cross-toolchain-action`, and the `:arm64` cross system-deps step. Each release build now also runs the full test suite (main app + example) and an iOS-log benchmark before packaging.

# Rewrite iOS test log generator in Rust: deterministic PCG64, no Python, 100K/1M sizes

Replaced the Python-in-shell generator with a pure-Rust Cargo example (`examples/gen_ios_logs.rs`). The generator uses a seeded PCG64 (`rand_pcg`), so the same `--seed` produces byte-identical output on every platform — no Python, no cross-version drift. It supports arbitrary sizes (default 1K + 10K, plus 100K and 1M via `--all` or positional args) with streaming `BufWriter` generation so 1M lines stays memory-safe. Messages are token-parametrized with ~30 seeded value generators (user IDs, IPs, status codes, UUIDs, durations, etc.), source files grew 12→20, threads/PIDs and timestamps (bursty 1–5 lines/sec, random microseconds) are randomized, and FAULT lines carry multi-frame crash stacks. Every size starts from the same seed, so output is byte-identical across runs and smaller files are exact prefixes of larger ones. Added 7 unit tests (determinism, prefix property, token scanner, level distribution, FAULT frames, exact count). `rand` + `rand_pcg` added as dev-dependencies. Removed the `examples/gen_ios_logs.sh` wrapper (the cargo example is the canonical entry point) and gave `examples/profile_pipeline.rs` an optional logfile argument so it can profile the generated iOS logs (defaults to its own synthetic ~64MB log).

# Fix clustering quality + 4x parsing throughput: token-based masking, learned headers, Drain tuning

Reworked the log-parsing pipeline for both clustering quality and speed. Masking is now token-based (scalar byte heuristics, no regex on the hot path) with `key=value` keys preserved (`status=200` → `status=<NUM>`) and a per-document memo cache; a header learner samples the first N lines to force-mask consistently-dynamic header slots (host/pid/thread); Drain got quality fixes (sim_th 0.4→0.5, wildcards score half-credit in similarity, >70%-wildcard templates stop attracting weak matches) and perf fixes (FxHashMap, stack-rendered length keys); ISO timestamp parsing got a scalar fast path replacing chrono strptime. New "Log Parsing" settings section exposes `sim_threshold` and `header_sample_lines`. Result on the 64MB bench log: 6 → 26 MB/s, 0 wildcard-degraded templates (was: templates collapsing to `<*>` after 4-5 words). Added `examples/profile_pipeline.rs` for per-phase timing and a clustering-quality regression test.

# Add pre-mining masking for Drain template clustering

Added `src/core/masking.rs` with a `LogMasker` that replaces dynamic values (IPs, IPv6, UUIDs, hex IDs, URLs, file paths, emails, inline JSON, times/durations, numbers) with semantic placeholders (`<IP>`, `<HEX>`, `<UUID>`, etc.) before Drain clustering. This improves template quality by clustering structurally-similar lines that differ only in dynamic values. Uses `LazyLock<Regex>` for zero-cost compilation, `Cow<'_, str>` for zero-allocation fast paths, and a byte-scan pre-check to skip regex work on simple lines. Integrated into `document.rs`'s analysis pipeline after timestamp stripping. Added 21 unit tests and 1 integration test.

# Restructure src/ui folder for consistent module organization

Renamed and reorganized UI modules for clarity: `app_model.rs` → `app/model.rs`, `app_view.rs` → `app/view.rs`, `timeline_panel/` → `timeline/`, `settings_viewer/` → `settings/`, `keywords.rs` → `keywords/view.rs`, and all `*_view.rs` files → `view.rs`. Each UI feature now follows a consistent folder pattern with `mod.rs` + `view.rs` (and `model.rs` where state is needed). Updated all imports and module references throughout the codebase.

# Remove dead code: unused context_panel module, empty model files, unused Theme fields, _tz_marker

Removed the entire `src/ui/context_panel/` module (never referenced from any code path), four empty model files (`log_view/model.rs`, `pin_viewer/pin_viewer_model.rs`, `timeline_panel/timeline_panel_model.rs`, `settings_viewer/settings_viewer_model.rs`), six unused `Theme` fields (`chip_text`, `status_green`, `progress_bg`, `progress_fill`, `template_id`, `timestamp`), the unused `context_radius` field on `LogTab`, and the `_tz_marker()` helper in `src/core/timestamp.rs` (along with its now-unused `TimeZone` import). All icons and their `#[allow(dead_code)]` markers are intentionally kept.

# Timeline: 20 keywords, fixed-height always-visible panel, eye/trash lane controls

The timeline now supports up to 20 keyword lanes (was 6) and is rendered in a fixed-height top panel so the whole timeline — histogram, all lanes, axis labels, and the zoom/minimap strip — is always fully visible and can never be shrunk by the user. Each keyword lane draws a straight 1px line in the keyword's color across the full lane width. Lane filtering now uses visible/invisible (eye) SVG icons instead of check/uncheck, and each keyword lane has a trash icon that opens a confirmation dialog before removal. The "Everything Else" lane stays first, uses the eye toggle, and cannot be removed. The top keyword chip row (with ✕ buttons) was removed since removal now lives in each lane. The keyword input is disabled at the 20-keyword cap. The timeline can still be popped out into its own window via a header button.

# Replace emoji/text icons with embedded SVG icons

All UI icons now use actual SVG files from `icons/` embedded directly into the binary via `include_bytes!`. The new `src/ui/icons.rs` module renders SVGs to textures using `resvg` + `tiny-skia` (pure Rust, cross-platform, no system deps) with a global texture cache keyed by (icon, color, size). Icons adapt to dark/light theme via `currentColor` CSS injection. The `resvg` crate is gated behind the `gui` feature so MCP-only builds stay lean.

# Fix Windows CI: skip mandatory shared lock, use in-place writes in tests

`load_inner()` now skips `try_lock_shared()` on Windows (where `LockFile` is mandatory and would block log writers from appending, breaking live tailing). The same-size-content-change test uses an in-place write instead of `std::fs::write` (which truncates and fails with `ERROR_USER_MAPPED_FILE` on Windows). The file-shrink test is gated to non-Windows since `SetEndOfFile` is refused on a mapped file — the mmap itself prevents shrinking on Windows.

# Fix selection count for filtered logs

The drag-selection popup now correctly counts the number of selected lines when the log view is filtered (e.g. by a keyword). It previously calculated `end - start + 1`, which was incorrect for non-contiguous selections, and now counts the actual visible lines within the selection range.

# Compression-first MCP tools for low-token AI log analysis

Added 5 analysis tools (available in both headless and GUI modes): `summarize_log` (one-call orientation: stats, top/error-ish templates, biggest time gaps, densest minute), `get_log_sequence` (window around a line/time with consecutive same-template runs collapsed — 10–50× token reduction), `get_timeline_histogram` (tiny `{x, counts}` distribution for whole log / keyword / template), `get_template_anomalies` (rare, first-seen-late, bursty templates), and `get_template_samples` (n concrete lines per template). `find_occurrences` gained `format="refs"` (anchors only, no raw text) and `context=N` (collapsed lines around each hit). Shared core functions serve both modes; GUI mode still hides `log_id`.

# Fix overflow-leaf catch-all defeated by normal similarity threshold

`child()` now returns `(&mut Node, bool)` where the bool is true only when bucketing under `"*"` due to capacity overflow; `add_line()` tracks this across token-routing levels and uses a permissive threshold of `0.0` at overflow leaves, so same-shape overflow lines cluster together and wildcard out instead of being split into separate clusters.

Replaced the Cline-only `.clinerules/` setup with a single source of truth: `AI_ASSISTANT.md` (compact rules + vital facts + short structure) and `AI_ASSISTANT_DETAIL.md` (the former `.clinerules/Project.md`, moved to top level). All assistant entry-point files (`AGENTS.md`, `CLAUDE.md`, `GEMINI.md`, `SKILL.md`, `.clinerules/Follow.md`) are now identical 2-line pointers to `AI_ASSISTANT.md`, so Claude, Codex, Gemini, and Cline all read the same canonical context with zero drift.

# Strip timestamps before Drain template mining for cleaner templates

`TimestampExtractor::extract` now returns `Option<(i64, Range<usize>)>` including the byte-offset span of the matched timestamp. `LogDocument` stores these spans in a new `ts_spans` field and strips the timestamp from each line before passing it to Drain, using a temporary `Cow::Owned` allocation only for timestamped lines. This produces significantly cleaner templates (e.g. `INFO request from <*> completed in <*>ms` instead of a separate template for every unique timestamp value).

# Fix MCP server port not freed on stop — shutdown flag + error popup

When stopping MCP from the GUI, `stop_mcp()` now signals a shutdown `AtomicBool` flag and joins the server thread, causing the HTTP listener loop to exit and release the port. Previously the thread kept running indefinitely, blocking the port on restart. On bind failure (e.g. port in use), an error popup is shown in the UI explaining the failure. The `run_http` function now takes a `shutdown: Arc<AtomicBool>` parameter and uses non-blocking accept with a 100ms sleep poll loop.

# Dual-mode MCP server: simplified GUI tools (no log_id) + headless tools preserved

When MCP is started from the GUI, the server now exposes 5 simplified tools (`get_occurrence_count`, `get_occurrence_time_range`, `get_logs_within_time`, `find_occurrences`, `get_templates`) that operate on the currently active log without requiring a `log_id` parameter. The `load_log`/`list_logs`/`close_log` tools are hidden in GUI mode. Headless mode (`haystack --mcp`) retains all 8 original tools. The active tab's document is shared directly (no disk reload), and MCP-originated mutations are auto-refreshed in the UI via a dirty flag. The MCP start button is disabled when no tabs are open with a "Open a log file first" hint. The serving tab shows a "📡 filename (MCP)" badge.

# Consolidated MCP, Integrate, and Settings menus under a single Settings popup

Moved the MCP server controls and AI integration guide out of standalone top-bar buttons into the Settings popup. Created a new `src/ui/settings_viewer/` module to host all settings-related UI. MCP status now shows a green/gray circle indicator with hover tooltip showing the running URL. Port input is disabled when MCP is running. Integration guide opens as a modal Window from a button inside settings. Code blocks in the guide are now theme-aware (dark/light backgrounds).

# Fixed pop-out windows: restored show_viewport_immediate block that was accidentally deleted during popup refactoring

The `ctx.show_viewport_immediate(...)` call that creates detached child viewport windows was accidentally removed when replacing `egui::Window` popups with `egui::Area`-based ones. Restored it between the CentralPanel and the MCP dropdown popup section.

# Fixed context menus and popups for egui 0.35 proper usage

Replaced all 5 `egui::Area`-based dropdowns (MCP, Integrate, Recent, Settings, Templates) with close-on-click-outside behavior. Replaced the selection popup `egui::Window` (used as floating popup with `title_bar(false)`) with `egui::Area` + `Frame::popup` for proper dismissal. Pin modal, new/rename template modals remain as `egui::Window` (correct usage for dialogs). Context menus via `context_menu()` are already correct for egui 0.35.

# Upgraded egui from 0.31 to 0.35 LTS

Upgraded all GUI dependencies: `eframe` 0.31 → 0.35, `egui_dock` 0.16 → 0.20, `rfd` 0.15 → 0.17. Fixed breaking changes: `TopBottomPanel`/`SidePanel` → `Panel::top`/`Panel::right`, `default_width` → `default_size`, `ctx.style()` → `ctx.style_of(Theme::from_dark_mode(...))`, `ctx.screen_rect()` → `ctx.globally_used_rect()`, `ctx.used_rect()` → `ctx.globally_used_rect()`, `ui.close_menu()` → `ui.close()`, `egui::Theme::default()` → `egui::Theme::default_style()`. Removed `show_viewport_deferred` usage (requires `Fn + 'static`) and the detached viewport feature. Removed test_app.rs binary. All 30 tests pass.

# Updated egui from 0.31 to 0.32

Upgraded all GUI dependencies: `eframe` 0.31 → 0.32, `egui_dock` 0.16 → 0.17, `rfd` 0.15 → 0.17. Fixed the `ui.close_menu()` deprecation in favor of `ui.close()`. No API breakages from the egui 0.32 release were encountered (the new popup/menu APIs are additive, not breaking). All 30 tests pass with zero warnings.

# Unified Pin & Analyze into single Pin feature — sorted cards, "…" gaps, time deltas, Enter/Esc modal

Removed separate Pin (pinned_lines) and Analyze (analyses) features. Replaced both with a single `PinEntry` struct storing line range, all line numbers, timestamps, and optional user comment. Right-click "📌 Pin" and drag-selection "📌 Pin" both open the same modal (Enter to save, Esc to cancel, Shift+Enter for newline). Bottom panel rewritten: pins sorted by start timestamp, shown as framed cards with bold comment (if any), log lines in smaller font, "…" between non-consecutive lines, and "after 2 sec" style duration labels between pins.

# Max line width tracking, non-selectable line numbers, keyword background alpha, removed color marker

Added `max_line_width` to `LogDocument` (computed during load) to enable horizontal scrolling in the log view. Split line numbers into a separate non-interactive label with `{n}:` format so they aren't selectable during drag. Removed the `▌` color marker character and replaced it with a keyword-color background at ~0.2 alpha on matched spans. Added TODO for future bold keyword support.

# Fix selection popup buttons not responding to clicks (Pin / Analyze / Cancel)

Removed `ui.close_menu()` calls from the selection popup buttons inside `egui::Area`. `close_menu()` is designed for egui's context menu system, not standalone `Area` popups, and was interfering with button click event propagation. Also added `drag_start_pos = None` cleanup to the Analyze button handler (was missing compared to Pin/Cancel).

# Trim Log: right-click context menu to trim lines before/after a selected line

Added "Trim right" (← ✂️) and "Trim left" (→ ✂️) options to the right-click context menu in both the timeline and log views. When triggered, the document is trimmed in-place: per-line arrays are narrowed, templates are re-mined, and keywords + timeline are rebuilt for the new range. A trim indicator (✂️ N / M lines) with a "↺ Reset" button appears in the log view toolbar when the document is trimmed. The `LogDocument` now stores `trim_start`/`trim_end` bounds and keeps the full mmap intact. 7 unit tests cover trim_left, trim_right, composition, reset, edge cases, timestamp preservation, and template rebuild.

# Fix detached view returns to original docked position instead of tabbing next to timeline

When a popped-out view window is closed, the dock layout is now restored from a full `DockState` snapshot saved before the first pop-out, preserving the original vertical split arrangement (Timeline top, Log middle, Pinned bottom) instead of collapsing the returned tab alongside whatever is currently focused.

# Fix detached viewport window: black screen, not closing, stale tab index

`update_detached` was a stub that only showed a placeholder label and didn't set theme visuals, causing a black window. Close handling didn't clean up `viewport_map` or `detached_views`, so eframe kept recreating the window every frame. Also fixed stale tab index in `viewport_map` by re-resolving via path lookup instead of storing a raw `usize` that goes stale on tab reorder/removal. Switched from `show_viewport_deferred` (empty closure) to `show_viewport_immediate` with inline rendering so the child viewport actually renders content.

# Fix log view left alignment + refactor show() into helpers

Replaced `add_sized` with `allocate_ui_with_layout` + `Layout::left_to_right(Align::Center).with_main_justify(true)` so log text is left-aligned instead of centered (add_sized forces `Layout::centered_and_justified` internally). Broke the monolithic `show()` into 6 focused helpers: `show_toolbar`, `compute_pending_scroll_offset`, `render_row`, `apply_context_actions`, `draw_scroll_indicator`, `update_viewport_range`. All existing behavior preserved.

# Fix log view row-height drift and scroll-to-line accuracy

Row heights now match the virtual layout contract exactly: `item_spacing.y` is set to 0 before `show_rows` so egui's internal `row_height_with_spacing == row_height`, and each row is rendered with `add_sized` + `truncate()` to prevent wrapping. Scroll-to-line uses `vertical_scroll_offset` computed before `show_rows` (deterministic, same-frame) instead of the broken `scroll_to_rect` in content coordinates. The "lines visible" label moved into the top toolbar so the ScrollArea gets the full available height.

# Fix diamond click not updating log view when line is outside visible range

Moved `scroll_to_rect` inside the `show_rows` callback so the scroll request targets the correct ScrollArea, and force `viewport_range` to include the target line on the same frame so the timeline shadow immediately covers the selection marker.
# Diamond click centers log view; double-click on timeline also selects + centers

Replaced `vertical_scroll_offset` with `ui.scroll_to_rect(…, Align::Center)` inside `show_rows` so the selected line is reliably scrolled to center view on every click.

# Persistent settings (recent files, dark mode, MCP port) + file logging

Settings (dark mode, MCP port, recent files list) are now persisted to `~/.haystack/settings.json` and logging writes to both stderr and a rotating file in `~/.haystack/logs/`.

# Timeline shadow position shift (viewport_range miscalculation)

Fixed four bugs causing the timeline viewport shadow to drift right of the selection marker: double height subtraction, off-by-one in last_virtual, Sequence domain 0-vs-1 mismatch, and out-of-bounds map_to_real fallback.

# Top bar alignment, timeline shadow/marker, keyword labels, log view expansion

Unified toolbar control heights, added theme-aware viewport shadow colors, clamped shadow range to always cover context_line, made active keyword lane labels bigger/bolder, and expanded log view to full available height.

# Toolbar alignment + timeline marker/shadow consistency fix

Flattened add-keyword section to match toolbar height, left-aligned status/MCP/Integrate controls, and expanded viewport shadow to include context_line when a click just happened so the selection marker is never outside the shadow band.

# Dashboard UI revamp: full-width timeline, viewport shadow, compact add-keyword, theme toggle, larger diamonds, log view expansion

Removed wasted 120px label column when no keywords exist, added viewport shadow band linking scroll position to timeline, moved add-keyword to toolbar, bumped theme toggle to far right, enlarged diamonds, and reclaimed log view height by removing collapsed-panel hint.

# Pin/Analysis, timeline ■/□ markers, inter-tick durations, prominent keyword add, scroll bar

Added right-click pin/analysis system with collapsible bottom panel, replaced checkbox lane toggles with ■/□ markers, drew duration labels between tick pairs, wrapped keyword add in a prominent frame, and added a scroll-position indicator bar.

# Single binary (GUI + MCP server merged)

Merged separate GUI and MCP binaries into a single `haystack` binary with an `--mcp` flag, sharing loaded documents and ServerState between modes via a `gui` feature gate.

# Dark/Light mode & logging

Replaced hardcoded colors with a `Theme` struct (30 semantic colors, dark/light constructors) and added `log`/`env_logger` for structured logging to stderr and file.

# Timeline layout improvements

Moved keyword labels to a compact left column with truncation+tooltip, added 1px density lines per lane, pan/zoom hint icons, full-height histogram, and larger axis font.

# Timeline zoom, pan fixes

Replaced `powi` with `powf` for continuous zoom that works on trackpad and mouse, and fixed erratic panning by using a direct pixel-to-value shift.

# MCP Server — Streamable HTTP transport

Replaced raw TCP with full HTTP/1.1 (JSON-RPC, SSE, CORS, health check) plus static port selector and stdio command path copy in GUI.

# Timeline lane toggles, log filtering, font size, color markers, smart axis

Added lane checkboxes with log filtering, increased legend width/font, smart axis labels (drops redundant date/hour), A−/A+ font size controls, and per-line keyword color markers.

# Remove keyword crash

Fixed index-out-of-bounds panic when removing keywords by bounding lane iteration to both keyword_buckets and keywords lengths; added unit test for length invariance.


# AI Coding Agent integration popup

Added a "🤖 Integrate" button that opens a scrollable popup with per-agent config instructions and copy buttons for Claude Desktop, Claude Code, Cline, Cursor, GitHub Copilot Chat, and OpenAI Codex.
# Fix: compact yellow filter controls and selection popup behavior

Made timeline/log filter controls compact with a light yellow treatment and trailing Enter affordance, removed the log header line-count text and row-number gutter, and improved selection pin popup placement and auto-dismiss behavior.
# UI: navigate selected filter lanes and multi-result searches with arrow keys

Filter lanes can now be selected by clicking their row or an occurrence diamond. Left/Right arrows navigate the selected lane's occurrences, while Up/Down arrows navigate multi-result Log View searches; contextual headers, SVG arrows, and shortcut toasts explain the controls.
Added a modular embedded-data inspector: JSON is now joined by logfmt/key-value, colon fields, Foundation/Python/JVM debug values, HTTP, protobuf text, stack traces, and JWT/Base64/hex/PEM payloads. Each format has an isolated detector and Log View highlighter with dedicated detection and visualization coverage; stack traces render as frames and encoded data requires an explicit preview action.
Timeline occurrence navigation now uses the requested larger, animated layout; timeline captions and buckets are resized, and clicking a filter name toggles its visibility.
Templates are now a dockable, pop-out-capable view with cached sorting/search, midpoint-aware occurrence navigation, and rare/late/bursty cues. Selected rows expose an inline SVG action strip, while template filter labels remain regular text filters pending template-aware filter support.
# Make Template ID timeline filters functional

Timeline filters now include a **Template ID** type that accepts `42`, `T42`, or `T{42}` and matches Drain template IDs directly. The Templates-tab action now creates that typed filter instead of an ineffective text phrase; matching state persists in investigation sidecars and is covered by core and UI-path tests. Template occurrence navigation also safely handles targets before the first occurrence.
# Performance: bound and reuse derived search caches

MCP keyword matches now use a 128-entry/64 MiB LRU and filtered unions are reused until their document or filter set changes. The Templates view also reuses its filtered ordering allocation while the query and document stay unchanged.
# Performance: compact visible and Find indexes

Filtered visible-line and Log View Find results now retain 32-bit line IDs, and lane recombination merges the already-sorted hit lists instead of allocating one byte per line per filter. Large lane toggles therefore avoid the previous multi-million-line temporary arrays while preserving include/exclude and Everything Else semantics.
# Add compatible advanced Log View search

Log View search now has a native SVG search icon and the same Text (Aa), Text (Ab), Regex, and Template ID matchers as Timeline filters, with live validation, typed filter promotion, and close/Escape cancellation. Selected Templates rows now start a Template ID Log search instead of exposing first/previous/next/last buttons.
# Recover visibly from invalid saved log settings

Opening a log now prompts before an invalid saved profile or sidecar can block loading. Users can continue with automatic detection and repair only the invalid profile; malformed state is replaced only after a successful open, while newer-version and unreadable sidecars are left untouched.
# Flag weak applied log formats

The top-bar Format control now turns amber when an applied format matches fewer than 90% of source log lines. Its menu shows the exact match percentage and directs users to check and update the format.
# Make log-format save requirements and editing responsive

The log-format editor now marks a missing required name and explains why Save is disabled. Left/Right arrow navigation and Backspace dismiss template suggestions without consuming native text-editor input.
# Repair invalid saved formats and keep template arrows responsive

Invalid unshipped saved log formats are discarded and the presets file is repaired automatically, with details recorded in the application log instead of a persistent Format warning. Template completion now routes Left/Right cursor movement directly while its popup is visible.
# Remove obsolete log-format compiler code

Removed unused field variants and custom-field compilation code left behind after the log-format grammar was simplified. The project now builds without warnings from that path.
