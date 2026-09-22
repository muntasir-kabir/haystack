**Log format templates, multiline records, and source-order timeline — phased delivery and performance plan**

Status: Phase 8 local verification documented on 2026-09-12; no push, tag, or release performed. Phases 0–7 have local commits. [Phase 8 measurements and remaining gates](record-parsing-phase8-report.md) are explicit. This document expands the [behavioral design](record-parsing-and-line-timeline-plan.md). Performance predictions below are engineering expectations; baseline measurements describe only the pre-implementation version.

**Delivery contract**

The default text layout is `{time} {log}`. Users can save and explicitly select layouts such as `{time} - [{log_level}] - {thread_id} {file}:{line} {log}`. An anchored layout identifies record starts, its designated time field supplies event time, and other physical lines continue the record. The primary GUI timeline always uses source-line positions. Known structured formats keep their field-aware adapters.

The initial release includes placeholder templates, custom fields with declared types, date-format selection, preview, sparse-header detection, explicit/inherited/unknown timestamp provenance, safe time queries, line-based navigation, investigation migration, and correct incremental tailing. It does not include arbitrary optional-template grammar, mixed-source stream reconstruction, whole-record Drain mining, or automatic clock correction. Advanced anchored rules cover unusual layouts without expanding the basic grammar indefinitely.

**Phase overview and dependencies**

| Phase | Deliverable | Depends on | Main risk |
| --- | --- | --- | --- |
| 0 | Fixtures, correctness oracle, and reproducible performance baseline | Existing code | Comparing unlike workloads or trusting the old parser as the oracle |
| 1 | Versioned profile model and compiled placeholder templates | 0 | Ambiguous field boundaries or expensive matching |
| 2 | Record classification, typed time state, format adapters, normalization | 1 | Inheritance leaks, malformed headers, or altered mining semantics |
| 3 | Bounded profile/date detection and diagnostics | 2 | Sparse headers, conflicting formats, and expensive discovery |
| 4 | Time queries independent of timeline coordinates | 2 | Missing matches on disordered clocks and incompatible range semantics |
| 5 | Line-based GUI timeline and exact record overview | 2, 4 | Hidden time assumptions in navigation/export/restore |
| 6 | Template editor, reusable presets, and atomic reparse | 1, 3, 5 | Loss of investigation state and temporary memory growth |
| 7 | Live-tail transactions and cache consistency | 2–6 | Partial-line replay and stale worker results |
| 8 | Performance tuning, compatibility verification, and release | All | Cross-platform regressions and misleading performance claims |

Suggested merge units are the phases, splitting large phases into core/UI commits where useful. Keep unfinished integration behind an internal development switch or on the feature branch. Enable the new behavior by default only when all release gates pass. The independent MCP time-bound correctness fix may ship earlier with its tests and documented contract.

**Phase 0 — Establish behavior and measurements**

Work:

1. Add small, human-reviewable fixtures with explicit expected record boundaries, timestamp values/status, exact source spans, and expected source-line order. Use the six-line payload-date/reversal example from the behavioral design as the central regression.
2. Cover the two user templates, time after level/thread prefixes, indented continuations, repeated times, missing-time structured headers, malformed calendars, JSON event fields versus JSON payload, and long sparse records. Expected values are authored from the fixture, not copied from current parser output.
3. Extend the Rust fixture generator to vary physical-line count, record length, header shape, timestamp placement, date family, backward-clock frequency, and payload size. Generate large inputs locally from fixed seeds; commit compact fixtures and generator code, not giant files.
4. Establish measurements against a recorded commit/compiler/feature set. Separate file generation and compilation from timed regions. Record line count, byte count, record count, filter hit counts, detected profile, and output correctness with every run.
5. Add actual pipeline instrumentation for index build, profile discovery, record classification/date conversion, normalization/masking, Drain mining, query selection, and timeline resolution. Keep instrumentation optional and out of normal rendering.
6. Record both retained storage and peak process memory. Existing `bench` exercises actual `LogDocument::open`, search, and timeline build/resolution. The existing `profile_pipeline` is supplementary: it materializes intermediate vectors, omits the final line through its `windows(2)` iteration without an EOF sentinel, and does not execute the complete production format-normalization path. Correct those limitations before using it for before/after stage comparisons.

Files: `tests/fixtures/`, new focused record/template integration tests, `examples/bench.rs`, `examples/profile_pipeline.rs`, and a Rust generator alongside `examples/gen_ios_logs.rs`.

Exit gate: every scenario has an independent expected answer, timings can be reproduced, and current failures are captured without requiring incorrect behavior to remain. Record known-broken cases as targeted regression work rather than leaving the default test suite intentionally red between merges.

Performance effect: instrumentation is opt-in; production behavior and runtime cost remain unchanged in this phase.

