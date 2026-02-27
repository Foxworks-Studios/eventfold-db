# Implementation Report: Ticket 4 -- Stamp `recorded_at` in the writer task

**Ticket:** 4 - Stamp `recorded_at` in the writer task in `src/writer.rs`
**Date:** 2026-02-27 15:30
**Status:** COMPLETE

---

## Files Changed

### Modified
- `src/writer.rs` - Added `SystemTime`/`UNIX_EPOCH` import, replaced hardcoded `0` with real timestamp captured once per batch iteration, added two new tests.

## Implementation Notes
- The timestamp is captured once per outer `while let Some(first) = rx.recv().await` iteration, before the `for req in batch` loop. This means all requests drained in a single batch cycle share the same millisecond timestamp, matching the PRD requirement.
- The stamping expression uses `.expect("system clock before Unix epoch")` as specified. This is appropriate because `UNIX_EPOCH` should always be in the past on any sane system, and a system clock before 1970 represents an invariant violation.
- The `Instant` import (used for metrics timing) was already present; `SystemTime` and `UNIX_EPOCH` were added alongside it in the same `use std::time::{...}` line.
- No changes were needed to existing tests -- they all pass because they don't assert `recorded_at == 0`.

## Acceptance Criteria
- [x] AC 1: `recorded_at` is stamped once per writer-loop iteration, before the `for req in batch` loop - Implemented at line 189, captured via `let recorded_at = ...` before the `for req in batch` loop at line 196.
- [x] AC 2: The stamping expression is `SystemTime::now().duration_since(UNIX_EPOCH).expect("system clock before Unix epoch").as_millis() as u64` - Exact expression used at lines 189-192.
- [x] AC 3: `store.append(req.stream_id, req.expected_version, recorded_at, req.events)` receives the captured value - Line 224 passes `recorded_at` instead of the former hardcoded `0`.
- [x] AC 4: Test: append a single event via `WriterHandle::append`, assert `recorded[0].recorded_at > 0` - Test `recorded_at_is_nonzero_for_single_event` at line 1201.
- [x] AC 5: Test: append a batch of 3 events in a single call, assert all three have the same `recorded_at` - Test `batch_of_three_events_share_same_recorded_at` at line 1228.
- [x] AC 6: All existing writer tests pass (dedup tests, channel tests, metrics tests) - All 26 writer tests pass.
- [x] AC 7: `cargo build` produces zero warnings - Confirmed.
- [x] AC 8: `cargo test` passes - All 276 tests pass (202 unit + 74 integration).

## Test Results
- Lint: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings` clean)
- Tests: PASS (276 tests, 0 failures)
- Build: PASS (zero warnings)
- Format: PASS (`cargo fmt --check` clean)
- New tests added:
  - `writer::tests::recorded_at_is_nonzero_for_single_event` in `src/writer.rs`
  - `writer::tests::batch_of_three_events_share_same_recorded_at` in `src/writer.rs`

## Concerns / Blockers
- None
