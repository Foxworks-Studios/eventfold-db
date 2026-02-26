# Tickets for PRD 005: Subscription Broker

**Source PRD:** prd/005-subscription-broker.md
**Created:** 2026-02-26
**Total Tickets:** 6
**Estimated Total Complexity:** 11 (S=1, M=2, M=2, M=2, M=2, M=2)

---

### Ticket 1: Add `async-stream` Dependency and `SubscriptionMessage` Type

**Description:**
Add `async-stream = "0.3"` to `Cargo.toml` and extend `src/types.rs` with the
`SubscriptionMessage` enum. This is the foundational deliverable that every other ticket
in this PRD builds on -- the enum is yielded by both `subscribe_all` and `subscribe_stream`,
and `async-stream` is the macro crate used to construct those streams.

**Scope:**
- Modify: `Cargo.toml` (add `async-stream = "0.3"` under `[dependencies]`)
- Modify: `src/types.rs` (add `SubscriptionMessage` enum and its tests)

**Acceptance Criteria:**
- [ ] `Cargo.toml` `[dependencies]` contains `async-stream = "0.3"`
- [ ] `SubscriptionMessage` is a `pub` enum in `src/types.rs` with exactly two variants:
  `Event(std::sync::Arc<RecordedEvent>)` and `CaughtUp`
- [ ] `SubscriptionMessage` derives `Debug` and `Clone`
- [ ] Test: construct `SubscriptionMessage::Event(Arc::new(recorded_event))` and
  `SubscriptionMessage::CaughtUp`; `format!("{:?}", msg)` returns a non-empty string for
  both variants (confirms `Debug` derive)
- [ ] Test: clone a `SubscriptionMessage::Event` variant; the cloned value's inner `Arc` and
  the original inner `Arc` satisfy `Arc::ptr_eq` -- i.e., cloning the message does not deep-
  clone the `RecordedEvent` allocation
- [ ] Test: `use async_stream::stream;` compiles successfully in a `#[cfg(test)]` block,
  confirming the dependency is present and accessible
- [ ] Quality gates pass: `cargo build`, `cargo clippy --all-targets --all-features --locked -- -D warnings`, `cargo fmt --check`, `cargo test`

**Dependencies:** None
**Complexity:** S
**Maps to PRD AC:** AC-1 (partial: type prerequisite), AC-12 (partial: Arc semantics confirmed
for the message wrapper)

---

### Ticket 2: `Broker` Struct with Publish, Subscribe, and Arc Sharing

**Description:**
Create `src/broker.rs` containing the `Broker` struct, which wraps a
`tokio::broadcast::Sender<Arc<RecordedEvent>>`. Implement `Broker::new`, `Broker::publish`,
and `Broker::subscribe`. Register the module in `lib.rs` and re-export `Broker`. This ticket
covers the raw publish/subscribe mechanics (ACs 1, 2, 3, 12) before any catch-up logic is
added.

**Scope:**
- Create: `src/broker.rs` (`Broker` struct, `new`, `publish`, `subscribe`, and unit tests)
- Modify: `src/lib.rs` (add `pub mod broker;` and `pub use broker::Broker;`)

**Acceptance Criteria:**
- [ ] `Broker` is a `pub` struct in `src/broker.rs` with a single private field
  `tx: tokio::sync::broadcast::Sender<std::sync::Arc<crate::types::RecordedEvent>>`
- [ ] `Broker::new(capacity: usize) -> Broker` constructs the broadcast channel with the
  given capacity and returns a `Broker` wrapping the sender
- [ ] `Broker::publish(&self, events: &[RecordedEvent])` wraps each event in `Arc::new` and
  calls `self.tx.send(arc_event)`, logging a `tracing::warn!` if there are no active
  receivers (a send error when no subscribers exist is not a fatal error)
- [ ] `Broker::subscribe(&self) -> tokio::sync::broadcast::Receiver<Arc<RecordedEvent>>`
  calls `self.tx.subscribe()` and returns the receiver