**Phase 1 — Profile schema and template compiler**

Work:

1. Add a focused core module, proposed `src/core/record/`, with separate `profile.rs`, `template.rs`, `matcher.rs`, and tests. Keep timestamp value parsing in `src/core/time/` and structured format handling in `src/core/format/`.
2. Define a serializable profile with schema version, stable ID/revision, display name, layout source, date-parser selection, prefix/whitespace policy, field definitions, and resource limits. Define a compiled immutable profile shared by `Arc`; do not add strings or regex instances to every line entry.
3. Parse literal text, escaped braces, and typed placeholders. Support `{time}`, `{log_level}`, `{thread_id}`, `{file}`, `{line}`, final `{log}`, and explicitly registered custom named fields. Distinguish source-code `{line}` from the physical line index.
4. Validate required/duplicate fields and layout order. A normal text template has exactly one final `{log}` and at most one `{time}`. A time-free layout can define timeless records; `{log}` alone explicitly selects independent physical lines.
5. Compile anchored literal comparisons plus typed field matchers. Keep a specialized path for `{time} {log}`. Date recognizers consume an exact token at their designated position, including embedded spaces; they never search the message. Literal brackets, pipes, and hyphens do not become regex operators.
6. Define unambiguous field delimiters. `{file}:{line}` must support drive-letter paths and validate the numeric suffix plus following separator. Reject layouts requiring unbounded segmentation/backtracking. Custom fields require a type and, optionally, a bounded validator.
7. Use a reusable scratch buffer/compact span collection for captures. Return borrowed source spans and header/message ranges, not a heap-allocated string map per line. Stop boundary matching at the message start; an arbitrarily long `{log}` should not increase header-matching work.
8. Initial tunable limits: 4 KiB template source, 32 captured header fields, 4 KiB inspected text-header prefix, and existing project regex compilation limits where reusable. Reaching a limit is a diagnostic, not permission to search the rest of the line. Structured-field adapters have separate byte/depth policies.
9. Make compilation errors actionable: unknown field, missing delimiter, ambiguous capture, duplicate time source, message field not last, or unsupported optional syntax. Advanced regex is explicit and anchored; it does not become Auto's fallback for failed lines.

Tests: literal escaping, spaces in dates, Windows paths, named thread IDs, level extensions, empty messages, custom field validators, capture offsets, invalid UTF-8 offset handling, and adversarial long headers.

Exit gate: compiled matching returns identical results in preview and batch parsing; layout compilation occurs once per revision; simple matching introduces no per-line heap allocation for field capture itself. Existing normalization/Drain allocations are measured separately.

Performance effect: one-time compile cost; bounded per-line validation proportional to inspected header bytes and configured fields. More fields cost more than the default layout. This phase must not try every saved template against every line.

**Phase 2 — Record state, time provenance, and normalization**

Work:

1. Introduce `Header`, `Continuation`, and `Unassigned` classification. A header's time state is valid, missing, or invalid. Match header shape separately from conversion so a recognizable header with an impossible date starts a new unknown-time record.
2. Make one sequential classifier own active record/time state. On every header, reset that state to the new known/unknown time. Continuations inherit only from that record. Preamble lines remain unknown; no forward fill across a new missing-time header.
3. Change `record_starts` to mean actual boundaries. Retain compact per-line offsets/times/spans/Drain IDs. Add a packed two-bit time-state representation if needed for valid/unknown/invalid distinctions; use a typed accessor throughout new code rather than inferring validity from `timestamp >= 0` or source-span representability.
4. Add a record-start rank directory: cumulative start count per 512 physical lines over the existing bitset. `count_records(a,b)` is the difference of two rank queries, each reading at most eight 64-bit words after its directory entry. Use rank/select-assisted boundary lookup for giant records instead of walking arbitrarily many empty bitset words on the UI thread.
5. Preserve separate header and timestamp ranges. Add sparse overflow handling for rare timestamp spans beyond the current packed limits. Keep normalized-text offsets mapped back to original source bytes for BOM/color sequences and lossy display; source extraction must not index raw bytes with shifted lossy-string offsets.
6. Implement adapters for all existing formats. JSON decodes its event object once and shares that result between field-time extraction and normalization; do not add a second full JSON parse for boundary classification. JSON message/nested dates are not event sources. RFC 5424 validates its actual timestamp slot, including missing time. Apache/glog and other nonleading-date formats use their own header grammar.
7. Use the same classifier in load, sample preview, header learning, and append. For custom templates, normalize explicit header fields directly. For automatic generic header learning, sample accepted headers only. Continuations receive content normalization without learned header masks.
8. Retain meaningful level and stable structural fields in Drain input; mask dynamic IDs/locations appropriately. Keep per-physical-line mining. Record the expected changes to cluster membership and template ID stability; do not treat identical old Drain IDs as a correctness requirement after reparsing.
9. Compute file time bounds and clock diagnostics only from valid record times. Count errors and save a bounded representative sample of diagnostic line IDs; repeated malformed input must not allocate an unbounded message per line.

