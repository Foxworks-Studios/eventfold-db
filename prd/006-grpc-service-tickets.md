# Tickets for PRD 006: gRPC Service

**Source PRD:** prd/006-grpc-service.md
**Created:** 2026-02-26
**Total Tickets:** 6
**Estimated Total Complexity:** 12 (S + M + M + L + S + M = 1+2+2+3+1+2)

---

### Ticket 1: Add tonic/prost Dependencies and Proto Schema

**Description:**
Add `tonic`, `prost`, and `tonic-build` to `Cargo.toml` and create
`proto/eventfold.proto` with the full service definition (5 RPCs, all message
types). Create `build.rs` to invoke `tonic_build::compile_protos`. This is the
bedrock: nothing else in this PRD can compile until `tonic` is a dependency and
the generated types are available.

**Scope:**
- Modify: `Cargo.toml` (add `tonic = "0.13"`, `prost = "0.13"` under
  `[dependencies]`; add `tonic-build = "0.13"` under `[build-dependencies]`)
- Create: `proto/eventfold.proto` (complete proto3 schema with `EventStore`
  service, all 5 RPCs, and all message types as specified in the PRD)
- Create: `build.rs` (calls `tonic_build::compile_protos("proto/eventfold.proto")`)

**Acceptance Criteria:**
- [ ] `Cargo.toml` `[dependencies]` contains `tonic = "0.13"` and `prost = "0.13"`
- [ ] `Cargo.toml` `[build-dependencies]` contains `tonic-build = "0.13"`
- [ ] `proto/eventfold.proto` uses `syntax = "proto3"` and `package eventfold`
- [ ] `proto/eventfold.proto` defines the `EventStore` service with all 5 RPCs:
  `Append` (unary), `ReadStream` (unary), `ReadAll` (unary), `SubscribeAll`
  (server-streaming), `SubscribeStream` (server-streaming)
- [ ] `proto/eventfold.proto` defines all message types: `ProposedEvent`,
  `RecordedEvent`, `ExpectedVersion` (with `oneof kind { Empty any = 1; Empty
  no_stream = 2; uint64 exact = 3; }`), `Empty`, `AppendRequest`,
  `AppendResponse`, `ReadStreamRequest`, `ReadStreamResponse`, `ReadAllRequest`,
  `ReadAllResponse`, `SubscribeAllRequest`, `SubscribeStreamRequest`,
  `SubscribeResponse` (with `oneof content { RecordedEvent event = 1; Empty
  caught_up = 2; }`)
- [ ] `build.rs` invokes `tonic_build::compile_protos("proto/eventfold.proto")`
  and returns `Ok(())`
- [ ] Test: `cargo build` compiles without errors; the generated module is
  accessible via `include_proto!("eventfold")` or `tonic::include_proto!`
- [ ] Test: in a `#[cfg(test)]` block, verify that
  `eventfold::AppendRequest::default()` constructs successfully (confirms
  generated types are accessible)
- [ ] Quality gates pass: `cargo build`, `cargo clippy --all-targets
  --all-features --locked -- -D warnings`, `cargo fmt --check`, `cargo test`

**Dependencies:** None
**Complexity:** S
**Maps to PRD AC:** AC-22, AC-23 (partial: build compiles)

---

### Ticket 2: `EventfoldService` Struct, Error Mapping, and Request Validation Helpers

**Description:**
Create `src/service.rs` containing the `EventfoldService` struct (holding
`WriterHandle`, `ReadIndex`, and `Broker`) and two foundational building blocks:
the `error_to_status` mapping function and the request validation helpers
(`parse_uuid`, `validate_expected_version`, `validate_proposed_event`). These
helpers are used by every RPC handler in subsequent tickets; getting them right
and tested first prevents error-mapping bugs from hiding in integration tests
later.

**Scope:**
- Create: `src/service.rs` (struct definition, `impl EventfoldService { fn new
  }`, `fn error_to_status`, `fn parse_uuid`, `fn validate_expected_version`,
  conversion helpers: `proto_to_proposed_event`, `recorded_to_proto`,
  `proto_to_expected_version`, and their unit tests)
- Modify: `src/lib.rs` (add `pub mod service;`)

