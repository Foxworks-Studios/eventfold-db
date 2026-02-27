//! gRPC service layer for EventfoldDB.
//!
//! This module contains the `EventfoldService` struct that holds the writer handle,
//! read index, and broker, along with conversion helpers that translate between
//! protobuf types and domain types, and map domain errors to gRPC status codes.

// `tonic::Status` is 176 bytes, which triggers clippy::result_large_err. This is
// inherent to tonic's API -- every gRPC handler returns `Result<T, tonic::Status>`.
// Suppressing at module level since all conversion helpers share this pattern.
#![allow(clippy::result_large_err)]

use bytes::Bytes;
use metrics::{counter, gauge};
use uuid::Uuid;

use futures_core::Stream;

use crate::broker::Broker;
use crate::error::Error;
use crate::proto;
use crate::reader::ReadIndex;
use crate::types::{ExpectedVersion, ProposedEvent, RecordedEvent, SubscriptionMessage};
use crate::writer::WriterHandle;

/// gRPC service implementation for EventfoldDB.
///
/// Holds the three dependencies needed by all RPC handlers:
///
/// - `writer` -- handle to submit append requests to the single writer task.
/// - `read_index` -- shared, read-only view of the in-memory event log.
/// - `broker` -- broadcast channel for live subscription events.
pub struct EventfoldService {
    /// Handle for submitting append requests to the writer task.
    pub writer: WriterHandle,
    /// Shared read-only view of the in-memory event log.
    pub read_index: ReadIndex,
    /// Broadcast broker for live event subscriptions.
    pub broker: Broker,
}

impl EventfoldService {
    /// Create a new `EventfoldService` with the given dependencies.
    ///
    /// # Arguments
    ///
    /// * `writer` - Handle for submitting append requests to the writer task.
    /// * `read_index` - Shared read-only view of the in-memory event log.
    /// * `broker` - Broadcast broker for live event subscriptions.
    pub fn new(writer: WriterHandle, read_index: ReadIndex, broker: Broker) -> Self {
        Self {
            writer,
            read_index,
            broker,
        }
    }
}

/// Type alias for the server-streaming response used by subscription RPCs.
///
/// Subscription handlers (Ticket 4) will return a pinned, boxed, `Send` stream
/// of `Result<SubscribeResponse, Status>`. The stubs below use this type for the
/// required associated types on the `EventStore` trait.
type SubscriptionStream = std::pin::Pin<
    Box<dyn futures_core::Stream<Item = Result<proto::SubscribeResponse, tonic::Status>> + Send>,
>;

#[tonic::async_trait]
impl proto::event_store_server::EventStore for EventfoldService {
    /// Append events to a stream with optimistic concurrency.
    ///
    /// Validates `stream_id`, `expected_version`, non-empty `events`, and each
    /// `event_id`; delegates to the writer task; returns positions on success.
    async fn append(
        &self,
        request: tonic::Request<proto::AppendRequest>,
    ) -> Result<tonic::Response<proto::AppendResponse>, tonic::Status> {
        let req = request.into_inner();

        // Validate stream_id.
        let stream_id = parse_uuid(&req.stream_id, "stream_id")?;

        // Validate expected_version.
        let expected_version = proto_to_expected_version(req.expected_version)?;

        // Validate events list is non-empty.
        if req.events.is_empty() {
            return Err(tonic::Status::invalid_argument("events must not be empty"));
        }

        // Convert proto events to domain events (validates each event_id).
        let events: Vec<ProposedEvent> = req
            .events
            .into_iter()
            .map(proto_to_proposed_event)
            .collect::<Result<Vec<_>, _>>()?;

        // Delegate to the writer task.
        let recorded = self
            .writer
            .append(stream_id, expected_version, events)
            .await
            .map_err(error_to_status)?;

        // Build the response from the first and last recorded events.
        let first = &recorded[0];
        let last = &recorded[recorded.len() - 1];
        Ok(tonic::Response::new(proto::AppendResponse {
            first_stream_version: first.stream_version,
            last_stream_version: last.stream_version,
            first_global_position: first.global_position,
            last_global_position: last.global_position,
        }))
    }

