# Log format UX specification

Follow-up proposal: [Log format editor refinement](log-format-editor-refinement-plan.md) defines the requested compact editor, generic names/ignore, and RF0–RF8 commit checkpoints. Its implementation is pending UI approval; see the [resume ledger](log-format-editor-refinement-checkpoints.md). The older specification below remains historical context where the refinement explicitly supersedes it.

Status: proposed design, part of the [implementation plan](log-format-editor-and-field-search-plan.md). No application implementation in this change.

## Design intent

The editor should feel like a small code editor with a live result. A programmer can discover the grammar in place, type arbitrary names, and understand what will happen before applying. New and Edit share one layout. Saving a reusable definition and applying it to a file are distinct operations with explicit labels.

## New and Edit layout

Default size: 880 × 740 logical pixels, constrained to the viewport minus 24 pixels on each edge. Comfortable content width: 800 pixels maximum, centered when expanded. Content scrolls vertically with a persistent footer; avoid a separate scroll area for every section. At small heights the footer remains visible and content scrolls beneath it. No horizontal window scrolling.

```text
New log format                                                  ×
Describe a log header. Check the extracted values before applying.

Name
[ MyApp console                                                ]

Sample log                           [Use current log] [Example ▾]
[ 2026-07-15 22:26:39.907481+0300 MyApp[12345:9] <FAULT>          ]
[ CameraService.swift:295 EXC_CRASH (SIGKILL) — jetsam killed …   ]
Paste one or more complete log entries. Long lines wrap visually.

Template
[ {time} MyApp[{a}:{b:number}] <{loglevel}> {log}                  ]
Required: {time} and {log}. Keep {log} last—it captures all remaining text.
Fields: {time}  {log}  {thread}  {process}  {loglevel}
Types: token · number · path · text                   [Syntax help]
Any name works: {a} means {a:token}. Try {b:number} or {source:path}.

✓ Sample matched · 1 record                         [Show highlights]
Field           Type            Value
time            timestamp       2026-07-15 22:26:39.907481+0300
a               token           12345
b               number          9
loglevel        token           FAULT
log             message         CameraService.swift:295 EXC_CRASH …

▸ Date parsing: Auto
─────────────────────────────────────────────────────────────────
Applies to: iOS-100K.log
[Cancel]                              [Save] [Save & apply]
```

The wireframe's sample wraps represent visual wrapping, not inserted newlines. Values abbreviated in this illustration must be expandable and copyable in the product. Field rows are read-only results, never a second set of definition inputs.

Edit title: **Edit log format — MyApp console**. Under the name, show `Saved format · Used by this file` or `File copy · Differs from saved format` as applicable. Initial New name is empty, with placeholder `e.g. MyApp console`; do not save every new item as “New log format.” New template starts with `{time} {log}`. Example → MyApp fills the supplied sample and compact template. Editing an applied format opens its exact snapshot, not an independently updated saved version.

### Typography, spacing, and colors

- Body, hints, rules, tables, and button labels: 15 logical pixels with about 21-pixel line height. Sample/template/value text: monospace at 15 pixels. Window title: 20 pixels; section labels: 16 pixels medium weight. No tiny helper text.
- Outer padding: 20 pixels; section spacing: 16 pixels; label-to-control gap: 6 pixels. Inputs have 10-pixel padding; buttons and menu rows are at least 32 pixels high. Avoid bolding every label.
- Main text and essential hints meet a 4.5:1 contrast target in both themes. Use neutral inset backgrounds and restrained borders for inputs. Focus has a visible accent border. Only the primary action receives a filled accent treatment.
- Syntax colors distinguish literals, field names, and types subtly. Matching highlights add a soft background and underline. Pair colors with names, text statuses, and focus indication; color alone never conveys errors or field identity.
- Sample starts at about three visual rows and expands to six before internal scrolling. Template starts at two rows and grows to four. Template soft-wraps; actual newline insertion is rejected with `Templates describe one header line.` A trailing clipboard newline can be removed; embedded newlines are not silently turned into separators.
- Completion/help panels stay inside the viewport. On a narrow editor, toolbar controls wrap to the next line, value tables become stacked Field / Type / Value rows, and footer actions wrap without disappearing.

### Sample controls

Use current log chooses the owning record near the selected line, including a bounded continuation sample. Show the file and source-line range when imported. Disable it with `Open a log to use its sample` when no file is open. No automatic replacement while the user types or changes tabs.

Example offers **MyApp console**, **Service with request ID**, and **Quoted path**. Selecting an example replaces the sample and template together as one reversible draft action. Keep the user's name. Offer **Undo example** after replacement. Use current log replaces only the sample. Copying or soft-wrapping must preserve source bytes; visible control-character cues may be toggled through Syntax help.

