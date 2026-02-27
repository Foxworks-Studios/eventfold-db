# Implementation Report: Ticket 4 -- Implement SubscribeAll and SubscribeStream Server-Streaming RPCs

**Ticket:** 4 - Implement SubscribeAll and SubscribeStream Server-Streaming RPCs
**Date:** 2026-02-26 18:30
**Status:** COMPLETE

---

## Files Changed

### Modified
- `src/service.rs` - Replaced `subscribe_all` and `subscribe_stream` stubs with real implementations that map `SubscriptionMessage` to `SubscribeResponse` via `async_stream::stream!` macro. Added `use futures_core::Stream` and `use crate::types::SubscriptionMessage` imports.
- `tests/grpc_service.rs` - Added 7 new integration tests (AC-15 through AC-21). Refactored `start_test_server` to delegate to a new `start_test_server_with_broker_capacity` helper to support AC-20's small-capacity lag test.

## Implementation Notes

- **Lifetime solution**: The returned stream must be `'static` (not borrowing `&self`). Cloning `read_index` and `broker` into owned bindings, then moving them into the `async_stream::stream!` closure ensures the stream owns all its data. The `crate::subscribe_all`/`subscribe_stream` calls happen inside the stream generator, so `broker` lives as long as the stream.

- **Avoiding `StreamExt` in production code**: The `futures` crate (which provides `StreamExt::next()`) is a dev-dependency only. Instead, `std::future::poll_fn` with `Stream::poll_next` is used to drive the inner stream item-by-item within the `async_stream::stream!` macro. This avoids adding new dependencies.

- **Mapping pattern**: Both handlers follow identical mapping logic:
  - `SubscriptionMessage::Event(arc)` -> `SubscribeResponse { content: Event(recorded_to_proto(&arc)) }`
  - `SubscriptionMessage::CaughtUp` -> `SubscribeResponse { content: CaughtUp(Empty {}) }`
  - `Err(e)` -> `Err(error_to_status(e))` then stream terminates
  - `None` -> stream terminates

- **Test helper refactoring**: `start_test_server()` now delegates to `start_test_server_with_broker_capacity(1024)`, and AC-20 uses `start_test_server_with_broker_capacity(4)` for lag testing.

## Acceptance Criteria

- [x] AC: `subscribe_all` handler - Calls `crate::subscribe_all(read_index, &broker, from_position).await`; maps Event/CaughtUp/Err correctly. Implemented in `src/service.rs` lines 160-205.
- [x] AC: `subscribe_stream` handler - Validates stream_id via `parse_uuid`; calls `crate::subscribe_stream(...).await`; maps identically. Implemented in `src/service.rs` lines 214-257.
- [x] AC-15: Append 3 events; SubscribeAll from 0; 3 events + CaughtUp; append 2 more; 2 live events. Test `ac15_subscribe_all_catchup_then_live`.
- [x] AC-16: Append 5 events; SubscribeAll from 3; 2 events + CaughtUp. Test `ac16_subscribe_all_from_middle`.
- [x] AC-17: Append to A, B, A; SubscribeStream A from 0; 2 events + CaughtUp; append to A; live event; no B events. Test `ac17_subscribe_stream_catchup_and_live`.
- [x] AC-18: SubscribeStream A; append B, C, A, B; only A's event after CaughtUp. Test `ac18_subscribe_stream_filters_other_streams`.
- [x] AC-19: SubscribeStream non-existent; CaughtUp immediately; append; live event. Test `ac19_subscribe_stream_nonexistent_then_live`.
- [x] AC-20: SubscribeAll with capacity 4; overflow terminates stream. Test `ac20_subscribe_all_lag_terminates_stream`.
- [x] AC-21: 2 concurrent SubscribeAll; both receive same events. Test `ac21_two_concurrent_subscribe_all`.
- [x] Quality gates pass.

## Test Results

- Lint: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings`)
- Tests: PASS (172 total: 148 unit + 2 broker integration + 21 gRPC integration + 1 writer integration)
- Build: PASS (zero warnings)
- Format: PASS (`cargo fmt --check`)
- New tests added:
  - `tests/grpc_service.rs::ac15_subscribe_all_catchup_then_live`
  - `tests/grpc_service.rs::ac16_subscribe_all_from_middle`
  - `tests/grpc_service.rs::ac17_subscribe_stream_catchup_and_live`
  - `tests/grpc_service.rs::ac18_subscribe_stream_filters_other_streams`
  - `tests/grpc_service.rs::ac19_subscribe_stream_nonexistent_then_live`
  - `tests/grpc_service.rs::ac20_subscribe_all_lag_terminates_stream`
  - `tests/grpc_service.rs::ac21_two_concurrent_subscribe_all`

## Concerns / Blockers

- None
