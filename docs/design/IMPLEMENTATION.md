# Design Implementation Guide

Agent-facing spec for converging the Slint app on the approved design. The
sources of truth are **in this directory** — open them in a browser, view
source for exact values:

- `mockup-desktop.html` — the approved desktop design. All tokens are CSS
  custom properties at the top of its `<style>` block (light + dark sets).
  The "Design spec — for the Slint port" section at the bottom of the page
  states layout metrics, sparkline specs, and component rules.
- `mockup-mobile.html` — mobile variant (future; not required now).

Work the P0 list first — each item is a place where the demo contradicts
itself and an interviewer will notice. P1 is visual parity.

Feature plans in this directory:
- [`expand-card-plan.md`](expand-card-plan.md) — make the digest-card
  expand/collapse (evidence) interaction work: id-based selection, a real
  close/toggle path, content-driven animated height.
- [`mcp-integration-plan.md`](mcp-integration-plan.md) — in-process MCP
  endpoint (streamable HTTP) so Claude Code/Codex/OpenCode drive the same
  ToolBridge and the UI updates live. Read its architecture-decision
  section before writing any code: in-process HTTP, never a stdio child.
- [`research-interface-plan.md`](research-interface-plan.md) — R1→R4 baseline is
implemented and locally verified. The ordered startup, ingest, enrichment, and
article-brief follow-ons are also implemented; its status section records the
remaining human review and mobile/R5 deferrals.

## Implementation status (2026-07-22)

- [x] Desktop parity/correctness baseline, attention budget, outbound links,
mobile-frame rotation, and the shared ingest controller are implemented and
covered by the existing engine/UI tests.
- [x] Research demo polish is implemented: companion-terminal lifecycle,
agent-family report reconciliation, supported markdown heading normalization,
and shared SVG disclosure/close/copy affordances.
- [x] Local artifact links and text ergonomics are implemented: canonicalized
`research/logs/` and `research/reports/` allowlists, selectable chat/tool text,
whole-message clipboard actions, and exact raw-markdown report copy.
- [x] Desktop viewport behavior is implemented: an independently scrolling and
collapsible Mix rail, constrained/elided digest content, and one persistent,
resizable Agent/Research pane width.
- [x] The expanded Mix viewport regression is fixed: topic channels and
suggestion pills stay inside the rail, long research suggestions elide, and no
horizontal scrollbar can obscure or displace rail controls.
- [x] Right-pane resizing preserves the user's exact clamped drag result; the
divider has no click-to-snap preset behavior.
- [x] The deterministic Xvfb shot harness and demo-launcher CLI smoke target are
implemented and are part of the documented verification path.
- [x] The shared delayed hover-tooltip primitive and dynamic z-score content are
implemented on digest-card and evidence z badges, including value bands and
conditional baseline caveats.
- [x] Source-provided post summaries are ingested without fabrication and shown
through shared headline tooltips plus expandable evidence-row attribution.
- [x] Mix channels can be removed without muting: the shared close affordance
clears the stored stance, removes the row, and lets trending topics resurface.
- [x] The scoring methodology is exposed through shared info tooltips and
desktop disclosure panels whose formula and live-budget copy are Rust-owned.
- [x] The clarity pass is implemented: shared bucket-hover tooltips and chart
context labels explain the 12-hour series, scorer-owned worked examples expose
each selected card's arithmetic, the mention stat defines its unit, and matched
aliases are required evidence-row provenance.
- [x] Desktop titlebar controls are vertically centered, including when i3 adds
its own tab decoration above the application window.
- [x] Source normalization is implemented: rolling seven-day source volumes
produce clamped weights shared by velocity, z inputs, and z baselines; raw post
counts stay unweighted in the UI; vote comparisons use within-source
percentiles; and the methodology panel renders the scorer's live weights.
- [x] Mentions tooltips include mixed-source bucket totals and source-prefixed
post titles; single-source buckets omit the redundant breakdown.
- [x] Desktop Mix channels use segmented `− / + / ++` interest controls with a
visible upper-right `×` for the zero/remove action; fractional weights render at
their nearest state and snap on the first tap, while attention budget stays a fader.

### Desktop titlebar alignment (operator request)

The faux desktop chrome must match the approved mockup instead of appearing
pressed against the top edge under i3. Its traffic-light controls, title,
theme toggle, and live badge are centered on the titlebar's vertical axis.

Acceptance: launch the desktop app in an i3 tabbed container; the full native
i3 decoration remains visible above the app, and every control in the app's
39px titlebar has balanced space above and below it.

## P0 — correctness (do these first)

### 1. Duplicate headline across digest cards
`engine.rs` `headline_and_sources()` (~line 277) picks each topic's newest
post independently, so one post headlines two cards (fixture: "Wasmtime
ships a faster component-model runtime" appears on both `rust` and
`wasm-runtimes`). Fix in `get_pulse()`:

1. Build all affinity-filtered rows first, sort by `score` desc.
2. Then assign headlines in rank order with a `HashSet<String>` of used post
   ids: change `headline_and_sources` to return the top-8 candidate posts
   `(post_id, title, source)` and pick the first whose id is unused; record
   it. Sources set stays as-is.
3. Add a test in `tests/engine.rs`: all `card.headline` values in the
   fixture digest are unique.

