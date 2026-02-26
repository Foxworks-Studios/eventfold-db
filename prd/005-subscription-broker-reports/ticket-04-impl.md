# Implementation Report: Ticket 4 -- `subscribe_all`: Catch-Up, CaughtUp Marker, Live Events, and Lag Termination

**Ticket:** 4 - subscribe_all: Catch-Up, CaughtUp Marker, Live Events, and Lag Termination
**Date:** 2026-02-26 16:30
**Status:** COMPLETE

---

## Files Changed

### Created
- None

### Modified
- `src/broker.rs` - Added `subscribe_all` function, `CATCHUP_BATCH_SIZE` constant, `#[derive(Clone)]` on `Broker`, and 5 unit tests (AC-4, AC-5, AC-6, AC-7, AC-11)
- `src/lib.rs` - Added re-exports for `subscribe_all` and `SubscriptionMessage`
- `Cargo.toml` - Added `futures-core = "0.3"` to `[dependencies]` (needed for `Stream` trait in `subscribe_all` return type) and `futures = "0.3"` to `[dev-dependencies]` (needed for `StreamExt::next` in tests)

## Implementation Notes
- **Subscribe before reading history**: `broker.subscribe()` is called as the first statement in `subscribe_all`, before any `read_index.read_all()` call. This prevents the documented race condition.
- **Batch-based catch-up**: Historical events are read in batches of `CATCHUP_BATCH_SIZE` (500). The loop terminates when a batch returns fewer events than the batch size.
- **Deduplication via `last_catchup_position`**: An `Option<u64>` tracks the last global position sent during catch-up. In the live phase, events with `global_position <= last_catchup_position` are skipped. When `None` (empty store), all live events pass through.
- **Lag termination**: `RecvError::Lagged` yields `Err(Error::InvalidArgument("subscription lagged: re-subscribe from last checkpoint"))` and terminates the stream via `return`.
- **`Broker: Clone`**: Added `#[derive(Clone)]` to `Broker` since `broadcast::Sender` is `Clone`. This is necessary for the tests (and real usage) where `spawn_writer` consumes the broker but `subscribe_all` borrows it. The clone is cheap (Arc internally).
- **`futures-core` dependency**: Added to `[dependencies]` because the public function return type `impl futures_core::Stream<...>` requires it. This is already a transitive dependency of `async-stream`.
- **Rust 2024 let chains**: Used `if let Some(last_pos) = last_catchup_position && arc_event.global_position <= last_pos` per clippy's `collapsible_if` requirement in edition 2024.
- **`SubscriptionMessage` re-export**: Added to `lib.rs` because it's part of the public API (returned by `subscribe_all`).

## Acceptance Criteria
- [x] AC (signature): `subscribe_all(read_index: ReadIndex, broker: &Broker, from_position: u64) -> impl Stream<Item = Result<SubscriptionMessage, Error>>` is `pub async fn` in `src/broker.rs`
- [x] AC (step 1): `broker.subscribe()` is called before any historical read
- [x] AC (step 2): Historical events read via `read_index.read_all(cursor, CATCHUP_BATCH_SIZE)` in a loop, each yielded as `SubscriptionMessage::Event(Arc::new(event))`; `CATCHUP_BATCH_SIZE: u64 = 500` at module scope
- [x] AC (step 3): `SubscriptionMessage::CaughtUp` yielded when batch returns fewer than `CATCHUP_BATCH_SIZE`
- [x] AC (step 4): Broadcast receiver drained; events with `global_position <= last_catchup_position` skipped
- [x] AC (step 5): `Err(tokio::sync::broadcast::error::RecvError::Lagged(_))` yields `Err(Error::InvalidArgument("subscription lagged: re-subscribe from last checkpoint"))` and terminates
- [x] AC-4 (test): Catch-up only -- 5 events appended, subscribe from 0, exactly 5 Event variants at positions 0..4 before CaughtUp
- [x] AC-5 (test): Catch-up from middle -- 10 events, subscribe from 5, exactly 5 events at positions 5..9
- [x] AC-6 (test): Live after catch-up -- 3 events, subscribe, CaughtUp, append 2 more, receive positions 3 and 4
- [x] AC-7 (test): No duplicates during transition -- empty store, subscribe, append 5, all 5 unique positions with no repeats
- [x] AC-11 (test): Lag termination -- broker capacity 4, append 10 without polling, stream yields Error::InvalidArgument then None
- [x] `subscribe_all` accessible at crate root via `eventfold_db::subscribe_all`
- [x] `futures` added to `[dev-dependencies]` for `StreamExt::next` in tests
- [x] Quality gates pass: `cargo build`, `cargo clippy`, `cargo fmt --check`, `cargo test`

## Test Results
- Lint: PASS (clippy --all-targets --all-features --locked -- -D warnings)
- Tests: PASS (128 unit + 1 integration = 129 total, 0 failures)
- Build: PASS (zero warnings)
- Format: PASS
- New tests added:
  - `broker::tests::ac4_subscribe_all_catchup_only` in `src/broker.rs`
  - `broker::tests::ac5_subscribe_all_catchup_from_middle` in `src/broker.rs`
  - `broker::tests::ac6_subscribe_all_live_after_catchup` in `src/broker.rs`
  - `broker::tests::ac7_subscribe_all_no_duplicates_during_transition` in `src/broker.rs`
  - `broker::tests::ac11_subscribe_all_lag_termination` in `src/broker.rs`

## Concerns / Blockers
- Added `futures-core = "0.3"` to `[dependencies]` (not just dev-dependencies) because the public return type of `subscribe_all` references `futures_core::Stream`. This was not explicitly mentioned in the ticket but is a necessary consequence of the function signature. It's already a transitive dependency of `async-stream`.
- Added `#[derive(Clone)]` to `Broker`. Without this, it's impossible to use `subscribe_all` in conjunction with `spawn_writer` (which takes `Broker` by value). `broadcast::Sender` is `Clone` so this is a zero-cost addition. Downstream tickets (e.g., `subscribe_stream`) will also need this.
- Added `SubscriptionMessage` to the re-exports in `lib.rs`. This was not explicitly listed in the ticket scope but is necessary for consumers to use the `subscribe_all` return type meaningfully.
