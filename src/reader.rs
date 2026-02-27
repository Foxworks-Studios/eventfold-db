//! Read-only handle to the in-memory event log.
//!
//! `ReadIndex` provides concurrent, read-only access to the in-memory event log
//! without going through the writer task. It wraps an `Arc<RwLock<EventLog>>` and
//! exposes read methods that acquire a read lock for the duration of the operation.

use std::sync::{Arc, RwLock};

use uuid::Uuid;

use crate::error::Error;
use crate::store::EventLog;
use crate::types::RecordedEvent;

/// Shared, read-only handle to the in-memory event log.
///
/// Holds an `Arc<RwLock<EventLog>>` and exposes read methods that acquire a
/// read lock. Multiple `ReadIndex` clones share the same underlying data --
/// cloning produces a new handle, not a copy of the data.
///
/// This is the handle that gRPC read handlers hold for concurrent reads
/// without going through the writer task.
#[derive(Clone, Debug)]
pub struct ReadIndex {
    /// Shared reference to the in-memory event log.
    log: Arc<RwLock<EventLog>>,
}

impl ReadIndex {
    /// Create a new `ReadIndex` backed by the given shared event log.
    ///
    /// # Arguments
    ///
    /// * `log` - Shared reference to the in-memory event log.
    ///
    /// # Returns
    ///
    /// A new `ReadIndex` handle.
    pub fn new(log: Arc<RwLock<EventLog>>) -> ReadIndex {
        ReadIndex { log }
    }

    /// Returns the current version of a stream (the last stream version assigned).
    ///
    /// Returns `None` if the stream does not exist. A stream with one event
    /// has version 0 (zero-based).
    ///
    /// # Arguments
    ///
    /// * `stream_id` - UUID of the stream to query.
    ///
    /// # Returns
    ///
    /// `Some(version)` if the stream exists, `None` otherwise.
    pub fn stream_version(&self, stream_id: &Uuid) -> Option<u64> {
        let log = self.log.read().expect("EventLog RwLock poisoned");
        log.streams
            .get(stream_id)
            .map(|positions| positions.len() as u64 - 1)
    }

    /// Returns the next global position (i.e., `events.len()` as `u64`).
    ///
    /// If the log is empty, returns 0. This is the position that the next
    /// appended event would receive.
    ///
    /// # Returns
    ///
    /// The number of events in the global log.
    pub fn global_position(&self) -> u64 {
        let log = self.log.read().expect("EventLog RwLock poisoned");
        log.events.len() as u64
    }

    /// Read events from a specific stream starting at a given version.
    ///
    /// Looks up the stream's global position list and returns cloned events
    /// from `from_version` up to `min(from_version + max_count, stream_length)`.
    ///
    /// # Arguments
    ///
    /// * `stream_id` - UUID of the stream to read.
    /// * `from_version` - Zero-based stream version to start reading from.
    /// * `max_count` - Maximum number of events to return.
    ///
    /// # Returns
    ///
    /// A `Vec` of `RecordedEvent` in stream version order.
    ///
    /// # Errors
    ///
    /// Returns `Error::StreamNotFound` if the stream does not exist.
    pub fn read_stream(
        &self,
        stream_id: Uuid,
        from_version: u64,
        max_count: u64,
    ) -> Result<Vec<RecordedEvent>, Error> {
        let log = self.log.read().expect("EventLog RwLock poisoned");
        let positions = log
            .streams
            .get(&stream_id)
            .ok_or(Error::StreamNotFound { stream_id })?;

        let stream_len = positions.len() as u64;
        let start = from_version.min(stream_len);
        let end = from_version.saturating_add(max_count).min(stream_len);

        Ok(positions[start as usize..end as usize]
            .iter()
            .map(|&global_pos| log.events[global_pos as usize].clone())
            .collect())
    }

