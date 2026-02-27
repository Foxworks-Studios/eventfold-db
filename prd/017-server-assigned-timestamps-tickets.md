# Tickets for PRD 017: Server-Assigned Timestamps

**Source PRD:** prd/017-server-assigned-timestamps.md
**Created:** 2026-02-27
**Total Tickets:** 6
**Estimated Total Complexity:** 13 (S=1, M=2, L=3)

---

### Ticket 1: Add `recorded_at` field to `RecordedEvent` in `src/types.rs`

**Description:**
Add `pub recorded_at: u64` to the `RecordedEvent` struct in `src/types.rs` with full
doc comments. Update all `RecordedEvent` literal constructions in `src/types.rs` tests
to include the new field. No other files are changed in this ticket -- later tickets
propagate the field through codec, store, writer, and service.

**Scope:**
- Modify: `src/types.rs` (add field + update all `RecordedEvent` constructions in the
  `#[cfg(test)]` module: `recorded_event_fields_round_trip`, `recorded_event_clone_is_equal`,
  `recorded_events_with_different_global_position_are_not_equal`,
  `subscription_message_event_debug_is_non_empty`, `subscription_message_clone_event_shares_arc`,
  `async_stream_dependency_compiles`)

**Acceptance Criteria:**
- [ ] `RecordedEvent` in `src/types.rs` has a new `pub recorded_at: u64` field after
  `global_position`, with a doc comment: "Unix epoch milliseconds, server-assigned at append time."
- [ ] The field is listed in the struct-level doc comment under `# Fields`.
- [ ] All existing `RecordedEvent { .. }` literals in `src/types.rs` tests include
  `recorded_at: 0` (or a specific nonzero sentinel, e.g. `1_700_000_000_000`, for tests that
  check `PartialEq`). No test in this file uses `..Default::default()` as `RecordedEvent`
  does not derive `Default`.
- [ ] Test: construct a `RecordedEvent` with `recorded_at: 1_700_000_000_123`, assert
  `event.recorded_at == 1_700_000_000_123`.
- [ ] Test: clone a `RecordedEvent` with `recorded_at: 42`, assert the clone's
  `recorded_at == 42` (field is preserved by `Clone`).
- [ ] Test: two `RecordedEvent`s that are identical except `recorded_at` differ are `!=`
  (confirms `PartialEq` includes the new field).
- [ ] `cargo build` produces zero warnings (all struct literals must include the new field).
- [ ] `cargo test` passes with all tests in `src/types.rs` green.

**Dependencies:** None
**Complexity:** S
**Maps to PRD AC:** AC 1

---

### Ticket 2: Update binary codec to format v3 with `recorded_at` in `src/codec.rs`

**Description:**
Bump `FORMAT_VERSION` from 2 to 3. Insert `recorded_at` (8 bytes, u64 LE) after
`global_position` in `encode_record` / `decode_record`. Update `FIXED_BODY_SIZE` constant
(+8 bytes). Update all `RecordedEvent` constructions in `src/codec.rs` tests to include the
new field. Update the two header-acceptance tests that hard-code version `2` to expect `3`.
`decode_header` already rejects any version other than `FORMAT_VERSION`, so format v2 files
will be rejected automatically with no additional code needed.

**Scope:**
- Modify: `src/codec.rs` (bump `FORMAT_VERSION`; update `FIXED_BODY_SIZE`; add `recorded_at`
  encode in `encode_record`; add `recorded_at` decode in `decode_record` after `global_position`;
  update all `RecordedEvent` constructions and header-version assertions in `#[cfg(test)]`)

**Acceptance Criteria:**
- [ ] `FORMAT_VERSION` constant is `3`.
- [ ] `FIXED_BODY_SIZE` is `70` (was 62; +8 for `recorded_at`). The comment on the constant
  reads: `global_position(8) + recorded_at(8) + stream_id(16) + stream_version(8) +
  event_id(16) + event_type_len(2) + metadata_len(4) + payload_len(4) + checksum(4) = 70`.
- [ ] `encode_record` writes `event.recorded_at` as u64 LE immediately after `global_position`.
- [ ] `decode_record` reads `recorded_at` as u64 LE immediately after `global_position` and
  populates `RecordedEvent::recorded_at`.
