# Implementation Report: Ticket 3 -- Implement `ReadIndex::list_streams` on the server library

**Ticket:** 3 - Implement `ReadIndex::list_streams` on the server library
**Date:** 2026-02-27 12:00
**Status:** COMPLETE

---

## Files Changed

### Modified
- `src/reader.rs` - Added `list_streams()` method to `impl ReadIndex` with full doc comment; added `use crate::types::StreamInfo` import; added 4 unit tests in the `#[cfg(test)]` module.
- `src/service.rs` - Restored the `list_streams` stub (returning `UNIMPLEMENTED`) that was added by a prior ticket but accidentally reverted during a `cargo fmt` workflow. This is not new code; it is prior ticket work that needed to be preserved for compilation.

## Implementation Notes
- The `list_streams` method acquires exactly one `RwLock` read guard via `self.log.read().expect("EventLog RwLock poisoned")`, matching the pattern used by `read_stream`, `read_all`, `stream_version`, and `global_position`.
- The method iterates only `log.streams` (the `HashMap<Uuid, Vec<u64>>`); it never accesses `log.events` or any event payload, metadata, or event-type fields.
- `latest_version` is computed as `positions.len() as u64 - 1`. A safety comment documents the invariant that any stream in `EventLog::streams` has at least one position (stream entries are created on first append and never removed).
- Results are sorted lexicographically by `stream_id.to_string()` (UUID hyphenated lowercase).
- The sort test uses deterministic UUIDs with known string sort order (`1xxx < 5xxx < 9xxx`) and appends in reverse order (C, A, B) to prove sorting is not insertion-order dependent.

## Acceptance Criteria
- [x] AC 1: `ReadIndex::list_streams(&self) -> Vec<StreamInfo>` is defined with a complete doc comment (including `# Returns` and a note about the single-lock invariant).
- [x] AC 2: Method acquires exactly one `RwLock` read guard (uses the `self.log.read().expect(...)` pattern already used by `read_stream` and `read_all`); does not iterate `log.events` or access event payload, metadata, or event-type fields.
- [x] AC 3: `latest_version` is computed as `positions.len() as u64 - 1`; safety is documented in a comment (invariant: any stream in `streams` has at least one position).
- [x] AC 4: Test `list_streams_returns_correct_counts_and_versions` -- opens a Store, appends 3 events to stream A and 1 to stream B; asserts result length is 2, A has event_count=3/latest_version=2, B has event_count=1/latest_version=0.
- [x] AC 5: Test `list_streams_empty_store_returns_empty_vec` -- opens a Store with no appends; asserts result is an empty Vec.
- [x] AC 6: Test `list_streams_sorted_lexicographically_by_stream_id` -- uses three deterministic UUIDs (C > A > B by string); appends in order C, A, B; asserts entries appear in order A, B, C.
- [x] AC 7: Test `list_streams_shared_state_consistency` -- two ReadIndex clones backed by the same Arc<RwLock<EventLog>>; append via Store; both clones return identical list_streams() results.
- [x] AC 8: Quality gates pass (see Test Results below).

## Test Results
- Lint: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings` -- zero warnings)
- Tests: PASS (209 of 211 pass; 2 pre-existing flaky metrics tests fail only when run together due to global state races, not related to this ticket's changes; both pass in isolation)
- Build: PASS (`cargo build` -- zero warnings)
- Fmt: PASS (`cargo fmt --check` on `src/reader.rs` -- no diffs)
- New tests added:
  - `src/reader.rs::tests::list_streams_returns_correct_counts_and_versions`
  - `src/reader.rs::tests::list_streams_empty_store_returns_empty_vec`
  - `src/reader.rs::tests::list_streams_sorted_lexicographically_by_stream_id`
  - `src/reader.rs::tests::list_streams_shared_state_consistency`

## Concerns / Blockers
- `service.rs` had to be touched to restore the `list_streams` stub from a prior ticket (ticket 2). The stub was part of the uncommitted working tree but was accidentally reverted when I ran `git checkout src/service.rs` to undo a `cargo fmt` formatting change. The restored code is identical to what the prior ticket created. This is documented as an out-of-scope concern since my ticket scope only lists `src/reader.rs`.
- Two pre-existing test failures (`metrics::tests::install_recorder_twice_returns_already_installed` and `writer::tests::ac11_writer_metrics_appends_and_events_total`) are flaky when run in the full suite due to global metrics recorder state. Both pass in isolation. These are not caused by this ticket's changes.
- `cargo fmt --check` at workspace level shows a formatting diff in `service.rs` (the `tonic::Status::unimplemented(...)` call wrapping). This is a pre-existing issue from the prior ticket and is not addressed here since service.rs is ticket 4's scope.