    /// Read events from a specific stream.
    ///
    /// Validates `stream_id`; delegates to the read index; maps
    /// `StreamNotFound` to `NOT_FOUND`.
    async fn read_stream(
        &self,
        request: tonic::Request<proto::ReadStreamRequest>,
    ) -> Result<tonic::Response<proto::ReadStreamResponse>, tonic::Status> {
        counter!("eventfold_reads_total", "rpc" => "read_stream").increment(1);
        let req = request.into_inner();

        let stream_id = parse_uuid(&req.stream_id, "stream_id")?;

        let events = self
            .read_index
            .read_stream(stream_id, req.from_version, req.max_count)
            .map_err(error_to_status)?;

        let proto_events = events.iter().map(recorded_to_proto).collect();
        Ok(tonic::Response::new(proto::ReadStreamResponse {
            events: proto_events,
        }))
    }

    /// Read events from the global log.
    ///
    /// Delegates to the read index (infallible); returns all matching events.
    async fn read_all(
        &self,
        request: tonic::Request<proto::ReadAllRequest>,
    ) -> Result<tonic::Response<proto::ReadAllResponse>, tonic::Status> {
        counter!("eventfold_reads_total", "rpc" => "read_all").increment(1);
        let req = request.into_inner();

        let events = self.read_index.read_all(req.from_position, req.max_count);

        let proto_events = events.iter().map(recorded_to_proto).collect();
        Ok(tonic::Response::new(proto::ReadAllResponse {
            events: proto_events,
        }))
    }

    type SubscribeAllStream = SubscriptionStream;

    /// Subscribe to all events globally (catch-up + live).
    ///
    /// Calls `crate::subscribe_all` with the read index and broker, then maps
    /// each `SubscriptionMessage` to a `SubscribeResponse` for the gRPC stream.
    async fn subscribe_all(
        &self,
        request: tonic::Request<proto::SubscribeAllRequest>,
    ) -> Result<tonic::Response<Self::SubscribeAllStream>, tonic::Status> {
        let req = request.into_inner();

        // Clone owned handles so the returned stream is `'static` (not borrowing
        // `&self`). Both `ReadIndex` and `Broker` are cheap `Arc`-based clones.
        let read_index = self.read_index.clone();
        let broker = self.broker.clone();

        let mapped = async_stream::stream! {
            // Guard increments the gauge now and decrements it on drop (including
            // when the client disconnects mid-stream).
            let _guard = SubscriptionGauge::new();

            let inner = crate::subscribe_all(read_index, &broker, req.from_position).await;
            tokio::pin!(inner);

            loop {
                let item = std::future::poll_fn(|cx| {
                    std::pin::Pin::as_mut(&mut inner).poll_next(cx)
                }).await;

                match item {
                    Some(Ok(SubscriptionMessage::Event(arc_event))) => {
                        yield Ok(proto::SubscribeResponse {
                            content: Some(proto::subscribe_response::Content::Event(
                                recorded_to_proto(&arc_event),
                            )),
                        });
                    }
                    Some(Ok(SubscriptionMessage::CaughtUp)) => {
                        yield Ok(proto::SubscribeResponse {
                            content: Some(proto::subscribe_response::Content::CaughtUp(
                                proto::Empty {},
                            )),
                        });
                    }
                    Some(Err(e)) => {
                        yield Err(error_to_status(e));
                        return;
                    }
                    None => return,
                }
            }
        };

        Ok(tonic::Response::new(Box::pin(mapped)))
    }

    type SubscribeStreamStream = SubscriptionStream;

    /// Subscribe to events in a single stream (catch-up + live).
    ///
    /// Validates `stream_id`, then calls `crate::subscribe_stream` with the read
    /// index, broker, stream ID, and starting version. Maps each
    /// `SubscriptionMessage` to a `SubscribeResponse` for the gRPC stream.
    async fn subscribe_stream(
        &self,
        request: tonic::Request<proto::SubscribeStreamRequest>,
    ) -> Result<tonic::Response<Self::SubscribeStreamStream>, tonic::Status> {
        let req = request.into_inner();

        let stream_id = parse_uuid(&req.stream_id, "stream_id")?;

        // Clone owned handles so the returned stream is `'static`.
        let read_index = self.read_index.clone();
        let broker = self.broker.clone();

        let mapped = async_stream::stream! {
            // Guard increments the gauge now and decrements it on drop (including
            // when the client disconnects mid-stream).
            let _guard = SubscriptionGauge::new();

            let inner = crate::subscribe_stream(
                read_index, &broker, stream_id, req.from_version,
            ).await;
            tokio::pin!(inner);

            loop {
                let item = std::future::poll_fn(|cx| {
                    std::pin::Pin::as_mut(&mut inner).poll_next(cx)
                }).await;

                match item {
                    Some(Ok(SubscriptionMessage::Event(arc_event))) => {
                        yield Ok(proto::SubscribeResponse {
                            content: Some(proto::subscribe_response::Content::Event(
                                recorded_to_proto(&arc_event),
                            )),
                        });
                    }
                    Some(Ok(SubscriptionMessage::CaughtUp)) => {
                        yield Ok(proto::SubscribeResponse {
                            content: Some(proto::subscribe_response::Content::CaughtUp(
                                proto::Empty {},
                            )),
                        });
                    }
                    Some(Err(e)) => {
                        yield Err(error_to_status(e));
                        return;
                    }
                    None => return,
                }
            }
        };

        Ok(tonic::Response::new(Box::pin(mapped)))
    }
}