**Acceptance Criteria:**
- [ ] `EventfoldService` is a `pub struct` in `src/service.rs` with three fields:
  `writer: WriterHandle`, `read_index: ReadIndex`, `broker: Broker`
- [ ] `EventfoldService::new(writer: WriterHandle, read_index: ReadIndex, broker:
  Broker) -> Self` is implemented
- [ ] `fn error_to_status(err: Error) -> tonic::Status` maps all 7 `Error`
  variants to the correct gRPC codes per the PRD table:
  `WrongExpectedVersion` -> `FAILED_PRECONDITION`, `StreamNotFound` ->
  `NOT_FOUND`, `Io` -> `INTERNAL`, `CorruptRecord` -> `DATA_LOSS`,
  `InvalidHeader` -> `DATA_LOSS`, `EventTooLarge` -> `INVALID_ARGUMENT`,
  `InvalidArgument` -> `INVALID_ARGUMENT`; status message includes
  `err.to_string()`
- [ ] `fn parse_uuid(s: &str, field_name: &str) -> Result<Uuid, tonic::Status>`
  parses a UUID string; returns `Status::invalid_argument` with a descriptive
  message if parsing fails
- [ ] `fn proto_to_expected_version(ev: Option<proto::ExpectedVersion>) ->
  Result<ExpectedVersion, tonic::Status>` converts proto `ExpectedVersion` to
  domain `ExpectedVersion`; returns `Status::invalid_argument` if the `kind`
  field is `None` (unset)
- [ ] `fn proto_to_proposed_event(p: proto::ProposedEvent) ->
  Result<ProposedEvent, tonic::Status>` validates `event_id` is a valid UUID
  (returns `INVALID_ARGUMENT` if not) and converts to domain `ProposedEvent`
- [ ] `fn recorded_to_proto(e: &RecordedEvent) -> proto::RecordedEvent` converts
  a domain `RecordedEvent` to its proto representation
- [ ] Test: `error_to_status(Error::WrongExpectedVersion { expected: "0".into(),
  actual: "1".into() })` -> `status.code() == Code::FailedPrecondition` and
  `status.message()` contains `"wrong expected version"`
- [ ] Test: `error_to_status(Error::StreamNotFound { stream_id })` -> `Code::NotFound`
- [ ] Test: `error_to_status(Error::Io(io_err))` -> `Code::Internal`
- [ ] Test: `error_to_status(Error::CorruptRecord { .. })` -> `Code::DataLoss`
- [ ] Test: `error_to_status(Error::InvalidHeader(_))` -> `Code::DataLoss`
- [ ] Test: `error_to_status(Error::EventTooLarge { .. })` -> `Code::InvalidArgument`
- [ ] Test: `error_to_status(Error::InvalidArgument(_))` -> `Code::InvalidArgument`
- [ ] Test: `parse_uuid("not-a-uuid", "stream_id")` -> `Err(status)` with
  `status.code() == Code::InvalidArgument` and message containing `"stream_id"`
- [ ] Test: `parse_uuid(&Uuid::new_v4().to_string(), "stream_id")` -> `Ok(uuid)`
- [ ] Test: `proto_to_expected_version(None)` -> `Err(status)` with
  `Code::InvalidArgument`
- [ ] Test: `proto_to_expected_version(Some(proto with kind = any))` ->
  `Ok(ExpectedVersion::Any)`
- [ ] Test: `proto_to_expected_version(Some(proto with kind = no_stream))` ->
  `Ok(ExpectedVersion::NoStream)`
- [ ] Test: `proto_to_expected_version(Some(proto with exact = 7))` ->
  `Ok(ExpectedVersion::Exact(7))`
- [ ] Test: `proto_to_proposed_event` with invalid `event_id` UUID string ->
  `Err(Status::invalid_argument)`
- [ ] Test: `proto_to_proposed_event` with valid UUID -> `Ok(ProposedEvent)` with
  matching fields
- [ ] Test: `recorded_to_proto(event)` -> proto with all fields matching (event_id
  and stream_id round-trip as UUID strings; stream_version, global_position,
  event_type, metadata, payload match)
