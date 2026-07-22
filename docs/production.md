# Production and scale path

## 100k+ users

Trend scores are community-shared. Compute them once per source/window using a
stream processor or scheduled job, retain the top few hundred candidates, and
store immutable snapshots in a columnar store. Do not recalculate velocity per
user.

At read time, load a compact interest vector from a KV store, rerank the shared
candidate set, apply policy filters and decay, and return five. This remains
cheap enough for fan-out-on-read and preserves a single definition of “moving.”

SQLite becomes Postgres for normalized operational records; an analytical or
streaming system owns high-volume windows. The `PulseEngine` boundary survives:
its query implementation changes, while the tool and UI contracts do not.

## Freshness and overload

- event-time watermarks handle delayed source events;
- idempotent source ids make retries safe;
- per-source health prevents one failed adapter from suppressing the rest;
- exponential decay removes stale trends even when no replacement arrives;
- the five-card cap is enforced after policy and personalization;
- source diversity remains a ranking feature and a visible trust signal.

## Mobile

Slint supports Rust applications on Android and iOS. Move the engine behind a
service client while retaining the generated UI models and shared tool contract.
For intermittent connectivity, cache the last digest and evidence, queue
interest/subscription mutations, and reconcile them by idempotency key.

## Voice lens

Voice is additive after the visual/chat product is reliable. A direct realtime
WebSocket is adequate for a single-client demo; production should put WebRTC at
the edge for echo control, reconnection, and mobile behavior, with horizontally
scaled agent workers behind the media plane. Both paths invoke the existing
tool schemas, so voice does not fork ranking or user state.

Voice minutes are materially more expensive than digest reads. Keep UI, chat,
and notification delivery first-class rather than making speech the only way to
receive value.