/// RAII guard that increments the `eventfold_subscriptions_active` gauge on
/// creation and decrements it on drop.
///
/// This guarantees the gauge is decremented even when a client disconnects
/// mid-stream, because the drop runs when the stream's async generator is
/// dropped by the tonic runtime.
struct SubscriptionGauge;

impl SubscriptionGauge {
    /// Create a new guard, incrementing the active subscriptions gauge by 1.
    fn new() -> Self {
        gauge!("eventfold_subscriptions_active").increment(1.0);
        Self
    }
}

impl Drop for SubscriptionGauge {
    fn drop(&mut self) {
        gauge!("eventfold_subscriptions_active").decrement(1.0);
    }
}

/// Map a domain [`Error`] to a [`tonic::Status`] with the appropriate gRPC status code.
///
/// The status message includes the error's `Display` string for debuggability.
///
/// # Mapping
///
/// | Domain Error           | gRPC Code            |
/// |------------------------|----------------------|
/// | `WrongExpectedVersion` | `FAILED_PRECONDITION`|
/// | `StreamNotFound`       | `NOT_FOUND`          |
/// | `Io`                   | `INTERNAL`           |
/// | `CorruptRecord`        | `DATA_LOSS`          |
/// | `InvalidHeader`        | `DATA_LOSS`          |
/// | `EventTooLarge`        | `INVALID_ARGUMENT`   |
/// | `InvalidArgument`      | `INVALID_ARGUMENT`   |
pub fn error_to_status(err: Error) -> tonic::Status {
    let message = err.to_string();
    match err {
        Error::WrongExpectedVersion { .. } => tonic::Status::failed_precondition(message),
        Error::StreamNotFound { .. } => tonic::Status::not_found(message),
        Error::Io(_) => tonic::Status::internal(message),
        Error::CorruptRecord { .. } => tonic::Status::data_loss(message),
        Error::InvalidHeader(_) => tonic::Status::data_loss(message),
        Error::EventTooLarge { .. } => tonic::Status::invalid_argument(message),
        Error::InvalidArgument(_) => tonic::Status::invalid_argument(message),
    }
}

/// Parse a UUID string, returning `tonic::Status::invalid_argument` on failure.
///
/// # Arguments
///
/// * `s` - The string to parse as a UUID.
/// * `field_name` - Name of the protobuf field, included in the error message
///   for debuggability.
///
/// # Returns
///
/// The parsed `Uuid` on success.
///
/// # Errors
///
/// Returns `tonic::Status` with `INVALID_ARGUMENT` if `s` is not a valid UUID.
pub fn parse_uuid(s: &str, field_name: &str) -> Result<Uuid, tonic::Status> {
    s.parse::<Uuid>()
        .map_err(|e| tonic::Status::invalid_argument(format!("invalid {field_name}: {e}")))
}