- [ ] CRC32 coverage: `recorded_at` bytes are inside the CRC-protected region (between
  `LENGTH_PREFIX_SIZE` and the final 4-byte checksum).
- [ ] `encode_header` returns bytes 4..8 encoding `3u32` LE.
- [ ] `decode_header` with a v2 header (magic `EFDB` + version `2` LE) returns
  `Err(Error::InvalidHeader(_))` -- verified by the existing
  `decode_header_accepts_version_2` test, which must be updated to assert version `3` instead.
- [ ] Test: construct a `RecordedEvent` with `recorded_at: 1_700_000_000_000`, encode it, decode
  it -- assert `decoded.recorded_at == 1_700_000_000_000` and all other fields match.
- [ ] Test: construct a `RecordedEvent` with `recorded_at: 0`, encode it, flip one bit at
  the `recorded_at` byte offset (bytes 12..20 of the full buffer, i.e., after the 4-byte
  length prefix and 8-byte `global_position`), recompute CRC -- `decode_record` returns
  `Err(Error::CorruptRecord { .. })` when the original CRC is kept (CRC mismatch).
- [ ] Test (field-boundary check): encode a `RecordedEvent` with `recorded_at: 0xDEAD_BEEF_CAFE_1234_u64`,
  assert bytes 12..20 of the buffer equal `0xDEAD_BEEF_CAFE_1234_u64.to_le_bytes()`.
- [ ] All existing round-trip tests (`ac3a`, `ac3b`, `ac3c`, `ac3d`, `ac7`) pass after adding
  `recorded_at` to `make_event` helper with a test sentinel value (e.g., `1_000_000_000_000`).
- [ ] `cargo test` passes -- all tests in `src/codec.rs` green.

**Dependencies:** Ticket 1
**Complexity:** M
**Maps to PRD AC:** AC 4, AC 5

---

### Ticket 3: Add `recorded_at` parameter to `Store::append` in `src/store.rs`

**Description:**
Add a `recorded_at: u64` parameter to `Store::append`. Use it when constructing each
`RecordedEvent` in the loop inside `append`. Update all `RecordedEvent` literal
constructions in `src/store.rs` tests, the `make_event` helper (if present), and every
direct call to `store.append(...)` in the store tests to pass a timestamp. No caller
outside `src/store.rs` tests calls `Store::append` directly (all production callers go
through `run_writer`), so this is the only file to update in this ticket.

**Scope:**
- Modify: `src/store.rs` (add `recorded_at: u64` parameter to `Store::append`; use it in
  the `RecordedEvent { .. }` construction; update the ~20 unit tests and any helper
  functions that call `store.append` or construct `RecordedEvent`)

**Acceptance Criteria:**
- [ ] `Store::append` signature is
  `pub fn append(&mut self, stream_id: Uuid, expected_version: ExpectedVersion,
  recorded_at: u64, proposed_events: Vec<ProposedEvent>) -> Result<Vec<RecordedEvent>, Error>`.
- [ ] Each `RecordedEvent` built inside `append` has `recorded_at` set to the `recorded_at`
  parameter passed in.
- [ ] All events in one `append` call share the same `recorded_at` value (the single
  parameter is applied to every event in the loop).
- [ ] Test: call `store.append(stream_id, Any, 1_700_000_000_000, vec![proposed])`, assert
  each returned `RecordedEvent` has `recorded_at == 1_700_000_000_000`.
- [ ] Test: call `store.append` twice with different `recorded_at` values on the same stream,
  assert the first batch's events have the first timestamp and the second batch's events have
  the second timestamp.
- [ ] Test: call `store.append` with `recorded_at: 0`, close and re-open the store
  (using `Store::open`), assert recovered events have `recorded_at == 0` (round-trip through
  codec preserves the value).
- [ ] All existing store unit tests pass (version-check tests, `WrongExpectedVersion` tests,
  `EventTooLarge` tests, recovery/truncation tests). Each test must supply a `recorded_at`
  argument (use `0` for tests where the value is not the focus).
- [ ] `cargo build` produces zero warnings.
- [ ] `cargo test` passes -- all tests in `src/store.rs` green.

**Dependencies:** Ticket 2
**Complexity:** M
**Maps to PRD AC:** AC 3, AC 5

---

### Ticket 4: Stamp `recorded_at` in the writer task in `src/writer.rs`

