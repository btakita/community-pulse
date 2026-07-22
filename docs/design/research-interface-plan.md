# Agentic Research Interface Plan

Turn the pulse into a research substrate: external agents (Claude Code,
Codex CLI) investigate live trends through MCP, write findings back as
first-class data, and the app renders them in a Research surface.

Division of labor: **the pulse provides candidates, series, posts, and
URLs; the agents bring their own web-fetching, reasoning, and accounts.**
The app never holds an LLM API key for research — delegation goes through
the locally-installed harnesses, which carry the user's Claude / ChatGPT
subscription auth themselves. This supersedes the "keep the tool surface
at four" note in mcp-integration-plan.md — research adds tools
deliberately.

## Phase R1 — Research tools + write-back storage (~1 day)

New MCP/bridge tools (same ToolBridge pattern; chat gets them too):

- `list_topics({ window?, min_z? })` → full ranked long tail (id, display,
  z, trend, mentions, sources) — the un-budgeted research view. Pull is
  not budget-capped; only the digest is.
- `topic_posts({ id, window_hours?, limit? })` → posts with title, url,
  source, points, published_at. The agent reads the actual threads via its
  own web tools using these URLs.
- `get_series({ id, buckets?, bucket_hours? })` → raw counts + baseline
  mean/stddev, for agents that want to do their own math.
- `submit_research({ topic_id, agent, title, markdown, citations: [{url,
  note?}] })` → persists a report, returns its id. THE key tool: findings
  become data the UI renders.
- `list_research({ topic_id? })` → stored reports (id, agent, title,
  created_at, status).

Storage: `research_reports` table (id INTEGER PK, topic_id, agent TEXT,
title, markdown, citations JSON, created_at, status TEXT
'submitted'|'superseded'). New `research: Source<Vec<ResearchReport>>`
cell in PulseState; `submit_research` updates it → any open UI reacts.

Tests: bridge round-trip (submit → list → cell updated); MCP tools/list
now contains nine tools; markdown stored verbatim.

## Phase R2 — Research viewing UI (~1–2 days)

- **Research panel** on desktop: a drawer that slides over the chat panel
  (tab strip at top of the right column: AGENT | RESEARCH), listing
  reports grouped by topic — agent badge (claude/codex), title, age.
  Selecting one opens the report view.
- **Report view**: markdown-lite renderer (headings, bold, bullets,
  links, code spans — a small parser to Slint rich text; do NOT attempt
  full markdown). Citations render as a source list; every link uses the
  existing `open-url` outbound path.
- **Card integration**: cards with reports get a small research badge
  (count); the expanded evidence panel gains actions: "Research with
  Claude" / "Research with Codex" (Phase R3) and "View research (n)".
- **Comparative view**: when a topic has reports from both agents, a
  two-column Claude | Codex layout — same question, two models. This is
  the demo peak: two vendors' agents, one tool surface, side by side.
- **Current focus: desktop ↔ agent.** Build and verify the desktop loop
  first (drawer, report view, comparative view). Mobile parity below is
  DEFERRED to a follow-on pass — it stays cheap whenever it lands
  (shared process/cells), but do not spend time on the bottom-sheet
  integration or mobile acceptance until the desktop research loop is
  demo-ready.
- Mobile parity (deferred follow-on) — includes **side-by-side demo
  launch**: `app --companion` (exists) is the canonical way to start
  desktop + mobile connected (one process, shared bridge/cells). Parity
  acceptance adds i3/tiling-WM ergonomics, since the operator presents
  from i3 and a tiling WM will stretch or clip the fixed-size frameless
  phone frame:
  - Give the mobile window a **stable, documented WM identity** (window
    title "Community Pulse — Mobile" and, if the Slint/winit backend
    exposes it, a distinct `WM_CLASS` instance like `pulse-mobile`) so WM
    rules can match it. Never rename it casually — WM configs bind to it.
  - Document the i3 rule in the README demo section:
    `for_window [title="Community Pulse — Mobile"] floating enable` (or
    the WM_CLASS match if available). Desktop window stays tiled; phone
    floats at its natural size beside it.
  - Acceptance: under i3, `app --companion` yields the desktop window
    tiled + phone frame floating at correct geometry; rotation (Ctrl+R)
    keeps it floating at the new size.
  reports surface inside the evidence bottom sheet ("Research (2) ▸" →
  report view sized for the phone frame), research badges show on mobile
  digest cards, and a report submitted by an external agent while the
  mobile view is open appears live (same state cells — this is free in
  `--companion`/`--mobile` because they share the process; the R2
  acceptance test is: run `--companion`, submit via MCP curl, verify BOTH
  windows update).
- The demo beat this enables (zero extra code once R1+R2 land): phone
  frame open next to desktop, Claude Code session runs research → the
  report badge appears on the phone's evidence sheet mid-conversation.
  That IS "mobile ↔ desktop service ↔ agent" for the demo — the process
  boundary between mobile and desktop is a production concern, handled
  below as optional R5.

