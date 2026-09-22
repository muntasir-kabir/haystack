# Log format refinement — checkpoint ledger

Execution plan: [phase details and acceptance gates](log-format-editor-refinement-plan.md#7-commit-sized-phases-and-resume-protocol).

## Current handoff

- Baseline before this series: `791a5d8` — Phase 5 field search checkpoint: typed captured-value queries.
- Current phase: RF7, in progress.
- Implementation: the user removed the unshipped compatibility requirement on 2026-09-13. The editor now has one grammar: generic fields default to tokens and `{ignore}` is always a non-retained matching declaration.
- Approval: the user approved implementation on 2026-09-13 and later explicitly removed pre-release saved-profile compatibility from scope.
- Next action: complete RF7 dirty-close lifecycle and apply failure/recovery coverage; native RF2/RF3 visual, keyboard and IME evidence remains queued for RF8.
- Scope: local phase commits; no push/tag/release requested.

## Phase status

| Phase | State | Commit/evidence | Next step |
|---|---|---|---|
| RF0 | Complete | `7713423` | RF1 fixtures and baseline |
| RF1 | Complete | `d34e133` | RF2 compact editor/dropdown |
| RF2 | Implementation complete; visual validation pending | `b982aa2` | Capture visual evidence |
| RF3 | Implementation complete; native keyboard/IME validation pending | `a6c8c7e` | Capture native evidence |
| RF4 | Complete | `d74ab5e` | RF5 schema 3 and zero retained ignore captures |
| RF5 | Complete | `feat: support typed ignored header fields (RF5)` | RF6 examples and preview diagnostics |
| RF6 | Complete | `feat: explain log format matches and ignored fields (RF6)` | RF7 draft lifecycle and legacy conversion |
| RF7 | In progress | — | Finish dirty-close lifecycle and apply failure/recovery coverage |
| RF8 | Not started | — | Integrated validation |

## RF7 progress — 2026-09-13

**Delivered so far:** Apply is bound to the document path that opened the editor; changing active tabs disables Apply rather than redirecting a draft. Advanced options can explicitly create a schema-3 copy of a safe schema-2 template. Formats using advanced/custom rules or a captured legacy `{ignore}` refuse conversion so no capture is silently dropped. Template fields, including legacy drafts, now default to token matching without a separate custom-field definition.

**Validation:** focused legacy conversion tests (2 passed); `legacy_templates_default_undeclared_fields_to_tokens` (passed); the existing retained legacy-ignore integration test was started after the core change and awaits its runner completion. `cargo fmt --check` and `git diff --check` remain required before the RF7 checkpoint.

**Next:** finish dirty-close and failure/recovery behavior, then complete the RF7 test gate and checkpoint.

## RF0 checkpoint — 2026-09-13

**Delivered:** detailed UI/grammar/performance proposal, nine sequential phases with individual commit subjects, acceptance gates and resume instructions; linked historical plan/spec to this proposal.

**Files:** this ledger, `log-format-editor-refinement-plan.md`, cross-reference notices in `log-format-ux-spec.md` and `log-format-editor-and-field-search-plan.md`.

**Validation:** documentation-only. `git diff --check` passed before commit. Application tests not run because application code was not changed.

**Remaining:** UI and semantics approval; all implementation phases RF1–RF8. No implementation, screenshots of the proposed UI, or performance claims have been verified yet. User-supplied screenshots describe the current application.

**Resume:** read current handoff; record actual user approval before RF1. Do not reimplement old-plan captured-field search from scratch.

## RF1 checkpoint — 2026-09-13

**State:** implementation complete.

**Approval reference:** user message “Okay start implementstion as per plan” on 2026-09-13 authorizes the approved RF0 UI, ignore semantics, and compatibility direction.

**Delivered:** `refinement_contract.json` is a human-authored schema-3 acceptance fixture covering typed/repeated ignore, invalid ignored number, multiline message, escaped braces, and quoted path. Its cases are intentionally not parser assertions before RF5. `record_inline_contract.rs` now validates that fixture's structure and proves schema-2 `ignore` remains an ordinary retained custom field through JSON round trip, compilation, and preview. This prevents RF5 from silently reinterpreting existing profiles.

**Evidence:** [RF1 baseline evidence](log-format-refinement-evidence/rf1-baseline.md) records deterministic 100K/1M default/detailed parser results and the exact environment/commands. Generated inputs live only in `/private/tmp` and can be regenerated from the recorded command.

**Validation:** `cargo test --test record_inline_contract` (5 passed); `cargo test` (passed); `cargo test --no-default-features` (passed). Release parser benchmark used `gen_record_logs` seed `20260911` and `bench_record`; detailed values are in the evidence file. No UI changes or native visual checks occurred in RF1.

**Known limitations:** Schema-3 behavior is still deliberately unimplemented; the new fixture is a contract, not proof that the current parser accepts it. Measurements are single-run baselines on Intel macOS, not cross-platform performance results.

**Resume:** start RF2 from `src/ui/record_format/mod.rs` and `src/ui/app/format_menu.rs`. Preserve schema-1/2 behavior, do not advertise `ignore` yet, and use the RF2 gate from the plan before committing.

## RF2 checkpoint — 2026-09-13

**State:** implementation complete; native visual validation pending.

**Delivered:** the editor now presents the compact approved order: Name, collapsed Template rules & syntax, Template with an immediate validation line, Sample log, Parsing preview, then collapsed Advanced options and Diagnostics. Body, code and dropdown content use the denser 13 px scale; Name and Template share aligned rows; the content area has a profile-specific scroll identity so a freshly created draft starts at its top. The modal defaults to 900 × 660 logical pixels and retains its footer outside the scroll area. The Format dropdown keeps Edit/New beside the applied format and uses the same compact type scale.

**Compatibility:** parser and saved-profile behavior is unchanged. The RF2 rules deliberately retain schema-2 wording and do not introduce `ignore`; RF5 owns that grammar and storage change.

**Files:** `src/ui/record_format/mod.rs`, `src/ui/app/format_menu.rs`, `UserGuide.md`, `AI_ASSISTANT_DETAIL.md`, `changes.md`.

**Validation:** `cargo fmt --check`, `cargo check`, and `cargo test` passed. The full suite reported 362 library tests, 169 GUI/main tests, integration tests, and doc tests passing. No schema or parser implementation changed.

**Pending:** no native GUI screenshot/manual visual check was run in this non-GUI checkpoint. Before RF8, capture light/dark New/Edit/dropdown screenshots and verify the 640 × 480 viewport, collapsed disclosure defaults, footer reachability, and long values. This is recorded as pending evidence rather than claimed complete.

**Resume:** inspect this RF2 diff, then work in `src/ui/record_format/completion.rs` and a focused template-input/highlighting module for RF3. Preserve native Left/Right selection behavior while completion is open; do not use an in-flow completion frame.

## RF3 checkpoint — 2026-09-13

**State:** implementation complete; native keyboard/IME validation pending.

**Delivered:** completion now determines the complete active field/type token and replaces it as one edit, including text after the caret. This preserves existing braces and Unicode prefixes. A foreground Area anchors the bounded six-row completion list below the text caret, so it no longer consumes space above Sample/Preview. When that list is visible, only Up/Down/Enter/Tab are consumed before the native editor; Left/Right, modified navigation, selection and undo/redo remain native. Outside clicks dismiss the list, Escape continues to use the application’s existing completion-first behavior, and Ctrl/Cmd+Space opens completion at the current caret. Template text now highlights literals, field names, recognized types, punctuation and invalid/unclosed declarations.

**Tests:** added completion regressions for replacing a field suffix after the caret and replacing a type suffix while retaining its field name. `cargo fmt --check`, `cargo check`, and `cargo test` passed; the full GUI/main unit count is now 171, including all four completion tests.

**Pending:** native checks remain necessary for IME composition, macOS Command and Windows/Linux Ctrl handling, focus traversal, pointer selection, exact caret anchoring near every viewport edge, and visual contrast. No claim is made for those unrun checks. The syntax layouter is deliberately simple; RF4 owns caching/work avoidance and must not add parsing work to paint.

**Resume:** start RF4 by separating name/save metadata from parse revisions in `RecordEditor`, moving compile/custom-recognizer preparation off the UI frame, and adding deterministic stale-result/cancellation tests. Preserve RF3 input routing.

## RF4 checkpoint — 2026-09-13

**State:** complete.

**Delivered:** each parse-affecting editor change receives a monotonic revision, cancels the active preview, and leaves the latest displayed preview in place while it is stale. Preview template compilation and custom date recognizer construction move into the worker, and stale/disconnected worker results cannot overwrite current validation. Name changes do not request a parser run. The UI reports `Checking template…` while validation is pending and `Updating sample…` for a stale result; final Save/Apply validates the current profile on that action rather than during layout.

**Tests and validation:** added deterministic unit tests for invalidation without dropping the prior revision and stale worker rejection. `cargo fmt --check`, `cargo check`, focused editor tests, and `cargo test` passed; the GUI/main test count is 173.

**Known limits:** RF4 keeps the existing 200 ms debounce, bounded worker input, and cancellation. Syntax layout caching and spinner-delay measurement remain RF8 evidence/performance work. Native RF2/RF3 visual and IME checks remain pending as previously recorded.

**Resume:** RF5 starts in `src/core/record/{profile,template,matcher,preview}.rs` and document capture integration. Implement schema 3 as a separate version path; schema-2 `ignore` must remain retained. Do not expose new ignore completion/help until production capture storage and field-query behavior are complete.

## RF5 checkpoint — 2026-09-13

**State:** complete.

**Delivered:** schema-3 profiles introduce repeatable `{ignore}` declarations with the same `token`, `number`, `path`, and `text` type grammar as other generic fields. They participate in bounded header matching and type validation but receive no retained capture index. Document schema, field spans, Field search/filter suggestions, and parsed-value previews therefore contain only `time`, `log`, and named non-ignored fields; raw source search still reads the original header. Schema-1 and schema-2 profiles retain their existing behavior, including their historical retained custom field named `ignore`.

**Editor:** new formats use schema 3 and the rules/help and completion catalog expose `{ignore}`. Legacy formats do not receive an invalid ignore completion.

**Validation:** `cargo fmt`; `cargo check --bin haystack`; focused document test `refined_ignore_fields_match_headers_without_creating_capture_columns` (passed); `cargo test --test record_inline_contract` (6 passed before the final test-name-only change); full `cargo test` passed before the final document-level coverage addition. The final full-suite/no-default-features gates remain RF8 integration work.

**Known limits:** ignored fields use temporary matcher state while parsing a header, but no per-record capture column or span is retained. Native UI checks remain deferred to RF8.

**Resume:** start RF6 in the editor rules/examples and preview presentation. Include copyable starter templates, one deliberately ignored component, and a concise reason when a sample does not match.

## RF6 checkpoint — 2026-09-13

**State:** complete.

**Delivered:** the editor's Examples menu provides Basic timestamp/message, Named fields, Ignore header values, and Quoted path starters. Choosing one changes only the sample and template, preserving the user-entered format name. Preview summaries distinguish matched records from continuation lines. Diagnostics reports the number of successful ignored header declarations in the sample while retaining neither their values nor a capture column.

**Validation:** `cargo fmt`; `git diff --check`; focused preview test `preview_counts_ignored_header_values_without_exposing_them_as_fields` (passed); focused GUI editor test `applying_an_example_preserves_the_user_entered_name` (passed). Native visual checks, complete-record import, multi-record selection/copy, linked highlights, and richer structured mismatch locations remain RF8 or later RF6 follow-up work.

**Resume:** RF7 starts with stable apply-target identity, draft dirty/close behavior, and explicit legacy schema conversion. Preserve the RF5 rule that legacy captured `ignore` fields never silently become schema-3 noncaptures.

## Entry template for later checkpoints

Copy this block for each phase/follow-up and update the current handoff and status table in the same commit:

```text
RFN checkpoint — date
State: in progress / implementation complete / validation pending / complete
Previous checkpoint hash:
Current commit subject:
User approval reference (if newly supplied):
Delivered behavior and completed subtasks:
Files and important entry points:
Decisions/deviations and reasons:
Validation commands, outcomes, and measured results:
Visual/platform checks performed; evidence paths:
Not run / failed / pending checks, with reasons:
Known limitations or unresolved issues:
Worktree changes intentionally left outside this phase:
Next phase and exact first task/command:
```