    /// Read events from the global log starting at a given position.
    ///
    /// Returns cloned events from `from_position` up to
    /// `min(from_position + max_count, events.len())`. An empty result means
    /// the caller is at the head of the log.
    ///
    /// # Arguments
    ///
    /// * `from_position` - Zero-based global position to start reading from.
    /// * `max_count` - Maximum number of events to return.
    ///
    /// # Returns
    ///
    /// A `Vec` of `RecordedEvent` in global position order.
    pub fn read_all(&self, from_position: u64, max_count: u64) -> Vec<RecordedEvent> {
        let log = self.log.read().expect("EventLog RwLock poisoned");
        let len = log.events.len() as u64;
        let start = from_position.min(len);
        let end = from_position.saturating_add(max_count).min(len);
        log.events[start as usize..end as usize].to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{EventLog, Store};
    use crate::types::{ExpectedVersion, ProposedEvent};
    use bytes::Bytes;
    use std::collections::HashMap;
    use std::sync::{Arc, RwLock};
    use uuid::Uuid;

    /// Helper: create a `ProposedEvent` with minimal fields for testing.
    fn proposed(event_type: &str) -> ProposedEvent {
        ProposedEvent {
            event_id: Uuid::new_v4(),
            event_type: event_type.to_string(),
            metadata: Bytes::new(),
            payload: Bytes::from_static(b"{}"),
        }
    }

    /// Helper: open a Store at a temp path and append `n` events to a single stream.
    /// Returns `(stream_id, store)`.
    fn store_with_events(n: usize) -> (Uuid, Store, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");
        let mut store = Store::open(&path).expect("open should succeed");
        let stream_id = Uuid::new_v4();
        for i in 0..n {
            let expected = if i == 0 {
                ExpectedVersion::NoStream
            } else {
                ExpectedVersion::Exact(i as u64 - 1)
            };
            store
                .append(stream_id, expected, 0, vec![proposed("TestEvent")])
                .expect("append should succeed");
        }
        (stream_id, store, dir)
    }

    #[test]
    fn read_index_is_clone_and_debug() {
        let log = Arc::new(RwLock::new(EventLog {
            events: Vec::new(),
            streams: HashMap::new(),
        }));
        let index = ReadIndex::new(log);
        let cloned = index.clone();
        // Both should format via Debug without panicking.
        let debug_str = format!("{index:?}");
        assert!(!debug_str.is_empty());
        let debug_cloned = format!("{cloned:?}");
        assert!(!debug_cloned.is_empty());
    }

    #[test]
    fn read_all_returns_all_events_from_store() {
        let (_stream_id, store, _dir) = store_with_events(3);
        let index = ReadIndex::new(store.log());
        let events = index.read_all(0, 100);
        assert_eq!(events.len(), 3);
        for (i, event) in events.iter().enumerate() {
            assert_eq!(event.global_position, i as u64);
        }
    }

    #[test]
    fn stream_version_returns_correct_version() {
        let (stream_id, store, _dir) = store_with_events(3);
        let index = ReadIndex::new(store.log());
        // 3 events appended -> last stream version is 2 (zero-based).
        assert_eq!(index.stream_version(&stream_id), Some(2));
    }

    #[test]
    fn stream_version_returns_none_for_nonexistent() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");
        let store = Store::open(&path).expect("open should succeed");
        let index = ReadIndex::new(store.log());
        assert_eq!(index.stream_version(&Uuid::new_v4()), None);
    }

    #[test]
    fn global_position_returns_event_count() {
        let (_stream_id, store, _dir) = store_with_events(5);
        let index = ReadIndex::new(store.log());
        assert_eq!(index.global_position(), 5);
    }

    #[test]
    fn global_position_on_empty_returns_zero() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");
        let store = Store::open(&path).expect("open should succeed");
        let index = ReadIndex::new(store.log());
        assert_eq!(index.global_position(), 0);
    }

    #[test]
    fn two_clones_observe_same_data_after_append() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");
        let mut store = Store::open(&path).expect("open should succeed");
        let log = store.log();
        let index_a = ReadIndex::new(Arc::clone(&log));
        let index_b = ReadIndex::new(log);

        // Before any appends, both see empty.
        assert_eq!(index_a.global_position(), 0);
        assert_eq!(index_b.global_position(), 0);

        // Append through store.
        let stream_id = Uuid::new_v4();
        store
            .append(
                stream_id,
                ExpectedVersion::NoStream,
                0,
                vec![proposed("Created")],
            )
            .expect("append should succeed");

        // Both clones see the appended event.
        assert_eq!(index_a.global_position(), 1);
        assert_eq!(index_b.global_position(), 1);
        assert_eq!(index_a.read_all(0, 100).len(), 1);
        assert_eq!(index_b.read_all(0, 100).len(), 1);
    }

    #[test]
    fn read_stream_nonexistent_returns_stream_not_found() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");
        let store = Store::open(&path).expect("open should succeed");
        let index = ReadIndex::new(store.log());
        let unknown = Uuid::new_v4();
        match index.read_stream(unknown, 0, 100) {
            Err(Error::StreamNotFound { stream_id }) => {
                assert_eq!(stream_id, unknown);
            }
            Err(other) => panic!("expected StreamNotFound, got: {other:?}"),
            Ok(_) => panic!("expected StreamNotFound error, but read_stream succeeded"),
        }
    }

    #[test]
    fn read_stream_returns_correct_events_in_version_order() {
        let (stream_id, store, _dir) = store_with_events(3);
        let index = ReadIndex::new(store.log());
        let events = index
            .read_stream(stream_id, 0, 100)
            .expect("read_stream should succeed");
        assert_eq!(events.len(), 3);
        for (i, event) in events.iter().enumerate() {
            assert_eq!(event.stream_version, i as u64);
            assert_eq!(event.stream_id, stream_id);
        }
    }
}