- [ ] Test (AC-1): create a `Broker::new(16)`, call `broker.subscribe()` to get `rx`, call
  `broker.publish(&[event_a, event_b, event_c])`, then `rx.recv().await` three times and
  collect the results -- assert all three `Arc<RecordedEvent>` values arrive and have the
  expected `event_type` values
- [ ] Test (AC-2): create a `Broker::new(16)`, subscribe twice to get `rx1` and `rx2`,
  publish 2 events; assert both `rx1` and `rx2` each receive exactly 2 events
- [ ] Test (AC-3): create a `Broker::new(2)`, subscribe to get `rx`, publish 5 events without
  receiving; assert `rx.recv().await` eventually returns
  `Err(tokio::sync::broadcast::error::RecvError::Lagged(_))`
- [ ] Test (AC-12): create a `Broker::new(16)`, subscribe twice to get `rx1` and `rx2`,
  publish 1 event; receive the `Arc<RecordedEvent>` from both; assert
  `Arc::ptr_eq(&arc_from_rx1, &arc_from_rx2)` is true (same heap allocation, no deep clone)
- [ ] `Broker` and `pub use broker::Broker` are accessible at the crate root
- [ ] Quality gates pass: `cargo build`, `cargo clippy --all-targets --all-features --locked -- -D warnings`, `cargo fmt --check`, `cargo test`

**Dependencies:** Ticket 1
**Complexity:** M
**Maps to PRD AC:** AC-1, AC-2, AC-3, AC-12

---

### Ticket 3: Writer Integration -- Broker Receives Published Events After Each Append

**Description:**
Modify `src/writer.rs` so that `run_writer` and `spawn_writer` accept a `Broker` argument.
After each successful append + fsync + index update inside the writer task loop, call
`broker.publish(&recorded_events)` to push newly written events to all subscribers. This is
the integration seam between the write path and the subscription path.

