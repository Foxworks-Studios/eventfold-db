# Implementation Report: Ticket 5 -- Proto + service + integration test updates

**Ticket:** 5 - Proto + service + integration test updates
**Date:** 2026-02-27 12:00
**Status:** COMPLETE

---

## Files Changed

### Modified
- `proto/eventfold.proto` - Added `uint64 recorded_at = 8;` with comment to `RecordedEvent` message
- `src/service.rs` - Added `recorded_at: e.recorded_at` mapping in `recorded_to_proto`; added unit test `recorded_to_proto_maps_recorded_at`
- `tests/grpc_service.rs` - Added 3 new integration tests: `read_all_returns_nonzero_recorded_at`, `read_stream_returns_nonzero_recorded_at`, `subscribe_all_yields_nonzero_recorded_at`

### Not Modified (no changes needed)
- `tests/writer_integration.rs` - Does not construct `proto::RecordedEvent` or assert on proto fields; uses domain types only
- `tests/broker_integration.rs` - Does not construct `proto::RecordedEvent` or assert on proto fields; uses domain types only
- `tests/idempotent_appends.rs` - Does not assert on `RecordedEvent` fields beyond positions; no changes needed
- `tests/metrics.rs` - Tests metrics counters/gauges, not event field values; no changes needed

## Implementation Notes
- The proto field `uint64 recorded_at = 8` triggers prost codegen via `build.rs`, automatically adding the field to the generated `proto::RecordedEvent` Rust struct.
- The mapping in `recorded_to_proto` is a direct passthrough: `recorded_at: e.recorded_at` (both u64).
- Integration tests confirm the full pipeline works end-to-end: writer stamps `SystemTime::now()` -> codec persists -> store/index holds -> service maps to proto -> gRPC client receives `recorded_at > 0`.
- The `writer_integration.rs`, `broker_integration.rs`, `idempotent_appends.rs`, and `metrics.rs` files were reviewed and do not need changes -- they don't construct or assert on proto-level `RecordedEvent` fields beyond positions.

## Acceptance Criteria
- [x] AC 1: `proto/eventfold.proto` `RecordedEvent` message includes `uint64 recorded_at = 8;` with comment `// Unix epoch milliseconds, server-assigned` - Added at line 27 of proto file
- [x] AC 2: `recorded_to_proto` in `src/service.rs` maps `e.recorded_at` to the proto field - Added at line 398
- [x] AC 3: Test: `recorded_to_proto` with `recorded_at: 1_700_000_000_000` produces proto with matching value - Unit test `recorded_to_proto_maps_recorded_at` in `src/service.rs`
- [x] AC 4: Integration test: append one event, ReadAll returns `recorded_at > 0` - `read_all_returns_nonzero_recorded_at` in `tests/grpc_service.rs`
- [x] AC 5: Integration test: append one event, ReadStream returns `recorded_at > 0` - `read_stream_returns_nonzero_recorded_at` in `tests/grpc_service.rs`
- [x] AC 6: Integration test: SubscribeAll yields event with `recorded_at > 0` - `subscribe_all_yields_nonzero_recorded_at` in `tests/grpc_service.rs`
- [x] AC 7: All existing integration tests compile and pass - 280 tests total, 0 failures
- [x] AC 8: `cargo build` produces zero warnings (proto codegen runs via build.rs) - Confirmed
- [x] AC 9: `cargo test` passes - 280 tests, all green

## Test Results
- Lint: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings` -- zero warnings)
- Tests: PASS (280 tests, 0 failures across all test targets)
- Build: PASS (zero warnings)
- Format: PASS (`cargo fmt --check` -- clean)
- New tests added:
  - `src/service.rs::tests::recorded_to_proto_maps_recorded_at` (unit test)
  - `tests/grpc_service.rs::read_all_returns_nonzero_recorded_at` (integration test)
  - `tests/grpc_service.rs::read_stream_returns_nonzero_recorded_at` (integration test)
  - `tests/grpc_service.rs::subscribe_all_yields_nonzero_recorded_at` (integration test)

## Concerns / Blockers
- The PRD lists `CLAUDE.md` as needing an update to domain rules (changing "The server does not assign timestamps" to reflect the new `recorded_at` behavior). This is NOT listed in the ticket's scope, so I did not modify it. A separate ticket or manual update should address this.
