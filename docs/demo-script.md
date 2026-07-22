# 60–90 second demo script

Start before joining the call:

```bash
cargo run --release -- --database demo.db --fixture --replay app
```

Keep the window at 1480×900 or use screen-share “portion of screen.”

## Run of show

**0:00–0:15 — attention budget**

Point at the `5 / 5` master strip and say: “Three public feeds become one shared
trend table. A budget always exists, but the user owns it.” Move the master
fader to 8; the fixture honestly shows `7 / 8` because only seven signals are
eligible. Return it to 5 for the core run and say: “Scale never turns this into
an infinite feed, and deeper evidence stays one pull away.”

Click **What's moving?**. The `get_pulse` chip should appear, followed by the
two-sentence response.

**0:15–0:35 — one state, two controls**

Click `+` on Rust and `×` on Crypto. Point out the immediate rerank. Then send
**More Rust** and say: “The visual mixer and the agent call the same bridge and
write the same lazily cells; there is no synchronization story to debug.”

**0:35–0:55 — evidence, not vibes**

Click **Why?** on a WASM or Rust card. Point to its 12-hour sparkline, 1h/6h/24h
velocity, seven-day mean and standard deviation, z-score, and named source
posts.

**0:55–1:10 — personal bridge**

Send **Track WASM runtimes for me**. Point to the tracked list and say: “That is
the seam from a community-shared trend to a personal Spark alert.”

**1:10–1:25 — resilience and scale**

Say: “This run is a deterministic SQLite fixture and replay agent, so interview
Wi-Fi cannot take it down. In production the shared trend computation is done
once; each user only reranks a small candidate set.”

## Additive demo beats (after the 90-second core)

- Start with `app --companion`: move a desktop fader or send **More Rust**, then
point to the synchronized Mix tab in the phone frame. Open **why?** there to
show the evidence bottom sheet and the previous-snapshot delta chips. Rotate
with the bezel control or Ctrl+R: the tabs become a left rail and the same open
evidence becomes a right panel without resetting state. A digest headline or
evidence row routes outward to its original community post.
- Start with `app --mcp-port 7432`, register the endpoint, then ask an external
agent to set the attention budget to eight, check the pulse, and track WASM
runtimes. Point to the synchronized master fader, live tracked state, and
threshold alert; no pointer input is involved.
- On desktop, expand one card and click **Research with Claude** and **Research
with Codex**. The two run chips may execute concurrently; when reports land,
open **RESEARCH** and show the side-by-side comparison. This is an additive
beat—skip it if account preflight was not completed before the call.
- For a live-data variant, start a previously populated database with
`app --live`; the header ingest action and timer share the same 120-second
cooldown. Snapshot a good run beforehand with `pulse snapshot` so network
trouble can fall back to an identical database.
- All additive beats are optional. If any surface has trouble, continue the core
desktop run without changing the story.

## Fast recovery

- Empty database: restart with `--fixture`.
- Missing API key: use `--replay`; the app also falls back automatically.
- Window trouble: run `cargo run -- --fixture top` and narrate the same data
  story from the CLI.
- Before every rehearsal: remove or choose a disposable `--database` path,
because `--fixture` intentionally replaces that database's data.

## Attention-budget rehearsal checks

- Start with a disposable database and confirm the default master strip reads
`5 / 5` on desktop and mobile.
- Move the desktop master 3 → 8 → 10; confirm both windows synchronize and the
fixture reads `3 / 3`, `7 / 8`, then `7 / 10`. Return it to 5.
- Send **Give me eight today** through replay chat; confirm the tool chip is
`set_interests`, both master faders read 8, and the status reads `7/8 signals`.
- Restart with the same database and confirm budget 8 persists. `--fixture`
refreshes community data but intentionally preserves the user setting.
- Through MCP request `attention_budget: 50`; confirm `isError: false`, the
returned/stored budget is 10, and both visible lenses update to `7 / 10`.
- Leave top-card evidence open while changing the master and confirm research
stays open; then restore budget 5 before the 90-second core.
