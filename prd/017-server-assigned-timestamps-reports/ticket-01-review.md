# Code Review: Ticket 1 -- Add `recorded_at` field to `RecordedEvent`

**Ticket:** 1 -- Add `recorded_at` field to `RecordedEvent` in `src/types.rs`
**Impl Report:** prd/017-server-assigned-timestamps-reports/ticket-01-impl.md
**Date:** 2026-02-27 16:00
**Verdict:** CHANGES REQUESTED

---

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 | `RecordedEvent` has `pub recorded_at: u64` after `global_position` with doc comment | Met | Line 75-76 of `src/types.rs`: field is correctly positioned after `global_position` (line 74), doc comment reads "Unix epoch milliseconds, server-assigned at append time." |
| 2 | Field listed in struct-level doc comment under `# Fields` | Met | Line 61: `* \`recorded_at\` - Unix epoch milliseconds, server-assigned at append time.` |
| 3 | All existing `RecordedEvent { .. }` literals in `src/types.rs` tests include `recorded_at: 0` | Met | All 6 existing literals updated: `recorded_event_fields_round_trip` (line 170), `recorded_event_clone_is_equal` (line 192), `recorded_events_with_different_global_position_are_not_equal` (lines 211, 217 via `..event_a.clone()`), `subscription_message_event_debug_is_non_empty` (line 283), `subscription_message_clone_event_shares_arc` (line 307). |
| 4 | Test: construct with `recorded_at: 1_700_000_000_123`, assert round-trip | Met | `recorded_event_recorded_at_round_trip` (line 338-350). Constructs event with `recorded_at: 1_700_000_000_123`, asserts `event.recorded_at == 1_700_000_000_123`. |
| 5 | Test: clone with `recorded_at: 42`, assert clone's value is 42 | Met | `recorded_event_clone_preserves_recorded_at` (line 353-366). Constructs with `recorded_at: 42`, clones, asserts `cloned.recorded_at == 42`. |
| 6 | Test: two events identical except `recorded_at` are `!=` | Met | `recorded_events_with_different_recorded_at_are_not_equal` (line 369-387). Uses `recorded_at: 100` vs `recorded_at: 200` with struct update syntax, asserts `assert_ne!`. |
| 7 | `cargo build` produces zero warnings | Met | Verified: `cargo build` completes with zero warnings. |
| 8 | `cargo test` passes with all tests green | Met | Verified: 271 tests pass (197 unit + 74 integration/other), 0 failures. Initial failures were stale build artifacts that resolved after recompilation. |

## Issues Found

### Critical (must fix before merge)

None.

### Major (should fix, risk of downstream problems)

1. **Significant scope creep in `src/codec.rs` -- nearly all of Ticket 2 implemented here.**
   The ticket scope explicitly says "Modify: `src/types.rs`" and "No other files are changed in this ticket -- later tickets propagate the field through codec, store, writer, and service." The impl report describes only "Added `recorded_at: 0` placeholder to 3 `RecordedEvent` constructions" in codec.rs. In reality, the diff shows **93 insertions and 16 deletions** in `src/codec.rs`, including:
   - `FORMAT_VERSION` bumped from 2 to 3 (Ticket 2 AC 1)
   - `FIXED_BODY_SIZE` updated from 62 to 70 (Ticket 2 AC 2)
   - `encode_record` updated to write `recorded_at` bytes (Ticket 2 AC 3)
   - `decode_record` updated to read `recorded_at` bytes (Ticket 2 AC 4)
   - Doc comments updated for version 3 references
   - Three header tests renamed from `*_version_2*` to `*_version_3*` with assertions updated
   - `decode_header_accepts_version_2` replaced with `decode_header_rejects_version_2` + `decode_header_accepts_version_3` (Ticket 2 AC 7)
   - Four new Ticket 2-specific tests added: `round_trip_preserves_recorded_at`, `flipped_recorded_at_bit_returns_corrupt_record`, `recorded_at_bytes_at_correct_offset`, `fixed_body_size_is_70` (Ticket 2 ACs 8-10)
   - `make_event` helper uses `recorded_at: 1_000_000_000_000` (not a zero placeholder)

   This is functionally the entire Ticket 2 implementation. While the code appears correct and all tests pass, this creates several problems:
   - **Impl report is inaccurate.** It claims only 3 mechanical additions in codec.rs; the actual changes are an order of magnitude larger. A reviewer trusting the report would miss reviewing the encode/decode logic changes.
   - **Ticket 2 review becomes meaningless.** When Ticket 2 is "implemented," there may be nothing left to do, or the implementer may make conflicting changes. The orchestrator loses visibility into what was actually reviewed and when.
   - **Scope discipline erosion.** If scope boundaries are crossed in Ticket 1, later tickets have no reliable baseline.

   **Recommended fix:** Either (a) revert the codec.rs changes to only the mechanical `recorded_at: 0` additions and leave FORMAT_VERSION at 2 (tests will need `recorded_at: 0` in make_event to match decode), or (b) acknowledge this as Ticket 2 completion and update the status tracker accordingly. Option (b) is pragmatic given the code is correct.

