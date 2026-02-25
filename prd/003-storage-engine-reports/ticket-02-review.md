# Code Review: Ticket 2 -- Startup Recovery -- Index Rebuild, Partial Trailing Truncation, Mid-File Corruption

**Ticket:** 2 -- Startup Recovery -- Index Rebuild, Partial Trailing Truncation, Mid-File Corruption
**Impl Report:** prd/003-storage-engine-reports/ticket-02-impl.md
**Date:** 2026-02-25 16:30
**Verdict:** APPROVED

---

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 | Header validated via `codec::decode_header()`; returns `Error::InvalidHeader` if invalid | Met | Lines 109-120 of `src/store.rs`: checks `data.len() < HEADER_SIZE`, converts slice to `[u8; 8]`, calls `codec::decode_header()`. Three dedicated tests cover short file, bad magic, and bad version. |
| 2 | Recovery loop calls `decode_record()`, advancing on Complete, stopping on Incomplete/CorruptRecord | Met | Lines 127-174: loop reads `remaining` slice, matches on `decode_record()` result. `Complete` pushes event and advances offset. `Incomplete` and `CorruptRecord` branch to trailing/mid-file detection. Tested by `recovery_rebuilds_index_from_5_events_across_2_streams`. |
| 3 | On Complete: pushes event into `events`, pushes `global_position` into `streams[stream_id]` | Met | Lines 134-139: `events.push(event)`, `streams.entry(stream_id).or_default().push(global_pos)`. Uses `global_position` directly from the decoded event (not re-assigned), matching the AC requirement. |
| 4 | On trailing incomplete/corrupt: truncates file via `set_len()`, `sync_all()`, logs `tracing::warn!` | Met | Lines 155-170: `tracing::warn!` with offset and valid_events, then `file.set_len(offset as u64)`, `file.sync_all()`. Tested by `recovery_truncates_trailing_garbage_bytes` (AC-3) and `recovery_truncates_crc_corrupt_last_record` (AC-4). |
| 5 | On mid-file corruption: returns `Err(Error::CorruptRecord { position, detail })` | Met | Lines 145-152: calls `has_valid_record_after(&data, offset)`, returns `CorruptRecord` if true. `position` is set to `events.len() as u64` (the index of the corrupt event). Tested by `recovery_returns_error_on_mid_file_corruption`. |
| Test AC-2 | 5 events across 2 streams recovered correctly | Met | `recovery_rebuilds_index_from_5_events_across_2_streams` (line 298): seeds 5 interleaved events, reopens, verifies `global_position() == 5`, `stream_version(a) == Some(2)`, `stream_version(b) == Some(1)`, and field-by-field comparison of all 5 recovered events. |
| Test AC-3 | 3 events + 10 garbage bytes -> only 3 recovered, file truncated | Met | `recovery_truncates_trailing_garbage_bytes` (line 360): seeds 3 events, appends 10 garbage bytes, reopens, verifies 3 events recovered and file size equals pre-garbage size. |
| Test AC-4 | 3 events, corrupt last record's payload -> only 2 recovered | Met | `recovery_truncates_crc_corrupt_last_record` (line 408): flips byte at `data.len() - 5` (last payload byte before CRC), reopens, verifies 2 events recovered and file truncated to boundary before 3rd record. |
| Test AC-5 | 3 events, corrupt 2nd record -> returns Error::CorruptRecord | Met | `recovery_returns_error_on_mid_file_corruption` (line 453): flips byte at `rec1_start + 20` (inside 2nd record), reopens, asserts `Err(Error::CorruptRecord { .. })`. The 3rd record remains valid, so `has_valid_record_after` returns true. |
| Quality gates | cargo build, clippy, fmt, test all pass | Met | Verified independently: 60 tests pass, clippy clean, fmt clean, zero build warnings. |

## Issues Found

### Critical (must fix before merge)

None.

### Major (should fix, risk of downstream problems)

None.

### Minor (nice to fix, not blocking)

1. **File handle mode inconsistency (new-file vs. recovery path):** The new-file path (line 95, `File::create`) produces a write-only handle, while the recovery path (lines 162, 177) produces a read+write handle via `OpenOptions::new().read(true).write(true)`. When Ticket 3 adds `append()`, the handle mode may matter. Since `append()` only needs to write (the in-memory index serves reads), write-only should suffice, but the inconsistency is worth noting for Ticket 3's implementer to verify.

2. **`has_valid_record_after` O(n*m) performance note:** The scan probes every byte offset from `start+1` to end of file, calling `decode_record()` at each. For a 1 GB file with corruption at byte 100, this is prohibitively slow. The impl report acknowledges this tradeoff and it is acceptable for the current design (no performance constraint on recovery). No action needed now, but worth documenting in a code comment for future maintainers.

3. **AC-2 test does not verify `metadata` field recovery:** The field-by-field comparison loop (lines 328-354) checks `event_id`, `stream_id`, `stream_version`, `global_position`, `event_type`, and `payload` but omits `metadata`. Since `make_event` uses `Bytes::new()` (empty metadata), the round-trip is trivially correct, and the codec tests already verify metadata preservation. Not a correctness gap, but adding `metadata` to the comparison would make the test strictly complete.

## Suggestions (non-blocking)

- The `tracing::warn!` message at line 158 redundantly includes `offset` both as a structured field and in the format string (`"truncating trailing partial/corrupt record at byte offset {offset}"`). Consider using either structured fields only (`tracing::warn!(offset, valid_events, "truncating trailing partial/corrupt record")`) or the format string only, to avoid log duplication. This is purely a logging style preference.

- The `seed_file` helper and `make_event` helper could be extracted to a shared test utility module (e.g., `src/test_helpers.rs` behind `#[cfg(test)]`) since the codec module has its own `make_event` helper with a slightly different signature. This would reduce duplication across test modules. However, for only two modules, inline helpers are acceptable.

## Scope Check

- Files within scope: YES -- only `src/store.rs` was modified, which is the sole file listed in the ticket scope.
- Scope creep detected: NO -- all changes serve the ticket's acceptance criteria. The 7 new tests are all directly mapped to ACs.
- Unauthorized dependencies added: NO -- no changes to `Cargo.toml`.

## Risk Assessment

- Regression risk: LOW -- Changes are additive (extending the existing `open()` function). The 3 Ticket-1 tests continue to pass unchanged. The new code path only activates when `path.exists()` is true, which the Ticket-1 tests avoid by using fresh tempdirs.
- Security concerns: NONE -- The store operates on local files with no network exposure at this layer.
- Performance concerns: NONE for normal operation. The `std::fs::read()` approach loads the entire file into memory, which is appropriate for the expected file sizes per the impl report. The `has_valid_record_after` scan is O(n*m) but only executes on the rare corruption path.
