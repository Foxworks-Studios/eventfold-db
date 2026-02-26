# Implementation Report: Ticket 5 -- `subscribe_stream`: Stream-Scoped Catch-Up, Filtering, and Non-Existent Stream

**Ticket:** 5 - subscribe_stream
**Date:** 2026-02-26 14:30
**Status:** COMPLETE

---

## Files Changed

### Created
- None

### Modified
- `src/broker.rs` - Added `subscribe_stream` function and 3 unit tests (AC-8, AC-9, AC-10)
- `src/lib.rs` - Added `subscribe_stream` to the crate-root re-export

## Implementation Notes
- `subscribe_stream` follows the exact same catch-up-then-live pattern as `subscribe_all`, with three key differences:
  1. Catch-up reads from `read_index.read_stream(stream_id, cursor, CATCHUP_BATCH_SIZE)` instead of `read_index.read_all()`
  2. On `Err(Error::StreamNotFound { .. })`, catch-up terminates immediately and `CaughtUp` is yielded with no preceding events
  3. The live phase filters broadcast events to only those matching `stream_id`, then applies dedup on `stream_version`
- Deduplication boundary uses `Option<u64>` for `last_catchup_version`: `None` (no catch-up events yielded) means accept all live events for this stream; `Some(v)` means accept only events where `stream_version > v`
- Broadcast receiver is registered before any historical read (same ordering invariant as `subscribe_all`)
- Reuses the existing `CATCHUP_BATCH_SIZE` constant (500)
- Added `use uuid::Uuid;` to broker.rs imports (needed for the `stream_id` parameter type)
- Lag termination is identical to `subscribe_all`: yields `Err(Error::InvalidArgument("subscription lagged: re-subscribe from last checkpoint"))` and ends the stream

## Acceptance Criteria
- [x] AC: `subscribe_stream(read_index, broker, stream_id, from_version)` is a `pub async fn` in `src/broker.rs` with the specified signature
- [x] AC: Broadcast receiver registered before historical read begins (line 207 in broker.rs)
- [x] AC: Historical catch-up reads `read_index.read_stream(stream_id, cursor, CATCHUP_BATCH_SIZE)` with `StreamNotFound` handling for immediate `CaughtUp`
- [x] AC: `last_catchup_version` tracked as `Option<u64>`; live phase filters `stream_id == stream_id` AND dedup on `stream_version`
- [x] AC: Lag termination identical to `subscribe_all`
- [x] AC-8: Test -- interleaved streams A, B, A, B, A; subscribe_stream(A, 0) yields exactly 3 events with stream_version 0, 1, 2
- [x] AC-9: Test -- empty store, subscribe_stream(A, 0), CaughtUp, then append B, A, B; exactly 1 live Event for stream A
- [x] AC-10: Test -- non-existent stream, subscribe_stream yields CaughtUp with zero events, then append creates stream, event arrives live with stream_version 0
- [x] AC: `subscribe_stream` accessible at crate root via `eventfold_db::subscribe_stream`
- [x] AC: Quality gates pass

## Test Results
- Lint: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings`)
- Tests: PASS (131 tests, all green)
- Build: PASS (`cargo build` with zero warnings)
- Format: PASS (`cargo fmt --check`)
- New tests added:
  - `src/broker.rs::tests::ac8_subscribe_stream_catchup_interleaved`
  - `src/broker.rs::tests::ac9_subscribe_stream_live_filtering`
  - `src/broker.rs::tests::ac10_subscribe_stream_nonexistent_then_live`

## Concerns / Blockers
- None
