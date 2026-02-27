# PRD 006: gRPC Service

**Status:** TICKETS READY

## Summary

Define the Protocol Buffers schema and implement the tonic gRPC service for EventfoldDB's five RPCs: `Append`, `ReadStream`, `ReadAll`, `SubscribeAll`, and `SubscribeStream`. This is the network-facing API that clients interact with. The service layer is thin -- it validates and translates gRPC requests into calls to the writer task and read index, and maps domain errors to gRPC status codes.

## Motivation

The gRPC service is the public API boundary. It must faithfully expose the semantics defined in the design doc: optimistic concurrency on append, forward reads by stream and globally, and catch-up-then-live subscriptions. The proto schema defines the wire contract that all clients (command APIs, projection services) depend on.

## Scope

### In scope

- `proto/eventfold.proto`: Protocol Buffers service definition.
- `build.rs`: prost/tonic code generation.
- `service.rs`: tonic service implementation (`EventfoldService`).
- Error-to-status-code mapping.
- Request validation (UUID format, non-empty event list, etc.).
- Integration tests in `tests/`: full gRPC client-server round-trips.

### Out of scope

- TLS (future consideration).
- Server startup and config (PRD 007).

## Detailed Design

### Proto Definition

```protobuf
syntax = "proto3";
package eventfold;

service EventStore {
    rpc Append(AppendRequest) returns (AppendResponse);
    rpc ReadStream(ReadStreamRequest) returns (ReadStreamResponse);
    rpc ReadAll(ReadAllRequest) returns (ReadAllResponse);
    rpc SubscribeAll(SubscribeAllRequest) returns (stream SubscribeResponse);
    rpc SubscribeStream(SubscribeStreamRequest) returns (stream SubscribeResponse);
}

message ProposedEvent {
    string event_id = 1;     // UUID string
    string event_type = 2;
    bytes metadata = 3;
    bytes payload = 4;
}

message RecordedEvent {
    string event_id = 1;     // UUID string
    string stream_id = 2;    // UUID string
    uint64 stream_version = 3;
    uint64 global_position = 4;
    string event_type = 5;
    bytes metadata = 6;
    bytes payload = 7;
}

message ExpectedVersion {
    oneof kind {
        Empty any = 1;
        Empty no_stream = 2;
        uint64 exact = 3;
    }
}

message Empty {}

message AppendRequest {
    string stream_id = 1;    // UUID string
    ExpectedVersion expected_version = 2;
    repeated ProposedEvent events = 3;
}

message AppendResponse {
    uint64 first_stream_version = 1;
    uint64 last_stream_version = 2;
    uint64 first_global_position = 3;
    uint64 last_global_position = 4;
}

message ReadStreamRequest {
    string stream_id = 1;    // UUID string
    uint64 from_version = 2;
    uint64 max_count = 3;
}

message ReadStreamResponse {
    repeated RecordedEvent events = 1;
}

message ReadAllRequest {
    uint64 from_position = 1;
    uint64 max_count = 2;
}

message ReadAllResponse {
    repeated RecordedEvent events = 1;
}

message SubscribeAllRequest {
    uint64 from_position = 1;
}

message SubscribeStreamRequest {
    string stream_id = 1;    // UUID string
    uint64 from_version = 2;
}

message SubscribeResponse {
    oneof content {
        RecordedEvent event = 1;
        Empty caught_up = 2;
    }
}
```

### Service Implementation

```rust
pub struct EventfoldService {
    writer: WriterHandle,
    read_index: ReadIndex,
    broker: Broker,
}
```

#### `Append`

1. Validate `stream_id` is a valid UUID. If not, return `Status::invalid_argument`.
2. Validate `expected_version` is set. If not, return `Status::invalid_argument`.
3. Validate `events` is non-empty. If not, return `Status::invalid_argument`.
4. For each proposed event, validate `event_id` is a valid UUID. If not, return `Status::invalid_argument`.
5. Convert proto types to domain types.
6. Call `writer.append(...)`.
7. Map the result:
   - `Ok(recorded)` -> `AppendResponse` with first/last versions and positions.
   - `Err(WrongExpectedVersion)` -> `Status::failed_precondition`.
   - `Err(EventTooLarge)` -> `Status::invalid_argument`.
   - `Err(InvalidArgument)` -> `Status::invalid_argument`.
   - `Err(Io(_))` -> `Status::internal`.

#### `ReadStream`

1. Validate `stream_id` is a valid UUID.
2. Call `read_index.read_stream(...)`.
3. Map the result:
   - `Ok(events)` -> `ReadStreamResponse`.
   - `Err(StreamNotFound)` -> `Status::not_found`.

#### `ReadAll`

1. Call `read_index.read_all(...)`.
2. Return `ReadAllResponse`.

#### `SubscribeAll`

1. Call `subscribe_all(read_index, broker, from_position)`.
2. Map the resulting `Stream<Item = Result<SubscriptionMessage, Error>>` into a `Stream<Item = Result<SubscribeResponse, Status>>`.
3. Return as a server-streaming response.

#### `SubscribeStream`

1. Validate `stream_id` is a valid UUID.
2. Call `subscribe_stream(read_index, broker, stream_id, from_version)`.
3. Map and return as a server-streaming response.

### Error Mapping

