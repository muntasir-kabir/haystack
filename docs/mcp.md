# MCP integration guide

Logotomy gives local AI agents token-efficient access to logs through one MCP server configuration. Configure `logotomy --mcp` once. The same stdio server works in two runtime modes:

| Mode | How it starts | Log selection |
|---|---|---|
| `standalone` | Default | Agent calls `load_log` and retains its `log_id` |
| `gui_attached` | Agent calls `attach_gui_session` with the temporary ID copied from Logotomy | The GUI's selected tab; no `load_log` or `log_id` |

There is no HTTP MCP transport and no temporary MCP server to add to an agent. The GUI uses authenticated private local IPC internally, but agents always communicate with `logotomy --mcp` through stdio.

## Configure once

Replace `/absolute/path/to/logotomy` with the executable path shown by **Settings → Integrate with AI Assistant**.

### Codex CLI, desktop app, and VS Code extension

Add this to `~/.codex/config.toml`, or `.codex/config.toml` for project scope:

```toml
[mcp_servers.logotomy]
command = "/absolute/path/to/logotomy"
args = ["--mcp"]
startup_timeout_sec = 20
```

Reload Codex after changing its MCP configuration. See the [Codex MCP documentation](https://developers.openai.com/codex/mcp/).

### Claude Desktop

Open **Settings → Developer → Edit Config** and merge this entry into `mcpServers`:

```json
{
  "mcpServers": {
    "logotomy": {
      "command": "/absolute/path/to/logotomy",
      "args": ["--mcp"]
    }
  }
}
```

The file is normally `~/Library/Application Support/Claude/claude_desktop_config.json` on macOS, `%APPDATA%\Claude\claude_desktop_config.json` on Windows, or `~/.config/Claude/claude_desktop_config.json` on Linux.

### Claude Code CLI and VS Code extension

Run this once with user scope so Logotomy is available across projects:

```sh
claude mcp add --scope user logotomy -- /absolute/path/to/logotomy --mcp
claude mcp list
```

See the [Claude Code MCP documentation](https://docs.anthropic.com/en/docs/claude-code/mcp).

### Cline CLI, standalone app, and VS Code extension

Add this to `.cline/mcp.json` for project scope or `~/.cline/mcp.json` for user scope:

```json
{
  "mcpServers": {
    "logotomy": {
      "command": "/absolute/path/to/logotomy",
      "args": ["--mcp"],
      "disabled": false,
      "autoApprove": []
    }
  }
}
```

See [Cline MCP configuration](https://docs.cline.bot/mcp/configuring-mcp-servers).

## Use a log from the GUI

1. Configure `logotomy --mcp` once using the relevant client instructions above.
2. Open a log in Logotomy and select the tab to share.
3. Click **Start MCP**.
4. Click **Copy GUI session instruction** and paste it into the agent conversation.
5. The agent calls `attach_gui_session` with that temporary session ID, then calls `session_info` and confirms `mode: "gui_attached"`.
6. Click **Stop MCP** when finished. The ID immediately expires.

While attached:

- the selected GUI tab is already the target;
- do not call `load_log` and do not pass `log_id`;
- changing tabs changes the target available to the agent;
- `trim` and filter changes synchronize with the GUI;
- the GUI already supplies the open log, so `load_log`, `list_logs`, and `close_log` are unavailable and `log_id` is not needed;
- use the suggested exploration approach below rather than treating the available analysis tools as a fixed checklist;
- call `detach_gui_session` to return the same MCP process to standalone mode.

The copied instruction includes the selected log path for orientation, but the session ID is the only attachment capability. It must not be saved in an MCP config, file, skill, shell history, or agent response. If attachment fails, start MCP again and copy a new instruction.

## Use standalone mode

Without `attach_gui_session`, the same configured server stays in `standalone` mode:

1. Call `session_info`.
2. Call `load_log` with an absolute local path.
3. Retain the returned `log_id` and pass it to subsequent per-log tools.
4. Call `close_log` when the document is no longer needed.

Standalone documents remain loaded if the process temporarily attaches to and later detaches from a GUI session.

## Agent contract

Logotomy exposes a stable tool catalog in both modes, so clients may safely cache discovery. Runtime rules come from:

- `session_info` — authoritative mode, lifecycle, active-log, and filter state;
- `logotomy://session` — the same state for clients that consume MCP resources;
- `logotomy://guide` — compact investigation guidance.

Suggested investigation approach:

1. `session_info`
2. In GUI-attached mode, understand the user's question and Pin-tab findings with `get_analysis`
3. Explore the log shape with `summarize_log(with_filtered_log=false)` and targeted `find_occurrences`
4. Use filters (`filters_add`), anomalies, histograms, templates, sequences, and `trim` when they help test a hypothesis and narrow the scope
5. Request only bounded `raw_log` ranges when exact evidence is needed; add useful evidence-backed root-cause conclusions with `add_analysis`

All tool results include `structuredContent` plus JSON text fallback. Tool errors set `isError: true` and return `{error, message, retryable}`. An expired attachment is retryable and automatically restores standalone routing.

## Tools

| Tool | Runtime use | Purpose |
|---|---|---|
| `session_info` | both | Report mode, lifecycle, active log/filter state, and attachment status |
| `attach_gui_session` / `detach_gui_session` | stdio lifecycle | Attach with a temporary GUI ID or return to standalone mode |
| `load_log` | standalone | Index an absolute file path and return `log_id` plus stats |
| `list_logs` / `close_log` | standalone | Inspect or unload standalone documents |
| `filters_get` / `filters_add` / `filters_remove` | both | Read or change the case-sensitive keyword filter set (maximum 20) |
| `get_analysis` / `add_analysis` | GUI-attached | Read or add user-visible Pin-tab analysis cards (`{text, lines}`); empty `lines` creates a text-only top card |
| `summarize_log` | both | Compact orientation, templates, errors, gaps, densest minute, and budget estimates |
| `get_timeline_histogram` | both | Small time/line distribution for a log, keyword, or template |
| `get_template_anomalies` | both | Rare, late-first-seen, and bursty templates |
| `get_template` / `get_template_samples` | both | Resolve template IDs and fetch representative examples |
| `find_occurrences` | both | Paginated `[one_based_line_number, epoch_ms\|null]` matches; supports time bounds, filter scope, and ASCII case mode |
| `log_sequence` | both | Dense or collapsed template sequence over a line/time range |
| `raw_log` | both | Exact bounded raw lines with truncation metadata |
| `trim` | GUI-attached | Focus or reset the GUI document window |

Analysis tools accept `with_filtered_log`, which defaults to `true`. This means the union of keyword filters; the GUI's **Everything Else** lane is excluded. With no filters, pass `with_filtered_log: false` or add one with `filters_add`.

Line bounds are 1-based. `find_occurrences` accepts `offset`, `max_results`, `with_filtered_log`, and `case_sensitive` (default `true`; `false` folds ASCII case), returning `{occurrences, offset, returned, total_matches, has_more}` where every occurrence is `[line, epoch_ms|null]`. Time inputs accept RFC 3339, `YYYY-MM-DD HH:MM:SS`, `YYYY-MM-DD`, or epoch seconds/milliseconds. Respect `returned`, `total_matches`, and `truncated`; narrow the range instead of requesting large raw dumps.

## Security and lifecycle

- MCP clients launch only the configured stdio command.
- Each GUI start creates a cryptographically random 12-character hexadecimal (48-bit) session ID.
- The GUI publishes a private, atomic local session manifest; Unix permissions are user-only.
- The internal IPC socket binds only to loopback and rejects requests without the matching ID.
- The ID and manifest expire when MCP stops or Logotomy exits.
- Logotomy does not log or return the session ID.
- A cloud-only agent cannot launch a local stdio command; use its local CLI, desktop app, or VS Code extension.

`--mcp-gui` remains a deprecated compatibility alias for older configurations. New and updated integrations must use `--mcp` plus explicit `attach_gui_session`.

## Protocol compatibility

Logotomy supports discovery-based MCP `2026-07-28` and initialization-based revisions `2025-11-25`, `2025-06-18`, `2025-03-26`, and `2024-11-05` over stdio. Modern clients call `server/discover`; existing clients call `initialize`, then `tools/list`.

## Smoke test and troubleshooting

```sh
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"smoke","version":"1"}}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
  | logotomy --mcp
```

| Symptom | Resolution |
|---|---|
| `attach_gui_session` is unavailable | Update the configured executable and reload the agent client |
| Session ID is invalid, expired, or unavailable | Open a log, click **Start MCP**, and copy a new GUI session instruction |
| `load_log` is rejected | Call `session_info`; while `gui_attached`, analyze the selected tab without `log_id` |
| Analysis returns `{"comment":"no log"}` | Pass `with_filtered_log:false` or add a keyword filter |
| Large result is truncated | Narrow line/time bounds or paginate before calling `raw_log` |
| Agent cannot see Logotomy | Confirm it runs locally and its configured command points to the current executable |

When reporting a problem, include the client/surface, requested MCP version, method, mode from `session_info`, and error text. Redact the session ID and log contents.
