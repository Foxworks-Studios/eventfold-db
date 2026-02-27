# Tickets for PRD 014: ListStreams RPC

**Source PRD:** prd/014-list-streams-rpc.md
**Created:** 2026-02-27
**Total Tickets:** 6
**Estimated Total Complexity:** 11 (Ticket 1 = S(1), Ticket 2 = M(2), Ticket 3 = M(2), Ticket 4 = M(2), Ticket 5 = M(2), Ticket 6 = M(2))

---

### Ticket 1: Add `StreamInfo` domain type and re-export

**Description:**
Add the `StreamInfo` struct to `src/types.rs` and re-export it from `src/lib.rs`. This is the foundational domain type that `ReadIndex::list_streams`, the service conversion helper, and the integration test all depend on. Nothing else can build until this type exists.

**Scope:**
- Modify: `src/types.rs` (add `StreamInfo` struct with doc comments and derives)
- Modify: `src/lib.rs` (add `StreamInfo` to the `pub use types::{...}` re-export list)

**Acceptance Criteria:**
- [ ] `StreamInfo` struct defined in `src/types.rs` with fields `stream_id: Uuid`, `event_count: u64`, `latest_version: u64`; derives `Debug, Clone, PartialEq, Eq`; all fields and the struct itself have doc comments matching the PRD spec.
- [ ] `StreamInfo` is re-exported from `src/lib.rs` so `crate::StreamInfo` resolves at the crate root.
- [ ] Test: construct `StreamInfo { stream_id: Uuid::new_v4(), event_count: 3, latest_version: 2 }`, clone it, assert the clone equals the original (verifies `Clone` + `PartialEq` + `Eq`).
- [ ] Test: construct two `StreamInfo` values with differing `event_count`, assert they are not equal (verifies `PartialEq`).
- [ ] Test: construct `StreamInfo` via `crate::StreamInfo { ... }` (crate-root path), assert `event_count` field reads back correctly (verifies re-export resolves).
- [ ] Quality gates pass: `cargo build`, `cargo clippy --all-targets --all-features --locked -- -D warnings`, `cargo test`, `cargo fmt --check`.

**Dependencies:** None
**Complexity:** S
**Maps to PRD AC:** AC 2 (field shape), AC 8 (zero warnings)

---

### Ticket 2: Add `ListStreams` messages and RPC to the proto file

**Description:**
Add `ListStreamsRequest`, `StreamInfo`, and `ListStreamsResponse` messages plus the `rpc ListStreams` declaration to `proto/eventfold.proto`. The existing `tonic_build::compile_protos` invocation in `build.rs` regenerates the Rust bindings automatically on `cargo build`, so no `build.rs` changes are needed. This ticket makes the generated proto types available to both `service.rs` and `eventfold-console/src/client.rs`.

**Scope:**
- Modify: `proto/eventfold.proto` (add `rpc ListStreams`, `message ListStreamsRequest {}`, `message StreamInfo`, `message ListStreamsResponse`)

**Acceptance Criteria:**
- [ ] `proto/eventfold.proto` contains `rpc ListStreams(ListStreamsRequest) returns (ListStreamsResponse);` inside the `EventStore` service.
- [ ] `message ListStreamsRequest {}` is defined (empty message, not reusing `Empty`).
- [ ] `message StreamInfo` is defined with fields `string stream_id = 1`, `uint64 event_count = 2`, `uint64 latest_version = 3`.
- [ ] `message ListStreamsResponse` is defined with `repeated StreamInfo streams = 1`.
- [ ] Test: `cargo build` succeeds, confirming prost/tonic code-gen completes without error and the generated `proto::ListStreamsRequest`, `proto::StreamInfo`, `proto::ListStreamsResponse` types are accessible via `crate::proto::ListStreamsRequest::default()`.
- [ ] Test: add a compile-time check in `src/lib.rs` `#[cfg(test)]` module: `let _: crate::proto::ListStreamsRequest = Default::default();` — verifies the generated type resolves at the `crate::proto::` path.
- [ ] Quality gates pass: `cargo build`, `cargo clippy --all-targets --all-features --locked -- -D warnings`, `cargo test`, `cargo fmt --check`.

