# Implementation Report: Ticket 2 -- Add `ListStreams` messages and RPC to the proto file

**Ticket:** 2 - Add `ListStreams` messages and RPC to the proto file
**Date:** 2026-02-27 12:00
**Status:** COMPLETE

---

## Files Changed

### Created
- None

### Modified
- `proto/eventfold.proto` - Added `rpc ListStreams(ListStreamsRequest) returns (ListStreamsResponse)` to the `EventStore` service, and added `message ListStreamsRequest {}`, `message StreamInfo { ... }`, and `message ListStreamsResponse { ... }` message definitions.
- `src/lib.rs` - Added `proto_list_streams_types_resolve` test in the existing `#[cfg(test)]` module to verify the generated proto types resolve at `crate::proto::` path.
- `src/service.rs` - Added a stub `list_streams` handler returning `UNIMPLEMENTED` to satisfy the tonic-generated trait requirement (adding an RPC to the proto makes the trait method mandatory for compilation).

## Implementation Notes
- Adding an RPC to the proto file causes `tonic_build` to generate a trait method that the existing `EventfoldService` impl must implement. Without a stub in `service.rs`, `cargo build` fails with `E0046: not all trait items implemented, missing: list_streams`. The stub returns `tonic::Status::unimplemented(...)` following standard practice for newly-declared RPCs that aren't wired to domain logic yet.
- The `service.rs` change is a minimal, unavoidable side-effect of the proto change -- it's the absolute minimum code to make compilation succeed. The actual domain-wired implementation is ticket 4's responsibility.
- The test in `lib.rs` verifies all three generated types (`ListStreamsRequest`, `StreamInfo`, `ListStreamsResponse`) are constructable and have the expected fields. It was added to the existing `#[cfg(test)]` module, not a duplicate module.
- Proto message definitions follow the exact field numbers and types specified in the PRD.

## Acceptance Criteria
- [x] AC 1: `proto/eventfold.proto` contains `rpc ListStreams(ListStreamsRequest) returns (ListStreamsResponse);` inside the `EventStore` service -- added at line 10.
- [x] AC 2: `message ListStreamsRequest {}` is defined (empty message, not reusing `Empty`) -- added at line 89.
- [x] AC 3: `message StreamInfo` is defined with fields `string stream_id = 1`, `uint64 event_count = 2`, `uint64 latest_version = 3` -- added at lines 91-95.
- [x] AC 4: `message ListStreamsResponse` is defined with `repeated StreamInfo streams = 1` -- added at lines 97-99.
- [x] AC 5: `cargo build` succeeds, confirming prost/tonic code-gen completes without error.
- [x] AC 6: Compile-time check in `src/lib.rs` `#[cfg(test)]` module: `let _: crate::proto::ListStreamsRequest = Default::default();` -- added in the existing `#[cfg(test)]` module as the `proto_list_streams_types_resolve` test. Also verifies `StreamInfo` field construction and `ListStreamsResponse` with `repeated StreamInfo`.
- [x] AC 7: Quality gates pass: `cargo build`, `cargo clippy --all-targets --all-features --locked -- -D warnings`, `cargo test`, `cargo fmt --check` -- all pass clean.

## Test Results
- Build: PASS (`cargo build` -- zero errors, zero warnings)
- Lint: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings` -- zero warnings)
- Tests: PASS (`cargo test` -- 288 tests, 0 failures)
- Fmt: PASS (`cargo fmt --check` -- no formatting issues)
- New tests added:
  - `src/lib.rs::tests::proto_list_streams_types_resolve` -- verifies `ListStreamsRequest`, `StreamInfo`, and `ListStreamsResponse` generated types resolve at `crate::proto::` path with correct fields

## Concerns / Blockers
- `src/service.rs` was modified to add a stub `list_streams` handler, which is technically outside the stated file scope of `proto/eventfold.proto`. However, this is an unavoidable consequence of adding the RPC -- tonic's code-gen creates a mandatory trait method, and without the stub, `cargo build` fails. The stub returns `UNIMPLEMENTED` and will be replaced by the real implementation in ticket 4.
- Ticket 1's prior work (in the working tree but uncommitted) includes `StreamInfo` domain type in `types.rs`, `list_streams()` method on `ReadIndex` in `reader.rs`, and `StreamInfo` re-export in `lib.rs`. These are NOT my changes but are present in the working tree.
