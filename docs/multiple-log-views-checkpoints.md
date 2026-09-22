# Multiple Log Views — checkpoint ledger

Execution plan: [multiple-log-views-plan.md](multiple-log-views-plan.md).

## Current handoff

- Baseline revision: `795a22b` — `Constrain the settings dropdown height`.
- Current phase: MLV8 complete; implementation plan finished.
- Worktree at baseline: clean before these planning documents were added.
- Scope: local phase commits only; no push, tag, or release requested.
- Next action: optional interactive native-window checks on release platforms; no implementation work remains in this plan.

## Phase status

| Phase | State | Commit/evidence | Next step |
|---|---|---|---|
| MLV0 | Complete | `041d2dd` | MLV1 extraction |
| MLV1 | Complete | `d2173be` | MLV2 keyed model/routing |
| MLV2 | Complete | `70dd1fd` | MLV3 docked add/close |
| MLV3 | Complete | `d744579` | MLV4 detached windows |
| MLV4 | Complete | `6893d28` | MLV5 schema 4 persistence |
| MLV5 | Complete | `c9f029a` | MLV6 lifecycle integration |
| MLV6 | Complete | `59d71c5` | MLV7 UX/docs/performance hardening |
| MLV7 | Complete | `5bf567a` | MLV8 integrated verification |
| MLV8 | Complete | Pending checkpoint commit | Plan complete |

## MLV0 baseline — 2026-09-17

**Code inspection:** current `LogTab` owns both shared investigation state and the only Log View's selection, scroll, Find, row selection, embedded-data, inspector, and popup state. `ViewTab::Log` is a singleton unit enum. Detached views are keyed by `ViewTab`. The timeline reads and mutates the singleton `context_line`, `pending_scroll`, and `viewport_range`. Sidecar schema 3 persists one search, selected line, and scroll position plus serialized singleton dock state.

**Important constraints found:**

- filters, timeline buckets, pins, templates, document, tail/reparse work, and MCP attachment must remain file-level;
- current filter-result installation preserves only one viewport anchor and must reconcile all views;
- several egui IDs in Log View are unqualified and would collide across docked/detached instances;
- current detached-window render order cannot be used as focus order;
- file close currently cancels one Find receiver and must cancel every view receiver;
- format reparse reconstructs `LogTab` from one sidecar search/position and must preserve all views; and
- `egui_dock`'s built-in add action is DockArea-wide, so the middle-pane plus needs a Log-specific control.

**Baseline validation:** elevated `cargo test` passed: 366 library tests, 179 GUI/binary tests, and 44 integration tests (589 total); 0 failed and 0 ignored. The command was run outside the filesystem/network sandbox as requested. No application code was changed.

**Decisions requiring approval:** the detailed contract in the plan, especially new-view initialization, stable numbered titles, native-window close meaning re-dock, Cmd/Ctrl+W remaining whole-file close, shared timeline zoom, and MCP search targeting the focused view.

**Approval:** the user replied “Okay start” on 2026-09-17, authorizing the phased implementation and local checkpoints.

## MLV1 checkpoint — 2026-09-17

**Delivered:** introduced `LogViewState` and moved the single Log View's selection, scroll, drag selection, Find, field suggestions, keyword highlight, embedded-data scan, inspector, annotation popup, and viewport layout state out of the file-level `LogTab` fields. `LogTab` owns exactly one view at this checkpoint and uses a temporary `Deref` compatibility bridge, so existing rendering/navigation behavior and sidecar schema remain unchanged. Shared document, filters, timeline, pins, template browser, history, dock, tail/reparse, and MCP state remain file-level.

**Files:** `src/ui/app/model.rs`, `src/ui/app/tab_model.rs`, two architecture references, and this ledger. Small explicit-borrow adjustments in Log View and Timeline resolve the newly visible shared/view boundary without changing behavior.

**Tests:** added `new_tab_owns_one_extracted_log_view_state`. Focused test passed. `cargo check` passed. Elevated `cargo test` passed with 366 library, 180 GUI/binary, and 44 integration tests (590 total), 0 failures and 0 ignored. `cargo fmt` was applied; `cargo fmt --check` and `git diff --check` are the final pre-commit checks.

**Known limitation by phase design:** there is still one view, `ViewTab::Log` is still a singleton, and explicit widget-ID namespacing is deferred until stable `LogViewId` arrives in MLV2. No user-visible multi-view controls exist yet.

**Resume:** define `LogViewId`, replace the singleton field with a keyed ordered collection plus focused/MRU IDs, remove the dereference bridge from multi-view paths, and add model tests before exposing plus/close controls.

## MLV2 checkpoint — 2026-09-17