### 2. Card sparklines are hardcoded
`ui/app.slint` ~line 370: every card shows the literal string `"▁▂▂▃▂▄▅▇"`.
Five identical sparklines read as fabricated data. Fix:

1. Extract the 12-bucket hourly loop from `explain_trend()` (engine.rs
   ~359) into `fn hourly_series(conn, topic, now, buckets) -> Vec<usize>`.
2. Add `sparkline: Vec<usize>` to `DigestCard` (domain.rs) and fill it in
   `get_pulse()`. `get_pulse` gains a `now: DateTime<Utc>` parameter —
   update callers in tools.rs (get_pulse / refresh_scores / set_interests /
   set_interest use `Utc::now()`), main.rs, and tests (fixed `now`).
3. app.rs: `DigestRow` gains `spark: string` rendered with the existing
   `sparkline()` helper; app.slint uses `data.spark` instead of the literal.

### 3. Displayed math contradicts computed math
- app.rs `apply_snapshot` baseline string: when `baseline_stddev < 1.0`,
  append a floor note, e.g. `baseline μ 0.1 · σ 0.3 · z floors σ at 1.0`
  (engine divides by `stddev.max(1.0)`, engine.rs ~471 — displaying μ/σ
  alone implies a much larger z than shown).
- app.slint EvidencePanel caption (~line 281) says
  `score = recent velocity ÷ seven-day baseline` which is not the formula.
  Replace with: `rank = trend × interest · trend = velocity × (1 + z⁺/4) + sources/2`.

### 4. Live-chat tool errors poison the API conversation
chat.rs `respond_live` (~line 164): `self.bridge.call(...)?` bails after the
assistant `tool_calls` message is already in history but before any tool
result message — every later turn then sends malformed history. Fix:

```rust
let result = match self.bridge.call(&call.name, &call.arguments) {
    Ok(value) => value,
    Err(error) => serde_json::json!({ "error": format!("{error:#}") }),
};
```

Always emit the ToolCall event and push the tool message. Add an `error`
branch to `compact_result()` in app.rs so the chip shows the failure.

### 5. Chat read-modify-write race
reactive.rs `append_to_chat` / `replace_chat` (~lines 134–148) do a
non-atomic get→mutate→set while the UI thread can append concurrently.
Wrap both in `self.context.batch(|context| { ... })` like `append_chat`.

## P1 — visual parity with mockup-desktop.html

### Fonts (bundle, don't rely on system faces)
Current UI uses Noto Sans / Noto Sans Mono; the design is **Instrument
Sans** (UI text) + **JetBrains Mono** (every number, label, readout, tool
chip; tabular figures; uppercase letter-spaced eyebrows). Bundle both so the
look can't drift per machine:

1. Download the OFL variable TTFs into `ui/fonts/` (include their OFL.txt):
   - https://github.com/google/fonts/raw/main/ofl/instrumentsans/InstrumentSans%5Bwdth%2Cwght%5D.ttf
   - https://github.com/google/fonts/raw/main/ofl/jetbrainsmono/JetBrainsMono%5Bwght%5D.ttf
2. In app.slint: `import "fonts/InstrumentSans[wdth,wght].ttf";` and
   `import "fonts/JetBrainsMono[wght].ttf";`, set
   `default-font-family: "Instrument Sans"`, and change every
   `"Noto Sans Mono"` to `"JetBrains Mono"`.

### Fader honesty (neutral shows 0.5 but weight is 0.0)
app.slint TopicChannel (~lines 98, 136): neutral channels display "0.5" and
a 0.42 fill while the real weight is 0. Make the fader linear in weight over
its actual domain [-1, +2]:

- `visual-weight = (weight + 1) / 3` → muted 0, neutral ⅓, +1 ⅔, +2 full.
- Click handler: `set-weight(mouse-x / width * 3.0 - 1.0)` (clamp to
  [-1, 2]; snap |w| < 0.05 to 0).
- Neutral readout: `0.0` (never a fabricated number).

### Suggested topics are static
app.rs `topic_rows` ignores its `_suggested` argument; the SUGGESTED chips
in the composer don't react to state. Add an
`in-out property <[string]> suggested-topics` to AppWindow, render the chips
from it (a click calls `set-interest(topic, 1.0)`), and wire
`snapshot.suggested_topics` in `apply_snapshot`. Filter out topics already
present in the mixer.

### Chat rows truncate
app.slint ChatMessage (~line 396) fixes heights at 66px/45px; a live LLM
answer longer than ~3 lines gets elided. Remove the fixed heights and let
the VerticalLayout size rows from wrapped text (keep the TOOL chip row a
fixed 27px).

### Ingest client hardening
ingest.rs `fetch_all`: add `.timeout(Duration::from_secs(20))` to the
`Client` builder so one hung feed can't stall `pulse ingest` forever.

## Outbound source links (new feature, small)

Evidence post rows and each digest card's headline must open the original
post (HN item / lobste.rs story / Product Hunt page) in the system
browser. The engine already stores `url` on every post — this is UI
plumbing only. Framing: the pulse routes outward, not a walled garden.

1. **Engine**: `headline_and_sources` already selects the headline post —
   also return its `url`; add `headline_url: String` to `DigestCard`
   (domain.rs). `EvidencePost` already has `url`.
