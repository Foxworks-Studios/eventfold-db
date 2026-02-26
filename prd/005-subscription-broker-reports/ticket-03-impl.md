# Implementation Report: Ticket 3 -- Writer Integration: Broker Receives Published Events After Each Append

**Ticket:** 3 - Writer Integration: Broker Receives Published Events After Each Append
**Date:** 2026-02-26 15:30
**Status:** COMPLETE

---

## Files Changed

### Modified
- `src/writer.rs` - Added `Broker` parameter to `run_writer` and `spawn_writer`; added `broker.publish()` call after successful appends; updated all existing test call sites; added 3 new broker integration tests
- `tests/writer_integration.rs` - Added `Broker` import and `Broker::new(64)` parameter to `spawn_writer` call

## Implementation Notes
- The `Broker` is passed by value (moved) into `run_writer` and then into the spawned task via `spawn_writer`. This follows the existing ownership pattern where `Store` is also moved into the writer task.
- `broker.publish(&recorded)` is called after `store.append()` returns `Ok(ref recorded)` but before `response_tx.send(result)`. This ensures subscribers see events in disk-write order and before callers receive their responses. The `if let Ok(ref recorded)` pattern avoids publishing on failed appends.
- All existing tests were updated mechanically: each `spawn_writer(store, N)` call gained a third argument `Broker::new(64)`. No test logic was changed.
- The integration test in `tests/writer_integration.rs` was updated to import `Broker` from `eventfold_db` and pass `Broker::new(64)` to `spawn_writer`.

## Acceptance Criteria
- [x] AC: `run_writer(store, rx, broker)` is `pub(crate) async fn` - The broker parameter was added as the third argument
- [x] AC: After `store.append()` returns `Ok(recorded)`, `broker.publish(&recorded)` is called before sending the response via `response_tx`
- [x] AC: `spawn_writer(store, channel_capacity, broker)` returns `(WriterHandle, ReadIndex, JoinHandle<()>)` - The broker parameter was added and is moved into the spawned task
- [x] AC: All existing writer tests updated to pass `Broker::new(64)` - 15 existing test call sites updated, no test logic changes
- [x] AC-13 test: `ac13_writer_publishes_to_broker` - Creates broker, subscribes, spawns writer, appends one event, asserts `rx.recv()` returns `Arc<RecordedEvent>` with matching `event_type`
- [x] AC: 3 events in order test: `broker_receives_three_events_in_order` - Appends 3 events sequentially, asserts broadcast receiver receives exactly 3 values with global positions 0, 1, 2
- [x] AC: Failed append does not publish test: `failed_append_does_not_publish_to_broker` - Appends successfully, drains receiver, then triggers `WrongExpectedVersion`, asserts `try_recv()` returns `Err(TryRecvError::Empty)`
- [x] AC: Quality gates pass

## Test Results
- Build: PASS (zero warnings)
- Clippy: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings`)
- Fmt: PASS (`cargo fmt --check`)
- Tests: PASS (124 total: 123 lib + 1 integration)
- New tests added:
  - `src/writer.rs::tests::ac13_writer_publishes_to_broker`
  - `src/writer.rs::tests::broker_receives_three_events_in_order`
  - `src/writer.rs::tests::failed_append_does_not_publish_to_broker`

## Concerns / Blockers
- None
