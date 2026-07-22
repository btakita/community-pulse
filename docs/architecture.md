# Architecture

```text
Slint UI ───────────┐
                    ├── shared ToolBridge ── PulseEngine ── SQLite
Replay agent ───────┤          │                 ▲
Live chat agent ────┘          ▼                 │
                       lazily reactive state     │
                                                │
HN Algolia ─┐                                   │
Lobsters ───┼── normalize + extract topics ─────┘
PH Atom ────┘
```

## Boundaries

`PulseEngine` is synchronous, deterministic, and unaware of Slint or language
models. Its only clock-sensitive methods accept an explicit `DateTime<Utc>`,
which lets tests and the fixture prove window behavior without sleeping.

Ingisters fetch concurrently and return normalized `CommunityPost` values.
SQLite writes then happen in one transaction on the owning thread. A failed
source is reported independently; healthy sources still land.

`ToolBridge` owns the engine behind a mutex and a cloneable `PulseState`. The
state uses `lazily::ThreadSafeContext` because live chat executes on a worker
thread while Slint owns the event-loop thread. Tool operations update persisted
engine state and reactive cells as one bridge operation, then publish an
immutable `UiSnapshot` back through `slint::invoke_from_event_loop`.

## Reactive graph

The source cells are:

- digest cards
- weighted interests
- selected evidence
- tracked and suggested topics
- chat transcript and loading state

The status string is a computed node that reads digest, interests, and tracked
topics. It demonstrates the same invalidation path used for larger derived UI
models without introducing a second state store.

## Agent loop

The live adapter sends streaming OpenAI-compatible chat completions. Content
deltas update the current assistant bubble as they arrive. Tool-call names and
arguments are accumulated by call index because providers can split either
field across SSE chunks.

When a turn contains tool calls, the adapter:

1. adds the assistant tool request to history;
2. invokes the shared bridge;
3. exposes the tool chip in Slint;
4. appends compact JSON as the tool result; and
5. continues the completion.

The loop is capped at four rounds. The replay adapter invokes the identical
bridge and emits the same event types.

## Persistence

SQLite stores normalized posts, many-to-many topic mentions, immutable score
snapshots, interest weights, and subscriptions. WAL mode permits inspection
while the app is open. The fixture intentionally clears only its selected
database so repeated rehearsals are deterministic.