**Dependencies:** Ticket 1 (no runtime dep, but proto changes should come after the domain type is established to keep the change set logical and reviewable)
**Complexity:** M
**Maps to PRD AC:** AC 1, AC 8

---

### Ticket 3: Implement `ReadIndex::list_streams` on the server library

**Description:**
Add the `list_streams()` method to `impl ReadIndex` in `src/reader.rs`. This method acquires a single `RwLock` read guard, iterates `EventLog::streams`, maps each entry to a domain `StreamInfo`, sorts lexicographically by `stream_id.to_string()`, and returns the `Vec`. Also add unit tests in the existing `#[cfg(test)]` module.

**Scope:**
- Modify: `src/reader.rs` (add `list_streams` method + unit tests)

**Acceptance Criteria:**
- [ ] `ReadIndex::list_streams(&self) -> Vec<StreamInfo>` is defined with a complete doc comment (including `# Returns` and a note about the single-lock invariant).
- [ ] Method acquires exactly one `RwLock` read guard (uses the `self.log.read().expect(...)` pattern already used by `read_stream` and `read_all`); does not iterate `log.events` or access event payload, metadata, or event-type fields.
- [ ] `latest_version` is computed as `positions.len() as u64 - 1`; safety is documented in a comment (invariant: any stream in `streams` has at least one position).
- [ ] Test: open a `Store`, append 3 events to stream A and 1 event to stream B; call `list_streams()`; assert result length is 2; assert A has `event_count=3, latest_version=2`; assert B has `event_count=1, latest_version=0`.
- [ ] Test: open a `Store` with no appends; call `list_streams()`; assert the result is an empty `Vec` (not a panic or error).
- [ ] Test: open a `Store`, append events to three streams whose UUID strings sort as C > A > B; call `list_streams()`; assert entries appear in order A, B, C (verifies lexicographic sort by `stream_id.to_string()`).
- [ ] Test: two `ReadIndex` clones backed by the same `Arc<RwLock<EventLog>>`; append via `Store`; both clones return identical `list_streams()` results (verifies shared-state consistency).
- [ ] Quality gates pass: `cargo build`, `cargo clippy --all-targets --all-features --locked -- -D warnings`, `cargo test`, `cargo fmt --check`.

**Dependencies:** Ticket 1 (`StreamInfo` type), Ticket 2 (not strictly required, but the domain type from Ticket 1 must exist)
**Complexity:** M
**Maps to PRD AC:** AC 2, AC 3, AC 4, AC 5, AC 8

---

### Ticket 4: Implement `list_streams` gRPC handler and `stream_info_to_proto` helper in `service.rs`

**Description:**
Add the `list_streams` RPC handler to the `EventStore` trait implementation on `EventfoldService`, and add the `stream_info_to_proto` conversion helper function. Wire them together following the existing `read_all` / `recorded_to_proto` pattern. Add unit tests for the conversion helper.

**Scope:**
- Modify: `src/service.rs` (add `list_streams` handler method on `EventfoldService`, add `stream_info_to_proto` public function, add unit tests)

**Acceptance Criteria:**
- [ ] `list_streams` handler is added to the `#[tonic::async_trait] impl proto::event_store_server::EventStore for EventfoldService`; it calls `self.read_index.list_streams()`, maps each entry through `stream_info_to_proto`, and returns `Ok(tonic::Response::new(proto::ListStreamsResponse { streams }))`.
- [ ] `pub fn stream_info_to_proto(s: StreamInfo) -> proto::StreamInfo` is defined with a doc comment; UUID is serialized as `s.stream_id.to_string()`.
- [ ] Calling `list_streams` on an empty store returns `Ok` with an empty `streams` list (no `NOT_FOUND` or any error status).
- [ ] Test (unit): construct a `StreamInfo { stream_id: known_uuid, event_count: 5, latest_version: 4 }`, call `stream_info_to_proto`, assert `proto_result.stream_id == known_uuid.to_string()`, `proto_result.event_count == 5`, `proto_result.latest_version == 4`.
- [ ] Test (async, using the existing `temp_service()` helper): call `service.list_streams(tonic::Request::new(proto::ListStreamsRequest {}))` on an empty service; assert `response.into_inner().streams` is empty.
- [ ] Test (async): append 2 events to one stream via `service.append`, then call `service.list_streams`; assert the response contains exactly 1 `StreamInfo` entry with `event_count=2`, `latest_version=1`, and `stream_id` matching the appended stream's UUID string.
- [ ] Quality gates pass: `cargo build`, `cargo clippy --all-targets --all-features --locked -- -D warnings`, `cargo test`, `cargo fmt --check`.