**Delivered:** added stable serializable `LogViewId`, an ordered keyed view collection, monotonic ID allocation, explicit focused/MRU tracking, location-only view forking, deterministic focus fallback, final-view protection, and per-view worker cancellation. Application polling visits every view-owned Find/embedded receiver while restoring the actual focused ID, so render order cannot redefine focus. File close now cancels every view's workers.

**Identity/overlay work:** Find inputs, suggestions, mode selectors, validation bubbles, full-line/embedded inspectors, analysis popups, and annotation callouts now include `LogViewId` in their egui identity. Overlay ownership inspects every view rather than only the focused one. The visible dock remains a single Log tab by phase design.

**Tests:** added independent fork/search/selection/widget-ID coverage and MRU close/cancellation/final-view coverage. `cargo check`, `cargo fmt --check`, and `git diff --check` passed. Elevated `cargo test` passed with 366 library, 182 GUI/binary, and 44 integration tests (592 total), 0 failures and 0 ignored.

**Known limitation:** shared filter-result installation, trim/reparse preservation, and full all-view document mutation reconciliation remain scheduled for MLV6. The currently hidden extra views are model-only until MLV3, and the dereference bridge still routes legacy APIs through `focused_log_view_id`.

**Scope update from user:** MLV5 must not migrate schema 1-3 sidecars because none of these schemas/features has shipped. It will persist schema 4 and test a safe non-crashing reject/reset fallback for older development sidecars only.

**Resume:** make `ViewTab::Log` identity-bearing, render the addressed view, implement header plus/close/external hit targets, and route genuine dock interaction to `focus_log_view`.

## MLV3 checkpoint — 2026-09-17

**Delivered:** made Log dock items identity-bearing, added stable numbered titles, unique dock IDs, an inline Log-specific plus action, native close buttons guarded by the final-view invariant, and activation of a newly forked view in the source leaf. The plus forks location only through the MLV2 model contract, while Find, row selection, inspectors, and transient state remain independent.

**Focus/timeline routing:** dock-tab clicks and pointer/scroll interaction select the addressed Log View. Rendering temporarily routes the compatibility bridge to the addressed state and restores the real focus afterward, so split or detached paint order cannot steal timeline ownership. A genuinely focused detached viewport can update the focused Log View.

**Tests:** added header/title geometry coverage for add, pop-out, and native close reservations; existing model tests cover fork/activate/close, owner-only worker cancellation, MRU fallback, and the final-view invariant. Focused Log View tests passed. `cargo check --all-targets`, `cargo fmt --check`, and `git diff --check` passed. Elevated `cargo test` passed with 366 library, 183 GUI/binary, and 44 integration tests (593 total), 0 failures and 0 ignored.

**Manual/native status:** no interactive native-window smoke test was claimed in this automated checkpoint. Multiple detached-window lifecycle and explicit permanent-close behavior are MLV4 scope.

**Known limitation:** shared filter-result installation and other document mutations still reconcile only the compatibility-focused view; all-view reconciliation remains explicitly scheduled for MLV6. Existing native-window close continues to re-dock rather than destroy the view.

**Resume:** make detached bookkeeping robust for several keyed Log Views, remove stale saved-layout resurrection paths, add explicit permanent-close behavior in detached headers, and test viewport identity/focus/close/re-dock semantics.

## MLV4 checkpoint — 2026-09-18

**Delivered:** detached viewport identities now include stable `LogViewId`, detached windows use numbered Log titles, and the viewport-to-file mapping is refreshed after top-level file-tab reorder. Several Log Views can be detached concurrently without sharing state or IDs. Actual native viewport focus selects its Log View; rendering alone does not.

**Lifecycle:** replaced the single full-layout snapshot restore path with independent best-effort return locations per dock item. Native window close queues that item for re-dock, while a dedicated detached header action permanently closes a Log View and cancels its workers. The final-view invariant disables permanent close. Closing a detached view cleans pending detach/re-dock state so a stale layout cannot resurrect it.

**Tests:** added stable/distinct detached viewport identity coverage and a two-detached-view lifecycle regression covering independent re-dock, permanent close, no duplicate dock entries, and no resurrection. `cargo check --all-targets`, focused tests, `cargo fmt --check`, and `git diff --check` passed. Elevated `cargo test` passed with 366 library, 185 GUI/binary, and 44 integration tests (595 total), 0 failures and 0 ignored.

**Manual/native status:** the model and viewport contracts are automated; an interactive macOS native-window smoke test has not yet been claimed. Windows/Linux native verification remains pending as planned for MLV8.

**Known limitation:** the existing schema-3 persistence fields still describe singleton view state and retain the now-unused pre-detach snapshot field. MLV5 replaces that representation with schema 4 rather than migrating it.

