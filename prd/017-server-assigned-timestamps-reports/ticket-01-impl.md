# Implementation Report: Ticket 1 -- Add `recorded_at` field to `RecordedEvent` in `src/types.rs`

**Ticket:** 1 - Add `recorded_at` field to `RecordedEvent` in `src/types.rs`
**Date:** 2026-02-27 15:30
**Status:** COMPLETE

---

## Files Changed

### Created
- None

### Modified
- `src/types.rs` - Added `pub recorded_at: u64` field to `RecordedEvent` struct with doc comment; updated struct-level doc comment; updated all 6 existing `RecordedEvent` literals in tests to include `recorded_at: 0`; added 3 new tests for the field.
- `src/store.rs` - Added `recorded_at: 0` placeholder to 2 `RecordedEvent` constructions (production `append()` and test helper `make_event()`).
- `src/codec.rs` - Added `recorded_at: 0` placeholder to 3 `RecordedEvent` constructions (decode function and 2 test helpers).
- `src/broker.rs` - Added `recorded_at: 0` placeholder to 1 `RecordedEvent` construction (test helper `make_event()`).
- `src/dedup.rs` - Added `recorded_at: 0` placeholder to 1 `RecordedEvent` construction (test helper `recorded()`).
- `src/service.rs` - Added `recorded_at: 0` placeholder to 1 `RecordedEvent` construction (test `recorded_to_proto_round_trip()`).
- `src/lib.rs` - Added `recorded_at: 0` placeholder to 1 `RecordedEvent` construction (test `reexport_recorded_event()`).

## Implementation Notes
- The ticket scope says "only types.rs" but adding a struct field breaks all callers. Per the orchestrator's revised approach, I mechanically added `recorded_at: 0` to all `RecordedEvent` literal constructions across the codebase to keep the build passing. Later tickets will replace these placeholder zeros with real timestamps.
- The field is positioned after `global_position` as specified in the AC, matching the binary codec layout described in the PRD.
- The `eventfold-console` crate was not affected because it constructs `proto::RecordedEvent` (prost-generated), not the domain `RecordedEvent`.

## Acceptance Criteria
- [x] AC 1: `RecordedEvent` has a new `pub recorded_at: u64` field after `global_position`, with doc comment: "Unix epoch milliseconds, server-assigned at append time." -- Field added at line 76 of `src/types.rs` with matching doc comment at line 75.
- [x] AC 2: The field is listed in the struct-level doc comment under `# Fields`. -- Added `* \`recorded_at\` - Unix epoch milliseconds, server-assigned at append time.` at line 61.
- [x] AC 3: All existing `RecordedEvent { .. }` literals in `src/types.rs` tests include `recorded_at: 0` (or a nonzero sentinel for PartialEq tests). -- All 6 existing literals updated; the inequality test uses `recorded_at: 0` on both sides (differing on `global_position`, not `recorded_at`).
- [x] AC 4: Test: construct a `RecordedEvent` with `recorded_at: 1_700_000_000_123`, assert `event.recorded_at == 1_700_000_000_123`. -- Test `recorded_event_recorded_at_round_trip` added.
- [x] AC 5: Test: clone a `RecordedEvent` with `recorded_at: 42`, assert clone's `recorded_at == 42`. -- Test `recorded_event_clone_preserves_recorded_at` added.
- [x] AC 6: Test: two `RecordedEvent`s identical except `recorded_at` differ are `!=`. -- Test `recorded_events_with_different_recorded_at_are_not_equal` added (100 vs 200).
- [x] AC 7: `cargo build` produces zero warnings. -- Confirmed.
- [x] AC 8: `cargo test` passes with all tests in `src/types.rs` green. -- All 19 types tests pass, plus all 266 tests across the full suite.

## Test Results
- Lint: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings` -- zero warnings)
- Tests: PASS (`cargo test` -- 266 tests, 0 failures)
- Build: PASS (`cargo build` -- zero warnings)
- Format: PASS (`cargo fmt --check` -- no issues)
- New tests added:
  - `src/types.rs::tests::recorded_event_recorded_at_round_trip`
  - `src/types.rs::tests::recorded_event_clone_preserves_recorded_at`
  - `src/types.rs::tests::recorded_events_with_different_recorded_at_are_not_equal`

## Concerns / Blockers
- Files outside the stated ticket scope (`src/store.rs`, `src/codec.rs`, `src/broker.rs`, `src/dedup.rs`, `src/service.rs`, `src/lib.rs`) were modified with mechanical `recorded_at: 0` placeholder additions. This was necessary to keep the build passing and was explicitly authorized by the orchestrator's revised approach in the ticket description. Later tickets (codec, store, writer, service) will replace these zeros with real timestamp values.
