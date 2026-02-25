//! Storage engine for EventfoldDB.
//!
//! This module owns the append-only log file and the in-memory index. It provides
//! methods for opening (or creating) the store, appending events with optimistic
//! concurrency, and reading events by stream or globally.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

use uuid::Uuid;

use crate::codec::{self, DecodeOutcome};
use crate::error::Error;
use crate::types::{
    ExpectedVersion, MAX_EVENT_SIZE, MAX_EVENT_TYPE_LEN, ProposedEvent, RecordedEvent,
};

/// Size of the file header in bytes (magic + format version).
const HEADER_SIZE: usize = 8;

/// Check whether any valid record exists in `data` after byte offset `start`.
///
/// Scans forward one byte at a time from `start + 1` through the end of the
/// buffer, attempting to decode a record at each offset. Returns `true` if a
/// valid `DecodeOutcome::Complete` is found, indicating mid-file corruption
/// (the corrupt region is not at the tail).
fn has_valid_record_after(data: &[u8], start: usize) -> bool {
    // Start scanning from the byte after the corruption point. We try every
    // possible offset because we do not know the record boundaries after
    // corruption.
    for probe in (start + 1)..data.len() {
        if let Ok(DecodeOutcome::Complete { .. }) = codec::decode_record(&data[probe..]) {
            return true;
        }
    }
    false
}

/// Core storage engine that manages the append-only log file and in-memory index.
///
/// The `Store` owns the file handle for the log file and maintains two in-memory
/// data structures:
///
/// - `events`: A global log where index `i` is the event at global position `i`.
/// - `streams`: A map from stream UUID to a list of global positions, where
///   index `j` is the event at stream version `j`.
///
/// All writes go through `append()`, which validates concurrency, serializes
/// records to disk, fsyncs, and updates the index. Reads go directly to the
/// in-memory index with no disk I/O.
pub struct Store {
    /// Append-only log file handle.
    file: File,
    /// Global event log. Index `i` = event at global position `i`.
    events: Vec<RecordedEvent>,
    /// Stream index. Maps stream ID to list of global positions.
    /// Index `j` in the vec = event at stream version `j`.
    streams: HashMap<Uuid, Vec<u64>>,
}

impl Store {
    /// Open or create the event store at the given file path.
    ///
    /// If the file does not exist, creates it with the 8-byte file header,
    /// fsyncs, and returns an empty store. If the file exists, validates
    /// the header and recovers all valid events from the log, rebuilding the
    /// in-memory index.
    ///
    /// # Recovery behavior
    ///
    /// - **Trailing incomplete/corrupt record**: truncated from the file with a
    ///   `tracing::warn!` log. The store opens successfully with all preceding
    ///   valid events.
    /// - **Mid-file corruption** (corrupt record followed by valid records):
    ///   returns [`Error::CorruptRecord`]. This is unrecoverable.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the append-only log file.
    ///
    /// # Returns
    ///
    /// A `Store` instance with the in-memory index populated from the log.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the file cannot be created or written.
    /// Returns [`Error::InvalidHeader`] if an existing file has a bad header.
    /// Returns [`Error::CorruptRecord`] if mid-file corruption is detected.
    pub fn open(path: &Path) -> Result<Store, Error> {
        if !path.exists() {
            // New file: create with read+write so append() can write later.
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(path)?;
            file.write_all(&codec::encode_header())?;
            file.sync_all()?;

            return Ok(Store {
                file,
                events: Vec::new(),
                streams: HashMap::new(),
            });
        }

        // Existing file: read contents, validate header, recover records.
        let data = std::fs::read(path)?;

        if data.len() < HEADER_SIZE {
            return Err(Error::InvalidHeader(format!(
                "file too short for header: {} bytes",
                data.len()
            )));
        }

        // Validate the 8-byte header.
        let header: &[u8; 8] = data[..HEADER_SIZE]
            .try_into()
            .expect("slice is exactly 8 bytes");
        codec::decode_header(header)?;

        // Decode records sequentially from offset HEADER_SIZE.
        let mut events = Vec::new();
        let mut streams: HashMap<Uuid, Vec<u64>> = HashMap::new();
        let mut offset = HEADER_SIZE;

        loop {
            let remaining = &data[offset..];
            if remaining.is_empty() {
                break;
            }

            match codec::decode_record(remaining) {
                Ok(DecodeOutcome::Complete { event, consumed }) => {
                    let global_pos = event.global_position;
                    let stream_id = event.stream_id;
                    events.push(event);
                    streams.entry(stream_id).or_default().push(global_pos);
                    offset += consumed;
                }
                Ok(DecodeOutcome::Incomplete) | Err(Error::CorruptRecord { .. }) => {
                    // Potential trailing corruption/incompleteness. Check
                    // whether any valid records exist after this point.
                    // If so, it is mid-file corruption (fatal).
                    if has_valid_record_after(&data, offset) {
                        return Err(Error::CorruptRecord {
                            position: events.len() as u64,
                            detail: "mid-file corruption: valid records follow \
                                     corrupt/incomplete record"
                                .to_string(),
                        });
                    }

                    // Trailing partial/corrupt record -- truncate.
                    tracing::warn!(
                        offset,
                        valid_events = events.len(),
                        "truncating trailing partial/corrupt record at byte offset {offset}"
                    );

                    // Reopen file for write, truncate to last valid boundary, fsync.
                    let file = OpenOptions::new().read(true).write(true).open(path)?;
                    file.set_len(offset as u64)?;
                    file.sync_all()?;

                    return Ok(Store {
                        file,
                        events,
                        streams,
                    });
                }
                Err(e) => return Err(e),
            }
        }

        // All records decoded successfully. Open file for future appends.
        let file = OpenOptions::new().read(true).write(true).open(path)?;

        Ok(Store {
            file,
            events,
            streams,
        })
    }

