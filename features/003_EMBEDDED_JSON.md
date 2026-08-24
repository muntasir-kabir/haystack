# Embedded JSON in the Log View

## Goal

Detect JSON objects and arrays embedded in visible log records without parsing on
the UI thread or giving up the virtualized Log View. A detected value may start
after an arbitrary log prefix, span multiple physical lines, and extend beyond
the 2,000-byte row display limit.

## User-visible contract

- A detected root receives a compact `JSON` cue on its first visible line.
- Every visible source segment receives a subtle underline; multiline roots also
  receive a gutter rail.
- Hovering the cue shows a bounded summary.
- Clicking it opens a persistent Tree / Pretty / Raw inspector.
- The inspector can copy the exact raw source or formatted JSON.
- Incomplete values are never presented as valid JSON. Explicit incomplete
  candidate cues and opt-in larger rescans remain follow-up work.

## Detection contract

- Recognize JSON objects and arrays, including roots embedded after prefixes.
- Balance braces/brackets while respecting strings and escapes.
- Validate the complete candidate with `serde_json`; delimiter balance alone is
  insufficient.
- Prefer the outer valid root over nested candidates.
- Preserve exact original-line and byte-in-line source coordinates.
- Analyze contiguous physical source, never concatenated filtered rows.
- Bound bytes, lines, nesting, result count, and preview nodes.
- Never evaluate content or perform network/file access while parsing.

## Fixture contract

`tests/fixtures/embedded_json/cases.json` is the initial compatibility corpus.
Each case records whether JSON should be detected and, for positive cases, its
inclusive physical line extent. Detector tests must consume this corpus rather
than duplicating the examples.

## Future detector roadmap

The JSON implementation establishes a detector registry, common source spans,
normalized data tree, background scheduling, and generic inspector. Later
detectors should plug into those boundaries in this order:

1. logfmt and conservative `key=value` / `key: value` groups;
2. Python literal, Apple/Foundation collection, and Java/Kotlin/Android debug
   profiles;
3. YAML, TOML/INI/properties, XML/text plist, and HTTP structures;
4. CSV/TSV, protobuf text, JWT/Base64/hex, and schema/similarity indexing.

Recursive formats must use real parsers or bounded state machines. Regex may
locate candidates but must not be the recursive parser.