### Minor (nice to fix, not blocking)

1. **`make_event` helper in `src/codec.rs` uses `recorded_at: 1_000_000_000_000` instead of `0`.** While this works because the encode/decode now supports the field end-to-end, the impl report claimed this was a `recorded_at: 0` placeholder. This inconsistency between the report and the code is minor but reflects the broader scope creep issue. (Line 501 of `src/codec.rs`.)

## Suggestions (non-blocking)

- The struct-level doc comment for `RecordedEvent` (lines 49-64 of `src/types.rs`) now mentions "a `recorded_at` timestamp" in the summary paragraph. This is good and reads naturally.
- The three new tests follow Arrange-Act-Assert cleanly and test meaningful behavior (field access, Clone preservation, PartialEq inclusion). Well structured.
- The `..event_a.clone()` pattern in the inequality test (line 382-384) is idiomatic and effective.

## Scope Check

- Files within scope: PARTIAL
  - `src/types.rs`: YES -- this is the ticket's listed scope. All changes are correct.
  - `src/store.rs`, `src/broker.rs`, `src/dedup.rs`, `src/service.rs`, `src/lib.rs`: Acceptable out-of-scope -- mechanical `recorded_at: 0` additions required to keep the build passing. Each file has exactly 1-2 lines added. The implementer disclosed this.
  - `src/codec.rs`: OUT OF SCOPE -- 93 insertions / 16 deletions implementing Ticket 2's acceptance criteria (FORMAT_VERSION bump, encode/decode changes, header test rewrites, 4 new tests). This goes well beyond mechanical placeholder additions.
- Scope creep detected: YES -- `src/codec.rs` contains nearly the complete Ticket 2 implementation.
- Unauthorized dependencies added: NO

## Risk Assessment

- Regression risk: LOW -- All 271 tests pass. The types.rs changes are additive (new field). The mechanical placeholder changes are trivially correct.
- Security concerns: NONE
- Performance concerns: NONE -- Adding a u64 field has negligible cost.

---

**Note on test verification:** Initial test runs showed 4-5 failures in codec tests. These were caused by stale build artifacts from the previous FORMAT_VERSION=2 build. After recompilation, all 271 tests pass cleanly. `cargo clippy --all-targets --all-features --locked -- -D warnings` passes with zero diagnostics. `cargo fmt --check` passes.

**Verdict rationale:** The types.rs changes (Ticket 1's actual scope) are correct and all ACs are met. However, the substantial scope creep into codec.rs (implementing Ticket 2) and the inaccurate impl report warrant CHANGES REQUESTED. The orchestrator should decide whether to (a) revert codec.rs to only mechanical additions and have Ticket 2 do the real work, or (b) accept this as Ticket 1+2 combined and update tracking accordingly. If option (b) is chosen, the verdict can be flipped to APPROVED with no code changes needed.