**Resume:** design the schema-4 DTO around the keyed view collection, serialize validated keyed dock/detached state, reject schemas 1-3 through the existing recovery path without conversion, and add corrupt-layout/default-one-view recovery tests.

## MLV5 checkpoint — 2026-09-18

**Delivered:** sidecars now use schema 4 with an explicit keyed Log View list, stable display numbers, focused view ID, independent search/selection/scroll anchors, typed detached-view entries, dock layout, detached return locations, and shared investigation state. Dirty comparison is deterministic: hash-backed detached collections are sorted, and transient `egui_dock` rectangle/viewport geometry is normalized while logical splits and tabs remain persisted.

**No migration:** schemas 1, 2, and 3 are rejected before schema-4 deserialization or application. There is no conversion path. The existing recovery dialog offers a fresh investigation and replaces an old development sidecar only after the log opens successfully. Writers also reject non-schema-4 state, preventing a hybrid file.

**Recovery/validation:** invalid or duplicate IDs/display numbers, a missing focused ID, missing/duplicate dock references, or a layout with no live Log View rebuilds one safe Log View without crashing. Valid shared filters and notes remain usable when only the view layout is corrupt. `next_log_view_id` advances beyond all restored IDs. Changed-source confirmation remaps anchors independently for every view.

**Tests:** added schema-4 JSON/default coverage; explicit non-migration coverage for schemas 1-3; multi-view/search/position/detached-layout round trip; stable reserialization; reopen after closing to one remaining view; duplicate/missing ID and bad-layout recovery; and changed-source all-view anchor confirmation. A generated pretty-JSON sidecar was inspected and contains the expected `schema_version`, `log_views`, focused ID, typed detached entry, and keyed dock items. `cargo check --all-targets`, focused tests, `cargo fmt --check`, and `git diff --check` passed. Elevated `cargo test --no-default-features` passed with 367 library, 3 binary, and 44 integration tests (414 total). Elevated default `cargo test` passed with 367 library, 188 GUI/binary, and 44 integration tests (599 total), 0 failures and 0 ignored.

**Known limitation:** mutation paths still use the temporary focused-view compatibility bridge in several places, so filter/trim/append/reparse/MCP reconciliation across all restored views is MLV6 scope. User-facing sidecar/workflow documentation remains scheduled for MLV7.

**Resume:** inventory every shared document mutation, introduce all-view anchor/clamp/reconciliation helpers, preserve the full schema-4 collection during format reparse, and route MCP GUI search effects to the focused view at delivery time.

## MLV6 checkpoint — 2026-09-19

**Delivered:** shared filter result installation now captures and reconciles each Log View's own selection or viewport anchor, then restarts every non-empty per-view Find session against the common visible-line index. Live-tail swaps invalidate embedded-data state in every view. Trim, reset/undo, and MCP document replacement rebase every view's line-positioned state to the new shared window, cancel stale per-view workers, clear stale results, and clamp all selections, scroll anchors, ranges, popups, and wrap caches safely.

**Routing/lifecycle audit:** schema-4 format reparse already snapshots and restores the full keyed view collection; its Template-ID and Field-query review loops now have explicit multi-view coverage. MCP GUI search remains isolated to the Log View focused when the result is delivered. Template, pin, timeline, format-current-line, and other selection-based commands continue through the focused-view routing bridge. File shutdown cancels Find, field-suggestion, and embedded-data workers for every view.

**Tests:** added two-view regressions for asynchronous filter visibility reconciliation, independent Find restarts over shared visibility, all-view clamping, physical-line trim/undo rebasing, format-reparse search review, focused-at-delivery MCP search, and all-worker shutdown cancellation. `cargo check --all-targets`, `cargo fmt --check`, and `git diff --check` passed. The sandboxed GUI suite reached 194/195 with only its expected loopback-listener permission failure; the requested elevated `cargo test` then passed with 367 library, 195 GUI/binary, and 44 integration tests (606 total), 0 failures and 0 ignored.

**Storage/performance:** document, filter matches, visible-line index, and timeline remain single file-level allocations. MLV6 adds no per-view document or shared-index copies; only independent searches/results and viewport-owned caches remain per view as designed.

**Manual/native status:** no interactive native-window smoke check is claimed in this checkpoint. Cross-platform native focus/re-dock verification remains an MLV8 gate.

**Resume:** document the user workflow, review header/tooltips/accessibility and compact behavior, then exercise at least eight docked/detached views to verify bounded worker scheduling and release-on-close behavior.

## MLV7 checkpoint — 2026-09-19

