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
- [`research-interface-plan.md`](research-interface-plan.md) — **next up**:
  agentic research over live data. Five new tools incl. `submit_research`
  write-back, Research drawer UI, CLI delegation to the user's
  `claude`/`codex` accounts, live-ingest loop. Start at its
  "Execution guide" section — phases R1→R4 with code anchors, pinned
  decisions, and per-phase acceptance gates.

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
