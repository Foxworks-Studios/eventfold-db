# PRD 005: Subscription Broker

**Status:** TICKETS READY

## Summary

Implement the subscription broker that enables catch-up-then-live subscriptions. The broker uses a `tokio::broadcast` channel to push newly appended events to all active subscribers. Subscribers receive historical events from the in-memory index (catch-up phase), a `CaughtUp` marker, and then live events from the broadcast channel (live phase). This PRD covers both `SubscribeAll` and `SubscribeStream` semantics.

## Motivation

Subscriptions are the mechanism by which external projection services build read models. The catch-up-then-live pattern ensures no events are missed during the transition from historical replay to live push. This is the most subtle correctness challenge in EventfoldDB -- the broadcast subscription must be registered *before* the catch-up read begins, and deduplication must handle the overlap window.

## Scope

### In scope

- `broker.rs`: The `Broker` struct, which wraps a `tokio::broadcast::Sender<Arc<RecordedEvent>>`.
- Integration with the writer task: after each successful append, the writer publishes new events to the broker.
- `SubscribeAll` logic: catch-up from a global position, `CaughtUp` marker, then live events.
- `SubscribeStream` logic: catch-up from a stream version, `CaughtUp` marker, then live events filtered by stream ID.
- A `SubscriptionMessage` enum: `Event(Arc<RecordedEvent>)` or `CaughtUp`.
- Deduplication during the catch-up-to-live transition.
- Lag detection: if a subscriber's broadcast receiver lags (buffer overflow), the subscription terminates.

### Out of scope

- gRPC streaming serialization (PRD 006 wraps the subscription stream into a gRPC response stream).
- Server startup (PRD 007).

## Detailed Design

### `SubscriptionMessage`

```rust
#[derive(Debug, Clone)]
pub enum SubscriptionMessage {
    Event(Arc<RecordedEvent>),
    CaughtUp,
}
```

### `Broker`

```rust
pub struct Broker {
    tx: broadcast::Sender<Arc<RecordedEvent>>,
}

impl Broker {
    pub fn new(capacity: usize) -> Self { ... }

    /// Publish events to all subscribers. Called by the writer task after fsync.
    pub fn publish(&self, events: &[RecordedEvent]) { ... }

    /// Create a new broadcast receiver.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<RecordedEvent>> { ... }
}
```

The broker wraps each `RecordedEvent` in `Arc` before sending to the broadcast channel, so all subscribers share the same allocation rather than cloning event data.

### Writer Integration

The writer task (from PRD 004) receives a `Broker` handle. After each successful append + fsync + index update, it calls `broker.publish(&recorded_events)`. This is added as a modification to `run_writer`.

### `subscribe_all`

A function (or method on a controller struct) that produces an async stream of `SubscriptionMessage`:

```rust
pub async fn subscribe_all(
    read_index: ReadIndex,
    broker: Broker,
    from_position: u64,
) -> impl Stream<Item = Result<SubscriptionMessage, Error>>
```

Sequence:
1. Subscribe to the broadcast channel (register the receiver).
2. Read historical events from `read_index.read_all(from_position, ...)` in batches, yielding each as `SubscriptionMessage::Event`.
3. Record the last global position sent during catch-up (`last_catchup_position`).
4. Yield `SubscriptionMessage::CaughtUp`.
5. Drain the broadcast receiver. For each received event:
   - If `event.global_position <= last_catchup_position`, skip (deduplication).
   - Otherwise, yield as `SubscriptionMessage::Event`.
6. If the broadcast receiver returns `RecvError::Lagged`, terminate the stream with an error (or simply end the stream). The client must re-subscribe.

### `subscribe_stream`

Same mechanics as `subscribe_all`, but scoped to a single stream:

```rust
pub async fn subscribe_stream(
    read_index: ReadIndex,
    broker: Broker,
    stream_id: Uuid,
    from_version: u64,
) -> impl Stream<Item = Result<SubscriptionMessage, Error>>
```

