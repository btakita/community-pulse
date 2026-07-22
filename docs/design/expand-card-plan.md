# Expand-Card (Evidence) Feature Plan

Make the digest card expand/collapse interaction actually work, matching
`mockup-desktop.html` (card 02 there shows the expanded target state).

## UX contract

1. Click **"why?"** on a card → the card expands inline: accent border,
   large sparkline with dashed 7-day baseline, stats row, contributing
   posts, rank-formula line. Everything else stays put; the list reflows.
2. Click **"close"** on the expanded card → it collapses back to 84px.
3. At most **one card is expanded at a time** — expanding another card
   collapses the current one implicitly.
4. The agent can drive it: a chat/voice `explain_trend` tool call expands
   the matching card with no pointer input. Clearing evidence collapses it.
   (This is the shared-state demo moment — keep selection *derived* from
   the evidence cell, never a local UI flag.)
5. Height animates (~180ms ease-in-out); no snap.

## Current state (what exists after 225bf9b)

- `ui/app.slint` DigestCard: `selected` in-property, `height: selected ?
  430px : 84px`, `if selected: EvidencePanel`, label flips "why?"/"close",
  `explain()` callback (line ~506-614).
- Digest loop derives `selected: root.has-evidence && root.evidence.topic
  == card.topic` (line ~967) — derived selection, correct idea.
- `src/app.rs` `on_explain_requested` → `bridge.explain_trend(id)` sets the
  evidence cell; `apply_snapshot` fills `evidence` + `has-evidence`.

## Defects that block it

- **No collapse path.** "close" fires `explain()` → re-fetches evidence →
  stays open. Nothing ever sets the evidence cell back to `None`.
- **Fragile match key.** `evidence.topic == card.topic` compares *display*
  strings ("Rust"); the stable key is the topic id. One rename or casing
  change kills selection silently.
- **Fixed 430px.** Evidence post count varies (explain returns up to 5);
  the panel clips or leaves dead space.
- **No animation** and no `clip: true`, so the conditional panel pops.

## Work items (ordered)

### 1. Select by id
- `EvidenceRow` (app.slint struct) gains `id: string`; populate it in
  `apply_snapshot` (src/app.rs ~285) from `evidence.id`.
- Digest loop: `selected: root.has-evidence && root.evidence.id == card.id`.

### 2. Close path (the actual toggle)
- tools.rs: add
  ```rust
  pub fn clear_evidence(&self) { self.state.set_evidence(None); }
  ```
- app.rs `on_explain_requested`: make it a toggle — if
  `bridge.snapshot().evidence` is already this id, call `clear_evidence()`
  instead of re-fetching; then `render_now`. (No new Slint callback needed:
  "close" keeps firing `explain()` and the toggle logic lives in Rust.)
- Chat stays as-is: a tool-driven `explain_trend` still expands the card.

### 3. Content-driven height + animation
In DigestCard:
```slint
clip: true;
height: selected ? layout.preferred-height + 26px : 84px;
animate height { duration: 180ms; easing: ease-in-out; }
```
where `layout :=` names the existing root VerticalLayout (padding 13 top +
bottom = 26px). Delete the fixed 430px. EvidencePanel keeps its own
internal sizing; verify with 1-post and 5-post evidence.

### 4. Polish (do only after 1–3 verified)
- Fade the panel in: keep `if selected:`, set `opacity: 0 → 1` with a
  120ms animation inside EvidencePanel's root.
- `Esc` collapses: window-level `FocusScope` key handler →
  `explain-requested` of the open id (toggle closes it) or a dedicated
  clear callback.
- Scroll-into-view: on expand, animate the ScrollView `viewport-y` so the
  expanded card's bottom is visible (`index * (84px + 10px)` puts the
  card top; only adjust when it would be clipped). Skip if fiddly — the
  list is short.

## Tests

- tests/bridge.rs: after `explain_trend("rust")`, `snapshot.evidence`
  is `Some` with `id == "rust"`; after `clear_evidence()` it is `None`.
- tests/bridge.rs: toggle semantics — calling the app-level toggle twice
  ends with `None` (exercise via `explain_trend` + `clear_evidence` if the
  toggle lives in app.rs; keep app.rs logic thin enough that this is fair).
- Manual: `make demo`, click "why?" on cards 1 → 3 → 3: card 1 collapses
  when 3 opens; second click on 3 closes it. Then type "Why?" in chat: the
  top card expands without pointer input — say this out loud in the demo.
- `make check` stays green.

## Non-goals

- Mobile bottom-sheet variant (`mockup-mobile.html`) — different surface.
- Multi-expand / comparison view.
- Evidence pagination; 5 posts max is fine for the demo.