2. **app.rs**: add `url` to `DigestRow` and `EvidencePostRow`; populate in
   `apply_snapshot`. Add an `open-url(string)` callback handler on the
   window that calls the `open` crate (`open::that_detached(url)`; add the
   dependency — cross-platform, avoids hand-rolled xdg-open).
3. **app.slint**: headline Text and each evidence post row get a TouchArea
   → `open-url(...)`; hover affordance: underline + `mouse-cursor:
   pointer`. Do NOT make the whole card a link target — it would collide
   with the expand-card "why?" toggle; keep link targets on the headline
   text and post rows only.
4. **Mobile frame**: same callback; links open the desktop browser (fine —
   it's a phone-frame demo, note it in the demo script).
5. **Test**: fixture digest cards all carry non-empty `headline_url`;
   evidence rows carry the post urls (bridge-level assertions; don't try
   to test the browser launch).

## Phone chrome + orientation (`--mobile` polish, ~half day)

Make the phone-frame window read as a device, at a true aspect, with live
portrait ⇄ landscape switching. Reference styling: the phone frames in
`mockup-mobile.html` (40px bezel radius, sysbar, caption row is NOT part
of the app).

1. **Aspect + chrome**: app surface portrait **390×844 logical** (19.5:9;
   replaces 390×740). Wrap it in a bezel Rectangle (~14px padding, ~40px
   outer radius, `Tokens.ink`-dark neutral fill, subtle drop shadow) so
   the outer window is ~418×872. Inside, above the app surface: a status
   bar row — live clock (populate a `sysbar-time` property from Rust with
   a `slint::Timer` every 30s, `chrono::Local` "%H:%M"), camera pill,
   battery glyph (static). Below: home-indicator bar (36×4px rounded,
   centered). None of these are interactive.
2. **Orientation state**: `in-out property <bool> landscape` on the mobile
   window + `callback rotate-requested()`. Triggers: a small rotate glyph
   button in the bezel margin (bottom-right) and a keyboard shortcut
   (FocusScope on `Ctrl+R`; don't use plain `r` — the chat input needs
   it).
3. **Rust handler**: on rotate, flip the property and swap the outer
   window size via `window.window().set_size(slint::LogicalSize::new(w,
   h))` (portrait ~418×872 ⇄ landscape ~872×418). Keep the window
   non-user-resizable so the aspect stays honest.
4. **Landscape relayout** (same components, conditional layout):
   - Tab bar: bottom `HorizontalLayout` → left vertical rail (icons +
     labels stacked, width ~72px).
   - Pulse tab: digest list keeps ~55% width; **evidence bottom sheet
     becomes a right-side panel** (~340px, full height, same scrim
     behavior and same derived-selection state — no new state).
   - Mix/Agent tabs: single column is fine in landscape; just let the
     content column max-width at ~560px centered.
5. **Demo script**: add "rotate mid-demo" to the mobile beat — the point
   is responsive relayout from one component set, so say that while
   rotating.
6. **Verify**: `make check` green; manual — rotate with the evidence sheet
   open (it must survive as the side panel, still dismissable), rotate
   while chat is streaming (no layout panic), and both orientations fit a
   1080p screen-share window.

## User-adjustable attention budget (master fader)

Status: **implemented and automated**; the presentation-machine sequence below
remains a human rehearsal check.

Refined invariant: **a budget always exists, the user owns it, and
unbounded is never the default.** The digest cap becomes user-adjustable
within an engine-enforced ceiling; research depth stays pull-based
(evidence, chat, outbound links) and is never governed by the budget.

1. **Engine**: `ATTENTION_BUDGET` (5) becomes `DEFAULT_BUDGET`; add
   `MAX_BUDGET: usize = 10`. `get_pulse` clamps to `1..=MAX_BUDGET` and
   reads the user's stored budget when the caller passes no limit. Persist
   in a new `settings` table (`key TEXT PRIMARY KEY, value TEXT`) with
   `attention_budget`; add `PulseEngine::{budget, set_budget}`.
2. **Bridge/state**: `budget: Source<usize>` in `PulseState`; ToolBridge
   `set_budget(n)` clamps, persists, refreshes the digest, updates the
   status Computed ("{cards}/{budget} signals …").
3. **Tool surface**: extend `set_interests` with an optional
   `attention_budget` integer (1–10) rather than adding a fifth tool —
   chat and MCP get it for free ("give me eight today" → set_interests
   with attention_budget: 8). Update the tool description text.
4. **UI (the metaphor payoff)**: the budget is the **master fader** — a
   channel strip pinned at the bottom of THE MIX panel, visually distinct
   (accent-colored fill, label "ATTENTION BUDGET", readout "5"). Fader
   range 3–10 stepped; the existing budget meter becomes `n / budget`
   segments and lives inside the master strip. Mobile: same strip at the
   bottom of the Mix tab.
5. **Copy rule**: never present it as "show more" — the label stays
   "attention budget" and the long-tail research path stays separate
   (evidence, chat, source links). If an "explore all topics" table view
   is added later it must be visually demarcated as leaving the pulse
   (different surface, explicit entry action) — do not implement it now.
6. **Tests**: budget persists across engine reopen; `get_pulse` with no
   limit honors stored budget; clamp at 10 (a request for 50 via MCP
   returns 10 and `isError: false` — clamping is policy, not an error);
   meter shows `count/budget`; digest re-ranks immediately on change.

## In-app ingest (~half day)

Status: **implemented** through the shared manual/live controller with
controller-level stub tests and snapshot/restore coverage.

Bring `pulse ingest` into the UI so fresh data is one click, not a CLI
round-trip. Build it as a shared `IngestController` that BOTH the UI
trigger and R4's `--live` timer call — one code path, two triggers.

1. **Controller (Rust)**: async task per the `start_chat` thread pattern:
   `ingest::fetch_all()` → `engine.ingest()` per source → `recompute` →
   refresh digest/suggested cells → render. Enforce the **politeness
   floor here** (shared const, min 120s between runs regardless of
   trigger — UI click, timer, anything); a too-soon request returns the
   remaining cooldown instead of fetching.
2. **State**: `ingesting: Source<bool>`, `last_ingest_at:
   Source<Option<DateTime>>`, and `source_status:
   Source<Vec<SourceStatus { name, ok, count, error }>>` filled from
   `fetch_all`'s per-source results.
3. **UI (desktop)**:
   - The header "ingest Ns ago" readout becomes the live trigger: shows
     real elapsed time (tick it with the existing sysbar timer), click →
     spinner while running, then flash the result ("+34 posts").
   - Status-bar source dots go live: green ✓ per succeeded source, red
     with the error text (elided) on failure. Per-source failure is
     non-fatal — ingest what succeeded.
   - Cooldown feedback: clicking during the floor shows "next ingest in
     Ns" instead of silently ignoring.
4. **Fixture guard**: in `--fixture` mode the trigger is disabled with
   hint "fixture mode — ingest off". Mixing live posts into the
   deterministic snapshot would poison demo reproducibility; protect it.
5. **Digest continuity**: after ingest, delta chips fire naturally from
   the snapshot comparison — that's the payoff moment (click ingest →
   "2 new · rust cooled" appears). Make sure the previous-snapshot
   capture happens BEFORE the recompute so the chips are truthful.
