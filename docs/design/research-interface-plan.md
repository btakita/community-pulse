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

## Implementation status (2026-07-22)

- [x] R1-lite and full R1: report storage/reactive write-back plus all five
research tools; MCP exposes exactly nine shared tools.
- [x] R2 desktop-first: Research drawer, report/comparison panes, markdown-lite,
citations/outbound links, raw-markdown copy, card counts, persistent resizable
pane width, and expanded-evidence entry.
- [x] R3: Claude/Codex evidence actions, concurrent run state, doctor, checked-in
prompt, web-report guardrails, and an account-free fake-CLI HTTP round trip.
- [x] R4: shared manual/live ingest controller, 120-second floor, failure
backoff, partial-source success, live state refresh, and tested SQLite snapshot
restore.
- [x] Follow-on queue: zero-ritual startup/setup/agent terminals, structured
verdict/summary/watch enrichment, and user-initiated article briefs with native
sections plus markdown fallback.
- [x] Demo polish: terminal pidfile/process-group lifecycle, agent-family run
reconciliation, markdown-lite fallback that normalizes all ATX heading levels,
and shared SVG disclosure/close/copy affordances.
- [ ] Needs human review: real subscription logins/quota, presentation-machine
doctor, live-provider timing, and final fallback assets. These are rehearsal,
not code blockers.
- [ ] Intentionally deferred: mobile research parity and its bundled companion/
i3 floating-rule pass; optional R5 remote client.

## Punch: report view renders plain text — implement markdown-lite (operator feedback)

**Status: implemented.** Topic reports and structured-section bodies now use
Slint `StyledText`; ATX headings `#` through `######` are normalized into the
supported markdown-lite subset so an unsupported `###` cannot force the entire
report back to literal source text.

The shipped report view shows raw markdown as plain text. Implement the
R2 renderer as specced: parse the subset — `#`/`##` headings, `**bold**`,
`- ` bullets, `[text](url)` links, `` `code` `` spans — into block-level
Slint elements (heading rows, bullet rows with markers, paragraphs).
Inline links: style them accent + underline; make the tappable target
work within Slint's inline limits — if true inline tap regions fight
back, render each paragraph's links as small link chips directly under
that paragraph (url via the existing open-url path) rather than
shipping untappable styled text. Everything outside the subset renders
as plain text — no tables/images/nesting. The structured-sections
native view (follow-on) supersedes this for article briefs; markdown-
lite remains the path for topic reports and the fallback for
unstructured submissions. Test: a fixture report exercising every
subset feature renders with correct hierarchy and all links open.

## Readability pass on report rendering (operator feedback, follow-up to the markdown punch)

Reports currently render as one continuous StyledText blob — bold works,
but there is no block spacing, so a full report reads as a wall of text
(operator screenshot: run-in bold headers, cramped bullets, zero
paragraph gaps). Move from "styled blob" to **block-level rendering**,
which the original punch specced:

1. **Parse to blocks**: split on blank lines; classify each block as
   heading (`#`–`######`, normalized), paragraph, bullet list (adjacent
   `- `/`·` lines), or code. Single newlines inside a paragraph collapse
   to spaces.
2. **Render blocks in a VerticalLayout with real spacing**:
   - paragraph gap 10px; before a heading 18px, after it 6px;
   - bullet items: marker column (a fixed 14px column with the dot) +
     hanging indent, 5px between items, 10px around the list;
   - inline code spans get an inset-background chip look; code blocks a
     bordered inset box.
3. **Type**: body 13px / line-height 1.55 ink-2; headings 14.5px / 650
   ink; bold runs stay ink (they read as labels — the agent's
   "What is happening." pattern works WITH spacing, not against it).
4. **Measure cap**: text max-width ~68ch inside the pane — with the
   panel now resizable, long lines must not stretch to arbitrary widths.
5. Applies to the single report view AND both comparison panes (shared
   block-list component + row templates). Structured-section briefs
   already render natively and are unaffected.
