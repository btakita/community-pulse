# 60–90 second demo script

Start before joining the call:

```bash
cargo run --release -- --database demo.db --fixture --replay app
```

Keep the window at 1480×900 or use screen-share “portion of screen.”

## Run of show

**0:00–0:15 — attention budget**

Point at the `5 / 5` digest and say: “Three public feeds become one shared trend
table. The product invariant is an attention budget, so scale never turns this
into an infinite feed.”

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

## Fast recovery

- Empty database: restart with `--fixture`.
- Missing API key: use `--replay`; the app also falls back automatically.
- Window trouble: run `cargo run -- --fixture top` and narrate the same data
  story from the CLI.
- Before every rehearsal: remove or choose a disposable `--database` path,
  because `--fixture` intentionally replaces that database's data.
