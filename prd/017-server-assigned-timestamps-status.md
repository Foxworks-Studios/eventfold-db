# Build Status: PRD 017 -- Server-Assigned Timestamps

**Source PRD:** prd/017-server-assigned-timestamps.md
**Tickets:** prd/017-server-assigned-timestamps-tickets.md
**Started:** 2026-02-27 12:00
**Last Updated:** 2026-02-27 13:00
**Overall Status:** QA READY

---

## Ticket Tracker

| Ticket | Title | Status | Impl Report | Review Report | Notes |
|--------|-------|--------|-------------|---------------|-------|
| 1 | Add `recorded_at` field to `RecordedEvent` | DONE | ticket-01-impl.md | ticket-01-review.md | APPROVED (scope included T2 codec work) |
| 2 | Update binary codec to format v3 | DONE | ticket-02-impl.md | -- | Completed by T1+T2 agents combined |
| 3 | Add `recorded_at` parameter to `Store::append` | DONE | ticket-03-impl.md | -- | Build+tests pass |
| 4 | Stamp `recorded_at` in the writer task | DONE | ticket-04-impl.md | -- | Build+tests pass |
| 5 | Proto + service + integration test updates | DONE | ticket-05-impl.md | -- | 280 tests, 3 new integration tests |
| 6 | Verification and integration check | DONE | -- | -- | All 7 ACs verified |

## Prior Work Summary

- `src/types.rs`: Added `pub recorded_at: u64` to `RecordedEvent` after `global_position`. 3 new tests. All RecordedEvent literals across codebase updated with `recorded_at: 0` placeholder.
- `src/codec.rs`: `FORMAT_VERSION` bumped to 3. `FIXED_BODY_SIZE` now 70 (+8). `encode_record`/`decode_record` handle `recorded_at` as u64 LE after `global_position`. Header tests updated to expect v3. 4 new codec tests (round-trip, CRC, byte offset, size constant).
- `src/store.rs`: `Store::append` gains `recorded_at: u64` parameter. Each `RecordedEvent` built inside uses the parameter. ~30 test call sites updated. 3 new tests (value preserved, two batches differ, round-trip through close/reopen).
- `src/writer.rs`: `SystemTime::now()` stamped once before `for req in batch` loop. Passed to `store.append()`. 2 new tests (nonzero timestamp, batch sharing).
- `proto/eventfold.proto`: Added `uint64 recorded_at = 8;` to `RecordedEvent` message.
- `src/service.rs`: `recorded_to_proto` maps `e.recorded_at`. 1 new unit test.
- `tests/grpc_service.rs`: 3 new integration tests (read_all, read_stream, subscribe_all all verify `recorded_at > 0`).
- `CLAUDE.md`: Updated domain rules to reflect server-assigned `recorded_at`.
- 280 tests passing. Build, clippy, fmt all clean.

## Follow-Up Tickets

[None.]

## Completion Report

**Completed:** 2026-02-27 13:00
**Tickets Completed:** 6/6

### Summary of Changes

**Files modified:**
- `src/types.rs` -- Added `pub recorded_at: u64` field to `RecordedEvent`, 3 new tests
- `src/codec.rs` -- FORMAT_VERSION 2->3, FIXED_BODY_SIZE 62->70, encode/decode recorded_at as u64 LE, 4 new tests
- `src/store.rs` -- `Store::append` gains `recorded_at: u64` parameter, 3 new tests, ~30 call sites updated
- `src/writer.rs` -- SystemTime::now() stamped per batch, passed to store.append(), 2 new tests
- `src/service.rs` -- `recorded_to_proto` maps recorded_at, 1 new test
- `proto/eventfold.proto` -- Added `uint64 recorded_at = 8` to RecordedEvent message
- `tests/grpc_service.rs` -- 3 new integration tests verifying recorded_at > 0
- `tests/writer_integration.rs` -- Updated RecordedEvent constructions
- `tests/broker_integration.rs` -- Updated RecordedEvent constructions
- `tests/idempotent_appends.rs` -- Updated RecordedEvent constructions
- `tests/metrics.rs` -- Updated RecordedEvent constructions
- `CLAUDE.md` -- Updated domain rules: server now assigns recorded_at

### Key Architectural Decisions
- Millisecond precision (u64 epoch millis) chosen over seconds for adequate resolution
- Timestamp stamped in writer task (not gRPC handler) to reflect persistence order
- All events in a batch share the same timestamp (stamped once before the for-loop)
- No migration support -- delete `data/` and start fresh (no consumers yet)

### AC Coverage Matrix
| AC | Verified |
|----|----------|
| 1 | Yes -- `pub recorded_at: u64` in RecordedEvent |
| 2 | Yes -- integration tests verify recorded_at > 0 on read/subscribe |
| 3 | Yes -- writer stamps once per batch, test confirms sharing |
| 4 | Yes -- FORMAT_VERSION == 3, old v2 files rejected |
| 5 | Yes -- codec + store round-trip tests preserve recorded_at |
| 6 | Yes -- proto field 8, recorded_to_proto maps it, 3 integration tests |
| 7 | Yes -- 280 tests, clippy, fmt, zero warnings |

### Ready for QA: YES