    /// Returns the current version of a stream (zero-based), or `None` if
    /// the stream does not exist.
    ///
    /// The version is the index of the last event in the stream. A stream
    /// with 3 events has version 2.
    ///
    /// # Arguments
    ///
    /// * `stream_id` - UUID of the stream to query.
    ///
    /// # Returns
    ///
    /// `Some(version)` if the stream exists, `None` otherwise.
    pub fn stream_version(&self, stream_id: &Uuid) -> Option<u64> {
        self.streams
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
        self.events.len() as u64
    }

    /// Read events from the global log starting at a given position.
    ///
    /// Returns cloned events from `from_position` up to
    /// `min(from_position + max_count, events.len())`. Never errors -- an empty
    /// result means the caller is at the head of the log.
    ///
    /// # Arguments
    ///
    /// * `from_position` - Zero-based global position to start reading from.
    /// * `max_count` - Maximum number of events to return.
    ///
    /// # Returns
    ///
    /// A `Vec<RecordedEvent>` containing the requested events. Returns an empty
    /// vec if `from_position >= events.len()`.
    pub fn read_all(&self, from_position: u64, max_count: u64) -> Vec<RecordedEvent> {
        let len = self.events.len() as u64;
        let start = from_position.min(len);
        let end = from_position.saturating_add(max_count).min(len);
        self.events[start as usize..end as usize].to_vec()
    }

    /// Read events from a specific stream starting at a given version.
    ///
    /// Looks up the stream's global position list and returns cloned events from
    /// `from_version` up to `min(from_version + max_count, stream_length)`.
    ///
    /// # Arguments
    ///
    /// * `stream_id` - UUID of the stream to read.
    /// * `from_version` - Zero-based stream version to start reading from.
    /// * `max_count` - Maximum number of events to return.
    ///
    /// # Returns
    ///
    /// A `Vec<RecordedEvent>` containing the requested events.
    /// Returns an empty vec if `from_version >= stream_length`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::StreamNotFound`] if the stream does not exist.
    pub fn read_stream(
        &self,
        stream_id: Uuid,
        from_version: u64,
        max_count: u64,
    ) -> Result<Vec<RecordedEvent>, Error> {
        let positions = self
            .streams
            .get(&stream_id)
            .ok_or(Error::StreamNotFound { stream_id })?;

        let stream_len = positions.len() as u64;
        let start = from_version.min(stream_len);
        let end = from_version.saturating_add(max_count).min(stream_len);

        Ok(positions[start as usize..end as usize]
            .iter()
            .map(|&global_pos| self.events[global_pos as usize].clone())
            .collect())
    }

    /// Append events to a stream with optimistic concurrency control.
    ///
    /// Validates the expected version against the current stream state, validates
    /// each proposed event's type and size, serializes records, writes them to
    /// the log file, fsyncs, updates the in-memory index, and returns the
    /// recorded events.
    ///
    /// Appends are atomic: if any validation fails, no events are written.
    ///
    /// # Arguments
    ///
    /// * `stream_id` - UUID of the target stream.
    /// * `expected_version` - Concurrency check against current stream state.
    /// * `proposed_events` - Events to append.
    ///
    /// # Returns
    ///
    /// A `Vec<RecordedEvent>` with server-assigned positions on success.
    ///
    /// # Errors
    ///
    /// Returns [`Error::WrongExpectedVersion`] if the concurrency check fails.
    /// Returns [`Error::InvalidArgument`] if an event type is empty or exceeds
    /// [`MAX_EVENT_TYPE_LEN`] bytes.
    /// Returns [`Error::EventTooLarge`] if a record exceeds [`MAX_EVENT_SIZE`].
    /// Returns [`Error::Io`] if writing to the log file fails.
    pub fn append(
        &mut self,
        stream_id: Uuid,
        expected_version: ExpectedVersion,
        proposed_events: Vec<ProposedEvent>,
    ) -> Result<Vec<RecordedEvent>, Error> {
        // Step 1: Validate expected version against current stream state.
        let stream_positions = self.streams.get(&stream_id);
        match expected_version {
            ExpectedVersion::Any => {} // always passes
            ExpectedVersion::NoStream => {
                if let Some(positions) = stream_positions {
                    let actual_version = positions.len() as u64 - 1;
                    return Err(Error::WrongExpectedVersion {
                        expected: "NoStream".to_string(),
                        actual: actual_version.to_string(),
                    });
                }
            }
            ExpectedVersion::Exact(n) => match stream_positions {
                None => {
                    return Err(Error::WrongExpectedVersion {
                        expected: n.to_string(),
                        actual: "NoStream".to_string(),
                    });
                }
                Some(positions) => {
                    let current_version = positions.len() as u64 - 1;
                    if current_version != n {
                        return Err(Error::WrongExpectedVersion {
                            expected: n.to_string(),
                            actual: current_version.to_string(),
                        });
                    }
                }
            },
        }

        // Step 2: Build RecordedEvents and validate each one.
        let mut next_global = self.events.len() as u64;
        let mut next_stream_version = stream_positions.map(|p| p.len() as u64).unwrap_or(0);

        let mut recorded = Vec::with_capacity(proposed_events.len());
        let mut encoded_batch = Vec::new();

        for proposed in &proposed_events {
            // Validate event type: must be non-empty.
            if proposed.event_type.is_empty() {
                return Err(Error::InvalidArgument(
                    "event type must not be empty".to_string(),
                ));
            }
            // Validate event type: must not exceed MAX_EVENT_TYPE_LEN bytes.
            if proposed.event_type.len() > MAX_EVENT_TYPE_LEN {
                return Err(Error::InvalidArgument(format!(
                    "event type exceeds {} byte limit: {} bytes",
                    MAX_EVENT_TYPE_LEN,
                    proposed.event_type.len()
                )));
            }

            let event = RecordedEvent {
                event_id: proposed.event_id,
                stream_id,
                stream_version: next_stream_version,
                global_position: next_global,
                event_type: proposed.event_type.clone(),
                metadata: proposed.metadata.clone(),
                payload: proposed.payload.clone(),
            };

            // Validate total encoded size.
            let encoded = codec::encode_record(&event);
            if encoded.len() > MAX_EVENT_SIZE {
                return Err(Error::EventTooLarge {
                    size: encoded.len(),
                    max: MAX_EVENT_SIZE,
                });
            }

            encoded_batch.extend_from_slice(&encoded);
            recorded.push(event);
            next_global += 1;
            next_stream_version += 1;
        }

        // Step 3: Write all encoded records to disk and fsync.
        use std::io::Seek;
        self.file.seek(std::io::SeekFrom::End(0))?;
        self.file.write_all(&encoded_batch)?;
        self.file.sync_all()?;

        // Step 4: Update in-memory index.
        let stream_entry = self.streams.entry(stream_id).or_default();
        for event in &recorded {
            stream_entry.push(event.global_position);
        }
        self.events.extend(recorded.clone());

        Ok(recorded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ExpectedVersion, ProposedEvent};
    use bytes::Bytes;

    /// Helper: build a `RecordedEvent` with specified fields for test convenience.
    fn make_event(
        global_position: u64,
        stream_id: Uuid,
        stream_version: u64,
        event_type: &str,
        payload: &[u8],
    ) -> RecordedEvent {
        RecordedEvent {
            event_id: Uuid::new_v4(),
            stream_id,
            stream_version,
            global_position,
            event_type: event_type.to_string(),
            metadata: Bytes::new(),
            payload: Bytes::copy_from_slice(payload),
        }
    }

    /// Helper: write a seeded log file with the given events (header + encoded records).
    fn seed_file(path: &std::path::Path, events: &[RecordedEvent]) {
        use std::io::Write;
        let mut file = File::create(path).expect("create seed file");
        file.write_all(&codec::encode_header())
            .expect("write header");
        for event in events {
            file.write_all(&codec::encode_record(event))
                .expect("write record");
        }
        file.sync_all().expect("sync seed file");
    }

    #[test]
    fn open_creates_file_with_header_and_empty_store() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        // File should not exist yet.
        assert!(!path.exists());

        let store = Store::open(&path).expect("open should succeed");

        // File should now exist on disk.
        assert!(path.exists());

        // First 8 bytes should equal the codec header.
        let contents = std::fs::read(&path).expect("read file");
        assert_eq!(&contents[..8], &crate::codec::encode_header());

        // Store should be empty.
        assert_eq!(store.global_position(), 0);
    }