/// Convert a protobuf `ExpectedVersion` to the domain [`ExpectedVersion`] type.
///
/// Returns `INVALID_ARGUMENT` if the outer `Option` is `None` (field not set)
/// or if the inner `kind` oneof is `None`.
///
/// # Arguments
///
/// * `ev` - The optional protobuf `ExpectedVersion` from the request.
///
/// # Returns
///
/// The domain `ExpectedVersion` on success.
///
/// # Errors
///
/// Returns `tonic::Status` with `INVALID_ARGUMENT` if the expected version
/// is missing or has no `kind` set.
pub fn proto_to_expected_version(
    ev: Option<proto::ExpectedVersion>,
) -> Result<ExpectedVersion, tonic::Status> {
    let ev = ev.ok_or_else(|| tonic::Status::invalid_argument("expected_version is required"))?;
    match ev.kind {
        Some(proto::expected_version::Kind::Any(_)) => Ok(ExpectedVersion::Any),
        Some(proto::expected_version::Kind::NoStream(_)) => Ok(ExpectedVersion::NoStream),
        Some(proto::expected_version::Kind::Exact(v)) => Ok(ExpectedVersion::Exact(v)),
        None => Err(tonic::Status::invalid_argument(
            "expected_version.kind is required",
        )),
    }
}

/// Convert a domain [`RecordedEvent`] to the protobuf `RecordedEvent` type.
///
/// UUIDs are serialized as hyphenated lowercase strings. `Bytes` fields are
/// converted to `Vec<u8>`.
///
/// # Arguments
///
/// * `e` - Reference to the domain `RecordedEvent` to convert.
///
/// # Returns
///
/// The corresponding protobuf `RecordedEvent`.
pub fn recorded_to_proto(e: &RecordedEvent) -> proto::RecordedEvent {
    proto::RecordedEvent {
        event_id: e.event_id.to_string(),
        stream_id: e.stream_id.to_string(),
        stream_version: e.stream_version,
        global_position: e.global_position,
        event_type: e.event_type.clone(),
        metadata: e.metadata.to_vec(),
        payload: e.payload.to_vec(),
        recorded_at: e.recorded_at,
    }
}

