# MCP Integration Plan (in-process, ~3–5h)

Expose the existing `ToolBridge` tools over MCP so external agents
(Claude Code, Codex, OpenCode) drive the same shared state as the UI and
chat. Success criterion = the demo moment: a Claude Code session calls
`subscribe_topic` and the running Slint window's tracked list updates
live, no pointer input.

## Architecture decision — read this first

**The MCP endpoint MUST run inside the `pulse app` process** (streamable
HTTP transport on localhost), NOT as a stdio server. Clients spawn stdio
servers as child processes; a spawned process would open its own SQLite +
its own `PulseState`, and the GUI would never reflect agent calls. The
whole point is one `ToolBridge` instance shared by UI callbacks, chat, and
MCP.

- Transport: MCP streamable HTTP, plain JSON responses (no SSE streams
  needed for a tools-only server). Protocol version `"2025-03-26"`.
- Scope: tools only (no resources/prompts), bind `127.0.0.1`, no auth,
  single-client assumption. All fine for a demo; say so in the doc header.

## Work items

### 1. `src/mcp.rs` — pure JSON-RPC handler (most of the work, ~200 lines)

A pure function keeps it testable without HTTP:

```rust
pub fn handle(bridge: &ToolBridge, request: &serde_json::Value) -> Option<serde_json::Value>
```

Methods:
- `initialize` → result: `{ "protocolVersion": "2025-03-26",
  "capabilities": { "tools": {} }, "serverInfo": { "name":
  "community-pulse", "version": env!("CARGO_PKG_VERSION") } }`
- `notifications/initialized` → no response (return None; HTTP layer
  answers 202).
- `tools/list` → map `ToolBridge::tool_definitions()` (tools.rs:124) into
  MCP shape: `{ "name", "description", "inputSchema" }` — the existing
  OpenAI-style `function.parameters` object IS the inputSchema; just
  re-nest it.
- `tools/call` → `bridge.call(name, &arguments.to_string())`;
  ok → `{ "content": [{ "type": "text", "text": result.to_string() }] }`;
  err → same shape plus `"isError": true`. Never a JSON-RPC error for tool
  failures (agents handle isError better).
- Unknown method → JSON-RPC error `-32601`.

### 2. HTTP layer

Add `axum` (workspace already has tokio). One route:
`POST /mcp` → parse JSON-RPC → `mcp::handle` → 200 JSON (or 202 empty for
notifications). `GET /mcp` → 405. That satisfies streamable-HTTP clients
that use plain JSON mode.

### 3. Wire into the app

- clap: `--mcp-port <u16>` on the `app` command (no default; absent =
  feature off).
- app.rs `run()`: if set, spawn a `std::thread` with its own
  current-thread tokio runtime running the axum server (same pattern as
  `start_chat`), holding `bridge.clone()` + `window.as_weak()`.
- After every successful `tools/call`, push the new snapshot to the UI:
  `render_later(&weak, bridge.snapshot())` (app.rs already has this
  helper and it is thread-safe via `invoke_from_event_loop`).
- Log one line to stderr on start: `mcp: listening on http://127.0.0.1:PORT/mcp`.

### 4. Client registration (document in README)

- Claude Code: `claude mcp add --transport http pulse http://127.0.0.1:7432/mcp`
- Codex / clients that are stdio-only: bridge with the `mcp-remote` npm
  shim (`npx mcp-remote http://127.0.0.1:7432/mcp`) — do not build a
  custom stdio proxy.
- OpenCode: http server entry in its MCP config.

### 5. Tests (tests/mcp.rs)

Pure-handler tests against a fixture bridge, no HTTP:
- `initialize` returns the protocol version + tools capability.
- `tools/list` contains exactly the four tool names.
- `tools/call get_pulse` returns content whose text parses as JSON with
  `count <= 5`, and afterwards `bridge.snapshot().digest.len() == count`
  — that last assertion IS the shared-state guarantee.
- `tools/call` with a bad tool name → `isError: true`.

### 6. Demo script addition

1. `pulse --fixture --replay app --mcp-port 7432`
2. In any terminal: `claude mcp add --transport http pulse http://127.0.0.1:7432/mcp`
3. In Claude Code: "Check the community pulse and start tracking wasm
   runtimes." → tracked list + status bar update in the visible window.

## Out of scope

SSE streaming, sessions/auth, resources/prompts, multi-client concurrency
beyond what `Arc<Mutex<PulseEngine>>` already gives, exposing
`refresh_scores` (keep the tool surface identical to chat's four).