6. **Not in scope**: exposing ingest as an MCP/chat tool (agents get
   fresh-enough data via the floor + timer; revisit later), and mobile
   pull-to-refresh (deferred with mobile parity — it becomes a second
   trigger on the same controller).
7. **Tests**: cooldown floor honored across triggers; per-source failure
   leaves other sources' posts ingested; fixture mode disables; the
   engine-level flow is already covered — add controller-level tests
   with a stub ingester, not network tests.

## Local file deep links + selectable chat text (operator feedback)

**Status: implemented.** Desktop and mobile chat rows share artifact metadata;
desktop messages expose native selection and hover-copy controls. The open path
resolves existing files before dispatch and rejects anything outside the two
approved artifact roots, including symlink escapes.

Two desktop-ergonomics items, related because the first's payoff depends
partly on the second:

### 1. Deep links to local files (logs, reports)

Chat messages and research surfaces mention local artifacts — most
importantly the `research/logs/<run>.log` path echoed into Agent chat by
the run diagnostics, and `research/reports/*.html` web reports. On
desktop, these must be clickable:

- Extend the `open-url` path to accept local paths: absolute or
  repo-relative → open via the `open` crate (default handler:
  editor/viewer per xdg association).
- **Allowlist, not open season**: only paths under `research/logs/` and
  `research/reports/` (canonicalize + prefix-check before opening —
  agent-authored text must not be able to open arbitrary files). Reject
  silently logged elsewhere.
- Render: detect known-path patterns in chat/tool-chip text and style
  them like links (accent underline, pointer cursor, TouchArea). Where
  detection inside wrapped text is fiddly, render a small "open log ↗"
  chip under the message instead — same pattern as the markdown link
  chips.
- Mobile frame: same process so it works, but do not spend layout time
  there (deferred with parity).

### 2. Selectable + copyable chat text

Chat bodies are plain `Text` — not selectable, so paths/ids can't be
copied. Fix:

- Swap chat message bodies (and tool-chip text) to read-only
  `TextInput` (`read-only: true; wrap: word-wrap;` styled borderless to
  match current rendering) — Slint gives selection + Ctrl+C natively.
  Verify styling parity (no focus frame, same colors/line-height) so it
  is visually indistinguishable from before.
- Add a small copy icon button on message hover (Rust callback +
  `arboard` clipboard crate) as the fallback for whole-message copy —
  useful where StyledText panes (research reports) can't become
  TextInput without losing formatting; there the button copies the raw
  markdown.
- Test: a chat message containing a log path can be selected, copied,
and its rendered link opens the file; report copy button round-trips
the exact markdown.

Acceptance is code-enforced by the allowlist and symlink-escape tests plus the
exact-markdown round-trip test. The UI may use a dedicated `open log ↗` or
`open report ↗` chip when a wrapped inline path cannot safely host a hit target;
the chip still routes through the same allowlisted `open-url` callback.

## Desktop viewport + side-pane behavior (operator feedback)

**Status: implemented.** The supported desktop minimum remains 1100×720 and
the surface must not depend on content extending beyond that viewport.

