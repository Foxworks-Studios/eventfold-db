# Implementation Report: Ticket 1 -- Add Cargo.toml dependencies and implement `types.rs`

**Ticket:** 1 - Add Cargo.toml dependencies and implement `types.rs`
**Date:** 2026-02-25 10:00
**Status:** COMPLETE

---

## Files Changed

### Created
- `src/types.rs` - Core domain types: `ProposedEvent`, `RecordedEvent`, `ExpectedVersion`, `MAX_EVENT_SIZE`, `MAX_EVENT_TYPE_LEN`, and 12 unit tests covering AC-1 through AC-4.
- `src/lib.rs` - Crate root declaring `pub mod types;` and re-exporting all public items. (See note below about scope.)

### Modified
- `Cargo.toml` - Added `bytes = "1"`, `thiserror = "2"`, `uuid = { version = "1", features = ["v4", "v7"] }` to `[dependencies]`.

## Implementation Notes
- **Out-of-scope file: `src/lib.rs`** -- The ticket scope lists only `Cargo.toml` (modify) and `src/types.rs` (create). However, `src/lib.rs` did not exist, and without it, `types.rs` would be an orphan file that `cargo test` cannot discover. The PRD and CLAUDE.md both specify `lib.rs` as the crate root that declares `pub mod types;` and re-exports public items. I created a minimal `lib.rs` with exactly the module declaration and re-exports needed for `types.rs` to compile and test. This is documented as a concern below.
- **`thiserror` is in dependencies but not yet used in `types.rs`.** The ticket requires adding it to `Cargo.toml` for use by the `error.rs` module (ticket 2). It does not produce an unused-dependency warning because clippy does not check for unused crate-level dependencies.
- All types follow the field definitions from the PRD exactly: `ProposedEvent` (4 fields), `RecordedEvent` (7 fields), `ExpectedVersion` (3 variants).
- All public items have doc comments per CLAUDE.md conventions.
- Tests follow the red/green TDD pattern as specified. Each acceptance criterion has dedicated test functions.

## Acceptance Criteria
- [x] AC: `Cargo.toml` contains `bytes`, `thiserror`, and `uuid` dependencies at the specified versions with `v4`/`v7` features enabled on `uuid` -- verified in `Cargo.toml`.
- [x] AC: `ProposedEvent` has fields `event_id: Uuid`, `event_type: String`, `metadata: Bytes`, `payload: Bytes`; derives `Debug`, `Clone`, `PartialEq` -- implemented with all derives.
- [x] AC: `RecordedEvent` has all seven fields as specified in the PRD; derives `Debug`, `Clone`, `PartialEq` -- implemented with all 7 fields and derives.
- [x] AC: `ExpectedVersion` enum has variants `Any`, `NoStream`, and `Exact(u64)`; derives `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq` -- implemented with all derives.
- [x] AC: `MAX_EVENT_SIZE` equals `65536` and `MAX_EVENT_TYPE_LEN` equals `256`; both are `pub const usize` -- implemented as `pub const`.
- [x] AC-1 Test: `proposed_event_fields_round_trip` and `proposed_event_clone_is_equal` -- both pass.
- [x] AC-2 Test: `recorded_event_fields_round_trip`, `recorded_event_clone_is_equal`, and `recorded_events_with_different_global_position_are_not_equal` -- all pass.
- [x] AC-3 Test: `expected_version_any_is_copy`, `expected_version_no_stream_constructs`, `expected_version_exact_pattern_matches`, `expected_version_debug_is_non_empty`, `expected_version_exact_equality_and_inequality` -- all pass.
- [x] AC-4 Test: `max_event_size_is_65536` and `max_event_type_len_is_256` -- both pass.
- [x] AC: `cargo build` emits zero warnings; `cargo clippy -- -D warnings` passes; `cargo fmt --check` passes; `cargo test` is all green -- verified.

## Test Results
- Lint: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings` -- zero warnings)
- Tests: PASS (12 passed, 0 failed, 0 ignored)
- Build: PASS (zero warnings)
- Format: PASS (`cargo fmt --check` clean)
- New tests added: 12 tests in `src/types.rs` `#[cfg(test)]` module:
  - `proposed_event_fields_round_trip`
  - `proposed_event_clone_is_equal`
  - `recorded_event_fields_round_trip`
  - `recorded_event_clone_is_equal`
  - `recorded_events_with_different_global_position_are_not_equal`
  - `expected_version_any_is_copy`
  - `expected_version_no_stream_constructs`
  - `expected_version_exact_pattern_matches`
  - `expected_version_debug_is_non_empty`
  - `expected_version_exact_equality_and_inequality`
  - `max_event_size_is_65536`
  - `max_event_type_len_is_256`

## Concerns / Blockers
- **Out-of-scope file created: `src/lib.rs`** -- This file was not listed in the ticket scope but was required for `types.rs` to be part of the crate's module tree. Without it, the types module would not compile and tests would not run. The `lib.rs` is minimal (module declaration + re-exports) and matches the PRD's specification exactly. Ticket 2 (error.rs) will need to add `pub mod error;` and `pub use error::Error;` to this file.
- None otherwise. All acceptance criteria are met.
