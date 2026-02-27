# Implementation Report: Ticket 3 -- Add `recorded_at` parameter to `Store::append`

**Ticket:** 3 - Add `recorded_at` parameter to `Store::append` in `src/store.rs`
**Date:** 2026-02-27 14:30
**Status:** COMPLETE

---

## Files Changed

### Modified
- `src/store.rs` - Added `recorded_at: u64` parameter to `Store::append` signature, updated `RecordedEvent` construction to use the parameter, updated all ~30 test call sites to pass `0` for existing tests, added 3 new tests for timestamp-specific behavior.
- `src/writer.rs` - Added `0` placeholder for `recorded_at` in the production `store.append()` call (line 217). This was necessary because changing the `Store::append` signature broke compilation in this file. The ticket description stated "No caller outside `src/store.rs` tests calls `Store::append` directly" but this was inaccurate.
- `src/reader.rs` - Added `0` placeholder for `recorded_at` in 2 test call sites. Same reason as writer.rs -- the signature change broke compilation.

## Implementation Notes
- The `recorded_at` parameter is placed between `expected_version` and `proposed_events` in the signature, matching the ticket spec exactly.
- Inside the `append` method, `recorded_at: 0` was replaced with `recorded_at` (the parameter), so all events in a single append call share the same timestamp value.
- The ticket scope said "Modify: `src/store.rs` ONLY" but `writer.rs` and `reader.rs` also call `Store::append` directly, so minimal changes (adding `0` as the recorded_at argument) were required in those files to maintain compilation. Ticket 4 will replace the `0` in `writer.rs` with an actual `SystemTime::now()` timestamp.
- Three new tests were added per the acceptance criteria, all following existing test patterns (tempdir isolation, make_proposed helper, assert on returned RecordedEvent fields).

## Acceptance Criteria
- [x] AC 1: `Store::append` signature adds `recorded_at: u64` parameter (after `expected_version`, before `proposed_events`). -- Implemented at line 447.
- [x] AC 2: Each `RecordedEvent` built inside `append` has `recorded_at` set from the parameter. -- Line 520 uses `recorded_at` instead of `recorded_at: 0`.
- [x] AC 3: All events in one `append` call share the same `recorded_at`. -- The single parameter is applied to every event in the loop.
- [x] AC 4: Test with `recorded_at = 1_700_000_000_000`, assert returned events match. -- `append_recorded_at_is_set_from_parameter` test.
- [x] AC 5: Test with two different `recorded_at` values, assert each batch has its own. -- `append_different_recorded_at_per_batch` test.
- [x] AC 6: Test with `recorded_at: 0`, close and reopen, assert recovered events have `recorded_at == 0`. -- `append_recorded_at_zero_survives_reopen` test.
- [x] AC 7: All existing store tests pass (supply `0` for `recorded_at` where value doesn't matter). -- All 200 lib tests pass.
- [x] AC 8: `cargo build` produces zero warnings. -- Verified.
- [x] AC 9: `cargo test` passes -- all store tests green. -- 274 total tests pass (200 lib + 20 binary + 54 integration).

## Test Results
- Lint: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings` -- zero diagnostics)
- Tests: PASS (274 tests, 0 failures)
- Build: PASS (zero warnings)
- Format: PASS (`cargo fmt --check` clean)
- New tests added:
  - `src/store.rs::tests::append_recorded_at_is_set_from_parameter`
  - `src/store.rs::tests::append_different_recorded_at_per_batch`
  - `src/store.rs::tests::append_recorded_at_zero_survives_reopen`

## Concerns / Blockers
- **Out-of-scope file changes required:** The ticket stated scope as `src/store.rs` ONLY, but `src/writer.rs` (production code, line 217) and `src/reader.rs` (test code, lines 172 and 253) also call `Store::append` directly. Changing the signature without updating these files prevents compilation entirely. Minimal changes were made: `0` was inserted as the `recorded_at` argument in all three call sites. Ticket 4 will replace the writer.rs placeholder with an actual timestamp.