- [ ] Quality gates pass: `cargo build`, `cargo clippy --all-targets --all-features
  --locked -- -D warnings`, `cargo fmt --check`, `cargo test`

**Dependencies:** Ticket 1
**Complexity:** M
**Maps to PRD AC:** AC-3, AC-4, AC-5, AC-6, AC-7 (error mapping prerequisite),
AC-10, AC-11 (UUID validation prerequisite)

---

### Ticket 3: Implement `Append`, `ReadStream`, and `ReadAll` Unary RPCs

**Description:**
Implement the three unary RPCs on `EventfoldService` by writing the tonic
`EventStore` service trait impl in `src/service.rs`: `append`, `read_stream`,
and `read_all`. Each handler validates its request, delegates to the writer or
read index, maps results to proto responses, and maps errors to gRPC status
codes using the helpers from Ticket 2.

**Scope:**
- Modify: `src/service.rs` (add `#[tonic::async_trait] impl eventfold::event_store_server::EventStore for EventfoldService` with `append`, `read_stream`, `read_all` methods)

**Acceptance Criteria:**
- [ ] `EventfoldService` implements the tonic-generated `EventStore` trait
- [ ] `append` handler:
  - validates `stream_id` via `parse_uuid`; returns `INVALID_ARGUMENT` on failure
  - validates `expected_version` is set; returns `INVALID_ARGUMENT` if `None`
  - validates `events` list is non-empty; returns `INVALID_ARGUMENT` if empty
  - validates each event's `event_id` via `proto_to_proposed_event`
  - calls `self.writer.append(stream_id, expected_version, events).await`
  - on `Ok(recorded)`: returns `AppendResponse` with `first_stream_version`,
    `last_stream_version`, `first_global_position`, `last_global_position`
  - on `Err(e)`: returns `error_to_status(e)`
- [ ] `read_stream` handler:
  - validates `stream_id` via `parse_uuid`
  - calls `self.read_index.read_stream(stream_id, from_version, max_count)`
  - on `Ok(events)`: returns `ReadStreamResponse` with events converted via
    `recorded_to_proto`
  - on `Err(StreamNotFound)`: returns `Status::not_found`
- [ ] `read_all` handler:
  - calls `self.read_index.read_all(from_position, max_count)` (infallible)
  - returns `ReadAllResponse` with events converted via `recorded_to_proto`
- [ ] Test (AC-1): use the `start_test_server` helper (defined in this ticket in
  `tests/grpc_service.rs`); append 1 event to a new stream with `no_stream`
  expected version; assert response has `first_stream_version = 0`,
  `last_stream_version = 0`, `first_global_position = 0`,
  `last_global_position = 0`
- [ ] Test (AC-2): append 3 events to a new stream with `no_stream`; assert
  `first_stream_version = 0`, `last_stream_version = 2`,
  `first_global_position = 0`, `last_global_position = 2`
- [ ] Test (AC-3): append to stream A with `no_stream`; append again with
  `no_stream`; assert second call returns `FAILED_PRECONDITION`
- [ ] Test (AC-4): append with `stream_id = "not-a-uuid"`; assert
  `INVALID_ARGUMENT`
- [ ] Test (AC-5): append with empty `events` list; assert `INVALID_ARGUMENT`
- [ ] Test (AC-6): append with an event whose `event_id` is `"bad-uuid"`; assert
  `INVALID_ARGUMENT`
- [ ] Test (AC-7): append with a payload of `vec![0u8; 65537]`; assert
  `INVALID_ARGUMENT`
- [ ] Test (AC-8): append 5 events; `read_stream` from version 0, max_count 100;
  assert response has 5 events in stream_version order 0..4
- [ ] Test (AC-9): append 5 events; `read_stream` from version 2, max_count 2;
  assert response has 2 events at stream_versions 2 and 3
- [ ] Test (AC-10): `read_stream` for a stream that does not exist; assert
  `NOT_FOUND`
- [ ] Test (AC-11): `read_stream` with `stream_id = "not-a-uuid"`; assert
  `INVALID_ARGUMENT`
- [ ] Test (AC-12): append 5 events across 2 streams (3 to stream A, 2 to stream
  B); `read_all` from position 0, max_count 100; assert 5 events in
  global_position order 0..4
