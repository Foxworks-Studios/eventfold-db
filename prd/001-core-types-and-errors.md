# PRD 001: Core Types and Error Handling

**Status:** QA READY

## Summary

Define the foundational data types and error enum that every other module in EventfoldDB depends on. These types represent the domain vocabulary of the event store: events (proposed and recorded), expected version semantics, and the error cases that can arise during operation.

This PRD has no internal dependencies -- it is the first module to build.

## Motivation

Every subsequent PRD (on-disk format, storage engine, writer task, gRPC service) imports these types. Getting the shapes right here avoids churn downstream. The types must faithfully represent the invariants described in the design doc: UUIDs for identifiers, contiguous zero-based positions, opaque byte payloads, and a well-defined set of error conditions.

## Scope

### In scope

- `types.rs`: `RecordedEvent`, `ProposedEvent`, `ExpectedVersion` enum, and any supporting newtypes or constants.
- `error.rs`: `Error` enum with `thiserror` derive.
- `lib.rs`: crate root that re-exports public types and errors.
- `Cargo.toml`: add `uuid`, `thiserror`, `bytes` dependencies.

### Out of scope

- Serialization/deserialization (PRD 002).
- Storage engine logic (PRD 003).
- Any async runtime or networking dependencies.

## Detailed Design

### `types.rs`

#### `ProposedEvent`

Represents an event the client wants to append. Fields:

| Field        | Type           | Description                                              |
|--------------|----------------|----------------------------------------------------------|
| `event_id`   | `Uuid`         | Client-assigned unique ID for this event                 |
| `event_type` | `String`       | Event type tag (UTF-8, max 256 bytes)                    |
| `metadata`   | `Bytes`        | Opaque infrastructure context (correlation ID, etc.)     |
| `payload`    | `Bytes`        | Opaque domain event body                                 |

#### `RecordedEvent`

Represents a persisted event with server-assigned positions. Fields:

| Field            | Type           | Description                                          |
|------------------|----------------|------------------------------------------------------|
| `event_id`       | `Uuid`         | Client-assigned unique ID                            |
| `stream_id`      | `Uuid`         | Stream this event belongs to                         |
| `stream_version` | `u64`          | Zero-based version within the stream                 |
| `global_position`| `u64`          | Zero-based position in the global log                |
| `event_type`     | `String`       | Event type tag                                       |
| `metadata`       | `Bytes`        | Opaque metadata bytes                                |
| `payload`        | `Bytes`        | Opaque payload bytes                                 |

Derive: `Debug`, `Clone`, `PartialEq`.

#### `ExpectedVersion`

Enum controlling optimistic concurrency on append:

```rust
pub enum ExpectedVersion {
    /// No concurrency check -- append succeeds regardless of stream state.
    Any,
    /// Stream must not exist (first write to a new stream).
    NoStream,
    /// Stream must be at exactly this version (zero-based).
    Exact(u64),
}
```

Derive: `Debug`, `Clone`, `Copy`, `PartialEq, Eq`.

#### Constants

```rust
/// Maximum size of a single event record in bytes (payload + metadata + fixed fields).
pub const MAX_EVENT_SIZE: usize = 64 * 1024; // 64 KB

/// Maximum length of an event type tag in bytes.
pub const MAX_EVENT_TYPE_LEN: usize = 256;
```

### `error.rs`

```rust
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("wrong expected version: expected {expected}, actual {actual}")]
    WrongExpectedVersion { expected: String, actual: String },

    #[error("stream not found: {stream_id}")]
    StreamNotFound { stream_id: Uuid },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("corrupt record at position {position}: {detail}")]
    CorruptRecord { position: u64, detail: String },

    #[error("invalid file header: {0}")]
    InvalidHeader(String),

    #[error("event too large: {size} bytes exceeds {max} byte limit")]
    EventTooLarge { size: usize, max: usize },

    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}
```

### `lib.rs`

Re-export all public types:

```rust
pub mod error;
pub mod types;

pub use error::Error;
pub use types::*;
```

## Acceptance Criteria

Each criterion maps to one or more unit tests. Tests live in `#[cfg(test)]` modules within the respective files.

### AC-1: ProposedEvent construction

- **Test**: Create a `ProposedEvent` with valid fields. Assert all fields are accessible and have the expected values.
- **Test**: Clone a `ProposedEvent` and assert equality with the original.

### AC-2: RecordedEvent construction

- **Test**: Create a `RecordedEvent` with all fields set. Assert each field returns the expected value.
- **Test**: Clone a `RecordedEvent` and assert equality with the original (`PartialEq`).
- **Test**: Two `RecordedEvent` values with different `global_position` are not equal.

### AC-3: ExpectedVersion variants

- **Test**: `ExpectedVersion::Any` is constructible and implements `Copy`.
- **Test**: `ExpectedVersion::NoStream` is constructible.
- **Test**: `ExpectedVersion::Exact(5)` stores the version and can be pattern-matched.
- **Test**: `ExpectedVersion` implements `Debug` (format string is non-empty).
- **Test**: `ExpectedVersion::Exact(3) == ExpectedVersion::Exact(3)` and `ExpectedVersion::Exact(3) != ExpectedVersion::Exact(4)`.

### AC-4: Constants

- **Test**: `MAX_EVENT_SIZE` equals 65536.
- **Test**: `MAX_EVENT_TYPE_LEN` equals 256.

### AC-5: Error variants

- **Test**: `Error::WrongExpectedVersion` display includes "wrong expected version" and the expected/actual values.
- **Test**: `Error::StreamNotFound` display includes the stream ID.
- **Test**: `Error::Io` can be created from a `std::io::Error` via `From`.
- **Test**: `Error::CorruptRecord` display includes the position and detail.
- **Test**: `Error::InvalidHeader` display includes the reason string.
- **Test**: `Error::EventTooLarge` display includes the size and max.
- **Test**: `Error::InvalidArgument` display includes the argument description.
- **Test**: All error variants implement `Debug`.

### AC-6: Crate re-exports

- **Test**: `eventfold_db::Error` is accessible (re-exported from `lib.rs`).
- **Test**: `eventfold_db::RecordedEvent`, `eventfold_db::ProposedEvent`, `eventfold_db::ExpectedVersion` are accessible.
- **Test**: `eventfold_db::MAX_EVENT_SIZE` and `eventfold_db::MAX_EVENT_TYPE_LEN` are accessible.

### AC-7: Cargo build and lint

- `cargo build` completes with zero warnings.
- `cargo clippy --all-targets --all-features --locked -- -D warnings` passes.
- `cargo fmt --check` passes.
- `cargo test` passes with all tests green.

## Dependencies

- **Depends on**: Nothing (first PRD).
- **Depended on by**: PRD 002, 003, 004, 005, 006, 007.

## Cargo.toml Additions

```toml
[dependencies]
bytes = "1"
thiserror = "2"
uuid = { version = "1", features = ["v4", "v7"] }
```