**Description:**
In `run_writer` (the writer loop), capture `recorded_at` via
`SystemTime::now().duration_since(UNIX_EPOCH).expect("...").as_millis() as u64` once
per batch (before the `for req in batch` loop -- so all requests drained in the same
batching pass share the same timestamp), then pass it to `store.append(...)`. Add the
necessary `std::time::{SystemTime, UNIX_EPOCH}` import. Update all `writer.rs` tests
that call `store.append` directly or construct `RecordedEvent` to include the new field.

**Scope:**
- Modify: `src/writer.rs` (add `recorded_at` capture; pass to `store.append`;
  add `SystemTime`/`UNIX_EPOCH` imports; update all `RecordedEvent` constructions and
  `store.append` calls in `#[cfg(test)]`)

**Acceptance Criteria:**
- [ ] `recorded_at` is stamped once per writer-loop iteration, before the
  `for req in batch` loop (not inside it), so all requests drained together share the
  same millisecond timestamp.
- [ ] The stamping expression is exactly:
  `SystemTime::now().duration_since(UNIX_EPOCH).expect("system clock before Unix epoch").as_millis() as u64`.
- [ ] `store.append(req.stream_id, req.expected_version, recorded_at, req.events)` receives
  the captured `recorded_at`.
- [ ] Test: append a single event via `WriterHandle::append`, receive the result, assert
  `recorded[0].recorded_at > 0` (timestamp is nonzero / plausible).
- [ ] Test: append a batch of 3 events in a single `WriterHandle::append` call, assert all
  three `RecordedEvent`s in the result have the same `recorded_at` value.
- [ ] All existing writer unit tests pass (dedup tests, `validate_batch_unique_ids` tests,
  channel-closed tests). Each test that constructs a `RecordedEvent` must include `recorded_at`.
- [ ] `cargo build` produces zero warnings.
- [ ] `cargo test` passes -- all tests in `src/writer.rs` green.

**Dependencies:** Ticket 3
**Complexity:** S
**Maps to PRD AC:** AC 2, AC 3

---

### Ticket 5: Add `recorded_at` to proto + `recorded_to_proto` in service + fix all callers

**Description:**
Add `uint64 recorded_at = 8` to the `RecordedEvent` message in `proto/eventfold.proto`.
Add `recorded_at: e.recorded_at` to `recorded_to_proto` in `src/service.rs`. Update
all `RecordedEvent` constructions in `src/service.rs` tests and all integration tests
(`tests/grpc_service.rs`, `tests/writer_integration.rs`, `tests/broker_integration.rs`,
`tests/idempotent_appends.rs`, `tests/metrics.rs`) to include `recorded_at`. Regenerated
proto code via `build.rs` will expose `recorded_at` on `proto::RecordedEvent`. Add an
assertion in the relevant integration test that `recorded_at > 0` on responses.

**Scope:**
- Modify: `proto/eventfold.proto` (add `uint64 recorded_at = 8` to `RecordedEvent` message)
- Modify: `src/service.rs` (`recorded_to_proto`: add `recorded_at: e.recorded_at`; update all
  `RecordedEvent` constructions in `#[cfg(test)]`)
- Modify: `tests/grpc_service.rs` (add `recorded_at` field to any `RecordedEvent` constructions;
  add assertion that `recorded_at > 0` on at least one read/subscribe response)
- Modify: `tests/writer_integration.rs` (update `RecordedEvent` constructions)
- Modify: `tests/broker_integration.rs` (update `RecordedEvent` constructions)
- Modify: `tests/idempotent_appends.rs` (update `RecordedEvent` constructions)
- Modify: `tests/metrics.rs` (update `RecordedEvent` constructions)

**Acceptance Criteria:**
- [ ] `proto/eventfold.proto` `RecordedEvent` message includes `uint64 recorded_at = 8;`
  with the comment `// Unix epoch milliseconds, server-assigned`.
- [ ] `recorded_to_proto` in `src/service.rs` maps `e.recorded_at` to the proto field.
- [ ] Test: `recorded_to_proto` with a `RecordedEvent` having `recorded_at: 1_700_000_000_000`
  produces a `proto::RecordedEvent` with `recorded_at == 1_700_000_000_000`.
