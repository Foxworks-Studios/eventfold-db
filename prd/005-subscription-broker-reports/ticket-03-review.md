# Code Review: Ticket 3 -- Writer Integration: Broker Receives Published Events After Each Append

**Ticket:** 3 -- Writer Integration: Broker Receives Published Events After Each Append
**Impl Report:** prd/005-subscription-broker-reports/ticket-03-impl.md
**Date:** 2026-02-26 16:00
**Verdict:** APPROVED

---

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 | `run_writer(store, rx, broker)` has broker parameter, is `pub(crate) async fn` | Met | `src/writer.rs` line 131-134: signature is `pub(crate) async fn run_writer(mut store: Store, mut rx: Receiver<AppendRequest>, broker: Broker)` |
| 2 | After `store.append()` returns `Ok(recorded)`, calls `broker.publish(&recorded)` before `response_tx.send()` | Met | `src/writer.rs` lines 147-158: `if let Ok(ref recorded) = result { broker.publish(recorded); }` precedes `req.response_tx.send(result)` |
| 3 | `spawn_writer(store, channel_capacity, broker)` has broker parameter | Met | `src/writer.rs` line 187-190: `pub fn spawn_writer(store: Store, channel_capacity: usize, broker: Broker)` |
| 4 | All existing writer tests updated to pass `Broker::new(64)` | Met | All 15 existing `spawn_writer` call sites in the `#[cfg(test)]` module now include `crate::broker::Broker::new(64)` as the third argument. No test logic changed. |
| 5 | Test (AC-13): writer-to-broker integration verified | Met | `ac13_writer_publishes_to_broker` (line 781): creates broker, subscribes, spawns writer, appends one event, asserts `rx.recv()` returns matching `event_type` "BrokerEvent" |
| 6 | Test: 3 events arrive in global-position order | Met | `broker_receives_three_events_in_order` (line 810): appends 3 events sequentially, asserts broadcast receiver gets positions 0, 1, 2 in order |
| 7 | Test: failed append does NOT publish to broker | Met | `failed_append_does_not_publish_to_broker` (line 847): succeeds once, drains receiver, triggers `WrongExpectedVersion`, asserts `try_recv()` returns `Err(TryRecvError::Empty)` |
| 8 | Quality gates pass | Met | Verified locally: `cargo build` (zero warnings), `cargo clippy --all-targets --all-features --locked -- -D warnings` (clean), `cargo test` (124 total: 123 lib + 1 integration, all pass), `cargo fmt --check` (clean) |

## Issues Found

### Critical (must fix before merge)

None.

### Major (should fix, risk of downstream problems)

None.

### Minor (nice to fix, not blocking)

None.

## Suggestions (non-blocking)

- The `use std::sync::Arc;` import at line 783 in `ac13_writer_publishes_to_broker` is used only for the type annotation `let received: Arc<RecordedEvent>`. This annotation is not strictly necessary (the type is inferred), but it does serve as documentation that the broker delivers `Arc`-wrapped events -- reasonable to keep for readability.

## Scope Check

- Files within scope: YES -- only `src/writer.rs` and `tests/writer_integration.rs` were modified, matching the ticket scope exactly.
- Scope creep detected: NO
- Unauthorized dependencies added: NO

## Risk Assessment

- Regression risk: LOW -- All 15 existing writer tests were updated mechanically (only the `spawn_writer` call site gained the broker argument) and continue to pass. The broker is a purely additive integration point; it does not alter the existing append/response flow.
- Security concerns: NONE
- Performance concerns: NONE -- `broker.publish()` calls `Arc::new(event.clone())` per event. The clone is of `RecordedEvent` (which contains `Bytes` fields that are reference-counted internally), and this occurs once per successful append, which is the expected cost per the CLAUDE.md design ("Use `Arc<RecordedEvent>` in the broadcast channel to share the allocation instead of cloning event data").

## Notes

The implementation is clean and minimal. Key correctness properties verified:

1. **Ordering**: `broker.publish()` is called after `store.append()` succeeds but before `response_tx.send()`, ensuring subscribers see events in disk-write order and before callers receive their response.
2. **Failure isolation**: The `if let Ok(ref recorded) = result` guard ensures failed appends never publish to the broker.
3. **Ownership**: `Broker` is moved into the writer task (same pattern as `Store`), which is correct since there should be exactly one publisher. Subscribers obtain receivers via `broker.subscribe()` before the move.
4. **Test pattern**: The new tests correctly subscribe to the broker before moving it into `spawn_writer`, which works because the `broadcast::Receiver` is independent of the `Broker` struct's lifetime -- the underlying broadcast channel stays alive as long as any sender or receiver exists.