Files: `src/core/document.rs`, proposed `src/core/record/`, `src/core/time/`, `src/core/format/`, and embedded-data boundary consumers.

Tests: payload dates cannot create boundaries; missing-time headers reset inheritance; malformed dates remain visible; raw spans round-trip; record lookup works across filters/trims; all preexisting structured formats retain their intended behavior. Include more than 512 lines without a header and span offsets beyond 65,535.

Exit gate: fixtures produce exact expected record ownership/time state and preserve every physical line. All record/time consumers use the shared classification result. Unknown state remains independent of whether a highlight span fits the compact representation.

Performance effect: bounded state updates per line; tiny rank/status indexes; possible savings from skipping date conversion on continuations and avoiding false timestamp removal. Masking and Drain still process physical lines, so sparse timestamps do not imply proportional end-to-end speedup.

**Phase 3 — Resolve profiles and dates predictably**

Work:

1. Give explicit per-file profile/date choices precedence. Auto considers known built-in shapes and user presets intentionally enabled for discovery, not every legacy regex unconditionally.
2. Replace the 25% physical-line hit threshold with independent anchored-header evidence and conflicts. A consistent pair of genuine headers can identify a sparse multiline file. A lone header has different confidence in a tiny complete file than in a large partially sampled file.
3. Start with a small leading sample. Expand only when unresolved, using line offsets for distributed contiguous windows. Initial total budget: 8,192 sampled lines and 2 MiB inspected bytes; limit candidate count, proposed 64 enabled profiles, with explicit selection available beyond that discovery limit.
4. Resolve date shape only within the template's time slot. Detect numeric-date ambiguity and overlapping recognizers, including 12-hour versus 24-hour forms. Prefer complete valid token consumption; never choose a shorter valid timestamp prefix that leaves AM/PM in the message.
5. Report profile, date parser, timezone/year assumptions, header counts, conflicts, and sample limits. Maintain a bounded sample of likely missed headers for troubleshooting. Auto must not infer an arbitrary payload prefix just because a date repeats there.
6. Lock the profile/date revision per document generation. A file that becomes recognizable during tailing needs an atomic explicit re-detection/reparse path; append must not silently reinterpret only its new suffix.
7. Cache compiled profiles by revision. Share discovery/preview machinery with headless loading and GUI so identical settings produce identical results.

Files: proposed `record/detect.rs`, existing `format/mod.rs`, `time/mod.rs`, document loading options, profile settings storage.

Exit gate: sparse headers and long preambles behave predictably; ambiguous cases are visible; resource budgets and cancellation are enforced; explicit selection works even when Auto has insufficient evidence.

Performance effect: extra bounded startup work in difficult files. Explicit selection skips competition. Candidate dispatch should use cheap literal/typed-prefix discrimination; never multiply full-file parsing cost by the number of saved profiles.

**Phase 4 — Independent time lookup and source-ordered queries**

Work:

1. Add a focused query service, proposed `src/core/time_query.rs`, with nearest-record-time lookup and source-ordered line/time selections. GUI timeline coordinates are not an input to this service.
2. Implement an exact cancellable scan first. Track monotonicity among valid record headers; unknown-time holes invalidate binary search over the raw forward-filled array. Add a compact valid-header index only where the measured repeated-query cost warrants it.
3. For disordered files, optional lazy time indexing stores valid header line IDs sorted by `(time, line)`, reading times from the document. It never supplies GUI x positions. Building it is worker-only, memory-budgeted, and optional. A filtered time selection still returns source order; retrieving time-sorted IDs incurs an additional source-order merge/sort or a source scan.
4. Define inclusive time predicates and preserve continuations through their owner record time. Apply line and time constraints together for mixed bounds. Exclude unknown times only when a time predicate is present. Out-of-range time requests return empty selections rather than clamped unrelated lines.
5. Replace `resolve_bound`/`resolve_range` assumptions in all MCP readers: raw log, log sequence, histograms, summaries, samples where applicable, and anomalies. Apply selection before pagination, counts, truncation, and collapse. Represent discontiguous results explicitly and add line-number mapping alongside raw text arrays.
6. Make Go to Time select the closest valid header, tie-breaking by earliest source line. For GUI time trim, select the contiguous envelope of matches and report included nonmatching intervening lines. An empty match leaves the view unchanged and reports no match.
7. Add explicit histogram domain/count-unit metadata. Preserve the established MCP omitted-domain behavior during compatibility migration; new GUI-equivalent requests use `domain: line`. A time histogram is rate/distribution analysis, not a source-order navigator.
8. Compute signed clock deltas between valid record headers with disclosed unknown intervals. Thresholds affect marker prominence, never timestamp values or membership. Retain first/last source time separately from numerical min/max.