## Hints and grammar help

The three short reference lines in the wireframe stay visible. Clicking a field reference inserts that complete placeholder at the template's remembered selection and returns focus there. After `:` is typed, type reference buttons complete the current declaration. Otherwise a type click inserts `{name:type}` with `name` selected. Each insertion is one undo step. Do not append blindly to the end or produce nested braces.

**Syntax help** opens an anchored, scrollable panel approximately 440 pixels wide. It contains the following concise examples at body size, with no navigation to a separate setup window:

| Entry | Explanation shown |
|---|---|
| `{time}` | Timestamp. Detected automatically. Required once. |
| `{log}` | Everything after the header, including spaces and structured content. Must be last. Continuation lines belong to this message. |
| `{thread}`, `{process}` | Common identifier names; token unless a type is specified. |
| `{loglevel}` | Severity such as INFO, FAULT, or your application's own level. |
| `{request_id}` | Your own name. Same as `{request_id:token}`. |
| `{count:number}` | A signed number, decimal, or scientific notation. |
| `{source:path}` | Windows/Unix filename or path, or HTTP(S) URL. |
| `{description:text} \|` | Text with spaces up to an unambiguous separator. |
| `"{source:path}"` | Quotes in the log bound a path containing spaces. Quotes are literal template characters. |
| `{{` and `}}` | Match literal braces. |

End with: `Names must be unique. Separate fields with literal text or whitespace. Types do not change the original captured value.` Explain that adding quotes to a template only works when those quotes exist in the sample. List accepted legacy aliases in a folded **Older names** section, not the main reference.

**Date parsing: Auto** expands only date-family choice and optional year for yearless dates. Keep Auto selected for a New format; show a concrete assumed year when relevant. Regex and old custom validators are exposed only when editing a legacy profile, clearly labeled, rather than being a prominent mode switch that erases the draft.

## Autocomplete contract

```text
Template: {time} MyApp[{process}:{worker:nu|}] <{loglevel}> {log}
                                      ┌─────────────────────────────┐
                                      │ number   9, -12, 3.5, 1e3    │
                                      │ Enter/Tab to insert · Esc   │
                                      └─────────────────────────────┘
```

The panel anchors to the caret; show at most six suggestions before scrolling. At `{`, offer time, log, thread, process, loglevel, and a hint `Or type your own field name`. Hide already-used semantic fields. At `{worker:`, offer token, number, path, text with one short example each. Typing an arbitrary name with no built-in match shows `Custom field “worker” · token`; it is not an error.

Up/Down moves selection while the panel is open; Tab/Enter accepts only an explicitly highlighted suggestion. Initial type/name matches may highlight the first result, but completing a custom name must never replace it with a different built-in. Escape closes only the panel, preserving text. Ctrl+Space reopens it. Outside click closes it. Keep existing closing braces and replace only the active partial name/type; do not duplicate delimiters. Undo restores both text and caret. Disable acceptance while an IME composition is active.

With no completion selected, Enter in the template requests immediate preview and does not submit the dialog. Tab follows normal focus order when no completion is active. App-level navigation/search shortcuts do not run while editing.

## Live results and failure states

Preview validates after 200 ms of inactivity; show a spinner only if the check lasts another 150 ms, avoiding flicker. Each result belongs to the current draft revision. During an edit, dim the previous table and label it `Updating…`; remove old success styling immediately. Autocomplete can tolerate unfinished syntax even when the complete template cannot compile.

| State | Exact style and example copy | Actions |
|---|---|---|
| Empty sample | Neutral: `Paste a sample log to check this template.` | Valid format may be saved; apply waits for active-file check. |
| Incomplete declaration | Neutral inline hint: `Finish the field with }.` | Save/apply unavailable until complete. |
| Missing mandatory field | Inline error: `Add {time}. Every new format needs a timestamp.` | Underline relevant location; no modal error. |
| Trailing content after log | Inline error: `{log} must be last. Move it after the other fields.` | Keep draft editable. |
| Unknown type | Inline error: `Unknown type “num”. Choose token, number, path, or text.` | Offer number as a completion, no automatic rewrite. |
| Literal mismatch | Warning: `Expected “MyApp[” after time.` | Mark expected/actual boundary in sample. |
| Invalid value | Warning: `b expects a number; found “worker-9”.` | Keep earlier valid captures as explicitly partial results. |
| Ambiguous split | Warning: `source can end in more than one place. Use a clearer separator.` | Show competing boundaries on demand; no arbitrary winner. |
| Successful sample | Success icon + `Sample matched · 1 record` | Table shows exact captures. |
| Multiline sample | `2 records · 3 continuation lines · 1 unmatched line` | Select a record to inspect captures; unmatched preamble has a jump action. |
| Preview capped | Neutral info: `Showing the first 200 lines of this sample.` | Do not imply the entire sample was checked. |

