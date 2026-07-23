# Community Pulse

Community Pulse turns noisy public community feeds into a user-budgeted digest.
It normalizes Hacker News, Lobsters, and Product Hunt; measures topic velocity
against a seven-day baseline; then reranks the shared trends with an explicit
interest mixer. The attention budget defaults to five, persists per user, and
has an engine-enforced ceiling of ten.

The desktop demo is built in Rust with [Slint](https://slint.dev/) and uses
[`lazily`](https://github.com/lazily-hub/lazily-rs) cells as the single source
of state for direct UI actions and agent tool calls.

![Community Pulse desktop demo](demo/community-pulse.png)

A short [offline backup recording](demo/community-pulse-demo.mp4) is checked in
for interview-room fallback.

## One-command offline demo

```bash
cargo run -- --fixture --replay app
```

This path needs no network. `--fixture` replaces the selected SQLite database
with a time-relative, deterministic 30-post snapshot. `--replay` drives the
core production actions through the same nine-tool bridge, including incremental
chat deltas and visible tool-call chips.

Add `--mobile` after `app` (or run `make demo-mobile`) to open only the 418×872
portrait phone frame, including a 390×844 app surface and live system-bar
chrome. Use the bezel rotate control or Ctrl+R to switch live to an 872×418
landscape frame; bottom tabs become a left rail and an open evidence sheet
becomes a right panel. The frameless mobile window uses your desktop's logical
DPI scaling so its custom phone chrome stays legible on HiDPI displays. Use
`--companion` (or `make demo-companion`) when you want synchronized desktop and
phone windows.

Run `make demo-shots` for deterministic Xvfb captures of the desktop,
expanded-evidence, mobile portrait, and mobile landscape states. The images are
written to `demo/shots/` for comparison with
`docs/design/mockup-desktop.html` and `docs/design/mockup-mobile.html`.

Pulse / Mix / Agent use the same bridge and row models. Fixture data drives
previous-snapshot delta chips plus a tracked-topic threshold alert. Digest
headlines and evidence rows route to their original community source in the
system browser.

Try these prompts:

- `What's the pulse today?`
- `More Rust, less crypto`
- `Why is WASM moving?`
- `Track WASM runtimes for me`

## Live desktop and mobile clients

The demo above is deterministic and offline. To drive the real clients against
live Hacker News, Lobsters, and Product Hunt data with live Claude Code and
Codex delegation, build once and launch with the checked-in helper scripts:

```bash
cargo build

./bin/desktop.sh              # desktop window
./bin/mobile.sh               # portrait phone frame
./bin/desktop-and-mobile.sh   # synchronized desktop + phone (companion)
```

Each script runs `app --mcp-port 7432 --live --ingest-interval 300`, so the
in-process MCP endpoint is live, public sources refresh every 300 seconds (never
below the 120-second floor), and no fixture data is loaded.
`desktop-and-mobile.sh` honors a `PULSE_BINARY` override (for example a
`target/release/pulse` build) and forwards any extra flags. The equivalent long
forms are:

```bash
cargo run -- app --live --ingest-interval 300
cargo run -- app --mobile --live --ingest-interval 300
cargo run -- app --companion --live --ingest-interval 300
```

### Wire in live Claude Code and Codex

The clients delegate to the installed `claude` and `codex` CLIs using each
harness's own account; Pulse stores no provider API keys on this path. Register
the running endpoint once and verify prerequisites:

```bash
cargo run -- setup                             # configure both; or setup claude / setup codex
cargo run -- research doctor --mcp-port 7432   # check binaries, MCP registration, reachability
```

Then, in any open desktop or phone window, expand a card and choose **Research
with Claude** or **Research with Codex**. Claude delegation defaults to
`--model opus` and pre-authorizes only the local `mcp__pulse__*` tools. Reports
stream back into card badges, the evidence panel, and the Research drawer; see
[Desktop research loop](#desktop-research-loop) for the full lifecycle. To keep
visible, interactive agent shells open alongside the app, add one or both
terminals:

```bash
cargo run -- app --live --agent-terminal=claude --agent-terminal=codex
```

Each `--agent-terminal` opens a Claude or Codex session in `$TERMINAL` that
mutates the exact `ToolBridge` shown in the desktop and phone windows.

## CLI-first data story

```bash
# Deterministic capped digest
cargo run -- --fixture top

# Machine-readable output and evidence
cargo run -- --fixture top --json
cargo run -- --fixture explain wasm-runtimes

# Live ingestion, then launch the UI
cargo run -- ingest
cargo run -- app

# Or keep the open desktop fresh (300s default; never below 120s)
cargo run -- app --live --ingest-interval 300
```

The core is a library with no agent dependency. The CLI deliberately proves
the ingestion → normalization → scoring → attention-budget story before any UI
or model is involved.

## Live OpenAI-compatible chat

Copy `.env.example` into your shell configuration (the app does not parse or
commit `.env` files):

```bash
export PULSE_API_BASE=https://api.openai.com/v1
export PULSE_API_KEY=...
export PULSE_MODEL=gpt-4.1-mini
cargo run -- --fixture app
```

`PULSE_API_BASE` can point at any compatible streaming chat-completions
endpoint. The client accumulates fragmented streaming tool arguments, executes
the tool, appends its compact JSON result, and continues the response for up to
four tool rounds. If no API key is present, the app clearly announces and uses
the deterministic replay agent.

## The shared tool bridge

| Tool | Effect |
| --- | --- |
| `get_pulse(limit?)` | Uses the stored attention budget unless a bounded limit is supplied. |
| `set_interests(add[], remove[], attention_budget?)` | Persists mixer/budget changes and reranks immediately. |
| `explain_trend(id)` | Opens velocity, baseline, z-score, sparkline, and source evidence. |
| `subscribe_topic(topic)` | Adds a durable tracked topic for the personal-alert bridge. |
| `list_topics(window?, min_z?)` | Returns the unbudgeted ranked research candidate set. |
| `topic_posts(id, window_hours?, limit?)` | Returns source posts and URLs for one topic. |
| `get_series(id, buckets?, bucket_hours?)` | Returns raw counts and baseline statistics. |
| `submit_research(topic_id, agent, title, markdown, citations[], …optional enrichment/article fields)` | Persists an attributed topic report or article brief and updates open UI state. |
| `list_research(topic_id?)` | Lists stored reports for the drawer and comparison view. |

Buttons, topic chips, replay chat, and live chat all invoke this bridge. Every
mutation updates `lazily::ThreadSafeContext` sources; the derived status line is
a `lazily` computed value, so agent and UI state cannot fork.

Repeated `+` clicks step a topic through 0.5× weights up to 2.0; `×` toggles a
hard mute. That makes the interest vector visible and directly editable instead
of hiding personalization behind the model. The distinct master fader sets a
3–10 attention budget in the UI; chat and MCP use the same persisted setting.

## External agent integration (MCP)

The MCP server runs inside the GUI process by default, so external agents mutate
the exact `ToolBridge` observed by desktop and mobile. It is a demo-scoped,
tools-only streamable-HTTP endpoint bound to `127.0.0.1:7432` with no authentication:

```bash
cargo run -- --fixture --replay app --mobile
```

Register it idempotently with Claude Code and Codex (or name one target):

```bash
cargo run -- setup
cargo run -- setup claude
cargo run -- setup codex
```

`setup` uses Claude's user-scoped HTTP registration and merges a `mcp-remote`
entry into Codex's TOML without replacing unrelated configuration. Use
`app --no-mcp` to disable the endpoint, `app --mcp-port <PORT>` to override it,
or repeat `app --agent-terminal[=claude|codex]` to open companion agent shells.
For a stdio-only client, use the standard remote bridge instead of spawning a
second Pulse process:

```bash
npx mcp-remote http://127.0.0.1:7432/mcp
```

Then ask the client to “set my attention budget to eight, check the community
pulse, and start tracking wasm runtimes.” The visible windows update after the
MCP tool calls. Policy clamps an out-of-range request such as 50 to 10 without
turning it into a tool error.

## Desktop research loop

Launch the desktop (its localhost MCP endpoint starts automatically), then run the preflight from
another terminal:

```bash
cargo run -- --fixture --replay app
cargo run -- research doctor --mcp-port 7432
```

Expand a card and choose **Research with Claude** or **Research with Codex**.
Pulse launches the installed CLI using that harness's own account (Claude
delegation explicitly defaults to `--model opus` and pre-authorizes only the
local `mcp__pulse__*` tools), records a running/done/failed state, and waits for
`submit_research`. An animated progress row in the evidence panel and Research
drawer shows the agent, live elapsed time, and latest harness activity; the
final duration remains on the completed/failed action. Reports appear live
in card badges, the evidence panel, and the desktop Research drawer. When both
agents report on one topic, the drawer renders the Claude | Codex comparison.
Delegation is headless rather than attached to an `--agent-terminal` companion
shell. The Agent chat posts the run state and log path; live stdout, stderr,
exit status, harness, model, and submission outcome are written under
`research/logs/`, so a running job can be inspected with `tail -f`.
Structured verdicts annotate cards without reranking and can contribute at most
two provenanced watch suggestions. Each evidence row can also request a
single-agent article brief. Submitted badges open native what/substance/reaction/
credibility/watch sections and deep-linked quote cards first, with complete
markdown as the fallback and an optional Claude Artifact or repo-local HTML
report as the rich escalation. Pulse does not store provider API keys for this path.

The fake-CLI integration test exercises the complete launch → HTTP MCP → submit
→ done transition without subscription accounts. Real provider login and quota
checks remain presentation-machine rehearsal work.

## Live ingest and snapshots

The desktop **ingest** action and `app --live` use the same controller. It
fetches each source independently, ingests partial success, recomputes the
digest, refreshes delta chips and tracked alerts, and shows per-source status.
All triggers share a 120-second politeness floor; scheduled failures back off.
Fixture mode disables ingest so deterministic demo data cannot be contaminated.

After a good live session, create a consistent standalone fallback database:

```bash
cargo run -- --database community-pulse.db snapshot demo-live-snapshot.db
cargo run -- --database demo-live-snapshot.db --replay app
```

The snapshot command refuses to overwrite an existing target.

## Scoring

For each deterministic extracted topic, the engine records mentions over
1-hour, 6-hour, and 24-hour windows. It compares the current six-hour bucket to
27 preceding six-hour buckets (roughly seven days):

```text
z = (mentions_6h - baseline_mean) / max(baseline_stddev, 1)
velocity = 4·mentions_1h + 0.8·mentions_6h + 0.15·mentions_24h
trend = velocity·(1 + 0.25·max(z, 0)) + 0.5·distinct_sources
final rank = trend × interest_affinity
```

A negative interest weight mutes the topic. Positive weights boost it. The
stored attention budget caps the default result; it defaults to five and can
never exceed ten regardless of feed or user scale.

## Project map

- `src/engine.rs` — SQLite schema, normalization, windows, baseline, scoring,
fixture, evidence, interests, subscriptions, and persisted settings
- `src/ingest.rs` — async adapters for HN Algolia, Lobsters JSON, and the
  Product Hunt Atom feed
- `src/reactive.rs` — `lazily` sources and derived UI status
- `src/tools.rs` — the nine shared UI/chat/MCP tools
- `src/mcp.rs` — in-process MCP JSON-RPC handler and localhost HTTP transport
- `src/research.rs` — CLI delegation, run lifecycle, prompt, and doctor checks
- `src/live.rs` — shared manual/live ingest controller, cooldown, and backoff
- `src/chat.rs` — streaming compatible API loop and deterministic replay
- `ui/app.slint` / `ui/mobile.slint` — synchronized desktop and phone-frame lenses
- `docs/demo-script.md` — the 60–90 second operator script
- `docs/case-study.md` — decision and outcome brief
- `docs/production.md` — 100k-user and mobile path
- `scripts/record-demo.sh` — reproducible X11 screenshot/video capture

See [architecture.md](docs/architecture.md) for the boundaries and concurrency
model.

## Quality gates

```bash
make check
```

CI enforces formatting, Clippy with warnings denied, all targets, and all
tests. The fixture test also launches the compiled `pulse` binary and verifies
that the default stored budget produces five JSON cards. Engine, bridge, chat,
and MCP tests cover persistence, dynamic count/budget status, and the ceiling.

## Scope

Chat, desktop research, and visual composition are the complete demo. Voice is
intentionally a separate lens over the same bridge, not a prerequisite. Mobile
research parity (including companion/i3 rules), a remote R5 client, and native
Android packaging are deferred. The production brief describes those seams
without pulling them into the 90-second core.

Licensed under MIT.
