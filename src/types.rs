//! Core domain types for EventfoldDB.
//!
//! This module defines the foundational data types that every other module depends on:
//! proposed events (client-submitted), recorded events (server-persisted with positions),
//! expected version semantics for optimistic concurrency, and size limit constants.

use std::sync::Arc;

use bytes::Bytes;
use uuid::Uuid;

/// Maximum size of a single event record in bytes (payload + metadata + fixed fields).
///
/// Events exceeding this limit are rejected on append. Domain events should be small,
/// structured data (typically JSON). Large artifacts belong in external storage; events
/// carry references to them.
pub const MAX_EVENT_SIZE: usize = 64 * 1024; // 64 KB

/// Maximum length of an event type tag in bytes.
///
/// Event type tags are UTF-8 strings identifying the kind of domain event
/// (e.g., `"OrderPlaced"`, `"PaymentReceived"`).
pub const MAX_EVENT_TYPE_LEN: usize = 256;

/// An event the client wants to append to a stream.
///
/// The client assigns the `event_id` (a UUID serving as an idempotency key) and provides
/// the event type tag, metadata, and payload as opaque byte buffers. The server does not
/// interpret payload or metadata contents.
///
/// # Fields
///
/// * `event_id` - Client-assigned unique ID for this event (UUID v4 or v7).
/// * `event_type` - Event type tag (UTF-8, max 256 bytes).
/// * `metadata` - Opaque infrastructure context (correlation ID, causation ID, etc.).
/// * `payload` - Opaque domain event body (the facts of what happened).
#[derive(Debug, Clone, PartialEq)]
pub struct ProposedEvent {
    /// Client-assigned unique ID for this event.
    pub event_id: Uuid,
    /// Event type tag (UTF-8, max 256 bytes).
    pub event_type: String,
    /// Opaque infrastructure context (correlation ID, causation ID, etc.).
    pub metadata: Bytes,
    /// Opaque domain event body.
    pub payload: Bytes,
}

/// A persisted event with server-assigned positions.
///
/// After a successful append, the server assigns a `global_position` (contiguous, zero-based
/// index in the global log), a `stream_version` (contiguous, zero-based index within the
/// stream), and a `recorded_at` timestamp. These fields are immutable once assigned.
///
/// # Fields
///
/// * `event_id` - Client-assigned unique ID.
/// * `stream_id` - UUID of the stream this event belongs to.
/// * `stream_version` - Zero-based version within the stream.
/// * `global_position` - Zero-based position in the global log.
/// * `recorded_at` - Unix epoch milliseconds, server-assigned at append time.
/// * `event_type` - Event type tag.
/// * `metadata` - Opaque metadata bytes.
/// * `payload` - Opaque payload bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedEvent {
    /// Client-assigned unique ID.
    pub event_id: Uuid,
    /// Stream this event belongs to.
    pub stream_id: Uuid,
    /// Zero-based version within the stream.
    pub stream_version: u64,
    /// Zero-based position in the global log.
    pub global_position: u64,
    /// Unix epoch milliseconds, server-assigned at append time.
    pub recorded_at: u64,
    /// Event type tag.
    pub event_type: String,
    /// Opaque metadata bytes.
    pub metadata: Bytes,
    /// Opaque payload bytes.
    pub payload: Bytes,
}

/// Controls optimistic concurrency on append.
///
/// The caller specifies what state the target stream must be in for the append to succeed.
/// If the check fails, the server rejects the append with `WrongExpectedVersion`.
///
/// # Variants
///
/// * `Any` - No concurrency check; append succeeds regardless of stream state.
/// * `NoStream` - Stream must not exist (first write to a new stream).
/// * `Exact(u64)` - Stream must be at exactly this version (zero-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedVersion {
    /// No concurrency check -- append succeeds regardless of stream state.
    Any,
    /// Stream must not exist (first write to a new stream).
    NoStream,
    /// Stream must be at exactly this version (zero-based).
    Exact(u64),
}

/// A message yielded by subscription streams (`subscribe_all`, `subscribe_stream`).
///
/// During the catch-up phase, the stream yields `Event` variants wrapping each historical
/// event in an `Arc` to avoid deep-cloning event data across subscribers. Once catch-up
/// completes, the stream yields a single `CaughtUp` marker, after which it transitions to
/// forwarding live events from the broadcast channel.
///
/// # Variants
///
/// * `Event(Arc<RecordedEvent>)` - A recorded event, shared via `Arc` across subscribers.
/// * `CaughtUp` - Marks the end of the catch-up phase; all historical events have been sent.
#[derive(Debug, Clone)]
pub enum SubscriptionMessage {
    /// A recorded event, shared via `Arc` to avoid deep-cloning across subscribers.
    Event(Arc<RecordedEvent>),
    /// Marks the end of the catch-up phase.
    CaughtUp,
}