Differences:
- Catch-up reads from `read_index.read_stream(stream_id, from_version, ...)`.
- The deduplication boundary is the last `stream_version` sent during catch-up.
- In the live phase, events from the broadcast channel are filtered: only events matching `stream_id` are yielded.

### Catch-up batching

During catch-up, events should be read in batches (e.g., 500 at a time) rather than loading the entire history into memory at once. This keeps memory bounded for large catch-up ranges.

## Acceptance Criteria

### AC-1: Broker publish and receive

- **Test**: Create a broker. Subscribe. Publish 3 events. Receive all 3 from the subscriber. Events are wrapped in `Arc`.

### AC-2: Multiple subscribers

- **Test**: Create a broker. Subscribe twice. Publish 2 events. Both subscribers receive both events.

### AC-3: Lagged subscriber

- **Test**: Create a broker with capacity 2. Subscribe. Publish 5 events without receiving. The receiver returns a lagged error.

### AC-4: subscribe_all -- catch-up only

- **Test**: Append 5 events to the store. Call `subscribe_all(from_position=0)`. Receive all 5 events, then `CaughtUp`. No more events arrive (the stream is idle, not terminated).

### AC-5: subscribe_all -- catch-up from middle

- **Test**: Append 10 events. Call `subscribe_all(from_position=5)`. Receive events at positions 5..9, then `CaughtUp`.

### AC-6: subscribe_all -- live events after catch-up

- **Test**: Append 3 events. Start `subscribe_all(from_position=0)`. Drain until `CaughtUp`. Then append 2 more events via the writer. Receive both new events from the subscription stream. Global positions are contiguous with no gaps.

### AC-7: subscribe_all -- no duplicates during transition

- **Test**: Start `subscribe_all(from_position=0)` on an empty store. Immediately append 5 events (they may arrive during catch-up or live). Drain the subscription. Verify exactly 5 unique events are received (no duplicates), followed by `CaughtUp` at some point in the sequence.

### AC-8: subscribe_stream -- catch-up

- **Test**: Append events to streams A, B, A, B, A. Call `subscribe_stream(stream_id=A, from_version=0)`. Receive only stream A's 3 events, then `CaughtUp`.

### AC-9: subscribe_stream -- live filtering

- **Test**: Start `subscribe_stream(stream_id=A, from_version=0)` on an empty store. Append to stream B, then stream A, then stream B. Only stream A's event appears in the subscription after `CaughtUp`.

### AC-10: subscribe_stream -- non-existent stream

- **Test**: Call `subscribe_stream` for a stream that does not exist with `from_version=0`. Receive `CaughtUp` immediately (empty catch-up). Then append to that stream. The event appears live.

### AC-11: Subscription termination on lag

- **Test**: Create a broker with small capacity (e.g., 4). Start `subscribe_all(from_position=0)`. Append many events without draining the subscription. The stream terminates (yields an error or ends).

### AC-12: Arc sharing (no deep clone)

- **Test**: Publish an event through the broker. Receive it from two subscribers. Both `Arc<RecordedEvent>` point to the same allocation (`Arc::ptr_eq` returns true).

### AC-13: Writer publishes to broker

- **Test**: Spawn a writer with a broker. Append via `WriterHandle`. A broadcast subscriber receives the event. This verifies the writer-broker integration.

### AC-14: Build and lint

- `cargo build` completes with zero warnings.
- `cargo clippy --all-targets --all-features --locked -- -D warnings` passes.
- `cargo fmt --check` passes.
- `cargo test` passes with all tests green.

## Dependencies

- **Depends on**: PRD 001 (types), PRD 003 (store/read index), PRD 004 (writer task).
- **Depended on by**: PRD 006, 007.

## Cargo.toml Additions

```toml
[dependencies]
async-stream = "0.3"
# tokio already added in PRD 004 (broadcast is in tokio::sync)
```
