# MCP agent usability review

The MCP interface is useful for agents: summaries, histograms, template mining,
and bounded raw evidence let an investigation narrow a large log before spending
context on its contents. Explicit GUI attachment, a stable public catalog,
structured results with text fallback, and actionable tool errors are good foundations.

The largest correctness gap was search/filter disagreement with the GUI. The GUI
had advanced matchers, but MCP synchronized only filter labels and interpreted
them as case-sensitive phrases. Regex, template IDs, case modes, exclusion, and
Any/All composition could therefore produce different evidence for the agent.

## Implemented essentials

- Advertise `search` with text case modes, regex, and typed template IDs, using
  the same core matcher as GUI Find and filters. The previous `find_occurrences`
  name remains callable with its legacy arguments and response, but is omitted
  from discovery to avoid giving agents two competing search tools.
- Preserve complete filter specifications and Any/All composition in both
  synchronization directions. Removing a filter no longer shifts another
  filter's case/regex/template/exclusion settings in the GUI.
- GUI searches queue their exact scoped match list, update Find/highlighting,
  reveal hits hidden by lane visibility, and navigate to the first returned
  result. Stale requests are discarded on document replacement; synchronization
  waits for pending filter work. `sync_ui:false` supports exploratory calls.
- Include regex/template/case identity in the bounded match cache and invalidate
  composed results when filters or their join change.

## Implemented low-hanging fruit

- New `search` defaults to the whole current document, so it works before an
  agent adds filters. Older analysis tools keep their existing filtered default.
- Search returns `scope`, `next_offset`, and `ui_sync` alongside match counts.
- Reject blank queries, conflicting modes, invalid regexes and booleans,
  reversed time bounds, and zero-sized pages before changing GUI state.
- Exclude unknown timestamps from explicitly time-bounded searches.
- Mark search as potentially changing GUI state in MCP annotations, update
  discovery/help/examples, and poll while MCP is active so an idle GUI refreshes.

## Essential next work

| Priority | Improvement | Why it matters / acceptance criteria |
|---|---|---|
| High | Document and filter revision tokens | GUI tab switches, live append, trim, or filter edits between pages can invalidate offsets and evidence anchors. Return target/revision with every result and reject requests carrying a stale expected revision. Add original-file line anchors alongside trim-relative lines. |
| High | Cancellable scans outside the shared state lock | Current dispatch can hold the GUI's server mutex during a full scan. Snapshot inputs, compute outside the lock, then publish only if the revision still matches. Verify GUI responsiveness and cancellation on a large log. |
| High | Strict output schemas and consistent error codes | Current tools advertise a generic object output schema. Define success/no-scope/error variants and explicit recoverable codes; validate real responses against them. This reduces agent guesswork and makes client integration failures visible. |

## Further low-hanging fruit

| Improvement | Concrete outcome |
|---|---|
| Stable filter IDs and an update operation | Agents can edit/remove a filter without fetching the list again after every removal; avoid positional-ID drift. |
| Consistent scope defaults across analysis tools | Explicitly version the remaining tools' filtered-default behavior; distinguish an empty filter scope from an empty document rather than presenting both as “no log.” |
| Small optional search samples | Return a bounded text preview for a few hits, with truncation metadata, to save a separate `raw_log` call when deciding which result matters. |
| Session capabilities and examples | Report supported matcher modes and current search/filter state in `session_info`; keep two or three executable examples beside each schema. |

The current GUI handoff is a snapshot. Editing filters, lane visibility, or the
Find query, or changing the document, returns to normal GUI search behavior; it
does not preserve an MCP time window indefinitely. `ui_sync:"queued"` reports a
queued UI update, not an acknowledgement that a frame was painted. Pagination
requires the document and filter scope to remain unchanged.

Protocol reference: the official [MCP tools specification](https://modelcontextprotocol.io/specification/2025-11-25/server/tools)
describes tool annotations, output schemas, structured content with text fallback,
and `isError` tool-execution feedback. This review checks those contracts and the
repository implementation; it does not claim interoperability testing against
every MCP client.
