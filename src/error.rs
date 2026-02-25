//! Error types for EventfoldDB.
//!
//! This module defines the unified error enum used throughout the crate. All fallible
//! operations return `Result<T, Error>`. The gRPC service layer maps these variants
//! to appropriate gRPC status codes.

use uuid::Uuid;

/// Unified error type for all EventfoldDB operations.
///
/// Each variant represents a distinct failure mode. The gRPC layer maps variants
/// to status codes:
///
/// - `WrongExpectedVersion` -> `FAILED_PRECONDITION`
/// - `StreamNotFound` -> `NOT_FOUND`
/// - `Io` -> `INTERNAL`
/// - `CorruptRecord` -> `DATA_LOSS`
/// - `InvalidHeader` -> `DATA_LOSS`
/// - `EventTooLarge` -> `INVALID_ARGUMENT`
/// - `InvalidArgument` -> `INVALID_ARGUMENT`
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Optimistic concurrency check failed: the stream's current version does not
    /// match the caller's expectation.
    #[error("wrong expected version: expected {expected}, actual {actual}")]
    WrongExpectedVersion {
        /// The version the caller expected the stream to be at.
        expected: String,
        /// The version the stream is actually at.
        actual: String,
    },

    /// The requested stream does not exist.
    #[error("stream not found: {stream_id}")]
    StreamNotFound {
        /// UUID of the stream that was not found.
        stream_id: Uuid,
    },

    /// An I/O error occurred during a file operation.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A record on disk is corrupt (e.g., CRC mismatch, truncated data).
    #[error("corrupt record at position {position}: {detail}")]
    CorruptRecord {
        /// Global position of the corrupt record.
        position: u64,
        /// Human-readable description of the corruption.
        detail: String,
    },

    /// The file header is invalid or unrecognized.
    #[error("invalid file header: {0}")]
    InvalidHeader(String),

    /// The event exceeds the maximum allowed size.
    #[error("event too large: {size} bytes exceeds {max} byte limit")]
    EventTooLarge {
        /// Actual size of the event in bytes.
        size: usize,
        /// Maximum allowed size in bytes.
        max: usize,
    },

    /// A request argument is invalid.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    // AC-5: WrongExpectedVersion display includes "wrong expected version" and both values.

    #[test]
    fn wrong_expected_version_display() {
        let err = Error::WrongExpectedVersion {
            expected: "0".into(),
            actual: "1".into(),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("wrong expected version"),
            "expected 'wrong expected version' in: {msg}"
        );
        assert!(msg.contains("0"), "expected '0' in: {msg}");
        assert!(msg.contains("1"), "expected '1' in: {msg}");
    }

    // AC-5: StreamNotFound display includes the UUID string.

    #[test]
    fn stream_not_found_display() {
        let stream_id = Uuid::new_v4();
        let err = Error::StreamNotFound { stream_id };
        let msg = err.to_string();
        assert!(
            msg.contains(&stream_id.to_string()),
            "expected UUID in: {msg}"
        );
    }

    // AC-5: std::io::Error converts to Error::Io via From; display contains "I/O error".

    #[test]
    fn io_error_from_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let err = Error::from(io_err);
        // Verify it is the Io variant.
        assert!(matches!(err, Error::Io(_)));
        let msg = err.to_string();
        assert!(msg.contains("I/O error"), "expected 'I/O error' in: {msg}");
    }

    // AC-5: Io coercion via the ? operator works.

    #[test]
    fn io_error_question_mark_coercion() {
        fn fallible() -> Result<(), Error> {
            let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
            Err(io_err)?
        }

        let result = fallible();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::Io(_)));
    }

    // AC-5: CorruptRecord display includes position and detail.

    #[test]
    fn corrupt_record_display() {
        let err = Error::CorruptRecord {
            position: 42,
            detail: "bad crc".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("42"), "expected '42' in: {msg}");
        assert!(msg.contains("bad crc"), "expected 'bad crc' in: {msg}");
    }

    // AC-5: InvalidHeader display includes the reason.

    #[test]
    fn invalid_header_display() {
        let err = Error::InvalidHeader("bad magic".into());
        let msg = err.to_string();
        assert!(msg.contains("bad magic"), "expected 'bad magic' in: {msg}");
    }

    // AC-5: EventTooLarge display includes size and max.

    #[test]
    fn event_too_large_display() {
        let err = Error::EventTooLarge {
            size: 70000,
            max: 65536,
        };
        let msg = err.to_string();
        assert!(msg.contains("70000"), "expected '70000' in: {msg}");
        assert!(msg.contains("65536"), "expected '65536' in: {msg}");
    }

    // AC-5: InvalidArgument display includes the argument description.

    #[test]
    fn invalid_argument_display() {
        let err = Error::InvalidArgument("stream_id is empty".into());
        let msg = err.to_string();
        assert!(
            msg.contains("stream_id is empty"),
            "expected 'stream_id is empty' in: {msg}"
        );
    }

    // AC-5: All seven variants implement Debug (format via {:?} produces non-empty strings).

    #[test]
    fn all_variants_debug_non_empty() {
        let stream_id = Uuid::new_v4();
        let io_err = std::io::Error::other("test");

        let variants: Vec<Error> = vec![
            Error::WrongExpectedVersion {
                expected: "0".into(),
                actual: "1".into(),
            },
            Error::StreamNotFound { stream_id },
            Error::Io(io_err),
            Error::CorruptRecord {
                position: 0,
                detail: "truncated".into(),
            },
            Error::InvalidHeader("missing magic".into()),
            Error::EventTooLarge {
                size: 100_000,
                max: 65_536,
            },
            Error::InvalidArgument("empty".into()),
        ];

        for (i, variant) in variants.iter().enumerate() {
            let debug_str = format!("{variant:?}");
            assert!(
                !debug_str.is_empty(),
                "variant {i} produced empty Debug output"
            );
        }
    }
}