## Phase R3 — In-app delegation via installed CLIs (~1 day)

"Research this trend" from the UI, using the user's own accounts.

Why CLI delegation is the architecture (not just convenience): as of
Feb 2026, Anthropic restricts subscription OAuth to Claude Code and
claude.ai — a third-party app calling the API with subscription tokens
is a ToS violation, and third-party subscription billing moved to a
separate prepaid credit balance. OpenAI's "Sign in with ChatGPT" is
identity-only for third parties; subscription-billed inference outside
first-party surfaces is still in limited testing, while Codex CLI's
ChatGPT sign-in IS the sanctioned subscription path. Spawning the
installed `claude` / `codex` binaries keeps all inference inside
first-party surfaces (compliant), and gives the agents local-data access
(filesystem, local MCP) that no remote API call could have.

- Spawn pattern (no API keys in pulse; auth lives in each harness):
  - Claude: `claude -p "<prompt>" --permission-mode acceptEdits` headless
    run with the pulse MCP server registered (one-time:
    `claude mcp add --transport http pulse http://127.0.0.1:<port>/mcp`).
  - Codex: `codex exec "<prompt>"` with pulse registered in
    `~/.codex/config.toml` (via `mcp-remote` shim if the installed Codex
    lacks native streamable-HTTP MCP).
- Prompt template (checked into `docs/research-prompt.md`): "Use the
  pulse MCP tools: get_series + topic_posts for <topic>. Read the top
  source threads with your web tools. Explain what is actually happening,
  who is affected, whether the spike is organic (source diversity,
  velocity shape), and what to watch next. Submit via submit_research
  with citations. Keep it under 400 words."
- Process handling: spawn detached with a run row in a new
  `research_runs` state (topic, agent, status running|done|failed,
  started_at); flip status when `submit_research` arrives (correlate by
  topic+agent+recency — keep it simple) or on process exit without a
  submission (failed; keep stderr tail for the UI).
- UI: the evidence-panel buttons show per-agent status chips (spinner /
  ✓ / ✗). Both buttons can run concurrently — that IS the comparative
  demo.
- Preflight doctor: `pulse research doctor` checks `claude --version` /
  `codex --version` on PATH, MCP registration, and endpoint reachability;
  prints the fix commands. Run it in demo prep, not on stage.

## Web reports (Claude Artifacts + Codex local HTML)

Beyond the in-app markdown report, each run may produce a rich **web
report** (interactive charts, full-length analysis). The two harnesses
differ, so normalize:

- `submit_research` gains optional `web_report: string` — an `https://`
  URL or an absolute local file path.
- **Claude**: the research prompt asks the agent to also publish a
  self-contained HTML page as a Claude **Artifact** (private claude.ai
  URL, shareable later) and pass that URL in `web_report`. Verify
  artifact-tool availability in the headless `claude -p` run during R3
  rehearsal; if unavailable headlessly, fall back to the local-file path
  below (Claude writes HTML too).
- **Codex**: no CLI-accessible hosted equivalent (ChatGPT Canvas is
  chat-UI-only; ChatGPT Sites is Business/Enterprise-gated). The prompt
  asks Codex to write a self-contained HTML file into
  `research/reports/<topic>-<agent>-<n>.html` (repo-ignored dir) and pass
  the path.
- **UI**: report view shows "Open web report ↗" when present — the
  existing `open-url` handler covers both `https://` and `file://`.
- Guardrail: web_report accepts only `https://claude.ai/...` URLs or
  paths inside `research/reports/` — reject anything else (agents are
  semi-trusted writers here).

## Phase R4 — Live data loop (~half day)

- `pulse app --live [--ingest-interval 300]`: background ingest +
  recompute on an interval (existing ingesters; politeness: keep the
  20s timeout, back off on failures, min interval 120s to respect the
  free source APIs).
- Delta chips + tracked alerts now fire on genuinely new data — the
  research flow runs against reality.
- Demo-repeatability guard: after a good live session, snapshot the db
  (`cp community-pulse.db demo-live-snapshot.db`) so the exact state is
  re-loadable if demo-day networks misbehave; `--fixture` remains the
  deep fallback.

## Optional R5 — Remote mobile client (true process/device split, ~half day)

Only build if the story needs a real device boundary; the in-process
`--companion` demo above already shows the full loop. NOT an hour-scale
task — budget ~half a day:

1. Extract a `Bridge` trait from `ToolBridge`'s read/tool surface so the
   UI layer can run against local OR remote.
2. `pulse app --mobile --connect http://<host>:<port>/mcp`: a
   `RemoteBridge` that issues the same tool calls over the MCP HTTP
   endpoint and **polls** a new cheap `get_snapshot` tool every ~2s for
   state (digest hash, research count, tracked list). Polling is honest
   v1; SSE push is a later upgrade, not part of this.