Tests: multiple disjoint time windows, duplicate timestamps, unknown-time headers in the middle, backward jumps, extreme requested times, mixed bounds, trimmed source offsets, paginated output, filter unions, and collapse across skipped lines.

Exit gate: a straightforward source scan and every optimized query agree on fixtures; Go to Time works without the timeline; old array/tuple contracts are preserved where possible and additive fields/tool descriptions are explicit.

Performance effect: correct disordered time queries may cost a linear scan instead of today's invalid logarithmic lookup. A lazy header index trades memory and initial sorting for repeated-query speed. Do not advertise that correctness fix as a universal speedup.

**Phase 5 — Source-line timeline and record overview**

Work:

1. Make the GUI timeline domain explicitly source-line based for every file. Centralize original-source, trim-relative, and displayed one-based conversion helpers; audit each caller rather than replacing only `Timeline::build`.
2. Remove eager timestamp-sorted density and lane indexes from the primary GUI path. Reuse existing source-sorted match IDs, preferably sharing the immutable vectors instead of duplicating them. If the time-domain implementation remains for tests/analysis, it must be called explicitly and cannot restore automatic GUI switching.
3. Resolve record-start overview bins with the rank directory from Phase 2. Keep matching physical-line counts in filter lanes and label both units. Empty/unresolved files use neutral coverage; a single-line-record file can legitimately have a flat overview.
4. Recompute exact overview/minimap counts from rank queries. Avoid accumulating approximation error by repeatedly reprojecting old density bucket centers after append. Full-range resolution should depend on screen/bin count, not visible source-line count.
5. Route selection markers, pins, lane hit-testing, nearest-match navigation, zoom/pan, brush selection, current-range filter scanning, range export, trim, and selection recentering through line coordinates. Hidden matches do not compress the source axis.
6. Display actual line labels with optional record-time annotations. Never interpolate clock values across evenly spaced line ticks. Resolve time/provenance only for the small set of visible labels/hover targets. Aggregate anomaly markers when many fall into one pixel.
7. Version saved zoom state and convert old time zooms with Phase 4's exact selection/envelope rules. Validate domains and clamp line bounds. No matching records means reset only the zoom, preserving notes/filters under current identity rules.
8. Invalidate viewport caches by document/profile generation, line window, width, trim, and lane revisions. Cache formatting where useful; do not launch full-file work in the paint path.

Files: `src/core/timeline.rs`, `src/ui/timeline/view.rs`, `src/ui/app/{model,tab_model,view}.rs`, `src/ui/log_view/view.rs`, and `src/core/sidecar/`.

Exit gate: for any two visible physical lines `a < b`, their source x coordinates satisfy `x(a) < x(b)` regardless of timestamps; pixel aggregation does not violate underlying order. All range actions select the same source interval. Go to Time and old saved investigations remain usable.

Performance effect: removes timestamp sorting on disordered files and timestamp dereferences during lane searches. Record overview adds rank queries but keeps work bounded by viewport width. Sharing lane indexes can reduce memory further; that saving is separate from removal of the density fallback.

**Phase 6 — Editor, presets, migration, and reparse**

Work:

1. Provide a Log format editor with template text, timestamp-format choice, field definitions, representative multiline sample, extracted-field preview, and record-boundary/time-status preview. Label it separately from the mined Templates browser.
2. Support Use for this file, Save preset, and Apply to open file. Use a debounced, cancellable preview worker, proposed 200 ms debounce and 200 lines / 256 KiB preview cap. Show truncation and sample limits; do not compile regex on every paint or rescan the open file on each keystroke.
3. Persist versioned presets and per-file chosen profile/revision. Import legacy date-only definitions safely, preserving their source file. Show a compatibility state for definitions relying on an arbitrary prefix; explicit advanced legacy mode is outside Auto.
4. Snapshot investigation state by source identity and original-line anchors before reparse. Build the replacement document in a worker. Keep the current view responsive and install parser indexes, timeline, filter results, and profile diagnostics as one coherent generation.
5. Preserve line-based notes/pins, trim, scroll, and text/regex filters for unchanged sources. Revalidate Template-ID filters/find state against changed mining output; do not silently bind an old ID to an unrelated new cluster.
6. Cancel/failed parse leaves the current document intact. Dispose of stale worker results by generation checks; allow superseded previews/reparse work to stop. Reuse the same mmap and immutable offsets for unchanged source data where lifecycle rules permit; rebuild derived parsing/mining state.
7. Make headless load options and attached GUI reporting use the same profile ID/revision and diagnostics. Define behavior for missing presets: retain an embedded profile snapshot or report the missing profile explicitly, never silently fall back to a different parse.
8. Honor Escape and outside-click dismissal according to the project's popup/editor rules. Persist intentional edits only through the defined save/apply actions.

