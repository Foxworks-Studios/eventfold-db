# Code Review: Ticket 4 -- `subscribe_all`: Catch-Up, CaughtUp Marker, Live Events, and Lag Termination

**Ticket:** 4 -- subscribe_all: Catch-Up, CaughtUp Marker, Live Events, and Lag Termination
**Impl Report:** prd/005-subscription-broker-reports/ticket-04-impl.md
**Date:** 2026-02-26 17:15
**Verdict:** APPROVED

---

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 (signature) | `subscribe_all(read_index, broker, from_position)` returns `impl Stream<Item = Result<SubscriptionMessage, Error>>` | Met | Lines 110-114 of `src/broker.rs`. `pub async fn` with correct parameter types and return type using `futures_core::Stream`. |
| 2 (step 1) | Broadcast receiver registered BEFORE historical read | Met | Line 116: `let mut rx = broker.subscribe();` executes when the `async fn` is `.await`ed, before the `stream!` macro body is polled. The `stream!` body contains all catch-up reads. Correct ordering. |
| 3 (step 2) | Historical events read in batches of `CATCHUP_BATCH_SIZE` (500) | Met | Lines 85, 120-138. `CATCHUP_BATCH_SIZE: u64 = 500` defined at module scope. Loop calls `read_index.read_all(cursor, CATCHUP_BATCH_SIZE)` and advances cursor. |
| 4 (step 3) | `CaughtUp` yielded when batch < 500 | Met | Lines 135-137: `if batch_len < CATCHUP_BATCH_SIZE { break; }`, then line 141: `yield Ok(SubscriptionMessage::CaughtUp);`. |
| 5 (dedup) | Events with `global_position <= last_catchup_position` skipped | Met | Lines 148-152: `if let Some(last_pos) = last_catchup_position && arc_event.global_position <= last_pos { continue; }`. Uses Rust 2024 let-chains. When `last_catchup_position` is `None` (empty store), all live events pass through. Correct. |
| 6 (lag) | `RecvError::Lagged` yields `Error::InvalidArgument` and terminates | Met | Lines 155-161: yields the error with the specified message, then `return;` terminates the stream. `RecvError::Closed` also handled (line 162-164) as defensive termination. |
| 7 (test AC-4) | Catch-up only: 5 events, positions 0..4, then CaughtUp | Met | Test `ac4_subscribe_all_catchup_only` (lines 304-340). Appends 5 events, subscribes from 0, collects until CaughtUp, asserts positions `[0, 1, 2, 3, 4]`. |
| 8 (test AC-5) | Catch-up from middle: positions 5..9 | Met | Test `ac5_subscribe_all_catchup_from_middle` (lines 345-381). Appends 10 events, subscribes from 5, asserts positions `[5, 6, 7, 8, 9]`. |
| 9 (test AC-6) | Live events after catch-up: positions 3 and 4 after CaughtUp | Met | Test `ac6_subscribe_all_live_after_catchup` (lines 387-460). Appends 3, subscribes, drains catch-up (0,1,2), appends 2 more, receives live events at positions 3 and 4 with timeout protection. |
| 10 (test AC-7) | No duplicates during transition: 5 unique events | Met | Test `ac7_subscribe_all_no_duplicates_during_transition` (lines 466-528). Subscribes on empty store, appends 5 events, collects from both catch-up and live phases using `HashSet` to detect duplicates, asserts exactly `{0, 1, 2, 3, 4}`. |
| 11 (test AC-11) | Lag termination: stream ends with error | Met | Test `ac11_subscribe_all_lag_termination` (lines 534-593). Broker capacity 4, appends 10 without polling, then polls until stream ends. Verifies `InvalidArgument` error is yielded and stream terminates with `None`. |
| 12 | `subscribe_all` accessible at crate root | Met | `src/lib.rs` line 11: `pub use broker::{Broker, subscribe_all};`. |
| 13 | `futures` in dev-dependencies | Met | `Cargo.toml` line 19: `futures = "0.3"` under `[dev-dependencies]`. |
| 14 | Quality gates pass | Met | Verified: `cargo test` (128 unit + 1 integration = 129, 0 failures), `cargo clippy --all-targets --all-features --locked -- -D warnings` (clean), `cargo fmt --check` (clean), `cargo build` (zero warnings). |

## Issues Found

### Critical (must fix before merge)
- None.

### Major (should fix, risk of downstream problems)
- None.

### Minor (nice to fix, not blocking)
- None.

## Suggestions (non-blocking)

1. **`SubscriptionMessage` re-export path**: The `SubscriptionMessage` re-export in `lib.rs` is added to the `types` re-export group (`pub use types::{..., SubscriptionMessage}`), which is the correct and conventional placement since `SubscriptionMessage` is defined in `src/types.rs`. Good.

2. **Test helper duplication**: The `proposed()` and `temp_store()` helpers in `broker::tests` (lines 284-299) duplicate the same helpers in `writer::tests` and `reader::tests`. This is a minor maintenance concern but acceptable -- each test module being self-contained is a valid pattern, and extracting shared test helpers would be a separate refactoring concern.

3. **`RecvError::Closed` handling**: Line 162-164 handles the `Closed` variant gracefully by terminating the stream silently. This is good defensive programming not explicitly required by the ticket but correct behavior -- when the broker shuts down, the stream should end cleanly rather than panic.

## Scope Check
- Files within scope: YES. Modified `src/broker.rs`, `src/lib.rs`, `Cargo.toml` as specified.
- Scope creep detected: NO. The `SubscriptionMessage` re-export and `futures-core` dependency additions are justified consequences of the function signature, as the impl report correctly notes.
- Unauthorized dependencies added: NO. `futures-core` (dependency) and `futures` (dev-dependency) are both specified in the ticket scope. `async-stream` was from Ticket 1 (co-mingled in the same uncommitted working tree).

## Risk Assessment
- Regression risk: LOW. The `subscribe_all` function is additive. The `#[derive(Clone)]` on `Broker` is safe -- `broadcast::Sender::clone()` creates a new handle to the same channel, which is the intended behavior.
- Security concerns: NONE.
- Performance concerns: NONE. Batch-based catch-up with `CATCHUP_BATCH_SIZE = 500` keeps memory bounded. `Arc` wrapping during catch-up (`Arc::new(event)`) avoids deep-cloning. The broadcast channel's `Arc<RecordedEvent>` sharing is already established by Ticket 2.

## Correctness Deep-Dive

The critical correctness property -- "subscribe before reading history" -- is correctly implemented. The `async fn` pattern ensures `broker.subscribe()` (line 116) runs when the caller `.await`s `subscribe_all()`, while the `stream!` body (containing all historical reads) executes lazily when the returned stream is polled. This guarantees the broadcast receiver captures all events appended after the subscription point, and the deduplication window correctly bridges the catch-up/live transition.

The deduplication logic handles the empty-store edge case correctly: when no catch-up events exist, `last_catchup_position` remains `None`, so the `if let Some(last_pos)` guard never fires and all live events pass through unfiltered.