**Dependencies:** Ticket 1 (`StreamInfo`), Ticket 2 (proto types `ListStreamsRequest`, `ListStreamsResponse`, `StreamInfo`), Ticket 3 (`ReadIndex::list_streams`)
**Complexity:** M
**Maps to PRD AC:** AC 2, AC 3, AC 4, AC 8, AC 9

---

### Ticket 5: Refactor `eventfold-console` — replace `ReadAll` scan with `ListStreams` RPC; remove `collect_streams`

**Description:**
Replace the `Client::list_streams` scan-loop implementation in `eventfold-console/src/client.rs` with a single `ListStreams` RPC call, remove the `page_size` parameter, update the call site in `eventfold-console/src/main.rs`, and delete `AppState::collect_streams` plus its two unit tests from `eventfold-console/src/app.rs`. The `LIST_STREAMS_PAGE_SIZE` constant in `main.rs` becomes dead code and must also be removed. All changes must be done in one ticket because `client.rs` currently calls `AppState::collect_streams`; removing that method without simultaneously replacing the call site in `client.rs` would leave the crate uncompilable.

**Scope:**
- Modify: `eventfold-console/src/client.rs` (replace `list_streams` body; update imports to add `ListStreamsRequest`; remove old `ReadAll` loop; drop `page_size` parameter)
- Modify: `eventfold-console/src/app.rs` (delete `collect_streams` method and its two tests: `collect_streams_groups_by_stream_id`, `collect_streams_empty_input_returns_empty`)
- Modify: `eventfold-console/src/main.rs` (remove `LIST_STREAMS_PAGE_SIZE` constant; update `client.list_streams(LIST_STREAMS_PAGE_SIZE)` call to `client.list_streams()`)

**Acceptance Criteria:**
- [ ] `Client::list_streams(&mut self) -> Result<Vec<StreamInfo>, ConsoleError>` takes no `page_size` parameter; its body calls `self.inner.list_streams(ListStreamsRequest {}).await?.into_inner()` and maps each `proto::StreamInfo` to `app::StreamInfo`.
- [ ] The imports at the top of `client.rs` reference `ListStreamsRequest` from `eventfold_db::proto`; no import of `ReadAllRequest` remains solely for `list_streams` (other callers of `ReadAllRequest` in `read_all` are unaffected).
- [ ] `AppState::collect_streams` is entirely absent from `app.rs` with no remaining `collect_streams` references in the file.
- [ ] `LIST_STREAMS_PAGE_SIZE` constant is absent from `main.rs`; the call site reads `client.list_streams().await`.
- [ ] Test (unit, `client.rs`): verify the `Client` type is still `Debug` and `Clone` (compile-time trait check, already exists — confirm it still passes).
- [ ] Test (unit, `app.rs`): verify `AppState` unit tests still compile and pass; no reference to `collect_streams` anywhere in `app.rs` tests.
- [ ] `cargo grep collect_streams` (or equivalent Grep) returns zero matches across the workspace.
- [ ] Quality gates pass: `cargo build`, `cargo clippy --all-targets --all-features --locked -- -D warnings`, `cargo test`, `cargo fmt --check`.