/// Convert a protobuf `ProposedEvent` to the domain [`ProposedEvent`] type.
///
/// Validates that `event_id` is a valid UUID string. The `event_type`, `metadata`,
/// and `payload` fields are converted directly (metadata and payload from `Vec<u8>`
/// to `Bytes`).
///
/// # Arguments
///
/// * `p` - The protobuf `ProposedEvent` to convert.
///
/// # Returns
///
/// The domain `ProposedEvent` on success.
///
/// # Errors
///
/// Returns `tonic::Status` with `INVALID_ARGUMENT` if `event_id` is not a valid UUID.
pub fn proto_to_proposed_event(p: proto::ProposedEvent) -> Result<ProposedEvent, tonic::Status> {
    let event_id = parse_uuid(&p.event_id, "event_id")?;
    Ok(ProposedEvent {
        event_id,
        event_type: p.event_type,
        metadata: Bytes::from(p.metadata),
        payload: Bytes::from(p.payload),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    // -- error_to_status tests --

    #[test]
    fn error_to_status_wrong_expected_version() {
        let err = Error::WrongExpectedVersion {
            expected: "0".into(),
            actual: "1".into(),
        };
        let status = error_to_status(err);
        assert_eq!(status.code(), tonic::Code::FailedPrecondition);
        assert!(status.message().contains("wrong expected version"));
    }

    #[test]
    fn error_to_status_stream_not_found() {
        let stream_id = Uuid::new_v4();
        let err = Error::StreamNotFound { stream_id };
        let status = error_to_status(err);
        assert_eq!(status.code(), tonic::Code::NotFound);
        assert!(status.message().contains(&stream_id.to_string()));
    }

    #[test]
    fn error_to_status_io() {
        let err = Error::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "file missing",
        ));
        let status = error_to_status(err);
        assert_eq!(status.code(), tonic::Code::Internal);
        assert!(status.message().contains("I/O error"));
    }

    #[test]
    fn error_to_status_corrupt_record() {
        let err = Error::CorruptRecord {
            position: 42,
            detail: "bad crc".into(),
        };
        let status = error_to_status(err);
        assert_eq!(status.code(), tonic::Code::DataLoss);
        assert!(status.message().contains("42"));
    }

    #[test]
    fn error_to_status_invalid_header() {
        let err = Error::InvalidHeader("bad magic".into());
        let status = error_to_status(err);
        assert_eq!(status.code(), tonic::Code::DataLoss);
        assert!(status.message().contains("bad magic"));
    }

    #[test]
    fn error_to_status_event_too_large() {
        let err = Error::EventTooLarge {
            size: 70000,
            max: 65536,
        };
        let status = error_to_status(err);
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
        assert!(status.message().contains("70000"));
    }

    #[test]
    fn error_to_status_invalid_argument() {
        let err = Error::InvalidArgument("bad field".into());
        let status = error_to_status(err);
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
        assert!(status.message().contains("bad field"));
    }

    // -- parse_uuid tests --

    #[test]
    fn parse_uuid_valid() {
        let id = Uuid::new_v4();
        let result = parse_uuid(&id.to_string(), "event_id");
        assert_eq!(result.unwrap(), id);
    }

    #[test]
    fn parse_uuid_invalid() {
        let result = parse_uuid("not-a-uuid", "stream_id");
        let status = result.unwrap_err();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
        assert!(
            status.message().contains("stream_id"),
            "expected field name in message: {}",
            status.message()
        );
    }

    // -- proto_to_expected_version tests --

    #[test]
    fn proto_to_expected_version_none() {
        let result = proto_to_expected_version(None);
        let status = result.unwrap_err();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn proto_to_expected_version_any() {
        let ev = Some(proto::ExpectedVersion {
            kind: Some(proto::expected_version::Kind::Any(proto::Empty {})),
        });
        let result = proto_to_expected_version(ev).unwrap();
        assert_eq!(result, ExpectedVersion::Any);
    }

    #[test]
    fn proto_to_expected_version_no_stream() {
        let ev = Some(proto::ExpectedVersion {
            kind: Some(proto::expected_version::Kind::NoStream(proto::Empty {})),
        });
        let result = proto_to_expected_version(ev).unwrap();
        assert_eq!(result, ExpectedVersion::NoStream);
    }

    #[test]
    fn proto_to_expected_version_exact() {
        let ev = Some(proto::ExpectedVersion {
            kind: Some(proto::expected_version::Kind::Exact(42)),
        });
        let result = proto_to_expected_version(ev).unwrap();
        assert_eq!(result, ExpectedVersion::Exact(42));
    }

    // -- proto_to_proposed_event tests --

    #[test]
    fn proto_to_proposed_event_invalid_uuid() {
        let p = proto::ProposedEvent {
            event_id: "not-a-uuid".to_string(),
            event_type: "TestEvent".to_string(),
            metadata: vec![],
            payload: vec![1, 2, 3],
        };
        let result = proto_to_proposed_event(p);
        let status = result.unwrap_err();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
        assert!(
            status.message().contains("event_id"),
            "expected 'event_id' in message: {}",
            status.message()
        );
    }

    #[test]
    fn proto_to_proposed_event_valid() {
        let event_id = Uuid::new_v4();
        let p = proto::ProposedEvent {
            event_id: event_id.to_string(),
            event_type: "OrderPlaced".to_string(),
            metadata: vec![1, 2],
            payload: vec![3, 4, 5],
        };
        let result = proto_to_proposed_event(p).unwrap();
        assert_eq!(result.event_id, event_id);
        assert_eq!(result.event_type, "OrderPlaced");
        assert_eq!(result.metadata, Bytes::from(vec![1u8, 2]));
        assert_eq!(result.payload, Bytes::from(vec![3u8, 4, 5]));
    }

    // -- recorded_to_proto tests --

    #[test]
    fn recorded_to_proto_round_trip() {
        let event_id = Uuid::new_v4();
        let stream_id = Uuid::new_v4();
        let domain = RecordedEvent {
            event_id,
            stream_id,
            stream_version: 5,
            global_position: 42,
            recorded_at: 0,
            event_type: "PaymentReceived".to_string(),
            metadata: Bytes::from_static(b"meta"),
            payload: Bytes::from_static(b"payload"),
        };

        let proto_event = recorded_to_proto(&domain);

        assert_eq!(proto_event.event_id, event_id.to_string());
        assert_eq!(proto_event.stream_id, stream_id.to_string());
        assert_eq!(proto_event.stream_version, 5);
        assert_eq!(proto_event.global_position, 42);
        assert_eq!(proto_event.event_type, "PaymentReceived");
        assert_eq!(proto_event.metadata, b"meta");
        assert_eq!(proto_event.payload, b"payload");
    }

    #[test]
    fn recorded_to_proto_maps_recorded_at() {
        let event_id = Uuid::new_v4();
        let stream_id = Uuid::new_v4();
        let domain = RecordedEvent {
            event_id,
            stream_id,
            stream_version: 0,
            global_position: 0,
            recorded_at: 1_700_000_000_000,
            event_type: "TimestampTest".to_string(),
            metadata: Bytes::from_static(b"meta"),
            payload: Bytes::from_static(b"payload"),
        };

        let proto_event = recorded_to_proto(&domain);

        assert_eq!(
            proto_event.recorded_at, 1_700_000_000_000,
            "recorded_at should be mapped from domain to proto"
        );
    }

    // -- metrics tests --

    /// Helper: ensure the global metrics recorder is installed and return its handle.
    ///
    /// Because the recorder is global and static, only the first call installs it.
    /// Subsequent calls retrieve the existing handle.
    fn ensure_metrics_handle() -> crate::metrics::MetricsHandle {
        let _ = crate::metrics::install_recorder();
        crate::metrics::get_installed_handle().expect("metrics recorder should be installed")
    }

    /// Helper: create a minimal `EventfoldService` backed by a temp store.
    ///
    /// Returns `(service, _temp_dir)`. The `_temp_dir` must be kept alive for
    /// the duration of the test to prevent cleanup.
    fn temp_service() -> (EventfoldService, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");
        let store = crate::store::Store::open(&path).expect("open should succeed");
        let broker = crate::broker::Broker::new(1024);
        let dedup_cap = std::num::NonZeroUsize::new(128).expect("nonzero");
        let (writer_handle, read_index, _join_handle) =
            crate::writer::spawn_writer(store, 64, broker.clone(), dedup_cap);
        let service = EventfoldService::new(writer_handle, read_index, broker);
        (service, dir)
    }

    /// Extract the numeric value for a metric line from Prometheus-format text.
    ///
    /// Searches for a line starting with `prefix` (e.g., `eventfold_reads_total{rpc="read_all"}`)
    /// and returns its parsed `f64` value. Returns `None` if not found.
    fn parse_metric_value(rendered: &str, prefix: &str) -> Option<f64> {
        rendered.lines().find_map(|line| {
            line.strip_prefix(prefix)
                .and_then(|rest| rest.trim().parse::<f64>().ok())
        })
    }

    #[tokio::test]
    #[serial]
    async fn read_handlers_increment_reads_total_counter() {
        use crate::proto::event_store_server::EventStore;

        let handle = ensure_metrics_handle();
        let (service, _dir) = temp_service();

        // Snapshot current counter values before our calls.
        let before = handle.render();
        let read_stream_before =
            parse_metric_value(&before, r#"eventfold_reads_total{rpc="read_stream"} "#)
                .unwrap_or(0.0);
        let read_all_before =
            parse_metric_value(&before, r#"eventfold_reads_total{rpc="read_all"} "#).unwrap_or(0.0);

        // First, append one event so we have a valid stream to read from.
        let stream_id = Uuid::new_v4();
        let append_req = tonic::Request::new(proto::AppendRequest {
            stream_id: stream_id.to_string(),
            expected_version: Some(proto::ExpectedVersion {
                kind: Some(proto::expected_version::Kind::NoStream(proto::Empty {})),
            }),
            events: vec![proto::ProposedEvent {
                event_id: Uuid::new_v4().to_string(),
                event_type: "TestEvent".to_string(),
                metadata: vec![],
                payload: b"{}".to_vec(),
            }],
        });
        service
            .append(append_req)
            .await
            .expect("append should succeed");

        // Call read_stream once.
        let rs_req = tonic::Request::new(proto::ReadStreamRequest {
            stream_id: stream_id.to_string(),
            from_version: 0,
            max_count: 100,
        });
        service
            .read_stream(rs_req)
            .await
            .expect("read_stream should succeed");

        // Call read_all twice.
        for _ in 0..2 {
            let ra_req = tonic::Request::new(proto::ReadAllRequest {
                from_position: 0,
                max_count: 100,
            });
            service
                .read_all(ra_req)
                .await
                .expect("read_all should succeed");
        }

        // Render metrics and check deltas.
        let after = handle.render();
        let read_stream_after =
            parse_metric_value(&after, r#"eventfold_reads_total{rpc="read_stream"} "#)
                .expect("read_stream counter should exist");
        let read_all_after =
            parse_metric_value(&after, r#"eventfold_reads_total{rpc="read_all"} "#)
                .expect("read_all counter should exist");

        assert_eq!(
            read_stream_after - read_stream_before,
            1.0,
            "read_stream counter should have incremented by 1"
        );
        assert_eq!(
            read_all_after - read_all_before,
            2.0,
            "read_all counter should have incremented by 2"
        );
    }

    #[tokio::test]
    #[serial]
    async fn subscribe_all_increments_and_decrements_gauge() {
        use crate::proto::event_store_server::EventStore;
        use futures::StreamExt;

        let handle = ensure_metrics_handle();
        let (service, _dir) = temp_service();

        // Snapshot gauge before.
        let before = handle.render();
        let gauge_before =
            parse_metric_value(&before, "eventfold_subscriptions_active ").unwrap_or(0.0);

        // Start a subscribe_all call. The returned stream increments the gauge.
        let req = tonic::Request::new(proto::SubscribeAllRequest { from_position: 0 });
        let response = service
            .subscribe_all(req)
            .await
            .expect("subscribe_all should succeed");
        let mut stream = response.into_inner();

        // The first item from the stream is a CaughtUp marker (empty log).
        // Poll it to ensure the stream has started and the gauge has been incremented.
        let item = stream.next().await;
        assert!(item.is_some(), "stream should yield CaughtUp");

        // While the stream is open, the gauge should have increased by 1.
        let during = handle.render();
        let gauge_during = parse_metric_value(&during, "eventfold_subscriptions_active ")
            .expect("subscriptions_active gauge should exist");
        assert_eq!(
            gauge_during - gauge_before,
            1.0,
            "gauge should be incremented while stream is open"
        );

        // Drop the stream to trigger cleanup. The async_stream block will
        // complete and the gauge should decrement.
        drop(stream);

        // Give the runtime a moment to run the stream's cleanup path.
        tokio::task::yield_now().await;

        let after = handle.render();
        let gauge_after = parse_metric_value(&after, "eventfold_subscriptions_active ")
            .expect("subscriptions_active gauge should exist after drop");
        assert_eq!(
            gauge_after - gauge_before,
            0.0,
            "gauge should return to original value after stream is dropped"
        );
    }

    #[tokio::test]
    #[serial]
    async fn subscribe_stream_increments_and_decrements_gauge() {
        use crate::proto::event_store_server::EventStore;
        use futures::StreamExt;

        let handle = ensure_metrics_handle();
        let (service, _dir) = temp_service();

        // Append an event to create a stream.
        let stream_id = Uuid::new_v4();
        let append_req = tonic::Request::new(proto::AppendRequest {
            stream_id: stream_id.to_string(),
            expected_version: Some(proto::ExpectedVersion {
                kind: Some(proto::expected_version::Kind::NoStream(proto::Empty {})),
            }),
            events: vec![proto::ProposedEvent {
                event_id: Uuid::new_v4().to_string(),
                event_type: "TestEvent".to_string(),
                metadata: vec![],
                payload: b"{}".to_vec(),
            }],
        });
        service
            .append(append_req)
            .await
            .expect("append should succeed");

        // Snapshot gauge before.
        let before = handle.render();
        let gauge_before =
            parse_metric_value(&before, "eventfold_subscriptions_active ").unwrap_or(0.0);

        // Start a subscribe_stream call.
        let req = tonic::Request::new(proto::SubscribeStreamRequest {
            stream_id: stream_id.to_string(),
            from_version: 0,
        });
        let response = service
            .subscribe_stream(req)
            .await
            .expect("subscribe_stream should succeed");
        let mut stream = response.into_inner();

        // Poll the first item (the event we appended).
        let item = stream.next().await;
        assert!(item.is_some(), "stream should yield the appended event");

        // Gauge should have increased while stream is open.
        let during = handle.render();
        let gauge_during = parse_metric_value(&during, "eventfold_subscriptions_active ")
            .expect("subscriptions_active gauge should exist");
        assert_eq!(
            gauge_during - gauge_before,
            1.0,
            "gauge should be incremented while stream is open"
        );

        // Drop the stream.
        drop(stream);
        tokio::task::yield_now().await;

        let after = handle.render();
        let gauge_after = parse_metric_value(&after, "eventfold_subscriptions_active ")
            .expect("subscriptions_active gauge should exist after drop");
        assert_eq!(
            gauge_after - gauge_before,
            0.0,
            "gauge should return to original value after stream is dropped"
        );
    }
}