Field table follows declaration order. Show raw timestamp plus parsed date/offset details on expansion, not epoch numbers as the primary value. Long values wrap to two lines and have **Show all** / **Copy** actions. Selecting a field highlights its declaration and sample span; selecting another clears the prior focus. A record selector appears only for multiple records. For continuations, show `Part of record starting at sample line 1`.

In the editor, draft capture values have **Copy**, not live-file Search/Filter actions: a draft has not necessarily been applied. Search / Add filter belong on the applied record's field inspector. This avoids querying the current file using a different draft schema.

## Save, update, and close semantics

| Context | Secondary actions | Primary action |
|---|---|---|
| New, file open | Cancel, Save | Save & apply |
| New, no file | Cancel | Save |
| Edit saved format | Cancel, Save as new, Save changes | Save & apply (when file open) |
| Edit file snapshot differing from library | Cancel, Save as new | Apply to this file |

For a differing snapshot, **Update saved format…** is an explicit additional action inside the saved-format status row. Its inline explanation names the format to be updated; it never silently overwrites a newer saved revision. A duplicate name on New shows `A saved format has this name. Choose another name or edit it.` Names are compared case-insensitively for display uniqueness, but identity remains a stable ID.

Saving does not reparse files. After Save, keep the editor open, clear the dirty marker, and show `Saved. This file still uses [current name].` Saving changes updates the reusable preset, not other open tabs. Save & apply saves first, then runs the same validation/reparse flow as the menu. If apply fails, explain `Format saved; could not apply to [file].` If saving fails, keep the draft and do not apply silently.

Apply to this file installs an embedded snapshot without changing the library. Successful apply closes the editor and briefly reports `Applied MyApp console to iOS-100K.log`. No-file mode omits apply actions; it does not show unexplained disabled buttons. Save requires a nonempty unique name where applicable and a syntactically valid format, but a sample mismatch may still be saved for a different file.

Bind the editor's apply target to the tab from which it opened and show the filename in the footer. If that tab closes, allow saving but disable apply with `Original file was closed`. Switching tabs never silently changes the target. The editor is modal to avoid inadvertent main-view edits while typing.

Escape first closes completion/help, then requests editor close. Cancel/×/Escape closes an unchanged draft immediately. A dirty draft shows an inline footer prompt **Discard changes?** with **Keep editing** and **Discard**, defaulting to Keep editing. Outside click does not discard or close the modal. No global Enter-to-apply shortcut. Cmd/Ctrl+S saves; keyboard focus and screen-reader labels identify all controls.

## Top-level Format control

```text
Open file   Recent   Saved filters   Commands   [Format ▾]   …
                                               │
                 ┌────────────────────────────────────────────┐
                 │ Current file · iOS-100K.log                 │
                 │ MyApp console                              │
                 │ {time} MyApp[{a}:{b:number}]                │
                 │ <{loglevel}> {log}                  [Copy] │
                 │ [Edit current]              [New format]  │
                 │────────────────────────────────────────────│
                 │ [Search saved formats…                   ] │
                 │ ✓ MyApp console                            │
                 │   Service requests                         │
                 │   Web access                               │
                 │────────────────────────────────────────────│
                 │   Auto-detect                              │
                 └────────────────────────────────────────────┘
```

Button label remains **Format** with a chevron; a tooltip names the active format. Do not stretch the toolbar with a long name. Pending work adds a spinner beside Format for the affected active tab. Preserve the applied state until completion.

Popover width: 440 pixels, clamped to viewport; max height 70% of viewport. Same 15-pixel typography as editor, 12-pixel padding, 34-pixel list rows. Current name/syntax and bottom Auto-detect action stay visible; only the saved list scrolls. Syntax wraps and Copy copies it without visual line breaks. List row labels truncate with full name available on focus/hover. Selected state uses both a check and a soft background.

Search filters names and syntax; placeholder **Search saved formats…**. Keyboard opening focuses search; pointer opening leaves focus on the menu. Up/Down navigates, Enter selects, Escape closes; Tab reaches Edit, New, Copy, and Auto-detect. Hover/focus on a saved row expands its syntax beneath that row without replacing the current-format header. No automatic apply on hover. Selecting the already-applied revision is a no-op. Selecting an updated revision of the same preset applies the update.