**Delivered:** Log tab hover and accessibility descriptions now explain add, separate-window, close, focused-navigation, and final-view behavior. The established compact inline geometry remains covered against close/pop-out overlap. The renderer-native screenshot hook has a deterministic `multiple-logs` state, and new light/dark fixtures visibly cover stable numbered titles plus add, pop-out, and close controls.

**Documentation:** updated the User Guide investigation-state, Log View, detached-window, focus, and keyboard contracts; README feature copy and light/dark screenshots; the feature inventory; architecture reference; and the running change log. Documentation explicitly distinguishes child Log Views from top-level file tabs and states that Cmd/Ctrl+W still closes the file.

**Resource/performance evidence:** a new eight-view regression starts independent searches in all views through the existing global pool (maximum four workers), waits for every result, verifies the single shared document Arc remains unchanged, closes seven views, verifies view-owned search highlighters are released, and retains exactly one usable survivor. Shared filter matches, visible lines, and timeline remain file-level fields and are not copied into view state; embedded detection and Find results remain intentionally view-owned.

**Validation:** `cargo check --all-targets`, focused accessibility/eight-view tests, `cargo fmt --check`, and `git diff --check` passed. Elevated `cargo test` passed with 367 library, 197 GUI/binary, and 44 integration tests (608 total), 0 failures and 0 ignored.

**Visual evidence:** generated and inspected `screen-multiple-logs.png` and `screen-multiple-logs-dark.png` at 2880×1694 through the renderer-native macOS GUI path. Both show two numbered Log tabs and readable add, external-window, and close affordances. This verifies light/dark docked header presentation and the default truncate mode only; it does not claim native detached-window focus/close behavior, Windows/Linux UI behavior, wrap/horizontal modes, or the full manual matrix.

**Resume:** audit unqualified widget/viewport identities and all close/restore paths, run headless/default/release gates, perform an appropriate release smoke check, and consolidate remaining manual/platform limitations into the final MLV8 evidence.

## MLV8 checkpoint — 2026-09-19

**Final audit/fix:** audited explicit egui identities across Log View rendering, Find/suggestions, analysis and annotation popups, inspectors, docks, and detached viewports. The main virtualized scroll area was the one remaining generic ID; it now includes stable `LogViewId`, with a regression proving two views produce distinct identities. Modal child controls inherit already-qualified modal IDs. No other unqualified Log View-specific explicit ID remains.

**Invariant audit:** model/UI tests cover monotonic IDs, independent state, MRU focus fallback, owner-worker cancellation, programmatic/dock close rejection for the final view, multiple detached views, OS-close re-dock modeling, explicit detached permanent close, stale-location cleanup, no resurrection, schema-4 round trip, malformed/duplicate/missing layout recovery to one safe view, changed-source anchor confirmation, and reopen after only one view remains. File close still uses the top-level tab path and cancels every view worker.

**Integrated validation:** `cargo fmt --all -- --check`, `cargo check --all-targets`, and `git diff --check` passed. Elevated default `cargo test --quiet` passed with 367 library, 198 GUI/binary, and 44 integration tests (609 total), 0 failures and 0 ignored. Elevated `cargo test --no-default-features --quiet` passed with 367 library, 3 binary, and 44 integration tests (414 total), 0 failures and 0 ignored. `cargo build --release` completed successfully in 7m46s, and `target/release/haystack --help` returned the expected usage text.

**Actual visual/platform evidence:** renderer-native macOS light/dark docked multi-view screenshots were generated and inspected in MLV7. Automated native-viewport IDs and detach/re-dock lifecycle pass. No interactive detached-window focus/OS-close session was performed, and no native Windows/Linux UI run is claimed. Those checks require their respective desktop environments and remain optional release-platform smoke evidence; Windows mmap ownership is structurally unchanged because views share the existing document Arc and never reopen the source.

**Outcome:** MLV0–MLV8 are complete as local phase commits. Multiple docked/detached Log Views, focus-aware navigation, shared investigation mutations, schema-4 persistence with deliberate no-migration handling for unreleased schemas 1–3, resource cleanup, documentation, screenshots, and integrated tests are all delivered. No push, tag, or release was performed.

## Checkpoint template

Copy and complete this section at every phase; update Current handoff and Phase status in the same commit.

```text
MLVN checkpoint — date
State: in progress / validation pending / complete
Previous checkpoint hash:
Current commit subject:
User approval reference (if newly supplied):
Delivered behavior and completed subtasks:
Files and important entry points:
Decisions/deviations and reasons:
Validation commands and exact outcomes:
Visual/platform checks performed; evidence paths:
Not run / failed / pending checks, with reasons:
Known limitations or unresolved issues:
Worktree changes intentionally left outside this phase:
Next phase and exact first task/command:
```