- [ ] Test (AC-13): append 5 events; `read_all` from position 3, max_count 2;
  assert 2 events at global_positions 3 and 4
- [ ] Test (AC-14): `read_all` from position 0 on an empty store; assert response
  has 0 events
- [ ] The `start_test_server` helper is defined in `tests/grpc_service.rs`:
  `async fn start_test_server() -> (EventStoreClient<Channel>, SocketAddr, TempDir)`
  — binds on `[::1]:0` (port 0), spawns the server task, and returns the
  connected client, the bound address, and a `TempDir` for the data directory
- [ ] Quality gates pass: `cargo build`, `cargo clippy --all-targets --all-features
  --locked -- -D warnings`, `cargo fmt --check`, `cargo test`

**Dependencies:** Ticket 2
**Complexity:** M
**Maps to PRD AC:** AC-1, AC-2, AC-3, AC-4, AC-5, AC-6, AC-7, AC-8, AC-9,
AC-10, AC-11, AC-12, AC-13, AC-14

---

### Ticket 4: Implement `SubscribeAll` and `SubscribeStream` Server-Streaming RPCs

**Description:**
Implement the two server-streaming RPCs on `EventfoldService`: `subscribe_all`
and `subscribe_stream`. Each handler calls the corresponding broker function,
maps the `Stream<Item = Result<SubscriptionMessage, Error>>` to a
`Stream<Item = Result<SubscribeResponse, Status>>`, and returns it as a tonic
server-streaming response. The mapping converts `SubscriptionMessage::Event` to
`SubscribeResponse` with the `event` oneof field set, and
`SubscriptionMessage::CaughtUp` to `SubscribeResponse` with the `caught_up`
field set.

**Scope:**
- Modify: `src/service.rs` (add `subscribe_all` and `subscribe_stream` methods
  to the `EventStore` impl; add `subscribe_response_stream` helper that maps
  `SubscriptionMessage` -> `SubscribeResponse` and `Error` ->
  `Status::internal`)
- Modify: `tests/grpc_service.rs` (add subscription integration tests)

**Acceptance Criteria:**
- [ ] `subscribe_all` handler:
  - validates no input UUID (no field to validate for `SubscribeAllRequest`)
  - calls `crate::subscribe_all(self.read_index.clone(), &self.broker,
    request.into_inner().from_position).await`
  - maps the resulting stream via `StreamExt::map`: `SubscriptionMessage::Event(arc)`
    -> `Ok(SubscribeResponse { content: Some(subscribe_response::Content::Event(recorded_to_proto(&arc))) })`;
    `SubscriptionMessage::CaughtUp` -> `Ok(SubscribeResponse { content: Some(subscribe_response::Content::CaughtUp(Empty {})) })`;
    `Err(e)` -> `Err(error_to_status(e))`
  - returns `Ok(Response::new(Box::pin(mapped_stream)))`
- [ ] `subscribe_stream` handler:
  - validates `stream_id` via `parse_uuid`; returns `INVALID_ARGUMENT` on failure
  - calls `crate::subscribe_stream(self.read_index.clone(), &self.broker,
    stream_id, request.into_inner().from_version).await`
  - maps and returns the stream identically to `subscribe_all`
- [ ] Test (AC-15): append 3 events; start `SubscribeAll` from position 0; collect
  messages via `streaming.message().await` until a `CaughtUp` is received;
  assert 3 `Event` messages received with global_positions 0, 1, 2; then append
  2 more events; collect 2 more messages; assert global_positions 3 and 4
- [ ] Test (AC-16): append 5 events; start `SubscribeAll` from position 3;
  collect until `CaughtUp`; assert 2 `Event` messages with global_positions 3
  and 4
- [ ] Test (AC-17): append events: stream A, stream B, stream A; start
  `SubscribeStream` for stream A from version 0; collect until `CaughtUp`; assert
  2 events both with `stream_id == A`; append to stream A; collect 1 more
  message; assert it has `stream_id == A` and `stream_version == 2`; confirm no
  stream B events appear
