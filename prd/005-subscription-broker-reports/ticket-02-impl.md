# Implementation Report: Ticket 2 -- Broker Struct with Publish, Subscribe, and Arc Sharing

**Ticket:** 2 - Broker Struct with Publish, Subscribe, and Arc Sharing
**Date:** 2026-02-26 12:00
**Status:** COMPLETE

---

## Files Changed

### Created
- `src/broker.rs` - `Broker` struct wrapping `tokio::broadcast::Sender<Arc<RecordedEvent>>`, with `new`, `publish`, `subscribe` methods and 4 unit tests

### Modified
- `src/lib.rs` - Added `pub mod broker;` and `pub use broker::Broker;` for crate-root accessibility

## Implementation Notes
- `Broker` has a single private field `tx: broadcast::Sender<Arc<RecordedEvent>>` as specified
- `Broker::new(capacity)` discards the initial receiver from `broadcast::channel` since subscribers obtain their own via `subscribe()`
- `Broker::publish` wraps each event in `Arc::new(event.clone())` before sending, so all subscribers share the same heap allocation
- When no receivers are active, `tx.send()` returns an error; this is logged via `tracing::warn!` rather than propagated as an error, matching the convention that publishing to an empty channel is expected during startup
- All public items have doc comments per CLAUDE.md conventions
- No `.unwrap()` in library code; tests use `.expect()` with descriptive messages

## Acceptance Criteria
- [x] AC: `Broker` is a `pub` struct with private field `tx: broadcast::Sender<Arc<RecordedEvent>>` - implemented in `src/broker.rs:25-27`
- [x] AC: `Broker::new(capacity)` constructs broadcast channel and returns `Broker` - implemented at `src/broker.rs:36-41`
- [x] AC: `Broker::publish(&self, events: &[RecordedEvent])` wraps in Arc, sends, warns on no receivers - implemented at `src/broker.rs:54-61`
- [x] AC: `Broker::subscribe(&self)` returns `broadcast::Receiver<Arc<RecordedEvent>>` - implemented at `src/broker.rs:72-74`
- [x] Test AC-1: publish 3 events, recv all 3 with correct event_type values - `publish_three_events_received_by_subscriber`
- [x] Test AC-2: two subscribers each receive all 2 published events - `multiple_subscribers_each_receive_all_events`
- [x] Test AC-3: capacity 2, publish 5 without recv, get `RecvError::Lagged` - `lagged_subscriber_receives_lagged_error`
- [x] Test AC-12: `Arc::ptr_eq` confirms both subscribers share the same allocation - `arc_sharing_across_subscribers`
- [x] `Broker` accessible at crate root via `pub use broker::Broker;` in `lib.rs`
- [x] Quality gates pass: build, clippy, fmt, test

## Test Results
- Lint: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings`)
- Tests: PASS (120 total: 4 new broker tests + 116 existing)
- Build: PASS (zero warnings)
- Format: PASS (`cargo fmt --check`)
- New tests added:
  - `src/broker.rs::tests::publish_three_events_received_by_subscriber` (AC-1)
  - `src/broker.rs::tests::multiple_subscribers_each_receive_all_events` (AC-2)
  - `src/broker.rs::tests::lagged_subscriber_receives_lagged_error` (AC-3)
  - `src/broker.rs::tests::arc_sharing_across_subscribers` (AC-12)

## Concerns / Blockers
- None