- The Mix rail is 262px expanded and 44px collapsed. Its SVG chevron toggles
the state; topic/suggestion content scrolls independently while the 78px
attention-budget master remains pinned at the bottom.
- The digest column owns the remaining width and scrolls vertically. Headline
layout has a zero minimum width and elides inside the card rather than forcing
the card or column outside the viewport.
- Agent and Research are two states of the same right pane, not two drawer
sizes. They share one 420px default width, retain the current width on tab
switch, and expose an 8px `ew-resize` divider clamped to 320–620px. Clicking the
divider does not alter pane width; dragging permits continuous adjustment and
the released width remains exactly where the user left it.
- The header, Mix rail, digest, and right pane remain inside the window at the
minimum size. `09-minimum-viewport.png` is the deterministic acceptance image;
`07-resizable-agent-pane.png` and `08-resizable-research-pane.png` must have the
same divider x-coordinate.

**Regression acceptance (2026-07-22): implemented.** The expanded Mix
ScrollView owns exactly the visible rail width and disables horizontal
scrolling. Every topic channel and suggestion pill is constrained to that
viewport; long author/research-derived suggestion labels elide within their
pill. Vertical scrolling remains independent and the attention-budget master
remains pinned below it.

## Shared icon affordances (operator feedback)

**Status: implemented.** Interactive disclosure, close, and copy controls use
repo-owned SVG assets through shared Slint components rather than Unicode text
glyphs. Disclosure icons rotate to reflect open/closed state, all buttons have
a 24–26px hit target, and hover/active treatment comes from the shared token
palette. Report copy always receives raw markdown even though its pane renders
styled content.

## Deterministic demo shots + launcher smoke (tracker item)

**Status: implemented.** `make demo-shots` builds the all-features binary and
runs `scripts/capture-demo-shots.sh` under a 1920×1080 Xvfb display with the
software Slint backend, temporary SQLite files, bundled fixture data, and the
replay agent. The harness emits one numbered, descriptive PNG for each scripted
desktop beat plus the legacy desktop/mobile/rotation smoke captures. The
manifest in `demo/shots/README.md` is the authoritative beat-to-file map.

`make demo-launcher-smoke` runs `bin/desktop-and-mobile.sh --help` against the
built binary, so Clap parses the launcher's real argument vector without
opening windows or starting ingest. `make check` depends on this smoke target.
The launcher contract is `app --companion --mcp-port 7432 --live
--ingest-interval 300`; `--live 300` is invalid and must fail the smoke.

## Dynamic z-score tooltip (operator request)

**Status: implemented.** Digest-card and evidence z badges share one 350ms,
token-styled hover primitive. Tooltip content comes from the pure `z_tooltip`
formatter, with unit coverage for every band and both conditional caveats.

Hovering any z badge (digest card `z +2.9 ▲`, evidence panel z stat)
shows a tooltip that BOTH teaches the metric and interprets the actual
value. Slint has no built-in tooltip — build a small hover popup
(TouchArea `has-hover` with ~350ms delay → floating Rectangle above the
badge, dismiss on hover-out; reuse tokens: raised surface, line border,
mono text, max-width ~280px).

Content is composed in Rust — `fn z_tooltip(z, mentions_6h,
baseline_mean, baseline_stddev) -> String` — from three parts:

1. **The concrete numbers, always**:
   `"{mentions_6h} mentions this 6h vs typical {μ:.1} ± {σ:.1}"`.
2. **A dynamic interpretation band** (pick by z value):
   - z ≥ 3 → `"rare: {z:.1}σ above its own weekly norm"`
   - 2 ≤ z < 3 → `"unusual: well above its typical week"`
   - 1 ≤ z < 2 → `"elevated: above its usual range"`
   - −1 < z < 1 → `"normal range for this topic"`
   - z ≤ −1 → `"cooling: below its weekly norm"`
3. **Honesty caveats, only when they apply**:
   - σ < 1.0 → `"quiet topic: z uses a σ floor of 1.0"`
   - baseline_mean < 1.0 → `"small baseline — z can overstate;
     check mentions + sources"`

Close with the fixed one-liner: `"z compares a topic to its own
history, not to other topics"` — the self-normalization property is the
thing users most misread.

Notes: same tooltip component reused on both badges; keyboard/touch
fallback not required (desktop hover feature); tests unit-test
`z_tooltip` band selection + caveat inclusion (pure function, no UI
test needed); the tooltip must never block clicks on the card beneath.

## Source-provided post summaries (operator request)

**Status: implemented.** Source-authored text is normalized and stored with an
existing-database migration guard, threaded to cards and evidence, and exposed
through the shared delayed tooltip. Evidence rows use the requested paragraph
glyph to toggle full inline text; exactly half of the fixture posts exercise the
summary state, while the rest retain no affordance.

The sources sometimes carry author-provided text: Product Hunt entries
have a summary/description; HN `story_text` exists for self-posts
(Ask/Show HN); Lobsters `description` for text posts. Link posts have
nothing — and we NEVER fabricate: no text → no affordance.

1. **Ingest**: capture into a new `summary TEXT NOT NULL DEFAULT ''`
   column on `posts` (add to the CREATE TABLE + a migration guard for
   existing dbs). Normalize: strip HTML tags/entities to plain text,
   collapse whitespace, trim to ~500 chars on ingest. Sources: PH
   `entry.summary`/`content`, HN `story_text`, Lobsters `description`.