- [ ] Test (AC-18): start `SubscribeStream` for stream A; append to streams B, C,
  A, B; collect 1 message (with timeout); assert it is stream A's event and
  nothing else arrives within 100ms
- [ ] Test (AC-19): start `SubscribeStream` for a stream UUID that does not exist;
  collect first message; assert it is `CaughtUp`; append to that stream; collect
  1 live event; assert it has `stream_version == 0`
- [ ] Test (AC-20): start `SubscribeAll` on an empty store; without consuming
  responses, append enough events to overflow the broadcast buffer (use a
  `Broker::new(4)` capacity in the test server for this test); poll the response
  stream; assert it eventually returns an error status or closes (the connection
  drops or the stream terminates)
- [ ] Test (AC-21): start 2 `SubscribeAll` subscriptions; append 3 events;
  collect 3 events from each subscription; assert both receive the same
  global_positions 0, 1, 2
- [ ] Quality gates pass: `cargo build`, `cargo clippy --all-targets --all-features
  --locked -- -D warnings`, `cargo fmt --check`, `cargo test`

**Dependencies:** Ticket 3
**Complexity:** L
**Maps to PRD AC:** AC-15, AC-16, AC-17, AC-18, AC-19, AC-20, AC-21

---

### Ticket 5: Register `EventfoldService` in `lib.rs` and Re-export Public Items

**Description:**
Wire the service module into the crate's public API. Re-export
`EventfoldService` from `lib.rs` so that the binary (`main.rs`) in PRD 007 can
import it by the crate-root path. Confirm the generated proto module is
accessible via a stable `pub use` path. This is the integration seam between
the service implementation and the rest of the crate.

**Scope:**
- Modify: `src/lib.rs` (add `pub use service::EventfoldService;`; confirm the
  proto include macro is placed in a pub module, e.g. `pub mod proto { tonic::include_proto!("eventfold"); }`)

**Acceptance Criteria:**
- [ ] `eventfold_db::EventfoldService` is accessible at the crate root
- [ ] `eventfold_db::proto::event_store_server::EventStoreServer` is accessible at
  the crate root (or via `eventfold_db::proto`) so `main.rs` can build the
  tonic server without importing from internal modules
- [ ] Test: in a `#[cfg(test)]` block in `lib.rs`, verify
  `crate::EventfoldService` compiles as a type reference (no construction
  needed; just `let _: fn(_, _, _) -> crate::EventfoldService =
  crate::EventfoldService::new;` confirms the `new` constructor is callable)
- [ ] Test: `crate::proto::event_store_server::EventStoreServer::<crate::EventfoldService>::new` compiles as a type path expression (confirms tonic server wrapping is accessible)
- [ ] All existing tests from Tickets 1-4 continue to pass without modification
- [ ] Quality gates pass: `cargo build`, `cargo clippy --all-targets --all-features
  --locked -- -D warnings`, `cargo fmt --check`, `cargo test`

**Dependencies:** Ticket 4
**Complexity:** S
**Maps to PRD AC:** AC-22, AC-23 (partial: re-exports wired up)

---

### Ticket 6: Verification and Integration

**Description:**
Run the complete PRD 006 acceptance criteria checklist end-to-end. Confirm all
gRPC RPCs (unary and streaming) work correctly through a real tonic client-server
round-trip. Verify that no regressions exist in PRDs 001-005 tests, all quality
gates pass clean, and all public items are accessible at the crate root. The
`start_test_server` helper (introduced in Ticket 3) is already in
`tests/grpc_service.rs`; this ticket adds a final end-to-end smoke test
covering all 5 RPCs in a single test run.

**Scope:**
- Modify: `tests/grpc_service.rs` (add `all_five_rpcs_smoke_test` integration
  test that exercises every RPC at least once)

**Acceptance Criteria:**
- [ ] All PRD 006 ACs (AC-1 through AC-23) pass with `cargo test`
- [ ] `cargo test` output shows zero failures across the full crate (PRDs 001-006
  tests all green)
- [ ] `cargo build` completes with zero warnings
- [ ] `cargo clippy --all-targets --all-features --locked -- -D warnings` passes
  with zero diagnostics