When a library revision differs, show `File uses an earlier saved version` below the current title and a **Newer version** badge on its saved row. An unsaved snapshot shows **File only**. Auto mode shows `Auto-detect · ISO timestamp + message`, or a structured adapter description; copy syntax only when a real template exists. A legacy format carries a **Legacy** badge with a short explanation on focus.

With no open file, header reads **No log open**, Edit/Auto-detect are omitted, and selecting a saved row opens it for editing instead of applying. New format remains available. With no presets, show `No saved formats yet` and **Create format**. No-result search shows `No formats match “[query]”` and **Clear search**. A preset-load failure shows **Could not load saved formats** with **Retry**, keeping current-format details available.

At narrow widths retain Format as a labeled top-level control; move Recent/Saved filters/Commands to More as needed. At the minimum supported width put utility controls on a second row rather than shrinking their font or hiding Format. Define and visually verify the exact breakpoint against actual measured button widths.

## Selecting and applying a format

1. Click a saved row. Keep the menu open and show `Checking Service requests against iOS-100K.log…` in a status strip. Keep the old current checkmark. Validation uses a bounded representative sample of that specific tab.
2. A valid representative match begins background apply automatically. Close the menu and show `Applying Service requests… [Cancel]` in the file's status area. The old view remains usable. A zero-match, ambiguous-date, or matcher-budget result keeps the menu open with a specific explanation and **Edit with file sample**; no automatic install.
3. Report sample evidence as `12 record headers found in 200 sampled lines`, never a percent success rate that incorrectly counts stack traces as failures. Existing field filters made incompatible by the change trigger a compact preflight notice listing them and **Apply and disable these filters** / **Cancel**. No generic confirmation for routine compatible switches.
4. During full parse, revalidate that records were found and that results still belong to the selected source generation. Install document, capture schema, compatible queries, and label together. If no records were actually found or the source changed incompatibly, keep the previous document and report the reason.
5. On failure keep old current-format identity, show **Retry** / **Edit format**, and retain the attempted selection for context. Cancellation is a neutral `Format change cancelled`. Successful apply reports the format and file and returns focus to the log.

Auto-detect follows the same process but clears the explicit file override only after success. A new selection supersedes a pending one for the same tab; a stale worker cannot install later. Reopening Format while work is pending shows current and pending names separately plus Cancel. Switching tabs shows that tab's own state; pending work stays attached to its original tab. Escape/outside click closes the popover but does not cancel already-started file work.

## Additional field types

Add **text** to the first release. It fills a real gap: names, descriptions, and quoted content can contain spaces without being paths. Example: `{time} user="{user:text}" status={status} {log}`. It uses the same unambiguous literal-boundary rules as path; it does not greedily swallow later fields. It is nonempty; optional/empty fields require a future explicit grammar, not a hidden wildcard rule. `{log}` remains the only arbitrary final remainder and may be empty.

Consider these later, in this order, when fixtures and query behavior justify them:

| Type | Benefit | Required semantics before adding |
|---|---|---|
| `integer` | Strict counts, process IDs, status codes | Signed decimal integer, exact ordering; reject decimals/exponents, unlike number. |
| `hex` | Addresses and hexadecimal identifiers | Define optional 0x prefix, width limits and comparison; preserve raw spelling. |
| `bool` | Flags with true/false filtering | Start with case-insensitive true/false; never silently coerce arbitrary yes/no/0/1 tokens. |
| `duration` | Compare 250ms with 1.5s | Explicit supported unit suffixes, decimal normalization, exact comparisons and display. |
| `ip` | Validate IPv4/IPv6 and later network filters | Keep ports/brackets explicit; normalization must not change displayed source. |
| `uuid` | Validate request/correlation IDs | Clearly defined accepted spelling; canonical equality with original display. |

Do not add all of these to initial autocomplete. token already captures UUIDs, IPs, and hex text; a new type earns its place by validation or useful typed operations. Do not add separate url/file types yet because path intentionally covers both. Do not add a quoted type: quotes are visible literal delimiters. Defer json/regex/optional/repeat declarations; they need separate grammar and performance contracts. Common field names are semantic conveniences, not an ever-growing type catalog.

## UX acceptance scenarios

Before phase 2 implementation, review wireframes for New, Edit saved, Edit differing snapshot, and a 640 × 480 viewport. Phase 2 is accepted only after keyboard and visual checks cover incomplete typing, completion, matched sample, malformed number, ambiguous path, long message, multiple records, no file, dirty close, and failed save in both themes/scaling settings.

Before phase 3 completion, verify no presets, many presets, search/no results, current automatic format, library-vs-file mismatch, successful switch, failed preflight, field incompatibility, cancel, rapid reselection, and tab close during apply. These states are required product behavior, not optional polish.