| Domain Error            | gRPC Status Code       |
|-------------------------|------------------------|
| `WrongExpectedVersion`  | `FAILED_PRECONDITION`  |
| `StreamNotFound`        | `NOT_FOUND`            |
| `Io`                    | `INTERNAL`             |
| `CorruptRecord`         | `DATA_LOSS`            |
| `InvalidHeader`         | `DATA_LOSS`            |
| `EventTooLarge`         | `INVALID_ARGUMENT`     |
| `InvalidArgument`       | `INVALID_ARGUMENT`     |

The status message should include the domain error's display string for debuggability.

### Test Harness

Integration tests spin up a real server on an ephemeral port (port 0, let the OS assign), connect a tonic client, and exercise the RPCs. Each test uses `tempfile::tempdir()` for the data directory.

A helper function should encapsulate server setup:

```rust
async fn start_test_server() -> (EventStoreClient<Channel>, SocketAddr, TempDir) { ... }
```

## Acceptance Criteria

### AC-1: Append -- happy path

- **Test**: Append 1 event to a new stream with `no_stream`. Response contains `first_stream_version = 0`, `last_stream_version = 0`, matching global positions.

### AC-2: Append -- batch

- **Test**: Append 3 events atomically. Response contains `first_stream_version = 0`, `last_stream_version = 2`, contiguous global positions.

### AC-3: Append -- version conflict

- **Test**: Append to stream A with `no_stream`. Append again to stream A with `no_stream`. Second call returns `FAILED_PRECONDITION`.

### AC-4: Append -- invalid stream ID

- **Test**: Append with `stream_id = "not-a-uuid"`. Returns `INVALID_ARGUMENT`.

### AC-5: Append -- empty events list

- **Test**: Append with an empty `events` list. Returns `INVALID_ARGUMENT`.

### AC-6: Append -- invalid event ID

- **Test**: Append with an event whose `event_id` is not a valid UUID. Returns `INVALID_ARGUMENT`.

### AC-7: Append -- event too large

- **Test**: Append an event with a payload exceeding 64 KB. Returns `INVALID_ARGUMENT`.

### AC-8: ReadStream -- happy path

- **Test**: Append 5 events to a stream. `ReadStream` from version 0 with max_count 100. Response contains all 5 events in order.

### AC-9: ReadStream -- partial read

- **Test**: Append 5 events. `ReadStream` from version 2, max_count 2. Response contains events at versions 2 and 3.

### AC-10: ReadStream -- non-existent stream

- **Test**: `ReadStream` for a stream that does not exist. Returns `NOT_FOUND`.

### AC-11: ReadStream -- invalid stream ID

- **Test**: `ReadStream` with `stream_id = "not-a-uuid"`. Returns `INVALID_ARGUMENT`.

### AC-12: ReadAll -- happy path

- **Test**: Append 5 events across 2 streams. `ReadAll` from position 0, max_count 100. Response contains all 5 events in global position order.

### AC-13: ReadAll -- partial read

- **Test**: Append 5 events. `ReadAll` from position 3, max_count 2. Response contains events at positions 3 and 4.

### AC-14: ReadAll -- empty store

- **Test**: `ReadAll` from position 0 on an empty store. Response contains 0 events.

### AC-15: SubscribeAll -- catch-up and live

- **Test**: Append 3 events. Start `SubscribeAll` from position 0. Receive events 0, 1, 2, then `CaughtUp`. Append 2 more events. Receive events 3, 4 on the stream.

### AC-16: SubscribeAll -- from middle

- **Test**: Append 5 events. Start `SubscribeAll` from position 3. Receive events 3, 4, then `CaughtUp`.

### AC-17: SubscribeStream -- catch-up and live

- **Test**: Append events: stream A, stream B, stream A. Start `SubscribeStream` for stream A from version 0. Receive A's 2 events, then `CaughtUp`. Append to stream A again. Receive the new event on the stream. Events from stream B are never received.

### AC-18: SubscribeStream -- filtering

- **Test**: Start `SubscribeStream` for stream A. Append to streams B, C, A, B. Only stream A's event appears after `CaughtUp`.

### AC-19: SubscribeStream -- non-existent stream

- **Test**: Start `SubscribeStream` for a stream that does not exist. Receive `CaughtUp` immediately. Append to that stream. The event appears on the subscription.

### AC-20: Subscription lag termination

- **Test**: Start `SubscribeAll` with a small broker capacity. Append many events without consuming the subscription. The stream eventually terminates (connection drops or error).

### AC-21: Multiple concurrent subscriptions

- **Test**: Start 2 `SubscribeAll` subscriptions. Append events. Both subscriptions receive the events.

### AC-22: Proto field names and types

- **Test**: Verify the generated proto types exist and have the expected fields. (This is implicitly tested by all other tests -- if the proto is wrong, nothing compiles.)

### AC-23: Build and lint

- `cargo build` completes with zero warnings.
- `cargo clippy --all-targets --all-features --locked -- -D warnings` passes.
- `cargo fmt --check` passes.
- `cargo test` passes with all tests green.

## Dependencies

- **Depends on**: PRD 001 (types), PRD 003 (store/read index), PRD 004 (writer), PRD 005 (broker/subscriptions).
- **Depended on by**: PRD 007.

## Cargo.toml Additions

```toml
[dependencies]
tonic = "0.13"
prost = "0.13"

[build-dependencies]
tonic-build = "0.13"
```
