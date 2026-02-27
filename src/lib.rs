//! EventfoldDB: a lightweight, single-node event store for event sourcing and CQRS.

pub mod broker;
pub mod codec;
pub(crate) mod dedup;
pub mod error;
/// Prometheus metrics infrastructure for EventfoldDB.
pub mod metrics;
/// Generated protobuf types for the EventfoldDB gRPC API.
pub mod proto {
    tonic::include_proto!("eventfold");
}
pub mod reader;
pub mod service;
pub mod store;
pub mod types;
pub mod writer;

pub use broker::{Broker, subscribe_all, subscribe_stream};
pub use codec::DecodeOutcome;
pub use error::Error;
pub use reader::ReadIndex;
pub use service::EventfoldService;
pub use store::Store;
pub use types::{
    ExpectedVersion, MAX_EVENT_SIZE, MAX_EVENT_TYPE_LEN, ProposedEvent, RecordedEvent, StreamInfo,
    SubscriptionMessage,
};
pub use writer::{WriterHandle, spawn_writer};

#[cfg(test)]
mod tests {
    // AC-6: Verify that all public items are accessible at the crate root.
    // Tests use fully-qualified `crate::` paths to confirm re-exports resolve.

    #[test]
    fn reexport_proposed_event() {
        let event = crate::ProposedEvent {
            event_id: uuid::Uuid::new_v4(),
            event_type: "TestEvent".to_string(),
            metadata: bytes::Bytes::new(),
            payload: bytes::Bytes::from_static(b"{}"),
        };
        assert_eq!(event.event_type, "TestEvent");
    }

    #[test]
    fn reexport_recorded_event() {
        let event = crate::RecordedEvent {
            event_id: uuid::Uuid::new_v4(),
            stream_id: uuid::Uuid::new_v4(),
            stream_version: 0,
            global_position: 0,
            recorded_at: 0,
            event_type: "TestEvent".to_string(),
            metadata: bytes::Bytes::new(),
            payload: bytes::Bytes::new(),
        };
        assert_eq!(event.global_position, 0);
    }

    #[test]
    fn reexport_expected_version() {
        // Construct each variant via the crate-root path.
        let any = crate::ExpectedVersion::Any;
        let no_stream = crate::ExpectedVersion::NoStream;
        let exact = crate::ExpectedVersion::Exact(7);

        // Copy semantics confirm the re-export resolves the correct type.
        let copy = any;
        assert_eq!(copy, crate::ExpectedVersion::Any);
        assert_eq!(no_stream, crate::ExpectedVersion::NoStream);
        assert_eq!(exact, crate::ExpectedVersion::Exact(7));
    }

    #[test]
    fn reexport_max_event_size() {
        assert_eq!(crate::MAX_EVENT_SIZE, 65_536);
    }

    #[test]
    fn reexport_max_event_type_len() {
        assert_eq!(crate::MAX_EVENT_TYPE_LEN, 256);
    }

    #[test]
    fn reexport_error() {
        let err = crate::Error::InvalidArgument("test".into());
        assert!(err.to_string().contains("test"));
    }

    #[test]
    fn proto_append_request_default() {
        // Verify that the generated proto types are accessible and constructable.
        let req = crate::proto::AppendRequest::default();
        assert!(req.stream_id.is_empty());
        assert!(req.events.is_empty());
        assert!(req.expected_version.is_none());
    }

    #[test]
    fn eventfold_service_accessible_at_crate_root() {
        // Verify that `crate::EventfoldService` resolves and that its constructor
        // has the expected signature (WriterHandle, ReadIndex, Broker) -> EventfoldService.
        let _: fn(crate::WriterHandle, crate::ReadIndex, crate::Broker) -> crate::EventfoldService =
            crate::EventfoldService::new;
    }

    #[test]
    fn reexport_stream_info() {
        let info = crate::StreamInfo {
            stream_id: uuid::Uuid::new_v4(),
            event_count: 3,
            latest_version: 2,
        };
        assert_eq!(info.event_count, 3);
    }

    #[test]
    fn event_store_server_accessible_via_proto() {
        // Verify that the tonic-generated EventStoreServer is reachable through
        // `crate::proto::event_store_server` and can be parameterized with
        // `crate::EventfoldService`.
        let _new_fn =
            crate::proto::event_store_server::EventStoreServer::<crate::EventfoldService>::new;
        // If this compiles, the type path is valid and accessible.
    }

    #[test]
    fn proto_list_streams_types_resolve() {
        // Verify that the tonic-generated ListStreams types are accessible at
        // the `crate::proto::` path and have the expected fields.
        let _: crate::proto::ListStreamsRequest = Default::default();

        let info = crate::proto::StreamInfo {
            stream_id: "test-id".to_string(),
            event_count: 5,
            latest_version: 4,
        };
        assert_eq!(info.stream_id, "test-id");
        assert_eq!(info.event_count, 5);
        assert_eq!(info.latest_version, 4);

        let resp = crate::proto::ListStreamsResponse {
            streams: vec![info],
        };
        assert_eq!(resp.streams.len(), 1);
    }
}