2. **Thread through**: `EvidencePost` gains `summary`; `DigestCard`
   gains `headline_summary` (the headline post's summary, often empty).
3. **UI (tooltip + expandable, space-economical)**:
   - Evidence post rows with a non-empty summary show a small
     **paragraph glyph (`¶`)** with a ~24px hit target. Hover the glyph →
     the shared tooltip component (same one as the z-score tooltip) shows
     the first ~200 chars.
   - Click the glyph → the summary expands inline under the post row
     (full stored text, ink-2, small); click it again to collapse.
   - Digest card headline: tooltip only (no inline expansion — the card
     already has the evidence panel for depth).
   - Attribution matters: label the tooltip/expansion "author's text ·
     <source>" so it's never mistaken for our summary or an agent's.
4. **Fixture**: give ~half the fixture posts realistic summaries so the
   feature demos deterministically and the empty-state (no glyph) is
   also visible.
5. **Tests**: HTML stripped; empty summary → no glyph/affordance;
   truncation at ingest; tooltip text matches stored summary prefix.

## Remove channels from The Mix (operator request)

**Status: implemented.** Desktop and mobile channel rows call the existing
`set_interest(topic, 0.0)` path from the shared close affordance. The M control
uses a distinct mute transition so toggling it off persists a tracked neutral
row instead of invoking removal. Desktop hover copy preserves the
remove-versus-mute distinction verbatim; the mix may be empty, and tests cover
row removal, persistence removal, neutral digest stability, suggested
resurfacing, mute exclusion, and unmute-to-neutral retention.

Suggested chips can ADD a channel but nothing can remove one — the mix
only grows. Fix with explicit removal, and make the semantics honest:

1. **Semantics — removal is "clear my stance", not "hide this topic"**:
   removing a channel calls the existing `set_interest(topic, 0.0)` path
   (weight 0 deletes the interests row). The topic leaves the mix list
   and may resurface as a suggested chip if it trends. CRITICAL
   distinction to preserve in copy/tooltip: a removed (neutral) topic
   can still appear in the digest (affinity 1.0) — **mute is the
   exclusion tool, remove is the tidy-up tool**. Tooltip on the remove
   button: "remove from mix (topic stays eligible — use M to exclude)".
2. **UI**: an ✕ icon button (shared SVG family, ≥24px target) revealed
   on channel hover, placed beside the M mute button. No confirmation —
   the action is reversible (re-add from suggested or by chat) and
   low-stakes by the semantics above. Any channel can be removed,
   including the seeded defaults; an empty mix is legal (digest then
   ranks purely by trend).
3. **Wiring**: reuses existing bridge `set_interest` — no new tool.
   Note for chat/agents: `set_interest(topic, 0)` already achieves
   removal; the replay agent's vocabulary may add "reset <topic>" if
   cheap, but that is optional.
4. **Mobile frame**: same button on the Mix tab channels (shared
   component); no extra layout work beyond it fitting the 56px strip.
5. **Tests**: remove → interests row gone, channel leaves topic_rows,
   digest unchanged for a neutral topic (affinity 1.0 before and
   after); removed-then-trending topic reappears in suggested; mute ≠
   remove covered explicitly.

## Methodology explainer — scoring + categorization (operator request)

**Status: implemented.** Shared ⓘ controls sit beside the desktop and mobile
pulse headings and evidence formulas. Desktop clicks open the four-stage
disclosure; mobile stays tooltip-only. Scoring constants and every explainer
string share `engine.rs`, with a no-drift unit test and a live surface-budget
value. Required per-post categorization transparency records the most-specific
alias and exposes it in evidence-row tooltips.

Users (and interview panelists) should be able to ask the UI "how are
these ranked and why is this post in this topic?" and get the real
answer. Two levels, reusing existing primitives:

1. **Tooltip level**: an ⓘ info icon (shared SVG family) beside the
   "TODAY'S PULSE" header and beside the evidence panel's rank-formula
   caption. Hover (shared tooltip primitive): the one-liner —
   `"rank = trend × interest · trend = velocity × (1 + z⁺/4) +
   sources/2 · capped at your budget"`.
2. **Expandable "Methodology" section**: clicking the ⓘ opens a
   disclosure panel (same chevron/expand grammar) explaining the
   pipeline in four steps, plain language:
   - **Ingest** — HN, Lobsters, Product Hunt, normalized; politeness
     floor between fetches.
   - **Categorize** — curated alias lists matched against title + tags
     (e.g. "wasmtime" → wasm-runtimes); a post can belong to several
     topics; posts matching nothing get keyword-derived topics. State
     the limitation honestly: keyword matching, no ML — misclassification
     is possible and visible (the aliases are inspectable).
   - **Score** — velocity (1h/6h/24h weighted), z vs the topic's own
     7-day baseline (σ floored at 1.0), source-diversity term, then ×
     your interest weight.
   - **Surface** — ranked, capped at YOUR attention budget (show the
     live value, not a hardcoded "5").
3. **No drift**: generate the explainer strings in Rust adjacent to the
   actual constants/formula (one module owns both), so the text can
   never disagree with the code. Unit test: the formula string contains
   the same constants the scorer uses.
4. **Required categorization transparency**: record the matched alias at
extraction and surface it in the evidence row tooltip ("in wasm-runtimes:
matched 'wasmtime'"). This requirement supersedes the original optional
stretch framing.
5. Mobile: tooltip level only for now (expandable section deferred with
   parity).

## Clarity pass: chart tooltips + worked-example scoring (operator request)

Operators are fielding methodology questions the product should answer
itself. Five items, in value order:

**Status: implemented.** Desktop charts use the shared delayed tooltip with
mouse-x bucket selection, raw-count/time context, point highlighting, and
post links in the evidence view. The selected card's scorer-owned arithmetic
expands beside the formula and is recompute-tested; mention and matched-alias
definitions are surfaced where users encounter those values.

1. **Chart hover tooltips (sparklines + evidence chart)**: hovering a
   chart shows, for the bucket under the cursor: the bucket's time range
   (local time), `"N mentions"`, and up to 2 post titles from that
   bucket (tap-through opens the post). Highlight the hovered point
   (dot + faint vertical rule). Card sparklines can show the compact
   form (time + count only); the evidence chart shows the full form
   with post titles. Implementation: bucket index from mouse-x over the
   chart's TouchArea; per-bucket post titles need `hourly_series` to
   optionally return post ids per bucket (or a second query on hover —
   it's SQLite-local, fine). Desktop only.
2. **Charts get context labels**: a small caption row under each chart —
   `"hourly mentions · last 12h"` on the evidence chart, and first/last
   bucket time labels at the chart's edges. An unlabeled axis is the #1
   source of "what am I looking at?" questions.
3. **Worked-example rank breakdown**: the evidence panel's formula
   caption becomes expandable (shared disclosure grammar) into THIS
   card's actual numbers, e.g.:
   `rank 42.1 = trend 38.3 × interest 1.10` and
   `trend 38.3 = velocity 12.4 × (1 + 1.9/4) + sources 1.5`.
   Values come from the engine's existing per-card fields; format in
   Rust beside the scorer (same no-drift rule as the methodology
   explainer — one module, unit test that the breakdown recomputes to
   the displayed rank). This is the definitive answer to "why is this
   card #1?".
4. **Define "mention" where it's shown**: tooltip on the `MENTIONS / 6H`
   stat label: `"a mention = one post categorized into this topic in
   the window; comments and votes are engagement, not mentions"`.
5. **Promote matched-alias transparency from optional to required**
   (supersedes the stretch note in § Methodology explainer): store the
   matched alias per post-topic edge and show it in the evidence-row
   tooltip ("in privacy: matched 'no-upload'"). This is the standing
   answer to "why is this post in this topic?".

Tests: bucket-index math at chart edges; breakdown string recomputes to
rank (pure fn); mention tooltip present; alias stored + surfaced.

**Addendum (operator request): visual equation breakdown.** Upgrade the
worked-example text into a **visual factor flow** in the expanded
evidence panel — the equation rendered as value chips connected by
operator glyphs, using THIS card's numbers:

```
[velocity 12.4] ×[surprise ×1.47] = 18.2  +[sources +1.5] = [trend 19.7]
   ×[interest ×1.65] → [RANK 32.5]
```

- Each chip: mono value on top, tiny ink-3 label beneath (velocity /
  surprise (z⁺/4) / sources / trend / interest / rank); operators as
  glyphs between chips. The interest chip wears the channel color; the
  final rank chip wears the accent (and matches the card's displayed
  score exactly).
- Optional second row: a proportion bar showing each factor's
  contribution to trend (velocity-driven vs surprise-amplification vs
  diversity) — only if cheap; the chip flow is the requirement.
- Values come from the same scorer-owned worked-example data (no-drift:
  the chips must recompute to the displayed rank — extend the existing
  test).
- Layout wraps at operator boundaries on narrow panes; never a
  horizontal scrollbar.
- Hovering a chip reuses the shared tooltip with that factor's
  one-line definition (same strings as the methodology explainer).

**Addendum (operator request): show sources in the mentions tooltip.**
The bucket-hover tooltip gains source attribution at two levels:
- a per-bucket summary line under the count: `"8 mentions — 6 HN ·
  2 Lobsters"` (omit zero-count sources);
- each post title line prefixed with its source in ink-3 mono
  (`"HN · <title>"`), matching the evidence-row convention.
Once source normalization lands, when weights ≠ 1 the summary line may
append the weighted value where the chart plots weighted mentions —
same raw-vs-weighted labeling rule as everywhere else. Test: bucket
with mixed sources renders the breakdown; single-source bucket omits
the redundant summary.

## Source normalization for mentions + votes (operator request)

HN's raw volume dwarfs Lobsters/PH, so uniform mention counting lets
HN-native topics outscore equally-significant Lobsters-native ones, and
raw points are incomparable across sources. Normalize both — with the
same legibility rules as the rest of the formula.

1. **Mention weights, computed not hardcoded**: per source s,
   `weight_s = clamp(global_avg_daily_posts / source_avg_daily_posts,
   0.25, 4.0)` over a rolling 7-day window from the posts table itself —
   data-derived, adapts as sources grow, no magic constants. A weighted
   mention = 1 × weight_s.
2. **Apply consistently or z breaks**: weighted counts feed velocity AND
   the 27-bucket baseline AND mentions_6h in the z numerator — mixing
   weighted current vs unweighted baseline would corrupt z. `hourly_series`
   gains a weighted variant for scoring; charts may show raw counts but
   must then be labeled "posts" not "weighted mentions".
3. **Votes: source-relative percentile, not raw comparison**: points
   stay OUT of the rank formula (unchanged), but wherever points imply
   comparison (headline pick tie-breaks, evidence row prominence),
   use the post's percentile within its own source's last-7-days points
   distribution. Display keeps raw points WITH source ("486↑ on HN");
   the percentile may appear in tooltips ("top 3% for Lobsters").
4. **Transparency (no-drift rule)**: the methodology explainer shows the
   live computed weights ("this week: HN ×0.4 · Lobsters ×1.8 · PH
   ×2.1") from the same module that applies them; unit test that
   explainer weights == scoring weights. The worked-example rank
   breakdown includes the weighting step.
5. **Display honesty**: stat rows showing raw counts (MENTIONS / 6H)
   keep raw counts — never show a weighted number where a count is
   implied. Weighted values appear only where labeled as such.
6. **Fixture**: seed volumes so the weights are visibly ≠1 in demo
   (HN-heavy fixture already implies this); snapshot tests on weight
   computation with a fixed corpus.
7. **Tests**: weight clamp bounds; weighted-consistency (z with all
   weights=1 equals old z); a Lobsters-native topic with equal
   per-capita activity ranks ≈ equal to an HN-native one on the fixture.

## Interest buttons instead of sliders (operator request)

Replace the per-channel fader with a **discrete segmented control** —
simpler to read, tap, and communicate ("privacy ++" beats "privacy
0.85"). The mixer identity survives in the channel strips, colors, and
the master budget fader (which stays a fader — it selects a count, a
genuinely continuous choice).

1. **Four states, fixed weights**: `−` mute (−1.0) · `0` neutral (0.0)
   · `+` boost (+1.0) · `++` strong (+2.0). Exactly the existing weight
   domain's landmarks — engine, tools, and chat vocabulary unchanged
   (set_interests already speaks these values). The separate M button
   merges into the `−` segment; the ✕ remove affordance is unchanged.
2. **UI**: a 4-segment control per channel (shared icon-button family,
   ≥24px per segment, active segment filled with the channel color;
   muted state keeps the dimmed-channel treatment). The weight readout
   shows the state glyph, not a decimal.
3. **Migration**: existing fractional weights display as the nearest
   state and snap to it on first tap (persisting the snapped value);
   until tapped, the stored value is untouched. Agent/chat writes of
   arbitrary weights remain legal — the control shows nearest state,
   tooltip shows the exact value when they differ.
4. **Ripple effects**: mockups keep faders (design docs note the swap);
   perspective ranking and mix sharing get simpler to display
   ("Alice: privacy ++"). Update the methodology explainer's interest
   line to name the four states.
5. **Tests**: state↔weight mapping; snap-on-first-tap persistence;
   mute-via-segment equals old M semantics (affinity 0, digest
   exclusion); nearest-state display for fractional agent-set weights.

## Typed attention-budget input (operator request)

The master fader stays; add direct numeric entry for the budget value:

1. **Interaction**: clicking the budget readout number turns it into a
   small borderless `TextInput` (mono, same size/position — no layout
   shift), pre-filled and selected. Enter commits; Esc or focus-loss
   cancels back to the previous value.
2. **Validation**: parse integer; clamp to the fader's 3–10 range (same
   clamp path as every other budget write — engine remains the final
   authority). Non-numeric input reverts silently with a brief ink-3
   hint ("3–10") shown beside the readout for ~2s.
3. **Propagation is free**: commit goes through the existing budget
   setter, so the fader thumb, meter segments, status line, and digest
   re-rank react like any other budget change — and MCP/chat writes
   still round-trip into the field like they do the fader.
4. Mobile: deferred with parity (the phone frame keeps fader-only).
5. **Tests**: commit/cancel/Esc semantics; clamp (typing 50 → 10,
   0 → 3); non-numeric revert; readout↔fader consistency after a typed
   commit.

## Explicit non-goals (don't spend time here)

- Dark theme: the app ships light-only for the demo; the mockup's dark
  token set exists if this is ever wanted, but it is NOT part of parity.
- Mobile (`mockup-mobile.html`): reference only.
- Voice: out of scope for this repo pass.

## Verification

1. `make check` (fmt + clippy -D warnings + tests) stays green.
2. `cargo run -q -- --database /tmp/pulse-check.db --fixture top` — all five
   headlines must be distinct.
3. `make demo` and compare side-by-side against `mockup-desktop.html` in a
   browser: fonts, channel strips, card sparklines (each card different),
   evidence panel caption, tool chips.
4. `make demo-shots` rebuilds and writes deterministic desktop/mobile pulse,
   evidence-open, and rotated-mobile captures under `demo/shots/`. Compare those
   captures with `mockup-desktop.html`, `mockup-mobile.html`, and the interaction
   specs in this directory before committing visual changes.