Exit gate: users can paste either proposed layout, verify a multiline sample, and apply it without losing unrelated investigation state. GUI and headless parsing agree; invalid templates cannot start a full reparse.

Performance effect: compilation is infrequent; preview is bounded. Applying a profile requires a full analysis/mining pass and may briefly retain old and new derived state. This peak must be budgeted explicitly; atomic replacement is not free.

**Phase 7 — Live append and partial-line transactions**

Work:

1. Carry profile revision, active record owner/time status, date assumptions, diagnostic aggregates, and rank-directory state across append batches. Process new complete lines and at most one amended final physical line. Do not scan the preceding giant multiline record to recover ownership.
2. Fix the existing partial-line hazard: `append_new_data` adjusts offsets but currently calls analysis from `old_line_count`, so merely adding newline/bytes to the old last line does not reanalyze that row correctly. Return `first_changed_line` separately from added-line count; appended bytes can change a row even when the count stays constant.
3. Make the final unterminated line a reversible analysis transaction. Retain a compact undo journal for its touched Drain cluster/template/path and counters, plus preceding timestamp/record/aggregate state. On extension, undo that one line, restore its indexes, and analyze its updated bytes. Once stable and followed by a next line, discard that journal and track the new tail. Do not rely on decrementing a cluster count alone: Drain merging also mutates token patterns and tree state.
4. Prototype that journal behind tests before integrating it. If exact bounded rollback proves too invasive, use a documented provisional-tail overlay built from committed state, and measure its copying cost; do not silently approximate template state. The release gate is equivalence with a fresh full load, including a final line without newline.
5. Recompute tail contributions to time bounds, invalid/reversal counts, record ranks, exact spans, and field metadata. Completing an invalid partial timestamp can change its status and the active record owner. Retained diagnostic samples must not duplicate the replayed row.
6. Update searches from `first_changed_line`, removing the prior version of the changed row before adding its new match. Invalidate viewport/embedded-data results overlapping that row or the open record. Install document and matches coherently.
7. Remove timestamp disorder as a reason to rebuild the GUI timeline. For density, use exact rank queries. For filter matches, preserve the current contiguous source-sorted vectors initially and measure their copying/rebucketing cost. If append gates fail, move them to shared chunks with a random-access search abstraction; do not describe append as fully proportional to new lines while old hits are still copied.
8. Include existing worker clone costs in measurements: `LogDocument::clone` detaches Drain/cache state even when line indexes share chunks. Profile this separately before considering structural sharing of mining state, which is a broader optimization.
9. Validate profile reparse racing with append, changed/shrunk file handling, closed-tab cancellation, and multiple tabs. Preserve cross-platform mmap/file-lock behavior and old-view/new-view lifetime safety.

Exit gate: the same final bytes loaded once versus appended at every tested byte boundary produce identical lines, ownership, times, spans, and mining output. Long-running tailing does not drift in counts or accumulate old generations. UI frame cost remains bounded while workers update.

Performance effect: normal parsing work is proportional to appended bytes plus one partial row, but total tail latency also includes existing mining/cache cloning and old match-vector processing until separately optimized. Atomic updates temporarily hold shared old/new snapshots.

**Phase 8 — Tune, validate, and release**

Work:

1. Run required `cargo test`, GUI/MCP-only checks, release tests, generator tests, and fixture contracts. Follow existing CI on macOS, Windows, and Linux; include Intel/Apple Silicon build coverage where available. No platform gets an untested alternate parser implementation.
2. Run the performance matrix below in release mode, one timed process at a time. Compare equivalent record semantics, filter hit counts, and outputs. Record both wins and regressions; a parser that skips work incorrectly is not faster in a useful sense.
3. Profile only failed budgets, then optimize the measured cause. Likely candidates: custom matcher allocation, repeated JSON decoding, record-count rank lookup, unnecessary lane copies, and Drain/cache clone size. Avoid speculative rewrites of all storage or the regex engine.
4. Exercise the complete user flow: open Auto file, choose custom layout/date, preview continuations, apply with existing pins/filters, navigate equal/backward times, select/export a timeline range, close/reopen a saved investigation, and append new data during a query.
5. Test legacy custom-date files, old time zooms, missing/edited presets, cancelled reparses, and older MCP callers. Preserve recoverable originals and make behavior changes visible in documentation.
6. Update `UserGuide.md`, `AI_ASSISTANT_DETAIL.md`, MCP tool schemas/descriptions and docs, and top-of-`changes.md` entries. Include time-vs-line histogram units and the difference between log format templates and mined message templates.
7. Publish a concise before/after performance report with source revision, workload fingerprints, hardware/OS/compiler when available, run count, feature set, medians/ranges, peak memory, and remaining limits. No predicted percentage is relabeled as measured.

