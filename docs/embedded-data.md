# Embedded Data Architecture and Roadmap

## Current capability

Logotomy detects structured payloads embedded in the physical records
intersecting the Log View viewport. Built-in profiles cover valid JSON,
logfmt/key-value pairs, colon fields, Swift/Foundation descriptions, Python
literals, JVM/Android debug values, XML and labeled OpenStep property lists,
HTTP structures, protobuf text, stack traces, binary plist representations,
and encoded JWT/base64/hex/PEM values. Every candidate is bounded, cancellable,
and has exact original source coordinates.

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

Detections may also carry multiple exact source fragments inside their overall
span. Loose field groups use one fragment per field so Log View can leave prose
and spacing between fields undecorated while preserving one logical inspector
object.

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
lossy UTF-8 display. Explicit timestamp lines are retained in a compact bitset,
with a packed four-byte exact source span per physical line for allocation-free
timestamp decoration and hover lookup. Continuation lines retain only their
forward-filled timeline value and therefore receive no source annotation.
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

Support single or grouped whitespace/comma/semicolon separated fields, quoted
values, escapes, and `key=value`, `key = "value"`, or conservative
`key: value`. Double-, single-, and backtick-quoted values may contain spaces;
balanced and quoted values may span physical lines. URL schemes and filesystem
paths stay intact as scalar values. Single colon fields use stronger structural
confidence checks to avoid highlighting timestamps, URL schemes, source
locations, IPv6 addresses, and normal prose.

### Platform debug literals

Build one bounded collection parser with syntax profiles:

- Python: single quotes, `True`, `False`, `None`, tuples and `pprint` layout;
- Apple/Swift/Foundation: synthesized `Type(field: value)` descriptions,
  nested `Optional(...)`, enum associated values, labeled tuples, Swift
  dictionaries/arrays, multiline `dump`/Mirror trees, classic
  `{ key = value; }` collections, and `<NSObject: address; property = value>`;
- Java/Kotlin/Android: `{key=value}`, `Bundle[{...}]`, Intent extras, and
  constructor/data-class forms such as `User(id=1, name=Ada)`.

These formats are debug representations, not stable platform serialization
standards, so confidence and strict structural checks are essential.

### Property lists

XML `<plist>` values parse dictionaries, arrays, strings, integers, real
numbers, booleans, dates, data, UIDs, entities, comments, and CDATA into the
shared tree. OpenStep/ASCII `{ key = value; }` and `(item, item)` payloads use
the dedicated plist profile only when the surrounding field is explicitly
named `plist`, `propertyList`, or a recognized spelling; otherwise the same
ambiguous syntax remains a Foundation description. Literal, hex, and Base64
representations carrying the `bplist00` magic receive a metadata-only binary
profile. Transport decoding requires the explicit inspector action and does
not automatically interpret the binary object table.

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
