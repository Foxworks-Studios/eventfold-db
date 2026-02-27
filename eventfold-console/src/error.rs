//! Error types for the eventfold-console TUI.
//!
//! Defines [`ConsoleError`], the unified error enum for all console operations.
//! Uses `thiserror` for derive-based error definitions. The top-level `main`
//! wraps this in `anyhow::Result` for convenience.

use thiserror::Error;

/// Unified error type for all eventfold-console operations.
///
/// # Variants
///
/// * `Grpc` - A gRPC transport or protocol error from tonic.
/// * `Io` - An I/O error (terminal, file system, etc.).
/// * `ConnectionFailed` - Could not connect to the EventfoldDB server.
#[derive(Debug, Error)]
pub enum ConsoleError {
    /// A gRPC error from the tonic transport or server.
    #[error("gRPC error: {0}")]
    Grpc(#[from] tonic::Status),

    /// An I/O error (terminal operations, etc.).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Failed to connect to the EventfoldDB server.
    #[error("connection failed: {0}")]
    ConnectionFailed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    // RED: ConsoleError::Grpc wraps a tonic::Status and displays it.
    #[test]
    fn grpc_error_from_tonic_status() {
        let status = tonic::Status::unavailable("server down");
        let err = ConsoleError::from(status);
        assert!(matches!(err, ConsoleError::Grpc(_)));
        let msg = err.to_string();
        assert!(msg.contains("gRPC error"), "got: {msg}");
        assert!(msg.contains("server down"), "got: {msg}");
    }

    // RED: ConsoleError::Io wraps a std::io::Error and displays it.
    #[test]
    fn io_error_from_std_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe broke");
        let err = ConsoleError::from(io_err);
        assert!(matches!(err, ConsoleError::Io(_)));
        let msg = err.to_string();
        assert!(msg.contains("I/O error"), "got: {msg}");
        assert!(msg.contains("pipe broke"), "got: {msg}");
    }

    // RED: ConsoleError::ConnectionFailed stores and displays the message.
    #[test]
    fn connection_failed_display() {
        let err = ConsoleError::ConnectionFailed("refused".into());
        let msg = err.to_string();
        assert!(msg.contains("connection failed"), "got: {msg}");
        assert!(msg.contains("refused"), "got: {msg}");
    }

    // RED: All variants implement Debug with non-empty output.
    #[test]
    fn all_variants_debug_non_empty() {
        let variants: Vec<ConsoleError> = vec![
            ConsoleError::Grpc(tonic::Status::internal("test")),
            ConsoleError::Io(std::io::Error::other("test")),
            ConsoleError::ConnectionFailed("test".into()),
        ];
        for (i, variant) in variants.iter().enumerate() {
            let debug = format!("{variant:?}");
            assert!(!debug.is_empty(), "variant {i} produced empty Debug");
        }
    }

    // RED: ConsoleError can be converted to anyhow::Error.
    #[test]
    fn converts_to_anyhow() {
        let err = ConsoleError::ConnectionFailed("test".into());
        let anyhow_err: anyhow::Error = err.into();
        assert!(anyhow_err.to_string().contains("connection failed"));
    }
}