- [ ] `cargo fmt --check` passes
- [ ] `eventfold_db::EventfoldService`, `eventfold_db::proto::event_store_server::EventStoreServer`
  are accessible at the crate root
- [ ] Test (smoke): open a store in a tempdir; create a `Broker::new(64)`; spawn the
  writer; construct `EventfoldService::new(writer_handle, read_index, broker)`;
  wrap in `EventStoreServer::new`; bind on port 0 and connect a tonic client;
  call `Append` (1 event, `no_stream`); call `ReadStream` (from version 0);
  call `ReadAll` (from position 0); call `SubscribeAll` (from position 0,
  collect until `CaughtUp`); call `SubscribeStream` for the appended stream
  (from version 0, collect until `CaughtUp`); assert all 5 responses are
  non-error and the event counts and positions are consistent
- [ ] Test (error codes): call `Append` with invalid `stream_id` -> `INVALID_ARGUMENT`;
  call `ReadStream` for non-existent stream -> `NOT_FOUND`; call `Append` with
  `no_stream` twice to same stream -> `FAILED_PRECONDITION`; all confirmed in a
  single test function to prove error mapping is wired end-to-end

**Dependencies:** Tickets 1, 2, 3, 4, 5
**Complexity:** M
**Maps to PRD AC:** AC-22, AC-23

---

## AC Coverage Matrix

| PRD AC # | Description                                                                    | Covered By Ticket(s) | Status  |
|----------|--------------------------------------------------------------------------------|----------------------|---------|
| AC-1     | Append happy path: 1 event, `no_stream`, correct first/last positions          | Ticket 3             | Covered |
| AC-2     | Append batch: 3 events, contiguous versions and positions                      | Ticket 3             | Covered |
| AC-3     | Append version conflict: `no_stream` twice -> `FAILED_PRECONDITION`            | Ticket 3             | Covered |
| AC-4     | Append invalid stream ID -> `INVALID_ARGUMENT`                                 | Ticket 2, Ticket 3   | Covered |
| AC-5     | Append empty events list -> `INVALID_ARGUMENT`                                 | Ticket 3             | Covered |
| AC-6     | Append invalid event ID -> `INVALID_ARGUMENT`                                  | Ticket 2, Ticket 3   | Covered |
| AC-7     | Append oversized event (>64 KB payload) -> `INVALID_ARGUMENT`                  | Ticket 3             | Covered |
| AC-8     | ReadStream happy path: 5 events, all returned in order                         | Ticket 3             | Covered |
| AC-9     | ReadStream partial: from_version=2, max_count=2                                | Ticket 3             | Covered |
| AC-10    | ReadStream non-existent stream -> `NOT_FOUND`                                  | Ticket 2, Ticket 3   | Covered |
| AC-11    | ReadStream invalid stream ID -> `INVALID_ARGUMENT`                             | Ticket 2, Ticket 3   | Covered |
| AC-12    | ReadAll happy path: 5 events across 2 streams, global order                    | Ticket 3             | Covered |
| AC-13    | ReadAll partial: from_position=3, max_count=2                                  | Ticket 3             | Covered |
| AC-14    | ReadAll empty store: 0 events returned                                         | Ticket 3             | Covered |
| AC-15    | SubscribeAll catch-up and live: 3 pre-existing + 2 live events                 | Ticket 4             | Covered |
| AC-16    | SubscribeAll from middle: start at position 3                                  | Ticket 4             | Covered |
| AC-17    | SubscribeStream catch-up and live: stream A events only, live append received  | Ticket 4             | Covered |
| AC-18    | SubscribeStream filtering: only stream A events appear after `CaughtUp`        | Ticket 4             | Covered |
| AC-19    | SubscribeStream non-existent stream: `CaughtUp` immediately, then live event   | Ticket 4             | Covered |
| AC-20    | Subscription lag termination: stream ends when broker buffer overflows         | Ticket 4             | Covered |
| AC-21    | Multiple concurrent subscriptions both receive events                          | Ticket 4             | Covered |
| AC-22    | Proto field names and types: generated types compile and have expected fields   | Ticket 1, Ticket 5   | Covered |
| AC-23    | Build and lint: zero warnings, clippy clean, fmt, all tests green              | Ticket 6             | Covered |