6. Test: fixture report with headings + multi-paragraph + bullets + code
   renders as N distinct blocks with expected spacing classes (assert on
   the parsed block list — pure function — not pixels).

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
- Claude: `claude --model opus -p "<prompt>" --permission-mode acceptEdits`
headless (Opus is the explicit default for delegated research)
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

## Research-driven UI enrichment (follow-on to R2/R3)

Reports must not be dead-ends behind a drawer — structured findings feed
the live surfaces. Pinned principle: **research annotates, it never
re-ranks.** The score math stays transparent (velocity/z/interest only);
research changes what the user SEES about a trend, not where it sits.
If an agent concludes a spike is manufactured, the user sees the flag
and decides — the formula is never silently overridden.

1. **Structured fields on `submit_research`** (all optional, so plain
   markdown reports keep working):
   - `verdict`: `"organic" | "manufactured" | "unclear"` — spike
     authenticity assessment.
   - `summary`: one sentence, ≤140 chars — the card-level insight.
   - `watch`: up to 3 suggested topic slugs ("what to watch next").
   Persist on `ResearchReport`; update the tool description and the R3
   prompt template so agents reliably fill them.
2. **Digest card research strip**: cards whose topic has research gain a
   compact strip under the meta row: agent badge (claude/codex) +
   verdict glyph (● organic / ⚠ manufactured / ? unclear) + the summary
   sentence in ink-2. Newest report wins the strip; the drawer holds
   history. Click → opens the report.
3. **Evidence panel**: verdict + summary shown prominently above the
   posts; with reports from both agents, show both verdicts side by
   side (agreement/disagreement is signal — and the comparative demo
   beat in miniature).
4. **Suggested channels with provenance**: `watch` topics flow into the
   composer's suggested chips, marked with a tiny agent glyph ("from
   research"). Clicking adds the channel as usual. Cap: research may
   contribute at most 2 of the visible suggested chips — trend-derived
   suggestions keep priority.
