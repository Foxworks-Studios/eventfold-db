# Implementation Report: Ticket 2 -- Update binary codec to format v3 with `recorded_at`

**Ticket:** 2 - Update binary codec to format v3 with `recorded_at` in `src/codec.rs`
**Date:** 2026-02-27 12:30
**Status:** COMPLETE

---

## Files Changed

### Modified
- `src/codec.rs` - Bumped FORMAT_VERSION from 2 to 3; updated FIXED_BODY_SIZE from 62 to 70; added `recorded_at` (u64 LE, 8 bytes) encode/decode after `global_position`; updated all doc comments referencing version 2 to version 3; updated `make_event` helper to use nonzero `recorded_at` (1_000_000_000_000); fixed `ac9_invalid_utf8_event_type` test offset (54 -> 62) to account for new field; renamed/updated header version tests; added 4 new tests for recorded_at behavior.

## Implementation Notes
- The binary layout change inserts `recorded_at` (8 bytes, u64 LE) immediately after `global_position` and before `stream_id`, matching the PRD specification.
- CRC32 naturally covers `recorded_at` because it hashes everything between the length prefix and the checksum, and `recorded_at` is encoded within that range.
- The `ac9_invalid_utf8_event_type` test had a hardcoded offset for the event_type region (was 54, now 62) that needed updating because the insertion of `recorded_at` shifted all subsequent fields by 8 bytes.
- The existing `decode_header_accepts_version_2` test was renamed to `decode_header_rejects_version_2` and rewritten to verify that v2 headers are properly rejected.
- One pre-existing flaky test (`writer::tests::ac11_writer_metrics_appends_and_events_total`) occasionally fails when run in the full suite due to global atomic counter bleed between tests. It passes consistently in isolation. This is not related to this ticket's changes.

## Acceptance Criteria
- [x] AC: `FORMAT_VERSION` constant is `3` -- Changed from 2 to 3 on line 21.
- [x] AC: `FIXED_BODY_SIZE` is `70` (was 62; +8 for `recorded_at`). Comment updated -- Line 131-134, comment lists `recorded_at(8)` and sum is 70.
- [x] AC: `encode_record` writes `event.recorded_at` as u64 LE immediately after `global_position` -- Line 164.
- [x] AC: `decode_record` reads `recorded_at` as u64 LE immediately after `global_position` and populates `RecordedEvent::recorded_at` -- Lines 275-277, used on line 324.
- [x] AC: CRC32 covers `recorded_at` bytes (between length prefix and checksum) -- The CRC is computed over `buf[LENGTH_PREFIX_SIZE..]` which includes `recorded_at`. Verified by `flipped_recorded_at_bit_returns_corrupt_record` test.
- [x] AC: `encode_header` returns bytes 4..8 encoding `3u32` LE -- `encode_header_bytes_4_to_8_are_version_3_le` test.
- [x] AC: `decode_header` with a v2 header returns `Err(Error::InvalidHeader(_))` -- `decode_header_rejects_version_2` test.
- [x] AC: Test: encode/decode round-trip with `recorded_at: 1_700_000_000_000` preserves value -- `round_trip_preserves_recorded_at` test.
- [x] AC: Test: flip a bit in the `recorded_at` bytes, keep original CRC -- decode returns `CorruptRecord` -- `flipped_recorded_at_bit_returns_corrupt_record` test.
- [x] AC: Test: encode with `recorded_at: 0xDEAD_BEEF_CAFE_1234_u64`, assert bytes 12..20 match `to_le_bytes()` -- `recorded_at_bytes_at_correct_offset` test.
- [x] AC: All existing round-trip tests pass with `recorded_at` added to the `make_event` helper -- `make_event` now sets `recorded_at: 1_000_000_000_000`; all 38 codec tests pass.
- [x] AC: Update any header version tests that hardcode `2` to expect `3` -- `encode_header_bytes_4_to_8_are_version_3_le`, `decode_header_round_trip_returns_version_3`, `decode_header_accepts_version_3`, `decode_header_rejects_version_2`.
- [x] AC: `cargo build` produces zero warnings.
- [x] AC: `cargo test` passes -- all codec tests green.

## Test Results
- Lint: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings` clean)
- Tests: PASS (197 tests, all green)
- Build: PASS (zero warnings)
- Format: PASS (`cargo fmt --check` clean)
- New tests added:
  - `codec::tests::round_trip_preserves_recorded_at` in `src/codec.rs`
  - `codec::tests::flipped_recorded_at_bit_returns_corrupt_record` in `src/codec.rs`
  - `codec::tests::recorded_at_bytes_at_correct_offset` in `src/codec.rs`
  - `codec::tests::fixed_body_size_is_70` in `src/codec.rs`
  - `codec::tests::decode_header_rejects_version_2` in `src/codec.rs`
  - `codec::tests::decode_header_accepts_version_3` in `src/codec.rs`

## Concerns / Blockers
- Pre-existing flaky test `writer::tests::ac11_writer_metrics_appends_and_events_total` -- global atomic counter bleed causes intermittent failure when run in the full suite. Not caused by this ticket; passes in isolation. Downstream tickets may want to address this with test isolation (e.g., `serial_test`).
- None related to this ticket's scope.