**Scope:**
- Modify: `src/writer.rs` (`run_writer` and `spawn_writer` signatures and bodies; update all
  callers in the module's own `#[cfg(test)]` block)

**Acceptance Criteria:**
- [ ] `run_writer(store: Store, rx: Receiver<AppendRequest>, broker: Broker)` is `pub(crate)`
  `async fn` -- the `broker` parameter is added; all other behavior is unchanged
- [ ] Inside the per-request processing loop, after `store.append(...)` returns `Ok(recorded)`,
  call `broker.publish(&recorded)` before sending the response via `response_tx`
- [ ] `spawn_writer(store: Store, channel_capacity: usize, broker: Broker) -> (WriterHandle, ReadIndex, JoinHandle<()>)` -- the `broker` parameter is added; the broker is moved into the spawned task via `run_writer`
- [ ] All existing writer tests in `src/writer.rs` are updated to pass a `Broker::new(64)` to
  `spawn_writer` -- no test logic changes, only the call site gains the broker argument
- [ ] Test (AC-13): `#[tokio::test]` -- create a `Broker::new(64)`, call `broker.subscribe()`
  to get `rx`, call `spawn_writer(store, 8, broker_clone_or_same)`, append one event via
  `WriterHandle::append`; assert `rx.recv().await` returns an `Arc<RecordedEvent>` whose
  `event_type` matches the appended event (writer-to-broker integration path verified)
- [ ] Test: append 3 events sequentially; assert the broadcast receiver receives exactly 3
  `Arc<RecordedEvent>` values in global-position order (0, 1, 2)
- [ ] Test: an append that returns `Err` (e.g., `WrongExpectedVersion`) does NOT publish to
  the broker -- subscribe before the failing append and assert `rx.try_recv()` returns
  `Err(TryRecvError::Empty)` after the failed append
- [ ] Quality gates pass: `cargo build`, `cargo clippy --all-targets --all-features --locked -- -D warnings`, `cargo fmt --check`, `cargo test`

**Dependencies:** Ticket 2
**Complexity:** M
**Maps to PRD AC:** AC-13

---

### Ticket 4: `subscribe_all` -- Catch-Up, CaughtUp Marker, Live Events, and Lag Termination

**Description:**
Add `pub async fn subscribe_all` to `src/broker.rs`. The function returns an
`impl Stream<Item = Result<SubscriptionMessage, Error>>` using the `async_stream::stream!`
macro. It implements the full catch-up-then-live sequence: register the broadcast receiver
before reading history, replay historical events in 500-event batches, emit `CaughtUp`, then
forward live events from the broadcast channel with deduplication and lag termination.

**Scope:**
- Modify: `src/broker.rs` (add `subscribe_all` and its unit tests)
- Modify: `src/lib.rs` (re-export `subscribe_all`)

**Acceptance Criteria:**
- [ ] `subscribe_all(read_index: ReadIndex, broker: &Broker, from_position: u64) -> impl Stream<Item = Result<SubscriptionMessage, Error>>` is a `pub async fn` in `src/broker.rs`
- [ ] Step 1 of implementation: `broker.subscribe()` is called to register the broadcast
  receiver **before** any historical read begins
- [ ] Step 2: historical events are read via `read_index.read_all(cursor, CATCHUP_BATCH_SIZE)`
  in a loop; each event is yielded as `SubscriptionMessage::Event(Arc::new(event))`; the
  constant `CATCHUP_BATCH_SIZE: u64 = 500` is defined at module scope
- [ ] Step 3: `SubscriptionMessage::CaughtUp` is yielded once when the historical read
  returns fewer events than `CATCHUP_BATCH_SIZE` (i.e., the head of the log has been reached)
- [ ] Step 4: the broadcast receiver is drained; events with
  `global_position <= last_catchup_position` are skipped (deduplication window); all others
  are yielded as `SubscriptionMessage::Event(arc_event)`
- [ ] Step 5: if `rx.recv().await` returns
  `Err(tokio::sync::broadcast::error::RecvError::Lagged(_))`, yield
  `Err(Error::InvalidArgument("subscription lagged: re-subscribe from last checkpoint".into()))`
  and terminate the stream
- [ ] Test (AC-4): `#[tokio::test]` -- spawn writer with broker; append 5 events; call
  `subscribe_all(read_index, &broker, 0)`; collect messages until `CaughtUp` via
  `futures::StreamExt::next`; assert exactly 5 `SubscriptionMessage::Event` variants were
  received before `CaughtUp`, in global-position order 0..4
- [ ] Test (AC-5): append 10 events; call `subscribe_all(read_index, &broker, 5)`; collect
  until `CaughtUp`; assert exactly 5 events received with global positions 5..9
- [ ] Test (AC-6): `#[tokio::test]` -- spawn writer with broker; append 3 events; start
  `subscribe_all(from_position=0)`; drive the stream until `CaughtUp`; then append 2 more
  events via `WriterHandle`; drive the stream for 2 more messages; assert both are
  `SubscriptionMessage::Event` with global positions 3 and 4 (no gap, no duplicate)
- [ ] Test (AC-7): `#[tokio::test]` -- start `subscribe_all(from_position=0)` on an empty
  store; concurrently append 5 events; collect all messages until `CaughtUp` is received;
  assert exactly 5 unique `Event` variants are present (deduplicated) with no repeated
  `global_position` values
- [ ] Test (AC-11): `#[tokio::test]` -- create broker with capacity 4; call
  `subscribe_all(from_position=0)` on an empty store; without polling the stream, append 10
  events via the writer (saturates the broadcast buffer); poll the stream; assert it
  eventually yields `Err(Error::InvalidArgument(_))` and then returns `None` (stream ends)
- [ ] `subscribe_all` is accessible at the crate root via `eventfold_db::subscribe_all`
- [ ] `futures` added to `[dev-dependencies]` in `Cargo.toml` if not already present (needed
  for `StreamExt::next` in tests)
- [ ] Quality gates pass: `cargo build`, `cargo clippy --all-targets --all-features --locked -- -D warnings`, `cargo fmt --check`, `cargo test`

**Dependencies:** Tickets 2, 3
**Complexity:** M
**Maps to PRD AC:** AC-4, AC-5, AC-6, AC-7, AC-11

---

### Ticket 5: `subscribe_stream` -- Stream-Scoped Catch-Up, Filtering, and Non-Existent Stream

**Description:**
Add `pub async fn subscribe_stream` to `src/broker.rs`. It follows the same catch-up-then-live
mechanics as `subscribe_all` but is scoped to a single stream: historical catch-up reads from
`read_index.read_stream(stream_id, ...)`, deduplication uses the last `stream_version` yielded
during catch-up, and the live phase filters the broadcast channel to only forward events that
match `stream_id`. A non-existent stream produces an immediate `CaughtUp` with no history.

**Scope:**
- Modify: `src/broker.rs` (add `subscribe_stream` and its unit tests)
- Modify: `src/lib.rs` (re-export `subscribe_stream`)

**Acceptance Criteria:**
- [ ] `subscribe_stream(read_index: ReadIndex, broker: &Broker, stream_id: Uuid, from_version: u64) -> impl Stream<Item = Result<SubscriptionMessage, Error>>` is a `pub async fn` in `src/broker.rs`
- [ ] The broadcast receiver is registered via `broker.subscribe()` **before** the historical
  read begins (same ordering invariant as `subscribe_all`)
- [ ] Historical catch-up reads `read_index.read_stream(stream_id, cursor, CATCHUP_BATCH_SIZE)`;
  if the stream does not exist (`Err(Error::StreamNotFound { .. })`), the catch-up phase
  terminates immediately and `SubscriptionMessage::CaughtUp` is yielded
- [ ] After catch-up, `last_catchup_version` is tracked (the last `stream_version` yielded, or
  `u64::MAX` sentinel if no events were yielded); in the live phase, broadcast events are
  filtered to only those where `event.stream_id == stream_id` AND
  `event.stream_version > last_catchup_version`
- [ ] Lag termination is identical to `subscribe_all`: `RecvError::Lagged` yields
  `Err(Error::InvalidArgument("subscription lagged: re-subscribe from last checkpoint".into()))`
  and ends the stream
- [ ] Test (AC-8): `#[tokio::test]` -- append events to streams A, B, A, B, A (interleaved);
  call `subscribe_stream(stream_id=A, from_version=0)`; collect until `CaughtUp`; assert
  exactly 3 `Event` variants received, each with `stream_id == A` and `stream_version` 0, 1, 2
  respectively
- [ ] Test (AC-9): `#[tokio::test]` -- start `subscribe_stream(stream_id=A, from_version=0)` on
  an empty store; drive until `CaughtUp`; then via the writer append to stream B, stream A,
  stream B (3 appends total); drive the stream for more messages; assert exactly 1 `Event`
  arrives (stream A's event), with no events for stream B appearing
- [ ] Test (AC-10): `#[tokio::test]` -- call `subscribe_stream` for a UUID that has no events,
  `from_version=0`; collect until `CaughtUp`; assert zero `Event` variants preceded `CaughtUp`
  (empty catch-up); then append one event to that stream via the writer; drive the stream;
  assert the event arrives as `SubscriptionMessage::Event` with `stream_version == 0`
- [ ] `subscribe_stream` is accessible at the crate root via `eventfold_db::subscribe_stream`
- [ ] Quality gates pass: `cargo build`, `cargo clippy --all-targets --all-features --locked -- -D warnings`, `cargo fmt --check`, `cargo test`

**Dependencies:** Tickets 2, 3, 4
**Complexity:** M
**Maps to PRD AC:** AC-8, AC-9, AC-10

---

### Ticket 6: Verification and Integration

**Description:**
Run the complete PRD 005 acceptance criteria checklist end-to-end. Add a
`tests/broker_integration.rs` integration test that exercises the full public subscription
API from outside the crate -- spawning the writer with a broker, appending events, and
consuming them through `subscribe_all` and `subscribe_stream`. Confirm no regressions in
PRDs 001-004 tests and that all quality gates pass clean.

**Scope:**
- Create: `tests/broker_integration.rs` (integration tests for the full subscription flow)
- Modify: `src/lib.rs` (confirm re-exports of `Broker`, `SubscriptionMessage`, `subscribe_all`,
  `subscribe_stream` are present and accessible; add if any were missed)

**Acceptance Criteria:**
- [ ] All PRD 005 ACs (AC-1 through AC-14) verified by passing `cargo test` output
- [ ] `cargo test` output shows zero failures across the full crate (PRDs 001-005 tests all green)
- [ ] `cargo build` completes with zero warnings
- [ ] `cargo clippy --all-targets --all-features --locked -- -D warnings` passes with zero diagnostics
- [ ] `cargo fmt --check` passes
- [ ] `Broker`, `SubscriptionMessage`, `subscribe_all`, `subscribe_stream` are all accessible
  at the crate root via `eventfold_db::` paths
- [ ] Test (integration): import `Broker`, `SubscriptionMessage`, `subscribe_all`,
  `spawn_writer`, `Store` from `eventfold_db`; open a store in a tempdir; create a
  `Broker::new(64)`; spawn the writer; append 3 events to stream X and 2 events to stream Y;
  collect `subscribe_all(from_position=0)` until `CaughtUp`; assert 5 events received in
  global-position order; then collect `subscribe_stream(stream_id=X, from_version=0)` until
  `CaughtUp`; assert 3 events received, all with `stream_id == X`
- [ ] Test (integration): verify `SubscriptionMessage::CaughtUp` is yielded before any live
  events when starting `subscribe_all` on a store with pre-existing history -- i.e., catch-up
  events arrive as `Event`, then `CaughtUp`, then silence until more are appended

**Dependencies:** Tickets 1, 2, 3, 4, 5
**Complexity:** M
**Maps to PRD AC:** AC-14

---

## AC Coverage Matrix

| PRD AC # | Description                                                            | Covered By Ticket(s) | Status  |
|----------|------------------------------------------------------------------------|----------------------|---------|
| AC-1     | Broker publish and receive: 3 events reach 1 subscriber               | Ticket 2             | Covered |
| AC-2     | Multiple subscribers both receive all published events                 | Ticket 2             | Covered |
| AC-3     | Lagged subscriber receives `RecvError::Lagged` after buffer overflow   | Ticket 2             | Covered |
| AC-4     | `subscribe_all` catch-up only: 5 pre-existing events then `CaughtUp`  | Ticket 4             | Covered |
| AC-5     | `subscribe_all` catch-up from middle: events at positions 5..9         | Ticket 4             | Covered |
| AC-6     | `subscribe_all` live events after catch-up: 2 new events arrive        | Ticket 4             | Covered |
| AC-7     | `subscribe_all` no duplicates during transition window                 | Ticket 4             | Covered |
| AC-8     | `subscribe_stream` catch-up: only stream A events, in order            | Ticket 5             | Covered |
| AC-9     | `subscribe_stream` live filtering: only stream A events live           | Ticket 5             | Covered |
| AC-10    | `subscribe_stream` non-existent stream: immediate `CaughtUp` then live | Ticket 5             | Covered |
| AC-11    | Subscription termination on lag: stream ends with error                | Ticket 4             | Covered |
| AC-12    | Arc sharing: two subscribers point to the same allocation              | Ticket 2             | Covered |
| AC-13    | Writer publishes to broker: broadcast receiver gets the event          | Ticket 3             | Covered |
| AC-14    | Build and lint: all quality gates pass                                 | Ticket 6             | Covered |