Exit gate: all correctness gates pass, performance budgets either pass or have an explicit evidenced design adjustment, and the default GUI never changes source positions according to event time.

**Performance model — what changes and what does not**

Let `N` be physical lines, `R` record starts, `V` valid-time record starts, `H` total filter hits across lanes (including duplicates across filters), `B` overview bins, and `P` rendered columns per lane. Let `L` be the bounded inspected text-header size and `K` the capped template field count. File byte scanning, masking, and Drain costs remain workload-dependent and are not replaced by header matching.

| Operation | Current cost/behavior | Planned cost/behavior | Expected effect |
| --- | --- | --- | --- |
| Compile/select explicit layout | Custom regex compilation and sampled competition | One compiled profile/date choice per revision | Small setup cost; explicit selection avoids competition |
| Auto detection | Family/format votes over up to 512 sampled lines | Budgeted samples and candidate profiles with ambiguity checks | Can be slower on sparse/ambiguous files; cost is bounded independently of full-file length |
| Text header/date parsing | Selected recognizer searches a short prefix on each line | Typed anchored matching, bounded by `K` and `L`; conversion only at the time slot | Default may be similar/faster; detailed layouts can be slower; measure |
| Continuations | Timestamp search, then mask/mine | Cheap header rejection, inherit record state, then mask/mine | Saves some timestamp work; full processing still touches content |
| JSON normalization | Full object decode and source-span search | Reuse one decode for classification/time/normalization | Avoid a regression from double decoding; no assumed JSON speedup |
| GUI timeline construction | `O(N + H)` normally; disordered case adds sorting up to `O(N log N + sum(h_i log h_i))` | Overview from rank queries plus line-sorted hit handling, `O(B + H)` for initial buckets after document indexing | Removes time-order sorting; document load still processes `N` lines |
| Overview resolution | Time mode uses per-bin binary searches over timestamp indexes | At most two bounded rank operations per bin, `O(B)` | Predictable at any zoom level; no visible-range scan |
| Filter lane resolution | Roughly `O(P log h_i)` per lane with timestamp dereferences | Same lookup order, comparing line IDs directly | Smaller constant cost; sharing vectors can reduce memory |
| Go to Line | Timeline-domain lookup | Direct clamp/conversion, `O(1)` | Independent of timestamp quality |
| Go to Time / time query | Some queries assume incorrect monotonicity | Correct scan; optionally indexed lookup after proving/building suitable index | Can cost more initially; correctness requires it |
| Live append | New-line scan plus mining/cache clone, old hit copies/rebucketing, possible disordered full timeline rebuild | New-line/tail parsing; no disorder-triggered GUI sort; old-hit/mining costs remain until optimized | Better worst cases, but not automatically `O(new lines)` end to end |
| Apply different profile | Reopen/full analysis | Worker full reanalysis with old view retained | Similar full-pass compute plus explicit temporary memory peak |

Timestamp parsing is only one fraction of loading. Illustrative arithmetic, not a measurement: if header/date work is 20% of load time and a detailed template doubles that portion, total load increases about 20%, not 100%. If that portion is halved, total load improves about 10%. Phase 0 instrumentation establishes the real fraction for each corpus.

**Retained memory budget**

The current main per-line arrays hold approximately `24.125 * N` bytes of logical payload: 8-byte offset, 8-byte time, 4-byte timestamp span, 4-byte Drain ID, and one record-start bit. This excludes EOF/rounding, vector capacity, chunk metadata, mmap residency, Drain structures/cache, filter/visible indexes, and GUI resources. It is not a process-RSS prediction.

| Item | Planned payload | At 1 million lines | At 10 million lines |
| --- | --- | --- | --- |
| Existing five indexes | 24.125 bytes/line | 23.01 MiB | 230.07 MiB |
| Packed time state, if retained at two bits/line | 0.25 bytes/line | 0.24 MiB | 2.38 MiB |
| Record rank directory, one `u32` per 512 lines | About 0.0078125 bytes/line | 7.6 KiB | 76.3 KiB |
| Proposed main-index subtotal | About 24.3828125 bytes/line | 23.25 MiB | 232.53 MiB |
| Removed GUI disorder-density line IDs, when currently present | Saves up to 4 bytes/line with known time | Up to 3.81 MiB saved | Up to 38.15 MiB saved |
| Optional lazy time header index | 4 bytes/valid header ID | Up to 3.81 MiB when every line is a valid header | Up to 38.15 MiB at the same density |
| Optional sharing of timeline/filter ID vectors | Saves 4 bytes per duplicated hit | Depends on `H`, not just file size | Depends on `H`, not just file size |