- [ ] Integration test (`tests/grpc_service.rs`): append one event, call `ReadAll`, assert
  the returned `RecordedEvent.recorded_at > 0`.
- [ ] Integration test (`tests/grpc_service.rs`): append one event, call `ReadStream`, assert
  the returned `RecordedEvent.recorded_at > 0`.
- [ ] Integration test (`tests/grpc_service.rs`) or (`tests/grpc_service.rs`): `SubscribeAll`
  stream yields an event with `recorded_at > 0`.
- [ ] All existing integration tests compile and pass without newly introduced failures.
- [ ] `cargo build` produces zero warnings (proto codegen runs via `build.rs`).
- [ ] `cargo test` passes -- all tests in `src/service.rs` and all integration tests green.

**Dependencies:** Ticket 4
**Complexity:** L
**Maps to PRD AC:** AC 2, AC 6

---

### Ticket 6: Verification and integration check

**Description:**
Run the full PRD 017 acceptance criteria checklist end-to-end. Verify all tickets
integrate correctly as a cohesive feature: `recorded_at` is present on every
`RecordedEvent` path (read, subscribe, replay after re-open), FORMAT_VERSION is 3,
and no existing tests regress.

**Scope:**
- No new files. Read-only verification pass.

**Acceptance Criteria:**
- [ ] `cargo build` completes with zero errors and zero warnings.
- [ ] `cargo clippy --all-targets --all-features --locked -- -D warnings` passes with zero
  diagnostics.
- [ ] `cargo test` passes -- all tests green (all previous tests plus new timestamp tests).
- [ ] `cargo fmt --check` passes.
- [ ] Grep confirms `FORMAT_VERSION` is 3:
  `grep "FORMAT_VERSION" src/codec.rs` shows `const FORMAT_VERSION: u32 = 3`.
- [ ] Grep confirms `recorded_at` appears in all key files:
  `grep -l "recorded_at" src/types.rs src/codec.rs src/store.rs src/writer.rs src/service.rs proto/eventfold.proto`
  lists all six files.
- [ ] Grep confirms `recorded_at` is mapped in `recorded_to_proto`:
  `grep "recorded_at" src/service.rs` shows `recorded_at: e.recorded_at`.
- [ ] Grep confirms the timestamp is stamped before the batch `for` loop:
  `grep -n "SystemTime\|UNIX_EPOCH\|recorded_at" src/writer.rs` output shows the stamping
  expression before the `for req in batch` line in file order.
- [ ] All PRD AC pass: AC 1 (field exists), AC 2 (nonzero in read/subscribe responses),
  AC 3 (all events in a batch share the same value), AC 4 (FORMAT_VERSION == 3),
  AC 5 (encode/decode round-trip preserves `recorded_at`), AC 6 (proto field 8 populated),
  AC 7 (quality gates pass).

**Dependencies:** Ticket 1, Ticket 2, Ticket 3, Ticket 4, Ticket 5
**Complexity:** S
**Maps to PRD AC:** AC 1, AC 2, AC 3, AC 4, AC 5, AC 6, AC 7

---

## AC Coverage Matrix

| PRD AC # | Description | Covered By Ticket(s) | Status |
|----------|-------------|----------------------|--------|
| 1 | `RecordedEvent` in `src/types.rs` has `pub recorded_at: u64` | Ticket 1, Ticket 6 | Covered |
| 2 | Every `RecordedEvent` returned by reads and subscriptions has `recorded_at > 0` | Ticket 4, Ticket 5, Ticket 6 | Covered |
| 3 | All events in a single append batch share the same `recorded_at` value | Ticket 3, Ticket 4, Ticket 6 | Covered |
| 4 | `FORMAT_VERSION` in `src/codec.rs` is 3; old v2 data files are rejected | Ticket 2, Ticket 6 | Covered |
| 5 | Encode/decode round-trip recovers correct `recorded_at` millisecond values | Ticket 2, Ticket 3, Ticket 6 | Covered |
| 6 | gRPC `RecordedEvent` proto includes `recorded_at` as field 8; all four RPCs populate it | Ticket 5, Ticket 6 | Covered |
| 7 | All quality gates pass: `cargo build` (zero warnings), `cargo clippy`, `cargo test`, `cargo fmt --check` | Ticket 6 | Covered |