5. **Alert hook**: a `manufactured` verdict on a topic the user tracks
   fires the existing tracked-alert banner ("research flags <topic> as
   manufactured — tap to read"). Reuses the alert cell; no new
   notification machinery.
6. **Live update path is already free**: all of this renders from the
   `research` cell that `submit_research` writes, so a report landing
   mid-demo updates cards/evidence/chips in real time in every open
   window.
7. **Tests**: structured fields round-trip; card strip shows newest
   report; verdict-disagreement renders both; `watch` chips capped at 2;
   ranking unchanged by any research content (assert digest order is
   identical before/after submit_research — the annotates-never-reranks
   invariant as a test).

## Article-level briefs (deep-dive per post)

Topic research answers "why is this trending?"; article briefs answer
"what does THIS post actually say, and what is the community making of
it?" — the agent reads one article + its comment thread and produces a
rich brief, ideally as an artifact.

1. **Scope on `submit_research`**: optional `article_url: string`. When
   present, the report is an **article brief** anchored to the matching
   evidence post (match by url against the topic's posts; unmatched →
   store as topic-level with a warning in the tool result). Article
   briefs NEVER feed the card verdict strip or suggested chips — those
   stay topic-scoped; a brief annotates its post row only.
2. **UI entry**: each evidence post row gains a small "brief" action
   (agent picker like the topic-level Research buttons). After
   submission the row shows a brief badge (agent glyph + 📄); click →
   report view, and "Open web report ↗" when an artifact/HTML exists.
3. **Prompt template** (`docs/article-brief-prompt.md`): read the
   article at <url> AND its discussion thread (HN/lobste.rs comments —
   the url is the thread for self posts; fetch both when distinct).
   Produce: (a) what it announces/argues in plain words, (b) the
   technical substance worth knowing, (c) the community's reaction —
   top substantive threads, notable disagreements, (d) credibility
   notes (author, prior art, marketing tells), (e) why it's driving the
   trend. In-app: ≤140-char `summary` + the markdown body. Rich
   version: **Claude publishes it as an Artifact** (self-contained HTML
   brief — charts/pull-quotes welcome) and passes the URL in
   `web_report`; **Codex writes the HTML file** into `research/reports/`
   per the web-report rules. Existing web_report guardrails apply
   unchanged.
   **Direct links are mandatory, not decorative**: every claim and
   pull-quote in the brief links to its exact source location — section
   anchors within the article when the page has heading ids
   (`url#anchor`), and per-comment permalinks for reaction claims (HN:
   `item?id=<comment-id>`; lobste.rs comment anchors). The `citations`
   array carries these deep URLs (not just the article root), so the
   in-app citation list and the artifact both route the reader to the
   precise paragraph/comment. Template instructs the agent: "no
   unlinked claims — if you can't link it, mark it as your inference."
4. **Demo beat this creates**: expand a card → click "brief" on the HN
   post → agent reads the thread live → badge appears on the row →
   open the artifact in the browser. The artifact is shareable
   afterward — a natural "and here's the one it wrote earlier" backup
   asset for the call (pre-generate one during rehearsal).
5. **Costs**: article briefs are the most token-hungry research unit
   (full thread reads) — user-initiated only, same as all delegation;
   one brief per click, no batch "brief everything" button.
6. **Native in-app brief view (structured sections, not embedded HTML)**.
   Slint has no webview and must not grow one; instead the agent submits
   structure and the app renders native components:
   - `submit_research` optional `sections: [{ kind: "what" | "substance"
     | "reaction" | "credibility" | "watch", body, quotes: [{ text, url,
     author? }] }]` mirroring the template's five sections. When
     `sections` is present the in-app view renders natively; the
     markdown body remains the fallback for unstructured reports.
   - Native rendering: section headers as mono eyebrows; `quotes` as
     styled pull-quote blocks with an author chip and a deep-link glyph
     (tap → open-url to the exact comment/anchor); the reaction section
     renders quotes as mini comment-cards (author · source · link).
   - The evidence panel's brief badge opens THIS native view first;
     "Open web report ↗" remains the escalation to the rich artifact.
     (Same content, two fidelities: native = fast in-demo reading,
     artifact = shareable rich version.)
   - Prompt template addendum: always fill `sections` AND the markdown
     body (markdown is the durable/portable copy; sections are the UI
     copy). Quote text ≤280 chars each, ≤3 quotes per section.
   - Tests: sections round-trip; missing sections → markdown fallback
     renders; every quote url passes the citation guardrails.

## Live chat: API for the demo; CLI-backed chat as optional follow-on

**Demo decision (pinned):** the in-app chat panel runs on the existing
API path (`ChatSession::live`, BYOK via `PULSE_API_KEY` /
`PULSE_API_BASE` / `PULSE_MODEL`), NOT on local CLIs. Rationale: it
exists, streams token-by-token with the nine-tool loop, and per-turn CLI
startup latency reads badly in a conversational panel. Rehearsal items:
set the key in `.env`, verify the configured model id is current with
the provider, run one full tool-loop conversation live, confirm
`--replay` fallback still engages when the key is absent.

Demo narrative bonus: chat on a metered API key + research buttons on
subscription CLIs + the interactive Claude terminal over MCP = all
billing tiers of the brief visible in one demo, one tool surface.

**Optional follow-on — CLI-backed chat mode** (`app --chat-agent
claude|codex`, ~1 day, do NOT build before the panel):
- Each user turn → headless run (`claude -p --resume <session-id>
  --output-format stream-json`, or `codex exec` resume equivalent);
  parse stream-json for text deltas into the existing chat cell.
- Tool calls need no parsing: the CLI agent uses the app's own MCP
  endpoint, so tool chips derive from MCP-side call events and the UI
  updates via shared state like any external agent.
- Show a per-turn "thinking" state to absorb CLI startup latency; keep
  the API path as the default; subscription auth prerequisites are the
  same as R3 (doctor + warm-up).
- Value: in-app conversation billed to the user's subscription with no
  API key — tier 1 ergonomics inside the app. Product-relevant, not
  demo-relevant.

## Agent terminal lifecycle (bug) + in-app console direction

**Status: implemented.** The optional companion terminal is guarded by the
runtime pidfile, spawned in its own process group, and reaped with the app. The
research subprocess remains independent. Submitted reports reconcile to the
newest running job by topic and Claude/Codex agent family, so display names such
as `Claude (Opus 4.8)` cannot be misreported as a clean-exit failure.

**Bug (operator observed): each app instance spawns an extra external
terminal and they accumulate.** Fix the `--agent-terminal` lifecycle:

1. Reuse before spawn: write a pidfile (`$XDG_RUNTIME_DIR/pulse-agent-
   term.pid`); on launch, if the pid is alive, do NOT spawn another.
2. Reap on exit: the spawned terminal is a child of the app for
   lifecycle purposes — terminate it (SIGTERM the process group) when
   the app exits. (This intentionally differs from research delegation
   runs, which must survive app exit; the interactive terminal is a
   companion window, not a job.)
3. `--agent-terminal` stays opt-in per run — never sticky state.

**In-app terminal: do NOT embed a terminal emulator.** Claude Code's
interactive TUI needs full terminal emulation (PTY + escape rendering) —
days of work and a fidelity rabbit hole; X11 XEmbed tricks are fragile
and Wayland-hostile. The right version of "the terminal inside the app"
is the already-specced **CLI-backed chat mode** (`--chat-agent claude`):
headless stream-json runs rendered in the app's own chat pane — native
UI, no window management, subscription-billed. If in-app agent
conversation is wanted, build that follow-on; keep the real terminal
(on its own i3 workspace) for the demo beat where the audience should
SEE Claude Code being Claude Code.

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
  (`pulse --database community-pulse.db snapshot demo-live-snapshot.db`) so the exact state is
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
(topic_id, agent family) as `done`; display labels such as `Claude (Opus 4.8)`
match a `claude` run. If none, the report stands alone (CLI-
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

## Agent integration at startup (make the loop zero-ritual)

Goal: `pulse app` alone yields an agent-connected app. Three layers —
automatic, explicit-once, and optional:

1. **MCP on by default.** `app` listens on `127.0.0.1:7432` unless
   `--no-mcp` (keep `--mcp-port` as override). Port-bind failure (second
   instance) is a non-fatal stderr warning — the app still runs.
   Status bar gains a persistent `mcp ● :7432` indicator; when a
   tools/call arrives over MCP, flash it (and tag the resulting tool
   chip "via mcp") so the audience can SEE the external agent acting.
   Localhost-only stays hard-coded; that is the security boundary.
2. **`pulse setup` (explicit, idempotent, run once).**
   `pulse setup claude` shells out to
   `claude mcp add -s user --transport http pulse http://127.0.0.1:7432/mcp`
   (detect already-registered via `claude mcp list` and say so);
   `pulse setup codex` merges the `[mcp_servers.pulse]` entry (mcp-remote
   shim) into `~/.codex/config.toml`, refusing to touch a malformed file.
   `pulse setup` with no arg runs both + prints doctor-style status.
   Never run these implicitly at app startup — mutating user agent
   configs at launch is rude and surprising.
3. **`app --agent-terminal [claude|codex]` (demo convenience, optional).**
   Spawns `$TERMINAL -e <agent>` (default claude; fall back to
   i3-sensible-terminal) so one command brings up app + visible agent
   session side by side. May be passed twice to spawn both for the
   comparative demo. Document that each session still needs its login
   warm (that part cannot be automated). R3's in-app Research buttons
   remain the fully in-app agent path; this flag is for the
   terminal-visible demo.

Acceptance: fresh machine flow is `pulse setup` once, then
`pulse app --companion` forever after — agent-connected with zero
per-run ritual; `--no-mcp` verified; two-instance bind conflict is a
warning, not a crash.

## Costs & guardrails

- Runs consume the user's Claude/ChatGPT subscription quota — the doctor
  prints a reminder; no auto-scheduled research (user-initiated only).
- MCP stays localhost-bound; `submit_research` sanitizes markdown length
  (cap ~16KB) and citation count (≤20); reports are agent-attributed and
  never auto-published anywhere.
- Live mode never bypasses the attention budget: research surfaces are
  pull; the digest stays capped.