**Dependencies:** Ticket 1 (domain `StreamInfo` must not conflict with `app::StreamInfo` — note: `app::StreamInfo` is the console-local struct, which maps from the proto type; no collision), Ticket 2 (proto `ListStreamsRequest` must be generated), Ticket 4 (the server-side handler must exist for the integration test to pass, but this ticket's unit tests can pass against any compiled proto)
**Complexity:** M
**Maps to PRD AC:** AC 6, AC 7, AC 8, AC 9

---

### Ticket 6: Add `ListStreams` gRPC integration test and run full verification

**Description:**
Add a `list_streams_*` test suite in `tests/grpc_service.rs` using the existing `start_test_server` infrastructure. The tests cover: three streams with varying counts, correct `stream_id`/`event_count`/`latest_version` values, lexicographic sort order, and the empty-store case. Then run the full acceptance criteria checklist end-to-end.

**Scope:**
- Modify: `tests/grpc_service.rs` (add integration tests for `ListStreams`)

**Acceptance Criteria:**
- [ ] Test `list_streams_on_empty_store_returns_empty`: start server, call `ListStreams`, assert `response.streams` is empty and the call returns `OK` (no error status).
- [ ] Test `list_streams_returns_correct_metadata_for_multiple_streams`: start server; append 1 event to stream A, 2 events to stream B, 3 events to stream C (use fixed UUIDs so sort order is known); call `ListStreams`; assert result contains exactly 3 entries; assert each entry's `stream_id` (UUID hyphenated lowercase string), `event_count`, and `latest_version` are correct.
- [ ] Test `list_streams_results_are_sorted_lexicographically`: use the same 3-stream setup; assert entries are in ascending lexicographic order by `stream_id` string (i.e., `entries[i].stream_id <= entries[i+1].stream_id` for all i).
- [ ] Test `list_streams_reflects_additional_appends`: start server; append 1 event to stream A; call `ListStreams`; assert count=1, latest_version=0; then append 2 more events to stream A; call `ListStreams` again; assert count=3, latest_version=2 (verifies the in-memory index is live).
- [ ] All PRD ACs 1-10 pass (see matrix below).
- [ ] No regressions: all previously passing tests in `tests/grpc_service.rs` still pass.
- [ ] `cargo build 2>&1 | tail -1` shows no errors.
- [ ] `cargo clippy --all-targets --all-features --locked -- -D warnings` exits 0.
- [ ] `cargo test` exits 0 (all tests in the entire workspace green).
- [ ] `cargo fmt --check` exits 0.

**Dependencies:** All previous tickets (1-5)
**Complexity:** M
**Maps to PRD AC:** AC 1, AC 2, AC 3, AC 4, AC 5, AC 6, AC 7, AC 8, AC 9, AC 10

---

## AC Coverage Matrix

| PRD AC # | Description | Covered By Ticket(s) | Status |
|----------|-------------|----------------------|--------|
| 1 | `proto/eventfold.proto` defines `rpc ListStreams`, `ListStreamsRequest`, `StreamInfo`, `ListStreamsResponse` with correct field numbers | Ticket 2, Ticket 6 | Covered |
| 2 | `ListStreams` on N-stream store returns exactly N entries with correct `stream_id`, `event_count`, `latest_version` | Ticket 1 (type shape), Ticket 3 (logic), Ticket 4 (handler), Ticket 6 (integration test) | Covered |
| 3 | `ListStreams` on empty store returns `OK` with empty `streams` list (not an error status) | Ticket 4 (unit), Ticket 6 (integration test) | Covered |
| 4 | Returned `StreamInfo` entries are sorted in ascending lexicographic order by `stream_id` string | Ticket 3 (sort impl + unit test), Ticket 6 (integration test) | Covered |
| 5 | `ReadIndex::list_streams()` acquires exactly one `RwLock` read guard and does not access event payload, metadata, or event-type fields | Ticket 3 (implementation + doc comment), Ticket 6 (verified via code review / grep) | Covered |
| 6 | `eventfold-console Client::list_streams` calls `ListStreams` RPC; does not call `ReadAll`; `page_size` parameter removed | Ticket 5, Ticket 6 | Covered |
| 7 | `AppState::collect_streams` is deleted from `eventfold-console/src/app.rs` with no remaining references | Ticket 5, Ticket 6 | Covered |
| 8 | `cargo build` at workspace root produces zero warnings for both `eventfold-db` and `eventfold-console` | Ticket 1-6 (each ticket requires quality gates) | Covered |
| 9 | `cargo clippy --all-targets --all-features --locked -- -D warnings` passes clean for the entire workspace | Ticket 1-6 (each ticket requires quality gates) | Covered |
| 10 | `cargo test` passes for the entire workspace, including the new `ListStreams` gRPC integration test | Ticket 6 | Covered |