/// Metadata about a single stream returned by `ReadIndex::list_streams`.
///
/// Contains the stream's UUID, the total number of events appended to it,
/// and the zero-based version of the most recently written event. This type
/// carries no event data (payload, metadata, event type) -- only stream-level
/// summary information.
///
/// # Fields
///
/// * `stream_id` - UUID identifying the stream.
/// * `event_count` - Total number of events in the stream.
/// * `latest_version` - Zero-based version of the last event written to the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamInfo {
    /// UUID identifying the stream.
    pub stream_id: Uuid,
    /// Total number of events in the stream.
    pub event_count: u64,
    /// Zero-based version of the last event written to the stream.
    pub latest_version: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    // AC-1: ProposedEvent construction and clone equality.

    #[test]
    fn proposed_event_fields_round_trip() {
        let id = Uuid::new_v4();
        let event = ProposedEvent {
            event_id: id,
            event_type: "OrderPlaced".to_string(),
            metadata: Bytes::from_static(b"meta"),
            payload: Bytes::from_static(b"payload"),
        };

        assert_eq!(event.event_id, id);
        assert_eq!(event.event_type, "OrderPlaced");
        assert_eq!(event.metadata, Bytes::from_static(b"meta"));
        assert_eq!(event.payload, Bytes::from_static(b"payload"));
    }

    #[test]
    fn proposed_event_clone_is_equal() {
        let event = ProposedEvent {
            event_id: Uuid::new_v4(),
            event_type: "ItemAdded".to_string(),
            metadata: Bytes::from_static(b"{}"),
            payload: Bytes::from_static(b"{\"qty\":1}"),
        };

        let cloned = event.clone();
        assert_eq!(event, cloned);
    }

    // AC-2: RecordedEvent construction, clone equality, and inequality on differing fields.

    #[test]
    fn recorded_event_fields_round_trip() {
        let event_id = Uuid::new_v4();
        let stream_id = Uuid::new_v4();
        let event = RecordedEvent {
            event_id,
            stream_id,
            stream_version: 0,
            global_position: 42,
            recorded_at: 0,
            event_type: "PaymentReceived".to_string(),
            metadata: Bytes::from_static(b"corr-123"),
            payload: Bytes::from_static(b"{\"amount\":100}"),
        };

        assert_eq!(event.event_id, event_id);
        assert_eq!(event.stream_id, stream_id);
        assert_eq!(event.stream_version, 0);
        assert_eq!(event.global_position, 42);
        assert_eq!(event.event_type, "PaymentReceived");
        assert_eq!(event.metadata, Bytes::from_static(b"corr-123"));
        assert_eq!(event.payload, Bytes::from_static(b"{\"amount\":100}"));
    }

    #[test]
    fn recorded_event_clone_is_equal() {
        let event = RecordedEvent {
            event_id: Uuid::new_v4(),
            stream_id: Uuid::new_v4(),
            stream_version: 3,
            global_position: 10,
            recorded_at: 0,
            event_type: "Shipped".to_string(),
            metadata: Bytes::new(),
            payload: Bytes::from_static(b"{}"),
        };

        let cloned = event.clone();
        assert_eq!(event, cloned);
    }

    #[test]
    fn recorded_events_with_different_global_position_are_not_equal() {
        let event_id = Uuid::new_v4();
        let stream_id = Uuid::new_v4();
        let event_a = RecordedEvent {
            event_id,
            stream_id,
            stream_version: 0,
            global_position: 0,
            recorded_at: 0,
            event_type: "Created".to_string(),
            metadata: Bytes::new(),
            payload: Bytes::new(),
        };
        let event_b = RecordedEvent {
            global_position: 1,
            ..event_a.clone()
        };

        assert_ne!(event_a, event_b);
    }

    // AC-3: ExpectedVersion variants, Copy, Debug, and equality.

    #[test]
    fn expected_version_any_is_copy() {
        let v = ExpectedVersion::Any;
        // Use `v` twice without clone -- only possible if `Copy` is implemented.
        let a = v;
        let b = v;
        assert_eq!(a, b);
    }

    #[test]
    fn expected_version_no_stream_constructs() {
        let v = ExpectedVersion::NoStream;
        assert_eq!(v, ExpectedVersion::NoStream);
    }

    #[test]
    fn expected_version_exact_pattern_matches() {
        let v = ExpectedVersion::Exact(5);
        match v {
            ExpectedVersion::Exact(n) => assert_eq!(n, 5),
            _ => panic!("expected Exact(5)"),
        }
    }

    #[test]
    fn expected_version_debug_is_non_empty() {
        let debug_str = format!("{:?}", ExpectedVersion::Any);
        assert!(!debug_str.is_empty());
    }

    #[test]
    fn expected_version_exact_equality_and_inequality() {
        assert_eq!(ExpectedVersion::Exact(3), ExpectedVersion::Exact(3));
        assert_ne!(ExpectedVersion::Exact(3), ExpectedVersion::Exact(4));
    }

    // AC-4: Constants.

    #[test]
    fn max_event_size_is_65536() {
        assert_eq!(MAX_EVENT_SIZE, 65536);
    }

    #[test]
    fn max_event_type_len_is_256() {
        assert_eq!(MAX_EVENT_TYPE_LEN, 256);
    }

    // SubscriptionMessage tests (PRD 005, Ticket 1)

    #[test]
    fn subscription_message_event_debug_is_non_empty() {
        let event = RecordedEvent {
            event_id: Uuid::new_v4(),
            stream_id: Uuid::new_v4(),
            stream_version: 0,
            global_position: 0,
            recorded_at: 0,
            event_type: "TestEvent".to_string(),
            metadata: Bytes::new(),
            payload: Bytes::from_static(b"{}"),
        };
        let msg = SubscriptionMessage::Event(std::sync::Arc::new(event));
        let debug_str = format!("{:?}", msg);
        assert!(!debug_str.is_empty());
    }

    #[test]
    fn subscription_message_caught_up_debug_is_non_empty() {
        let msg = SubscriptionMessage::CaughtUp;
        let debug_str = format!("{:?}", msg);
        assert!(!debug_str.is_empty());
    }

    #[test]
    fn subscription_message_clone_event_shares_arc() {
        let event = RecordedEvent {
            event_id: Uuid::new_v4(),
            stream_id: Uuid::new_v4(),
            stream_version: 0,
            global_position: 0,
            recorded_at: 0,
            event_type: "TestEvent".to_string(),
            metadata: Bytes::new(),
            payload: Bytes::from_static(b"{}"),
        };
        let arc = std::sync::Arc::new(event);
        let msg = SubscriptionMessage::Event(arc.clone());
        let cloned = msg.clone();

        // Cloning the message must not deep-clone the RecordedEvent allocation.
        match (&msg, &cloned) {
            (SubscriptionMessage::Event(a), SubscriptionMessage::Event(b)) => {
                assert!(std::sync::Arc::ptr_eq(a, b));
            }
            _ => panic!("expected Event variant"),
        }
    }

    // PRD 014, Ticket 1: StreamInfo tests.

    #[test]
    fn stream_info_clone_equals_original() {
        let info = StreamInfo {
            stream_id: Uuid::new_v4(),
            event_count: 3,
            latest_version: 2,
        };
        let cloned = info.clone();
        assert_eq!(info, cloned);
    }

    #[test]
    fn stream_info_differing_event_count_not_equal() {
        let stream_id = Uuid::new_v4();
        let info_a = StreamInfo {
            stream_id,
            event_count: 3,
            latest_version: 2,
        };
        let info_b = StreamInfo {
            stream_id,
            event_count: 5,
            latest_version: 2,
        };
        assert_ne!(info_a, info_b);
    }

    #[test]
    fn async_stream_dependency_compiles() {
        // Confirms that the async-stream crate is available.
        // The stream! macro requires an async context, so we just verify it resolves.
        use async_stream::stream;
        let _s = stream! {
            yield 1u32;
        };
    }

    // PRD 017, Ticket 1: recorded_at field tests.

    #[test]
    fn recorded_event_recorded_at_round_trip() {
        let event = RecordedEvent {
            event_id: Uuid::new_v4(),
            stream_id: Uuid::new_v4(),
            stream_version: 0,
            global_position: 0,
            recorded_at: 1_700_000_000_123,
            event_type: "TimestampTest".to_string(),
            metadata: Bytes::new(),
            payload: Bytes::from_static(b"{}"),
        };
        assert_eq!(event.recorded_at, 1_700_000_000_123);
    }

    #[test]
    fn recorded_event_clone_preserves_recorded_at() {
        let event = RecordedEvent {
            event_id: Uuid::new_v4(),
            stream_id: Uuid::new_v4(),
            stream_version: 0,
            global_position: 0,
            recorded_at: 42,
            event_type: "CloneTest".to_string(),
            metadata: Bytes::new(),
            payload: Bytes::from_static(b"{}"),
        };
        let cloned = event.clone();
        assert_eq!(cloned.recorded_at, 42);
    }

    #[test]
    fn recorded_events_with_different_recorded_at_are_not_equal() {
        let event_id = Uuid::new_v4();
        let stream_id = Uuid::new_v4();
        let event_a = RecordedEvent {
            event_id,
            stream_id,
            stream_version: 0,
            global_position: 0,
            recorded_at: 100,
            event_type: "DiffTest".to_string(),
            metadata: Bytes::new(),
            payload: Bytes::new(),
        };
        let event_b = RecordedEvent {
            recorded_at: 200,
            ..event_a.clone()
        };
        assert_ne!(event_a, event_b);
    }
}
