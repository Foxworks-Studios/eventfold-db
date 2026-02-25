//! EventfoldDB: a lightweight, single-node event store for event sourcing and CQRS.

pub mod error;
pub mod types;

pub use error::Error;
pub use types::{
    ExpectedVersion, MAX_EVENT_SIZE, MAX_EVENT_TYPE_LEN, ProposedEvent, RecordedEvent,
};

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
}