Rank counters use `u32` consistently with the existing maximum supported line count and need checked arithmetic at bounds. Sparse overflow spans, diagnostics, compiled profiles, and caches have separate bounded budgets. Avoid a new `String` or `HashMap` per record/line: their allocation cost can dwarf these compact indexes. Extract nonessential field values on demand from source spans in bounded workers.

Atomic reparse can approach two copies of derived parse/mining state while the old view is retained. Offsets/mmap should be shared for an unchanged source, but new timestamps/spans/IDs/statuses and the new Drain/cache still coexist with old ones. Query workers retaining old generations can extend that peak; cancellation and prompt result disposal are part of the memory design. Do not promise that peak RSS will equal the retained-index table.

**Benchmark matrix and proposed performance gates**

Use deterministic 100K, 1M, and where memory permits 10M physical-line fixtures. Include small files to measure setup overhead and large files to expose memory/append scaling.

| Corpus | Purpose | Must report |
| --- | --- | --- |
| Timestamp-first single-line text | Default fast path | Load stages, match counts, timeline build/resolve, retained/peak memory |
| User's six-field template | Typed captures and separators | Parse-only ns/line, full load, compile latency, allocation counts |
| 10 and 100 continuations per header | Sparse timestamp discovery and record lookup | Detection confidence/time, ownership correctness, load, rank-query time |
| Payload containing many valid dates | False-positive resistance | Explicit timestamp count and record boundaries before interpreting speed |
| Equal timestamps / 0.1%, 1%, and heavy reversals | Sorting removal and time-query correctness | Timeline time/memory; first/repeated Go to Time and query latency |
| JSON with early/late time fields and large payloads | No duplicate decode or prefix-only assumption | Load bytes/sec, peak memory, correct field source/span |
| Long text headers and multi-MB messages | Bounded header work | Matcher byte counts, cancellation, full normalization cost separately |
| 0, 3, and 20 filter lanes with sparse/dense hits | Rendering and match-index costs | Resolution p50/p95, lane memory, append old-hit processing |
| Append 1, 100, and 10K lines to a 1M-line source | Incrementality | New-line analysis, clone/copy work, publication latency, peak memory |
| Append bytes to a final partial line | Correct replay | Full-load equivalence, undo/replay cost, same-count row update |
| Reparse a pinned/filtered large document | Replacement lifecycle | Time to apply, peak memory, old-generation release, UI responsiveness |

Method: perform one untimed warm-up and at least five measured sequential runs for release decisions; retain medians and min/max, and enough repetitions for meaningful p95 interaction metrics. Report OS-cache-warm runs as such. Do not claim disk-cold results without controlling/measuring cache state. Compilation and fixture generation are excluded from stage timings. Record a file digest when comparing preexisting local fixtures.

Initial engineering budgets, to calibrate on the measured baseline rather than treat as promises:

- Default-layout median full load: target no more than 5% regression on representative ordinary logs; investigate >10%. Correctness-changed fixtures need an equivalent semantic workload, not direct comparison with the old parser's missed records.
- Six-field layout: target at most 15% full-load overhead versus an equivalent specialized parser under the new record semantics. Also report parser-only cost so a masking-heavy workload cannot hide a slow matcher. This comparison does not exist until that baseline can parse the same fields correctly.
- Profile compile: target <20 ms for normal presets; debounced preview target <100 ms after dispatch for its bounded sample. These are UI targets on the eventual reference hardware, not global worst-case guarantees.
- Timeline CPU resolution: target p95 <4 ms for a 1,000-column overview plus 20 representative lanes; measure the full UI frame separately against a 16.7 ms budget for 60 Hz. No full-file scan/sort in paint or per scroll event.
- Added persistent main-index payload: target <=0.5 bytes per physical line beyond today's five arrays, before optional indexes and bounded sparse exceptions. Proposed state+rank uses about 0.258 bytes/line.
- Append: reject any unconditional scan/sort over old physical lines in the new record parser/timeline density path. Report old-hit copies and Drain/cache clone time separately; set absolute tail-latency budgets from the measured match density and target hardware in Phase 0.
- Resource-limited/adversarial input: predictable cap diagnostic, bounded worker memory, and prompt cancellation. A bounded header matcher does not by itself cap the existing masker/JSON decoder's whole-line work; measure and preserve their documented limits separately.

**Current baseline measurements**