3. LAN phone: bind `--mcp-host 0.0.0.0` behind an explicit flag with a
   loud warning (endpoint is unauthenticated — LAN demo only), then the
   same binary on a laptop — or eventually the Android build — connects
   over WiFi.

Why it's half a day and not an hour: the UI currently constructs a
concrete `ToolBridge`; the trait extraction touches app.rs wiring and
both window setups, and remote state needs the snapshot poll loop plus
staleness handling. No shortcuts that don't create demo risk.

## Test plan on live data (human-in-the-loop)

1. `pulse ingest` (real sources) → `pulse app --live --mcp-port 7432`.
2. Register MCP in both harnesses; run the doctor.
3. In Claude Code: "What's spiking in the pulse beyond my digest?
   Research the biggest anomaly and submit your findings." Verify:
   list_topics → topic_posts → external reads → submit_research → report
   appears in the Research panel without touching the app.
4. Same prompt in Codex; open comparative view.
5. From the UI: click both Research buttons on one card; watch status
   chips; open the two reports side by side.
6. Rehearse once on the presentation machine — subscription auth
   (claude login / codex login) is per-machine state no test can cover.

## Execution guide (for the implementing agent)

**Hour-scale slice (R1-lite), if asked for the fastest demoable loop:**
`submit_research` + `list_research` tools only (skip list_topics /
topic_posts / get_series for the moment — agents can already use the
existing four tools plus their own web access), the `research_reports`
table, the `research` state cell, and a minimal render: a "Research (n)"
line with report titles inside the desktop evidence panel (mobile
evidence sheet: deferred with the rest of mobile parity). That is ~5 small changes in the established patterns
(tools.rs, engine.rs, reactive.rs, app.rs snapshot, two slint touchpoints)
and immediately demos: Claude Code researches a trend → report title
appears in BOTH the desktop window and the phone frame. Full R1+R2
upgrade the same plumbing; nothing is thrown away.

Work R1 → R2 → R3 → R4 strictly; `make check` green at each phase
boundary before starting the next. R1 and R2 need no external accounts —
everything is testable with fixture data. R3 is where the human's
`claude`/`codex` logins matter; build it against the doctor + spawn
plumbing and leave live verification to the human rehearsal checklist.

Code anchors (current tree):

- New tools: methods on `ToolBridge` (src/tools.rs — struct at ~line 11),
  dispatch arms in `call()` (~153), schemas in `tool_definitions()`
  (~179). Follow the existing per-tool Args-struct pattern.
- MCP exposure comes free via src/mcp.rs (it lists whatever
  `tool_definitions()` returns) — update its tool-count test to 9.
- State: add `research: Source<Vec<ResearchReport>>` and
  `research_runs: Source<Vec<ResearchRun>>` to `PulseState`
  (src/reactive.rs ~20), snapshot fields, setters batched like
  `append_chat`.
- Storage: new tables in `PulseEngine::initialize` (src/engine.rs);
  follow the `subscriptions` table pattern.
- UI: research drawer in ui/app.slint right column (tab strip over the
  existing chat panel); mobile hook in the evidence bottom sheet.
- Spawn (R3): follow the `start_chat` thread pattern in src/app.rs;
  `std::process::Command` detached, working dir = repo root, inherit env
  (PATH must find `claude`/`codex`); do not kill children on app exit
  (report lands via MCP even if the window closes).

Pinned decisions (do not re-litigate during implementation):

- `ResearchReport { id: i64, topic_id: String, agent: String, title:
  String, markdown: String, citations: Vec<Citation{url, note:
  Option<String>}>, web_report: Option<String>, created_at, status }`.
- Run↔report correlation: on submit, mark the newest `running` run for
  (topic_id, agent) as `done`; if none, the report stands alone (CLI-
  initiated research is legal without a run row).
- Markdown-lite subset: `#`/`##` headings, `**bold**`, `- ` bullets,
  `[text](url)` links, `` `code` `` spans. Everything else renders as
  plain text — no tables, no images, no nesting.
- Per-phase acceptance: R1 = bridge round-trip test + 9-tool MCP list;
  R2 = fixture-seeded reports render, comparative view with 2 agents'
  reports, all links via open-url; R3 = doctor passes + spawn path
  produces a `running` chip and a fake-CLI test script (a stub shell
  script on PATH that calls the MCP endpoint) completes the loop without
  real accounts; R4 = live ingest interval honors the 120s floor and
  snapshot/restore works.

## Costs & guardrails

- Runs consume the user's Claude/ChatGPT subscription quota — the doctor
  prints a reminder; no auto-scheduled research (user-initiated only).
- MCP stays localhost-bound; `submit_research` sanitizes markdown length
  (cap ~16KB) and citation count (≤20); reports are agent-attributed and
  never auto-published anywhere.
- Live mode never bypasses the attention budget: research surfaces are
  pull; the digest stays capped.
