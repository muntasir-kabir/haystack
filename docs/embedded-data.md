# Embedded Data Architecture and Roadmap

## Current capability

Logotomy detects structured payloads embedded in the physical records
intersecting the Log View viewport. Built-in profiles cover valid JSON,
logfmt/key-value pairs, colon fields, Foundation descriptions, Python literals,
JVM/Android debug values, HTTP structures, protobuf text, stack traces, and
encoded JWT/base64/hex/PEM values. Every candidate is bounded, cancellable, and
has exact original source coordinates.

Detection is separate from document-format recognition:

- `core::format` chooses one envelope format for mining the log document.
- `core::embedded_data` finds zero or more payloads inside individual records.

The GUI waits 60 ms for scrolling to settle, analyzes on a cancellable worker,
rejects stale results, and performs only interval lookup/styling while painting.
Filtered rows are translated back to physical source intervals before analysis.

## Shared detector contract

All formats return the common model:

- original-file `SourceSpan` with line and byte coordinates;
- stable detector ID and root kind;
- bounded raw and pretty representations;
- normalized `DataNode` tree;
- source line count and summary.

Implement `DataDetector` in `detectors/<format>.rs`, register it in
`EmbeddedDataEngine`, add fixture cases, and add its matching Log View
presentation in `ui/log_view/embedded/highlighters/<format>.rs`. Candidate
discovery can use byte heuristics, but recursive boundaries require a bounded
lexer/state machine and successful syntax-specific validation. Detectors must
honor cancellation and all `AnalysisLimits`.

The normalized tree intentionally belongs to core and contains no egui types.
Most formats use the generic Tree/Pretty/Raw inspector. Stack traces default to
Frames/Raw, while encoded values start with a metadata Summary and require an
explicit Decode preview action.

## Source and record behavior

`LogDocument` exposes exact raw line bytes so source offsets never depend on
lossy UTF-8 display. Explicit timestamp lines are retained in a compact bitset.
For timestamped logs, viewport scans use the complete containing record; for
timeless or pre-timestamp text, they use bounded look-behind/look-ahead windows.

GUI scans currently allow at most:

- 8 MiB and 20,000 physical lines per interval;
- 128 nested containers;
- 20,000 normalized nodes;
- 64 detections per interval.

Candidates exceeding a bound are not claimed as valid JSON. Future work may add
an explicit incomplete-candidate cue and an opt-in larger analysis action.

## Built-in detector profiles

### Key/value and logfmt

Support whitespace/comma/semicolon separated pairs, quoted values, escapes, and
`key=value`, `key = "value"`, or conservative `key: value`. Require multiple
pairs, enclosure, indentation, or a payload-like prefix to avoid highlighting
timestamps, URLs, IPv6 addresses, and normal prose.

### Platform debug literals

Build one bounded collection parser with syntax profiles:

- Python: single quotes, `True`, `False`, `None`, tuples and `pprint` layout;
- Apple/Foundation: `{ key = value; }` and `(item, item)` descriptions;
- Java/Kotlin/Android: `{key=value}`, `Bundle[{...}]`, Intent extras, and
  constructor/data-class forms such as `User(id=1, name=Ada)`.

These formats are debug representations, not stable platform serialization
standards, so confidence and strict structural checks are essential.

### HTTP and protobuf text

HTTP recognizes request/response lines, repeated headers, query/form values, and
cookies. Protobuf text supports fields, nested message braces, and scalar
values. Keep every parser offline and resource-bounded.

### Stack traces and encoded values

Stack traces normalize exception messages and frame rows. JWT, Base64, hex, and
PEM show metadata and require an explicit preview action before decoding; JWT
signatures are never verified by this feature. Future work can add field/path
search, structural fingerprints, schema grouping, comparison, similar-payload
search, and MCP extraction without changing the detector/UI boundary.

## Test requirements for every detector

- Positive single-line and multiline fixtures.
- Exact original source spans, including CRLF and Unicode.
- Quotes, escapes, nesting, empty roots, and malformed/truncated inputs.
- False-positive fixtures from real log headers and prose.
- Limit and cancellation behavior.
- Overlap arbitration with already-supported formats.
- Filtered-view tests proving hidden physical continuation lines are used while
  nonadjacent displayed rows are never concatenated.