Measured on 2026-09-11 using current commit `8e9d2391292f42279e4c247ef4f145c92a5281ce`, `rustc 1.97.1`, Cargo `1.97.1`, and `Darwin x86_64`. The hardware model was not captured. Built with `cargo build --release --locked --no-default-features --example bench --example profile_pipeline`; only the production-path `bench` executable was used for the figures below. No runtime source was modified.

For each corpus, run 0 was a warm-up and runs 1–3 were measured sequentially with `/usr/bin/time -l`. Stage timings come from `bench`; peak resident memory is the timing utility's maximum resident set size converted to MiB. The initial sandboxed timing attempt was excluded because the timing utility could not complete its system query; the reported runs completed outside the sandbox. These are preliminary three-run baselines, not the five-plus-run release qualification defined above.

| Corpus | Size / physical lines | Load median (range) | 3-filter scan median (range) | Timeline build median (range) | Timeline resolution median (range) | Peak RSS median (range) |
| --- | --- | --- | --- | --- | --- | --- |
| Existing synthetic generator | 64.0 MiB / 787,234 | 2.4 s (1.9–2.6 s) | 472.4 ms (415.5–496.1 ms) | 150.9 ms (140.0–199.6 ms) | 1.4 ms (1.4–1.5 ms) | 100.28 MiB (100.21–100.50 MiB) |
| Preexisting local `iOS-1M.log` | 123.2 MiB / 1,000,000 | 6.2 s (5.3–8.8 s) | About 1.1 s (936.5 ms–1.3 s) | 158.6 ms (149.4–177.4 ms) | 2.5 ms (2.4–2.8 ms) | 163.02 MiB (162.96–163.07 MiB) |

The tool labels binary MiB as MB in its output; this table uses MiB. Resolution measures 1,000 overview columns plus three lanes at 125 bins each; it is CPU aggregation, not complete GUI rendering or the proposed 20-lane release gate. Load includes indexing/mining/timestamps; it excludes the separately timed filter and timeline stages. The benchmark's overview/lane total-count assertions passed on every completed run.

Recorded measured runs, preserving the executable's timing precision:

| Corpus / run | Load | Scan | Timeline build | Resolve | Maximum resident bytes |
| --- | --- | --- | --- | --- | --- |
| Synthetic 1 | 1.9 s | 496.1 ms | 140.0 ms | 1.4 ms | 105,377,792 |
| Synthetic 2 | 2.4 s | 472.4 ms | 199.6 ms | 1.5 ms | 105,074,688 |
| Synthetic 3 | 2.6 s | 415.5 ms | 150.9 ms | 1.4 ms | 105,148,416 |
| iOS 1 | 8.8 s | 1.3 s | 177.4 ms | 2.8 ms | 170,938,368 |
| iOS 2 | 6.2 s | 1.1 s | 158.6 ms | 2.4 ms | 170,995,712 |
| iOS 3 | 5.3 s | 936.5 ms | 149.4 ms | 2.5 ms | 170,872,832 |

Filters were `ERROR`, `timeout`, and `user_id=42`. Synthetic hit counts were 268,825 / 98,404 / 98,404, with 33 mined templates. iOS hit counts were 48,547 / 819 / 498, with 146 mined templates. The local iOS fixture SHA-256 is `db6fb2e7144e99ae3e44f3119843710015ffa5baebe4e6f902799a9f8d20f713`; it was not regenerated, so the digest identifies this particular input rather than assuming a generator seed.

Reproduction commands after building (repeat sequentially, discard the first run of each corpus):

```sh
/usr/bin/time -l target/release/examples/bench
/usr/bin/time -l target/release/examples/bench iOS-1M.log
```

Interpretation and limits:

- The synthetic generator advances time-of-day modulo 24 hours while emitting a fixed date. At this size it crosses midnight and moves backward, exercising the current timestamp-sorting fallback. It is useful for this problem but is not a purely monotonic baseline. Phase 0 should add an explicitly monotonic counterpart with advancing dates rather than silently changing the old workload.
- Input generation is outside the executable's reported load timer, but inside `/usr/bin/time`'s process lifetime. Peak RSS therefore includes generation and all benchmark stages for the synthetic case; it is not isolated document-index memory. Existing file input is reused and cache state was not flushed. These are warm/repeated-process measurements, not cold-storage throughput.
- Run-to-run load variation is substantial, especially on iOS. The environment was not isolated. These numbers establish scale and justify measurement work; they are insufficient to resolve a 5% regression budget or predict a precise speedup.
- The current record/time annotations on these inputs have not been independently audited by this benchmark. Its count assertions validate timeline aggregation consistency, not correctness of timestamp extraction or multiline ownership.
- At the pre-implementation baseline, no six-field matcher, revised timeline, lazy time index, append workload, or atomic reparse implementation existed. Their effects were not measured in that baseline. The analytical memory budgets and target limits above are separate from those historical results; current measurements are in the Phase 8 report.