    #[test]
    fn global_position_on_empty_store_returns_zero() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let store = Store::open(&path).expect("open should succeed");
        assert_eq!(store.global_position(), 0);
    }

    #[test]
    fn stream_version_on_empty_store_returns_none() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let store = Store::open(&path).expect("open should succeed");
        let random_id = Uuid::new_v4();
        assert_eq!(store.stream_version(&random_id), None);
    }

    // -- AC-2: Seed 5 events across 2 streams, reopen -- all 5 recovered with correct
    // stream versions and global positions.
    #[test]
    fn recovery_rebuilds_index_from_5_events_across_2_streams() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let stream_a = Uuid::new_v4();
        let stream_b = Uuid::new_v4();

        // 5 events: stream_a gets 3 (versions 0,1,2), stream_b gets 2 (versions 0,1).
        // Interleaved: A0, B0, A1, B1, A2
        let events = vec![
            make_event(0, stream_a, 0, "TypeA", b"a0"),
            make_event(1, stream_b, 0, "TypeB", b"b0"),
            make_event(2, stream_a, 1, "TypeA", b"a1"),
            make_event(3, stream_b, 1, "TypeB", b"b1"),
            make_event(4, stream_a, 2, "TypeA", b"a2"),
        ];

        seed_file(&path, &events);

        let store = Store::open(&path).expect("recovery should succeed");

        // Global position should be 5 (next available).
        assert_eq!(store.global_position(), 5);

        // Stream versions: stream_a at version 2, stream_b at version 1.
        assert_eq!(store.stream_version(&stream_a), Some(2));
        assert_eq!(store.stream_version(&stream_b), Some(1));

        // Verify all recovered events match originals by comparing field-by-field
        // (event_id, stream_id, stream_version, global_position, event_type, payload).
        for (i, expected) in events.iter().enumerate() {
            let recovered = &store.events[i];
            assert_eq!(
                recovered.event_id, expected.event_id,
                "event {i} event_id mismatch"
            );
            assert_eq!(
                recovered.stream_id, expected.stream_id,
                "event {i} stream_id mismatch"
            );
            assert_eq!(
                recovered.stream_version, expected.stream_version,
                "event {i} stream_version mismatch"
            );
            assert_eq!(
                recovered.global_position, expected.global_position,
                "event {i} global_position mismatch"
            );
            assert_eq!(
                recovered.event_type, expected.event_type,
                "event {i} event_type mismatch"
            );
            assert_eq!(
                recovered.payload, expected.payload,
                "event {i} payload mismatch"
            );
        }
    }

    // -- AC-3: Seed 3 events, append 10 garbage bytes, reopen -- only 3 events
    // recovered, file truncated to valid boundary, no error.
    #[test]
    fn recovery_truncates_trailing_garbage_bytes() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let stream = Uuid::new_v4();
        let events = vec![
            make_event(0, stream, 0, "Evt", b"p0"),
            make_event(1, stream, 1, "Evt", b"p1"),
            make_event(2, stream, 2, "Evt", b"p2"),
        ];

        seed_file(&path, &events);

        // Compute the expected valid file size before appending garbage.
        let valid_size = std::fs::metadata(&path).expect("metadata").len();

        // Append 10 garbage bytes (simulating crash mid-write).
        {
            use std::io::Write;
            let mut file = OpenOptions::new()
                .append(true)
                .open(&path)
                .expect("open for append");
            file.write_all(&[0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0x01, 0x02, 0x03, 0x04])
                .expect("write garbage");
            file.sync_all().expect("sync");
        }

        // File should now be larger than valid_size.
        assert!(std::fs::metadata(&path).expect("metadata").len() > valid_size);

        // Reopen -- should recover 3 events and truncate the garbage.
        let store = Store::open(&path).expect("recovery should succeed");

        assert_eq!(store.global_position(), 3);
        assert_eq!(store.stream_version(&stream), Some(2));

        // File should be truncated back to the valid boundary.
        let final_size = std::fs::metadata(&path).expect("metadata").len();
        assert_eq!(
            final_size, valid_size,
            "file should be truncated to valid boundary"
        );
    }

    // -- AC-4: Seed 3 events, flip last byte of 3rd record's payload (CRC fails),
    // reopen -- only 2 events recovered, corrupt 3rd treated as trailing.
    #[test]
    fn recovery_truncates_crc_corrupt_last_record() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let stream = Uuid::new_v4();
        let events = vec![
            make_event(0, stream, 0, "Evt", b"payload0"),
            make_event(1, stream, 1, "Evt", b"payload1"),
            make_event(2, stream, 2, "Evt", b"payload2"),
        ];

        seed_file(&path, &events);

        // Compute the byte offset where the 3rd record starts:
        // header (8) + record_0 size + record_1 size.
        let rec0 = codec::encode_record(&events[0]);
        let rec1 = codec::encode_record(&events[1]);
        let valid_boundary = HEADER_SIZE + rec0.len() + rec1.len();

        // Read the file, flip the last byte of the 3rd record's payload region
        // (which is just before the CRC at the end).
        let mut data = std::fs::read(&path).expect("read file");
        // The last 4 bytes of the file are the CRC of the 3rd record.
        // The byte just before the CRC is the last payload byte.
        let corrupt_idx = data.len() - 5;
        data[corrupt_idx] ^= 0xFF;
        std::fs::write(&path, &data).expect("write corrupted file");

        // Reopen -- should recover only the first 2 events.
        let store = Store::open(&path).expect("recovery should succeed");

        assert_eq!(store.global_position(), 2);
        assert_eq!(store.stream_version(&stream), Some(1));

        // File should be truncated to the boundary before the 3rd record.
        let final_size = std::fs::metadata(&path).expect("metadata").len() as usize;
        assert_eq!(
            final_size, valid_boundary,
            "file should be truncated to end of 2nd record"
        );
    }

    // -- AC-5: Seed 3 events, corrupt a byte in the 2nd record (not last),
    // reopen -- returns Err(Error::CorruptRecord) because valid data follows.
    #[test]
    fn recovery_returns_error_on_mid_file_corruption() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let stream = Uuid::new_v4();
        let events = vec![
            make_event(0, stream, 0, "Evt", b"payload0"),
            make_event(1, stream, 1, "Evt", b"payload1"),
            make_event(2, stream, 2, "Evt", b"payload2"),
        ];

        seed_file(&path, &events);

        // Corrupt a byte in the 2nd record. The 2nd record starts at:
        // header (8) + record_0 size.
        let rec0 = codec::encode_record(&events[0]);
        let rec1_start = HEADER_SIZE + rec0.len();

        // Flip a byte within the 2nd record's payload region. The payload
        // starts after fixed fields inside the record. We flip a byte
        // roughly in the middle of the 2nd record body.
        let mut data = std::fs::read(&path).expect("read file");
        let corrupt_idx = rec1_start + 20; // well inside the 2nd record body
        data[corrupt_idx] ^= 0xFF;
        std::fs::write(&path, &data).expect("write corrupted file");

        // Reopen -- should return CorruptRecord because the 3rd record is valid.
        match Store::open(&path) {
            Err(Error::CorruptRecord { .. }) => {} // expected
            Err(other) => panic!("expected CorruptRecord, got: {other:?}"),
            Ok(_) => panic!("expected CorruptRecord error, but open() succeeded"),
        }
    }

    /// Helper: build a `ProposedEvent` with the given event type and payload.
    fn make_proposed(event_type: &str, payload: &[u8]) -> ProposedEvent {
        ProposedEvent {
            event_id: Uuid::new_v4(),
            event_type: event_type.to_string(),
            metadata: Bytes::new(),
            payload: Bytes::copy_from_slice(payload),
        }
    }

    // -- AC-6: Append one event with NoStream to a new stream.
    #[test]
    fn append_single_event_no_stream() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let mut store = Store::open(&path).expect("open should succeed");
        let stream_id = Uuid::new_v4();
        let proposed = vec![make_proposed("OrderPlaced", b"{\"qty\":1}")];

        let recorded = store
            .append(stream_id, ExpectedVersion::NoStream, proposed)
            .expect("append should succeed");

        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].stream_version, 0);
        assert_eq!(recorded[0].global_position, 0);
        assert_eq!(recorded[0].stream_id, stream_id);
        assert_eq!(recorded[0].event_type, "OrderPlaced");
        assert_eq!(store.global_position(), 1);
        assert_eq!(store.stream_version(&stream_id), Some(0));
    }

    // -- AC-7: Append 3 events in one call -> versions 0,1,2 and positions 0,1,2.
    #[test]
    fn append_batch_three_events() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let mut store = Store::open(&path).expect("open should succeed");
        let stream_id = Uuid::new_v4();
        let proposed = vec![
            make_proposed("Evt1", b"p1"),
            make_proposed("Evt2", b"p2"),
            make_proposed("Evt3", b"p3"),
        ];

        let recorded = store
            .append(stream_id, ExpectedVersion::NoStream, proposed)
            .expect("append should succeed");

        assert_eq!(recorded.len(), 3);
        for (i, event) in recorded.iter().enumerate() {
            assert_eq!(
                event.stream_version, i as u64,
                "stream_version for event {i}"
            );
            assert_eq!(
                event.global_position, i as u64,
                "global_position for event {i}"
            );
            assert_eq!(event.stream_id, stream_id);
        }
        assert_eq!(store.global_position(), 3);
        assert_eq!(store.stream_version(&stream_id), Some(2));
    }

    // -- AC-8a: Any on non-existent stream succeeds.
    #[test]
    fn append_any_on_nonexistent_stream_succeeds() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let mut store = Store::open(&path).expect("open should succeed");
        let stream_id = Uuid::new_v4();
        let proposed = vec![make_proposed("Created", b"{}")];

        let recorded = store
            .append(stream_id, ExpectedVersion::Any, proposed)
            .expect("append with Any on new stream should succeed");

        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].stream_version, 0);
        assert_eq!(recorded[0].global_position, 0);
    }

    // -- AC-8b: Any on existing stream at version 2 -> appended event has stream_version = 3.
    #[test]
    fn append_any_on_existing_stream_appends_at_correct_version() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let mut store = Store::open(&path).expect("open should succeed");
        let stream_id = Uuid::new_v4();

        // First append: 3 events -> versions 0, 1, 2.
        let first_batch = vec![
            make_proposed("Evt1", b"p1"),
            make_proposed("Evt2", b"p2"),
            make_proposed("Evt3", b"p3"),
        ];
        store
            .append(stream_id, ExpectedVersion::Any, first_batch)
            .expect("first append should succeed");

        assert_eq!(store.stream_version(&stream_id), Some(2));

        // Second append with Any: should get stream_version = 3.
        let second_batch = vec![make_proposed("Evt4", b"p4")];
        let recorded = store
            .append(stream_id, ExpectedVersion::Any, second_batch)
            .expect("second append with Any should succeed");

        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].stream_version, 3);
        assert_eq!(recorded[0].global_position, 3);
    }

    // -- AC-9a: NoStream on non-existent stream succeeds.
    #[test]
    fn append_no_stream_on_nonexistent_stream_succeeds() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let mut store = Store::open(&path).expect("open should succeed");
        let stream_id = Uuid::new_v4();
        let proposed = vec![make_proposed("Created", b"{}")];

        let recorded = store
            .append(stream_id, ExpectedVersion::NoStream, proposed)
            .expect("append with NoStream on new stream should succeed");

        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].stream_version, 0);
    }

    // -- AC-9b: NoStream on existing stream -> Err(WrongExpectedVersion).
    #[test]
    fn append_no_stream_on_existing_stream_returns_error() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let mut store = Store::open(&path).expect("open should succeed");
        let stream_id = Uuid::new_v4();

        // Create the stream first.
        let first = vec![make_proposed("Created", b"{}")];
        store
            .append(stream_id, ExpectedVersion::NoStream, first)
            .expect("first append should succeed");

        // Now try NoStream again -- should fail.
        let second = vec![make_proposed("Updated", b"{}")];
        match store.append(stream_id, ExpectedVersion::NoStream, second) {
            Err(Error::WrongExpectedVersion { .. }) => {} // expected
            Err(other) => panic!("expected WrongExpectedVersion, got: {other:?}"),
            Ok(_) => panic!("expected WrongExpectedVersion error, but append succeeded"),
        }
    }

    // -- AC-10a: Exact(0) after single event -> succeeds, new event at version 1.
    #[test]
    fn append_exact_version_matches_succeeds() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let mut store = Store::open(&path).expect("open should succeed");
        let stream_id = Uuid::new_v4();

        // Append one event -> stream at version 0.
        let first = vec![make_proposed("Created", b"{}")];
        store
            .append(stream_id, ExpectedVersion::NoStream, first)
            .expect("first append should succeed");

        // Append with Exact(0) -> should succeed, new event at version 1.
        let second = vec![make_proposed("Updated", b"{}")];
        let recorded = store
            .append(stream_id, ExpectedVersion::Exact(0), second)
            .expect("append with Exact(0) should succeed");

        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].stream_version, 1);
    }

    // -- AC-10b: Exact(5) when stream at version 3 -> Err(WrongExpectedVersion).
    #[test]
    fn append_exact_version_mismatch_returns_error() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let mut store = Store::open(&path).expect("open should succeed");
        let stream_id = Uuid::new_v4();

        // Append 4 events -> stream at version 3.
        let events = vec![
            make_proposed("Evt1", b"p1"),
            make_proposed("Evt2", b"p2"),
            make_proposed("Evt3", b"p3"),
            make_proposed("Evt4", b"p4"),
        ];
        store
            .append(stream_id, ExpectedVersion::NoStream, events)
            .expect("first append should succeed");

        assert_eq!(store.stream_version(&stream_id), Some(3));

        // Try Exact(5) -- should fail.
        let next = vec![make_proposed("Evt5", b"p5")];
        match store.append(stream_id, ExpectedVersion::Exact(5), next) {
            Err(Error::WrongExpectedVersion { .. }) => {} // expected
            Err(other) => panic!("expected WrongExpectedVersion, got: {other:?}"),
            Ok(_) => panic!("expected WrongExpectedVersion error, but append succeeded"),
        }
    }

    // -- AC-10c: Exact(0) on non-existent stream -> Err(WrongExpectedVersion).
    #[test]
    fn append_exact_on_nonexistent_stream_returns_error() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let mut store = Store::open(&path).expect("open should succeed");
        let stream_id = Uuid::new_v4();

        let proposed = vec![make_proposed("Created", b"{}")];
        match store.append(stream_id, ExpectedVersion::Exact(0), proposed) {
            Err(Error::WrongExpectedVersion { .. }) => {} // expected
            Err(other) => panic!("expected WrongExpectedVersion, got: {other:?}"),
            Ok(_) => panic!("expected WrongExpectedVersion error, but append succeeded"),
        }
    }

    // -- AC-11a: Oversized payload -> Err(EventTooLarge), file size unchanged.
    #[test]
    fn append_oversized_payload_returns_event_too_large() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let mut store = Store::open(&path).expect("open should succeed");
        let stream_id = Uuid::new_v4();

        let file_size_before = std::fs::metadata(&path).expect("metadata").len();

        // Create a payload that will make the total encoded record exceed MAX_EVENT_SIZE.
        let oversized_payload = vec![0xAA; MAX_EVENT_SIZE];
        let proposed = vec![ProposedEvent {
            event_id: Uuid::new_v4(),
            event_type: "BigEvent".to_string(),
            metadata: Bytes::new(),
            payload: Bytes::from(oversized_payload),
        }];

        match store.append(stream_id, ExpectedVersion::Any, proposed) {
            Err(Error::EventTooLarge { .. }) => {} // expected
            Err(other) => panic!("expected EventTooLarge, got: {other:?}"),
            Ok(_) => panic!("expected EventTooLarge error, but append succeeded"),
        }

        // File size should not have changed.
        let file_size_after = std::fs::metadata(&path).expect("metadata").len();
        assert_eq!(
            file_size_before, file_size_after,
            "file should not grow on failed append"
        );
    }

    // -- AC-11b: Event type exceeding MAX_EVENT_TYPE_LEN -> Err(InvalidArgument).
    #[test]
    fn append_event_type_too_long_returns_invalid_argument() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let mut store = Store::open(&path).expect("open should succeed");
        let stream_id = Uuid::new_v4();

        let long_type = "A".repeat(MAX_EVENT_TYPE_LEN + 1);
        let proposed = vec![make_proposed(&long_type, b"{}")];

        match store.append(stream_id, ExpectedVersion::Any, proposed) {
            Err(Error::InvalidArgument(msg)) => {
                assert!(
                    msg.contains("event type"),
                    "error should mention 'event type', got: {msg}"
                );
            }
            Err(other) => panic!("expected InvalidArgument, got: {other:?}"),
            Ok(_) => panic!("expected InvalidArgument error, but append succeeded"),
        }
    }

    // -- AC-11c: Empty event type -> Err(InvalidArgument).
    #[test]
    fn append_empty_event_type_returns_invalid_argument() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let mut store = Store::open(&path).expect("open should succeed");
        let stream_id = Uuid::new_v4();

        let proposed = vec![make_proposed("", b"{}")];

        match store.append(stream_id, ExpectedVersion::Any, proposed) {
            Err(Error::InvalidArgument(msg)) => {
                assert!(
                    msg.contains("event type"),
                    "error should mention 'event type', got: {msg}"
                );
            }
            Err(other) => panic!("expected InvalidArgument, got: {other:?}"),
            Ok(_) => panic!("expected InvalidArgument error, but append succeeded"),
        }
    }

    // -- AC-15a: stream_version() on stream with 3 events returns Some(2).
    #[test]
    fn stream_version_with_events_returns_last_version() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");
        let (stream_id, store) = open_and_append_to_stream(&path, 3);

        assert_eq!(store.stream_version(&stream_id), Some(2));
    }

    // -- AC-15b: global_position() after 5 appends returns 5.
    #[test]
    fn global_position_after_five_appends_returns_five() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");
        let (_stream_id, store) = open_and_append_to_stream(&path, 5);

        assert_eq!(store.global_position(), 5);
    }

    // -- AC-14: Interleaved appends to 3 streams; read_all returns global order,
    // read_stream for each returns only that stream's events in version order.
    #[test]
    fn multi_stream_interleaved_reads() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");
        let mut store = Store::open(&path).expect("open should succeed");

        let stream_a = Uuid::new_v4();
        let stream_b = Uuid::new_v4();
        let stream_c = Uuid::new_v4();

        // Append interleaved: A, B, A, C, B
        store
            .append(
                stream_a,
                ExpectedVersion::NoStream,
                vec![make_proposed("A0", b"a0")],
            )
            .expect("append A0");
        store
            .append(
                stream_b,
                ExpectedVersion::NoStream,
                vec![make_proposed("B0", b"b0")],
            )
            .expect("append B0");
        store
            .append(
                stream_a,
                ExpectedVersion::Exact(0),
                vec![make_proposed("A1", b"a1")],
            )
            .expect("append A1");
        store
            .append(
                stream_c,
                ExpectedVersion::NoStream,
                vec![make_proposed("C0", b"c0")],
            )
            .expect("append C0");
        store
            .append(
                stream_b,
                ExpectedVersion::Exact(0),
                vec![make_proposed("B1", b"b1")],
            )
            .expect("append B1");

        // read_all: all 5 in global position order.
        let all = store.read_all(0, 100);
        assert_eq!(all.len(), 5);
        for (i, event) in all.iter().enumerate() {
            assert_eq!(event.global_position, i as u64, "global order at index {i}");
        }
        assert_eq!(all[0].event_type, "A0");
        assert_eq!(all[1].event_type, "B0");
        assert_eq!(all[2].event_type, "A1");
        assert_eq!(all[3].event_type, "C0");
        assert_eq!(all[4].event_type, "B1");

        // read_stream for A: versions 0, 1
        let a_events = store.read_stream(stream_a, 0, 100).expect("read_stream A");
        assert_eq!(a_events.len(), 2);
        assert_eq!(a_events[0].stream_version, 0);
        assert_eq!(a_events[0].event_type, "A0");
        assert_eq!(a_events[1].stream_version, 1);
        assert_eq!(a_events[1].event_type, "A1");

        // read_stream for B: versions 0, 1
        let b_events = store.read_stream(stream_b, 0, 100).expect("read_stream B");
        assert_eq!(b_events.len(), 2);
        assert_eq!(b_events[0].stream_version, 0);
        assert_eq!(b_events[0].event_type, "B0");
        assert_eq!(b_events[1].stream_version, 1);
        assert_eq!(b_events[1].event_type, "B1");

        // read_stream for C: version 0
        let c_events = store.read_stream(stream_c, 0, 100).expect("read_stream C");
        assert_eq!(c_events.len(), 1);
        assert_eq!(c_events[0].stream_version, 0);
        assert_eq!(c_events[0].event_type, "C0");
    }

    // -- AC-13d: read_all(0, 100) on empty log -> empty Vec.
    #[test]
    fn read_all_empty_log_returns_empty() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let store = Store::open(&path).expect("open should succeed");
        let events = store.read_all(0, 100);

        assert!(events.is_empty(), "expected empty vec from empty log");
    }

    // -- AC-13a: read_all(0, 100) on 5-event log -> all 5.
    #[test]
    fn read_all_returns_all_events() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");
        let (_stream_id, store) = open_and_append_to_stream(&path, 5);

        let events = store.read_all(0, 100);

        assert_eq!(events.len(), 5);
        for (i, event) in events.iter().enumerate() {
            assert_eq!(event.global_position, i as u64);
        }
    }

    // -- AC-13b: read_all(3, 2) on 5-event log -> events at positions 3 and 4.
    #[test]
    fn read_all_partial_range() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");
        let (_stream_id, store) = open_and_append_to_stream(&path, 5);

        let events = store.read_all(3, 2);

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].global_position, 3);
        assert_eq!(events[1].global_position, 4);
    }

    // -- AC-13c: read_all(100, 10) on 5-event log -> empty Vec.
    #[test]
    fn read_all_beyond_end_returns_empty() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");
        let (_stream_id, store) = open_and_append_to_stream(&path, 5);

        let events = store.read_all(100, 10);

        assert!(
            events.is_empty(),
            "expected empty vec for out-of-range position"
        );
    }

    // -- AC-1: Opening an existing file with bad magic returns InvalidHeader.
    #[test]
    fn recovery_rejects_invalid_header_magic() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        // Write a file with wrong magic bytes.
        std::fs::write(&path, [0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00])
            .expect("write file");

        match Store::open(&path) {
            Err(Error::InvalidHeader(msg)) => {
                assert!(
                    msg.contains("magic"),
                    "error should mention 'magic', got: {msg}"
                );
            }
            Err(other) => panic!("expected InvalidHeader, got: {other:?}"),
            Ok(_) => panic!("expected InvalidHeader, but open() succeeded"),
        }
    }

    // -- AC-1: Opening an existing file with wrong version returns InvalidHeader.
    #[test]
    fn recovery_rejects_invalid_header_version() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        // Correct magic, wrong version (99).
        let mut header = [0u8; 8];
        header[0..4].copy_from_slice(&[0x45, 0x46, 0x44, 0x42]);
        header[4..8].copy_from_slice(&99u32.to_le_bytes());
        std::fs::write(&path, header).expect("write file");

        match Store::open(&path) {
            Err(Error::InvalidHeader(msg)) => {
                assert!(
                    msg.contains("version"),
                    "error should mention 'version', got: {msg}"
                );
            }
            Err(other) => panic!("expected InvalidHeader, got: {other:?}"),
            Ok(_) => panic!("expected InvalidHeader, but open() succeeded"),
        }
    }

    /// Helper: open a store at the given path and append `count` events to
    /// a single stream, returning the stream UUID and the store.
    fn open_and_append_to_stream(path: &std::path::Path, count: usize) -> (Uuid, Store) {
        let mut store = Store::open(path).expect("open should succeed");
        let stream_id = Uuid::new_v4();
        let proposed: Vec<ProposedEvent> = (0..count)
            .map(|i| make_proposed(&format!("Evt{i}"), format!("p{i}").as_bytes()))
            .collect();
        store
            .append(stream_id, ExpectedVersion::NoStream, proposed)
            .expect("append should succeed");
        (stream_id, store)
    }

    // -- AC-12a: read_stream from version 0 with max_count 100 on 5-event stream -> all 5.
    #[test]
    fn read_stream_all_events_from_start() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");
        let (stream_id, store) = open_and_append_to_stream(&path, 5);

        let events = store
            .read_stream(stream_id, 0, 100)
            .expect("read_stream should succeed");

        assert_eq!(events.len(), 5);
        for (i, event) in events.iter().enumerate() {
            assert_eq!(event.stream_version, i as u64);
            assert_eq!(event.stream_id, stream_id);
        }
    }

    // -- AC-12b: read_stream from version 2 with max_count 2 on 5-event stream -> versions 2, 3.
    #[test]
    fn read_stream_partial_range() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");
        let (stream_id, store) = open_and_append_to_stream(&path, 5);

        let events = store
            .read_stream(stream_id, 2, 2)
            .expect("read_stream should succeed");

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].stream_version, 2);
        assert_eq!(events[1].stream_version, 3);
    }

    // -- AC-12c: read_stream from version 10 on 5-event stream -> empty Vec.
    #[test]
    fn read_stream_beyond_end_returns_empty() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");
        let (stream_id, store) = open_and_append_to_stream(&path, 5);

        let events = store
            .read_stream(stream_id, 10, 100)
            .expect("read_stream should succeed");

        assert!(
            events.is_empty(),
            "expected empty vec for out-of-range version"
        );
    }

    // -- AC-12d: read_stream on non-existent UUID -> Err(StreamNotFound).
    #[test]
    fn read_stream_nonexistent_returns_stream_not_found() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let store = Store::open(&path).expect("open should succeed");
        let unknown = Uuid::new_v4();

        match store.read_stream(unknown, 0, 100) {
            Err(Error::StreamNotFound { stream_id }) => {
                assert_eq!(stream_id, unknown);
            }
            Err(other) => panic!("expected StreamNotFound, got: {other:?}"),
            Ok(_) => panic!("expected StreamNotFound error, but read_stream succeeded"),
        }
    }

    // -- AC-2 integration: Open, append 5 events via Store::append() across 2 streams,
    // drop, reopen -- all 5 recovered, positions match, subsequent append continues correctly.
    #[test]
    fn recovery_via_append_5_events_across_2_streams() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let stream_a = Uuid::new_v4();
        let stream_b = Uuid::new_v4();

        // Phase 1: Open store, append 5 events across 2 streams via Store::append().
        // Interleaved: A(0,1), B(0), A(2), B(1)
        let (event_ids, event_types) = {
            let mut store = Store::open(&path).expect("open should succeed");

            let batch_a1 = vec![make_proposed("A0", b"a0"), make_proposed("A1", b"a1")];
            let rec_a1 = store
                .append(stream_a, ExpectedVersion::NoStream, batch_a1)
                .expect("append A batch 1");

            let batch_b1 = vec![make_proposed("B0", b"b0")];
            let rec_b1 = store
                .append(stream_b, ExpectedVersion::NoStream, batch_b1)
                .expect("append B batch 1");

            let batch_a2 = vec![make_proposed("A2", b"a2")];
            let rec_a2 = store
                .append(stream_a, ExpectedVersion::Exact(1), batch_a2)
                .expect("append A batch 2");

            let batch_b2 = vec![make_proposed("B1", b"b1")];
            let rec_b2 = store
                .append(stream_b, ExpectedVersion::Exact(0), batch_b2)
                .expect("append B batch 2");

            assert_eq!(store.global_position(), 5);

            // Collect event IDs and types for verification after recovery.
            let mut ids = Vec::new();
            let mut types = Vec::new();
            for rec in rec_a1
                .iter()
                .chain(rec_b1.iter())
                .chain(rec_a2.iter())
                .chain(rec_b2.iter())
            {
                ids.push(rec.event_id);
                types.push(rec.event_type.clone());
            }
            (ids, types)
            // `store` dropped here -- file handle closed.
        };

        // Phase 2: Reopen and verify recovery.
        let mut store = Store::open(&path).expect("reopen should succeed");

        assert_eq!(store.global_position(), 5);
        assert_eq!(store.stream_version(&stream_a), Some(2));
        assert_eq!(store.stream_version(&stream_b), Some(1));

        // Verify all 5 recovered events match by event_id and event_type.
        let all = store.read_all(0, 100);
        assert_eq!(all.len(), 5);
        for (i, event) in all.iter().enumerate() {
            assert_eq!(event.global_position, i as u64, "global_position at {i}");
            assert_eq!(event.event_id, event_ids[i], "event_id at {i}");
            assert_eq!(event.event_type, event_types[i], "event_type at {i}");
        }

        // Verify stream reads return correct events.
        let a_events = store.read_stream(stream_a, 0, 100).expect("read_stream A");
        assert_eq!(a_events.len(), 3);
        assert_eq!(a_events[0].stream_version, 0);
        assert_eq!(a_events[1].stream_version, 1);
        assert_eq!(a_events[2].stream_version, 2);

        let b_events = store.read_stream(stream_b, 0, 100).expect("read_stream B");
        assert_eq!(b_events.len(), 2);
        assert_eq!(b_events[0].stream_version, 0);
        assert_eq!(b_events[1].stream_version, 1);

        // Phase 3: Subsequent append continues from correct positions.
        let next = vec![make_proposed("A3", b"a3")];
        let recorded = store
            .append(stream_a, ExpectedVersion::Exact(2), next)
            .expect("post-recovery append should succeed");

        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].global_position, 5);
        assert_eq!(recorded[0].stream_version, 3);
        assert_eq!(store.global_position(), 6);
    }

    // -- AC-3 integration: Append 3 events via Store::append(), close, append 10 garbage
    // bytes, reopen -- 3 events recovered, garbage truncated, next append at position 3.
    #[test]
    fn recovery_via_append_truncates_garbage_after_real_appends() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let stream_id = Uuid::new_v4();

        // Phase 1: Open store, append 3 events via Store::append().
        {
            let mut store = Store::open(&path).expect("open should succeed");
            let proposed = vec![
                make_proposed("Evt0", b"p0"),
                make_proposed("Evt1", b"p1"),
                make_proposed("Evt2", b"p2"),
            ];
            store
                .append(stream_id, ExpectedVersion::NoStream, proposed)
                .expect("append should succeed");
            assert_eq!(store.global_position(), 3);
            // `store` dropped here -- file handle closed.
        }

        // Record the valid file size before corruption.
        let valid_size = std::fs::metadata(&path).expect("metadata").len();

        // Phase 2: Append 10 garbage bytes to simulate a crash mid-write.
        {
            use std::io::Write;
            let mut file = OpenOptions::new()
                .append(true)
                .open(&path)
                .expect("open for append");
            file.write_all(&[0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0x01, 0x02, 0x03, 0x04])
                .expect("write garbage");
            file.sync_all().expect("sync");
        }

        // File should now be larger.
        assert!(std::fs::metadata(&path).expect("metadata").len() > valid_size);

        // Phase 3: Reopen -- should recover exactly 3 events and truncate garbage.
        let mut store = Store::open(&path).expect("reopen should succeed");

        assert_eq!(store.global_position(), 3);
        assert_eq!(store.stream_version(&stream_id), Some(2));

        // File should be truncated back to valid boundary.
        let final_size = std::fs::metadata(&path).expect("metadata").len();
        assert_eq!(
            final_size, valid_size,
            "file should be truncated to valid boundary"
        );

        // Verify recovered events.
        let all = store.read_all(0, 100);
        assert_eq!(all.len(), 3);
        for (i, event) in all.iter().enumerate() {
            assert_eq!(event.global_position, i as u64);
            assert_eq!(event.stream_version, i as u64);
            assert_eq!(event.stream_id, stream_id);
        }

        // Phase 4: Subsequent append succeeds at global_position = 3.
        let next = vec![make_proposed("Evt3", b"p3")];
        let recorded = store
            .append(stream_id, ExpectedVersion::Exact(2), next)
            .expect("post-recovery append should succeed");

        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].global_position, 3);
        assert_eq!(recorded[0].stream_version, 3);
        assert_eq!(store.global_position(), 4);
    }

    // -- AC-1: Opening a file too short for the header returns InvalidHeader.
    #[test]
    fn recovery_rejects_file_too_short_for_header() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        // Write only 4 bytes (less than the 8-byte header).
        std::fs::write(&path, [0x45, 0x46, 0x44, 0x42]).expect("write file");

        match Store::open(&path) {
            Err(Error::InvalidHeader(msg)) => {
                assert!(
                    msg.contains("short"),
                    "error should mention 'short', got: {msg}"
                );
            }
            Err(other) => panic!("expected InvalidHeader, got: {other:?}"),
            Ok(_) => panic!("expected InvalidHeader, but open() succeeded"),
        }
    }
}
