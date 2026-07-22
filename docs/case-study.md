# Community Pulse case-study brief

## Problem

A healthy community emits more potentially useful information than a person
can evaluate. A naive activity feed makes discovery worse as the community
grows: volume rises, stale items linger, and personalization becomes an opaque
filter bubble.

## Smallest credible product

The prototype treats interface choice as a lens over a deterministic engine:

1. ingest real public community sources;
2. extract explainable topics;
3. score change relative to a baseline;
4. apply explicit user weights; and
5. stop at five cards.

Chat is the primary agent surface because it demonstrates tool use and shared
state without introducing microphone, resampling, VAD, echo cancellation, or
meeting-audio routing. Voice can use the same four tools later.

## Product decisions made concrete

- **Doesn't overwhelm:** a hard five-card attention budget, tested in the
  engine and CLI.
- **Doesn't go stale:** sliding time windows and snapshots; recent velocity
  decays out of the score naturally.
- **Earns trust:** every card opens the quantitative baseline and source posts.
- **User control:** interests are visible positive or negative weights, not a
  hidden profile.
- **Demo reliability:** real adapters prove the data path; fixture and replay
  make the live story independent of interview Wi-Fi.
- **One state:** agent calls and pointer actions cross the same bridge and update
  the same `lazily` graph.

## What the prototype proves

The working artifact proves ingestion boundaries, normalized persistence,
deterministic scoring, capped personalization, evidence retrieval, direct
manipulation, streaming compatible tool calls, and a complete offline demo.

It deliberately does not claim that title/tag extraction is a production
ontology or that one SQLite process is a 100k-user architecture. Those are
replaceable adapters behind already-tested product semantics.
